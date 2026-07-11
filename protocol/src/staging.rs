// SPDX-License-Identifier: GPL-3.0-only
//! Provisioning staging state machine.
//!
//! Pure, `no_std`-compatible in-memory buffer for an in-progress provisioning
//! session.  Enforces the three invariants that `firmware::provisioning_server::
//! process_frame` must maintain and that are otherwise exercised only by the
//! live device:
//!
//! 1. **Contact list capacity** — `ADD_CONTACT` is rejected when `contact_count`
//!    reaches `MAX_STAGING_CONTACTS`.
//! 2. **DEL_CONTACT shift-down** — deleting a contact at index `i` left-shifts
//!    `contacts[i+1..]` so the list remains dense.
//! 3. **ADD_CHANNEL primary exclusivity** — when a new primary channel is added,
//!    all existing channels have their `primary` flag cleared first.
//! 4. **Channel list capacity** — `ADD_CHANNEL` is rejected when `channel_count`
//!    reaches `MAX_STAGING_CHANNELS`.
//! 5. **DEL_CHANNEL shift-down** — same shift-down as contacts.
//!
//! These functions are the **testable core** extracted from `process_frame`
//! (which cannot be tested host-side because it takes an
//! `EspNvsPartition<NvsDefault>` argument).  The protocol-level unit tests here
//! give regression coverage for the firmware logic.  A future refactoring task
//! should have the firmware import and call these functions directly instead of
//! duplicating the logic.

use crate::provisioning::MAX_NAME_LEN;

// ── Capacity limits ───────────────────────────────────────────────────────────

/// Maximum number of provisioned contacts.  Mirrors `config_store::MAX_CONTACTS`.
pub const MAX_STAGING_CONTACTS: usize = 16;

/// Maximum number of provisioned channels.  Mirrors `config_store::MAX_CHANNELS`.
pub const MAX_STAGING_CHANNELS: usize = 8;

// ── Entry types ───────────────────────────────────────────────────────────────

/// A contact entry in the staging buffer.  Field layout matches
/// `config_store::Contact` so that a `From` conversion is trivial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StagingContact {
    /// Ed25519 public key (32 bytes).
    pub pubkey: [u8; 32],
    /// Whether this contact may pull GPS telemetry.
    pub telemetry_enable: bool,
    /// UTF-8 display name, zero-padded to `MAX_NAME_LEN`.
    pub display_name: [u8; MAX_NAME_LEN],
    /// Actual byte length of `display_name`.
    pub display_name_len: u8,
}

impl StagingContact {
    /// A zeroed-out null contact (used to fill unused slots).
    pub const NULL: Self = Self {
        pubkey: [0u8; 32],
        telemetry_enable: false,
        display_name: [0u8; MAX_NAME_LEN],
        display_name_len: 0,
    };
}

/// A channel entry in the staging buffer.  Field layout matches
/// `config_store::Channel` so that a `From` conversion is trivial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StagingChannel {
    /// 32-byte symmetric channel secret.
    pub secret: [u8; 32],
    /// If `true`, this is the primary (default outgoing) channel.
    pub primary: bool,
    /// UTF-8 channel name, zero-padded to `MAX_NAME_LEN`.
    pub name: [u8; MAX_NAME_LEN],
    /// Actual byte length of `name`.
    pub name_len: u8,
}

impl StagingChannel {
    /// A zeroed-out null channel (used to fill unused slots).
    pub const NULL: Self = Self {
        secret: [0u8; 32],
        primary: false,
        name: [0u8; MAX_NAME_LEN],
        name_len: 0,
    };
}

/// Outcome of [`ProvisioningStaging::upsert_channel`].
///
/// Mirrors `config_store::ChannelUpsert` in the firmware crate so the
/// host-side regression tests exercise the same upsert contract the device
/// enforces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelUpsert {
    /// An existing channel with the same secret was updated in place
    /// (count unchanged).
    Updated,
    /// A new channel was appended (count incremented by one).
    Added,
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors returned by `ProvisioningStaging` mutation methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagingError {
    /// `ADD_CONTACT` rejected: list already holds `MAX_STAGING_CONTACTS` entries.
    ContactListFull,
    /// `ADD_CHANNEL` rejected: list already holds `MAX_STAGING_CHANNELS` entries.
    ChannelListFull,
    /// `DEL_CONTACT` rejected: no contact with the given public key exists.
    ContactNotFound,
    /// `DEL_CHANNEL` rejected: no channel with the given secret exists.
    ChannelNotFound,
}

