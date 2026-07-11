// SPDX-License-Identifier: GPL-3.0-only
//! Firmware-side USB-serial provisioning server.
//!
//! Runs exclusively in the UNPROVISIONED first-boot state.  Reads provisioning
//! frames directly from the USB-Serial-JTAG driver ring buffer (bypassing the
//! VFS line discipline that blocks binary frames lacking a newline terminator),
//! processes each command, and saves the resulting config to NVS via
//! [`config_store`] when a [`FRAME_COMMIT_PROVISIONING`] frame arrives.
//!
//! # Protocol
//!
//! All frames follow the `protocol::provisioning` wire format (ADR-0002).
//! The host CLI (`host/`) encodes frames; this module decodes them and
//! encodes the response frames it sends back.
//!
//! # Exit condition
//!
//! `run()` returns `Ok(())` after a successful `CommitProvisioning` — the
//! config has been persisted to NVS.  The caller (`main.rs`) then calls
//! `esp_restart()` so the device reboots cleanly into the provisioned state.
//!
//! # Logging vs. binary output
//!
//! Log messages (via `log::info!`) go to the ESP-IDF console (UART0 /
//! USB-JTAG).  Binary response frames are written to `stdout`.  On a single
//! USB-JTAG port these share the same byte stream, so the host CLI must sync
//! on the `PROV_MAGIC` bytes when reading responses — standard framing
//! synchronisation, identical to how radio receivers sync on preambles.

use std::io::Write;

use anyhow::anyhow;
use esp_idf_svc::nvs::{EspNvsPartition, NvsDefault};

use protocol::provisioning::{
    FRAME_ADD_CHANNEL, FRAME_ADD_CONTACT, FRAME_COMMIT_PROVISIONING, FRAME_DEL_CHANNEL,
    FRAME_DEL_CONTACT, FRAME_QUERY_CHANNELS, FRAME_QUERY_CONTACTS, FRAME_QUERY_STATUS,
    FRAME_RSP_CHANNEL, FRAME_RSP_CHANNELS_DONE, FRAME_RSP_CONTACT, FRAME_RSP_CONTACTS_DONE,
    FRAME_RSP_ERROR, FRAME_RSP_IDENTITY, FRAME_RSP_OK, FRAME_RSP_STATUS,
    FRAME_SET_DEVICE_NAME, FRAME_SET_NOTIF_DEFAULTS, FRAME_SET_PIN,
    FRAME_OVERHEAD, PROV_MAGIC,
    decode_frame, encode_frame,
    decode_add_contact, decode_del_contact,
    decode_add_channel, decode_del_channel,
    decode_set_device_name, decode_set_notif_defaults, decode_set_pin,
    encode_rsp_status, encode_rsp_identity, encode_rsp_error,
    encode_rsp_contact, encode_rsp_channel,
    RspStatusPayload,
};
use protocol::channel_hash_var;

use crate::config_store::{
    Channel, ChannelListFull, ChannelUpsert, Contact, NotifDefaults, ProvisionedConfig, RadioPreset,
    MAX_CHANNELS, MAX_CONTACTS, MAX_NAME_LEN, MAX_PIN_LEN,
};

/// Frame receive buffer.  Sized to hold the largest possible provisioning frame
/// plus headroom for partial reads.
const RX_BUF_LEN: usize = 512;

