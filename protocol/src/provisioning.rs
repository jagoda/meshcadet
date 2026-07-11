// SPDX-License-Identifier: GPL-3.0-only
//! USB-serial provisioning wire protocol.
//!
//! Shared by the MeshCadet firmware and the admin host CLI (`host/`). Defines
//! every frame type, the encode/decode functions, and all payload structures.
//! See `docs/adr/0002-provisioning-wire-format.md` for the format rationale.
//!
//! # Frame layout (ADR-0002)
//!
//! ```text
//! byte 0-1  MAGIC        = 0x4D 0x43  ("MC")
//! byte 2    frame_type   = FrameType constant
//! byte 3-4  payload_len  = little-endian u16 (bytes in the payload field)
//! byte 5..  payload      = payload_len bytes (frame-type specific)
//! last 2    crc16        = CRC-16/ARC over bytes [0 .. 5+payload_len]
//! ```
//!
//! Total overhead: 7 bytes per frame.  Maximum payload: 65 535 bytes (in
//! practice every provisioning message fits in under 100 bytes).
//!
//! # Security model
//!
//! Provisioning is gated on **physical USB possession** (charter ADR-0001 §4).
//! No transport-layer encryption is applied to the provisioning channel: the
//! cable IS the authentication.  The CRC catches accidental corruption; it is
//! not a MAC.
//!
//! # no_std compatibility
//!
//! This module is `no_std` (the protocol crate sets `#![cfg_attr(not(test), no_std)]`).
//! All payloads are encoded into/decoded from caller-supplied byte slices; no
//! heap allocation is used.

// ── Constants ────────────────────────────────────────────────────────────────

/// Frame synchronisation magic bytes ("MC").
pub const PROV_MAGIC: [u8; 2] = [0x4D, 0x43];

/// Overhead bytes in every frame: magic(2) + type(1) + len(2) + crc(2).
pub const FRAME_OVERHEAD: usize = 7;

/// Maximum name length for contact display names and channel names (bytes).
pub const MAX_NAME_LEN: usize = 32;

/// Maximum PIN length (bytes / UTF-8 code units).
pub const MAX_PIN_LEN: usize = 16;

/// Maximum error message length in an RspError payload.
pub const MAX_ERR_MSG_LEN: usize = 64;

// ── Frame type constants ─────────────────────────────────────────────────────
//
// 0x01–0x7F  host → device (commands)
// 0x80–0xFF  device → host (responses)

/// Query device provisioning status.  Payload: empty.
pub const FRAME_QUERY_STATUS: u8 = 0x01;

/// Query the device's configured contacts (streamed enumeration).  Payload: empty.
///
/// The device replies with N × [`FRAME_RSP_CONTACT`] then
/// [`FRAME_RSP_CONTACTS_DONE`], mirroring the history-export streaming pattern.
pub const FRAME_QUERY_CONTACTS: u8 = 0x02;

/// Query the device's configured channels (streamed enumeration).  Payload: empty.
///
/// The device replies with N × [`FRAME_RSP_CHANNEL`] then
/// [`FRAME_RSP_CHANNELS_DONE`].
pub const FRAME_QUERY_CHANNELS: u8 = 0x03;

/// Add a contact entry.
pub const FRAME_ADD_CONTACT: u8 = 0x10;
/// Delete a contact entry by pubkey.
pub const FRAME_DEL_CONTACT: u8 = 0x11;

/// Add (or replace) a channel entry.
pub const FRAME_ADD_CHANNEL: u8 = 0x20;
/// Delete a channel entry by its full 32-byte secret.
pub const FRAME_DEL_CHANNEL: u8 = 0x21;

// NOTE: 0x30 (formerly FRAME_SET_RADIO_PRESET) and 0x60 (formerly
// FRAME_SET_LOCKS) are retired frame types, removed after an audit found
// neither had a
// firmware consumer: the radio preset is a hard-locked ADR-0001 interop
// requirement (Radio::init() never reads a config value), and no code
// anywhere branched on the lock-flag bits. Do not reuse these byte values for
// new frame types without re-auditing for the same reasons.

/// Set notification defaults (visual + audible flags).
pub const FRAME_SET_NOTIF_DEFAULTS: u8 = 0x40;

/// Set the admin PIN.
pub const FRAME_SET_PIN: u8 = 0x50;

/// Set the device's display name (persisted via the identity/NVS store, not
/// the provisioning config blob — a device name is a property of the node's
/// identity, and applies immediately regardless of provisioned state).
pub const FRAME_SET_DEVICE_NAME: u8 = 0x51;

/// Commit: mark provisioning complete and persist config to flash.
/// Payload: empty.  Device reboots after responding RspOk.
pub const FRAME_COMMIT_PROVISIONING: u8 = 0x70;

/// Response: command accepted.  Payload: empty.
pub const FRAME_RSP_OK: u8 = 0x80;
/// Response: command rejected.  Payload: RspErrorPayload.
pub const FRAME_RSP_ERROR: u8 = 0x81;
/// Response: device status (answer to QueryStatus).
pub const FRAME_RSP_STATUS: u8 = 0x82;
/// Response: device public identity (pubkey + pub_hash).
pub const FRAME_RSP_IDENTITY: u8 = 0x83;
/// Request: host asks device to stream conversation history.
pub const FRAME_EXPORT_HISTORY: u8 = 0x71;
/// Request: host asks device to erase ALL persisted conversation history (both
/// DM contacts and channels, both directions — every `HistoryEntry` regardless
/// of `is_ours`). Payload: empty. Device replies `RSP_OK` on success or
/// `RSP_ERROR` on a flash/NVS failure — mirrors `FRAME_COMMIT_PROVISIONING`'s
/// empty-payload / single-ack shape rather than the streamed `FRAME_EXPORT_HISTORY`
/// pattern, since there is nothing to enumerate back to the host.
///
/// DESIGN DECISION: the clear
/// takes effect on the flash-backed `mc_hist` store immediately, but the
/// live in-memory UI state (`ui::UiRuntime`'s `messages`/`unread` maps,
/// owned by the main thread) is NOT touched by this frame — only
/// `admin_server`'s thread (which owns the `HISTORY` mutex) handles it, and
/// it has no channel back to the UI thread's state today. A reboot re-hydrates
/// the UI from the now-empty store (see `main.rs`'s boot-hydrate step), which
/// is the existing, already-documented pattern for every other runtime edit in
/// this protocol (`ADD_CONTACT`/`ADD_CHANNEL`/etc. also only take full effect
/// after a reboot — see their host CLI "note: reboot the device..." messages).
/// The host CLI's `clear-history` output tells the user a reboot is needed.
pub const FRAME_CLEAR_HISTORY: u8 = 0x72;
/// Response: one history entry in a streamed export sequence.
pub const FRAME_RSP_HISTORY_ENTRY: u8 = 0x84;
/// Response: terminal frame of a history export stream (no payload).
pub const FRAME_RSP_HISTORY_DONE: u8 = 0x85;
/// Response: one contact entry in a streamed contact enumeration.
pub const FRAME_RSP_CONTACT: u8 = 0x86;
/// Response: terminal frame of a contact enumeration stream (no payload).
pub const FRAME_RSP_CONTACTS_DONE: u8 = 0x87;
/// Response: one channel entry in a streamed channel enumeration.
pub const FRAME_RSP_CHANNEL: u8 = 0x88;
/// Response: terminal frame of a channel enumeration stream (no payload).
pub const FRAME_RSP_CHANNELS_DONE: u8 = 0x89;

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors from provisioning frame encode/decode operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProvError {
    /// Buffer is too short for a complete frame or payload.
    TruncatedFrame,
    /// Frame does not begin with `PROV_MAGIC`.
    BadMagic,
    /// CRC-16 check failed (corrupted frame).
    CrcMismatch,
    /// Payload is shorter than required for this frame type.
    TruncatedPayload,
    /// A name field exceeds `MAX_NAME_LEN`.
    NameTooLong,
    /// The PIN exceeds `MAX_PIN_LEN`.
    PinTooLong,
}

