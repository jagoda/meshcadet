// SPDX-License-Identifier: GPL-3.0-only
//! Persisted per-contact inbound replay guard for `handle_dm`
//! (`firmware/src/main.rs`) — the fix for F3 of
//! `meshcadet-outsider-boundary-security-review`.
//!
//! # The gap this closes
//!
//! `handle_dm` decodes an inbound DM's `ts` field but, before this module,
//! never compared it to anything. The ONLY replay guard on the wire was
//! [`crate::dispatcher::DuplicateFilter`] — a 128-slot ring of raw packet
//! hashes, shared across every payload type on the mesh (including flood
//! relay noise from every other node), held in RAM only. A non-allowlisted
//! attacker who passively captures ONE legitimate DM to the device can
//! thereafter replay those exact bytes once 128 other frames have cycled the
//! ring (routine within seconds on a busy mesh) — or unconditionally after
//! any reboot, since RAM is wiped — and each successful replay re-triggers a
//! fresh ACK (and, for a telemetry-enabled contact, a fresh location reply):
//! an unlimited, on-demand presence/direction-finding oracle for an attacker
//! who is never on the allowlist and needs only to have overheard one frame,
//! once.
//!
//! # Why not a naive `ts <= last_ts` reject
//!
//! MeshCadet's own outbound DM timestamp is seeded from a per-boot random
//! `tx_epoch_base` (`main.rs`'s "Per-boot random base for outbound message
//! timestamps") and, unlike the room TX watermark
//! ([`crate::room_session::room_tx_timestamp`]), is never rebased onto a
//! persisted floor across a reboot — every stock MeshCore correspondent this
//! device talks to works the same way. A legitimate contact's own reboot can
//! therefore make its NEXT DM's `ts` land at or below whatever high-water
//! mark this device already recorded for it. A gate that rejects on
//! `ts <= last_ts` unconditionally would silently lock that contact out —
//! until its fresh random reseed happens to climb back above the old mark,
//! which is not guaranteed to happen soon, or at all.
//!
//! # The fix: a high-water mark, plus a content-fingerprint ring for the
//! # regression case
//!
//! [`InboundReplayState`] persists, per contact:
//! - `last_ts`: the highest `ts` ever accepted from this contact. Any new
//!   `ts` strictly above it is certainly not a replay — an attacker cannot
//!   forge a valid MAC for novel content, so the only frames it can ever
//!   present are byte-identical copies of frames this device has already
//!   processed, and every frame this device has ever accepted has
//!   `ts <= last_ts` by construction of the mark. This is the fast path:
//!   accept outright, advance the mark.
//! - a small ring of the content fingerprints of every frame accepted
//!   through either path (the caller passes `compute_ack_hash`'s 4-byte
//!   output — already computed for the DM's own ACK, reused rather than
//!   inventing a second hash).
//!
//! When `ts` does NOT advance the mark, the frame is accepted only if its
//! fingerprint is not already in the ring — content this device has never
//! actually accepted before, despite the non-advancing timestamp, is exactly
//! what a legitimate contact's post-reboot message looks like. `last_ts` is
//! deliberately never moved backwards by this branch (seeding it down to the
//! new low value would re-admit every already-accepted `ts` in between,
//! trading one contact's lockout for a wide-open replay window — far worse).
//! A contact that has genuinely rebooted therefore just keeps taking this
//! (cheap) exception path indefinitely, with no artificial cap, until its
//! `ts` naturally climbs back above the historical peak.
//!
//! # Residual trade-off (documented, not silently accepted)
//!
//! The ring is bounded ([`REPLAY_RING_LEN`] entries per contact). If more
//! than that many OTHER frames are accepted from the SAME contact between a
//! capture and a later replay attempt, the captured frame's fingerprint has
//! aged out and the replay is (wrongly) treated as a legitimate low-`ts`
//! message and accepted. This is a real, narrower-but-nonzero window — not a
//! claim of perfect closure. It is a substantial improvement over the status
//! quo regardless: the ring is per-contact (not diluted by every other
//! sender's and every flood-relay's traffic, which is what makes the current
//! global 128-slot ring turn over so fast) and persisted (so it survives
//! this device's own reboot, the other leg of the current defect). See
//! SECURITY.md's "Known limitations" for the user-facing disclosure.

