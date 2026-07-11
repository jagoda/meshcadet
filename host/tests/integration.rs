// SPDX-License-Identifier: GPL-3.0-only
//! Integration tests for the host CLI session layer.
//!
//! Uses `MockTransport` — an in-process device simulator — instead of a real
//! USB-serial port.  Each test drives `Session<MockTransport>` directly,
//! exercising the same code path used against a real device.
//!
//! # Acceptance criteria covered
//!
//! The integration test must:
//! - Provision a contact                   ✓ `test_add_contact`
//! - Provision a channel                   ✓ `test_add_channel`
//! - Read back identity                    ✓ `test_identity_readout`
//! - Set + persist the device name         ✓ `test_set_device_name_persists_across_reboot`
//! - Reset the PIN                         ✓ `test_reset_pin`
//! - Full provisioning session end-to-end  ✓ `test_full_provisioning_session`

use std::collections::VecDeque;

use protocol::channel_hash_var;
use protocol::history::{
    encode_rsp_history_entry, HistoryEntry, HistoryMsgType, MAX_HISTORY_TEXT_LEN,
    MAX_RSP_HISTORY_ENTRY_PAYLOAD,
};
use protocol::provisioning::{
    decode_add_channel, decode_add_contact, decode_del_channel, decode_del_contact, decode_frame,
    decode_set_device_name, decode_set_notif_defaults, decode_set_pin, encode_frame,
    encode_rsp_channel, encode_rsp_contact, encode_rsp_error, encode_rsp_identity,
    encode_rsp_status, RspStatusPayload, FRAME_ADD_CHANNEL, FRAME_ADD_CONTACT, FRAME_CLEAR_HISTORY,
    FRAME_COMMIT_PROVISIONING, FRAME_DEL_CHANNEL, FRAME_DEL_CONTACT, FRAME_EXPORT_HISTORY,
    FRAME_QUERY_CHANNELS, FRAME_QUERY_CONTACTS, FRAME_QUERY_STATUS, FRAME_RSP_CHANNEL,
    FRAME_RSP_CHANNELS_DONE, FRAME_RSP_CONTACT, FRAME_RSP_CONTACTS_DONE, FRAME_RSP_ERROR,
    FRAME_RSP_HISTORY_DONE, FRAME_RSP_HISTORY_ENTRY, FRAME_RSP_IDENTITY, FRAME_RSP_OK,
    FRAME_RSP_STATUS, FRAME_SET_DEVICE_NAME, FRAME_SET_NOTIF_DEFAULTS, FRAME_SET_PIN,
};

// Pull in the host crate's public types.
use host::session::Session;
use host::transport::Transport;

// ── MockDevice ────────────────────────────────────────────────────────────────

/// In-process device simulator.  Processes provisioning frames and produces the
/// correct response frames, mirroring what `firmware/src/provisioning_server.rs`
/// does on the real device.
/// A staged contact entry stored by the mock device.
#[derive(Clone)]
struct ContactRec {
    pubkey: [u8; 32],
    telemetry: bool,
    name: Vec<u8>,
}

/// A staged channel entry stored by the mock device.
#[derive(Clone)]
struct ChannelRec {
    secret: [u8; 32],
    key_len: u8,
    primary: bool,
    name: Vec<u8>,
}

struct MockDevice {
    /// Ed25519 pubkey the mock reports as its identity.
    pubkey: [u8; 32],
    /// Whether CommitProvisioning has been called.
    provisioned: bool,
    /// Staged contacts (mirrors firmware provisioning_server staging).
    contacts: Vec<ContactRec>,
    /// Staged channels.
    channels: Vec<ChannelRec>,
    /// Last PIN received (for verification in tests).
    last_pin: Option<Vec<u8>>,
    /// Whether the most recent AddContact had telemetry enabled.
    last_contact_telemetry: Option<bool>,
    /// Last contact display name received.
    last_contact_name: Option<Vec<u8>>,
    /// Whether the most recent AddChannel was marked primary.
    last_channel_primary: Option<bool>,
    /// Last channel name received.
    last_channel_name: Option<Vec<u8>>,
    /// Simulated conversation history entries (entry, is_ours), returned
    /// oldest-first on FRAME_EXPORT_HISTORY.
    mock_history: Vec<(HistoryEntry, bool)>,
    /// Persisted device display name (mirrors firmware `identity_store`'s
    /// `mc_id`/`name` key) — set via `FRAME_SET_DEVICE_NAME`, read back in
    /// every `RSP_IDENTITY`, and — because the mock outlives a `Session` the
    /// same way NVS outlives a reboot — the natural stand-in for "persists
    /// across reboots" in these host-side tests (see `docs/adr` HIL note:
    /// real reboot persistence is verified on hardware).
    device_name: Vec<u8>,
}

impl MockDevice {
    fn new(pubkey: [u8; 32]) -> Self {
        Self {
            pubkey,
            provisioned: false,
            contacts: Vec::new(),
            channels: Vec::new(),
            last_pin: None,
            last_contact_telemetry: None,
            last_contact_name: None,
            last_channel_primary: None,
            last_channel_name: None,
            mock_history: Vec::new(),
            device_name: Vec::new(),
        }
    }

    fn with_history(pubkey: [u8; 32], history: Vec<(HistoryEntry, bool)>) -> Self {
        let mut d = Self::new(pubkey);
        d.mock_history = history;
        d
    }

