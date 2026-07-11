// SPDX-License-Identifier: GPL-3.0-only
//! Provisioning session: frame-level I/O + high-level provisioning commands.
//!
//! `Session<T>` owns a `Transport` and exposes one method per provisioning
//! operation.  All methods are synchronous (block until response or timeout).
//!
//! Frame format reference: `docs/adr/0002-provisioning-wire-format.md`.

use std::time::{Duration, Instant};

use anyhow::Context;

use protocol::provisioning::{
    ProvError, RspChannelPayload, RspContactPayload, RspStatusPayload,
    // frame synchronisation
    PROV_MAGIC,
    // frame-type constants
    FRAME_ADD_CHANNEL, FRAME_ADD_CONTACT, FRAME_CLEAR_HISTORY, FRAME_COMMIT_PROVISIONING,
    FRAME_DEL_CHANNEL, FRAME_DEL_CONTACT, FRAME_EXPORT_HISTORY, FRAME_QUERY_CHANNELS,
    FRAME_QUERY_CONTACTS, FRAME_QUERY_STATUS, FRAME_RSP_CHANNEL, FRAME_RSP_CHANNELS_DONE,
    FRAME_RSP_CONTACT, FRAME_RSP_CONTACTS_DONE, FRAME_RSP_ERROR, FRAME_RSP_HISTORY_DONE,
    FRAME_RSP_HISTORY_ENTRY, FRAME_RSP_IDENTITY, FRAME_RSP_OK, FRAME_RSP_STATUS,
    FRAME_SET_DEVICE_NAME, FRAME_SET_NOTIF_DEFAULTS, FRAME_SET_PIN,
    // decode helpers
    decode_frame, decode_rsp_channel, decode_rsp_contact, decode_rsp_error, decode_rsp_identity,
    decode_rsp_status,
    // encode helpers
    encode_add_channel, encode_add_contact, encode_del_channel, encode_del_contact,
    encode_frame, encode_set_device_name, encode_set_notif_defaults, encode_set_pin,
};
use protocol::history::{HistoryEntry, MAX_RSP_HISTORY_ENTRY_PAYLOAD, decode_rsp_history_entry};

use crate::transport::Transport;

// ── Session ───────────────────────────────────────────────────────────────────

/// Provisioning session over a `Transport`.
pub struct Session<T: Transport> {
    transport: T,
    /// How long to wait for a complete response frame before giving up on one
    /// retry attempt.  Overridden to `retry_attempt_ms` inside
    /// `send_recv_with_retry` for the duration of the retry loop.
    frame_timeout: Duration,
    /// Persistent byte accumulation buffer.
    ///
    /// Leftover bytes from a previous `recv_frame` call (e.g. when the
    /// transport delivers multiple frames in a burst) are kept here so the
    /// next call picks them up without a round-trip to the transport.
    acc_buf: Vec<u8>,
    /// Per-attempt receive timeout used by `send_recv_with_retry` (ms).
    /// Default: 500 ms.  Lower values are useful in tests to keep the
    /// dropped-frame test fast.
    retry_attempt_ms: u64,
    /// Overall retry deadline used by `send_recv_with_retry` (ms).
    /// Default: 10 000 ms (10 s).
    retry_total_ms: u64,
    /// Device display name from the most recent `RSP_IDENTITY` frame (always
    /// received as the second frame of a `query_status()` round-trip).
    /// `None` if no name is persisted on the device, or before the first
    /// `query_status()` call in this session.
    last_device_name: Option<String>,
}

