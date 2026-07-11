// SPDX-License-Identifier: GPL-3.0-only
//! MeshCadet allowlist policy layer.
//!
//! This module owns the **allowlist-only filter** that sits between the radio
//! dispatcher and the application.  Every inbound frame classification decision
//! routes through here before any crypto or application logic runs.
//!
//! # Rules enforced
//!
//! | Rule | Implementation |
//! |------|----------------|
//! | Allowlist-only DMs | [`PolicyFilter::allow_inbound_dm`] — `false` → silent DROP |
//! | ACK only known contacts | Corollary: ACK path is only reached when policy passes |
//! | Never emit ADVERT | [`PolicyFilter::is_advert_type`] — guard for TX path |
//! | No auto-discovery | Contacts are never auto-added; only `add_contact` mutates the list |
//! | No public channels | Enforced by `add_contact` / `add_channel` being admin-only |
//! | Telemetry pull gating | [`PolicyFilter::telemetry_enabled`] — `false` → no GPS reply |
//!
//! # Silence invariant
//!
//! **An unknown sender receives NO response of any kind** — no ACK, no NACK, no
//! error log visible on the wire.  The filter returns `false` from
//! `allow_inbound_dm` and the caller is expected to `return` immediately,
//! emitting nothing.  This prevents presence detection by unenrolled nodes.
//!
//! # no_std compatibility
//!
//! This module has no heap allocation and no `std` dependency.  It compiles for
//! the ESP32-S3 (Xtensa / esp-idf) target without changes.

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of contacts the policy filter can hold.
///
/// Matches `config_store::MAX_CONTACTS` — sized so the filter can mirror the
/// full NVS provisioned contact list without overflow.
pub const MAX_POLICY_CONTACTS: usize = 16;

/// Wire payload type for ADVERT frames (MeshCore `PAYLOAD_TYPE_ADVERT`, 0x04).
///
/// MeshCadet MUST NEVER emit this frame type.  Use [`PolicyFilter::is_advert_type`]
/// as a TX guard.
const PAYLOAD_TYPE_ADVERT: u8 = 0x04;

// ── Internal per-contact record ───────────────────────────────────────────────

/// Compact per-contact record stored in the policy filter.
///
/// We store the full 32-byte public key so `handle_dm` / `handle_path_return`
/// can compute the per-contact ECDH shared secret on demand, without keeping a
/// separate precomputed-secret array.
#[derive(Clone, Copy)]
struct PolicyContact {
    /// Full Ed25519 public key (32 bytes).  The 1-byte routing hash used for
    /// quick allowlist lookup is `pubkey[0]`.
    pubkey: [u8; 32],
    /// Whether this contact may pull GPS telemetry from us.
    telemetry: bool,
}

impl PolicyContact {
    /// 1-byte routing hash: `pubkey[0]`.
    #[inline]
    fn pub_hash(&self) -> u8 {
        self.pubkey[0]
    }
}

// ── PolicyFilter ──────────────────────────────────────────────────────────────

/// Runtime allowlist filter implementing MeshCadet's allowlist policy.
///
/// Built once at boot (from the NVS provisioned config in production, or from
/// compiled-in constants in HIL builds) and then consulted for every inbound
/// frame classification decision.
///
/// The contact list is fixed at construction time; no dynamic mutation is
/// supported in the running system.  The filter is O(n) in the number of
/// registered contacts (n ≤ [`MAX_POLICY_CONTACTS`] = 16), which is
/// negligible compared to AES / HMAC crypto costs.
pub struct PolicyFilter {
    contacts: [PolicyContact; MAX_POLICY_CONTACTS],
    count: usize,
}

impl PolicyFilter {
    /// Create an empty filter.
    ///
    /// An empty filter blocks ALL inbound DMs (no contacts are known yet).
    /// Populate with [`add_contact`](Self::add_contact) before entering the
    /// receive loop.
    pub const fn new() -> Self {
        Self {
            contacts: [PolicyContact {
                pubkey: [0u8; 32],
                telemetry: false,
            }; MAX_POLICY_CONTACTS],
            count: 0,
        }
    }

    /// Register a contact in the allowlist.
    ///
    /// `pubkey` is the contact's full Ed25519 public key (32 bytes).  The
    /// 1-byte routing hash (`pubkey[0]`) is derived automatically.
    ///
    /// Silently ignores overflow beyond [`MAX_POLICY_CONTACTS`].  Duplicate
    /// pub-hashes are allowed (the first match wins in lookups), but in
    /// practice the provisioning store deduplicates by pubkey.
    pub fn add_contact(&mut self, pubkey: &[u8; 32], telemetry: bool) {
        if self.count < MAX_POLICY_CONTACTS {
            self.contacts[self.count] = PolicyContact {
                pubkey: *pubkey,
                telemetry,
            };
            self.count += 1;
        }
    }