    /// Process one frame (already decoded) and return the response bytes.
    fn handle(&mut self, frame_type: u8, payload: &[u8]) -> Vec<u8> {
        match frame_type {
            FRAME_QUERY_STATUS => {
                let status = RspStatusPayload {
                    provisioned: self.provisioned,
                    pubkey: self.pubkey,
                    contact_count: self.contacts.len() as u8,
                    channel_count: self.channels.len() as u8,
                    // Mock device has no GPS hardware; mirrors an unprovisioned
                    // / never-had-fix device for the purposes of these tests.
                    gps_has_fix: false,
                    gps_lat_e7: 0,
                    gps_lon_e7: 0,
                    gps_fix_age_secs: 0,
                    gps_clock_synced: false,
                    gps_clock_sync_age_secs: 0,
                    // Mock device has no battery ADC either; mirrors the
                    // "no reading yet" state.
                    battery_percent: 0,
                    battery_charging: false,
                    battery_raw_mv: 0,
                    battery_held_raw_mv: 0,
                };
                let mut pbuf = [0u8; 64];
                let plen = encode_rsp_status(&status, &mut pbuf);
                let mut fbuf = [0u8; 128];
                let n = encode_frame(FRAME_RSP_STATUS, &pbuf[..plen], &mut fbuf);
                let mut response = fbuf[..n].to_vec();
                // Firmware sends RSP_IDENTITY immediately after RSP_STATUS.
                // The host's query_status() consumes both frames; mock must
                // mirror the firmware to avoid desync in tests.
                let mut ibuf = [0u8; 96];
                let ilen = encode_rsp_identity(&self.pubkey, &self.device_name, &mut ibuf);
                let mut iframebuf = [0u8; 128];
                let in_ = encode_frame(FRAME_RSP_IDENTITY, &ibuf[..ilen], &mut iframebuf);
                response.extend_from_slice(&iframebuf[..in_]);
                response
            }

            FRAME_ADD_CONTACT => match decode_add_contact(payload) {
                Ok(c) => {
                    self.last_contact_telemetry = Some(c.telemetry_enable);
                    let name = c.display_name[..c.display_name_len as usize].to_vec();
                    self.last_contact_name = Some(name.clone());
                    self.contacts.push(ContactRec {
                        pubkey: c.pubkey,
                        telemetry: c.telemetry_enable,
                        name,
                    });
                    ok_frame()
                }
                Err(e) => error_frame(1, &format!("{:?}", e)),
            },

            FRAME_DEL_CONTACT => match decode_del_contact(payload) {
                Ok(d) => match self.contacts.iter().position(|c| c.pubkey == d.pubkey) {
                    Some(idx) => {
                        self.contacts.remove(idx);
                        ok_frame()
                    }
                    None => error_frame(3, "contact not found"),
                },
                Err(e) => error_frame(1, &format!("{:?}", e)),
            },

            FRAME_ADD_CHANNEL => match decode_add_channel(payload) {
                Ok(ch) => {
                    self.last_channel_primary = Some(ch.primary);
                    let name = ch.name[..ch.name_len as usize].to_vec();
                    self.last_channel_name = Some(name.clone());
                    if ch.primary {
                        for existing in self.channels.iter_mut() {
                            existing.primary = false;
                        }
                    }
                    self.channels.push(ChannelRec {
                        secret: ch.secret,
                        key_len: ch.key_len,
                        primary: ch.primary,
                        name,
                    });
                    ok_frame()
                }
                Err(e) => error_frame(1, &format!("{:?}", e)),
            },

            FRAME_DEL_CHANNEL => match decode_del_channel(payload) {
                Ok(d) => match self.channels.iter().position(|ch| ch.secret == d.secret) {
                    Some(idx) => {
                        self.channels.remove(idx);
                        ok_frame()
                    }
                    None => error_frame(4, "channel not found"),
                },
                Err(e) => error_frame(1, &format!("{:?}", e)),
            },

            FRAME_QUERY_CONTACTS => {
                let mut response: Vec<u8> = Vec::new();
                for (idx, c) in self.contacts.iter().enumerate() {
                    let mut pbuf = [0u8; 80];
                    let plen =
                        encode_rsp_contact(idx as u8, &c.pubkey, c.telemetry, &c.name, &mut pbuf);
                    let mut fbuf = [0u8; 128];
                    let n = encode_frame(FRAME_RSP_CONTACT, &pbuf[..plen], &mut fbuf);
                    response.extend_from_slice(&fbuf[..n]);
                }
                let mut done = [0u8; 16];
                let n = encode_frame(FRAME_RSP_CONTACTS_DONE, &[], &mut done);
                response.extend_from_slice(&done[..n]);
                response
            }

            FRAME_QUERY_CHANNELS => {
                let mut response: Vec<u8> = Vec::new();
                for (idx, ch) in self.channels.iter().enumerate() {
                    let hash = channel_hash_var(&ch.secret[..ch.key_len as usize]);
                    let mut pbuf = [0u8; 80];
                    let plen = encode_rsp_channel(
                        idx as u8, hash, ch.key_len, ch.primary, &ch.name, &mut pbuf,
                    );
                    let mut fbuf = [0u8; 128];
                    let n = encode_frame(FRAME_RSP_CHANNEL, &pbuf[..plen], &mut fbuf);
                    response.extend_from_slice(&fbuf[..n]);
                }
                let mut done = [0u8; 16];
                let n = encode_frame(FRAME_RSP_CHANNELS_DONE, &[], &mut done);
                response.extend_from_slice(&done[..n]);
                response
            }

            FRAME_SET_NOTIF_DEFAULTS => match decode_set_notif_defaults(payload) {
                Ok(_) => ok_frame(),
                Err(e) => error_frame(1, &format!("{:?}", e)),
            },

            FRAME_SET_PIN => match decode_set_pin(payload) {
                Ok(p) => {
                    self.last_pin = Some(p.pin[..p.pin_len as usize].to_vec());
                    ok_frame()
                }
                Err(e) => error_frame(1, &format!("{:?}", e)),
            },

            FRAME_SET_DEVICE_NAME => match decode_set_device_name(payload) {
                Ok(n) => {
                    self.device_name = n.name[..n.name_len as usize].to_vec();
                    ok_frame()
                }
                Err(e) => error_frame(1, &format!("{:?}", e)),
            },

            FRAME_COMMIT_PROVISIONING => {
                self.provisioned = true;
                ok_frame()
            }

            FRAME_CLEAR_HISTORY => {
                // Mirrors `HistoryStore::clear_all` — wipe every conversation,
                // both directions, unconditionally.
                self.mock_history.clear();
                ok_frame()
            }

            FRAME_EXPORT_HISTORY => {
                // Build a streaming response: N HISTORY_ENTRY frames + 1 HISTORY_DONE frame.
                let mut response: Vec<u8> = Vec::new();
                for (idx, (entry, is_ours)) in self.mock_history.iter().enumerate() {
                    let mut pbuf = [0u8; MAX_RSP_HISTORY_ENTRY_PAYLOAD + 1];
                    let plen = encode_rsp_history_entry(idx as u8, entry, *is_ours, &mut pbuf);
                    let mut fbuf = [0u8; MAX_RSP_HISTORY_ENTRY_PAYLOAD + 16];
                    let n = encode_frame(FRAME_RSP_HISTORY_ENTRY, &pbuf[..plen], &mut fbuf);
                    response.extend_from_slice(&fbuf[..n]);
                }
                // Terminal DONE frame.
                let mut done_buf = [0u8; 16];
                let n = encode_frame(FRAME_RSP_HISTORY_DONE, &[], &mut done_buf);
                response.extend_from_slice(&done_buf[..n]);
                response
            }

            _ => error_frame(0xFF, &format!("unknown frame type 0x{:02X}", frame_type)),
        }
    }
}

fn ok_frame() -> Vec<u8> {
    let mut buf = [0u8; 16];
    let n = encode_frame(FRAME_RSP_OK, &[], &mut buf);
    buf[..n].to_vec()
}

fn error_frame(code: u8, msg: &str) -> Vec<u8> {
    let msg_bytes = msg.as_bytes();
    let mut pbuf = [0u8; 80];
    let plen = encode_rsp_error(code, msg_bytes, &mut pbuf);
    let mut fbuf = [0u8; 128];
    let n = encode_frame(FRAME_RSP_ERROR, &pbuf[..plen], &mut fbuf);
    fbuf[..n].to_vec()
}

// ── MockTransport ─────────────────────────────────────────────────────────────

/// In-process transport: connects a `Session` to a `MockDevice`.
///
/// Invariant: `send()` always delivers a complete provisioning frame to the
/// mock device (the session layer always sends exactly one frame per call).
struct MockTransport {
    device: MockDevice,
    /// Accumulation buffer for host→device bytes (handles partial sends).
    send_buf: Vec<u8>,
    /// Response bytes queued for the session to read.
    recv_buf: VecDeque<u8>,
}

impl MockTransport {
    fn new(device: MockDevice) -> Self {
        Self {
            device,
            send_buf: Vec::new(),
            recv_buf: VecDeque::new(),
        }
    }
}

