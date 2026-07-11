// SPDX-License-Identifier: GPL-3.0-only
//! Firmware-side provisioning config store — NVS-backed flash persistence.
//!
//! Reads and writes all provisioned fields (identity contacts, channels, radio
//! preset, notification defaults, PIN, feature locks) to/from the ESP-IDF NVS
//! default partition.  Uses a separate namespace (`mc_cfg`) from the identity
//! store (`mc_id`) so the two subsystems are independently addressable.
//!
//! # NVS layout
//!
//! | Namespace | Key          | Type  | Contents                               |
//! |-----------|--------------|-------|----------------------------------------|
//! | `mc_cfg`  | `prov`       | u8    | 0 = unprovisioned, 1 = provisioned    |
//! | `mc_cfg`  | `cfg_blob`   | blob  | Serialised `ProvisionedConfig` binary  |
//!
//! The `prov` flag is written to `1` only by [`mark_provisioned`] /
//! [`save_provisioned_config`], which persists the config blob first.  Reads at
//! boot check the flag first; a missing or zero flag means UNPROVISIONED.
//!
//! # Serialisation format (internal, version-tagged)
//!
//! ```text
//! byte 0       version = 0x01
//! byte 1       contact_count  (0–MAX_CONTACTS)
//! byte 2       channel_count  (0–MAX_CHANNELS)
//! byte 3       lock_flags     (bitfield, see LOCK_* in protocol::provisioning)
//! byte 4       pin_len        (0 ⇒ no PIN set)
//! bytes 5–20   pin            (MAX_PIN_LEN bytes, zero-padded)
//! byte 21      notif_visual   (0/1)
//! byte 22      notif_audible  (0/1)
//! bytes 23–26  radio_freq_hz  (little-endian u32)
//! byte 27      radio_bw_code
//! byte 28      radio_sf
//! byte 29      radio_cr
//! byte 30      radio_tx_power_dbm
//! — for each contact (contact_count × CONTACT_ENTRY_LEN bytes): —
//!   bytes +0..+31   pubkey (32)
//!   byte  +32       telemetry_enable
//!   byte  +33       display_name_len
//!   bytes +34..+65  display_name (MAX_NAME_LEN, zero-padded)
//! — for each channel (channel_count × CHANNEL_ENTRY_LEN bytes): —
//!   bytes +0..+31   secret (32)
//!   byte  +32       key_len (16 or 32)
//!   byte  +33       primary
//!   byte  +34       name_len
//!   bytes +35..+66  name (MAX_NAME_LEN, zero-padded)
//! ```
//!
//! Blob format version: `0x02` (bumped from `0x01` when `key_len` was added to channels).
//! Max blob size: 31 + 16×66 + 8×67 = 1623 bytes (well within a 24 KB NVS partition).

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
pub use esp_idf_svc::sys::EspError;

// ── Capacity limits ───────────────────────────────────────────────────────────

/// Maximum number of provisioned contacts.
pub const MAX_CONTACTS: usize = 16;

/// Maximum number of provisioned channels.
pub const MAX_CHANNELS: usize = 8;

/// Maximum byte length of a contact display name or channel name.
pub const MAX_NAME_LEN: usize = 32;

/// Maximum byte length of the PIN.
pub const MAX_PIN_LEN: usize = 16;

// ── Default locked radio preset (ADR-0001) ────────────────────────────────────

/// Default radio frequency in Hz: 910.525 MHz.
pub const DEFAULT_FREQ_HZ: u32 = 910_525_000;
/// Default bandwidth code: 0 = 62.5 kHz.
pub const DEFAULT_BW_CODE: u8 = 0;
/// Default spreading factor: SF7.
pub const DEFAULT_SF: u8 = 7;
/// Default coding rate: 1 = 4/5.
pub const DEFAULT_CR: u8 = 1;
/// Default TX power: +22 dBm (SX1262 maximum, matching the deployed mesh).
pub const DEFAULT_TX_POWER_DBM: u8 = 22;