    /// Return `true` if an inbound DM from `src_hash` should be processed.
    ///
    /// `src_hash` is the unencrypted source-routing-hash byte from the DM wire
    /// payload (`payload[1]`).  When `false`, the caller MUST silently drop the
    /// frame — no ACK, no log entry that reveals our presence.
    ///
    /// # Silence invariant
    ///
    /// This is the primary gate for the allowlist-only DM policy.  Returning
    /// `false` is a silent drop: the caller returns immediately with no
    /// outbound frames and no diagnostic log visible externally.
    pub fn allow_inbound_dm(&self, src_hash: u8) -> bool {
        self.contacts[..self.count]
            .iter()
            .any(|c| c.pub_hash() == src_hash)
    }

    /// Return the full 32-byte public key for the contact identified by `src_hash`.
    ///
    /// Returns `None` if `src_hash` is not in the allowlist.
    ///
    /// Callers that have already checked [`allow_inbound_dm`](Self::allow_inbound_dm)
    /// can safely `unwrap()` the result — the existence check and the pubkey
    /// lookup use the same inner scan.
    pub fn contact_pubkey(&self, src_hash: u8) -> Option<&[u8; 32]> {
        self.contacts[..self.count]
            .iter()
            .find(|c| c.pub_hash() == src_hash)
            .map(|c| &c.pubkey)
    }

    /// Return `true` if `src_hash` belongs to a contact with GPS telemetry pull
    /// enabled.
    ///
    /// Unknown contacts always return `false` — no telemetry data is leaked to
    /// nodes that are not in the allowlist.
    ///
    /// This is the gating hook for the pull-only telemetry policy (M3 GPS
    /// integration provides the actual GPS fix).  The call site: when a telemetry
    /// request arrives, check this before constructing any location response.
    pub fn telemetry_enabled(&self, src_hash: u8) -> bool {
        self.contacts[..self.count]
            .iter()
            .find(|c| c.pub_hash() == src_hash)
            .map(|c| c.telemetry)
            .unwrap_or(false)
    }

    /// Return `true` if `payload_type` is the ADVERT payload type.
    ///
    /// MeshCadet MUST NEVER emit ADVERT frames — adverts would leak the
    /// device's presence to the open mesh.  Use this as a TX-path guard:
    ///
    /// ```text
    /// debug_assert!(!PolicyFilter::is_advert_type(payload_type),
    ///     "policy violation: attempted to enqueue an ADVERT frame");
    /// ```
    ///
    /// Value: `0x04` (MeshCore `PAYLOAD_TYPE_ADVERT`).
    #[inline]
    pub fn is_advert_type(payload_type: u8) -> bool {
        payload_type == PAYLOAD_TYPE_ADVERT
    }

    /// Return the number of registered contacts.
    #[inline]
    pub fn contact_count(&self) -> usize {
        self.count
    }
}

impl Default for PolicyFilter {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: construct a 32-byte pubkey whose first byte is `hash`.
    fn pubkey_with_hash(hash: u8) -> [u8; 32] {
        let mut pk = [0u8; 32];
        pk[0] = hash;
        pk
    }

    // ── Acceptance criterion 1: unknown-contact DMs are dropped silently ──────

    #[test]
    fn unknown_contact_dm_blocked() {
        // Acceptance: "unknown-contact DMs are dropped silently"
        let mut policy = PolicyFilter::new();
        let known_pk = pubkey_with_hash(0x11);
        policy.add_contact(&known_pk, false);

        // 0x42 is NOT a registered contact → allow_inbound_dm must return false.
        assert!(
            !policy.allow_inbound_dm(0x42),
            "DM from unknown src_hash 0x42 must be blocked"
        );
    }

    #[test]
    fn empty_filter_blocks_all_dms() {
        // Edge case: no contacts registered → every src_hash is unknown.
        let policy = PolicyFilter::new();
        for hash in [0x00u8, 0x11, 0x42, 0xFF] {
            assert!(
                !policy.allow_inbound_dm(hash),
                "empty filter must block src_hash 0x{:02x}",
                hash
            );
        }
    }

    #[test]
    fn unknown_contact_gets_no_pubkey() {
        // contact_pubkey returns None for unknown hashes — caller cannot compute
        // ECDH shared secret and therefore cannot accidentally ACK.
        let policy = PolicyFilter::new();
        assert!(policy.contact_pubkey(0xAB).is_none());
    }

    // ── Acceptance criterion 2: known-contact DMs are ACKed ──────────────────

    #[test]
    fn known_contact_dm_allowed() {
        // Acceptance: "known-contact DMs are ACKed" (policy gate passes → ACK path reached)
        let mut policy = PolicyFilter::new();
        let pk_alice = pubkey_with_hash(0x11);
        let pk_bob = pubkey_with_hash(0x42);
        policy.add_contact(&pk_alice, false);
        policy.add_contact(&pk_bob, true);

        assert!(
            policy.allow_inbound_dm(0x11),
            "DM from known src_hash 0x11 (Alice) must be allowed"
        );
        assert!(
            policy.allow_inbound_dm(0x42),
            "DM from known src_hash 0x42 (Bob) must be allowed"
        );
    }