// ── Payload structs ───────────────────────────────────────────────────────────

/// Payload for `FRAME_ADD_CONTACT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddContactPayload {
    /// Ed25519 public key (32 bytes) — the contact identity.
    pub pubkey: [u8; 32],
    /// Whether this contact may pull telemetry (GPS fix) from us.
    pub telemetry_enable: bool,
    /// UTF-8 display name, padded with zeros to `MAX_NAME_LEN`.
    pub display_name: [u8; MAX_NAME_LEN],
    /// Actual byte length of `display_name` (0 ⇒ use pub_hash as label).
    pub display_name_len: u8,
}

/// Payload for `FRAME_DEL_CONTACT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelContactPayload {
    /// Full Ed25519 public key of the contact to remove.
    pub pubkey: [u8; 32],
}

/// Payload for `FRAME_ADD_CHANNEL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddChannelPayload {
    /// Symmetric channel secret (32 bytes).
    /// For 128-bit channels only bytes `[0..16]` carry the secret;
    /// bytes `[16..32]` are zero-padded.
    pub secret: [u8; 32],
    /// Number of secret bytes that are significant: 16 (128-bit) or 32 (256-bit).
    ///
    /// The device uses this to select the correct channel-hash computation:
    /// - `16`: `SHA-256(secret[0..16])[0]`
    /// - `32`: `SHA-256(secret)[0]`
    pub key_len: u8,
    /// If `true`, this channel is the primary (default) outgoing channel.
    pub primary: bool,
    /// UTF-8 channel name, padded with zeros to `MAX_NAME_LEN`.
    pub name: [u8; MAX_NAME_LEN],
    /// Actual byte length of `name`.
    pub name_len: u8,
}

/// Payload for `FRAME_DEL_CHANNEL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelChannelPayload {
    /// Full 32-byte channel secret identifying the channel to remove.
    pub secret: [u8; 32],
}

/// Payload for `FRAME_SET_NOTIF_DEFAULTS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotifDefaultsPayload {
    /// Enable visual notifications (screen flash / LED) on message receipt.
    pub visual: bool,
    /// Enable audible notifications (buzzer / speaker) on message receipt.
    pub audible: bool,
}

/// Payload for `FRAME_SET_PIN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetPinPayload {
    /// UTF-8 PIN bytes, padded with zeros to `MAX_PIN_LEN`.
    pub pin: [u8; MAX_PIN_LEN],
    /// Actual byte length of the PIN (0 ⇒ PIN not set / PIN lock disabled).
    pub pin_len: u8,
}

/// Payload for `FRAME_SET_DEVICE_NAME`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetDeviceNamePayload {
    /// UTF-8 device display name, padded with zeros to `MAX_NAME_LEN`.
    pub name: [u8; MAX_NAME_LEN],
    /// Actual byte length of `name` (0 ⇒ clear the stored name).
    pub name_len: u8,
}

/// Payload for `FRAME_RSP_STATUS` (response to `FRAME_QUERY_STATUS`).
///
/// The `gps_*` fields mirror the on-device admin-menu GPS status view
/// (read-only: fix state, coordinates + age, clock-sync state + age) so the
/// host `status` command surfaces the same three facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RspStatusPayload {
    /// `true` if the device has been provisioned (CommitProvisioning was completed).
    pub provisioned: bool,
    /// Device's Ed25519 public key (32 bytes).
    pub pubkey: [u8; 32],
    /// Number of provisioned contacts (0 if unprovisioned).
    pub contact_count: u8,
    /// Number of provisioned channels (0 if unprovisioned).
    pub channel_count: u8,
    /// `true` if the GPS driver has ever obtained a fix since boot.
    pub gps_has_fix: bool,
    /// Latitude in units of 1e-7 degrees. Only meaningful when `gps_has_fix`.
    pub gps_lat_e7: i32,
    /// Longitude in units of 1e-7 degrees. Only meaningful when `gps_has_fix`.
    pub gps_lon_e7: i32,
    /// Seconds since the cached fix was captured. Only meaningful when `gps_has_fix`.
    pub gps_fix_age_secs: u32,
    /// `true` if the system clock has been set from a valid GPS date+time
    /// sentence ($GPRMC/$GNRMC) since boot.
    pub gps_clock_synced: bool,
    /// Seconds since the system clock was last synced from GPS. Only
    /// meaningful when `gps_clock_synced`.
    pub gps_clock_sync_age_secs: u32,
    /// Battery charge percentage, `0..=100`. Mirrors the same
    /// `battery::BatteryStatus` the radio telemetry RESPONSE and the
    /// admin-menu screen read — see that firmware module's docs for how it
    /// is derived (ADC voltage divider; no fuel-gauge IC on this board). `0`
    /// before the first ADC sample / on an unprovisioned device (battery ADC
    /// not yet initialised at that boot stage).
    pub battery_percent: u8,
    /// `true` if the battery is inferred to be charging. See
    /// `battery::BatteryStatus` docs (firmware) for the inference mechanism
    /// and its limits — this is a best-effort derived signal, not a direct
    /// hardware read.
    pub battery_charging: bool,
    /// Last live (post-divider, averaged) ADC millivolt reading —
    /// diagnostic-only field added for the 2026-07-05 ADC-calibration
    /// investigation (see firmware `battery` module docs' "raw_mv" section).
    /// Unlike `battery_percent`, this is NOT frozen while charging: it always
    /// reflects the current live voltage, so it can be compared directly
    /// against a multimeter reading or the charge-status LED. `0` before the
    /// first ADC sample / on an unprovisioned device.
    pub battery_raw_mv: u16,
    /// Last known non-charge-inflated ("resting") millivolt reading — the
    /// same frozen basis `battery_percent` is derived from, but as raw
    /// millivolts rather than a lossy-rounded percentage (see firmware
    /// `battery` module docs' "held_raw_mv" section). Added for the
    /// full-anchor-and-held-raw-exposure work (2026-07-05): because USB
    /// carries both the host CLI UART and charge power on this board, any
    /// live CLI read of `battery_raw_mv` is necessarily taken while the
    /// charger's contaminated rail is on the pin. This field is instead
    /// frozen the instant charging starts (same latch as `battery_percent`),
    /// so reading it after a brief unplug/replug cycle (to re-attach the
    /// CLI) surfaces the true pre-charge pack voltage, contamination-free.
    /// `0` before the first ADC sample / on an unprovisioned device.
    pub battery_held_raw_mv: u16,
}

/// Payload for `FRAME_RSP_IDENTITY`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RspIdentityPayload {
    /// Device's Ed25519 public key (32 bytes).
    pub pubkey: [u8; 32],
    /// 1-byte routing hash = `pubkey[0]`.
    pub pub_hash: u8,
    /// UTF-8 device display name, zero-padded to `MAX_NAME_LEN`.
    pub device_name: [u8; MAX_NAME_LEN],
    /// Actual byte length of `device_name` (0 ⇒ no name set; callers fall
    /// back to a pub_hash-derived label).
    pub device_name_len: u8,
}