// ── NVS keys ─────────────────────────────────────────────────────────────────

const NVS_NAMESPACE: &str = "mc_cfg";
const NVS_KEY_PROV_FLAG: &str = "prov";
const NVS_KEY_CFG_BLOB: &str = "cfg_blob";

// ── Serialisation constants ───────────────────────────────────────────────────

const CFG_VERSION: u8 = 0x02;
const CFG_HEADER_LEN: usize = 31;
const CONTACT_ENTRY_LEN: usize = 66; // pubkey(32) + telemetry(1) + name_len(1) + name(32)
const CHANNEL_ENTRY_LEN: usize = 67; // secret(32) + key_len(1) + primary(1) + name_len(1) + name(32)
const MAX_BLOB_LEN: usize = CFG_HEADER_LEN
    + MAX_CONTACTS * CONTACT_ENTRY_LEN
    + MAX_CHANNELS * CHANNEL_ENTRY_LEN; // = 1623 bytes

// ── Config structs ────────────────────────────────────────────────────────────

/// A provisioned contact entry.
#[derive(Clone, Copy, Debug)]
pub struct Contact {
    /// Ed25519 public key (32 bytes).
    pub pubkey: [u8; 32],
    /// Whether this contact may pull our GPS telemetry.
    pub telemetry_enable: bool,
    /// UTF-8 display name, zero-padded to `MAX_NAME_LEN`.
    pub display_name: [u8; MAX_NAME_LEN],
    /// Actual byte length of `display_name` (0 ⇒ use 1-byte hash as label).
    pub display_name_len: u8,
}

impl Contact {
    /// 1-byte routing hash for this contact: `pubkey[0]`.
    pub fn pub_hash(&self) -> u8 {
        self.pubkey[0]
    }
}

/// A provisioned channel entry.
#[derive(Clone, Copy, Debug)]
pub struct Channel {
    /// 32-byte symmetric channel secret.
    /// For 128-bit channels, only bytes `[0..16]` carry the secret;
    /// bytes `[16..32]` are zero-padded.
    pub secret: [u8; 32],
    /// Number of significant secret bytes: 16 (128-bit) or 32 (256-bit).
    ///
    /// Selects the channel-hash computation:
    /// - `16`: `SHA-256(secret[0..16])[0]`
    /// - `32`: `SHA-256(secret)[0]`
    pub key_len: u8,
    /// If `true`, this channel is the primary (default) outgoing channel.
    pub primary: bool,
    /// UTF-8 channel name, zero-padded to `MAX_NAME_LEN`.
    pub name: [u8; MAX_NAME_LEN],
    /// Actual byte length of `name`.
    pub name_len: u8,
}

/// Outcome of [`ProvisionedConfig::upsert_channel`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelUpsert {
    /// An existing channel with the same secret was updated in place
    /// (count unchanged).
    Updated,
    /// A new channel was appended (count incremented by one).
    Added,
}

/// Returned by [`ProvisionedConfig::upsert_channel`] when a genuinely new key
/// cannot be appended because the channel list is at [`MAX_CHANNELS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelListFull;

/// Outcome of [`ProvisionedConfig::upsert_contact`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactUpsert {
    /// An existing contact with the same pubkey was updated in place
    /// (count unchanged) — telemetry flag / display name refreshed.
    Updated,
    /// A new contact was appended (count incremented by one).
    Added,
}

/// Returned by [`ProvisionedConfig::upsert_contact`] when a genuinely new
/// contact cannot be appended because the contact list is at [`MAX_CONTACTS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContactListFull;

/// Radio modem preset parameters.
///
/// The default values match the locked ADR-0001 preset.
#[derive(Clone, Copy, Debug)]
pub struct RadioPreset {
    /// Center frequency in Hz.
    pub freq_hz: u32,
    /// Bandwidth code: 0=62.5 kHz, 1=125 kHz, 2=250 kHz, 3=500 kHz.
    pub bw_code: u8,
    /// Spreading factor (5–12).
    pub sf: u8,
    /// Coding rate: 1=4/5, 2=4/6, 3=4/7, 4=4/8.
    pub cr: u8,
    /// TX power in dBm (0–22 for SX1262).
    pub tx_power_dbm: u8,
}

