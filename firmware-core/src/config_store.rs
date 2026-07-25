// SPDX-License-Identifier: GPL-3.0-only
//! Provisioning config store — blob codec + capacity-checked structs.
//!
//! This module is the pure byte-slice codec (`serialize_config`/
//! `deserialize_config`) plus every provisioned-config struct
//! (`Contact`/`Channel`/`RoomExtra`/`ProvisionedConfig`) and their upsert/
//! lookup helpers. The `EspNvs` read/write wrapper (`is_provisioned`/
//! `load_provisioned_config`/`save_provisioned_config`) stays in
//! `firmware::config_store` — it needs a real NVS partition — and
//! re-exports this module via `pub use firmware_core::config_store::*;` so
//! its tests execute under `cargo test --workspace` (this crate is a
//! detached, cross-compiled workspace — see `Cargo.toml`'s doc comment — so
//! a `#[cfg(test)]` block written there would type-check but never run).
//! See `docs/adr/0005-firmware-core-extraction.md`.
//!
//! # NVS layout (firmware side)
//!
//! | Namespace | Key          | Type  | Contents                               |
//! |-----------|--------------|-------|----------------------------------------|
//! | `mc_cfg`  | `prov`       | u8    | 0 = unprovisioned, 1 = provisioned    |
//! | `mc_cfg`  | `cfg_blob`   | blob  | Serialised `ProvisionedConfig` binary  |
//!
//! # Serialisation format (internal, version-tagged)
//!
//! Current version `0x03` (bumped from `0x02` when room-server contacts —
//! `role` + room extras — were added):
//!
//! ```text
//! byte 0       version = 0x03
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
//! byte 31      room_count     (0–MAX_CONTACTS; ADDED in v0x03)
//! — for each contact (contact_count × CONTACT_ENTRY_LEN bytes): —
//!   bytes +0..+31   pubkey (32)
//!   byte  +32       telemetry_enable
//!   byte  +33       role            (0 = chat, 3 = room; ADDED in v0x03)
//!   byte  +34       display_name_len
//!   bytes +35..+66  display_name (MAX_NAME_LEN, zero-padded)
//! — for each channel (channel_count × CHANNEL_ENTRY_LEN bytes, unchanged since v0x02): —
//!   bytes +0..+31   secret (32)
//!   byte  +32       key_len (16 or 32)
//!   byte  +33       primary
//!   byte  +34       name_len
//!   bytes +35..+66  name (MAX_NAME_LEN, zero-padded)
//! — for each room (room_count × ROOM_EXTRA_ENTRY_LEN bytes; ADDED in v0x03): —
//!   bytes +0..+31    pubkey (32) — the room's identity; keys this entry to
//!                    its `Contact` (NOT a positional/array-index link — see
//!                    `RoomExtra`'s own doc comment for why)
//!   byte  +32        guest_password_len
//!   bytes +33..+48   guest_password (MAX_LOGIN_PASSWORD_LEN, zero-padded)
//!   bytes +49..+52   sync_since (little-endian u32)
//!   byte  +53        permissions (raw `protocol::room::RoomPermission` byte)
//!   byte  +54        out_path_len
//!   bytes +55..+118  out_path (MAX_PATH_SIZE, zero-padded)
//! ```
//!
//! # Forward migration (v0x02 → v0x03)
//!
//! A v0x02 blob (pre-room-server) has no `role` byte on its contact entries,
//! no `room_count` header byte, and no room section at all. [`deserialize_config`]
//! recognises `CFG_VERSION_V2` and loads it losslessly: every contact and
//! channel intact, every contact's `role` defaulted to [`ROLE_CHAT`], zero
//! rooms. A provisioned device in the field is never bricked, wiped, or
//! silently reset to unprovisioned by this firmware upgrade — the very next
//! [`save_config`]/`save_provisioned_config` call persists it forward as a
//! full v0x03 blob. There is deliberately no v0x01 migration path (that
//! predates the current codebase's channel `key_len` field and was already
//! a hard reprovisioning boundary before this mission).
//!
//! # Blob size budget
//!
//! `MAX_BLOB_LEN` = `CFG_HEADER_LEN + MAX_CONTACTS×CONTACT_ENTRY_LEN +
//! MAX_CHANNELS×CHANNEL_ENTRY_LEN + MAX_CONTACTS×ROOM_EXTRA_ENTRY_LEN` = `32 +
//! 16×67 + 8×67 + 16×119` = **3544 bytes**, well within a 24 KB NVS partition.
//! `room_count` is capacity-bounded by `MAX_CONTACTS` (every room is also a
//! contact — see [`ProvisionedConfig::upsert_room`]), so the worst case
//! (every configured contact is a room) is the figure budgeted above; there
//! is no separate `MAX_ROOMS` constant to keep in sync.

use protocol::constants::MAX_PATH_SIZE;
use protocol::room::{RoomPermission, MAX_LOGIN_PASSWORD_LEN};

// ── Capacity limits ───────────────────────────────────────────────────────────

/// Maximum number of provisioned contacts.
pub const MAX_CONTACTS: usize = 16;

/// Maximum number of provisioned channels.
pub const MAX_CHANNELS: usize = 8;

/// Maximum byte length of a contact display name or channel name.
pub const MAX_NAME_LEN: usize = 32;

/// Maximum byte length of the PIN.
pub const MAX_PIN_LEN: usize = 16;