impl Transport for MockTransport {
    fn send(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.send_buf.extend_from_slice(data);

        // Drain complete frames from send_buf and process each one.
        loop {
            if self.send_buf.len() < 5 {
                break;
            }
            let plen = (self.send_buf[3] as usize) | ((self.send_buf[4] as usize) << 8);
            let total = 7 + plen;
            if self.send_buf.len() < total {
                break;
            }
            // Decode (validates magic + CRC).
            let (ft, payload_slice) = decode_frame(&self.send_buf[..total])
                .map_err(|e| anyhow::anyhow!("mock: host sent bad frame: {:?}", e))?;
            let payload: Vec<u8> = payload_slice.to_vec();
            self.send_buf.drain(..total);

            // Process through mock device, buffer response.
            let response = self.device.handle(ft, &payload);
            self.recv_buf.extend(response);
        }

        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> anyhow::Result<usize> {
        let n = buf.len().min(self.recv_buf.len());
        for b in buf[..n].iter_mut() {
            *b = self.recv_buf.pop_front().unwrap();
        }
        Ok(n)
    }
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Create a `Session` backed by a `MockDevice` with the given public key.
/// The session timeout is 1 second (immediate in tests since the mock is synchronous).
fn make_session_v2(pubkey: [u8; 32]) -> Session<MockTransport> {
    let device = MockDevice::new(pubkey);
    let transport = MockTransport::new(device);
    Session::with_timeout(transport, 1)
}

/// Create a `Session` backed by a `MockDevice` pre-loaded with conversation history.
fn make_session_with_history(history: Vec<(HistoryEntry, bool)>) -> Session<MockTransport> {
    let device = MockDevice::with_history([0xAA_u8; 32], history);
    let transport = MockTransport::new(device);
    Session::with_timeout(transport, 1)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Acceptance: identity readout.
#[test]
fn test_identity_readout() {
    let pubkey = [0xAA_u8; 32];
    let mut session = make_session_v2(pubkey);

    let status = session.query_status().expect("query_status should succeed");

    assert_eq!(status.pubkey, pubkey, "device pubkey must round-trip");
    assert_eq!(status.pubkey[0], pubkey[0], "pub_hash = pubkey[0]");
    // A fresh mock device is not yet provisioned.
    assert!(!status.provisioned);
    assert_eq!(status.contact_count, 0);
    assert_eq!(status.channel_count, 0);
}

/// Acceptance: a fresh device (never had a name set) reads back unnamed.
#[test]
fn test_identity_readout_unnamed_by_default() {
    let mut session = make_session_v2([0xBB_u8; 32]);
    session.query_status().expect("query_status should succeed");
    assert_eq!(session.last_device_name(), None);
}

/// Acceptance: `identity --set-name` persists the device name, and the
/// persisted value survives a reboot.
///
/// A `MockDevice` lives only as long as its `Session`/`MockTransport`, so
/// there is no literal power-cycle to trigger here — that is a HIL concern
/// (verified against real firmware, which reloads the name from
/// `identity_store::load_name()` / NVS at boot; see
/// `firmware/src/identity_store.rs`). The host-testable proxy is: build a
/// brand-new `Session` over a brand-new `MockDevice` pre-loaded with exactly
/// the name the prior session persisted, and confirm a fresh connection reads
/// it back — the same wire contract (`RSP_IDENTITY`) a rebooted device answers
/// with.
#[test]
fn test_set_device_name_persists_across_reboot() {
    let pubkey = [0x33_u8; 32];
    let mut session = make_session_v2(pubkey);

    session
        .set_device_name(b"Alex's MeshCadet")
        .expect("set_device_name should succeed");

    session
        .query_status()
        .expect("query_status after set_device_name");
    assert_eq!(session.last_device_name(), Some("Alex's MeshCadet"));

    // "Reboot": fresh session, fresh mock device pre-loaded with the
    // persisted name (mirrors NVS surviving a real power-cycle).
    let mut rebooted_device = MockDevice::new(pubkey);
    rebooted_device.device_name = b"Alex's MeshCadet".to_vec();
    let transport = MockTransport::new(rebooted_device);
    let mut session_after_reboot = Session::with_timeout(transport, 1);

    session_after_reboot
        .query_status()
        .expect("query_status after reboot");
    assert_eq!(
        session_after_reboot.last_device_name(),
        Some("Alex's MeshCadet"),
        "device name must survive a reboot"
    );
}

/// Acceptance: setting an empty name clears the persisted name.
#[test]
fn test_set_device_name_empty_clears() {
    let pubkey = [0x44_u8; 32];
    let mut session = make_session_v2(pubkey);

    session
        .set_device_name(b"Temporary")
        .expect("set_device_name should succeed");
    session.query_status().expect("query_status after set");
    assert_eq!(session.last_device_name(), Some("Temporary"));

    session
        .set_device_name(b"")
        .expect("set_device_name (clear) should succeed");
    session.query_status().expect("query_status after clear");
    assert_eq!(session.last_device_name(), None);
}

/// Acceptance: setting the device name does not disturb any other identity/
/// status field — additive, not a rewrite.
#[test]
fn test_set_device_name_does_not_affect_other_identity_fields() {
    let pubkey = [0x55_u8; 32];
    let mut session = make_session_v2(pubkey);

    let before = session
        .query_status()
        .expect("query_status before set_device_name");
    session
        .set_device_name(b"Naming Things")
        .expect("set_device_name should succeed");
    let after = session
        .query_status()
        .expect("query_status after set_device_name");

    assert_eq!(before.pubkey, after.pubkey);
    assert_eq!(before.provisioned, after.provisioned);
    assert_eq!(before.contact_count, after.contact_count);
    assert_eq!(before.channel_count, after.channel_count);
}

/// Acceptance: provision a contact.
#[test]
fn test_add_contact() {
    let mut session = make_session_v2([0x11_u8; 32]);

    let contact_pubkey = [0xCC_u8; 32];
    session
        .add_contact(&contact_pubkey, /*telemetry=*/ true, b"Alice")
        .expect("add_contact should succeed");

    // Status should now reflect 1 contact.
    let status = session
        .query_status()
        .expect("query_status after add_contact");
    assert_eq!(status.contact_count, 1);
}

/// Acceptance: provision a channel.
///
/// Regression guard for the HIL "list-channels empty / add-channel timeout"
/// defects: `add-channel` must complete (reply OK, no timeout), and the added
/// channel must then appear in `list-channels` with the same count that
/// `status` reports — i.e. the status count and the channel-list response are
/// the same source of truth, not two that can disagree.
#[test]
fn test_add_channel() {
    let mut session = make_session_v2([0x22_u8; 32]);

    let channel_secret = [0x6D_u8; 32]; // b'm' — the HIL test secret
    session
        .add_channel(&channel_secret, 32, /*primary=*/ true, b"family")
        .expect("add_channel should succeed");

    // Status should now reflect 1 channel.
    let status = session
        .query_status()
        .expect("query_status after add_channel");
    assert_eq!(status.channel_count, 1);

    // …and that channel must be enumerable: list-channels returns exactly the
    // one we added, with a count matching status (no status/list mismatch).
    let channels = session
        .list_channels()
        .expect("list_channels after add_channel");
    assert_eq!(channels.len() as u8, status.channel_count);
    assert_eq!(channels.len(), 1);
    let ch = &channels[0];
    assert!(ch.primary, "added channel should be primary");
    assert_eq!(ch.key_len, 32);
    assert_eq!(
        channel_hash_var(&channel_secret),
        ch.channel_hash,
        "enumerated channel_hash must match the added secret"
    );
    assert_eq!(&ch.name[..ch.name_len as usize], b"family");
}

/// Regression for the HIL list-channels defect (status count vs enumeration
/// divergence): with MORE than one channel configured, the streamed enumeration
/// must return every channel — in index order, with matching hashes — and the
/// count must equal what `status` reports.  The on-device root cause was a
/// delivery-layer bug in the firmware's admin_server (batched LineWriter flush
/// across the whole stream vs the proven per-frame flush); this test pins the
/// host-visible contract that a multi-frame channel stream is fully consumed.
#[test]
fn test_list_channels_multiple_matches_status() {
    let mut session = make_session_v2([0x44_u8; 32]);

    // Mirror the HIL sequence: a primary channel plus a second non-primary one.
    let primary_secret = [0x6D_u8; 32]; // '#home'-style primary
    let second_secret = [0xA5_u8; 32];
    session
        .add_channel(&primary_secret, 32, /*primary=*/ true, b"home")
        .expect("add primary channel");
    session
        .add_channel(&second_secret, 16, /*primary=*/ false, b"scouts")
        .expect("add second channel");

    let status = session.query_status().expect("query_status");
    assert_eq!(status.channel_count, 2, "status must count both channels");

    let channels = session.list_channels().expect("list_channels");
    assert_eq!(
        channels.len() as u8,
        status.channel_count,
        "enumeration count must equal the status count — no empty-vs-count divergence",
    );
    assert_eq!(channels.len(), 2);

    // Index 0 = first added (primary, 256-bit); index 1 = second (128-bit).
    assert!(channels[0].primary);
    assert_eq!(channels[0].key_len, 32);
    assert_eq!(channel_hash_var(&primary_secret), channels[0].channel_hash);
    assert_eq!(&channels[0].name[..channels[0].name_len as usize], b"home");

    assert!(!channels[1].primary);
    assert_eq!(channels[1].key_len, 16);
    assert_eq!(
        channel_hash_var(&second_secret[..16]),
        channels[1].channel_hash,
        "128-bit channel hashes over the first 16 secret bytes",
    );
    assert_eq!(
        &channels[1].name[..channels[1].name_len as usize],
        b"scouts"
    );
}

/// Regression guard for the exact HIL acceptance sequence:
/// `add-channel` → `list-channels` lists the channel with a count matching
/// `status` → `commit` returns a normal response (RSP_OK, NOT a 0-byte
/// timeout).
///
/// The on-device root cause was that the runtime `admin_server` had no
/// `COMMIT_PROVISIONING` handler — the frame fell through to the unknown-frame
/// arm, which sends NO response, so the host's `commit` blocked until its retry
/// deadline and reported "timeout (accumulated 0 bytes)".  This pins the
/// host-visible contract that `commit` is answered in the same session that
/// `add-channel`/`list-channels` are answered, so the firmware (which now
/// mirrors this `MockDevice`) cannot silently drop it again.
#[test]
fn test_add_channel_then_list_then_commit_no_timeout() {
    let mut session = make_session_v2([0x77_u8; 32]);

    let channel_secret = [0x6D_u8; 32]; // 'm' — the HIL test secret
    session
        .add_channel(&channel_secret, 32, /*primary=*/ true, b"family")
        .expect("add_channel should reply OK (no timeout)");

    // list-channels must enumerate the just-added channel, and its count must
    // equal what status reports — the precise "empty vs count" invariant.
    let status = session.query_status().expect("status after add_channel");
    let channels = session
        .list_channels()
        .expect("list_channels after add_channel");
    assert_eq!(
        channels.len() as u8,
        status.channel_count,
        "list-channels count must match status channels:N",
    );
    assert_eq!(
        channels.len(),
        1,
        "the added channel must be listed (not empty)"
    );

    // commit must return a normal response — no 0-byte timeout.  Before the fix
    // the runtime server dropped this frame silently.
    session
        .commit()
        .expect("commit must reply (no 0-byte timeout)");
}

/// Acceptance: reset PIN (physical possession flow).
#[test]
fn test_reset_pin() {
    let mut session = make_session_v2([0x33_u8; 32]);

    // Initial set.
    session
        .set_pin(b"initial")
        .expect("set_pin (initial) should succeed");
    // Reset to a new PIN (physical possession = auth; same wire frame).
    session
        .set_pin(b"newpin99")
        .expect("set_pin (reset) should succeed");
}

/// Acceptance: full provisioning session — contact + channel + identity + PIN
/// + radio preset + notif defaults + locks + commit, in sequence.
///
/// This mirrors the expected happy-path admin workflow.
#[test]
fn test_full_provisioning_session() {
    let device_pubkey = [0xDE_u8; 32];
    let mut session = make_session_v2(device_pubkey);

    // 1. Read identity before provisioning.
    let status = session.query_status().expect("initial status");
    assert_eq!(status.pubkey, device_pubkey);
    assert!(!status.provisioned);

    // 2. Provision a contact.
    let alice_key = [0xA1_u8; 32];
    session
        .add_contact(&alice_key, true, b"Alice")
        .expect("add contact Alice");

    // 3. Provision a second contact (no telemetry, no name).
    let bob_key = [0xB0_u8; 32];
    session
        .add_contact(&bob_key, false, b"")
        .expect("add contact Bob");

    // 4. Provision a primary channel.
    let channel_secret = [0xFA_u8; 32];
    session
        .add_channel(&channel_secret, 32, true, b"family")
        .expect("add primary channel");

    // 5. Set notification defaults.
    session
        .set_notif_defaults(true, true)
        .expect("set notif defaults");

    // 6. Set PIN.
    session.set_pin(b"1234").expect("set PIN");

    // 7. Commit.
    session.commit().expect("commit provisioning");

    // 8. Verify status after commit (provisioned flag set, counts correct).
    let final_status = session.query_status().expect("status after commit");
    assert!(
        final_status.provisioned,
        "device must be provisioned after commit"
    );
    assert_eq!(final_status.contact_count, 2);
    assert_eq!(final_status.channel_count, 1);
    assert_eq!(final_status.pubkey, device_pubkey);
}

/// Verify the session surfaces device error responses correctly.
#[test]
fn test_device_error_surfaces_to_caller() {
    let mut session = make_session_v2([0x55_u8; 32]);

    // Attempt to delete a non-existent contact (mock returns RSP_ERROR code=2).
    let result = session.del_contact(&[0xFF_u8; 32]);
    assert!(result.is_err(), "del_contact on empty list must fail");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("error") || msg.contains("delete"),
        "error message should describe the problem: {msg}"
    );
}

/// Verify that del_contact removes a previously added contact.
#[test]
fn test_del_contact() {
    let mut session = make_session_v2([0x44_u8; 32]);

    let key = [0xCC_u8; 32];
    session
        .add_contact(&key, false, b"Temp")
        .expect("add contact");
    session.del_contact(&key).expect("del contact");

    let status = session.query_status().expect("status after del");
    assert_eq!(status.contact_count, 0);
}

/// Verify that del_channel removes a previously added channel.
#[test]
fn test_del_channel() {
    let mut session = make_session_v2([0x44_u8; 32]);

    let secret = [0xBB_u8; 32];
    session
        .add_channel(&secret, 32, false, b"temp")
        .expect("add channel");
    session.del_channel(&secret).expect("del channel");

    let status = session.query_status().expect("status after del");
    assert_eq!(status.channel_count, 0);
}

// ── Contact / channel enumeration ─────────────────────────────────────────────

/// Acceptance: list-contacts returns the configured entries (name + pubkey +
/// telemetry), in device-index order, matching what was provisioned.
#[test]
fn test_list_contacts() {
    let mut session = make_session_v2([0x11_u8; 32]);

    let alice = [0xA1_u8; 32];
    let bob = [0xB0_u8; 32];
    session
        .add_contact(&alice, true, b"Alice")
        .expect("add Alice");
    session
        .add_contact(&bob, false, b"")
        .expect("add Bob (no name)");

    let contacts = session.list_contacts().expect("list_contacts");
    assert_eq!(contacts.len(), 2);

    assert_eq!(contacts[0].index, 0);
    assert_eq!(contacts[0].pubkey, alice);
    assert!(contacts[0].telemetry_enable);
    assert_eq!(
        &contacts[0].display_name[..contacts[0].display_name_len as usize],
        b"Alice"
    );

    assert_eq!(contacts[1].index, 1);
    assert_eq!(contacts[1].pubkey, bob);
    assert!(!contacts[1].telemetry_enable);
    assert_eq!(contacts[1].display_name_len, 0);
}

/// list-contacts on a fresh device returns an empty list (DONE with no entries).
#[test]
fn test_list_contacts_empty() {
    let mut session = make_session_v2([0x11_u8; 32]);
    let contacts = session.list_contacts().expect("list_contacts empty");
    assert!(contacts.is_empty());
}

/// Acceptance: list-channels returns name + channel hash + key length + primary,
/// and the channel hash matches the on-air hash of the provisioned secret.
#[test]
fn test_list_channels() {
    let mut session = make_session_v2([0x22_u8; 32]);

    let secret = [0x6D_u8; 32]; // b'm'
    session
        .add_channel(&secret, 32, true, b"family")
        .expect("add channel");

    let channels = session.list_channels().expect("list_channels");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].index, 0);
    assert_eq!(channels[0].key_len, 32);
    assert!(channels[0].primary);
    assert_eq!(
        &channels[0].name[..channels[0].name_len as usize],
        b"family"
    );
    assert_eq!(
        channels[0].channel_hash,
        channel_hash_var(&secret),
        "listed channel hash must match on-air hash of the secret",
    );
}