/// Payload for `FRAME_RSP_CONTACT` (one entry in a contact enumeration).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RspContactPayload {
    /// 0-based index of this contact in the device's configured list.
    pub index: u8,
    /// Contact Ed25519 public key (32 bytes) — the contact identity.
    pub pubkey: [u8; 32],
    /// Whether this contact may pull our GPS telemetry.
    pub telemetry_enable: bool,
    /// UTF-8 display name, zero-padded to `MAX_NAME_LEN`.
    pub display_name: [u8; MAX_NAME_LEN],
    /// Actual byte length of `display_name` (0 ⇒ use pub_hash as label).
    pub display_name_len: u8,
}

/// Payload for `FRAME_RSP_CHANNEL` (one entry in a channel enumeration).
///
/// The 32-byte channel secret is intentionally **not** echoed back: it is
/// symmetric key material, and the on-air `channel_hash` (1-byte identifier)
/// plus the name are sufficient to verify "matching what was provisioned".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RspChannelPayload {
    /// 0-based index of this channel in the device's configured list.
    pub index: u8,
    /// 1-byte on-air channel hash identifier (`SHA-256(secret[..key_len])[0]`).
    pub channel_hash: u8,
    /// Number of significant secret bytes: 16 (128-bit) or 32 (256-bit).
    pub key_len: u8,
    /// If `true`, this channel is the primary (default) outgoing channel.
    pub primary: bool,
    /// UTF-8 channel name, zero-padded to `MAX_NAME_LEN`.
    pub name: [u8; MAX_NAME_LEN],
    /// Actual byte length of `name`.
    pub name_len: u8,
}

/// Payload for `FRAME_RSP_ERROR`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RspErrorPayload {
    /// Error code (application-defined; 0 = generic error).
    pub error_code: u8,
    /// UTF-8 error message, padded with zeros to `MAX_ERR_MSG_LEN`.
    pub msg: [u8; MAX_ERR_MSG_LEN],
    /// Actual byte length of `msg`.
    pub msg_len: u8,
}

// ── Frame-level encode / decode ───────────────────────────────────────────────

/// Encode a provisioning frame into `out`.
///
/// Writes `MAGIC | frame_type | len_lo | len_hi | payload | crc_lo | crc_hi`.
/// Returns the total number of bytes written (= `FRAME_OVERHEAD + payload.len()`).
///
/// `out` must be at least `FRAME_OVERHEAD + payload.len()` bytes.
pub fn encode_frame(frame_type: u8, payload: &[u8], out: &mut [u8]) -> usize {
    let plen = payload.len();
    out[0] = PROV_MAGIC[0];
    out[1] = PROV_MAGIC[1];
    out[2] = frame_type;
    out[3] = (plen & 0xFF) as u8;
    out[4] = ((plen >> 8) & 0xFF) as u8;
    if plen > 0 {
        out[5..5 + plen].copy_from_slice(payload);
    }
    let crc = crc16(&out[..5 + plen]);
    out[5 + plen] = (crc & 0xFF) as u8;
    out[5 + plen + 1] = ((crc >> 8) & 0xFF) as u8;
    FRAME_OVERHEAD + plen
}

/// Decode a provisioning frame from `buf`.
///
/// On success, returns `(frame_type, payload_slice)` where `payload_slice` is a
/// sub-slice of `buf` (zero-copy).
///
/// On failure:
/// - `BadMagic` — `buf` does not start with `PROV_MAGIC`.
/// - `TruncatedFrame` — `buf` is too short for the complete frame.
/// - `CrcMismatch` — the trailing CRC-16 does not match.
pub fn decode_frame(buf: &[u8]) -> Result<(u8, &[u8]), ProvError> {
    if buf.len() < FRAME_OVERHEAD {
        return Err(ProvError::TruncatedFrame);
    }
    if buf[0] != PROV_MAGIC[0] || buf[1] != PROV_MAGIC[1] {
        return Err(ProvError::BadMagic);
    }
    let frame_type = buf[2];
    let plen = buf[3] as usize | ((buf[4] as usize) << 8);
    let total = FRAME_OVERHEAD + plen;
    if buf.len() < total {
        return Err(ProvError::TruncatedFrame);
    }
    let crc_expected = buf[5 + plen] as u16 | ((buf[5 + plen + 1] as u16) << 8);
    let crc_actual = crc16(&buf[..5 + plen]);
    if crc_actual != crc_expected {
        return Err(ProvError::CrcMismatch);
    }
    Ok((frame_type, &buf[5..5 + plen]))
}

// ── Payload encode functions ──────────────────────────────────────────────────

/// Encode an `AddContact` payload.  Returns bytes written.
///
/// Wire layout: `pubkey(32) | telemetry_enable(1) | name_len(1) | name(name_len)`
pub fn encode_add_contact(
    pubkey: &[u8; 32],
    telemetry_enable: bool,
    display_name: &[u8],
    out: &mut [u8],
) -> usize {
    let name_len = display_name.len().min(MAX_NAME_LEN);
    out[0..32].copy_from_slice(pubkey);
    out[32] = telemetry_enable as u8;
    out[33] = name_len as u8;
    out[34..34 + name_len].copy_from_slice(&display_name[..name_len]);
    34 + name_len
}

/// Encode a `DelContact` payload.  Returns bytes written (always 32).
///
/// Wire layout: `pubkey(32)`
pub fn encode_del_contact(pubkey: &[u8; 32], out: &mut [u8]) -> usize {
    out[0..32].copy_from_slice(pubkey);
    32
}

/// Encode an `AddChannel` payload.  Returns bytes written.
///
/// Wire layout: `secret(32) | key_len(1) | primary(1) | name_len(1) | name(name_len)`
///
/// `key_len` must be 16 (128-bit channel) or 32 (256-bit channel).
pub fn encode_add_channel(
    secret: &[u8; 32],
    key_len: u8,
    primary: bool,
    name: &[u8],
    out: &mut [u8],
) -> usize {
    let name_len = name.len().min(MAX_NAME_LEN);
    out[0..32].copy_from_slice(secret);
    out[32] = key_len;
    out[33] = primary as u8;
    out[34] = name_len as u8;
    out[35..35 + name_len].copy_from_slice(&name[..name_len]);
    35 + name_len
}

/// Encode a `DelChannel` payload.  Returns bytes written (always 32).
///
/// Wire layout: `secret(32)`
pub fn encode_del_channel(secret: &[u8; 32], out: &mut [u8]) -> usize {
    out[0..32].copy_from_slice(secret);
    32
}

/// Encode a `SetNotifDefaults` payload.  Returns bytes written (always 2).
///
/// Wire layout: `visual(1) | audible(1)`
pub fn encode_set_notif_defaults(visual: bool, audible: bool, out: &mut [u8]) -> usize {
    out[0] = visual as u8;
    out[1] = audible as u8;
    2
}

/// Encode a `SetPin` payload.  Returns bytes written.
///
/// Wire layout: `pin_len(1) | pin(pin_len)`
pub fn encode_set_pin(pin: &[u8], out: &mut [u8]) -> usize {
    let pin_len = pin.len().min(MAX_PIN_LEN);
    out[0] = pin_len as u8;
    out[1..1 + pin_len].copy_from_slice(&pin[..pin_len]);
    1 + pin_len
}

/// Encode a `SetDeviceName` payload.  Returns bytes written.
///
/// Wire layout: `name_len(1) | name(name_len)`
pub fn encode_set_device_name(name: &[u8], out: &mut [u8]) -> usize {
    let name_len = name.len().min(MAX_NAME_LEN);
    out[0] = name_len as u8;
    out[1..1 + name_len].copy_from_slice(&name[..name_len]);
    1 + name_len
}