// ── ProvisioningStaging ───────────────────────────────────────────────────────

/// In-memory staging buffer for a provisioning session.
///
/// Maintains the contact and channel lists while the host CLI sends commands,
/// and enforces the capacity and ordering invariants required by ADR-0002.
///
/// # Invariants
/// - `contact_count <= MAX_STAGING_CONTACTS`
/// - `channel_count <= MAX_STAGING_CHANNELS`
/// - At most one channel has `primary == true` at any point.
/// - After a delete at index `i`, entries `[i+1..]` are shifted left by one;
///   the list remains dense (no gaps).
pub struct ProvisioningStaging {
    contacts: [StagingContact; MAX_STAGING_CONTACTS],
    contact_count: usize,
    channels: [StagingChannel; MAX_STAGING_CHANNELS],
    channel_count: usize,
}

impl ProvisioningStaging {
    /// Create a new, empty staging buffer.
    pub fn new() -> Self {
        Self {
            contacts:      [StagingContact::NULL; MAX_STAGING_CONTACTS],
            contact_count: 0,
            channels:      [StagingChannel::NULL; MAX_STAGING_CHANNELS],
            channel_count: 0,
        }
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    /// Number of staged contacts.
    pub fn contact_count(&self) -> usize { self.contact_count }

    /// Number of staged channels.
    pub fn channel_count(&self) -> usize { self.channel_count }

    /// Slice of the live (non-null) staged contacts.
    pub fn contacts(&self) -> &[StagingContact] {
        &self.contacts[..self.contact_count]
    }

    /// Slice of the live (non-null) staged channels.
    pub fn channels(&self) -> &[StagingChannel] {
        &self.channels[..self.channel_count]
    }

    // ── Mutation methods ──────────────────────────────────────────────────────

    /// Append a contact to the staging list.
    ///
    /// Returns `Err(ContactListFull)` if `contact_count == MAX_STAGING_CONTACTS`.
    pub fn add_contact(&mut self, contact: StagingContact) -> Result<(), StagingError> {
        if self.contact_count >= MAX_STAGING_CONTACTS {
            return Err(StagingError::ContactListFull);
        }
        self.contacts[self.contact_count] = contact;
        self.contact_count += 1;
        Ok(())
    }

    /// Remove the contact whose `pubkey` matches `pubkey`, shifting later
    /// entries left so the list stays dense.
    ///
    /// Returns `Err(ContactNotFound)` if no matching contact exists.
    pub fn del_contact(&mut self, pubkey: &[u8; 32]) -> Result<(), StagingError> {
        let idx = self.contacts[..self.contact_count]
            .iter()
            .position(|c| &c.pubkey == pubkey)
            .ok_or(StagingError::ContactNotFound)?;
        // Shift entries after `idx` left by one to fill the gap.
        for j in idx..self.contact_count - 1 {
            self.contacts[j] = self.contacts[j + 1];
        }
        self.contact_count -= 1;
        Ok(())
    }

    /// Append a channel to the staging list.
    ///
    /// **Primary-exclusivity invariant**: if `channel.primary` is `true`, all
    /// existing channels in the list have their `primary` flag cleared before
    /// the new channel is appended.  This guarantees at most one primary channel.
    ///
    /// Returns `Err(ChannelListFull)` if `channel_count == MAX_STAGING_CHANNELS`.
    pub fn add_channel(&mut self, channel: StagingChannel) -> Result<(), StagingError> {
        if self.channel_count >= MAX_STAGING_CHANNELS {
            return Err(StagingError::ChannelListFull);
        }
        // Enforce at-most-one-primary: clear primary on all existing channels
        // before inserting this one if it claims the primary slot.
        if channel.primary {
            for existing in self.channels[..self.channel_count].iter_mut() {
                existing.primary = false;
            }
        }
        self.channels[self.channel_count] = channel;
        self.channel_count += 1;
        Ok(())
    }

    /// Insert or update a channel, keyed on its `secret` (the channel's
    /// cryptographic identity — the on-air `channel_hash` derives from it).
    ///
    /// Idempotent upsert: this is the host-testable mirror of
    /// `config_store::ProvisionedConfig::upsert_channel`, the shared add-channel
    /// core used by both firmware servers.
    ///
    /// - **Known key → update in place.** If a channel with the same `secret`
    ///   already exists, that entry is refreshed (name, `primary`) and
    ///   `channel_count` is left UNCHANGED — re-adding a known key RENAMES the
    ///   existing channel instead of stacking a duplicate. Returns
    ///   [`ChannelUpsert::Updated`].
    /// - **New key → append.** An unseen secret is appended; returns
    ///   [`ChannelUpsert::Added`], or `Err(ChannelListFull)` when a genuinely
    ///   new key would overflow [`MAX_STAGING_CHANNELS`] (an in-place update
    ///   never fails on capacity).
    ///
    /// **Single-primary invariant.** When the inserted/updated channel has
    /// `primary == true`, every other channel is demoted first, so at most one
    /// channel is ever primary.
    pub fn upsert_channel(
        &mut self,
        channel: StagingChannel,
    ) -> Result<ChannelUpsert, StagingError> {
        let existing = self.channels[..self.channel_count]
            .iter()
            .position(|c| c.secret == channel.secret);

        // Capacity only constrains a genuinely new key; an in-place update
        // reuses the matched slot and can never overflow.
        if existing.is_none() && self.channel_count >= MAX_STAGING_CHANNELS {
            return Err(StagingError::ChannelListFull);
        }

        // Enforce at-most-one-primary before placing this one.
        if channel.primary {
            for c in self.channels[..self.channel_count].iter_mut() {
                c.primary = false;
            }
        }

        match existing {
            Some(idx) => {
                self.channels[idx] = channel;
                Ok(ChannelUpsert::Updated)
            }
            None => {
                self.channels[self.channel_count] = channel;
                self.channel_count += 1;
                Ok(ChannelUpsert::Added)
            }
        }
    }

    /// Remove the channel whose `secret` matches `secret`, shifting later
    /// entries left so the list stays dense.
    ///
    /// Returns `Err(ChannelNotFound)` if no matching channel exists.
    pub fn del_channel(&mut self, secret: &[u8; 32]) -> Result<(), StagingError> {
        let idx = self.channels[..self.channel_count]
            .iter()
            .position(|ch| &ch.secret == secret)
            .ok_or(StagingError::ChannelNotFound)?;
        for j in idx..self.channel_count - 1 {
            self.channels[j] = self.channels[j + 1];
        }
        self.channel_count -= 1;
        Ok(())
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn contact(key_byte: u8) -> StagingContact {
        let mut pubkey = [0u8; 32];
        pubkey[0] = key_byte;
        StagingContact { pubkey, ..StagingContact::NULL }
    }

    fn contact_tele(key_byte: u8, telemetry: bool) -> StagingContact {
        let mut c = contact(key_byte);
        c.telemetry_enable = telemetry;
        c
    }

    fn channel(secret_byte: u8, primary: bool) -> StagingChannel {
        let mut secret = [0u8; 32];
        secret[0] = secret_byte;
        StagingChannel { secret, primary, ..StagingChannel::NULL }
    }

    // ── ADD_CONTACT ───────────────────────────────────────────────────────────

    #[test]
    fn add_contact_appends_in_order() {
        let mut s = ProvisioningStaging::new();
        s.add_contact(contact(0xA1)).unwrap();
        s.add_contact(contact(0xA2)).unwrap();
        assert_eq!(s.contact_count(), 2);
        assert_eq!(s.contacts()[0].pubkey[0], 0xA1, "first contact must be A1");
        assert_eq!(s.contacts()[1].pubkey[0], 0xA2, "second contact must be A2");
    }

    #[test]
    fn add_contact_preserves_telemetry_flag() {
        let mut s = ProvisioningStaging::new();
        s.add_contact(contact_tele(0x01, true)).unwrap();
        s.add_contact(contact_tele(0x02, false)).unwrap();
        assert!(s.contacts()[0].telemetry_enable);
        assert!(!s.contacts()[1].telemetry_enable);
    }

    /// Invariant 1: `ADD_CONTACT` is rejected when the list is at capacity.
    #[test]
    fn add_contact_list_full_returns_error() {
        let mut s = ProvisioningStaging::new();
        for i in 0..MAX_STAGING_CONTACTS {
            let mut pk = [0u8; 32];
            pk[0] = i as u8;
            s.add_contact(StagingContact { pubkey: pk, ..StagingContact::NULL }).unwrap();
        }
        assert_eq!(s.contact_count(), MAX_STAGING_CONTACTS);
        let result = s.add_contact(contact(0xFF));
        assert_eq!(result, Err(StagingError::ContactListFull));
        // Count must not change on error.
        assert_eq!(s.contact_count(), MAX_STAGING_CONTACTS);
    }

    // ── DEL_CONTACT ───────────────────────────────────────────────────────────

    /// Invariant 2: deleting the middle entry shifts later entries left.
    #[test]
    fn del_contact_middle_shifts_left() {
        let mut s = ProvisioningStaging::new();
        s.add_contact(contact(0xA1)).unwrap();
        s.add_contact(contact(0xB2)).unwrap();
        s.add_contact(contact(0xC3)).unwrap();

        let mut key_b2 = [0u8; 32];
        key_b2[0] = 0xB2;
        s.del_contact(&key_b2).unwrap();

        assert_eq!(s.contact_count(), 2);
        assert_eq!(s.contacts()[0].pubkey[0], 0xA1, "A1 must remain at index 0");
        assert_eq!(s.contacts()[1].pubkey[0], 0xC3, "C3 must shift to index 1");
    }

    #[test]
    fn del_contact_first_shifts_all_left() {
        let mut s = ProvisioningStaging::new();
        s.add_contact(contact(0x01)).unwrap();
        s.add_contact(contact(0x02)).unwrap();
        s.add_contact(contact(0x03)).unwrap();

        let mut key_01 = [0u8; 32];
        key_01[0] = 0x01;
        s.del_contact(&key_01).unwrap();

        assert_eq!(s.contact_count(), 2);
        assert_eq!(s.contacts()[0].pubkey[0], 0x02, "02 must shift to index 0");
        assert_eq!(s.contacts()[1].pubkey[0], 0x03, "03 must shift to index 1");
    }

    #[test]
    fn del_contact_last_decrements_count() {
        let mut s = ProvisioningStaging::new();
        s.add_contact(contact(0x01)).unwrap();
        s.add_contact(contact(0x02)).unwrap();

        let mut key_02 = [0u8; 32];
        key_02[0] = 0x02;
        s.del_contact(&key_02).unwrap();

        assert_eq!(s.contact_count(), 1);
        assert_eq!(s.contacts()[0].pubkey[0], 0x01, "01 must remain");
    }

    #[test]
    fn del_contact_not_found_returns_error() {
        let mut s = ProvisioningStaging::new();
        s.add_contact(contact(0xAA)).unwrap();
        let key = [0xBB_u8; 32];
        assert_eq!(s.del_contact(&key), Err(StagingError::ContactNotFound));
        // Count must not change on error.
        assert_eq!(s.contact_count(), 1);
    }

    #[test]
    fn del_contact_empty_list_returns_error() {
        let mut s = ProvisioningStaging::new();
        let key = [0x01_u8; 32];
        assert_eq!(s.del_contact(&key), Err(StagingError::ContactNotFound));
    }

    #[test]
    fn add_del_contact_round_trip() {
        let mut s = ProvisioningStaging::new();
        let key = [0xA1_u8; 32];
        s.add_contact(StagingContact { pubkey: key, telemetry_enable: true, ..StagingContact::NULL })
            .unwrap();
        s.del_contact(&key).unwrap();
        assert_eq!(s.contact_count(), 0);
    }

    // ── ADD_CHANNEL ───────────────────────────────────────────────────────────

    #[test]
    fn add_channel_appends_in_order() {
        let mut s = ProvisioningStaging::new();
        s.add_channel(channel(0x10, false)).unwrap();
        s.add_channel(channel(0x20, false)).unwrap();
        assert_eq!(s.channel_count(), 2);
        assert_eq!(s.channels()[0].secret[0], 0x10);
        assert_eq!(s.channels()[1].secret[0], 0x20);
    }

    /// Invariant 3: adding a primary channel strips `primary` from all existing channels.
    #[test]
    fn add_channel_primary_strips_existing_primaries() {
        let mut s = ProvisioningStaging::new();
        s.add_channel(channel(0x10, true)).unwrap();
        s.add_channel(channel(0x20, false)).unwrap();

        // Baseline: ch[0] is primary, ch[1] is not.
        assert!(s.channels()[0].primary, "ch[0] must start as primary");
        assert!(!s.channels()[1].primary, "ch[1] must start as non-primary");

        // Adding a second primary channel must clear the existing one.
        s.add_channel(channel(0x30, true)).unwrap();

        assert!(!s.channels()[0].primary,
            "ch[0] must lose primary flag when a new primary is added");
        assert!(!s.channels()[1].primary,
            "ch[1] (already non-primary) must remain non-primary");
        assert!(s.channels()[2].primary,
            "ch[2] (newly added) must hold the primary flag");
    }

    #[test]
    fn add_channel_non_primary_does_not_strip_existing_primary() {
        let mut s = ProvisioningStaging::new();
        s.add_channel(channel(0x10, true)).unwrap();

        // Adding a non-primary must NOT disturb the existing primary.
        s.add_channel(channel(0x20, false)).unwrap();

        assert!(s.channels()[0].primary, "ch[0] must remain primary");
        assert!(!s.channels()[1].primary, "ch[1] must be non-primary");
    }

    /// Invariant 4: `ADD_CHANNEL` is rejected when the list is at capacity.
    #[test]
    fn add_channel_list_full_returns_error() {
        let mut s = ProvisioningStaging::new();
        for i in 0..MAX_STAGING_CHANNELS {
            let mut sec = [0u8; 32];
            sec[0] = i as u8;
            s.add_channel(StagingChannel { secret: sec, ..StagingChannel::NULL }).unwrap();
        }
        assert_eq!(s.channel_count(), MAX_STAGING_CHANNELS);
        let result = s.add_channel(channel(0xFF, false));
        assert_eq!(result, Err(StagingError::ChannelListFull));
        // Count must not change on error.
        assert_eq!(s.channel_count(), MAX_STAGING_CHANNELS);
    }

    // ── DEL_CHANNEL ───────────────────────────────────────────────────────────

    /// Invariant 5: deleting the middle channel shifts later channels left.
    #[test]
    fn del_channel_middle_shifts_left() {
        let mut s = ProvisioningStaging::new();
        s.add_channel(channel(0x10, false)).unwrap();
        s.add_channel(channel(0x20, true)).unwrap();
        s.add_channel(channel(0x30, false)).unwrap();

        let mut sec_20 = [0u8; 32];
        sec_20[0] = 0x20;
        s.del_channel(&sec_20).unwrap();

        assert_eq!(s.channel_count(), 2);
        assert_eq!(s.channels()[0].secret[0], 0x10, "ch[0] must remain");
        assert_eq!(s.channels()[1].secret[0], 0x30, "ch[2] must shift to index 1");
    }

    #[test]
    fn del_channel_first_shifts_all_left() {
        let mut s = ProvisioningStaging::new();
        s.add_channel(channel(0x10, true)).unwrap();
        s.add_channel(channel(0x20, false)).unwrap();
        s.add_channel(channel(0x30, false)).unwrap();

        let mut sec_10 = [0u8; 32];
        sec_10[0] = 0x10;
        s.del_channel(&sec_10).unwrap();

        assert_eq!(s.channel_count(), 2);
        assert_eq!(s.channels()[0].secret[0], 0x20);
        assert_eq!(s.channels()[1].secret[0], 0x30);
    }

    #[test]
    fn del_channel_not_found_returns_error() {
        let mut s = ProvisioningStaging::new();
        s.add_channel(channel(0xAA, true)).unwrap();
        let sec = [0xBB_u8; 32];
        assert_eq!(s.del_channel(&sec), Err(StagingError::ChannelNotFound));
        // Count must not change on error.
        assert_eq!(s.channel_count(), 1);
    }

    #[test]
    fn del_channel_empty_list_returns_error() {
        let mut s = ProvisioningStaging::new();
        let sec = [0x01_u8; 32];
        assert_eq!(s.del_channel(&sec), Err(StagingError::ChannelNotFound));
    }

    #[test]
    fn add_del_channel_round_trip() {
        let mut s = ProvisioningStaging::new();
        let secret = [0xCC_u8; 32];
        s.add_channel(StagingChannel { secret, primary: true, ..StagingChannel::NULL }).unwrap();
        s.del_channel(&secret).unwrap();
        assert_eq!(s.channel_count(), 0);
    }

    // ── UPSERT_CHANNEL (dedup-by-key) ─────────────────────────────────────────

    /// Helper: a channel with a given secret byte, primary flag, and a one-byte
    /// name (so renames are observable).
    fn named_channel(secret_byte: u8, primary: bool, name_byte: u8) -> StagingChannel {
        let mut secret = [0u8; 32];
        secret[0] = secret_byte;
        let mut name = [0u8; MAX_NAME_LEN];
        name[0] = name_byte;
        StagingChannel { secret, primary, name, name_len: 1 }
    }

    /// Regression: re-adding the SAME key updates in place — count stays put,
    /// no cryptographically-identical duplicate is stacked. (This is the exact
    /// HIL failure: 8×#home, same hash 0x98, from repeated add-channel.)
    #[test]
    fn upsert_channel_same_key_updates_in_place_no_duplicate() {
        let mut s = ProvisioningStaging::new();
        assert_eq!(s.upsert_channel(named_channel(0x98, false, b'a')), Ok(ChannelUpsert::Added));
        assert_eq!(s.channel_count(), 1);

        // Re-add the same secret seven more times — must never grow the list.
        for _ in 0..7 {
            assert_eq!(
                s.upsert_channel(named_channel(0x98, false, b'a')),
                Ok(ChannelUpsert::Updated),
                "re-adding a known key must update in place, not append"
            );
        }
        assert_eq!(s.channel_count(), 1, "count must stay at 1 — no duplicates");
        assert_eq!(s.channels()[0].secret[0], 0x98);
    }

    /// Re-adding a known key with a DIFFERENT name renames the existing entry
    /// (does not create a sibling).
    #[test]
    fn upsert_channel_same_key_different_name_renames() {
        let mut s = ProvisioningStaging::new();
        s.upsert_channel(named_channel(0x42, false, b'x')).unwrap();
        assert_eq!(s.channels()[0].name[0], b'x');

        let outcome = s.upsert_channel(named_channel(0x42, false, b'y')).unwrap();
        assert_eq!(outcome, ChannelUpsert::Updated);
        assert_eq!(s.channel_count(), 1, "rename must not append a sibling");
        assert_eq!(s.channels()[0].name[0], b'y', "name must be updated in place");
    }

    /// A new/unseen key still appends normally and increments the count.
    #[test]
    fn upsert_channel_new_key_appends() {
        let mut s = ProvisioningStaging::new();
        assert_eq!(s.upsert_channel(named_channel(0x10, false, b'a')), Ok(ChannelUpsert::Added));
        assert_eq!(s.upsert_channel(named_channel(0x20, false, b'b')), Ok(ChannelUpsert::Added));
        assert_eq!(s.channel_count(), 2);
        assert_eq!(s.channels()[0].secret[0], 0x10);
        assert_eq!(s.channels()[1].secret[0], 0x20);
    }

    /// Single-primary invariant on upsert: promoting one channel to primary
    /// demotes any previously-primary channel — at most one primary survives.
    #[test]
    fn upsert_channel_promoting_primary_demotes_previous() {
        let mut s = ProvisioningStaging::new();
        s.upsert_channel(named_channel(0x10, true, b'a')).unwrap();
        s.upsert_channel(named_channel(0x20, false, b'b')).unwrap();
        assert!(s.channels()[0].primary, "ch0 starts primary");

        // Promote ch1 (a known key) to primary; ch0 must be demoted.
        let outcome = s.upsert_channel(named_channel(0x20, true, b'b')).unwrap();
        assert_eq!(outcome, ChannelUpsert::Updated);
        assert_eq!(s.channel_count(), 2);
        assert!(!s.channels()[0].primary, "ch0 must lose primary");
        assert!(s.channels()[1].primary, "ch1 must now be the sole primary");
        assert_eq!(
            s.channels().iter().filter(|c| c.primary).count(),
            1,
            "exactly one primary must remain"
        );
    }

    /// Updating a known key with `primary == false` refreshes that entry's flag
    /// in place and does not disturb the others.
    #[test]
    fn upsert_channel_update_non_primary_does_not_touch_others() {
        let mut s = ProvisioningStaging::new();
        s.upsert_channel(named_channel(0x10, true, b'a')).unwrap();
        s.upsert_channel(named_channel(0x20, false, b'b')).unwrap();

        // Re-upsert ch1 as non-primary: ch0 (the primary) must be untouched.
        s.upsert_channel(named_channel(0x20, false, b'c')).unwrap();
        assert!(s.channels()[0].primary, "ch0 must remain primary");
        assert!(!s.channels()[1].primary, "ch1 stays non-primary");
        assert_eq!(s.channels()[1].name[0], b'c', "ch1 renamed in place");
    }

    /// A genuinely new key overflowing the list returns `ChannelListFull`, but
    /// an in-place update of a KNOWN key at capacity still succeeds (consumes
    /// no new slot).
    #[test]
    fn upsert_channel_capacity_only_blocks_new_keys() {
        let mut s = ProvisioningStaging::new();
        for i in 0..MAX_STAGING_CHANNELS {
            s.upsert_channel(named_channel(i as u8, false, b'a')).unwrap();
        }
        assert_eq!(s.channel_count(), MAX_STAGING_CHANNELS);

        // New key at capacity → rejected, count unchanged.
        assert_eq!(
            s.upsert_channel(named_channel(0xFF, false, b'a')),
            Err(StagingError::ChannelListFull)
        );
        assert_eq!(s.channel_count(), MAX_STAGING_CHANNELS);

        // Known key at capacity → still updates in place.
        assert_eq!(
            s.upsert_channel(named_channel(0x00, true, b'z')),
            Ok(ChannelUpsert::Updated)
        );
        assert_eq!(s.channel_count(), MAX_STAGING_CHANNELS);
        assert!(s.channels()[0].primary, "in-place update applied");
    }

    // ── Combined round-trip ───────────────────────────────────────────────────

    /// Full provisioning sequence: add contacts + channels, del one of each,
    /// verify final state is consistent.
    #[test]
    fn full_provisioning_staging_sequence() {
        let mut s = ProvisioningStaging::new();

        let alice = [0xA1_u8; 32];
        let bob   = [0xB0_u8; 32];
        let family_sec = [0xFA_u8; 32];
        let backup_sec = [0xBB_u8; 32];

        // Add two contacts.
        s.add_contact(StagingContact { pubkey: alice, telemetry_enable: true, ..StagingContact::NULL }).unwrap();
        s.add_contact(StagingContact { pubkey: bob, telemetry_enable: false, ..StagingContact::NULL }).unwrap();
        assert_eq!(s.contact_count(), 2);

        // Add a primary channel, then a non-primary.
        s.add_channel(StagingChannel { secret: family_sec, primary: true, ..StagingChannel::NULL }).unwrap();
        s.add_channel(StagingChannel { secret: backup_sec, primary: false, ..StagingChannel::NULL }).unwrap();
        assert_eq!(s.channel_count(), 2);
        assert!(s.channels()[0].primary, "family must be primary");
        assert!(!s.channels()[1].primary, "backup must be non-primary");

        // Remove Bob (middle/last contact).
        s.del_contact(&bob).unwrap();
        assert_eq!(s.contact_count(), 1);
        assert_eq!(s.contacts()[0].pubkey, alice, "alice must remain");

        // Remove backup channel, add a new primary (which must steal primary from family).
        s.del_channel(&backup_sec).unwrap();
        let mesh_sec = [0xCC_u8; 32];
        s.add_channel(StagingChannel { secret: mesh_sec, primary: true, ..StagingChannel::NULL }).unwrap();

        assert_eq!(s.channel_count(), 2);
        assert!(!s.channels()[0].primary, "family must have lost primary");
        assert!(s.channels()[1].primary, "mesh must now be primary");
    }
}