/// Acceptance (criterion 3): del-contact removes an entry on-device — delete one,
/// then re-list shows it gone AND status count drops.
#[test]
fn test_del_contact_then_list_shows_gone_and_count_drops() {
    let mut session = make_session_v2([0x44_u8; 32]);

    let alice = [0xA1_u8; 32];
    let bob = [0xB0_u8; 32];
    session
        .add_contact(&alice, true, b"Alice")
        .expect("add Alice");
    session.add_contact(&bob, false, b"Bob").expect("add Bob");
    assert_eq!(session.query_status().unwrap().contact_count, 2);

    // Delete Alice.
    session.del_contact(&alice).expect("del Alice");

    // Re-list: only Bob remains, and the index is re-packed.
    let contacts = session.list_contacts().expect("list after del");
    assert_eq!(
        contacts.len(),
        1,
        "deleted contact must be gone from the list"
    );
    assert_eq!(contacts[0].pubkey, bob);
    assert_eq!(contacts[0].index, 0);

    // Status count drops.
    assert_eq!(session.query_status().unwrap().contact_count, 1);
}

/// Acceptance (criterion 3, channels): del-channel removes an entry — re-list
/// shows it gone and status channel count drops.
#[test]
fn test_del_channel_then_list_shows_gone_and_count_drops() {
    let mut session = make_session_v2([0x44_u8; 32]);

    let fam = [0x6D_u8; 32];
    let work = [0x77_u8; 32];
    session
        .add_channel(&fam, 32, true, b"family")
        .expect("add family");
    session
        .add_channel(&work, 32, false, b"work")
        .expect("add work");
    assert_eq!(session.query_status().unwrap().channel_count, 2);

    session.del_channel(&fam).expect("del family");

    let channels = session.list_channels().expect("list after del");
    assert_eq!(channels.len(), 1);
    assert_eq!(&channels[0].name[..channels[0].name_len as usize], b"work");
    assert_eq!(session.query_status().unwrap().channel_count, 1);
}

/// Verify notification defaults are accepted.
#[test]
fn test_set_notif_defaults() {
    let mut session = make_session_v2([0x88_u8; 32]);

    session
        .set_notif_defaults(true, false)
        .expect("set_notif_defaults visual-only");
    session
        .set_notif_defaults(false, true)
        .expect("set_notif_defaults audible-only");
    session
        .set_notif_defaults(false, false)
        .expect("set_notif_defaults both-off");
}

// ── History export tests ──────────────────────────────────────────────────────

fn make_dm_entry(sender_hash: u8, timestamp: u32, text: &[u8]) -> HistoryEntry {
    let text_len = text.len().min(MAX_HISTORY_TEXT_LEN) as u8;
    let mut text_buf = [0u8; MAX_HISTORY_TEXT_LEN];
    text_buf[..text_len as usize].copy_from_slice(&text[..text_len as usize]);
    HistoryEntry {
        sender_hash,
        msg_type: HistoryMsgType::Dm,
        timestamp,
        text: text_buf,
        text_len,
    }
}

fn make_grp_entry(sender_hash: u8, timestamp: u32, text: &[u8]) -> HistoryEntry {
    let text_len = text.len().min(MAX_HISTORY_TEXT_LEN) as u8;
    let mut text_buf = [0u8; MAX_HISTORY_TEXT_LEN];
    text_buf[..text_len as usize].copy_from_slice(&text[..text_len as usize]);
    HistoryEntry {
        sender_hash,
        msg_type: HistoryMsgType::GrpTxt,
        timestamp,
        text: text_buf,
        text_len,
    }
}

/// Acceptance: export from device with no history returns empty list.
#[test]
fn test_export_history_empty() {
    let mut session = make_session_with_history(vec![]);
    let entries = session
        .export_history()
        .expect("export_history (empty) must succeed");
    assert!(
        entries.is_empty(),
        "expected no entries, got {}",
        entries.len()
    );
}