/// Encode an `RspStatus` payload.  Returns bytes written (always 59).
///
/// Wire layout: `provisioned(1) | pubkey(32) | contact_count(1) | channel_count(1) |
/// gps_has_fix(1) | gps_lat_e7(4 LE) | gps_lon_e7(4 LE) | gps_fix_age_secs(4 LE) |
/// gps_clock_synced(1) | gps_clock_sync_age_secs(4 LE) | battery_percent(1) |
/// battery_charging(1) | battery_raw_mv(2 LE) | battery_held_raw_mv(2 LE)`
pub fn encode_rsp_status(payload: &RspStatusPayload, out: &mut [u8]) -> usize {
    out[0] = payload.provisioned as u8;
    out[1..33].copy_from_slice(&payload.pubkey);
    out[33] = payload.contact_count;
    out[34] = payload.channel_count;
    out[35] = payload.gps_has_fix as u8;
    out[36..40].copy_from_slice(&payload.gps_lat_e7.to_le_bytes());
    out[40..44].copy_from_slice(&payload.gps_lon_e7.to_le_bytes());
    out[44..48].copy_from_slice(&payload.gps_fix_age_secs.to_le_bytes());
    out[48] = payload.gps_clock_synced as u8;
    out[49..53].copy_from_slice(&payload.gps_clock_sync_age_secs.to_le_bytes());
    out[53] = payload.battery_percent;
    out[54] = payload.battery_charging as u8;
    out[55..57].copy_from_slice(&payload.battery_raw_mv.to_le_bytes());
    out[57..59].copy_from_slice(&payload.battery_held_raw_mv.to_le_bytes());
    59
}

/// Encode an `RspIdentity` payload.  Returns bytes written.
///
/// Wire layout: `pubkey(32) | pub_hash(1) | name_len(1) | name(name_len)`
pub fn encode_rsp_identity(pubkey: &[u8; 32], device_name: &[u8], out: &mut [u8]) -> usize {
    let name_len = device_name.len().min(MAX_NAME_LEN);
    out[0..32].copy_from_slice(pubkey);
    out[32] = pubkey[0]; // pub_hash = pubkey[0]
    out[33] = name_len as u8;
    out[34..34 + name_len].copy_from_slice(&device_name[..name_len]);
    34 + name_len
}

/// Encode an `RspContact` payload.  Returns bytes written.
///
/// Wire layout: `index(1) | pubkey(32) | telemetry(1) | name_len(1) | name(name_len)`
///
/// Maximum size: `35 + MAX_NAME_LEN` (67 bytes), within the host's per-frame
/// payload guard (`MAX_RSP_HISTORY_ENTRY_PAYLOAD` = 72).
pub fn encode_rsp_contact(
    index: u8,
    pubkey: &[u8; 32],
    telemetry_enable: bool,
    name: &[u8],
    out: &mut [u8],
) -> usize {
    let name_len = name.len().min(MAX_NAME_LEN);
    out[0] = index;
    out[1..33].copy_from_slice(pubkey);
    out[33] = telemetry_enable as u8;
    out[34] = name_len as u8;
    out[35..35 + name_len].copy_from_slice(&name[..name_len]);
    35 + name_len
}

/// Encode an `RspChannel` payload.  Returns bytes written.
///
/// Wire layout: `index(1) | channel_hash(1) | key_len(1) | primary(1) | name_len(1) | name(name_len)`
///
/// Maximum size: `5 + MAX_NAME_LEN` (37 bytes).
pub fn encode_rsp_channel(
    index: u8,
    channel_hash: u8,
    key_len: u8,
    primary: bool,
    name: &[u8],
    out: &mut [u8],
) -> usize {
    let name_len = name.len().min(MAX_NAME_LEN);
    out[0] = index;
    out[1] = channel_hash;
    out[2] = key_len;
    out[3] = primary as u8;
    out[4] = name_len as u8;
    out[5..5 + name_len].copy_from_slice(&name[..name_len]);
    5 + name_len
}

/// Encode an `RspError` payload.  Returns bytes written.
///
/// Wire layout: `error_code(1) | msg_len(1) | msg(msg_len)`
pub fn encode_rsp_error(error_code: u8, msg: &[u8], out: &mut [u8]) -> usize {
    let msg_len = msg.len().min(MAX_ERR_MSG_LEN);
    out[0] = error_code;
    out[1] = msg_len as u8;
    out[2..2 + msg_len].copy_from_slice(&msg[..msg_len]);
    2 + msg_len
}

// ── Payload decode functions ──────────────────────────────────────────────────

/// Decode an `AddContact` payload.
pub fn decode_add_contact(payload: &[u8]) -> Result<AddContactPayload, ProvError> {
    // Minimum: pubkey(32) + telemetry(1) + name_len(1) = 34 bytes
    if payload.len() < 34 {
        return Err(ProvError::TruncatedPayload);
    }
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&payload[0..32]);
    let telemetry_enable = payload[32] != 0;
    let name_len = payload[33] as usize;
    if name_len > MAX_NAME_LEN {
        return Err(ProvError::NameTooLong);
    }
    if payload.len() < 34 + name_len {
        return Err(ProvError::TruncatedPayload);
    }
    let mut display_name = [0u8; MAX_NAME_LEN];
    display_name[..name_len].copy_from_slice(&payload[34..34 + name_len]);
    Ok(AddContactPayload {
        pubkey,
        telemetry_enable,
        display_name,
        display_name_len: name_len as u8,
    })
}

/// Decode a `DelContact` payload.
pub fn decode_del_contact(payload: &[u8]) -> Result<DelContactPayload, ProvError> {
    if payload.len() < 32 {
        return Err(ProvError::TruncatedPayload);
    }
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&payload[0..32]);
    Ok(DelContactPayload { pubkey })
}

/// Decode an `AddChannel` payload.
pub fn decode_add_channel(payload: &[u8]) -> Result<AddChannelPayload, ProvError> {
    // Minimum: secret(32) + key_len(1) + primary(1) + name_len(1) = 35 bytes
    if payload.len() < 35 {
        return Err(ProvError::TruncatedPayload);
    }
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&payload[0..32]);
    let key_len = payload[32];
    let primary = payload[33] != 0;
    let name_len = payload[34] as usize;
    if name_len > MAX_NAME_LEN {
        return Err(ProvError::NameTooLong);
    }
    if payload.len() < 35 + name_len {
        return Err(ProvError::TruncatedPayload);
    }
    let mut name = [0u8; MAX_NAME_LEN];
    name[..name_len].copy_from_slice(&payload[35..35 + name_len]);
    Ok(AddChannelPayload {
        secret,
        key_len,
        primary,
        name,
        name_len: name_len as u8,
    })
}

/// Decode a `DelChannel` payload.
pub fn decode_del_channel(payload: &[u8]) -> Result<DelChannelPayload, ProvError> {
    if payload.len() < 32 {
        return Err(ProvError::TruncatedPayload);
    }
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&payload[0..32]);
    Ok(DelChannelPayload { secret })
}

/// Decode a `SetNotifDefaults` payload.
pub fn decode_set_notif_defaults(payload: &[u8]) -> Result<NotifDefaultsPayload, ProvError> {
    if payload.len() < 2 {
        return Err(ProvError::TruncatedPayload);
    }
    Ok(NotifDefaultsPayload {
        visual: payload[0] != 0,
        audible: payload[1] != 0,
    })
}