/// Application error codes sent in `RspError.error_code`.
mod err {
    pub const CONTACT_LIST_FULL: u8 = 0x01;
    pub const CHANNEL_LIST_FULL: u8 = 0x02;
    pub const CONTACT_NOT_FOUND: u8 = 0x03;
    pub const CHANNEL_NOT_FOUND: u8 = 0x04;
    pub const DECODE_ERROR: u8      = 0x05;
    pub const STORAGE_ERROR: u8     = 0x06;
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the provisioning server until `CommitProvisioning` is received and the
/// config has been written to NVS.
///
/// `identity_pubkey` is the device's own Ed25519 public key, returned in
/// `QueryStatus` / `RspIdentity` responses so the host CLI can confirm the target
/// device identity before provisioning.
pub fn run(
    nvs_partition: EspNvsPartition<NvsDefault>,
    identity_pubkey: &[u8; 32],
    #[cfg(feature = "diagnostics")]
    rx_counter: &std::sync::atomic::AtomicU32,
) -> anyhow::Result<()> {
    log::info!("prov_server: ready — waiting for provisioning frames on USB-serial");
    log::info!("prov_server: send QUERY_STATUS (0x01) to begin");

    let null_contact = Contact {
        pubkey: [0u8; 32],
        telemetry_enable: false,
        display_name: [0u8; MAX_NAME_LEN],
        display_name_len: 0,
    };
    let null_channel = Channel {
        secret: [0u8; 32],
        key_len: 32,
        primary: false,
        name: [0u8; MAX_NAME_LEN],
        name_len: 0,
    };

    let mut staging = ProvisionedConfig {
        contacts:       [null_contact; MAX_CONTACTS],
        contact_count:  0,
        channels:       [null_channel; MAX_CHANNELS],
        channel_count:  0,
        radio_preset:   RadioPreset::default(),
        notif_defaults: NotifDefaults::default(),
        pin:            [0u8; MAX_PIN_LEN],
        pin_len:        0,
        lock_flags:     0,
    };

    // Device display name lives in the identity store (`mc_id`/`name`), not
    // the provisioning config blob above — it applies immediately (like
    // SET_PIN/SET_NOTIF_DEFAULTS do NOT), independent of CommitProvisioning.
    // Load whatever is already persisted (empty on first boot) so QUERY_STATUS
    // reflects it, and keep it in sync in-thread as SET_DEVICE_NAME arrives.
    let (mut device_name, mut device_name_len) =
        crate::identity_store::load_name(nvs_partition.clone()).unwrap_or_else(|e| {
            log::warn!("prov_server: identity_store::load_name failed: {:?} — starting unnamed", e);
            ([0u8; MAX_NAME_LEN], 0)
        });

    let mut rx_buf  = [0u8; RX_BUF_LEN];
    let mut rx_len  = 0usize;
    let mut stdout_ = std::io::stdout();

    loop {
        // ── Read more bytes ──────────────────────────────────────────────────
        // Read directly from the USB-Serial-JTAG driver ring buffer, bypassing
        // the VFS line discipline.  std::io::stdin() holds bytes until a CR/LF
        // arrives; binary provisioning frames carry no newline, so they would
        // sit unread forever via the VFS path.  usb_serial_jtag_read_bytes()
        // returns however many bytes are in the ring buffer immediately
        // (ticks_to_wait = 0 → non-blocking); the delay below handles idle.
        if rx_len < RX_BUF_LEN {
            let slice = &mut rx_buf[rx_len..];
            // SAFETY: driver is installed before this thread is spawned (main.rs).
            let n_signed = unsafe {
                esp_idf_svc::sys::usb_serial_jtag_read_bytes(
                    slice.as_mut_ptr() as *mut core::ffi::c_void,
                    slice.len() as u32,
                    0, // non-blocking: return immediately; delay below handles idle
                )
            };
            if n_signed <= 0 {
                // No bytes available — yield and retry.
                esp_idf_hal::delay::FreeRtos::delay_ms(50);
                continue;
            }
            let n = n_signed as usize;
            rx_len += n;
            // Raw-byte hex log — compiled in only with --features diagnostics.
            // Logs the cumulative byte count and last ≤16 bytes before
            // find_magic_start so non-frame bytes are visible during bring-up.
            #[cfg(feature = "diagnostics")]
            {
                let total = rx_counter.fetch_add(
                    n as u32,
                    std::sync::atomic::Ordering::Relaxed,
                ) + n as u32;
                let tail_start = rx_len.saturating_sub(16);
                let tail = &rx_buf[tail_start..rx_len];
                let mut hex = [b'0'; 32];
                for (i, &b) in tail.iter().enumerate().take(16) {
                    let hi = b >> 4;
                    let lo = b & 0x0F;
                    hex[i * 2]     = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
                    hex[i * 2 + 1] = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
                }
                let hex_str = core::str::from_utf8(&hex[..tail.len() * 2]).unwrap_or("?");
                log::info!("prov_server: raw RX n={} total={} [{}]", n, total, hex_str);
            }
        }

        // ── Frame synchronisation ────────────────────────────────────────────
        // Discard bytes that cannot be the start of a valid frame.  ASCII log
        // traffic and espflash monitor banners appear here before the first
        // provisioning frame arrives.
        let sync = find_magic_start(&rx_buf[..rx_len]);
        if sync > 0 {
            rx_buf.copy_within(sync..rx_len, 0);
            rx_len -= sync;
            continue;
        }

        // ── Try to decode one frame ──────────────────────────────────────────
        match decode_frame(&rx_buf[..rx_len]) {
            Ok((frame_type, payload)) => {
                let payload_len = payload.len();
                let frame_len   = FRAME_OVERHEAD + payload_len;

                // Copy payload before shifting buffer.
                let mut payload_copy = [0u8; 256];
                let copy_len = payload_len.min(payload_copy.len());
                payload_copy[..copy_len].copy_from_slice(&payload[..copy_len]);

                // Consume the frame.
                rx_buf.copy_within(frame_len..rx_len, 0);
                rx_len -= frame_len;

                let done = process_frame(
                    frame_type,
                    &payload_copy[..copy_len],
                    &mut staging,
                    &nvs_partition,
                    identity_pubkey,
                    &mut device_name,
                    &mut device_name_len,
                    &mut stdout_,
                )?;
                if done {
                    log::info!("prov_server: provisioning committed — caller will reboot");
                    return Ok(());
                }
            }

            Err(protocol::provisioning::ProvError::TruncatedFrame) => {
                // Need more data.
                if rx_len >= RX_BUF_LEN {
                    log::warn!("prov_server: RX buffer full with no valid frame — flushing");
                    rx_len = 0;
                }
            }
            Err(protocol::provisioning::ProvError::BadMagic) => {
                // Shouldn't happen after find_magic_start, but be defensive.
                if rx_len > 0 {
                    rx_buf.copy_within(1..rx_len, 0);
                    rx_len -= 1;
                }
            }
            Err(e) => {
                log::warn!("prov_server: frame decode error: {:?}", e);
                if rx_len > 0 {
                    rx_buf.copy_within(1..rx_len, 0);
                    rx_len -= 1;
                }
            }
        }
    }
}

// ── Frame processor ───────────────────────────────────────────────────────────

/// Process one decoded provisioning command.  Returns `true` if
/// `CommitProvisioning` was processed successfully.
#[allow(clippy::too_many_arguments)]
fn process_frame(
    frame_type: u8,
    payload: &[u8],
    staging: &mut ProvisionedConfig,
    nvs_partition: &EspNvsPartition<NvsDefault>,
    identity_pubkey: &[u8; 32],
    device_name: &mut [u8; MAX_NAME_LEN],
    device_name_len: &mut u8,
    out: &mut impl Write,
) -> anyhow::Result<bool> {
    match frame_type {
        // ── QUERY_STATUS ─────────────────────────────────────────────────────
        FRAME_QUERY_STATUS => {
            log::info!("prov_server: QUERY_STATUS — contacts={} channels={}",
                staging.contact_count, staging.channel_count);
            // Send RspStatus. GPS hardware is not initialised until AFTER the
            // provisioning gate in main.rs (step 6, past this thread's
            // lifetime), so this server has no fix/clock-sync data to report —
            // always "never" (no fix, not synced), mirroring the on-device
            // GPS status view before first fix.
            let status = RspStatusPayload {
                provisioned:   false, // by definition (we are in the unprovisioned server)
                pubkey:        *identity_pubkey,
                contact_count: staging.contact_count,
                channel_count: staging.channel_count,
                gps_has_fix: false,
                gps_lat_e7: 0,
                gps_lon_e7: 0,
                gps_fix_age_secs: 0,
                gps_clock_synced: false,
                gps_clock_sync_age_secs: 0,
                // The battery ADC (like GPS) is not initialised until AFTER
                // the provisioning gate in main.rs — see the GPS comment
                // just above. Report "0%, not charging" rather than a reading
                // this server has no way to take.
                battery_percent: 0,
                battery_charging: false,
                battery_raw_mv: 0,
                battery_held_raw_mv: 0,
            };
            let mut pbuf = [0u8; 64];
            let plen = encode_rsp_status(&status, &mut pbuf);
            send_frame(out, FRAME_RSP_STATUS, &pbuf[..plen])?;
            // Also send RspIdentity for host CLI convenience.
            let mut ibuf = [0u8; 64];
            let ilen = encode_rsp_identity(
                identity_pubkey,
                &device_name[..*device_name_len as usize],
                &mut ibuf,
            );
            send_frame(out, FRAME_RSP_IDENTITY, &ibuf[..ilen])?;
        }

        // ── QUERY_CONTACTS ───────────────────────────────────────────────────
        // Stream every staged contact (index-ordered) as RSP_CONTACT frames,
        // terminated by RSP_CONTACTS_DONE — mirrors the history-export pattern.
        // Reads the live staging set so add/del are reflected immediately.
        FRAME_QUERY_CONTACTS => {
            let cnt = staging.contact_count as usize;
            log::info!("prov_server: QUERY_CONTACTS — {} contact(s)", cnt);
            for i in 0..cnt {
                let c = &staging.contacts[i];
                let name = &c.display_name[..c.display_name_len as usize];
                let mut pbuf = [0u8; 80];
                let plen = encode_rsp_contact(i as u8, &c.pubkey, c.telemetry_enable, name, &mut pbuf);
                send_frame(out, FRAME_RSP_CONTACT, &pbuf[..plen])?;
            }
            send_frame(out, FRAME_RSP_CONTACTS_DONE, &[])?;
        }

        // ── QUERY_CHANNELS ───────────────────────────────────────────────────
        FRAME_QUERY_CHANNELS => {
            let cnt = staging.channel_count as usize;
            log::info!("prov_server: QUERY_CHANNELS — {} channel(s)", cnt);
            for i in 0..cnt {
                let ch = &staging.channels[i];
                let hash = channel_hash_var(&ch.secret[..ch.key_len as usize]);
                let name = &ch.name[..ch.name_len as usize];
                let mut pbuf = [0u8; 80];
                let plen = encode_rsp_channel(i as u8, hash, ch.key_len, ch.primary, name, &mut pbuf);
                send_frame(out, FRAME_RSP_CHANNEL, &pbuf[..plen])?;
            }
            send_frame(out, FRAME_RSP_CHANNELS_DONE, &[])?;
        }

        // ── ADD_CONTACT ──────────────────────────────────────────────────────
        FRAME_ADD_CONTACT => {
            match decode_add_contact(payload) {
                Ok(c) => {
                    if staging.contact_count as usize >= MAX_CONTACTS {
                        return send_error(out, err::CONTACT_LIST_FULL, b"contact list full");
                    }
                    let i = staging.contact_count as usize;
                    staging.contacts[i] = Contact {
                        pubkey:            c.pubkey,
                        telemetry_enable:  c.telemetry_enable,
                        display_name:      c.display_name,
                        display_name_len:  c.display_name_len,
                    };
                    staging.contact_count += 1;
                    log::info!("prov_server: ADD_CONTACT pub_hash=0x{:02x} telemetry={} name_len={}",
                        c.pubkey[0], c.telemetry_enable, c.display_name_len);
                    send_ok(out)?;
                }
                Err(e) => {
                    log::warn!("prov_server: ADD_CONTACT decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"add_contact decode error");
                }
            }
        }

        // ── DEL_CONTACT ──────────────────────────────────────────────────────
        FRAME_DEL_CONTACT => {
            match decode_del_contact(payload) {
                Ok(d) => {
                    let cnt = staging.contact_count as usize;
                    match staging.contacts[..cnt].iter().position(|c| c.pubkey == d.pubkey) {
                        Some(idx) => {
                            for j in idx..cnt - 1 {
                                staging.contacts[j] = staging.contacts[j + 1];
                            }
                            staging.contact_count -= 1;
                            log::info!("prov_server: DEL_CONTACT pub_hash=0x{:02x}", d.pubkey[0]);
                            send_ok(out)?;
                        }
                        None => return send_error(out, err::CONTACT_NOT_FOUND, b"contact not found"),
                    }
                }
                Err(e) => {
                    log::warn!("prov_server: DEL_CONTACT decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"del_contact decode error");
                }
            }
        }

        // ── ADD_CHANNEL ──────────────────────────────────────────────────────
        // Idempotent UPSERT keyed on the channel secret — same shared helper as
        // the runtime admin_server, so re-adding a known key during first-boot
        // provisioning renames/refreshes in place instead of stacking
        // cryptographically-identical duplicates.  Enforces at-most-one-primary.
        FRAME_ADD_CHANNEL => {
            match decode_add_channel(payload) {
                Ok(ch) => {
                    let new_channel = Channel {
                        secret:   ch.secret,
                        key_len:  ch.key_len,
                        primary:  ch.primary,
                        name:     ch.name,
                        name_len: ch.name_len,
                    };
                    match staging.upsert_channel(new_channel) {
                        Ok(outcome) => {
                            log::info!(
                                "prov_server: ADD_CHANNEL ({}) secret[0]=0x{:02x} key_len={} primary={} name_len={}",
                                match outcome {
                                    ChannelUpsert::Updated => "updated",
                                    ChannelUpsert::Added => "added",
                                },
                                ch.secret[0], ch.key_len, ch.primary, ch.name_len
                            );
                            send_ok(out)?;
                        }
                        Err(ChannelListFull) => {
                            return send_error(out, err::CHANNEL_LIST_FULL, b"channel list full");
                        }
                    }
                }
                Err(e) => {
                    log::warn!("prov_server: ADD_CHANNEL decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"add_channel decode error");
                }
            }
        }

        // ── DEL_CHANNEL ──────────────────────────────────────────────────────
        FRAME_DEL_CHANNEL => {
            match decode_del_channel(payload) {
                Ok(d) => {
                    let cnt = staging.channel_count as usize;
                    match staging.channels[..cnt].iter().position(|ch| ch.secret == d.secret) {
                        Some(idx) => {
                            for j in idx..cnt - 1 {
                                staging.channels[j] = staging.channels[j + 1];
                            }
                            staging.channel_count -= 1;
                            log::info!("prov_server: DEL_CHANNEL secret[0]=0x{:02x}", d.secret[0]);
                            send_ok(out)?;
                        }
                        None => return send_error(out, err::CHANNEL_NOT_FOUND, b"channel not found"),
                    }
                }
                Err(e) => {
                    log::warn!("prov_server: DEL_CHANNEL decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"del_channel decode error");
                }
            }
        }

        // ── SET_NOTIF_DEFAULTS ───────────────────────────────────────────────
        FRAME_SET_NOTIF_DEFAULTS => {
            match decode_set_notif_defaults(payload) {
                Ok(n) => {
                    staging.notif_defaults = NotifDefaults { visual: n.visual, audible: n.audible };
                    log::info!("prov_server: SET_NOTIF_DEFAULTS visual={} audible={}",
                        n.visual, n.audible);
                    send_ok(out)?;
                }
                Err(e) => {
                    log::warn!("prov_server: SET_NOTIF_DEFAULTS decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"set_notif_defaults decode error");
                }
            }
        }

        // ── SET_PIN ──────────────────────────────────────────────────────────
        FRAME_SET_PIN => {
            match decode_set_pin(payload) {
                Ok(p) => {
                    staging.pin     = p.pin;
                    staging.pin_len = p.pin_len;
                    log::info!("prov_server: SET_PIN len={}", p.pin_len);
                    send_ok(out)?;
                }
                Err(e) => {
                    log::warn!("prov_server: SET_PIN decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"set_pin decode error");
                }
            }
        }

        // ── SET_DEVICE_NAME ──────────────────────────────────────────────────
        // Unlike SET_PIN/SET_NOTIF_DEFAULTS (staged, persisted only on
        // CommitProvisioning), the device name persists to the identity store
        // immediately — it is a property of the node's identity, not the mesh
        // provisioning config, and applies the same way before or after
        // first-boot provisioning completes.
        FRAME_SET_DEVICE_NAME => {
            match decode_set_device_name(payload) {
                Ok(n) => {
                    match crate::identity_store::set_name(nvs_partition.clone(), &n.name[..n.name_len as usize]) {
                        Ok(()) => {
                            *device_name = n.name;
                            *device_name_len = n.name_len;
                            log::info!("prov_server: SET_DEVICE_NAME len={}", n.name_len);
                            send_ok(out)?;
                        }
                        Err(e) => {
                            log::error!("prov_server: identity_store::set_name failed: {:?}", e);
                            return send_error(out, err::STORAGE_ERROR, b"NVS save failed");
                        }
                    }
                }
                Err(e) => {
                    log::warn!("prov_server: SET_DEVICE_NAME decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"set_device_name decode error");
                }
            }
        }

        // ── COMMIT_PROVISIONING ──────────────────────────────────────────────
        FRAME_COMMIT_PROVISIONING => {
            log::info!("prov_server: COMMIT_PROVISIONING — persisting to NVS");
            match crate::config_store::save_provisioned_config(nvs_partition.clone(), staging) {
                Ok(()) => {
                    send_ok(out)?;
                    // USB-DRAIN GUARD (HIL "commit → 0-byte timeout" fix): send_ok
                    // flushes the RSP_OK into the USB-Serial-JTAG TX ring, but the
                    // bytes still have to be transferred to the host over USB.  The
                    // caller (main.rs) reacts to our `Ok(true)` by calling
                    // `esp_restart()`, and that reset re-enumerates the USB device —
                    // discarding any TX bytes the controller has not yet handed to
                    // the host.  Without this dwell the host's `commit` reliably
                    // times out "accumulated 0 bytes" even though provisioning DID
                    // persist.  250 ms is far longer than a USB micro-frame, so the
                    // 7-byte RSP_OK reaches the host before the reboot.
                    esp_idf_hal::delay::FreeRtos::delay_ms(250);
                    return Ok(true); // caller exits the loop and reboots
                }
                Err(e) => {
                    log::error!("prov_server: NVS save failed: {:?}", e);
                    send_error(out, err::STORAGE_ERROR, b"NVS save failed")?;
                }
            }
        }

        unknown => {
            log::warn!("prov_server: unknown frame type 0x{:02x}", unknown);
            send_error(out, 0xFF, b"unknown frame type")?;
        }
    }
    Ok(false)
}

// ── Response helpers ──────────────────────────────────────────────────────────

fn send_ok(out: &mut impl Write) -> anyhow::Result<()> {
    // Route through send_frame so the RSP_OK shares the same serial-TX lock
    // discipline as every other frame (no separate, unlocked write path).
    send_frame(out, FRAME_RSP_OK, &[])
}

fn send_error(out: &mut impl Write, code: u8, msg: &[u8]) -> anyhow::Result<bool> {
    let mut payload_buf = [0u8; 128];
    let plen = encode_rsp_error(code, msg, &mut payload_buf);
    send_frame(out, FRAME_RSP_ERROR, &payload_buf[..plen])?;
    Ok(false)
}

fn send_frame(out: &mut impl Write, frame_type: u8, payload: &[u8]) -> anyhow::Result<()> {
    let mut frame_buf = [0u8; 512];
    let n = encode_frame(frame_type, payload, &mut frame_buf);
    // Hold the shared serial-TX lock across the whole frame (write + flush): the
    // first-boot UI pump thread logs concurrently on this same USB-Serial-JTAG
    // stdout, so without the lock a log line could interleave mid-frame and
    // corrupt the host's parse — identical mechanism to the post-provisioning
    // admin_server defect. The C logger takes the same lock via the
    // serial_console vprintf hook. No logging inside this section → no nesting.
    let _tx = crate::serial_console::lock_tx();
    out.write_all(&frame_buf[..n]).map_err(|e| anyhow!("stdout write: {}", e))?;
    out.flush().map_err(|e| anyhow!("stdout flush: {}", e))?;
    Ok(())
}

// ── Frame synchronisation ─────────────────────────────────────────────────────

/// Return the index of the first byte in `buf` that could be the start of a
/// `PROV_MAGIC` sequence.  Returns `buf.len()` if no candidate start is found.
fn find_magic_start(buf: &[u8]) -> usize {
    let m0 = PROV_MAGIC[0];
    let m1 = PROV_MAGIC[1];
    for i in 0..buf.len() {
        if buf[i] == m0 {
            if i + 1 < buf.len() {
                if buf[i + 1] == m1 {
                    return i;
                }
                // This 0x4D is not part of MAGIC — keep scanning.
            } else {
                return i; // MAGIC[0] at end — can't discard yet
            }
        }
    }
    buf.len()
}