impl Default for RadioPreset {
    fn default() -> Self {
        Self {
            freq_hz:       DEFAULT_FREQ_HZ,
            bw_code:       DEFAULT_BW_CODE,
            sf:            DEFAULT_SF,
            cr:            DEFAULT_CR,
            tx_power_dbm:  DEFAULT_TX_POWER_DBM,
        }
    }
}

/// Notification default settings.
#[derive(Clone, Copy, Debug)]
pub struct NotifDefaults {
    /// Visual notification (screen flash / LED) enabled by default.
    pub visual: bool,
    /// Audible notification (buzzer / speaker) enabled by default.
    pub audible: bool,
}

impl Default for NotifDefaults {
    fn default() -> Self {
        Self { visual: true, audible: true }
    }
}

/// The full provisioned configuration persisted to flash.
///
/// Returned by [`load_provisioned_config`] when the device has been provisioned.
/// Written by [`save_provisioned_config`] at the end of a provisioning session.
#[derive(Clone, Debug)]
pub struct ProvisionedConfig {
    pub contacts: [Contact; MAX_CONTACTS],
    pub contact_count: u8,
    pub channels: [Channel; MAX_CHANNELS],
    pub channel_count: u8,
    pub radio_preset: RadioPreset,
    pub notif_defaults: NotifDefaults,
    /// UTF-8 PIN, zero-padded to `MAX_PIN_LEN`.
    pub pin: [u8; MAX_PIN_LEN],
    /// Actual byte length of the PIN (0 ⇒ PIN lock disabled).
    pub pin_len: u8,
    /// Feature-lock flags; see `LOCK_*` in `protocol::provisioning`.
    pub lock_flags: u8,
}

impl ProvisionedConfig {
    /// An empty config: zero contacts, zero channels, default radio/notif
    /// settings, no PIN, no locks.  Used as the admin-server fallback when a
    /// provisioned device's config blob is missing or fails to load — the
    /// server still answers queries (reporting zero entries) and accepts edits
    /// rather than hanging the host.
    pub fn empty() -> Self {
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
        Self {
            contacts:       [null_contact; MAX_CONTACTS],
            contact_count:  0,
            channels:       [null_channel; MAX_CHANNELS],
            channel_count:  0,
            radio_preset:   RadioPreset::default(),
            notif_defaults: NotifDefaults::default(),
            pin:            [0u8; MAX_PIN_LEN],
            pin_len:        0,
            lock_flags:     0,
        }
    }

    /// Return the primary channel, if one is configured.
    pub fn primary_channel(&self) -> Option<&Channel> {
        let count = self.channel_count as usize;
        self.channels[..count].iter().find(|ch| ch.primary)
    }