/// Acceptance: single DM entry round-trips through export.
#[test]
fn test_export_history_single_entry() {
    let entry = make_dm_entry(0xAB, 1_000_000, b"hello world");
    let mut session = make_session_with_history(vec![(entry, false)]);
    let got = session
        .export_history()
        .expect("export_history must succeed");
    assert_eq!(got.len(), 1, "expected 1 entry");
    assert_eq!(got[0].0.sender_hash, 0xAB);
    assert_eq!(got[0].0.timestamp, 1_000_000);
    let text = &got[0].0.text[..got[0].0.text_len as usize];
    assert_eq!(text, b"hello world");
}

/// Acceptance: multiple entries arrive in oldest-first order.
#[test]
fn test_export_history_multiple_entries_oldest_first() {
    let entries = vec![
        (make_dm_entry(0x01, 100, b"oldest"), false),
        (make_dm_entry(0x02, 200, b"middle"), false),
        (make_dm_entry(0x03, 300, b"newest"), false),
    ];
    let mut session = make_session_with_history(entries.clone());
    let got = session
        .export_history()
        .expect("export_history must succeed");
    assert_eq!(got.len(), 3, "expected 3 entries");
    assert_eq!(got[0].0.timestamp, 100, "first must be oldest");
    assert_eq!(got[1].0.timestamp, 200);
    assert_eq!(got[2].0.timestamp, 300, "last must be newest");
}

/// Acceptance: GRP_TXT entries export with correct message type.
#[test]
fn test_export_history_grp_txt_entries() {
    let entry = make_grp_entry(0xCC, 9999, b"group message");
    let mut session = make_session_with_history(vec![(entry, false)]);
    let got = session
        .export_history()
        .expect("export_history must succeed");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0.msg_type, HistoryMsgType::GrpTxt);
    let text = &got[0].0.text[..got[0].0.text_len as usize];
    assert_eq!(text, b"group message");
}

/// Acceptance: mixed DM and GRP_TXT entries preserve their types.
#[test]
fn test_export_history_mixed_message_types() {
    let entries = vec![
        (make_dm_entry(0x10, 1, b"dm msg"), false),
        (make_grp_entry(0x20, 2, b"grp msg"), false),
        (make_dm_entry(0x30, 3, b"dm msg 2"), false),
    ];
    let mut session = make_session_with_history(entries);
    let got = session
        .export_history()
        .expect("export_history must succeed");
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].0.msg_type, HistoryMsgType::Dm);
    assert_eq!(got[1].0.msg_type, HistoryMsgType::GrpTxt);
    assert_eq!(got[2].0.msg_type, HistoryMsgType::Dm);
}

/// Acceptance: text content is preserved byte-for-byte.
#[test]
fn test_export_history_text_preserved() {
    let text = b"exact bytes 0x42";
    let entry = make_dm_entry(0xFF, 42, text);
    let mut session = make_session_with_history(vec![(entry, false)]);
    let got = session
        .export_history()
        .expect("export_history must succeed");
    assert_eq!(got.len(), 1);
    let got_text = &got[0].0.text[..got[0].0.text_len as usize];
    assert_eq!(got_text, text, "text bytes must be preserved exactly");
}

/// Acceptance: the `export-history` CLI output prints a header row, and each
/// entry's line renders the raw `u32` timestamp as a local human-readable
/// string rather than a bare decimal — the exact rendering the `ExportHistory`
/// command arm in `host/src/main.rs` prints via `host::history_format`.
#[test]
fn test_export_history_cli_output_has_header_and_local_timestamp() {
    let entry = make_dm_entry(0xAB, 1_700_000_000, b"hello world");
    let mut session = make_session_with_history(vec![(entry, false)]);
    let got = session
        .export_history()
        .expect("export_history must succeed");
    assert_eq!(got.len(), 1);

    let iw = host::history_format::idx_width(got.len());
    let header = host::history_format::history_header(iw);
    assert!(
        header.starts_with("idx"),
        "header must start with idx column: {header:?}"
    );
    assert!(
        header.contains("timestamp"),
        "header missing timestamp column: {header:?}"
    );
    assert!(
        header.contains("type"),
        "header missing type column: {header:?}"
    );
    assert!(
        header.contains("from"),
        "header missing from column: {header:?}"
    );
    assert!(
        header.ends_with("text"),
        "header must end with text column: {header:?}"
    );

    let (e, is_ours) = &got[0];
    let line = host::history_format::format_history_line(0, e, *is_ours, iw);
    // Header and data row must align: each column starts at the same byte
    // offset in both strings, regardless of terminal tab stops.
    let ts_start = header
        .find("timestamp")
        .expect("header has timestamp column");
    let ty_start = header.find("type").expect("header has type column");
    let dr_start = header.find("dir").expect("header has dir column");
    let fr_start = header.find("from").expect("header has from column");
    let tx_start = header.find("text").expect("header has text column");

    let ts_field = &line[ts_start..ty_start];
    let ty_field = &line[ty_start..dr_start];
    let dr_field = &line[dr_start..fr_start];
    let fr_field = &line[fr_start..tx_start];
    let tx_field = &line[tx_start..];

    assert!(
        line.starts_with('0'),
        "expected idx column to start with '0': {line:?}"
    );
    // The bare u32 must not appear verbatim — it's rendered as a local
    // timestamp string, not the raw decimal.
    assert!(
        !ts_field.contains("1700000000"),
        "raw epoch leaked into timestamp column: {ts_field:?}"
    );
    assert!(
        ts_field.starts_with(&host::history_format::format_local_timestamp(1_700_000_000)),
        "unexpected timestamp column: {ts_field:?}"
    );
    assert_eq!(ty_field.trim(), "DM");
    assert_eq!(dr_field.trim(), "RECV");
    assert_eq!(fr_field.trim(), "0xAB");
    assert_eq!(tx_field, "hello world");
}

/// Acceptance: an
/// outbound entry (`is_ours=true`) round-trips through the full wire export
/// path (mock device → `Session::export_history` → CLI rendering) and is
/// distinguishable from an inbound one via the `dir` column, closing the
/// observability gap the HIL finding surfaced (`from` alone is always the
/// conversation hash, never the device's own hash, for either direction).
#[test]
fn test_export_history_outbound_entry_renders_as_sent() {
    let sent = make_dm_entry(0x46, 100, b"outbound dm");
    let received = make_dm_entry(0x46, 200, b"inbound dm");
    let mut session = make_session_with_history(vec![(sent, true), (received, false)]);
    let got = session
        .export_history()
        .expect("export_history must succeed");
    assert_eq!(got.len(), 2);
    assert!(got[0].1, "first entry must round-trip is_ours=true");
    assert!(!got[1].1, "second entry must round-trip is_ours=false");

    let iw = host::history_format::idx_width(got.len());
    let sent_line = host::history_format::format_history_line(0, &got[0].0, got[0].1, iw);
    let recv_line = host::history_format::format_history_line(1, &got[1].0, got[1].1, iw);
    assert!(
        sent_line.contains("SENT"),
        "outbound row must show SENT: {sent_line:?}"
    );
    assert!(
        recv_line.contains("RECV"),
        "inbound row must show RECV: {recv_line:?}"
    );
}

// ── Clear-history tests ────────────────────────────────────────────────────────

/// Acceptance: clear-history against a device with existing DM + channel
/// history succeeds, and a subsequent export shows zero entries — the
/// device-side check called for (this mock's `FRAME_CLEAR_HISTORY`
/// handler mirrors `HistoryStore::clear_all` in intent: wipe every
/// conversation, both directions).
#[test]
fn test_clear_history_then_export_is_empty() {
    let entries = vec![
        (make_dm_entry(0x01, 100, b"dm sent"), true),
        (make_dm_entry(0x01, 150, b"dm received"), false),
        (make_grp_entry(0x6D, 200, b"channel msg"), false),
    ];
    let mut session = make_session_with_history(entries);

    // Sanity: history is present before the clear.
    let before = session.export_history().expect("export before clear");
    assert_eq!(
        before.len(),
        3,
        "fixture must seed 3 entries before clearing"
    );

    session.clear_history().expect("clear_history must succeed");

    let after = session.export_history().expect("export after clear");
    assert!(
        after.is_empty(),
        "expected zero entries after clear-history, got {}",
        after.len()
    );
}

/// Clear-history on a device with no history is a harmless no-op success —
/// there is nothing to wipe, but the command must not error.
#[test]
fn test_clear_history_on_empty_device_succeeds() {
    let mut session = make_session_with_history(vec![]);
    session
        .clear_history()
        .expect("clear_history on empty history must still succeed");
    let after = session.export_history().expect("export after clear");
    assert!(after.is_empty());
}