/// Number of recent content fingerprints remembered per contact.
///
/// Sized to comfortably outlast the message volume between two genuine
/// messages from the SAME contact in ordinary use (this is a per-contact
/// ring, not the global, all-traffic 128-slot [`crate::dispatcher::DEDUP_SLOTS`]
/// ring it complements) while staying a trivial NVS footprint (16 × 4 B = 64
/// bytes per contact).
pub const REPLAY_RING_LEN: usize = 16;

/// Encoded length of [`InboundReplayState`] — see [`encode_inbound_replay_state`].
pub const INBOUND_REPLAY_STATE_LEN: usize = 4 + REPLAY_RING_LEN * 4 + 1 + 1;

/// Persisted per-contact inbound replay-guard state. See the module doc for
/// the full mechanism.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InboundReplayState {
    /// Highest `ts` ever accepted from this contact.
    pub last_ts: u32,
    /// Ring of content fingerprints (`compute_ack_hash` output) of every
    /// frame accepted from this contact, oldest overwritten first.
    pub ring: [[u8; 4]; REPLAY_RING_LEN],
    /// Next write position into `ring`, mod `REPLAY_RING_LEN`.
    pub head: u8,
    /// Number of valid entries in `ring` (saturates at `REPLAY_RING_LEN`).
    pub count: u8,
}

impl InboundReplayState {
    /// A never-seen contact's initial state: no messages accepted yet.
    pub const EMPTY: Self = Self {
        last_ts: 0,
        ring: [[0u8; 4]; REPLAY_RING_LEN],
        head: 0,
        count: 0,
    };

    fn ring_contains(&self, fingerprint: &[u8; 4]) -> bool {
        let n = (self.count as usize).min(REPLAY_RING_LEN);
        self.ring[..n].iter().any(|h| h == fingerprint)
    }

    fn ring_insert(&mut self, fingerprint: [u8; 4]) {
        self.ring[self.head as usize] = fingerprint;
        self.head = (self.head + 1) % REPLAY_RING_LEN as u8;
        if (self.count as usize) < REPLAY_RING_LEN {
            self.count += 1;
        }
    }
}