    /// Insert or update a channel, keyed on its `secret` (the channel's
    /// cryptographic identity — the on-air `channel_hash` derives from the
    /// secret, so the secret IS the channel; the name is just a mutable label).
    ///
    /// Idempotent upsert semantics (the shared add-channel core used by both
    /// the runtime `admin_server` and the first-boot `provisioning_server`):
    ///
    /// - **Known key → update in place.** If a channel with the same `secret`
    ///   already exists, that entry is refreshed (name, `key_len`, `primary`)
    ///   and `channel_count` is left UNCHANGED. Re-adding a known key with a
    ///   different name therefore RENAMES the existing channel rather than
    ///   stacking a cryptographically-identical duplicate.
    /// - **New key → append.** An unseen secret is appended normally; returns
    ///   [`ChannelUpsert::Added`]. Returns `Err(ChannelListFull)` only when a
    ///   genuinely new key would overflow [`MAX_CHANNELS`] (a known-key update
    ///   never fails on capacity, since it consumes no new slot).
    ///
    /// **Single-primary invariant.** When the inserted/updated channel has
    /// `primary == true`, every other channel is demoted first, so at most one
    /// channel is ever primary. (An upsert with `primary == false` refreshes
    /// the matched entry's flag to non-primary and does not touch the others.)
    ///
    /// The caller is responsible for persisting the mutated config to NVS so
    /// the dedup survives reboot.
    pub fn upsert_channel(&mut self, ch: Channel) -> Result<ChannelUpsert, ChannelListFull> {
        let cnt = self.channel_count as usize;
        let existing = self.channels[..cnt].iter().position(|c| c.secret == ch.secret);

        // Capacity only constrains a genuinely new key; an in-place update
        // reuses the matched slot and can never overflow.
        if existing.is_none() && cnt >= MAX_CHANNELS {
            return Err(ChannelListFull);
        }

        // Enforce at-most-one-primary: demote every existing channel before
        // placing this one if it claims the primary slot.
        if ch.primary {
            for c in self.channels[..cnt].iter_mut() {
                c.primary = false;
            }
        }

        match existing {
            Some(idx) => {
                self.channels[idx] = ch;
                Ok(ChannelUpsert::Updated)
            }
            None => {
                self.channels[cnt] = ch;
                self.channel_count += 1;
                Ok(ChannelUpsert::Added)
            }
        }
    }

    /// Insert or update a contact, keyed on its full 32-byte `pubkey` (the
    /// contact's cryptographic identity).
    ///
    /// A known pubkey updates the existing entry in place (telemetry flag and
    /// display name refreshed; count unchanged) — re-adding the same contact no
    /// longer stacks duplicates.  A new pubkey appends.
    ///
    /// # Why upsert, not append
    ///
    /// The dispatcher's [`PolicyFilter`](protocol::policy::PolicyFilter) and its
    /// telemetry gate (`PolicyFilter::telemetry_enabled` / `contact_pubkey`) are
    /// **first-match-wins** over the contact list.  An appended duplicate would
    /// leave the STALE first entry shadowing the refreshed one — so enabling
    /// telemetry by re-adding a contact would silently fail (the exact
    /// pull-telemetry HIL defect).  Upsert keyed on pubkey is the invariant that
    /// keeps the stored flag and the enforced gate in agreement.  Mirrors
    /// [`upsert_channel`](Self::upsert_channel).
    pub fn upsert_contact(&mut self, c: Contact) -> Result<ContactUpsert, ContactListFull> {
        let cnt = self.contact_count as usize;
        let existing = self.contacts[..cnt].iter().position(|x| x.pubkey == c.pubkey);

        // Capacity only constrains a genuinely new contact; an in-place update
        // reuses the matched slot and can never overflow.
        if existing.is_none() && cnt >= MAX_CONTACTS {
            return Err(ContactListFull);
        }

        match existing {
            Some(idx) => {
                self.contacts[idx] = c;
                Ok(ContactUpsert::Updated)
            }
            None => {
                self.contacts[cnt] = c;
                self.contact_count += 1;
                Ok(ContactUpsert::Added)
            }
        }
    }

    // NOTE: this type intentionally has no `contact_by_hash` / `is_known_contact`
    // / `telemetry_enabled_for` query helpers. The live allowlist and telemetry
    // gate are `protocol::policy::PolicyFilter::contact_pubkey` /
    // `PolicyFilter::telemetry_enabled`, populated from this config's contact
    // list at boot (see `main.rs::run()`) — that is the single enforced gate.
    // A second, unused implementation of the same lookup here would be a
    // redundant source of truth that could silently drift from the enforced
    // one; deleted rather than kept as dead code.
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Check whether this device has been provisioned (flash flag is set).
///
/// Does NOT load the full config blob; suitable for the boot gate.
pub fn is_provisioned(nvs_partition: EspNvsPartition<NvsDefault>) -> Result<bool, EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    Ok(nvs.get_u8(NVS_KEY_PROV_FLAG)?.unwrap_or(0) == 1)
}