/// Clear-history must not desync the frame stream: a command sent right
/// after it (query_status) must still get a normal, well-formed reply —
/// guards against the new frame type leaving stray bytes behind the way the
/// historical `commit`/`list-channels` 0-byte-timeout defects did for other
/// previously-unhandled frame types.
#[test]
fn test_clear_history_then_status_no_desync() {
    let mut session = make_session_with_history(vec![(make_dm_entry(0x09, 1, b"hi"), false)]);
    session.clear_history().expect("clear_history must succeed");
    let status = session
        .query_status()
        .expect("status after clear_history must not hang/desync");
    assert_eq!(status.contact_count, 0);
}

// ── Magic-sync regression tests ───────────────────────────────────────────────

/// A transport that delivers pre-loaded bytes on recv() and silently discards
/// sent frames.  Used to inject log noise before real provisioning frames,
/// simulating the shared USB-serial channel used by the T-Deck firmware.
struct PreloadTransport {
    recv_buf: std::collections::VecDeque<u8>,
}

impl PreloadTransport {
    fn with_data(data: Vec<u8>) -> Self {
        Self {
            recv_buf: data.into(),
        }
    }
}

impl Transport for PreloadTransport {
    fn send(&mut self, _data: &[u8]) -> anyhow::Result<()> {
        // Discard — we are testing recv_frame sync, not the send path.
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> anyhow::Result<usize> {
        let n = buf.len().min(self.recv_buf.len());
        for b in buf[..n].iter_mut() {
            *b = self.recv_buf.pop_front().unwrap();
        }
        Ok(n)
    }
}

/// Regression: recv_frame must skip leading ESP-IDF log bytes and find the
/// real PROV_MAGIC frame start.
///
/// Before the fix, acc_buf filled with ~147 bytes of log text and the parser
/// misread log characters at offset 3–4 as a huge payload length, hanging
/// until timeout.  After the fix, find_magic_start discards the log preamble
/// and the frame is parsed correctly.
#[test]
fn test_recv_frame_skips_esp_log_preamble() {
    // ~150 bytes of realistic ESP-IDF log output.  Deliberately free of any
    // "MC" (0x4D 0x43) byte pair so there is no false PROV_MAGIC match.
    let log_noise: &[u8] = b"I (100) boot: ESP-IDF v5.1.0 second stage bootloader
I (200) prov_server: UNPROVISIONED
I (300) prov_server: ready
I (400) ui: pumping render loop
";

    // Build a real RSP_OK frame (what the device sends after add_contact).
    let real_frame = ok_frame();

    // Pre-load: log noise followed immediately by the real frame.
    let mut preloaded: Vec<u8> = log_noise.to_vec();
    preloaded.extend_from_slice(&real_frame);

    let transport = PreloadTransport::with_data(preloaded);
    // 2-second timeout — should complete immediately since all bytes are ready.
    let mut session = Session::with_timeout(transport, 2);

    // add_contact sends a frame (discarded by PreloadTransport) then calls
    // recv_frame, which must skip the log noise and find the RSP_OK frame.
    let contact_pubkey = [0x11_u8; 32];
    session
        .add_contact(&contact_pubkey, true, b"Alice")
        .expect("add_contact must succeed despite log preamble in recv buffer");
}

/// Regression: recv_frame must handle log noise that contains a false PROV_MAGIC
/// ("MC") sequence — e.g. if a device label or message text happens to contain
/// those bytes — by discarding 1 byte past the bad start and re-scanning.
#[test]
fn test_recv_frame_recovers_from_false_magic_in_log_noise() {
    // Craft noise that embeds a false "MC" (0x4D 0x43) followed by bytes that
    // would produce a CrcMismatch when decode_frame is attempted, then the
    // real RSP_OK frame follows.
    //
    // False-magic sequence: "MC" + some bytes that won't form a valid frame.
    let false_magic_noise: &[u8] = b"I (42) debug: Magic=MC trigger test
";

    // Real RSP_OK frame.
    let real_frame = ok_frame();

    let mut preloaded: Vec<u8> = false_magic_noise.to_vec();
    preloaded.extend_from_slice(&real_frame);

    let transport = PreloadTransport::with_data(preloaded);
    let mut session = Session::with_timeout(transport, 2);

    let contact_pubkey = [0x22_u8; 32];
    session
        .add_contact(&contact_pubkey, false, b"Bob")
        .expect("add_contact must recover from false PROV_MAGIC in log noise");
}

/// Regression (HIL: unexpected frame 0x82 while streaming history):
/// `export-history` at a realistic on-device entry count (38 — 18 DM + 20
/// GRP, matching the HIL repro) must stream to completion without the host
/// misreading a byte as an unexpected frame, even with ESP-IDF log noise
/// (including noise containing literal "MC" bytes and one deliberately
/// oversized false length) interleaved **between every single streamed
/// `FRAME_RSP_HISTORY_ENTRY`** — not just once before the whole response, the
/// way the existing `NoisyMockTransport` exercises it (it queues the entire
/// multi-frame response as one block per host command, so `find_magic_start`
/// only ever resyncs at the very front of that block; on real hardware the
/// serial-TX lock is released and reacquired **per frame**
/// (`admin_server::send_frame`), so a concurrent log line can land between
/// any two entry frames in the stream, not just before the first one).
///
/// Entries include several at the maximum 64-byte text length (73-byte wire
/// payload, `MAX_RSP_HISTORY_ENTRY_PAYLOAD`) — the boundary the `is_ours` byte
/// (commit 95be0ad)
/// pushed the payload out to — so a length-handling drift at exactly that
/// boundary would surface here.
#[test]
fn test_export_history_at_hil_scale_with_interleaved_log_noise() {
    // Noise block injected between every streamed frame. Includes a literal
    // "MC" substring (mirroring a plausible log tag) to exercise the
    // CRC-mismatch resync path repeatedly across the stream, not just once.
    fn noise_block(i: usize) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(format!("I ({i}) MC_HIST: read slot {i}\n").as_bytes());
        v
    }

    // Build 38 entries: 18 DM + 20 GRP, alternating is_ours, several at the
    // maximum 64-byte text length so some payloads hit the full 73-byte wire
    // maximum.
    let mut entries: Vec<(HistoryEntry, bool)> = Vec::new();
    for i in 0..18u32 {
        let text: Vec<u8> = if i % 3 == 0 {
            vec![b'D'; MAX_HISTORY_TEXT_LEN] // max-length payload (73-byte wire frame)
        } else {
            format!("dm message {i}").into_bytes()
        };
        entries.push((
            make_dm_entry((i % 256) as u8, 1_000_000 + i, &text),
            i % 2 == 0,
        ));
    }
    for i in 0..20u32 {
        let text: Vec<u8> = if i % 4 == 0 {
            vec![b'G'; MAX_HISTORY_TEXT_LEN] // max-length payload (73-byte wire frame)
        } else {
            format!("grp message {i}").into_bytes()
        };
        entries.push((
            make_grp_entry((i % 256) as u8, 2_000_000 + i, &text),
            i % 2 == 1,
        ));
    }
    assert_eq!(entries.len(), 38, "HIL repro scale: 18 DM + 20 GRP");

    // Manually encode the streamed response with noise between every frame —
    // PreloadTransport delivers exactly these bytes regardless of what the
    // session sends, giving full control over frame placement (unlike
    // MockDevice/MockTransport, which only ever emit clean back-to-back
    // frames for a streamed response).
    let mut stream: Vec<u8> = Vec::new();
    for (idx, (entry, is_ours)) in entries.iter().enumerate() {
        stream.extend_from_slice(&noise_block(idx));
        let mut pbuf = [0u8; MAX_RSP_HISTORY_ENTRY_PAYLOAD + 1];
        let plen = encode_rsp_history_entry(idx as u8, entry, *is_ours, &mut pbuf);
        assert!(
            plen > 0,
            "entry {idx} must encode (buffer must be large enough)"
        );
        let mut fbuf = [0u8; MAX_RSP_HISTORY_ENTRY_PAYLOAD + 16];
        let n = encode_frame(FRAME_RSP_HISTORY_ENTRY, &pbuf[..plen], &mut fbuf);
        stream.extend_from_slice(&fbuf[..n]);
    }
    stream.extend_from_slice(&noise_block(999));
    let mut done_buf = [0u8; 16];
    let n = encode_frame(FRAME_RSP_HISTORY_DONE, &[], &mut done_buf);
    stream.extend_from_slice(&done_buf[..n]);

    let transport = PreloadTransport::with_data(stream);
    let mut session = Session::with_timeout(transport, 5);

    let got = session
        .export_history()
        .expect("export_history must stream to completion at HIL scale with interleaved log noise");