// ── Contact role (mirrors MeshCore's advert node-type nibble; see `Contact::role`) ──

/// A plain chat contact — the zero value, so a v0x02 blob (which has no
/// `role` byte at all) migrates every existing contact to this role for
/// free, just by zero-filling. Deliberately NOT `1` (upstream's advert
/// node-type nibble uses `1 = chat`) — see the module doc's migration
/// section for why `0` has to mean "chat" here.
pub const ROLE_CHAT: u8 = 0;
/// A room-server contact. Reuses upstream's advert node-type nibble value
/// (`3 = room server`) directly — see `docs/adr/0002-provisioning-wire-format.md`
/// §7 — so this byte can be passed straight through as the `type=` value of
/// a room's `meshcore://contact/add` URI with no translation.
pub const ROLE_ROOM: u8 = 3;

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

// ── Serialisation constants ───────────────────────────────────────────────────

/// Current blob format version — bumped from `0x02` when room-server
/// contacts (`role` + room extras) were added.
pub const CFG_VERSION: u8 = 0x03;
/// Pre-room-server blob format version. [`deserialize_config`] still accepts
/// this for the forward migration — see the module doc's "Forward migration"
/// section.
pub const CFG_VERSION_V2: u8 = 0x02;

const CFG_HEADER_LEN: usize = 32;
const CFG_HEADER_LEN_V2: usize = 31; // pre-`room_count` header (v0x02)
const CONTACT_ENTRY_LEN: usize = 67; // pubkey(32) + telemetry(1) + role(1) + name_len(1) + name(32)
const CONTACT_ENTRY_LEN_V2: usize = 66; // pre-`role` byte (v0x02)
const CHANNEL_ENTRY_LEN: usize = 67; // secret(32) + key_len(1) + primary(1) + name_len(1) + name(32); unchanged since v0x02
/// pubkey(32) + pw_len(1) + pw(MAX_LOGIN_PASSWORD_LEN=16) + sync_since(4) +
/// permissions(1) + out_path_len(1) + out_path(MAX_PATH_SIZE=64) = 119.
const ROOM_EXTRA_ENTRY_LEN: usize = 32 + 1 + MAX_LOGIN_PASSWORD_LEN + 4 + 1 + 1 + MAX_PATH_SIZE;

/// See the module doc's "Blob size budget" section for the full breakdown.
pub const MAX_BLOB_LEN: usize = CFG_HEADER_LEN
    + MAX_CONTACTS * CONTACT_ENTRY_LEN
    + MAX_CHANNELS * CHANNEL_ENTRY_LEN
    + MAX_CONTACTS * ROOM_EXTRA_ENTRY_LEN; // = 3544 bytes

// ── Config structs ────────────────────────────────────────────────────────────

/// A provisioned contact entry.
#[derive(Clone, Copy, Debug)]
pub struct Contact {
    /// Ed25519 public key (32 bytes).
    pub pubkey: [u8; 32],
    /// Whether this contact may pull our GPS telemetry.
    pub telemetry_enable: bool,
    /// [`ROLE_CHAT`] or [`ROLE_ROOM`]. A room-role contact's room-specific
    /// state (guest password, `sync_since`, `permissions`, `out_path`) lives
    /// in the paired [`RoomExtra`] entry, looked up by `pubkey` — see
    /// [`ProvisionedConfig::room_extra`].
    pub role: u8,
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

    /// Whether this contact is a room server (`role == ROLE_ROOM`) — the
    /// one-field predicate a Contacts-tab filter / Groups-tab union needs.
    pub fn is_room(&self) -> bool {
        self.role == ROLE_ROOM
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

/// Room-specific persisted state for a `role == ROLE_ROOM` contact.
///
/// Keyed by [`pubkey`](Self::pubkey), NOT a positional index into
/// `ProvisionedConfig::contacts` — a contact delete/reorder (e.g.
/// `FRAME_DEL_CONTACT`'s existing compaction shift in `admin_server.rs`/
/// `provisioning_server.rs`) must never desync this from its owning contact,
/// which a parallel fixed-index array would risk. [`ProvisionedConfig::upsert_room`]
/// / [`ProvisionedConfig::remove_room`] / [`ProvisionedConfig::room_extra`] /
/// [`ProvisionedConfig::room_extra_mut`] are the only ways this list should
/// be mutated or read.
#[derive(Clone, Copy, Debug)]
pub struct RoomExtra {
    /// The room's Ed25519 public key — links this entry to its `Contact`.
    pub pubkey: [u8; 32],
    /// UTF-8 guest password, zero-padded to `MAX_LOGIN_PASSWORD_LEN`. Crosses
    /// the USB link in the clear (ADR-0001 §4: the cable is the
    /// authentication) — never logged, echoed, or placed in an error message.
    pub guest_password: [u8; MAX_LOGIN_PASSWORD_LEN],
    /// Actual byte length of `guest_password`.
    pub guest_password_len: u8,
    /// Persisted sync watermark: posts with `post_ts <= sync_since` have
    /// already been delivered. MUST survive reboot — see the module doc.
    pub sync_since: u32,
    /// Raw `protocol::room::RoomPermission` byte last granted at login. See
    /// [`Self::permission`] for the decoded form.
    pub permissions: u8,
    /// Learned mesh-route path to the room server, zero-padded to
    /// `MAX_PATH_SIZE`. Empty (`out_path_len == 0`) until a login or
    /// keep-alive response has taught the device a path.
    pub out_path: [u8; MAX_PATH_SIZE],
    /// Actual byte length of `out_path` (0 ⇒ no path learned yet).
    pub out_path_len: u8,
}

impl RoomExtra {
    /// An empty/unset room extras slot — array-fill sentinel, never a live
    /// room (see `ProvisionedConfig::room_extras`' `room_count`-bounded
    /// prefix invariant).
    pub const EMPTY: Self = Self {
        pubkey: [0u8; 32],
        guest_password: [0u8; MAX_LOGIN_PASSWORD_LEN],
        guest_password_len: 0,
        sync_since: 0,
        permissions: 0,
        out_path: [0u8; MAX_PATH_SIZE],
        out_path_len: 0,
    };

    /// The decoded permission role — see [`RoomPermission::from_u8`].
    pub fn permission(&self) -> RoomPermission {
        RoomPermission::from_u8(self.permissions)
    }
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

/// Outcome of [`ProvisionedConfig::upsert_contact`] / [`ProvisionedConfig::upsert_room`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactUpsert {
    /// An existing contact with the same pubkey was updated in place
    /// (telemetry flag / role / display name refreshed; count unchanged).
    Updated,
    /// A new contact was appended (count incremented by one).
    Added,
}

/// Returned by [`ProvisionedConfig::upsert_contact`] / [`ProvisionedConfig::upsert_room`]
/// when a genuinely new contact cannot be appended because the contact list
/// is at [`MAX_CONTACTS`].
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
            freq_hz: DEFAULT_FREQ_HZ,
            bw_code: DEFAULT_BW_CODE,
            sf: DEFAULT_SF,
            cr: DEFAULT_CR,
            tx_power_dbm: DEFAULT_TX_POWER_DBM,
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
        Self {
            visual: true,
            audible: true,
        }
    }
}