impl<T: Transport> Session<T> {
    /// Create a session with a 5-second per-frame timeout and default retry
    /// parameters (500 ms per attempt, 10 s overall).
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            frame_timeout: Duration::from_secs(5),
            acc_buf: Vec::new(),
            retry_attempt_ms: 500,
            retry_total_ms: 10_000,
            last_device_name: None,
        }
    }

    /// Create a session with a custom per-frame timeout (useful in tests).
    ///
    /// Retry parameters default to 500 ms / 10 s; use [`Session::with_retry_params`]
    /// to override them as well.
    pub fn with_timeout(transport: T, secs: u64) -> Self {
        Self {
            transport,
            frame_timeout: Duration::from_secs(secs),
            acc_buf: Vec::new(),
            retry_attempt_ms: 500,
            retry_total_ms: 10_000,
            last_device_name: None,
        }
    }

    /// Create a session with fully custom timeout and retry parameters.
    ///
    /// Primarily for testing: production code should use [`Session::new`].
    /// Setting `retry_attempt_ms` to a small value (e.g. 50) keeps the
    /// dropped-frame test fast.
    pub fn with_retry_params(
        transport: T,
        frame_secs: u64,
        retry_attempt_ms: u64,
        retry_total_ms: u64,
    ) -> Self {
        Self {
            transport,
            frame_timeout: Duration::from_secs(frame_secs),
            acc_buf: Vec::new(),
            retry_attempt_ms,
            retry_total_ms,
            last_device_name: None,
        }
    }

    // ── Low-level frame I/O ───────────────────────────────────────────────────

    fn send_frame(&mut self, frame_type: u8, payload: &[u8]) -> anyhow::Result<()> {
        let mut buf = [0u8; 512];
        let n = encode_frame(frame_type, payload, &mut buf);
        self.transport.send(&buf[..n])
    }

    /// Accumulate bytes from the transport until a complete provisioning frame
    /// is available, then decode and return `(frame_type, payload_bytes)`.
    ///
    /// Uses `self.acc_buf` as a persistent accumulation buffer so that bytes
    /// delivered ahead of the current frame (e.g. in a streaming response) are
    /// preserved for the next call rather than silently discarded.
    ///
    /// Magic-byte synchronisation: the T-Deck firmware writes ESP-IDF log lines
    /// (boot banner, UNPROVISIONED notice, `prov_server: ready`, UI pump
    /// messages) to the same USB-serial port used for provisioning frames.
    /// `recv_frame` calls `find_magic_start` to discard any leading non-frame
    /// bytes before attempting to parse, mirroring `find_magic_start` in
    /// `firmware/src/provisioning_server.rs`.
    ///
    /// Invariants:
    /// - Returns `Err` if no complete frame arrives within `frame_timeout`.
    /// - On `CrcMismatch` or `BadMagic` from `decode_frame` (false `PROV_MAGIC`
    ///   sequence in log traffic), advances 1 byte and re-scans.
    fn recv_frame(&mut self) -> anyhow::Result<(u8, Vec<u8>)> {
        let deadline = Instant::now() + self.frame_timeout;

        loop {
            if Instant::now() > deadline {
                anyhow::bail!(
                    "timeout waiting for response frame (accumulated {} bytes)",
                    self.acc_buf.len()
                );
            }

            // Pull more bytes from the transport into the persistent buffer.
            let mut tmp = [0u8; 256];
            let n = self
                .transport
                .recv(&mut tmp)
                .context("transport recv error")?;
            self.acc_buf.extend_from_slice(&tmp[..n]);

            // Discard all bytes that precede a PROV_MAGIC sequence.  The device
            // writes ESP-IDF log lines on the same USB-serial before (and
            // sometimes between) provisioning frames.  This mirrors
            // `find_magic_start` in `firmware/src/provisioning_server.rs`.
            let sync = find_magic_start(&self.acc_buf);
            if sync > 0 {
                self.acc_buf.drain(..sync);
            }

            // Frame layout: magic(2) + type(1) + len_lo(1) + len_hi(1) + payload(N) + crc(2)
            // We need at least 5 bytes to determine payload_len.
            if self.acc_buf.len() >= 5 {
                let plen = (self.acc_buf[3] as usize) | ((self.acc_buf[4] as usize) << 8);
                // Guard against false PROV_MAGIC in log traffic.  All valid
                // protocol payloads fit within MAX_RSP_HISTORY_ENTRY_PAYLOAD
                // (72 bytes); a larger plen means the "MC" at acc_buf[0..2]
                // was ASCII log noise, not a real frame header.  Advance 1
                // byte so find_magic_start can re-scan on the next iteration.
                if plen > MAX_RSP_HISTORY_ENTRY_PAYLOAD {
                    self.acc_buf.drain(..1);
                    continue;
                }
                let total = 7 + plen;
                if self.acc_buf.len() >= total {
                    match decode_frame(&self.acc_buf[..total]) {
                        Ok((ft, payload_slice)) => {
                            let result = (ft, payload_slice.to_vec());
                            // Consume only the current frame; leave remaining bytes
                            // for the next call.
                            self.acc_buf.drain(..total);
                            return Ok(result);
                        }
                        Err(ProvError::CrcMismatch) | Err(ProvError::BadMagic) => {
                            // False PROV_MAGIC sequence in log traffic: advance
                            // 1 byte past the fake magic and re-scan on the next
                            // iteration.
                            self.acc_buf.drain(..1);
                        }
                        Err(e) => {
                            anyhow::bail!("frame decode error: {:?}", e);
                        }
                    }
                }
            }

            // No new data: yield briefly to avoid hot-spinning on a real port.
            if n == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Send a command frame and wait for the response, retrying the send every
    /// `self.retry_attempt_ms` milliseconds until a valid response frame
    /// arrives or `self.retry_total_ms` milliseconds have elapsed.
    ///
    /// Belt-and-suspenders against timing races: even with the DTR/RTS fix in
    /// `SerialTransport::open()`, a kernel modem-line glitch or a marginal USB
    /// cable can drop the first command frame.  Retrying every 500 ms
    /// (production default) with a 10 s overall deadline means any single lost
    /// frame self-heals without user intervention.
    ///
    /// Between retry attempts the persistent `acc_buf` is cleared so that log
    /// bytes accumulated during any device-side processing delay do not confuse
    /// the next `recv_frame` call.
    fn send_recv_with_retry(
        &mut self,
        frame_type: u8,
        payload: &[u8],
    ) -> anyhow::Result<(u8, Vec<u8>)> {
        let deadline = Instant::now() + Duration::from_millis(self.retry_total_ms);
        let attempt_timeout = Duration::from_millis(self.retry_attempt_ms);
        let saved_timeout = self.frame_timeout;
        self.frame_timeout = attempt_timeout;

        loop {
            if let Err(e) = self.send_frame(frame_type, payload) {
                self.frame_timeout = saved_timeout;
                return Err(e);
            }
            match self.recv_frame() {
                Ok(frame) => {
                    self.frame_timeout = saved_timeout;
                    return Ok(frame);
                }
                Err(_) if Instant::now() < deadline => {
                    // This attempt timed out but we still have overall budget.
                    // Clear the Session-level accumulation buffer AND the OS-level
                    // receive buffer before retrying.  Without the OS flush, a
                    // late reply from the previous send (arriving after the
                    // retry_attempt_ms deadline) would sit in the kernel buffer
                    // and be consumed as the reply to the re-sent command,
                    // causing command/response desync.
                    self.acc_buf.clear();
                    if let Err(e) = self.transport.flush_input() {
                        self.frame_timeout = saved_timeout;
                        return Err(e);
                    }
                    // Loop → send again.
                }
                Err(e) => {
                    self.frame_timeout = saved_timeout;
                    return Err(e);
                }
            }
        }
    }

    /// Send a command frame and assert the response is `RSP_OK`.
    fn send_and_expect_ok(&mut self, frame_type: u8, payload: &[u8]) -> anyhow::Result<()> {
        let (ft, rsp_payload) = self.send_recv_with_retry(frame_type, payload)?;
        match ft {
            FRAME_RSP_OK => Ok(()),
            FRAME_RSP_ERROR => {
                let e = decode_rsp_error(&rsp_payload)
                    .map_err(|de| anyhow::anyhow!("decode RSP_ERROR payload: {:?}", de))?;
                let msg = std::str::from_utf8(&e.msg[..e.msg_len as usize])
                    .unwrap_or("<invalid utf-8>");
                anyhow::bail!("device returned error {}: {}", e.error_code, msg)
            }
            _ => anyhow::bail!(
                "unexpected response frame 0x{:02X} (expected RSP_OK 0x{:02X})",
                ft,
                FRAME_RSP_OK,
            ),
        }
    }

    // ── High-level provisioning API ───────────────────────────────────────────

    /// Query the device's provisioning status and identity.
    ///
    /// The firmware sends TWO response frames for `QUERY_STATUS`: first
    /// `FRAME_RSP_STATUS` (0x82) then `FRAME_RSP_IDENTITY` (0x83).  This
    /// method consumes both so that `FRAME_RSP_IDENTITY` does not accumulate
    /// in the receive buffer and cause command/response desync on the next
    /// operation.
    pub fn query_status(&mut self) -> anyhow::Result<RspStatusPayload> {
        let (ft, payload) = self.send_recv_with_retry(FRAME_QUERY_STATUS, &[])?;
        let status = match ft {
            FRAME_RSP_STATUS => decode_rsp_status(&payload)
                .map_err(|e| anyhow::anyhow!("decode RSP_STATUS payload: {:?}", e))?,
            FRAME_RSP_ERROR => {
                let e = decode_rsp_error(&payload)
                    .map_err(|de| anyhow::anyhow!("decode RSP_ERROR payload: {:?}", de))?;
                let msg = std::str::from_utf8(&e.msg[..e.msg_len as usize])
                    .unwrap_or("<invalid utf-8>");
                anyhow::bail!("device error {}: {}", e.error_code, msg)
            }
            _ => anyhow::bail!("unexpected response 0x{:02X} to QUERY_STATUS", ft),
        };
        // Consume the trailing RSP_IDENTITY frame that the firmware always sends
        // after RSP_STATUS.  Leaving it in the buffer would desync the next command.
        // Decode it (rather than discard) to capture the persisted device name —
        // `last_device_name()` exposes it after this call.
        let (ft2, id_payload) = self.recv_frame()
            .map_err(|e| anyhow::anyhow!("timeout waiting for RSP_IDENTITY after RSP_STATUS: {}", e))?;
        if ft2 != FRAME_RSP_IDENTITY {
            anyhow::bail!(
                "expected RSP_IDENTITY (0x{:02X}) after RSP_STATUS; got 0x{:02X}",
                FRAME_RSP_IDENTITY,
                ft2
            );
        }
        let identity = decode_rsp_identity(&id_payload)
            .map_err(|e| anyhow::anyhow!("decode RSP_IDENTITY payload: {:?}", e))?;
        self.last_device_name = if identity.device_name_len > 0 {
            Some(
                String::from_utf8_lossy(&identity.device_name[..identity.device_name_len as usize])
                    .into_owned(),
            )
        } else {
            None
        };
        Ok(status)
    }

    /// The device display name captured by the most recent [`Session::query_status`]
    /// call.  `None` if no name is persisted on the device, or if `query_status`
    /// has not yet been called this session.
    pub fn last_device_name(&self) -> Option<&str> {
        self.last_device_name.as_deref()
    }

    /// Enumerate the device's configured contacts.
    ///
    /// Sends `FRAME_QUERY_CONTACTS`, then receives a stream of
    /// `FRAME_RSP_CONTACT` frames terminated by `FRAME_RSP_CONTACTS_DONE`.
    /// Returns the entries in device-index order.
    ///
    /// Served by the firmware's provisioning server against the in-progress
    /// (pre-commit) staging config — pair it with `add-contact` / `del-contact`
    /// / `status` to verify the configured set before `commit`.
    pub fn list_contacts(&mut self) -> anyhow::Result<Vec<RspContactPayload>> {
        let (mut ft, mut payload) =
            self.send_recv_with_retry(FRAME_QUERY_CONTACTS, &[])?;
        let mut entries: Vec<RspContactPayload> = Vec::new();
        loop {
            match ft {
                FRAME_RSP_CONTACT => {
                    let c = decode_rsp_contact(&payload)
                        .map_err(|e| anyhow::anyhow!("decode RSP_CONTACT: {:?}", e))?;
                    entries.push(c);
                }
                FRAME_RSP_CONTACTS_DONE => break,
                FRAME_RSP_ERROR => {
                    let e = decode_rsp_error(&payload)
                        .map_err(|de| anyhow::anyhow!("decode RSP_ERROR: {:?}", de))?;
                    let msg = std::str::from_utf8(&e.msg[..e.msg_len as usize])
                        .unwrap_or("<invalid utf-8>");
                    anyhow::bail!("device error {}: {}", e.error_code, msg)
                }
                other => anyhow::bail!(
                    "unexpected frame 0x{:02X} during contact enumeration", other
                ),
            }
            (ft, payload) = self.recv_frame()?;
        }
        Ok(entries)
    }

    /// Enumerate the device's configured channels.
    ///
    /// Sends `FRAME_QUERY_CHANNELS`, then receives a stream of
    /// `FRAME_RSP_CHANNEL` frames terminated by `FRAME_RSP_CHANNELS_DONE`.
    /// Returns the entries in device-index order.
    pub fn list_channels(&mut self) -> anyhow::Result<Vec<RspChannelPayload>> {
        let (mut ft, mut payload) =
            self.send_recv_with_retry(FRAME_QUERY_CHANNELS, &[])?;
        let mut entries: Vec<RspChannelPayload> = Vec::new();
        loop {
            match ft {
                FRAME_RSP_CHANNEL => {
                    let ch = decode_rsp_channel(&payload)
                        .map_err(|e| anyhow::anyhow!("decode RSP_CHANNEL: {:?}", e))?;
                    entries.push(ch);
                }
                FRAME_RSP_CHANNELS_DONE => break,
                FRAME_RSP_ERROR => {
                    let e = decode_rsp_error(&payload)
                        .map_err(|de| anyhow::anyhow!("decode RSP_ERROR: {:?}", de))?;
                    let msg = std::str::from_utf8(&e.msg[..e.msg_len as usize])
                        .unwrap_or("<invalid utf-8>");
                    anyhow::bail!("device error {}: {}", e.error_code, msg)
                }
                other => anyhow::bail!(
                    "unexpected frame 0x{:02X} during channel enumeration", other
                ),
            }
            (ft, payload) = self.recv_frame()?;
        }
        Ok(entries)
    }

    /// Add a contact.  `name` is a UTF-8 display name (empty = use pub_hash as label).
    pub fn add_contact(
        &mut self,
        pubkey: &[u8; 32],
        telemetry_enable: bool,
        name: &[u8],
    ) -> anyhow::Result<()> {
        let mut buf = [0u8; 80];
        let plen = encode_add_contact(pubkey, telemetry_enable, name, &mut buf);
        self.send_and_expect_ok(FRAME_ADD_CONTACT, &buf[..plen])
    }

    /// Delete a contact by its Ed25519 public key.
    pub fn del_contact(&mut self, pubkey: &[u8; 32]) -> anyhow::Result<()> {
        let mut buf = [0u8; 32];
        let plen = encode_del_contact(pubkey, &mut buf);
        self.send_and_expect_ok(FRAME_DEL_CONTACT, &buf[..plen])
    }

    /// Add (or replace) a channel.
    ///
    /// `key_len` must be 16 (128-bit channel) or 32 (256-bit channel).
    /// For 128-bit channels, `secret[16..32]` must be zero-padded on the
    /// caller side; the firmware uses `key_len` to select the correct
    /// channel-hash computation.
    pub fn add_channel(
        &mut self,
        secret: &[u8; 32],
        key_len: u8,
        primary: bool,
        name: &[u8],
    ) -> anyhow::Result<()> {
        let mut buf = [0u8; 80];
        let plen = encode_add_channel(secret, key_len, primary, name, &mut buf);
        self.send_and_expect_ok(FRAME_ADD_CHANNEL, &buf[..plen])
    }

    /// Delete a channel by its 32-byte secret.
    pub fn del_channel(&mut self, secret: &[u8; 32]) -> anyhow::Result<()> {
        let mut buf = [0u8; 32];
        let plen = encode_del_channel(secret, &mut buf);
        self.send_and_expect_ok(FRAME_DEL_CHANNEL, &buf[..plen])
    }

    /// Set notification defaults.
    pub fn set_notif_defaults(&mut self, visual: bool, audible: bool) -> anyhow::Result<()> {
        let mut buf = [0u8; 2];
        let plen = encode_set_notif_defaults(visual, audible, &mut buf);
        self.send_and_expect_ok(FRAME_SET_NOTIF_DEFAULTS, &buf[..plen])
    }

    /// Set (or reset) the admin PIN.  `pin` must be ≤ `MAX_PIN_LEN` bytes.
    pub fn set_pin(&mut self, pin: &[u8]) -> anyhow::Result<()> {
        let mut buf = [0u8; 20];
        let plen = encode_set_pin(pin, &mut buf);
        self.send_and_expect_ok(FRAME_SET_PIN, &buf[..plen])
    }

    /// Set (or clear, with an empty slice) the device display name.
    ///
    /// Persists to the device's identity store (NVS) immediately — survives a
    /// reboot the same way the Ed25519 identity itself does — regardless of
    /// whether the device has completed first-boot provisioning yet.
    /// `name` must be ≤ `MAX_NAME_LEN` (32) bytes.
    pub fn set_device_name(&mut self, name: &[u8]) -> anyhow::Result<()> {
        let mut buf = [0u8; 40];
        let plen = encode_set_device_name(name, &mut buf);
        self.send_and_expect_ok(FRAME_SET_DEVICE_NAME, &buf[..plen])
    }

    /// Commit provisioning: persist config to flash.
    ///
    /// On a first-boot device the firmware reboots into the mesh after committing
    /// (closing the USB-serial connection).  On an already-provisioned device the
    /// runtime handler re-persists live config and replies RSP_OK without rebooting.
    pub fn commit(&mut self) -> anyhow::Result<()> {
        self.send_and_expect_ok(FRAME_COMMIT_PROVISIONING, &[])
    }

    /// Clear ALL persisted conversation history on the device: every DM
    /// contact and channel conversation, both sent and received entries.
    ///
    /// Sends `FRAME_CLEAR_HISTORY` (empty payload) and expects `RSP_OK`. The
    /// flash-backed store is erased immediately, but the device's live
    /// in-memory UI state is not (see `docs/adr/0002-provisioning-wire-format.md`'s
    /// `CLEAR_HISTORY` amendment for the reboot-required design decision) — the
    /// caller (the CLI's `clear-history` command) is responsible for telling
    /// the user a reboot is needed before the cleared state shows on screen.
    pub fn clear_history(&mut self) -> anyhow::Result<()> {
        self.send_and_expect_ok(FRAME_CLEAR_HISTORY, &[])
    }

    /// Export conversation history from the device.
    ///
    /// Sends `FRAME_EXPORT_HISTORY`, then receives a stream of
    /// `FRAME_RSP_HISTORY_ENTRY` frames terminated by `FRAME_RSP_HISTORY_DONE`.
    ///
    /// Returns `(entry, is_ours)` pairs in oldest-first order — `is_ours` is
    /// `true` for a message this device sent, `false` for one it received
    /// (`sender_hash`
    /// alone cannot distinguish direction, since it is always the
    /// conversation hash for both). Returns an error if the device responds
    /// with `RSP_ERROR` or if a frame-level error occurs.
    ///
    /// Tolerates a bounded number of *stray* well-formed replies to an
    /// unrelated command (`RSP_STATUS`, `RSP_IDENTITY`, `RSP_OK`,
    /// `RSP_CONTACT[S_DONE]`, `RSP_CHANNEL[S_DONE]`) arriving before the real
    /// history stream. `SerialTransport::open` clears the kernel receive buffer exactly once,
    /// at connection time; that clear races against bytes from an *earlier*
    /// CLI invocation's command that are still in flight over USB at the
    /// instant this process opens the port (e.g. a `status` run whose own
    /// `send_recv_with_retry` retried after the dropped-first-frame timing
    /// glitch documented on that method — the device answers both the
    /// original and the retried request, and `query_status()` only ever
    /// reads one `RSP_STATUS`/`RSP_IDENTITY` pair, leaving the other
    /// trailing, unread, in the OS buffer when that earlier process exits).
    /// A stray reply to another command is harmless leftover noise here, not
    /// evidence of a corrupted stream — skip it and keep waiting for the
    /// real `HISTORY_ENTRY`/`HISTORY_DONE` frames. The skip count is bounded
    /// so a device that is genuinely stuck (or a truly corrupted stream)
    /// still surfaces as an error rather than hanging or looping forever.
    pub fn export_history(&mut self) -> anyhow::Result<Vec<(HistoryEntry, bool)>> {
        // Use the retry wrapper for the initial command so a timing race is
        // healed before the streaming response begins.  The first frame from
        // the device is HISTORY_ENTRY, HISTORY_DONE, or RSP_ERROR — all
        // handled inside the loop below.
        let (mut ft, mut payload) =
            self.send_recv_with_retry(FRAME_EXPORT_HISTORY, &[])?;

        let mut entries: Vec<(HistoryEntry, bool)> = Vec::new();
        let mut stray_frames = 0u32;
        const MAX_STRAY_FRAMES: u32 = 64;

        // Receive the streamed response.  The persistent acc_buf in recv_frame
        // ensures that when the transport delivers multiple frames in one read
        // (e.g. HISTORY_ENTRY + HISTORY_DONE concatenated), none are lost.
        loop {
            match ft {
                FRAME_RSP_HISTORY_ENTRY => {
                    if let Some((_idx, entry, is_ours)) = decode_rsp_history_entry(&payload) {
                        entries.push((entry, is_ours));
                    } else {
                        anyhow::bail!("malformed RSP_HISTORY_ENTRY payload");
                    }
                }
                FRAME_RSP_HISTORY_DONE => {
                    // End of stream.
                    break;
                }
                FRAME_RSP_ERROR => {
                    let e = decode_rsp_error(&payload)
                        .map_err(|de| anyhow::anyhow!("decode RSP_ERROR: {:?}", de))?;
                    let msg = std::str::from_utf8(&e.msg[..e.msg_len as usize])
                        .unwrap_or("<invalid utf-8>");
                    anyhow::bail!("device error {}: {}", e.error_code, msg)
                }
                FRAME_RSP_STATUS | FRAME_RSP_IDENTITY | FRAME_RSP_OK | FRAME_RSP_CONTACT
                | FRAME_RSP_CONTACTS_DONE | FRAME_RSP_CHANNEL | FRAME_RSP_CHANNELS_DONE => {
                    stray_frames += 1;
                    if stray_frames > MAX_STRAY_FRAMES {
                        anyhow::bail!(
                            "too many stray non-history frames (last: 0x{:02X}) during \
                             history export — device may be stuck, or the stream is \
                             genuinely corrupted rather than carrying leftover replies",
                            ft
                        );
                    }
                    eprintln!(
                        "warning: ignoring stray frame 0x{:02X} during history export \
                         (leftover reply to an earlier command, not a corrupted stream)",
                        ft
                    );
                }
                other => {
                    anyhow::bail!(
                        "unexpected frame 0x{:02X} during history export", other
                    );
                }
            }
            // Receive the next streaming frame (no retry — the device is already
            // awake and streaming; a timeout here is a genuine protocol error).
            (ft, payload) = self.recv_frame()?;
        }

        Ok(entries)
    }
}

// ── Frame synchronisation ─────────────────────────────────────────────────────

/// Return the index of the first byte in `buf` that could be the start of a
/// `PROV_MAGIC` sequence.  Returns `buf.len()` if no candidate start is found.
///
/// Identical in logic to `find_magic_start` in
/// `firmware/src/provisioning_server.rs` — the host and device parsers share
/// the same resync strategy so that interleaved ASCII log traffic on the shared
/// USB-serial channel is transparent to both sides.
fn find_magic_start(buf: &[u8]) -> usize {
    let m0 = PROV_MAGIC[0];
    let m1 = PROV_MAGIC[1];
    for i in 0..buf.len() {
        if buf[i] == m0 {
            if i + 1 < buf.len() {
                if buf[i + 1] == m1 {
                    return i;
                }
                // m0 not followed by m1 — keep scanning.
            } else {
                // m0 at the end of the buffer — can't confirm or deny yet;
                // preserve it for the next recv iteration.
                return i;
            }
        }
    }
    buf.len() // No magic candidate found — discard the entire buffer.
}