/// Decode a `SetPin` payload.
pub fn decode_set_pin(payload: &[u8]) -> Result<SetPinPayload, ProvError> {
    if payload.is_empty() {
        return Err(ProvError::TruncatedPayload);
    }
    let pin_len = payload[0] as usize;
    if pin_len > MAX_PIN_LEN {
        return Err(ProvError::PinTooLong);
    }
    if payload.len() < 1 + pin_len {
        return Err(ProvError::TruncatedPayload);
    }
    let mut pin = [0u8; MAX_PIN_LEN];
    pin[..pin_len].copy_from_slice(&payload[1..1 + pin_len]);
    Ok(SetPinPayload {
        pin,
        pin_len: pin_len as u8,
    })
}

/// Decode a `SetDeviceName` payload.
pub fn decode_set_device_name(payload: &[u8]) -> Result<SetDeviceNamePayload, ProvError> {
    if payload.is_empty() {
        return Err(ProvError::TruncatedPayload);
    }
    let name_len = payload[0] as usize;
    if name_len > MAX_NAME_LEN {
        return Err(ProvError::NameTooLong);
    }
    if payload.len() < 1 + name_len {
        return Err(ProvError::TruncatedPayload);
    }
    let mut name = [0u8; MAX_NAME_LEN];
    name[..name_len].copy_from_slice(&payload[1..1 + name_len]);
    Ok(SetDeviceNamePayload {
        name,
        name_len: name_len as u8,
    })
}

/// Decode an `RspStatus` payload.
///
/// Accepts a legacy 55-byte payload (pre-`battery_raw_mv`) and a 57-byte
/// payload (pre-`battery_held_raw_mv`) for backward compatibility with a
/// not-yet-updated firmware/host pairing during a staged rollout:
/// `battery_raw_mv` / `battery_held_raw_mv` each default to `0` when their
/// trailing bytes are absent, rather than truncating-erroring the whole
/// frame.
pub fn decode_rsp_status(payload: &[u8]) -> Result<RspStatusPayload, ProvError> {
    // provisioned(1) + pubkey(32) + contact_count(1) + channel_count(1) +
    // gps_has_fix(1) + gps_lat_e7(4) + gps_lon_e7(4) + gps_fix_age_secs(4) +
    // gps_clock_synced(1) + gps_clock_sync_age_secs(4) + battery_percent(1) +
    // battery_charging(1) + battery_raw_mv(2) + battery_held_raw_mv(2)
    // = 59 (57 pre-held_raw_mv, 55 pre-raw_mv)
    if payload.len() < 55 {
        return Err(ProvError::TruncatedPayload);
    }
    let provisioned = payload[0] != 0;
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&payload[1..33]);
    let battery_raw_mv = if payload.len() >= 57 {
        u16::from_le_bytes(payload[55..57].try_into().unwrap())
    } else {
        0
    };
    let battery_held_raw_mv = if payload.len() >= 59 {
        u16::from_le_bytes(payload[57..59].try_into().unwrap())
    } else {
        0
    };
    Ok(RspStatusPayload {
        provisioned,
        pubkey,
        contact_count: payload[33],
        channel_count: payload[34],
        gps_has_fix: payload[35] != 0,
        gps_lat_e7: i32::from_le_bytes(payload[36..40].try_into().unwrap()),
        gps_lon_e7: i32::from_le_bytes(payload[40..44].try_into().unwrap()),
        gps_fix_age_secs: u32::from_le_bytes(payload[44..48].try_into().unwrap()),
        gps_clock_synced: payload[48] != 0,
        gps_clock_sync_age_secs: u32::from_le_bytes(payload[49..53].try_into().unwrap()),
        battery_percent: payload[53],
        battery_charging: payload[54] != 0,
        battery_raw_mv,
        battery_held_raw_mv,
    })
}

/// Decode an `RspIdentity` payload.
pub fn decode_rsp_identity(payload: &[u8]) -> Result<RspIdentityPayload, ProvError> {
    // Minimum: pubkey(32) + pub_hash(1) + name_len(1) = 34 bytes
    if payload.len() < 34 {
        return Err(ProvError::TruncatedPayload);
    }
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&payload[0..32]);
    let pub_hash = payload[32];
    let name_len = payload[33] as usize;
    if name_len > MAX_NAME_LEN {
        return Err(ProvError::NameTooLong);
    }
    if payload.len() < 34 + name_len {
        return Err(ProvError::TruncatedPayload);
    }
    let mut device_name = [0u8; MAX_NAME_LEN];
    device_name[..name_len].copy_from_slice(&payload[34..34 + name_len]);
    Ok(RspIdentityPayload {
        pubkey,
        pub_hash,
        device_name,
        device_name_len: name_len as u8,
    })
}

/// Decode an `RspContact` payload.
pub fn decode_rsp_contact(payload: &[u8]) -> Result<RspContactPayload, ProvError> {
    // Minimum: index(1) + pubkey(32) + telemetry(1) + name_len(1) = 35 bytes
    if payload.len() < 35 {
        return Err(ProvError::TruncatedPayload);
    }
    let index = payload[0];
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&payload[1..33]);
    let telemetry_enable = payload[33] != 0;
    let name_len = payload[34] as usize;
    if name_len > MAX_NAME_LEN {
        return Err(ProvError::NameTooLong);
    }
    if payload.len() < 35 + name_len {
        return Err(ProvError::TruncatedPayload);
    }
    let mut display_name = [0u8; MAX_NAME_LEN];
    display_name[..name_len].copy_from_slice(&payload[35..35 + name_len]);
    Ok(RspContactPayload {
        index,
        pubkey,
        telemetry_enable,
        display_name,
        display_name_len: name_len as u8,
    })
}

/// Decode an `RspChannel` payload.
pub fn decode_rsp_channel(payload: &[u8]) -> Result<RspChannelPayload, ProvError> {
    // Minimum: index(1) + channel_hash(1) + key_len(1) + primary(1) + name_len(1) = 5 bytes
    if payload.len() < 5 {
        return Err(ProvError::TruncatedPayload);
    }
    let index = payload[0];
    let channel_hash = payload[1];
    let key_len = payload[2];
    let primary = payload[3] != 0;
    let name_len = payload[4] as usize;
    if name_len > MAX_NAME_LEN {
        return Err(ProvError::NameTooLong);
    }
    if payload.len() < 5 + name_len {
        return Err(ProvError::TruncatedPayload);
    }
    let mut name = [0u8; MAX_NAME_LEN];
    name[..name_len].copy_from_slice(&payload[5..5 + name_len]);
    Ok(RspChannelPayload {
        index,
        channel_hash,
        key_len,
        primary,
        name,
        name_len: name_len as u8,
    })
}

/// Decode an `RspError` payload.
pub fn decode_rsp_error(payload: &[u8]) -> Result<RspErrorPayload, ProvError> {
    if payload.len() < 2 {
        return Err(ProvError::TruncatedPayload);
    }
    let error_code = payload[0];
    let msg_len = payload[1] as usize;
    if msg_len > MAX_ERR_MSG_LEN || payload.len() < 2 + msg_len {
        return Err(ProvError::TruncatedPayload);
    }
    let mut msg = [0u8; MAX_ERR_MSG_LEN];
    msg[..msg_len].copy_from_slice(&payload[2..2 + msg_len]);
    Ok(RspErrorPayload {
        error_code,
        msg,
        msg_len: msg_len as u8,
    })
}

// ── CRC-16/ARC helper ─────────────────────────────────────────────────────────