/// Load the provisioned config from NVS.
///
/// Returns `Ok(None)` if the device is unprovisioned (or if the blob is
/// missing / corrupt — in which case reprovisioning is required).
/// Returns `Ok(Some(config))` if the device is provisioned and the blob
/// deserialises without error.
pub fn load_provisioned_config(
    nvs_partition: EspNvsPartition<NvsDefault>,
) -> Result<Option<ProvisionedConfig>, EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;

    // Check the provisioned flag first.
    if nvs.get_u8(NVS_KEY_PROV_FLAG)?.unwrap_or(0) != 1 {
        return Ok(None);
    }

    // Read the config blob.
    let mut blob = [0u8; MAX_BLOB_LEN];
    match nvs.get_blob(NVS_KEY_CFG_BLOB, &mut blob)? {
        Some(bytes) if bytes.len() >= CFG_HEADER_LEN && bytes[0] == CFG_VERSION => {
            match deserialize_config(bytes) {
                Some(cfg) => Ok(Some(cfg)),
                None => {
                    log::warn!("config_store: blob deserialization failed — treating as unprovisioned");
                    Ok(None)
                }
            }
        }
        Some(bytes) => {
            log::warn!(
                "config_store: blob version mismatch or truncated ({} bytes, version=0x{:02x}) — reprovisioning required",
                bytes.len(),
                bytes.first().copied().unwrap_or(0xFF)
            );
            Ok(None)
        }
        None => {
            // Provisioned flag was set but blob is missing — inconsistent NVS state.
            log::warn!("config_store: prov flag set but cfg_blob absent — treating as unprovisioned");
            Ok(None)
        }
    }
}

/// Save `config` to NVS and set the provisioned flag.
///
/// This is the atomic commit step: the blob is written first, then the flag.
/// If the blob write fails the flag remains unset (UNPROVISIONED state
/// is preserved — a correct invariant).
pub fn save_provisioned_config(
    nvs_partition: EspNvsPartition<NvsDefault>,
    config: &ProvisionedConfig,
) -> Result<(), EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;

    // Serialise the config.
    let mut blob = [0u8; MAX_BLOB_LEN];
    let blob_len = serialize_config(config, &mut blob);

    // Write the blob first, then the flag (atomicity property).
    nvs.set_blob(NVS_KEY_CFG_BLOB, &blob[..blob_len])?;
    nvs.set_u8(NVS_KEY_PROV_FLAG, 1)?;

    log::info!(
        "config_store: provisioning committed — {} contacts, {} channels, blob {} bytes",
        config.contact_count,
        config.channel_count,
        blob_len,
    );
    Ok(())
}

/// Clear the provisioned flag (and optionally the blob) — for factory reset.
///
/// After this call, `is_provisioned` returns `false` and the next boot enters
/// the UNPROVISIONED state.
///
/// No caller today — there is no factory-reset trigger anywhere in the
/// firmware (no menu row, no host command) yet. Kept as the primitive that
/// feature will need.
#[allow(dead_code)]
pub fn clear_provisioned_flag(nvs_partition: EspNvsPartition<NvsDefault>) -> Result<(), EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    nvs.set_u8(NVS_KEY_PROV_FLAG, 0)?;
    log::info!("config_store: provisioned flag cleared (factory reset path)");
    Ok(())
}

// ── Serialisation helpers ─────────────────────────────────────────────────────