/// The full provisioned configuration persisted to flash.
///
/// Returned by `load_provisioned_config` when the device has been
/// provisioned. Written by `save_provisioned_config` at the end of a
/// provisioning session.
#[derive(Clone, Debug)]
pub struct ProvisionedConfig {
    pub contacts: [Contact; MAX_CONTACTS],
    pub contact_count: u8,
    pub channels: [Channel; MAX_CHANNELS],
    pub channel_count: u8,
    /// Room-specific extras, keyed by pubkey — see [`RoomExtra`]'s doc
    /// comment. The live prefix is `room_extras[..room_count]`.
    pub room_extras: [RoomExtra; MAX_CONTACTS],
    pub room_count: u8,
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
    /// An empty config: zero contacts, zero channels, zero rooms, default
    /// radio/notif settings, no PIN, no locks. Used as the admin-server
    /// fallback when a provisioned device's config blob is missing or fails
    /// to load — the server still answers queries (reporting zero entries)
    /// and accepts edits rather than hanging the host.
    pub fn empty() -> Self {
        let null_contact = Contact {
            pubkey: [0u8; 32],
            telemetry_enable: false,
            role: ROLE_CHAT,
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
            contacts: [null_contact; MAX_CONTACTS],
            contact_count: 0,
            channels: [null_channel; MAX_CHANNELS],
            channel_count: 0,
            room_extras: [RoomExtra::EMPTY; MAX_CONTACTS],
            room_count: 0,
            radio_preset: RadioPreset::default(),
            notif_defaults: NotifDefaults::default(),
            pin: [0u8; MAX_PIN_LEN],
            pin_len: 0,
            lock_flags: 0,
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
        let existing = self.channels[..cnt]
            .iter()
            .position(|c| c.secret == ch.secret);

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
    /// A known pubkey updates the existing entry in place (telemetry flag,
    /// role, and display name refreshed; count unchanged) — re-adding the
    /// same contact no longer stacks duplicates. A new pubkey appends.
    ///
    /// Note this can also flip an existing room's `role` back to
    /// [`ROLE_CHAT`] if called with a plain contact sharing a room's pubkey
    /// (e.g. the `FRAME_ADD_CONTACT` path) — same "last write wins" semantics
    /// this upsert already has for every other field. Prefer [`Self::upsert_room`]
    /// for room contacts, which forces `role` and keeps the paired
    /// [`RoomExtra`] in sync.
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
        let existing = self.contacts[..cnt]
            .iter()
            .position(|x| x.pubkey == c.pubkey);

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

    /// Insert or update a room-server contact: upserts the `Contact` entry
    /// (forcing `role = ROLE_ROOM` regardless of what `contact.role` was set
    /// to) AND its paired [`RoomExtra`] (forcing `extra.pubkey =
    /// contact.pubkey`), keyed by pubkey in both lists.
    ///
    /// Capacity is bounded solely by [`MAX_CONTACTS`] (via the inner
    /// `upsert_contact` call) — there is no separate room-capacity check,
    /// since `room_count` can never exceed `contact_count` (every room this
    /// method ever adds is also added as a contact in the same call).
    pub fn upsert_room(
        &mut self,
        mut contact: Contact,
        mut extra: RoomExtra,
    ) -> Result<ContactUpsert, ContactListFull> {
        contact.role = ROLE_ROOM;
        extra.pubkey = contact.pubkey;
        let outcome = self.upsert_contact(contact)?;

        let cnt = self.room_count as usize;
        match self.room_extras[..cnt]
            .iter()
            .position(|r| r.pubkey == extra.pubkey)
        {
            Some(idx) => self.room_extras[idx] = extra,
            None => {
                // Always in-bounds: room_count <= contact_count <= MAX_CONTACTS,
                // and upsert_contact above already returned Err on a genuinely
                // new contact that would overflow MAX_CONTACTS.
                self.room_extras[cnt] = extra;
                self.room_count += 1;
            }
        }
        Ok(outcome)
    }

    /// Remove a room: its [`RoomExtra`] entry AND its `Contact` entry (a
    /// room's identity IS its contact entry). Returns whether a contact with
    /// this pubkey was found and removed (mirrors `FRAME_DEL_CONTACT`'s
    /// compaction-shift semantics for both lists).
    pub fn remove_room(&mut self, pubkey: &[u8; 32]) -> bool {
        let rcnt = self.room_count as usize;
        if let Some(idx) = self.room_extras[..rcnt]
            .iter()
            .position(|r| &r.pubkey == pubkey)
        {
            for j in idx..rcnt - 1 {
                self.room_extras[j] = self.room_extras[j + 1];
            }
            self.room_count -= 1;
        }

        let ccnt = self.contact_count as usize;
        match self.contacts[..ccnt]
            .iter()
            .position(|c| &c.pubkey == pubkey)
        {
            Some(idx) => {
                for j in idx..ccnt - 1 {
                    self.contacts[j] = self.contacts[j + 1];
                }
                self.contact_count -= 1;
                true
            }
            None => false,
        }
    }

    /// Look up a room's extras by pubkey, if one is configured.
    pub fn room_extra(&self, pubkey: &[u8; 32]) -> Option<&RoomExtra> {
        let cnt = self.room_count as usize;
        self.room_extras[..cnt].iter().find(|r| &r.pubkey == pubkey)
    }

    /// Mutable lookup — the primitive later session logic uses to persist
    /// learned `sync_since` / `permissions` / `out_path` back to this config
    /// (the caller is responsible for then persisting the mutated
    /// `ProvisionedConfig` to NVS, same contract as every other field here).
    pub fn room_extra_mut(&mut self, pubkey: &[u8; 32]) -> Option<&mut RoomExtra> {
        let cnt = self.room_count as usize;
        self.room_extras[..cnt]
            .iter_mut()
            .find(|r| &r.pubkey == pubkey)
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

// ── Serialisation helpers ─────────────────────────────────────────────────────

/// Serialise `config` as a current-version (`CFG_VERSION`) blob into `out`.
/// Returns the number of bytes written.
pub fn serialize_config(cfg: &ProvisionedConfig, out: &mut [u8]) -> usize {
    out[0] = CFG_VERSION;
    out[1] = cfg.contact_count;
    out[2] = cfg.channel_count;
    out[3] = cfg.lock_flags;
    out[4] = cfg.pin_len;
    out[5..5 + MAX_PIN_LEN].copy_from_slice(&cfg.pin);
    out[5 + MAX_PIN_LEN] = cfg.notif_defaults.visual as u8;
    out[5 + MAX_PIN_LEN + 1] = cfg.notif_defaults.audible as u8;
    out[23..27].copy_from_slice(&cfg.radio_preset.freq_hz.to_le_bytes());
    out[27] = cfg.radio_preset.bw_code;
    out[28] = cfg.radio_preset.sf;
    out[29] = cfg.radio_preset.cr;
    out[30] = cfg.radio_preset.tx_power_dbm;
    out[31] = cfg.room_count;

    let mut off = CFG_HEADER_LEN;
    for i in 0..cfg.contact_count as usize {
        let c = &cfg.contacts[i];
        out[off..off + 32].copy_from_slice(&c.pubkey);
        out[off + 32] = c.telemetry_enable as u8;
        out[off + 33] = c.role;
        out[off + 34] = c.display_name_len;
        out[off + 35..off + CONTACT_ENTRY_LEN].copy_from_slice(&c.display_name);
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
    for i in 0..cfg.room_count as usize {
        let r = &cfg.room_extras[i];
        out[off..off + 32].copy_from_slice(&r.pubkey);
        out[off + 32] = r.guest_password_len;
        out[off + 33..off + 33 + MAX_LOGIN_PASSWORD_LEN].copy_from_slice(&r.guest_password);
        let p = off + 33 + MAX_LOGIN_PASSWORD_LEN;
        out[p..p + 4].copy_from_slice(&r.sync_since.to_le_bytes());
        out[p + 4] = r.permissions;
        out[p + 5] = r.out_path_len;
        out[p + 6..p + 6 + MAX_PATH_SIZE].copy_from_slice(&r.out_path);
        off += ROOM_EXTRA_ENTRY_LEN;
    }
    off
}

/// Deserialise a config blob, dispatching on its version byte:
/// [`CFG_VERSION`] (current, full room-aware format) or [`CFG_VERSION_V2`]
/// (pre-room-server — migrated losslessly; see the module doc's "Forward
/// migration" section). Any other version (including the older, unsupported
/// v0x01) returns `None`, same as a structurally-corrupt blob.
pub fn deserialize_config(blob: &[u8]) -> Option<ProvisionedConfig> {
    match *blob.first()? {
        CFG_VERSION => deserialize_v3(blob),
        CFG_VERSION_V2 => deserialize_v2_migrate(blob),
        _ => None,
    }
}

fn null_contact() -> Contact {
    Contact {
        pubkey: [0u8; 32],
        telemetry_enable: false,
        role: ROLE_CHAT,
        display_name: [0u8; MAX_NAME_LEN],
        display_name_len: 0,
    }
}

fn null_channel() -> Channel {
    Channel {
        secret: [0u8; 32],
        key_len: 32,
        primary: false,
        name: [0u8; MAX_NAME_LEN],
        name_len: 0,
    }
}

/// Shared header-field parse for both blob versions — identical byte
/// offsets `[3..31]` in both v0x02 and v0x03 (only `byte 31` `room_count` is
/// v0x03-only, handled by the caller).
struct Header {
    lock_flags: u8,
    pin_len: u8,
    pin: [u8; MAX_PIN_LEN],
    notif_visual: bool,
    notif_audible: bool,
    radio_preset: RadioPreset,
}

fn parse_header(blob: &[u8]) -> Header {
    let lock_flags = blob[3];
    let pin_len = blob[4];
    let mut pin = [0u8; MAX_PIN_LEN];
    pin.copy_from_slice(&blob[5..5 + MAX_PIN_LEN]);
    let notif_visual = blob[5 + MAX_PIN_LEN] != 0;
    let notif_audible = blob[5 + MAX_PIN_LEN + 1] != 0;
    let freq_hz = u32::from_le_bytes([blob[23], blob[24], blob[25], blob[26]]);
    Header {
        lock_flags,
        pin_len,
        pin,
        notif_visual,
        notif_audible,
        radio_preset: RadioPreset {
            freq_hz,
            bw_code: blob[27],
            sf: blob[28],
            cr: blob[29],
            tx_power_dbm: blob[30],
        },
    }
}

fn deserialize_v3(blob: &[u8]) -> Option<ProvisionedConfig> {
    if blob.len() < CFG_HEADER_LEN {
        return None;
    }

    let contact_count = blob[1] as usize;
    let channel_count = blob[2] as usize;
    let room_count = blob[31] as usize;
    if contact_count > MAX_CONTACTS || channel_count > MAX_CHANNELS || room_count > MAX_CONTACTS {
        return None;
    }
    // Every room is also a contact (see `upsert_room`) — a blob claiming
    // more rooms than contacts violates that invariant and is corrupt.
    if room_count > contact_count {
        return None;
    }

    let required = CFG_HEADER_LEN
        + contact_count * CONTACT_ENTRY_LEN
        + channel_count * CHANNEL_ENTRY_LEN
        + room_count * ROOM_EXTRA_ENTRY_LEN;
    if blob.len() < required {
        return None;
    }

    let header = parse_header(blob);

    let mut contacts = [null_contact(); MAX_CONTACTS];
    let mut channels = [null_channel(); MAX_CHANNELS];
    let mut room_extras = [RoomExtra::EMPTY; MAX_CONTACTS];

    let mut off = CFG_HEADER_LEN;
    for c in contacts.iter_mut().take(contact_count) {
        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(&blob[off..off + 32]);
        let telemetry_enable = blob[off + 32] != 0;
        let role = blob[off + 33];
        let display_name_len = blob[off + 34];
        let mut display_name = [0u8; MAX_NAME_LEN];
        display_name.copy_from_slice(&blob[off + 35..off + CONTACT_ENTRY_LEN]);
        *c = Contact {
            pubkey,
            telemetry_enable,
            role,
            display_name,
            display_name_len,
        };
        off += CONTACT_ENTRY_LEN;
    }
    for ch in channels.iter_mut().take(channel_count) {
        let mut secret = [0u8; 32];
        secret.copy_from_slice(&blob[off..off + 32]);
        let key_len = blob[off + 32];
        let primary = blob[off + 33] != 0;
        let name_len = blob[off + 34];
        let mut name = [0u8; MAX_NAME_LEN];
        name.copy_from_slice(&blob[off + 35..off + CHANNEL_ENTRY_LEN]);
        *ch = Channel {
            secret,
            key_len,
            primary,
            name,
            name_len,
        };
        off += CHANNEL_ENTRY_LEN;
    }
    for r in room_extras.iter_mut().take(room_count) {
        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(&blob[off..off + 32]);
        let guest_password_len = blob[off + 32];
        if guest_password_len as usize > MAX_LOGIN_PASSWORD_LEN {
            return None;
        }
        let mut guest_password = [0u8; MAX_LOGIN_PASSWORD_LEN];
        guest_password.copy_from_slice(&blob[off + 33..off + 33 + MAX_LOGIN_PASSWORD_LEN]);
        let p = off + 33 + MAX_LOGIN_PASSWORD_LEN;
        let sync_since = u32::from_le_bytes(blob[p..p + 4].try_into().unwrap());
        let permissions = blob[p + 4];
        let out_path_len = blob[p + 5];
        if out_path_len as usize > MAX_PATH_SIZE {
            return None;
        }
        let mut out_path = [0u8; MAX_PATH_SIZE];
        out_path.copy_from_slice(&blob[p + 6..p + 6 + MAX_PATH_SIZE]);
        *r = RoomExtra {
            pubkey,
            guest_password,
            guest_password_len,
            sync_since,
            permissions,
            out_path,
            out_path_len,
        };
        off += ROOM_EXTRA_ENTRY_LEN;
    }

    Some(ProvisionedConfig {
        contacts,
        contact_count: contact_count as u8,
        channels,
        channel_count: channel_count as u8,
        room_extras,
        room_count: room_count as u8,
        radio_preset: header.radio_preset,
        notif_defaults: NotifDefaults {
            visual: header.notif_visual,
            audible: header.notif_audible,
        },
        pin: header.pin,
        pin_len: header.pin_len,
        lock_flags: header.lock_flags,
    })
}

/// Migrate a pre-room-server (`CFG_VERSION_V2`) blob: every contact and
/// channel intact, `role` defaulted to [`ROLE_CHAT`] (there is no `role`
/// byte at this version — every contact was implicitly chat-only), zero
/// rooms. See the module doc's "Forward migration" section.
fn deserialize_v2_migrate(blob: &[u8]) -> Option<ProvisionedConfig> {
    if blob.len() < CFG_HEADER_LEN_V2 {
        return None;
    }

    let contact_count = blob[1] as usize;
    let channel_count = blob[2] as usize;
    if contact_count > MAX_CONTACTS || channel_count > MAX_CHANNELS {
        return None;
    }

    let required = CFG_HEADER_LEN_V2
        + contact_count * CONTACT_ENTRY_LEN_V2
        + channel_count * CHANNEL_ENTRY_LEN;
    if blob.len() < required {
        return None;
    }

    let header = parse_header(blob);

    let mut contacts = [null_contact(); MAX_CONTACTS];
    let mut channels = [null_channel(); MAX_CHANNELS];

    let mut off = CFG_HEADER_LEN_V2;
    for c in contacts.iter_mut().take(contact_count) {
        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(&blob[off..off + 32]);
        let telemetry_enable = blob[off + 32] != 0;
        let display_name_len = blob[off + 33];
        let mut display_name = [0u8; MAX_NAME_LEN];
        display_name.copy_from_slice(&blob[off + 34..off + CONTACT_ENTRY_LEN_V2]);
        *c = Contact {
            pubkey,
            telemetry_enable,
            role: ROLE_CHAT,
            display_name,
            display_name_len,
        };
        off += CONTACT_ENTRY_LEN_V2;
    }
    for ch in channels.iter_mut().take(channel_count) {
        let mut secret = [0u8; 32];
        secret.copy_from_slice(&blob[off..off + 32]);
        let key_len = blob[off + 32];
        let primary = blob[off + 33] != 0;
        let name_len = blob[off + 34];
        let mut name = [0u8; MAX_NAME_LEN];
        name.copy_from_slice(&blob[off + 35..off + CHANNEL_ENTRY_LEN]);
        *ch = Channel {
            secret,
            key_len,
            primary,
            name,
            name_len,
        };
        off += CHANNEL_ENTRY_LEN;
    }

    Some(ProvisionedConfig {
        contacts,
        contact_count: contact_count as u8,
        channels,
        channel_count: channel_count as u8,
        room_extras: [RoomExtra::EMPTY; MAX_CONTACTS],
        room_count: 0,
        radio_preset: header.radio_preset,
        notif_defaults: NotifDefaults {
            visual: header.notif_visual,
            audible: header.notif_audible,
        },
        pin: header.pin,
        pin_len: header.pin_len,
        lock_flags: header.lock_flags,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Pure functions over byte slices and in-memory structs — no NVS/hardware
// required. These EXECUTE under `cargo test --workspace` (this module lives
// in `firmware-core`, a root-workspace member).
#[cfg(test)]
mod tests {
    use super::*;

    fn contact(pubkey_byte: u8, role: u8, name: &[u8]) -> Contact {
        let mut display_name = [0u8; MAX_NAME_LEN];
        display_name[..name.len()].copy_from_slice(name);
        Contact {
            pubkey: [pubkey_byte; 32],
            telemetry_enable: pubkey_byte.is_multiple_of(2),
            role,
            display_name,
            display_name_len: name.len() as u8,
        }
    }

    fn channel(secret_byte: u8, primary: bool, name: &[u8]) -> Channel {
        let mut n = [0u8; MAX_NAME_LEN];
        n[..name.len()].copy_from_slice(name);
        Channel {
            secret: [secret_byte; 32],
            key_len: 32,
            primary,
            name: n,
            name_len: name.len() as u8,
        }
    }

    // ── MAX_BLOB_LEN budget ──────────────────────────────────────────────────

    #[test]
    fn max_blob_len_matches_documented_budget_and_fits_nvs_partition() {
        assert_eq!(MAX_BLOB_LEN, 32 + 16 * 67 + 8 * 67 + 16 * 119);
        assert_eq!(MAX_BLOB_LEN, 3544);
        const NVS_PARTITION_BUDGET: usize = 24 * 1024;
        const {
            assert!(
                MAX_BLOB_LEN < NVS_PARTITION_BUDGET,
                "v0x03 blob must fit the 24 KB NVS partition"
            );
        }
    }

    // ── v0x03 roundtrip (contacts + channels + rooms) ───────────────────────

    #[test]
    fn v3_roundtrip_preserves_contacts_channels_and_rooms() {
        let mut cfg = ProvisionedConfig::empty();
        cfg.upsert_contact(contact(0x11, ROLE_CHAT, b"Alice"))
            .unwrap();
        cfg.upsert_channel(channel(0x22, true, b"family")).unwrap();

        let room_pubkey = [0x33u8; 32];
        let mut guest_password = [0u8; MAX_LOGIN_PASSWORD_LEN];
        guest_password[..7].copy_from_slice(b"hunter2");
        let mut out_path = [0u8; MAX_PATH_SIZE];
        out_path[..2].copy_from_slice(&[0xAA, 0xBB]);
        cfg.upsert_room(
            contact(
                0x33, ROLE_CHAT, /* forced to ROLE_ROOM regardless */
                b"Lobby",
            ),
            RoomExtra {
                pubkey: room_pubkey,
                guest_password,
                guest_password_len: 7,
                sync_since: 42,
                permissions: RoomPermission::ReadWrite as u8,
                out_path,
                out_path_len: 2,
            },
        )
        .unwrap();

        let mut blob = [0u8; MAX_BLOB_LEN];
        let n = serialize_config(&cfg, &mut blob);
        let restored = deserialize_config(&blob[..n]).expect("v3 blob must deserialize");

        assert_eq!(restored.contact_count, 2, "chat contact + room contact");
        assert_eq!(restored.channel_count, 1);
        assert_eq!(restored.room_count, 1);

        let chat = restored.contacts[..restored.contact_count as usize]
            .iter()
            .find(|c| c.pubkey == [0x11; 32])
            .unwrap();
        assert_eq!(chat.role, ROLE_CHAT);
        assert!(!chat.is_room());

        let room = restored.contacts[..restored.contact_count as usize]
            .iter()
            .find(|c| c.pubkey == [0x33; 32])
            .unwrap();
        assert_eq!(room.role, ROLE_ROOM, "upsert_room must force ROLE_ROOM");
        assert!(room.is_room());

        let extra = restored
            .room_extra(&room_pubkey)
            .expect("room extra must persist");
        assert_eq!(extra.guest_password_len, 7);
        assert_eq!(&extra.guest_password[..7], b"hunter2");
        assert_eq!(extra.sync_since, 42);
        assert_eq!(extra.permission(), RoomPermission::ReadWrite);
        assert_eq!(extra.out_path_len, 2);
        assert_eq!(&extra.out_path[..2], &[0xAA, 0xBB]);
    }

    #[test]
    fn upsert_room_is_idempotent_keyed_on_pubkey() {
        let mut cfg = ProvisionedConfig::empty();
        let pubkey = [0x44u8; 32];
        cfg.upsert_room(
            contact(0x44, ROLE_CHAT, b"Lobby"),
            RoomExtra {
                sync_since: 1,
                ..RoomExtra::EMPTY
            },
        )
        .unwrap();
        cfg.upsert_room(
            contact(0x44, ROLE_CHAT, b"Lobby Renamed"),
            RoomExtra {
                sync_since: 99,
                ..RoomExtra::EMPTY
            },
        )
        .unwrap();

        assert_eq!(
            cfg.contact_count, 1,
            "re-adding the same room must not stack"
        );
        assert_eq!(cfg.room_count, 1);
        assert_eq!(cfg.room_extra(&pubkey).unwrap().sync_since, 99);
    }

    #[test]
    fn remove_room_deletes_both_contact_and_extra() {
        let mut cfg = ProvisionedConfig::empty();
        let pubkey = [0x55u8; 32];
        cfg.upsert_contact(contact(0x11, ROLE_CHAT, b"Alice"))
            .unwrap();
        cfg.upsert_room(contact(0x55, ROLE_CHAT, b"Lobby"), RoomExtra::EMPTY)
            .unwrap();
        assert_eq!(cfg.contact_count, 2);
        assert_eq!(cfg.room_count, 1);

        assert!(cfg.remove_room(&pubkey));
        assert_eq!(cfg.contact_count, 1, "only the chat contact remains");
        assert_eq!(cfg.room_count, 0);
        assert!(cfg.room_extra(&pubkey).is_none());
        assert!(!cfg.remove_room(&pubkey), "already removed");
    }

    #[test]
    fn room_extra_mut_persists_learned_state() {
        let mut cfg = ProvisionedConfig::empty();
        let pubkey = [0x66u8; 32];
        cfg.upsert_room(contact(0x66, ROLE_CHAT, b"Lobby"), RoomExtra::EMPTY)
            .unwrap();

        let extra = cfg.room_extra_mut(&pubkey).expect("room must exist");
        extra.sync_since = 1234;
        extra.out_path_len = 3;
        extra.out_path[..3].copy_from_slice(&[1, 2, 3]);

        assert_eq!(cfg.room_extra(&pubkey).unwrap().sync_since, 1234);
        assert_eq!(cfg.room_extra(&pubkey).unwrap().out_path_len, 3);
    }

    // ── v0x02 → v0x03 forward migration (mandatory, lossless) ───────────────

    /// Hand-builds a legacy v0x02 blob byte-for-byte (mirroring the pre-room
    /// serialisation this migration replaces) rather than calling
    /// `serialize_config` (which only ever emits the current version) — this
    /// is the acceptance test that a real device's flash blob, written by
    /// old firmware, still loads.
    fn build_v2_blob(
        contacts: &[([u8; 32], bool, &[u8])],
        channels: &[([u8; 32], u8, bool, &[u8])],
    ) -> Vec<u8> {
        const HEADER_V2: usize = 31;
        const CONTACT_V2: usize = 66;
        const CHANNEL_V2: usize = 67;
        let mut blob =
            vec![0u8; HEADER_V2 + contacts.len() * CONTACT_V2 + channels.len() * CHANNEL_V2];
        blob[0] = CFG_VERSION_V2;
        blob[1] = contacts.len() as u8;
        blob[2] = channels.len() as u8;
        blob[3] = 0x07; // lock_flags — proves it survives migration
        blob[4] = 4; // pin_len
        blob[5..9].copy_from_slice(b"1234");
        blob[21] = 1; // notif_visual
        blob[22] = 0; // notif_audible
        blob[23..27].copy_from_slice(&915_000_000u32.to_le_bytes());
        blob[27] = 1; // bw_code
        blob[28] = 9; // sf
        blob[29] = 2; // cr
        blob[30] = 17; // tx_power_dbm

        let mut off = HEADER_V2;
        for (pubkey, telemetry, name) in contacts {
            blob[off..off + 32].copy_from_slice(pubkey);
            blob[off + 32] = *telemetry as u8;
            blob[off + 33] = name.len() as u8;
            blob[off + 34..off + 34 + name.len()].copy_from_slice(name);
            off += CONTACT_V2;
        }
        for (secret, key_len, primary, name) in channels {
            blob[off..off + 32].copy_from_slice(secret);
            blob[off + 32] = *key_len;
            blob[off + 33] = *primary as u8;
            blob[off + 34] = name.len() as u8;
            blob[off + 35..off + 35 + name.len()].copy_from_slice(name);
            off += CHANNEL_V2;
        }
        blob
    }

    #[test]
    fn v2_blob_migrates_losslessly_every_contact_and_channel_intact_zero_rooms() {
        let contacts: [([u8; 32], bool, &[u8]); 2] =
            [([0x11; 32], true, b"Alice"), ([0x22; 32], false, b"Bob")];
        let channels: [([u8; 32], u8, bool, &[u8]); 1] = [([0x33; 32], 32, true, b"family")];
        let v2_blob = build_v2_blob(&contacts, &channels);

        let restored = deserialize_config(&v2_blob).expect("v0x02 blob must migrate, not brick");

        assert_eq!(restored.contact_count, 2, "no data loss on contacts");
        assert_eq!(restored.channel_count, 1, "no data loss on channels");
        assert_eq!(restored.room_count, 0, "v0x02 predates rooms entirely");

        assert_eq!(restored.contacts[0].pubkey, [0x11; 32]);
        assert!(restored.contacts[0].telemetry_enable);
        assert_eq!(
            restored.contacts[0].role, ROLE_CHAT,
            "role defaults to chat"
        );
        assert_eq!(
            &restored.contacts[0].display_name[..5],
            b"Alice",
            "display name intact"
        );

        assert_eq!(restored.contacts[1].pubkey, [0x22; 32]);
        assert!(!restored.contacts[1].telemetry_enable);
        assert_eq!(restored.contacts[1].role, ROLE_CHAT);

        assert_eq!(restored.channels[0].secret, [0x33; 32]);
        assert_eq!(restored.channels[0].key_len, 32);
        assert!(restored.channels[0].primary);
        assert_eq!(&restored.channels[0].name[..6], b"family");

        // Non-contact/channel header fields also survive.
        assert_eq!(restored.lock_flags, 0x07);
        assert_eq!(restored.pin_len, 4);
        assert_eq!(&restored.pin[..4], b"1234");
        assert!(restored.notif_defaults.visual);
        assert!(!restored.notif_defaults.audible);
        assert_eq!(restored.radio_preset.freq_hz, 915_000_000);

        // The device still reads as provisioned: a config this function
        // returns `Some(..)` for IS the "provisioned" signal one layer up
        // (`load_provisioned_config` only returns `None` on a missing/corrupt
        // blob) — migrating, not rejecting, is what keeps that true.
    }

    #[test]
    fn v2_blob_then_resave_upgrades_to_v3_on_next_write() {
        let contacts: [([u8; 32], bool, &[u8]); 1] = [([0x11; 32], true, b"Alice")];
        let v2_blob = build_v2_blob(&contacts, &[]);
        let migrated = deserialize_config(&v2_blob).unwrap();

        let mut resaved = [0u8; MAX_BLOB_LEN];
        let n = serialize_config(&migrated, &mut resaved);
        assert_eq!(
            resaved[0], CFG_VERSION,
            "next save persists forward as v0x03"
        );

        let reloaded = deserialize_config(&resaved[..n]).unwrap();
        assert_eq!(reloaded.contact_count, 1);
        assert_eq!(reloaded.contacts[0].role, ROLE_CHAT);
        assert_eq!(reloaded.room_count, 0);
    }

    #[test]
    fn unsupported_v1_blob_is_rejected() {
        let mut blob = [0u8; 31];
        blob[0] = 0x01;
        assert!(deserialize_config(&blob).is_none());
    }

    #[test]
    fn corrupt_blob_with_more_rooms_than_contacts_is_rejected() {
        let cfg = ProvisionedConfig::empty();
        let mut blob = [0u8; MAX_BLOB_LEN];
        let n = serialize_config(&cfg, &mut blob);
        // Forge room_count > contact_count (both zero here, so claim 1 room).
        let mut forged = blob[..n].to_vec();
        forged[31] = 1;
        assert!(deserialize_config(&forged).is_none());
        // Sanity: the untouched blob still deserializes fine.
        assert!(deserialize_config(&blob[..n]).is_some());
    }

    #[test]
    fn truncated_blob_is_rejected() {
        let cfg = ProvisionedConfig::empty();
        let mut blob = [0u8; MAX_BLOB_LEN];
        let n = serialize_config(&cfg, &mut blob);
        assert!(deserialize_config(&blob[..n - 1]).is_none());
    }
}