    assert_eq!(
        got.len(),
        38,
        "all 38 entries must be received, none dropped by a false resync"
    );
    for (i, ((want_entry, want_ours), (got_entry, got_ours))) in
        entries.iter().zip(got.iter()).enumerate()
    {
        assert_eq!(
            got_entry.sender_hash, want_entry.sender_hash,
            "entry {i} sender_hash"
        );
        assert_eq!(
            got_entry.msg_type, want_entry.msg_type,
            "entry {i} msg_type"
        );
        assert_eq!(
            got_entry.timestamp, want_entry.timestamp,
            "entry {i} timestamp"
        );
        assert_eq!(
            &got_entry.text[..got_entry.text_len as usize],
            &want_entry.text[..want_entry.text_len as usize],
            "entry {i} text"
        );
        assert_eq!(got_ours, want_ours, "entry {i} is_ours");
    }
}

/// Regression (HIL: unexpected frame 0x82 while streaming history): a
/// stray, well-formed `RSP_STATUS` + `RSP_IDENTITY` pair sitting ahead of the
/// real history stream in the receive buffer — e.g. leftover replies to an
/// earlier CLI invocation's `status` command that raced `SerialTransport::
/// open`'s one-time kernel-buffer clear (see `Session::export_history`'s doc
/// comment for the exact mechanism) — must be skipped, not treated as a fatal
/// "unexpected frame" desync. This is the literal reported symptom: `0x82` is
/// `FRAME_RSP_STATUS`, a real, valid frame type, not corrupted bytes.
#[test]
fn test_export_history_tolerates_stray_leading_status_and_identity_frames() {
    let pubkey = [0x67_u8; 32];
    let status = RspStatusPayload {
        provisioned: true,
        pubkey,
        contact_count: 0,
        channel_count: 0,
        gps_has_fix: false,
        gps_lat_e7: 0,
        gps_lon_e7: 0,
        gps_fix_age_secs: 0,
        gps_clock_synced: false,
        gps_clock_sync_age_secs: 0,
        battery_percent: 0,
        battery_charging: false,
        battery_raw_mv: 0,
        battery_held_raw_mv: 0,
    };
    let mut stray: Vec<u8> = Vec::new();
    let mut sbuf = [0u8; 64];
    let slen = encode_rsp_status(&status, &mut sbuf);
    let mut sfbuf = [0u8; 80];
    let n = encode_frame(FRAME_RSP_STATUS, &sbuf[..slen], &mut sfbuf);
    stray.extend_from_slice(&sfbuf[..n]);

    let mut ibuf = [0u8; 64];
    let ilen = encode_rsp_identity(&pubkey, b"Alex's MeshCadet", &mut ibuf);
    let mut ifbuf = [0u8; 80];
    let n = encode_frame(FRAME_RSP_IDENTITY, &ibuf[..ilen], &mut ifbuf);
    stray.extend_from_slice(&ifbuf[..n]);

    // The real history stream: two entries, then DONE.
    let entries = [
        (make_dm_entry(0x10, 100, b"first"), false),
        (make_grp_entry(0x20, 200, b"second"), true),
    ];
    let mut stream = stray;
    for (idx, (entry, is_ours)) in entries.iter().enumerate() {
        let mut pbuf = [0u8; MAX_RSP_HISTORY_ENTRY_PAYLOAD + 1];
        let plen = encode_rsp_history_entry(idx as u8, entry, *is_ours, &mut pbuf);
        let mut fbuf = [0u8; MAX_RSP_HISTORY_ENTRY_PAYLOAD + 16];
        let n = encode_frame(FRAME_RSP_HISTORY_ENTRY, &pbuf[..plen], &mut fbuf);
        stream.extend_from_slice(&fbuf[..n]);
    }
    let mut done_buf = [0u8; 16];
    let n = encode_frame(FRAME_RSP_HISTORY_DONE, &[], &mut done_buf);
    stream.extend_from_slice(&done_buf[..n]);

    let transport = PreloadTransport::with_data(stream);
    let mut session = Session::with_timeout(transport, 2);

    let got = session
        .export_history()
        .expect("export_history must skip stray leading RSP_STATUS/RSP_IDENTITY frames, not bail");

    assert_eq!(
        got.len(),
        2,
        "both real history entries must still be returned"
    );
    assert_eq!(got[0].0.sender_hash, 0x10);
    assert_eq!(got[1].0.sender_hash, 0x20);
    assert!(
        got[1].1,
        "second entry's is_ours must round-trip through the stray-frame prefix"
    );
}

// ── NoisyMockTransport — full round-trip with interleaved log noise ───────────

/// A `MockTransport` variant that injects realistic T-Deck firmware log noise
/// (ASCII lines + false PROV_MAGIC sequences) before **every** device response.
///
/// Purpose: verify that `Session::recv_frame`'s three resync paths all function
/// correctly throughout a multi-step provisioning session, not just for the
/// first frame.  The noise exercised before each response:
///
/// ```text
/// (A) ASCII log line — no 'MC' bytes (exercises find_magic_start skip)
/// (B) [MC|type=0x01|plen=0|CRC=0x0000] — bad-CRC false MC (session.rs:143)
/// (C) ASCII log line — no 'MC' bytes
/// (D) [MC|type=0x00|plen_lo=0xF4|plen_hi=0x01|0xAA|0xBB] — plen=500>72 (session.rs:129)
/// (E) ASCII log line — no 'MC' bytes
/// ```
struct NoisyMockTransport {
    device: MockDevice,
    send_buf: Vec<u8>,
    recv_buf: VecDeque<u8>,
}

impl NoisyMockTransport {
    fn new(device: MockDevice) -> Self {
        Self {
            device,
            send_buf: Vec::new(),
            recv_buf: VecDeque::new(),
        }
    }

    /// Noise bytes prepended before every device response.
    ///
    /// Exercises all three `recv_frame` resync paths in a single burst:
    ///
    ///   **(A)+(C)+(E)** `find_magic_start` discards pure-ASCII bytes that
    ///     contain no `0x4D` ('M') byte.
    ///
    ///   **(B)** CRC-mismatch 1-byte-advance (session.rs:143): false MC where
    ///     plen = 0 (≤ `MAX_RSP_HISTORY_ENTRY_PAYLOAD` = 72), but the CRC bytes
    ///     are `0x00 0x00` which won't match the actual CRC-16/ARC of the five
    ///     header bytes `[0x4D, 0x43, 0x01, 0x00, 0x00]`.
    ///
    ///   **(D)** Large-plen guard (session.rs:129): plen = `0xF4 | (0x01 << 8)`
    ///     = 500 > 72, so the fake magic start is discarded with `drain(..1)` and
    ///     `continue` — `decode_frame` is never called.
    fn make_noise() -> Vec<u8> {
        let mut v: Vec<u8> = Vec::new();
        // (A) ASCII log line — every byte is <0x4D or >0x4D; no 'M' (0x4D).
        v.extend_from_slice(b"I (100) prov_server: UNPROVISIONED\n");
        // (B) False MC with plen=0 and CRC=0x0000 → CrcMismatch → 1-byte advance.
        v.extend_from_slice(&[0x4D, 0x43, 0x01, 0x00, 0x00, 0x00, 0x00]);
        // (C) Another ASCII log line — no 0x4D.
        v.extend_from_slice(b"I (101) boot: starting\n");
        // (D) False MC with plen=500 (> MAX_RSP_HISTORY_ENTRY_PAYLOAD=72) → drained.
        v.extend_from_slice(&[0x4D, 0x43, 0x00, 0xF4, 0x01, 0xAA, 0xBB]);
        // (E) Trailing ASCII — no 0x4D.
        v.extend_from_slice(b"\nI (102) ui: render tick\n");
        v
    }
}