/// CRC-16/ARC (polynomial 0x8005, reflected; init 0x0000; no final XOR).
///
/// Used for frame integrity only (USB-serial is reliable; this catches
/// accidental byte corruption, NOT adversarial tampering).
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0x0000;
    for &byte in data {
        crc ^= byte as u16;
        for _ in 0..8 {
            if crc & 0x0001 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Frame-level roundtrip ────────────────────────────────────────────────

    #[test]
    fn frame_encode_decode_empty_payload() {
        let mut buf = [0u8; 16];
        let n = encode_frame(FRAME_RSP_OK, &[], &mut buf);
        assert_eq!(n, FRAME_OVERHEAD);
        let (ft, payload) = decode_frame(&buf[..n]).unwrap();
        assert_eq!(ft, FRAME_RSP_OK);
        assert!(payload.is_empty());
    }

    #[test]
    fn frame_roundtrip_with_payload() {
        let data = b"hello provisioning";
        let mut buf = [0u8; 64];
        let n = encode_frame(0x99, data, &mut buf);
        assert_eq!(n, FRAME_OVERHEAD + data.len());
        let (ft, payload) = decode_frame(&buf[..n]).unwrap();
        assert_eq!(ft, 0x99);
        assert_eq!(payload, data);
    }

    #[test]
    fn decode_frame_bad_magic() {
        let mut buf = [0u8; 16];
        let n = encode_frame(FRAME_RSP_OK, &[], &mut buf);
        buf[0] = 0xFF; // corrupt magic
        assert_eq!(decode_frame(&buf[..n]), Err(ProvError::BadMagic));
    }

    #[test]
    fn decode_frame_truncated() {
        let mut buf = [0u8; 16];
        let n = encode_frame(FRAME_RSP_OK, &[], &mut buf);
        // Pass only 4 bytes — too short for the 7-byte overhead
        assert_eq!(decode_frame(&buf[..4]), Err(ProvError::TruncatedFrame));
        // Pass n-1 bytes — CRC bytes missing
        assert_eq!(decode_frame(&buf[..n - 1]), Err(ProvError::TruncatedFrame));
    }

    #[test]
    fn decode_frame_crc_mismatch() {
        let mut buf = [0u8; 16];
        let n = encode_frame(FRAME_QUERY_STATUS, &[], &mut buf);
        buf[n - 1] ^= 0xFF; // flip CRC high byte
        assert_eq!(decode_frame(&buf[..n]), Err(ProvError::CrcMismatch));
    }

    // ── AddContact roundtrip ─────────────────────────────────────────────────

    #[test]
    fn add_contact_roundtrip_with_name() {
        let pubkey = [0xABu8; 32];
        let name = b"Alice";
        let mut payload_buf = [0u8; 64];
        let plen = encode_add_contact(&pubkey, true, name, &mut payload_buf);

        let mut frame_buf = [0u8; 128];
        let n = encode_frame(FRAME_ADD_CONTACT, &payload_buf[..plen], &mut frame_buf);
        let (ft, payload) = decode_frame(&frame_buf[..n]).unwrap();
        assert_eq!(ft, FRAME_ADD_CONTACT);

        let contact = decode_add_contact(payload).unwrap();
        assert_eq!(contact.pubkey, pubkey);
        assert!(contact.telemetry_enable);
        assert_eq!(contact.display_name_len, name.len() as u8);
        assert_eq!(&contact.display_name[..name.len()], name);
    }

    #[test]
    fn add_contact_roundtrip_no_name() {
        let pubkey = [0x11u8; 32];
        let mut payload_buf = [0u8; 64];
        let plen = encode_add_contact(&pubkey, false, &[], &mut payload_buf);
        let decoded = decode_add_contact(&payload_buf[..plen]).unwrap();
        assert_eq!(decoded.pubkey, pubkey);
        assert!(!decoded.telemetry_enable);
        assert_eq!(decoded.display_name_len, 0);
    }

    // ── AddChannel roundtrip ─────────────────────────────────────────────────

    #[test]
    fn add_channel_roundtrip_primary() {
        let secret = [0x6Du8; 32]; // 'm' — the HIL test channel secret
        let name = b"family";
        let mut payload_buf = [0u8; 70];
        let plen = encode_add_channel(&secret, 32, true, name, &mut payload_buf);

        let mut frame_buf = [0u8; 128];
        let n = encode_frame(FRAME_ADD_CHANNEL, &payload_buf[..plen], &mut frame_buf);
        let (ft, payload) = decode_frame(&frame_buf[..n]).unwrap();
        assert_eq!(ft, FRAME_ADD_CHANNEL);

        let ch = decode_add_channel(payload).unwrap();
        assert_eq!(ch.secret, secret);
        assert_eq!(ch.key_len, 32);
        assert!(ch.primary);
        assert_eq!(ch.name_len, name.len() as u8);
        assert_eq!(&ch.name[..name.len()], name);
    }

    #[test]
    fn add_channel_roundtrip_128bit_secret() {
        // 16-byte (128-bit) secret: only first 16 bytes significant; last 16 zero-padded.
        let mut secret = [0u8; 32];
        secret[..16].copy_from_slice(&[0xABu8; 16]);
        let name = b"family128";
        let mut payload_buf = [0u8; 70];
        let plen = encode_add_channel(&secret, 16, false, name, &mut payload_buf);

        let mut frame_buf = [0u8; 128];
        let n = encode_frame(FRAME_ADD_CHANNEL, &payload_buf[..plen], &mut frame_buf);
        let (ft, payload) = decode_frame(&frame_buf[..n]).unwrap();
        assert_eq!(ft, FRAME_ADD_CHANNEL);

        let ch = decode_add_channel(payload).unwrap();
        assert_eq!(ch.secret, secret);
        assert_eq!(ch.key_len, 16);
        assert!(!ch.primary);
        assert_eq!(ch.name_len, name.len() as u8);
        assert_eq!(&ch.name[..name.len()], name);
    }

    // ── SetNotifDefaults roundtrip ───────────────────────────────────────────

    #[test]
    fn set_notif_defaults_roundtrip() {
        let mut payload_buf = [0u8; 8];
        let plen = encode_set_notif_defaults(true, false, &mut payload_buf);
        assert_eq!(plen, 2);
        let decoded = decode_set_notif_defaults(&payload_buf[..plen]).unwrap();
        assert!(decoded.visual);
        assert!(!decoded.audible);
    }

    // ── SetPin roundtrip ─────────────────────────────────────────────────────

    #[test]
    fn set_pin_roundtrip() {
        let pin = b"1234";
        let mut payload_buf = [0u8; 32];
        let plen = encode_set_pin(pin, &mut payload_buf);

        let mut frame_buf = [0u8; 64];
        let n = encode_frame(FRAME_SET_PIN, &payload_buf[..plen], &mut frame_buf);
        let (ft, payload) = decode_frame(&frame_buf[..n]).unwrap();
        assert_eq!(ft, FRAME_SET_PIN);

        let decoded = decode_set_pin(payload).unwrap();
        assert_eq!(decoded.pin_len, 4);
        assert_eq!(&decoded.pin[..4], b"1234");
    }

    // ── RspStatus roundtrip ──────────────────────────────────────────────────

    #[test]
    fn rsp_status_roundtrip() {
        let status = RspStatusPayload {
            provisioned: false,
            pubkey: [0x55u8; 32],
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
        let mut payload_buf = [0u8; 64];
        let plen = encode_rsp_status(&status, &mut payload_buf);
        assert_eq!(plen, 59);

        let mut frame_buf = [0u8; 72];
        let n = encode_frame(FRAME_RSP_STATUS, &payload_buf[..plen], &mut frame_buf);
        let (ft, payload) = decode_frame(&frame_buf[..n]).unwrap();
        assert_eq!(ft, FRAME_RSP_STATUS);

        let decoded = decode_rsp_status(payload).unwrap();
        assert!(!decoded.provisioned);
        assert_eq!(decoded.pubkey, [0x55u8; 32]);
        assert_eq!(decoded.contact_count, 0);
        assert_eq!(decoded.channel_count, 0);
        assert!(!decoded.gps_has_fix);
        assert!(!decoded.gps_clock_synced);
        assert_eq!(decoded.battery_percent, 0);
        assert!(!decoded.battery_charging);
        assert_eq!(decoded.battery_raw_mv, 0);
        assert_eq!(decoded.battery_held_raw_mv, 0);
    }

    #[test]
    fn rsp_status_roundtrip_with_gps_fix_and_sync() {
        let status = RspStatusPayload {
            provisioned: true,
            pubkey: [0xAAu8; 32],
            contact_count: 2,
            channel_count: 1,
            gps_has_fix: true,
            gps_lat_e7: -481_173_000,
            gps_lon_e7: 115_166_667,
            gps_fix_age_secs: 42,
            gps_clock_synced: true,
            gps_clock_sync_age_secs: 300,
            battery_percent: 76,
            battery_charging: true,
            battery_raw_mv: 4142,
            battery_held_raw_mv: 3775,
        };
        let mut payload_buf = [0u8; 64];
        let plen = encode_rsp_status(&status, &mut payload_buf);
        assert_eq!(plen, 59);
        let decoded = decode_rsp_status(&payload_buf[..plen]).unwrap();
        assert_eq!(decoded, status);
        assert_eq!(decoded.battery_percent, 76);
        assert!(decoded.battery_charging);
        assert_eq!(decoded.battery_raw_mv, 4142);
        assert_eq!(decoded.battery_held_raw_mv, 3775);
    }

    #[test]
    fn rsp_status_truncated_payload_rejected() {
        let payload_buf = [0u8; 54]; // one byte short of the legacy 55-byte minimum
        assert_eq!(
            decode_rsp_status(&payload_buf),
            Err(ProvError::TruncatedPayload)
        );
    }

    #[test]
    fn rsp_status_legacy_55_byte_payload_decodes_with_zero_raw_mv() {
        // A pre-`battery_raw_mv` 55-byte payload (no trailing raw_mv/
        // held_raw_mv bytes) must still decode — both new fields default to
        // 0 rather than the whole frame being rejected as truncated.
        let status = RspStatusPayload {
            provisioned: true,
            pubkey: [0x11u8; 32],
            contact_count: 1,
            channel_count: 1,
            gps_has_fix: false,
            gps_lat_e7: 0,
            gps_lon_e7: 0,
            gps_fix_age_secs: 0,
            gps_clock_synced: false,
            gps_clock_sync_age_secs: 0,
            battery_percent: 50,
            battery_charging: false,
            battery_raw_mv: 0,
            battery_held_raw_mv: 0,
        };
        let mut payload_buf = [0u8; 64];
        let plen = encode_rsp_status(&status, &mut payload_buf);
        assert_eq!(plen, 59);

        // Truncate to the legacy 55-byte length before decoding.
        let decoded = decode_rsp_status(&payload_buf[..55]).unwrap();
        assert_eq!(decoded.battery_percent, 50);
        assert_eq!(
            decoded.battery_raw_mv, 0,
            "raw_mv absent on the wire must default to 0"
        );
        assert_eq!(
            decoded.battery_held_raw_mv, 0,
            "held_raw_mv absent on the wire must default to 0"
        );
    }

    #[test]
    fn rsp_status_legacy_57_byte_payload_decodes_with_zero_held_raw_mv() {
        // A pre-`battery_held_raw_mv` 57-byte payload (raw_mv present, held
        // raw_mv trailing bytes absent) must still decode — the new field
        // defaults to 0 rather than the whole frame being rejected.
        let status = RspStatusPayload {
            provisioned: true,
            pubkey: [0x22u8; 32],
            contact_count: 0,
            channel_count: 0,
            gps_has_fix: false,
            gps_lat_e7: 0,
            gps_lon_e7: 0,
            gps_fix_age_secs: 0,
            gps_clock_synced: false,
            gps_clock_sync_age_secs: 0,
            battery_percent: 82,
            battery_charging: false,
            battery_raw_mv: 4180,
            battery_held_raw_mv: 0,
        };
        let mut payload_buf = [0u8; 64];
        let plen = encode_rsp_status(&status, &mut payload_buf);
        assert_eq!(plen, 59);

        // Truncate to the legacy 57-byte length before decoding.
        let decoded = decode_rsp_status(&payload_buf[..57]).unwrap();
        assert_eq!(decoded.battery_percent, 82);
        assert_eq!(decoded.battery_raw_mv, 4180);
        assert_eq!(
            decoded.battery_held_raw_mv, 0,
            "held_raw_mv absent on the wire must default to 0"
        );
    }

    // ── RspIdentity roundtrip ────────────────────────────────────────────────

    #[test]
    fn rsp_identity_roundtrip_no_name() {
        let pubkey = [0xCCu8; 32];
        let mut payload_buf = [0u8; 64];
        let plen = encode_rsp_identity(&pubkey, &[], &mut payload_buf);
        assert_eq!(plen, 34);
        let decoded = decode_rsp_identity(&payload_buf[..plen]).unwrap();
        assert_eq!(decoded.pubkey, pubkey);
        assert_eq!(decoded.pub_hash, 0xCC); // pubkey[0]
        assert_eq!(decoded.device_name_len, 0);
    }

    #[test]
    fn rsp_identity_roundtrip_with_name() {
        let pubkey = [0xCCu8; 32];
        let name = b"My T-Deck Plus";
        let mut payload_buf = [0u8; 96];
        let plen = encode_rsp_identity(&pubkey, name, &mut payload_buf);
        assert_eq!(plen, 34 + name.len());
        let decoded = decode_rsp_identity(&payload_buf[..plen]).unwrap();
        assert_eq!(decoded.pubkey, pubkey);
        assert_eq!(decoded.pub_hash, 0xCC); // pubkey[0]
        assert_eq!(decoded.device_name_len, name.len() as u8);
        assert_eq!(&decoded.device_name[..name.len()], name);
    }

    // ── SetDeviceName roundtrip ──────────────────────────────────────────────

    #[test]
    fn set_device_name_roundtrip() {
        let name = b"Alex's MeshCadet";
        let mut payload_buf = [0u8; 64];
        let plen = encode_set_device_name(name, &mut payload_buf);

        let mut frame_buf = [0u8; 96];
        let n = encode_frame(FRAME_SET_DEVICE_NAME, &payload_buf[..plen], &mut frame_buf);
        let (ft, payload) = decode_frame(&frame_buf[..n]).unwrap();
        assert_eq!(ft, FRAME_SET_DEVICE_NAME);

        let decoded = decode_set_device_name(payload).unwrap();
        assert_eq!(decoded.name_len, name.len() as u8);
        assert_eq!(&decoded.name[..name.len()], name);
    }

    #[test]
    fn set_device_name_empty_clears() {
        let mut payload_buf = [0u8; 8];
        let plen = encode_set_device_name(&[], &mut payload_buf);
        assert_eq!(plen, 1);
        let decoded = decode_set_device_name(&payload_buf[..plen]).unwrap();
        assert_eq!(decoded.name_len, 0);
    }

    #[test]
    fn decode_set_device_name_too_long() {
        let mut payload = [0u8; 40];
        payload[0] = (MAX_NAME_LEN + 1) as u8;
        assert_eq!(
            decode_set_device_name(&payload),
            Err(ProvError::NameTooLong)
        );
    }

    // ── RspError roundtrip ───────────────────────────────────────────────────

    #[test]
    fn rsp_error_roundtrip() {
        let mut payload_buf = [0u8; 128];
        let plen = encode_rsp_error(0x42, b"contact list full", &mut payload_buf);
        let decoded = decode_rsp_error(&payload_buf[..plen]).unwrap();
        assert_eq!(decoded.error_code, 0x42);
        assert_eq!(decoded.msg_len, b"contact list full".len() as u8);
        assert_eq!(
            &decoded.msg[..decoded.msg_len as usize],
            b"contact list full"
        );
    }

    // ── RspContact roundtrip ─────────────────────────────────────────────────

    #[test]
    fn rsp_contact_roundtrip_with_name() {
        let pubkey = [0xA1u8; 32];
        let name = b"Alice";
        let mut payload_buf = [0u8; 80];
        let plen = encode_rsp_contact(2, &pubkey, true, name, &mut payload_buf);
        assert_eq!(plen, 35 + name.len());

        let mut frame_buf = [0u8; 128];
        let n = encode_frame(FRAME_RSP_CONTACT, &payload_buf[..plen], &mut frame_buf);
        let (ft, payload) = decode_frame(&frame_buf[..n]).unwrap();
        assert_eq!(ft, FRAME_RSP_CONTACT);

        let c = decode_rsp_contact(payload).unwrap();
        assert_eq!(c.index, 2);
        assert_eq!(c.pubkey, pubkey);
        assert!(c.telemetry_enable);
        assert_eq!(c.display_name_len, name.len() as u8);
        assert_eq!(&c.display_name[..name.len()], name);
    }

    #[test]
    fn rsp_contact_roundtrip_no_name() {
        let pubkey = [0xB0u8; 32];
        let mut payload_buf = [0u8; 80];
        let plen = encode_rsp_contact(0, &pubkey, false, &[], &mut payload_buf);
        assert_eq!(plen, 35);
        let c = decode_rsp_contact(&payload_buf[..plen]).unwrap();
        assert_eq!(c.index, 0);
        assert!(!c.telemetry_enable);
        assert_eq!(c.display_name_len, 0);
    }

    #[test]
    fn rsp_contact_max_payload_fits_recv_guard() {
        // Worst case (index + pubkey + telemetry + name_len + MAX_NAME_LEN name)
        // must stay within the host recv_frame plen guard.
        let pubkey = [0xCCu8; 32];
        let name = [b'X'; MAX_NAME_LEN];
        let mut payload_buf = [0u8; 80];
        let plen = encode_rsp_contact(15, &pubkey, true, &name, &mut payload_buf);
        assert_eq!(plen, 35 + MAX_NAME_LEN);
        assert!(plen <= crate::history::MAX_RSP_HISTORY_ENTRY_PAYLOAD);
    }

    #[test]
    fn decode_rsp_contact_truncated() {
        assert_eq!(
            decode_rsp_contact(&[0u8; 34]),
            Err(ProvError::TruncatedPayload)
        );
    }

    // ── RspChannel roundtrip ─────────────────────────────────────────────────

    #[test]
    fn rsp_channel_roundtrip_primary() {
        let name = b"family";
        let mut payload_buf = [0u8; 80];
        let plen = encode_rsp_channel(1, 0x6D, 32, true, name, &mut payload_buf);
        assert_eq!(plen, 5 + name.len());

        let mut frame_buf = [0u8; 128];
        let n = encode_frame(FRAME_RSP_CHANNEL, &payload_buf[..plen], &mut frame_buf);
        let (ft, payload) = decode_frame(&frame_buf[..n]).unwrap();
        assert_eq!(ft, FRAME_RSP_CHANNEL);

        let ch = decode_rsp_channel(payload).unwrap();
        assert_eq!(ch.index, 1);
        assert_eq!(ch.channel_hash, 0x6D);
        assert_eq!(ch.key_len, 32);
        assert!(ch.primary);
        assert_eq!(ch.name_len, name.len() as u8);
        assert_eq!(&ch.name[..name.len()], name);
    }

    #[test]
    fn rsp_channel_roundtrip_128bit_no_name() {
        let mut payload_buf = [0u8; 80];
        let plen = encode_rsp_channel(0, 0xAB, 16, false, &[], &mut payload_buf);
        assert_eq!(plen, 5);
        let ch = decode_rsp_channel(&payload_buf[..plen]).unwrap();
        assert_eq!(ch.key_len, 16);
        assert!(!ch.primary);
        assert_eq!(ch.name_len, 0);
    }

    #[test]
    fn decode_rsp_channel_truncated() {
        assert_eq!(
            decode_rsp_channel(&[0u8; 4]),
            Err(ProvError::TruncatedPayload)
        );
    }

    // ── CommitProvisioning frame (empty payload) ─────────────────────────────

    #[test]
    fn commit_provisioning_frame_roundtrip() {
        let mut buf = [0u8; 16];
        let n = encode_frame(FRAME_COMMIT_PROVISIONING, &[], &mut buf);
        let (ft, payload) = decode_frame(&buf[..n]).unwrap();
        assert_eq!(ft, FRAME_COMMIT_PROVISIONING);
        assert!(payload.is_empty());
    }

    // ── ClearHistory frame (empty payload) ───────────────────────────────────

    #[test]
    fn clear_history_frame_roundtrip() {
        let mut buf = [0u8; 16];
        let n = encode_frame(FRAME_CLEAR_HISTORY, &[], &mut buf);
        let (ft, payload) = decode_frame(&buf[..n]).unwrap();
        assert_eq!(ft, FRAME_CLEAR_HISTORY);
        assert!(payload.is_empty());
    }

    #[test]
    fn clear_history_is_additive_distinct_from_existing_frame_types() {
        // Guard against accidental byte-value collision with the retired
        // 0x30/0x60 range or any existing command/response frame type — the
        // ADR-0002 additive-only invariant this frame type must preserve.
        assert_eq!(FRAME_CLEAR_HISTORY, 0x72);
        assert_ne!(FRAME_CLEAR_HISTORY, FRAME_COMMIT_PROVISIONING);
        assert_ne!(FRAME_CLEAR_HISTORY, FRAME_EXPORT_HISTORY);
    }

    // ── CRC-16 sanity ────────────────────────────────────────────────────────

    #[test]
    fn crc16_known_answer() {
        // CRC-16/ARC of "123456789" = 0xBB3D
        let crc = crc16(b"123456789");
        assert_eq!(crc, 0xBB3D, "CRC-16/ARC known-answer vector");
    }

    #[test]
    fn crc16_empty_input() {
        assert_eq!(crc16(&[]), 0x0000);
    }

    // ── Error paths ──────────────────────────────────────────────────────────

    #[test]
    fn decode_add_contact_truncated() {
        // Need at least 34 bytes (32 + 1 + 1)
        assert_eq!(
            decode_add_contact(&[0u8; 33]),
            Err(ProvError::TruncatedPayload)
        );
    }

    #[test]
    fn decode_add_channel_name_too_long() {
        let mut payload = [0u8; 70];
        payload[34] = (MAX_NAME_LEN + 1) as u8; // name_len > MAX_NAME_LEN (now at byte 34)
        assert_eq!(decode_add_channel(&payload), Err(ProvError::NameTooLong));
    }

    #[test]
    fn decode_set_pin_too_long() {
        let mut payload = [0u8; 20];
        payload[0] = (MAX_PIN_LEN + 1) as u8; // pin_len > MAX_PIN_LEN
        assert_eq!(decode_set_pin(&payload), Err(ProvError::PinTooLong));
    }
}