fn serialize_config(cfg: &ProvisionedConfig, out: &mut [u8]) -> usize {
    out[0]  = CFG_VERSION;
    out[1]  = cfg.contact_count;
    out[2]  = cfg.channel_count;
    out[3]  = cfg.lock_flags;
    out[4]  = cfg.pin_len;
    out[5..5 + MAX_PIN_LEN].copy_from_slice(&cfg.pin);
    out[5 + MAX_PIN_LEN]     = cfg.notif_defaults.visual  as u8;
    out[5 + MAX_PIN_LEN + 1] = cfg.notif_defaults.audible as u8;
    out[23..27].copy_from_slice(&cfg.radio_preset.freq_hz.to_le_bytes());
    out[27] = cfg.radio_preset.bw_code;
    out[28] = cfg.radio_preset.sf;
    out[29] = cfg.radio_preset.cr;
    out[30] = cfg.radio_preset.tx_power_dbm;

    let mut off = CFG_HEADER_LEN;
    for i in 0..cfg.contact_count as usize {
        let c = &cfg.contacts[i];
        out[off..off + 32].copy_from_slice(&c.pubkey);
        out[off + 32] = c.telemetry_enable as u8;
        out[off + 33] = c.display_name_len;
        out[off + 34..off + CONTACT_ENTRY_LEN].copy_from_slice(&c.display_name);
        off += CONTACT_ENTRY_LEN;
    }
    for i in 0..cfg.channel_count as usize {
        let ch = &cfg.channels[i];
        out[off..off + 32].copy_from_slice(&ch.secret);
        out[off + 32] = ch.key_len;
        out[off + 33] = ch.primary as u8;
        out[off + 34] = ch.name_len;
        out[off + 35..off + CHANNEL_ENTRY_LEN].copy_from_slice(&ch.name);
        off += CHANNEL_ENTRY_LEN;
    }
    off
}

fn deserialize_config(blob: &[u8]) -> Option<ProvisionedConfig> {
    if blob.len() < CFG_HEADER_LEN || blob[0] != CFG_VERSION {
        return None;
    }

    let contact_count = blob[1] as usize;
    let channel_count = blob[2] as usize;
    if contact_count > MAX_CONTACTS || channel_count > MAX_CHANNELS {
        return None;
    }

    let required = CFG_HEADER_LEN
        + contact_count * CONTACT_ENTRY_LEN
        + channel_count * CHANNEL_ENTRY_LEN;
    if blob.len() < required {
        return None;
    }

    let lock_flags    = blob[3];
    let pin_len       = blob[4];
    let mut pin       = [0u8; MAX_PIN_LEN];
    pin.copy_from_slice(&blob[5..5 + MAX_PIN_LEN]);
    let notif_visual  = blob[5 + MAX_PIN_LEN] != 0;
    let notif_audible = blob[5 + MAX_PIN_LEN + 1] != 0;
    let freq_hz       = u32::from_le_bytes([blob[23], blob[24], blob[25], blob[26]]);
    let bw_code       = blob[27];
    let sf            = blob[28];
    let cr            = blob[29];
    let tx_power_dbm  = blob[30];

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

    let mut contacts = [null_contact; MAX_CONTACTS];
    let mut channels = [null_channel; MAX_CHANNELS];

    let mut off = CFG_HEADER_LEN;
    for i in 0..contact_count {
        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(&blob[off..off + 32]);
        let telemetry_enable = blob[off + 32] != 0;
        let display_name_len = blob[off + 33];
        let mut display_name = [0u8; MAX_NAME_LEN];
        display_name.copy_from_slice(&blob[off + 34..off + CONTACT_ENTRY_LEN]);
        contacts[i] = Contact { pubkey, telemetry_enable, display_name, display_name_len };
        off += CONTACT_ENTRY_LEN;
    }
    for i in 0..channel_count {
        let mut secret = [0u8; 32];
        secret.copy_from_slice(&blob[off..off + 32]);
        let key_len  = blob[off + 32];
        let primary  = blob[off + 33] != 0;
        let name_len = blob[off + 34];
        let mut name = [0u8; MAX_NAME_LEN];
        name.copy_from_slice(&blob[off + 35..off + CHANNEL_ENTRY_LEN]);
        channels[i] = Channel { secret, key_len, primary, name, name_len };
        off += CHANNEL_ENTRY_LEN;
    }

    Some(ProvisionedConfig {
        contacts,
        contact_count: contact_count as u8,
        channels,
        channel_count: channel_count as u8,
        radio_preset: RadioPreset { freq_hz, bw_code, sf, cr, tx_power_dbm },
        notif_defaults: NotifDefaults { visual: notif_visual, audible: notif_audible },
        pin,
        pin_len,
        lock_flags,
    })
}