impl Transport for NoisyMockTransport {
    fn send(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.send_buf.extend_from_slice(data);

        loop {
            if self.send_buf.len() < 5 {
                break;
            }
            let plen = (self.send_buf[3] as usize) | ((self.send_buf[4] as usize) << 8);
            let total = 7 + plen;
            if self.send_buf.len() < total {
                break;
            }
            let (ft, payload_slice) = decode_frame(&self.send_buf[..total])
                .map_err(|e| anyhow::anyhow!("noisy-mock: host sent bad frame: {:?}", e))?;
            let payload: Vec<u8> = payload_slice.to_vec();
            self.send_buf.drain(..total);

            let response = self.device.handle(ft, &payload);
            // Prepend noise before every device response so recv_frame's resync
            // logic is exercised on every single operation in the session.
            self.recv_buf.extend(NoisyMockTransport::make_noise());
            self.recv_buf.extend(response);
        }

        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> anyhow::Result<usize> {
        let n = buf.len().min(self.recv_buf.len());
        for b in buf[..n].iter_mut() {
            *b = self.recv_buf.pop_front().unwrap();
        }
        Ok(n)
    }
}

/// Regression: the full provisioning round-trip must succeed when the transport
/// injects realistic T-Deck firmware log noise (ASCII + both species of false
/// PROV_MAGIC) before **every** device response frame.
///
/// This catches regressions in code paths that the clean `MockTransport` never
/// exercises:
///
///  - `find_magic_start` — discards the ASCII prefix before the first false MC
///    in every noise block.
///  - Large-plen guard (session.rs:129) — `plen=500 > MAX_RSP_HISTORY_ENTRY_PAYLOAD`
///    is rejected without calling `decode_frame`; the false magic is advanced past
///    with `drain(..1)` + `continue`.
///  - CRC-mismatch 1-byte-advance (session.rs:143) — the zero-CRC false MC is
///    tried, fails CRC, and is stepped past.
///
/// If any of these paths regresses, `Session` hangs until timeout and the test
/// fails with a timeout error.
#[test]
fn test_full_provisioning_round_trip_with_log_noise() {
    let pubkey = [0xDE_u8; 32];
    let device = MockDevice::new(pubkey);
    let transport = NoisyMockTransport::new(device);
    // 5-second timeout: all bytes are pre-loaded so resolution takes microseconds;
    // the budget absorbs CI scheduling jitter without letting a hang run forever.
    let mut session = Session::with_timeout(transport, 5);

    // Step 1: query status — noise injected before RSP_STATUS.
    let status = session
        .query_status()
        .expect("query_status must succeed with log noise in stream");
    assert_eq!(
        status.pubkey, pubkey,
        "pubkey must round-trip through noisy stream"
    );
    assert!(!status.provisioned, "device must start unprovisioned");
    assert_eq!(status.contact_count, 0);
    assert_eq!(status.channel_count, 0);

    // Step 2: add contact — noise injected before RSP_OK.
    let alice = [0xA1_u8; 32];
    session
        .add_contact(&alice, true, b"Alice")
        .expect("add_contact must succeed with log noise between frames");

    // Step 3: add primary channel — noise injected before RSP_OK.
    let secret = [0xFA_u8; 32];
    session
        .add_channel(&secret, 32, /*primary=*/ true, b"family")
        .expect("add_channel must succeed with log noise between frames");

    // Step 4: commit — noise injected before RSP_OK.
    session
        .commit()
        .expect("commit must succeed with log noise between frames");

    // Step 5: final status — noise injected before RSP_STATUS.
    let final_status = session
        .query_status()
        .expect("final query_status must succeed with log noise");
    assert!(
        final_status.provisioned,
        "device must be provisioned after commit"
    );
    assert_eq!(final_status.contact_count, 1, "one contact must be staged");
    assert_eq!(final_status.channel_count, 1, "one channel must be staged");
    assert_eq!(
        final_status.pubkey, pubkey,
        "pubkey must survive full noisy session"
    );
}

// ── DroppedSendTransport — retry regression ───────────────────────────────────

/// A transport that silently drops the first `drops` complete frames sent by
/// the host (simulating the device not yet listening — mid-boot, USB not ready,
/// or a marginal cable glitch) and only processes frames once the drop budget
/// is exhausted.
///
/// Purpose: verify that `Session::send_recv_with_retry` actually re-sends the
/// command frame and succeeds once the device becomes reachable, even when
/// earlier sends were silently lost.
struct DroppedSendTransport {
    device: MockDevice,
    /// Number of complete frames still to be silently dropped.
    drops_remaining: usize,
    /// Accumulation buffer for host→device bytes.
    send_buf: Vec<u8>,
    /// Response bytes queued for the session to read.
    recv_buf: VecDeque<u8>,
}

impl DroppedSendTransport {
    fn new(device: MockDevice, drops: usize) -> Self {
        Self {
            device,
            drops_remaining: drops,
            send_buf: Vec::new(),
            recv_buf: VecDeque::new(),
        }
    }
}

impl Transport for DroppedSendTransport {
    fn send(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.send_buf.extend_from_slice(data);

        // Try to decode and consume a complete frame from send_buf.
        loop {
            if self.send_buf.len() < 5 {
                break;
            }
            let plen = (self.send_buf[3] as usize) | ((self.send_buf[4] as usize) << 8);
            let total = 7 + plen;
            if self.send_buf.len() < total {
                break;
            }
            let (ft, payload_slice) = decode_frame(&self.send_buf[..total])
                .map_err(|e| anyhow::anyhow!("drop-transport: host sent bad frame: {:?}", e))?;
            let payload: Vec<u8> = payload_slice.to_vec();
            self.send_buf.drain(..total);

            if self.drops_remaining > 0 {
                // Silently drop — simulate device mid-boot, not yet reading.
                self.drops_remaining -= 1;
                continue;
            }

            // Device is ready: process the frame and queue the response.
            let response = self.device.handle(ft, &payload);
            self.recv_buf.extend(response);
        }

        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> anyhow::Result<usize> {
        let n = buf.len().min(self.recv_buf.len());
        for b in buf[..n].iter_mut() {
            *b = self.recv_buf.pop_front().unwrap();
        }
        Ok(n)
    }
}

/// Regression: `query_status` must succeed even when the first command frame is
/// silently dropped (simulating a device that is not yet ready to receive —
/// e.g. USB enumeration lag or a kernel modem-line glitch after port open).
///
/// `DroppedSendTransport` drops the first 2 complete host frames; the session's
/// `send_recv_with_retry` must resend until the device is reachable.
///
/// Timing note: the test uses `retry_attempt_ms=50` so each dropped attempt
/// times out in ~50 ms instead of the production 500 ms; total test time is
/// therefore ~100 ms for 2 drops.
#[test]
fn test_query_status_retries_on_dropped_frame() {
    let pubkey = [0xAB_u8; 32];
    let device = MockDevice::new(pubkey);
    // Drop the first 2 sends; respond on the 3rd.
    let transport = DroppedSendTransport::new(device, 2);
    // frame_secs=1, retry_attempt_ms=50, retry_total_ms=5000
    // 2 dropped attempts × ~50 ms each = ~100 ms actual test time.
    let mut session = Session::with_retry_params(transport, 1, 50, 5_000);

    let status = session
        .query_status()
        .expect("query_status must succeed after 2 dropped frames via retry");

    assert_eq!(
        status.pubkey, pubkey,
        "pubkey must round-trip through retry"
    );
    assert!(
        !status.provisioned,
        "fresh mock device must not be provisioned"
    );
}

/// Acceptance: `add_channel` must accept a 16-byte (128-bit) secret (`key_len=16`),
/// return RSP_OK, and have status show channels incremented.
#[test]
fn test_add_channel_128bit_secret() {
    let mut session = make_session_v2([0x22_u8; 32]);

    // 128-bit channel secret: first 16 bytes significant; last 16 zero-padded.
    let mut channel_secret = [0u8; 32];
    channel_secret[..16].copy_from_slice(&[0x6D_u8; 16]);

    session
        .add_channel(&channel_secret, 16, /*primary=*/ true, b"family128")
        .expect("add_channel with 128-bit secret (key_len=16) must return RSP_OK");

    let status = session
        .query_status()
        .expect("query_status after 128-bit channel add");
    assert_eq!(
        status.channel_count, 1,
        "channel count must increment after 128-bit add"
    );
}

/// Regression: `send_and_expect_ok` (used by add_contact, add_channel, set_pin,
/// etc.) must also retry on a dropped frame.
///
/// Uses `DroppedSendTransport` dropping the first 1 frame.
#[test]
fn test_add_contact_retries_on_dropped_frame() {
    let device = MockDevice::new([0x11_u8; 32]);
    let transport = DroppedSendTransport::new(device, 1);
    let mut session = Session::with_retry_params(transport, 1, 50, 5_000);

    let key = [0xCC_u8; 32];
    session
        .add_contact(&key, true, b"Alice")
        .expect("add_contact must succeed after 1 dropped frame via retry");
}

// ── CLI surface ────────────────────────────────────────────────────────────────

/// Acceptance: `meshcadet --help` must list `clear-history` as a subcommand —
/// the host CLI's contract for exposing the new command.
#[test]
fn test_cli_help_lists_clear_history() {
    let exe = env!("CARGO_BIN_EXE_meshcadet");
    let output = std::process::Command::new(exe)
        .arg("--help")
        .output()
        .expect("meshcadet --help must run");
    assert!(output.status.success(), "--help must exit successfully");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("clear-history"),
        "--help output must list the clear-history subcommand:\n{stdout}"
    );
}

/// Acceptance: `meshcadet clear-history --help` must itself run cleanly
/// (valid subcommand wiring, no missing-arg surprises since the command
/// takes none).
#[test]
fn test_cli_clear_history_subcommand_help() {
    let exe = env!("CARGO_BIN_EXE_meshcadet");
    let output = std::process::Command::new(exe)
        .args(["--port", "/dev/null", "clear-history", "--help"])
        .output()
        .expect("meshcadet clear-history --help must run");
    assert!(
        output.status.success(),
        "clear-history --help must exit successfully"
    );
}