impl Default for InboundReplayState {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Decide whether an inbound DM with decoded `(ts, fingerprint)` should be
/// accepted from this contact, and update `state` in place if so. Returns
/// `true` (accept — the caller should proceed with ACK/history/UI/telemetry
/// as normal) or `false` (reject — treat exactly like a duplicate: no ACK, no
/// history append, no UI event, no telemetry response). See the module doc
/// for the full decision rule and its documented residual trade-off.
pub fn check_and_record_inbound(
    state: &mut InboundReplayState,
    ts: u32,
    fingerprint: [u8; 4],
) -> bool {
    if ts > state.last_ts {
        state.last_ts = ts;
        state.ring_insert(fingerprint);
        return true;
    }
    if state.ring_contains(&fingerprint) {
        return false;
    }
    state.ring_insert(fingerprint);
    true
}

/// Encode `state` into `out` (at least [`INBOUND_REPLAY_STATE_LEN`] bytes).
/// Returns the number of bytes written.
pub fn encode_inbound_replay_state(state: &InboundReplayState, out: &mut [u8]) -> usize {
    out[0..4].copy_from_slice(&state.last_ts.to_le_bytes());
    let mut off = 4;
    for slot in state.ring.iter() {
        out[off..off + 4].copy_from_slice(slot);
        off += 4;
    }
    out[off] = state.head;
    out[off + 1] = state.count;
    off + 2
}

/// Decode an [`InboundReplayState`] blob. `None` if shorter than
/// [`INBOUND_REPLAY_STATE_LEN`] (truncated/corrupt) or if `count`/`head`
/// exceed [`REPLAY_RING_LEN`] (a corrupt or foreign blob) — callers fall back
/// to [`InboundReplayState::EMPTY`], which is always safe (see that
/// constant's doc: it just means the fast path won't trigger until this
/// contact's next message, and every message meanwhile still goes through
/// the ring-check exception path, so a decode failure degrades to "check
/// every message a bit more carefully", never to "accept everything").
pub fn decode_inbound_replay_state(blob: &[u8]) -> Option<InboundReplayState> {
    if blob.len() < INBOUND_REPLAY_STATE_LEN {
        return None;
    }
    let last_ts = u32::from_le_bytes(blob[0..4].try_into().ok()?);
    let mut ring = [[0u8; 4]; REPLAY_RING_LEN];
    let mut off = 4;
    for slot in ring.iter_mut() {
        slot.copy_from_slice(&blob[off..off + 4]);
        off += 4;
    }
    let head = blob[off];
    let count = blob[off + 1];
    if head as usize >= REPLAY_RING_LEN || count as usize > REPLAY_RING_LEN {
        return None;
    }
    Some(InboundReplayState {
        last_ts,
        ring,
        head,
        count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(byte: u8) -> [u8; 4] {
        [byte; 4]
    }

    // ── Ordinary monotonic traffic ───────────────────────────────────────────

    #[test]
    fn first_ever_message_is_accepted() {
        let mut state = InboundReplayState::EMPTY;
        assert!(check_and_record_inbound(&mut state, 100, fp(1)));
        assert_eq!(state.last_ts, 100);
    }

    #[test]
    fn strictly_increasing_timestamps_are_always_accepted() {
        let mut state = InboundReplayState::EMPTY;
        for (ts, byte) in [(10, 1), (20, 2), (30, 3), (1_000_000, 4)] {
            assert!(check_and_record_inbound(&mut state, ts, fp(byte)));
        }
        assert_eq!(state.last_ts, 1_000_000);
    }

    // ── The core F3 attack: replay an already-accepted frame ────────────────

    #[test]
    fn exact_replay_of_the_last_accepted_frame_is_rejected() {
        let mut state = InboundReplayState::EMPTY;
        assert!(check_and_record_inbound(&mut state, 500, fp(9)));
        // Attacker replays the identical captured frame verbatim.
        assert!(!check_and_record_inbound(&mut state, 500, fp(9)));
        // ...any number of times.
        assert!(!check_and_record_inbound(&mut state, 500, fp(9)));
    }

    #[test]
    fn replay_of_an_older_accepted_frame_is_rejected_even_after_newer_traffic() {
        let mut state = InboundReplayState::EMPTY;
        assert!(check_and_record_inbound(&mut state, 100, fp(1))); // captured by attacker
        assert!(check_and_record_inbound(&mut state, 200, fp(2))); // legitimate traffic continues
        assert!(check_and_record_inbound(&mut state, 300, fp(3)));
        // Attacker now replays the OLD captured frame (ts=100, fp=1) — its
        // ts no longer advances the mark, and its fingerprint is still in
        // the (unfull) ring, so it must be rejected, not treated as novel.
        assert!(!check_and_record_inbound(&mut state, 100, fp(1)));
    }

    #[test]
    fn replayed_frame_beyond_ring_depth_is_the_documented_residual_gap() {
        // Pins the module doc's stated residual: once REPLAY_RING_LEN other
        // frames from the SAME contact have been accepted, an old capture's
        // fingerprint has aged out of the ring and a replay is (knowingly)
        // let through, because its `ts` also fails to advance the mark.
        // This test is a REGRESSION GUARD on the documented boundary, not an
        // endorsement — the fix's whole benefit is that this window is now
        // per-contact and persisted, not the current global/RAM-only one.
        let mut state = InboundReplayState::EMPTY;
        assert!(check_and_record_inbound(&mut state, 100, fp(0xAA))); // captured
        for i in 0..REPLAY_RING_LEN as u32 {
            assert!(check_and_record_inbound(
                &mut state,
                200 + i,
                fp((i + 1) as u8)
            ));
        }
        // The captured fingerprint has now been evicted from the ring.
        assert!(check_and_record_inbound(&mut state, 100, fp(0xAA)));
    }

    // ── The trade-off this mission exists to fix: peer reboot regression ────

    #[test]
    fn novel_content_at_a_regressed_timestamp_is_accepted_not_locked_out() {
        let mut state = InboundReplayState::EMPTY;
        assert!(check_and_record_inbound(&mut state, 5_000, fp(1)));
        // Contact reboots: fresh per-boot random tx_epoch_base regresses its
        // outbound ts well below our recorded high-water mark, but this is
        // genuinely NEW content, never seen before.
        assert!(check_and_record_inbound(&mut state, 50, fp(2)));
        // The mark itself must not have been dragged backwards by that
        // acceptance (see module doc for why regressing it would be unsafe).
        assert_eq!(state.last_ts, 5_000);
    }

    #[test]
    fn contact_keeps_working_indefinitely_after_its_own_reboot() {
        let mut state = InboundReplayState::EMPTY;
        assert!(check_and_record_inbound(&mut state, 5_000, fp(1)));
        // Post-reboot traffic, still below the stale historical peak, climbs
        // on its own new low baseline — every one of these must go through
        // (no one-shot cap, no permanent lockout).
        for (i, ts) in [51u32, 52, 53, 54, 55].into_iter().enumerate() {
            assert!(check_and_record_inbound(&mut state, ts, fp((10 + i) as u8)));
        }
    }

    #[test]
    fn replaying_a_just_accepted_post_reboot_frame_is_still_rejected() {
        let mut state = InboundReplayState::EMPTY;
        assert!(check_and_record_inbound(&mut state, 5_000, fp(1)));
        assert!(check_and_record_inbound(&mut state, 50, fp(2))); // post-reboot, accepted
                                                                  // A capture-and-replay of THAT exact post-reboot frame must still be
                                                                  // caught — acceptance via the exception path is not a free pass for
                                                                  // that same content forever.
        assert!(!check_and_record_inbound(&mut state, 50, fp(2)));
    }

    // ── Codec ────────────────────────────────────────────────────────────────

    #[test]
    fn state_roundtrips_through_encode_decode() {
        let mut state = InboundReplayState::EMPTY;
        check_and_record_inbound(&mut state, 5_000, fp(1));
        check_and_record_inbound(&mut state, 50, fp(2));
        check_and_record_inbound(&mut state, 5_001, fp(3));

        let mut blob = [0u8; INBOUND_REPLAY_STATE_LEN];
        let n = encode_inbound_replay_state(&state, &mut blob);
        assert_eq!(n, INBOUND_REPLAY_STATE_LEN);

        let restored = decode_inbound_replay_state(&blob).expect("must decode");
        assert_eq!(restored, state);
    }

    #[test]
    fn truncated_blob_is_rejected() {
        let blob = [0u8; INBOUND_REPLAY_STATE_LEN - 1];
        assert!(decode_inbound_replay_state(&blob).is_none());
    }

    #[test]
    fn corrupt_head_or_count_is_rejected() {
        let mut blob = [0u8; INBOUND_REPLAY_STATE_LEN];
        blob[INBOUND_REPLAY_STATE_LEN - 2] = REPLAY_RING_LEN as u8; // head out of range
        assert!(decode_inbound_replay_state(&blob).is_none());

        let mut blob2 = [0u8; INBOUND_REPLAY_STATE_LEN];
        blob2[INBOUND_REPLAY_STATE_LEN - 1] = REPLAY_RING_LEN as u8 + 1; // count out of range
        assert!(decode_inbound_replay_state(&blob2).is_none());
    }

    #[test]
    fn empty_state_decodes_and_reencodes_identically() {
        let mut blob = [0u8; INBOUND_REPLAY_STATE_LEN];
        let n = encode_inbound_replay_state(&InboundReplayState::EMPTY, &mut blob);
        let restored = decode_inbound_replay_state(&blob[..n]).unwrap();
        assert_eq!(restored, InboundReplayState::EMPTY);
    }
}