    #[test]
    fn known_contact_pubkey_returned() {
        // When the policy passes, the caller must be able to retrieve the full
        // pubkey for ECDH shared-secret derivation.
        let mut policy = PolicyFilter::new();
        let pk = pubkey_with_hash(0x55);
        policy.add_contact(&pk, false);

        let found = policy.contact_pubkey(0x55);
        assert!(
            found.is_some(),
            "contact_pubkey must return Some for known hash"
        );
        assert_eq!(
            *found.unwrap(),
            pk,
            "returned pubkey must match the registered one"
        );
    }

    #[test]
    fn multiple_contacts_independent() {
        // With multiple contacts, each is independently allowed/blocked.
        let mut policy = PolicyFilter::new();
        policy.add_contact(&pubkey_with_hash(0xAA), false);
        policy.add_contact(&pubkey_with_hash(0xBB), true);

        assert!(policy.allow_inbound_dm(0xAA));
        assert!(policy.allow_inbound_dm(0xBB));
        assert!(!policy.allow_inbound_dm(0xCC)); // not registered
    }

    // ── Acceptance criterion 3: no ADVERT frame is ever emitted ─────────────

    #[test]
    fn advert_type_detected() {
        // Acceptance: "no advert frame is ever emitted"
        // is_advert_type(0x04) must return true so TX guards can block emission.
        assert!(
            PolicyFilter::is_advert_type(0x04),
            "0x04 (PAYLOAD_TYPE_ADVERT) must be detected as advert"
        );
    }

    #[test]
    fn non_advert_types_not_blocked() {
        // Non-advert payload types must NOT be flagged (would block legitimate TX).
        for pt in [0x02u8, 0x03, 0x05, 0x07, 0x08] {
            assert!(
                !PolicyFilter::is_advert_type(pt),
                "payload type 0x{:02x} incorrectly flagged as advert",
                pt
            );
        }
    }

    // ── Telemetry gating ──────────────────────────────────────────────────────

    #[test]
    fn telemetry_unknown_contact_denied() {
        let policy = PolicyFilter::new();
        assert!(
            !policy.telemetry_enabled(0x11),
            "unknown contact must not have telemetry access"
        );
    }

    #[test]
    fn telemetry_flag_respected() {
        let mut policy = PolicyFilter::new();
        let no_telem = pubkey_with_hash(0xAA);
        let yes_telem = pubkey_with_hash(0xBB);
        policy.add_contact(&no_telem, false);
        policy.add_contact(&yes_telem, true);

        assert!(
            !policy.telemetry_enabled(0xAA),
            "telemetry=false contact must be denied"
        );
        assert!(
            policy.telemetry_enabled(0xBB),
            "telemetry=true contact must be allowed"
        );
    }

    #[test]
    fn duplicate_pubkey_first_match_shadows_telemetry() {
        // Regression guard for the pull-telemetry HIL defect: if a contact is
        // registered TWICE under the same pubkey (e.g. re-added to enable
        // telemetry without an upsert), lookups are first-match-wins — so the
        // STALE first entry (telemetry=false) shadows the later telemetry=true
        // one and the pull is silently dropped. The fix is upsert-on-add at the
        // provisioning layer (config_store::upsert_contact); this test pins the
        // shadowing behavior so the upsert invariant cannot silently regress.
        let mut policy = PolicyFilter::new();
        let pk = pubkey_with_hash(0x33);
        policy.add_contact(&pk, false); // stale entry first
        policy.add_contact(&pk, true); // "enabled" re-add appended as a duplicate
        assert!(
            !policy.telemetry_enabled(0x33),
            "first-match-wins: the stale telemetry=false entry shadows the later \
             telemetry=true duplicate — provisioning MUST upsert, not append"
        );
    }

    // ── Overflow protection ────────────────────────────────────────────────────

    #[test]
    fn overflow_beyond_max_silently_ignored() {
        let mut policy = PolicyFilter::new();
        // Fill to capacity
        for i in 0..MAX_POLICY_CONTACTS {
            policy.add_contact(&pubkey_with_hash(i as u8), false);
        }
        assert_eq!(policy.contact_count(), MAX_POLICY_CONTACTS);

        // One more — should not panic; just silently ignored
        policy.add_contact(&pubkey_with_hash(0xFF), false);
        assert_eq!(
            policy.contact_count(),
            MAX_POLICY_CONTACTS,
            "count must not exceed capacity"
        );
    }

    // ── contact_count ─────────────────────────────────────────────────────────

    #[test]
    fn contact_count_tracks_additions() {
        let mut policy = PolicyFilter::new();
        assert_eq!(policy.contact_count(), 0);
        policy.add_contact(&pubkey_with_hash(0x01), false);
        assert_eq!(policy.contact_count(), 1);
        policy.add_contact(&pubkey_with_hash(0x02), true);
        assert_eq!(policy.contact_count(), 2);
    }
}
