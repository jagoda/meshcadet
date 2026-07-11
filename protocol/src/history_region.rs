// SPDX-License-Identifier: GPL-3.0-only
//! Per-conversation flash history region — self-describing header + append/
//! compaction ring codec, and a conversation directory helper.
//!
//! This module is `no_std`-safe and host-testable, exactly like the sibling
//! [`crate::history`] module it builds on. **No flash syscalls live here** —
//! this is a pure codec over byte buffers. The firmware store (a later
//! phase) drives these primitives against real `esp_partition`
//! reads/writes/erases; the [`HistoryRegion`] type additionally models the
//! full two-sector lifecycle in memory so the codec can be exhaustively
//! host-tested without real flash.
//!
//! # Region layout (2 flash sectors = one double-buffered region)
//!
//! Each sector starts with an 8-byte self-describing header, followed by a
//! sequential append area of 72-byte slot records (the existing
//! [`crate::history::encode_entry_blob`] / [`crate::history::decode_entry_blob`]
//! layout — see [`encode_slot`] / [`decode_slot`]).
//!
//! ```text
//! sector:  [ header(8B) | slot 0 (72B) | slot 1 (72B) | ... | slot N-1 (72B) | pad ]
//! header:  magic(2) | version(1) | kind(1) | conv_hash(1) | generation(1) | flags(1) | rsvd(1)
//! ```
//!
//! - **`kind`** reuses [`crate::history::HistoryMsgType`] (`Dm` | `GrpTxt`) —
//!   every entry in a region shares the conversation's kind.
//! - **`conv_hash`** is the 1-byte truncated hash identifying the
//!   conversation (contact pubkey hash or channel hash), matching the
//!   existing 1-byte hash convention used elsewhere in this crate.
//! - **`generation`** disambiguates which of the two sectors is
//!   authoritative (see "Compaction" below); compared with wraparound-aware
//!   arithmetic.
//! - **`flags`/`rsvd`** are reserved (always `0` today).
//!
//! # Append
//!
//! The write head is the first slot in the active sector whose 72 bytes are
//! all `0xFF` (erased). Appends are strictly sequential; a slot, once
//! written, is never rewritten in place (flash cannot un-write bits without
//! an erase).
//!
//! # Compaction (power-loss-safe hand-off)
//!
//! When the active sector's append area is full, the newest
//! [`crate::history::MAX_HISTORY_ENTRIES`] (32) entries are copied into the
//! spare sector, **entries first, header last**. Writing the header last
//! makes the hand-off atomic from the perspective of the next boot:
//!
//! - If power is lost **during the entry copy**, the spare sector's header
//!   is still erased/invalid → [`HistoryRegion::active_sector_index`] keeps
//!   resolving to the *old* sector (still fully valid, lower generation) —
//!   no data lost, compaction simply restarts.
//! - If power is lost **after the header commit**, the spare sector (higher
//!   generation, valid header) is authoritative — the old sector is stale.
//!   Erasing it does not need to happen atomically with the hand-off; it
//!   only needs to happen before that sector is reused as a spare, and a
//!   stale-but-unerased sector is handled defensively at that point too.
//!
//! A torn *append* (not compaction) can leave at most one slot half-written
//! (some bytes correctly written, the rest still `0xFF`, since program
//! operations proceed from an erased state and cannot be interrupted
//! mid-byte). [`find_write_head`] treats such a slot as occupied (not
//! all-`0xFF`), so the write head advances past it rather than colliding
//! with it — the documented "≤1 stale slot" cost, tolerated rather than
//! detected (there is no spare byte left for a checksum: the only former
//! "reserved" byte is now the flags byte, carrying both `is_ours`
//! ([`FLAG_IS_OURS`]) and ack/delivery status ([`FLAG_ACKED`])).
//!
//! # Live view: newest ≤32, oldest-first
//!
//! Regardless of how many slots physically remain in the active sector
//! before the next compaction (up to `SLOTS_PER_SECTOR`, ~56), the *logical*
//! live view exposed by [`HistoryRegion::iter_oldest_first`] is always the
//! newest `min(appended_so_far, MAX_HISTORY_ENTRIES)` entries — matching
//! what compaction itself preserves.

use crate::history::{
    decode_entry_blob, encode_entry_blob, HistoryEntry, HistoryMsgType, HISTORY_ENTRY_BLOB_LEN,
    MAX_HISTORY_ENTRIES,
};

/// A decoded region slot: `(entry, is_ours, acked)`. Named alias purely to
/// keep signatures below under clippy's `type_complexity` threshold — see
/// `decode_slot` for what each element means.
type DecodedSlot = (HistoryEntry, bool, bool);

// ── Slot codec: reuse the legacy 72-byte blob, repurposing blob[71] ───────────

/// Bit 0 of the (formerly-reserved) last blob byte: message was sent by us.
///
/// Legacy blobs (written by [`crate::history::encode_entry_blob`] before this
/// module existed) always wrote `0x00` there, so `decode_slot` on a legacy
/// blob yields `is_ours == false` — byte-compatible with existing history.
pub const FLAG_IS_OURS: u8 = 0x01;

/// Bit 1 of the same flags byte: message ack/delivery status.
///
/// For an outbound (`is_ours == true`) entry, this is the actual delivery
/// state: `true` once a v1.15 ACK has matched the send, `false` while still
/// pending. For an inbound entry it is `true` unconditionally (an entry we
/// received is trivially "delivered" — there is no pending ACK to model for
/// it), matching what the UI already assumed via `MessageRecord::acked` for
/// non-`is_ours` records. Legacy blobs (reserved byte always `0x00`) decode
/// this bit as `false`, same byte-compatibility rule as `FLAG_IS_OURS` — but
/// that only affects legacy `is_ours == false` entries, whose `acked` bit the
/// UI never renders, so it is a silent, harmless default.
///
/// **On-flash migration note**: firmware built *between* this bit's
/// introduction and `FLAG_IS_OURS`'s (i.e. any build that called
/// `encode_slot`/wrote `history_region` slots before this bit existed) also only
/// ever wrote `0x00` or `FLAG_IS_OURS` to the flags byte — this bit was never
/// set. Booting this firmware over that data is exactly the legacy case
/// above: every pre-existing outbound entry decodes `acked = false` (shows
/// pending, "✓") rather than a stale/incorrect `true`. That is the safe
/// direction for the original bug (which was the inverse: hydrate
/// hardcoding a blanket `true`) — no explicit migration step is needed.
pub const FLAG_ACKED: u8 = 0x02;

/// Encode one region slot: the legacy 72-byte entry blob with the flags byte
/// (`blob[71]`) set from `is_ours` and `acked`.
pub fn encode_slot(
    entry: &HistoryEntry,
    is_ours: bool,
    acked: bool,
) -> [u8; HISTORY_ENTRY_BLOB_LEN] {
    let mut blob = [0u8; HISTORY_ENTRY_BLOB_LEN];
    encode_entry_blob(entry, &mut blob);
    let mut flags = 0u8;
    if is_ours {
        flags |= FLAG_IS_OURS;
    }
    if acked {
        flags |= FLAG_ACKED;
    }
    blob[HISTORY_ENTRY_BLOB_LEN - 1] = flags;
    blob
}

/// Decode one region slot. Returns `None` for an erased (`0xFF`) or
/// otherwise-unrecognised slot, exactly like [`crate::history::decode_entry_blob`].
///
/// Returns `(entry, is_ours, acked)`.
pub fn decode_slot(blob: &[u8; HISTORY_ENTRY_BLOB_LEN]) -> Option<DecodedSlot> {
    let entry = decode_entry_blob(blob)?;
    let flags = blob[HISTORY_ENTRY_BLOB_LEN - 1];
    let is_ours = (flags & FLAG_IS_OURS) != 0;
    let acked = (flags & FLAG_ACKED) != 0;
    Some((entry, is_ours, acked))
}

/// Index of the newest `is_ours && !acked` entry among `live` (oldest-first,
/// `None` gaps allowed for unfilled trailing slots — matches the shape both
/// [`HistoryRegion`]'s in-RAM model and the firmware `HistoryStore` gather
/// into before a compaction/rewrite), or `None` if nothing is pending.
///
/// Pulled out as a pure, host-testable function so "is the right entry
/// selected to flip on a live ack event" has direct test coverage —
/// `firmware::HistoryStore` (the only caller, via `mark_last_ours_acked`)
/// only cross-compiles for the esp target and cannot run `cargo test`
/// directly, so any selection logic living there would otherwise be
/// HIL-only-verified. Same "extract for testability" precedent as
/// `ui::mark_last_unacked_outbound`'s in-memory counterpart — newest-first
/// search, first unacked outbound hit wins, matching the "only one send is
/// ever outstanding at a time" single-pending-slot model both the DM and
/// channel ack trackers use.
pub fn find_newest_ours_unacked(live: &[Option<DecodedSlot>]) -> Option<usize> {
    (0..live.len())
        .rev()
        .find(|&i| matches!(live[i], Some((_, is_ours, acked)) if is_ours && !acked))
}

// ── Region geometry ────────────────────────────────────────────────────────────

/// Flash erase-sector size targeted by this layout (ESP32-S3 native sector).
pub const SECTOR_SIZE: usize = 4096;

/// Sectors per region (a double buffer for power-loss-safe compaction).
pub const SECTORS_PER_REGION: usize = 2;

/// Total bytes per conversation region (2 × 4 KB = 8 KB).
pub const REGION_SIZE: usize = SECTOR_SIZE * SECTORS_PER_REGION;

/// Region/sector header length: `magic(2) | version(1) | kind(1) | conv_hash(1)
/// | generation(1) | flags(1) | rsvd(1)`.
pub const REGION_HEADER_LEN: usize = 8;

/// Number of 72-byte slots that fit after the header in one sector.
pub const SLOTS_PER_SECTOR: usize = (SECTOR_SIZE - REGION_HEADER_LEN) / HISTORY_ENTRY_BLOB_LEN;

/// Maximum number of conversation regions scanned by the directory helper
/// (sizing target: 16 contacts + 8 channels = 24).
pub const MAX_CONVERSATION_REGIONS: usize = 24;

const REGION_MAGIC: [u8; 2] = *b"MH";
const REGION_VERSION: u8 = 1;

/// Byte offset of slot `index` within a sector.
pub const fn slot_offset(index: usize) -> usize {
    REGION_HEADER_LEN + index * HISTORY_ENTRY_BLOB_LEN
}

// ── Region header ──────────────────────────────────────────────────────────────

/// Decoded sector header identifying a conversation and its generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionHeader {
    /// Conversation kind (`Dm` or `GrpTxt`) — shared by every slot in the region.
    pub kind: HistoryMsgType,
    /// 1-byte truncated conversation hash (contact pubkey hash / channel hash).
    pub conv_hash: u8,
    /// Compaction generation counter (wraparound-aware comparison).
    pub generation: u8,
}

/// Encode a sector header.
pub fn encode_region_header(header: &RegionHeader) -> [u8; REGION_HEADER_LEN] {
    let mut out = [0u8; REGION_HEADER_LEN];
    out[0] = REGION_MAGIC[0];
    out[1] = REGION_MAGIC[1];
    out[2] = REGION_VERSION;
    out[3] = header.kind as u8;
    out[4] = header.conv_hash;
    out[5] = header.generation;
    out[6] = 0; // flags (reserved)
    out[7] = 0; // rsvd
    out
}

/// Decode a sector header. Returns `None` for an erased sector (all `0xFF`),
/// a bad magic/version, or an unrecognised `kind` — all treated as "not a
/// committed, claimed region".
pub fn decode_region_header(bytes: &[u8]) -> Option<RegionHeader> {
    if bytes.len() < REGION_HEADER_LEN {
        return None;
    }
    if bytes[0] != REGION_MAGIC[0] || bytes[1] != REGION_MAGIC[1] {
        return None;
    }
    if bytes[2] != REGION_VERSION {
        return None;
    }
    let kind = HistoryMsgType::from_u8(bytes[3])?;
    Some(RegionHeader {
        kind,
        conv_hash: bytes[4],
        generation: bytes[5],
    })
}

/// Wraparound-aware "is `candidate` newer than `current`" comparison for the
/// 1-byte generation counter (standard signed sequence-number comparison).
///
/// `pub`: the firmware store drives raw `esp_partition` reads directly (it
/// cannot materialize a whole [`HistoryRegion`] in RAM per conversation — see
/// the module docs) and needs this same generation-resolution rule to decide
/// which of a region's two on-flash sectors is authoritative.
pub fn generation_is_newer(candidate: u8, current: u8) -> bool {
    (candidate.wrapping_sub(current) as i8) > 0
}

/// Find the write head of a sector: the index of the first slot whose 72
/// bytes are all `0xFF` (erased), or `SLOTS_PER_SECTOR` if the sector is
/// full. A torn (half-written) slot is not all-`0xFF`, so it counts as
/// occupied — the write head advances past it.
pub fn find_write_head(sector: &[u8]) -> usize {
    for i in 0..SLOTS_PER_SECTOR {
        let off = slot_offset(i);
        let end = off + HISTORY_ENTRY_BLOB_LEN;
        if end > sector.len() {
            return i;
        }
        if sector[off..end].iter().all(|&b| b == 0xFF) {
            return i;
        }
    }
    SLOTS_PER_SECTOR
}

/// Errors from [`HistoryRegion`] operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionError {
    /// The region has no valid header (never claimed by a conversation).
    Unclaimed,
}

// ── HistoryRegion: in-memory double-buffer model ──────────────────────────────

/// A conversation's 2-sector double-buffered history region.
///
/// This owns two `SECTOR_SIZE` byte buffers directly, which is the shape
/// host tests need to exercise the full append/compaction/torn-write
/// lifecycle without real flash. Firmware may drive the free functions in
/// this module directly against `esp_partition` scratch buffers instead of
/// materializing a whole region in RAM.
pub struct HistoryRegion {
    sectors: [[u8; SECTOR_SIZE]; SECTORS_PER_REGION],
}

impl HistoryRegion {
    /// A fresh, unclaimed region (both sectors fully erased).
    pub fn new_erased() -> Self {
        Self {
            sectors: [[0xFFu8; SECTOR_SIZE]; SECTORS_PER_REGION],
        }
    }

    fn header_of(&self, sector_idx: usize) -> Option<RegionHeader> {
        decode_region_header(&self.sectors[sector_idx][..REGION_HEADER_LEN])
    }

    fn write_header(&mut self, sector_idx: usize, header: &RegionHeader) {
        let bytes = encode_region_header(header);
        self.sectors[sector_idx][..REGION_HEADER_LEN].copy_from_slice(&bytes);
    }

    fn write_head(&self, sector_idx: usize) -> usize {
        find_write_head(&self.sectors[sector_idx])
    }

    fn decode_slot_at(&self, sector_idx: usize, slot: usize) -> Option<DecodedSlot> {
        let off = slot_offset(slot);
        let bytes: &[u8; HISTORY_ENTRY_BLOB_LEN] = self.sectors[sector_idx]
            [off..off + HISTORY_ENTRY_BLOB_LEN]
            .try_into()
            .ok()?;
        decode_slot(bytes)
    }

    /// Which sector is currently authoritative: the sector with a valid
    /// header; if both are valid (old sector not yet erased after a prior
    /// compaction), the higher generation wins. `None` if neither sector has
    /// been claimed.
    pub fn active_sector_index(&self) -> Option<usize> {
        match (self.header_of(0), self.header_of(1)) {
            (Some(a), Some(b)) => {
                if generation_is_newer(b.generation, a.generation) {
                    Some(1)
                } else {
                    Some(0)
                }
            }
            (Some(_), None) => Some(0),
            (None, Some(_)) => Some(1),
            (None, None) => None,
        }
    }

    /// `true` if this region has been claimed by a conversation.
    pub fn is_claimed(&self) -> bool {
        self.active_sector_index().is_some()
    }

    /// The active sector's header, if claimed.
    pub fn header(&self) -> Option<RegionHeader> {
        self.active_sector_index().and_then(|i| self.header_of(i))
    }

    /// Claim this (unclaimed) region for a conversation: writes a
    /// generation-0 header to sector 0. Overwrites any existing header —
    /// callers (the directory helper) must only call this on a region for
    /// which [`Self::is_claimed`] is `false`; debug builds assert this.
    pub fn claim(&mut self, kind: HistoryMsgType, conv_hash: u8) {
        debug_assert!(
            !self.is_claimed(),
            "claim() called on an already-claimed region — data loss"
        );
        self.write_header(
            0,
            &RegionHeader {
                kind,
                conv_hash,
                generation: 0,
            },
        );
        // Ensure sector 1 reads as unclaimed spare (fresh regions already are,
        // but re-claiming a previously-used region must not leave stale data
        // behind that could resolve as "active" via a higher generation).
        self.sectors[1] = [0xFFu8; SECTOR_SIZE];
    }

    /// Gather the live newest-`MAX_HISTORY_ENTRIES` entries from `sector_idx`,
    /// oldest-first, skipping any slot that fails to decode (the tolerated
    /// torn-write cost). Returns the entries and how many were found.
    fn gather_live(
        &self,
        sector_idx: usize,
    ) -> ([Option<DecodedSlot>; MAX_HISTORY_ENTRIES], usize) {
        let head = self.write_head(sector_idx);
        let start = head.saturating_sub(MAX_HISTORY_ENTRIES);
        let mut buf: [Option<DecodedSlot>; MAX_HISTORY_ENTRIES] = [None; MAX_HISTORY_ENTRIES];
        let mut n = 0;
        for slot in start..head {
            if let Some(triple) = self.decode_slot_at(sector_idx, slot) {
                buf[n] = Some(triple);
                n += 1;
            }
        }
        (buf, n)
    }

    /// Compact `active`'s newest-32 into the spare sector, committing the
    /// spare's header **last** (see module docs: entries-first-header-last
    /// is what makes a torn compaction recoverable by generation).
    fn compact(&mut self, active: usize) -> Result<(), RegionError> {
        let header = self.header_of(active).ok_or(RegionError::Unclaimed)?;
        let spare = 1 - active;
        let (live, n) = self.gather_live(active);

        // The spare must start erased. This in-memory model erases the
        // superseded sector eagerly at the end of every compaction (below),
        // so in steady state the spare is already clean here. Defensively
        // erase it anyway: a region reconstructed from real flash contents
        // (the firmware store's job, not this codec's) may observe a spare
        // that a prior boot committed to but never got around to erasing —
        // this keeps compaction correct even then.
        if self.header_of(spare).is_some() {
            self.sectors[spare] = [0xFFu8; SECTOR_SIZE];
        }

        for (i, slot) in live.iter().take(n).enumerate() {
            if let Some((entry, is_ours, acked)) = slot {
                let off = slot_offset(i);
                let bytes = encode_slot(entry, *is_ours, *acked);
                self.sectors[spare][off..off + HISTORY_ENTRY_BLOB_LEN].copy_from_slice(&bytes);
            }
        }

        let new_header = RegionHeader {
            kind: header.kind,
            conv_hash: header.conv_hash,
            generation: header.generation.wrapping_add(1),
        };
        self.write_header(spare, &new_header);

        // The old sector is now stale (superseded generation) and could be
        // erased any time before it's needed as the next spare; this model
        // erases it immediately (the simplest choice that satisfies that
        // constraint). A real flash-backed store may legitimately defer this
        // specific erase call — correctness does not depend on when it
        // happens, only that it happens before the sector is reused, which
        // the defensive check above covers either way.
        self.sectors[active] = [0xFFu8; SECTOR_SIZE];

        Ok(())
    }

    /// Append one entry, compacting first if the active sector's append
    /// area is full. Fails with [`RegionError::Unclaimed`] if the region has
    /// never been claimed.
    pub fn append(
        &mut self,
        entry: &HistoryEntry,
        is_ours: bool,
        acked: bool,
    ) -> Result<(), RegionError> {
        let mut active = self.active_sector_index().ok_or(RegionError::Unclaimed)?;
        let mut head = self.write_head(active);
        if head >= SLOTS_PER_SECTOR {
            self.compact(active)?;
            active = self
                .active_sector_index()
                .expect("just committed a header above");
            head = self.write_head(active);
        }
        let off = slot_offset(head);
        let bytes = encode_slot(entry, is_ours, acked);
        self.sectors[active][off..off + HISTORY_ENTRY_BLOB_LEN].copy_from_slice(&bytes);
        Ok(())
    }

    /// Number of entries in the current live (newest-≤32) view.
    pub fn live_count(&self) -> usize {
        match self.active_sector_index() {
            Some(active) => self.gather_live(active).1,
            None => 0,
        }
    }

    /// Iterate the live (newest-≤32) entries oldest-first, calling
    /// `f(index, entry, is_ours, acked)` for each. `index` is 0-based (oldest = 0).
    pub fn iter_oldest_first<F: FnMut(u8, &HistoryEntry, bool, bool)>(&self, mut f: F) {
        let Some(active) = self.active_sector_index() else {
            return;
        };
        let (live, n) = self.gather_live(active);
        for (i, slot) in live.iter().take(n).enumerate() {
            if let Some((entry, is_ours, acked)) = slot {
                f(i as u8, entry, *is_ours, *acked);
            }
        }
    }
}

impl Default for HistoryRegion {
    fn default() -> Self {
        Self::new_erased()
    }
}

// ── Conversation directory ─────────────────────────────────────────────────────

/// Locate the region owned by `(kind, conv_hash)` via a linear header scan,
/// or claim the first unclaimed region for it if none matches. Returns the
/// region index, or `None` if every region is claimed and none matches (the
/// directory is full).
pub fn find_or_claim_region(
    regions: &mut [HistoryRegion],
    kind: HistoryMsgType,
    conv_hash: u8,
) -> Option<usize> {
    for (i, region) in regions.iter().enumerate() {
        if let Some(header) = region.header() {
            if header.kind == kind && header.conv_hash == conv_hash {
                return Some(i);
            }
        }
    }
    for (i, region) in regions.iter_mut().enumerate() {
        if !region.is_claimed() {
            region.claim(kind, conv_hash);
            return Some(i);
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::HistoryMsgType as Kind;

    fn entry(ts: u32, text: &[u8]) -> HistoryEntry {
        let text_len = text.len().min(crate::history::MAX_HISTORY_TEXT_LEN) as u8;
        let mut buf = [0u8; crate::history::MAX_HISTORY_TEXT_LEN];
        buf[..text_len as usize].copy_from_slice(&text[..text_len as usize]);
        HistoryEntry {
            sender_hash: 0x01,
            msg_type: Kind::Dm,
            timestamp: ts,
            text: buf,
            text_len,
        }
    }

    // ── Slot codec (is_ours flag) ──────────────────────────────────────────

    #[test]
    fn slot_is_ours_round_trips_true_and_false() {
        let e = entry(42, b"hi");
        let blob_ours = encode_slot(&e, true, false);
        let (decoded, is_ours, _acked) = decode_slot(&blob_ours).unwrap();
        assert!(is_ours);
        assert_eq!(decoded.timestamp, 42);

        let blob_theirs = encode_slot(&e, false, false);
        let (_, is_ours2, _acked2) = decode_slot(&blob_theirs).unwrap();
        assert!(!is_ours2);
    }

    #[test]
    fn slot_byte_compatible_with_legacy_blob() {
        // A legacy blob (encode_entry_blob directly, as the old NVS store
        // does) always writes 0x00 to blob[71].
        let e = entry(7, b"legacy");
        let mut legacy_blob = [0u8; HISTORY_ENTRY_BLOB_LEN];
        encode_entry_blob(&e, &mut legacy_blob);
        assert_eq!(legacy_blob[HISTORY_ENTRY_BLOB_LEN - 1], 0x00);

        let (decoded, is_ours, acked) =
            decode_slot(&legacy_blob).expect("legacy blob still decodes");
        assert!(!is_ours, "legacy flags=0 must decode as is_ours=false");
        assert!(!acked, "legacy flags=0 must decode as acked=false");
        assert_eq!(decoded.timestamp, 7);

        // encode_slot(is_ours=false, acked=false) must be byte-identical to
        // the legacy blob.
        let our_blob = encode_slot(&e, false, false);
        assert_eq!(our_blob, legacy_blob);
    }

    // ── Slot codec (acked flag) ──────────────────────────────────────────────

    /// Acceptance: ack/delivery status round-trips through the slot codec.
    #[test]
    fn slot_acked_round_trips_true_and_false() {
        let e = entry(1, b"hi");

        let blob_acked = encode_slot(&e, true, true);
        let (_, is_ours, acked) = decode_slot(&blob_acked).unwrap();
        assert!(is_ours);
        assert!(acked);

        let blob_pending = encode_slot(&e, true, false);
        let (_, is_ours2, acked2) = decode_slot(&blob_pending).unwrap();
        assert!(is_ours2);
        assert!(!acked2);
    }

    /// Acceptance: `is_ours` and `acked` are independent bits — every
    /// combination round-trips without cross-contamination.
    #[test]
    fn slot_is_ours_and_acked_are_independent_bits() {
        let e = entry(2, b"hi");
        for is_ours in [false, true] {
            for acked in [false, true] {
                let blob = encode_slot(&e, is_ours, acked);
                let (_, got_ours, got_acked) = decode_slot(&blob).unwrap();
                assert_eq!(got_ours, is_ours, "is_ours must not depend on acked");
                assert_eq!(got_acked, acked, "acked must not depend on is_ours");
            }
        }
    }

    // ── find_newest_ours_unacked ──
    //
    // Regression guard for "the right entry gets flipped": `HistoryStore::
    // mark_last_ours_acked` (firmware, HIL-only-runnable) delegates its
    // selection logic to this pure function so it has direct, host-runnable
    // test coverage.

    #[test]
    fn find_newest_ours_unacked_picks_the_newest_pending_outbound() {
        let e = entry(1, b"hi");
        let live: [Option<DecodedSlot>; 4] = [
            Some((e, true, false)), // index 0: outbound, pending (older)
            Some((e, false, true)), // index 1: inbound
            Some((e, true, false)), // index 2: outbound, pending (newest pending)
            Some((e, true, true)),  // index 3: outbound, already acked
        ];
        assert_eq!(
            find_newest_ours_unacked(&live),
            Some(2),
            "must pick the NEWEST unacked outbound entry, not the oldest"
        );
    }

    #[test]
    fn find_newest_ours_unacked_ignores_inbound_and_already_acked() {
        let e = entry(2, b"hi");
        let live: [Option<DecodedSlot>; 3] = [
            Some((e, false, false)), // inbound, "unacked" bit is inert for inbound
            Some((e, true, true)),   // outbound, already acked
            Some((e, false, true)),  // inbound
        ];
        assert_eq!(
            find_newest_ours_unacked(&live),
            None,
            "no pending outbound entry present"
        );
    }

    #[test]
    fn find_newest_ours_unacked_skips_none_gaps() {
        let e = entry(3, b"hi");
        let live: [Option<DecodedSlot>; 3] = [Some((e, true, false)), None, None];
        assert_eq!(
            find_newest_ours_unacked(&live),
            Some(0),
            "must find the entry past trailing gaps"
        );
    }

    #[test]
    fn find_newest_ours_unacked_empty_slice_is_none() {
        let live: [Option<DecodedSlot>; 0] = [];
        assert_eq!(find_newest_ours_unacked(&live), None);
    }

    // ── Region header ───────────────────────────────────────────────────────

    #[test]
    fn region_header_round_trips() {
        let h = RegionHeader {
            kind: Kind::GrpTxt,
            conv_hash: 0xAB,
            generation: 3,
        };
        let bytes = encode_region_header(&h);
        let decoded = decode_region_header(&bytes).expect("decode");
        assert_eq!(decoded, h);
    }

    #[test]
    fn region_header_rejects_erased_bytes() {
        let erased = [0xFFu8; REGION_HEADER_LEN];
        assert!(decode_region_header(&erased).is_none());
    }

    #[test]
    fn region_header_rejects_bad_magic_and_version() {
        let mut bytes = encode_region_header(&RegionHeader {
            kind: Kind::Dm,
            conv_hash: 1,
            generation: 0,
        });
        bytes[0] = 0x00;
        assert!(decode_region_header(&bytes).is_none());

        let mut bytes2 = encode_region_header(&RegionHeader {
            kind: Kind::Dm,
            conv_hash: 1,
            generation: 0,
        });
        bytes2[2] = 0xFF; // bad version
        assert!(decode_region_header(&bytes2).is_none());
    }

    #[test]
    fn generation_comparison_handles_wraparound() {
        assert!(generation_is_newer(1, 0));
        assert!(!generation_is_newer(0, 1));
        assert!(
            generation_is_newer(0, 255),
            "255 -> 0 must be treated as newer (wraparound)"
        );
        assert!(!generation_is_newer(255, 0));
    }

    // ── HistoryRegion lifecycle ─────────────────────────────────────────────

    #[test]
    fn fresh_region_is_unclaimed() {
        let r = HistoryRegion::new_erased();
        assert!(!r.is_claimed());
        assert!(r.header().is_none());
        assert!(r.active_sector_index().is_none());
        assert_eq!(r.live_count(), 0);
    }

    #[test]
    fn append_before_claim_fails() {
        let mut r = HistoryRegion::new_erased();
        let e = entry(1, b"x");
        assert_eq!(r.append(&e, false, false), Err(RegionError::Unclaimed));
    }

    #[test]
    fn claim_then_append_and_iterate() {
        let mut r = HistoryRegion::new_erased();
        r.claim(Kind::Dm, 0x42);
        assert_eq!(
            r.header(),
            Some(RegionHeader {
                kind: Kind::Dm,
                conv_hash: 0x42,
                generation: 0
            })
        );

        r.append(&entry(100, b"first"), false, false).unwrap();
        r.append(&entry(200, b"second"), true, true).unwrap();

        let mut got = Vec::new();
        r.iter_oldest_first(|idx, e, is_ours, acked| got.push((idx, e.timestamp, is_ours, acked)));
        assert_eq!(got, vec![(0, 100, false, false), (1, 200, true, true)]);
        assert_eq!(r.live_count(), 2);
    }

    /// Acceptance: append + wrap — filling a sector triggers compaction and
    /// the live view always exposes exactly the newest MAX_HISTORY_ENTRIES.
    #[test]
    fn append_fill_and_wrap_keeps_newest_32() {
        let mut r = HistoryRegion::new_erased();
        r.claim(Kind::Dm, 0x01);

        let total = SLOTS_PER_SECTOR + 1; // forces exactly one compaction
        for i in 0..total {
            r.append(&entry(i as u32, b"x"), false, false).unwrap();
        }

        assert_eq!(r.live_count(), MAX_HISTORY_ENTRIES);
        let mut ts_seq = Vec::new();
        r.iter_oldest_first(|_, e, _, _| ts_seq.push(e.timestamp));
        let expected_oldest = (total - MAX_HISTORY_ENTRIES) as u32;
        assert_eq!(ts_seq[0], expected_oldest);
        assert_eq!(*ts_seq.last().unwrap(), (total - 1) as u32);
        assert_eq!(ts_seq.len(), MAX_HISTORY_ENTRIES);

        // Compaction must have bumped the generation.
        assert_eq!(r.header().unwrap().generation, 1);
    }

    /// Acceptance: compaction preserves each surviving entry's exact
    /// `is_ours`/`acked` pair — the ack/delivery bit must not be dropped,
    /// defaulted, or cross-wired with `is_ours` across a compaction's
    /// entries-first-header-last copy.
    #[test]
    fn compaction_preserves_is_ours_and_acked_bits() {
        let mut r = HistoryRegion::new_erased();
        r.claim(Kind::Dm, 0x02);

        let total = SLOTS_PER_SECTOR + 1; // forces exactly one compaction
        for i in 0..total {
            // Alternate all four (is_ours, acked) combinations across appends
            // so the compacted newest-32 window contains every combination.
            let is_ours = i % 2 == 0;
            let acked = i % 4 < 2;
            r.append(&entry(i as u32, b"x"), is_ours, acked).unwrap();
        }
        assert_eq!(r.header().unwrap().generation, 1, "must have compacted");
        assert_eq!(r.live_count(), MAX_HISTORY_ENTRIES);

        r.iter_oldest_first(|_, e, is_ours, acked| {
            let i = e.timestamp as usize;
            assert_eq!(
                is_ours,
                i.is_multiple_of(2),
                "is_ours must survive compaction for ts={i}"
            );
            assert_eq!(acked, i % 4 < 2, "acked must survive compaction for ts={i}");
        });
    }

    /// Acceptance: many compactions still preserve the newest-32 invariant.
    #[test]
    fn multiple_compactions_preserve_newest_32() {
        let mut r = HistoryRegion::new_erased();
        r.claim(Kind::GrpTxt, 0x07);

        let total = SLOTS_PER_SECTOR + 24 * 2 + 5;
        for i in 0..total {
            r.append(&entry(i as u32, b"x"), false, false).unwrap();
        }

        assert_eq!(r.live_count(), MAX_HISTORY_ENTRIES);
        let mut ts_seq = Vec::new();
        r.iter_oldest_first(|_, e, _, _| ts_seq.push(e.timestamp));
        assert_eq!(ts_seq[0], (total - MAX_HISTORY_ENTRIES) as u32);
        assert_eq!(*ts_seq.last().unwrap(), (total - 1) as u32);
        assert!(
            r.header().unwrap().generation >= 2,
            "expect at least 2 compactions over {total} appends"
        );
    }

    /// Acceptance: torn slot write — a half-written slot doesn't jam the
    /// write head or corrupt neighboring slots. Also covers the
    /// ack/delivery bit:
    /// the surviving (untorn) slots' `is_ours`/`acked` pair must come through
    /// exactly as written, regardless of the torn slot sitting between them.
    #[test]
    fn torn_slot_write_leaves_at_most_one_stale_slot() {
        let mut r = HistoryRegion::new_erased();
        r.claim(Kind::Dm, 0x09);
        r.append(&entry(1, b"a"), false, true).unwrap(); // inbound, delivered
        r.append(&entry(2, b"b"), true, false).unwrap(); // outbound, pending

        // Simulate a torn write of slot index 2: only the first 4 bytes of
        // the 72-byte record actually landed before power was lost; the
        // rest — including the flags byte carrying is_ours/acked — is still
        // erased (0xFF), as a NOR-flash program op leaves it.
        let active = r.active_sector_index().unwrap();
        let full = encode_slot(&entry(3, b"c"), true, true);
        let off = slot_offset(2);
        r.sectors[active][off..off + 4].copy_from_slice(&full[..4]);
        // bytes off+4.. remain 0xFF from new_erased()

        // The write head must have advanced past the torn slot (not all
        // 0xFF), so the next append lands in slot 3, not slot 2.
        assert_eq!(r.write_head(active), 3);

        r.append(&entry(4, b"d"), true, true).unwrap(); // outbound, acked

        // The two clean entries are intact; the torn slot is tolerated
        // (skipped if it fails to decode, which msg_type-byte corruption
        // typically causes here since byte[1] was never written).
        let mut got: Vec<(u32, bool, bool)> = Vec::new();
        r.iter_oldest_first(|_, e, is_ours, acked| got.push((e.timestamp, is_ours, acked)));
        assert!(got.contains(&(1, false, true)));
        assert!(got.contains(&(2, true, false)));
        assert!(got.contains(&(4, true, true)));
        assert!(got.len() <= 4, "at most one stale slot tolerated");
    }

    /// Acceptance: interrupted compaction — if the spare sector's header
    /// commit never lands, the old sector (still valid, lower generation)
    /// remains authoritative and no data is lost.
    #[test]
    fn interrupted_compaction_recovers_via_generation() {
        let mut r = HistoryRegion::new_erased();
        r.claim(Kind::Dm, 0x0A);
        for i in 0..SLOTS_PER_SECTOR {
            r.append(&entry(i as u32, b"x"), false, false).unwrap();
        }
        let old_active = r.active_sector_index().unwrap();
        let old_header = r.header().unwrap();

        // Manually replay compaction's entry-copy step but crash before the
        // header commit (simulating power loss mid-compaction).
        let (live, n) = r.gather_live(old_active);
        let spare = 1 - old_active;
        for (i, slot) in live.iter().take(n).enumerate() {
            let (e, is_ours, acked) = slot.unwrap();
            let off = slot_offset(i);
            let bytes = encode_slot(&e, is_ours, acked);
            r.sectors[spare][off..off + HISTORY_ENTRY_BLOB_LEN].copy_from_slice(&bytes);
        }
        // Header commit deliberately skipped — spare's header bytes remain 0xFF.

        // Recovery: active sector must still be the old one, generation unchanged.
        assert_eq!(r.active_sector_index(), Some(old_active));
        assert_eq!(r.header(), Some(old_header));
        assert_eq!(r.live_count(), MAX_HISTORY_ENTRIES);

        // A subsequent append must still work normally against the old sector
        // (it is not full — wait, it is full; this exercises real compaction
        // running again, cleanly, from the (still valid) old sector).
        r.append(&entry(999, b"resumed"), false, false).unwrap();
        assert_eq!(
            r.header().unwrap().generation,
            old_header.generation.wrapping_add(1)
        );
    }

    /// Acceptance: interrupted compaction where the header commit itself
    /// *did* land (power lost only after) — the new sector must win.
    #[test]
    fn completed_compaction_header_commit_wins() {
        let mut r = HistoryRegion::new_erased();
        r.claim(Kind::Dm, 0x0B);
        for i in 0..=SLOTS_PER_SECTOR {
            r.append(&entry(i as u32, b"x"), false, false).unwrap();
        }
        // append() already ran compact() to completion above.
        assert_eq!(r.header().unwrap().generation, 1);
        assert_eq!(r.live_count(), MAX_HISTORY_ENTRIES);
    }

    /// Acceptance: compaction defensively erases a spare that still carries
    /// a stale header — the scenario a real flash-backed store hits when a
    /// region reconstructed from partition contents observes a previously
    /// superseded sector that was committed over but never got erased.
    #[test]
    fn compact_erases_stale_spare_defensively() {
        let mut r = HistoryRegion::new_erased();
        r.claim(Kind::Dm, 0x0C);
        for i in 0..=SLOTS_PER_SECTOR {
            r.append(&entry(i as u32, b"x"), false, false).unwrap();
        }
        // First compaction ran: generation 1 is active, sector 0 is erased
        // (this model's own eager erase). Simulate a store that hasn't
        // erased it yet by planting a stale (lower-generation) header back
        // onto sector 0 directly, bypassing the normal claim/append flow.
        let active = r.active_sector_index().unwrap();
        assert_eq!(r.header().unwrap().generation, 1);
        let stale_spare = 1 - active;
        r.write_header(
            stale_spare,
            &RegionHeader {
                kind: Kind::Dm,
                conv_hash: 0x0C,
                generation: 0,
            },
        );
        assert!(r.header_of(stale_spare).is_some(), "stale header planted");

        // Fill the active sector again to force a second compaction; it
        // must erase the stale spare before writing into it rather than
        // corrupting/merging with the leftover bytes.
        for i in 0..24 {
            r.append(&entry(1000 + i as u32, b"y"), false, false)
                .unwrap();
        }

        assert_eq!(
            r.header().unwrap().generation,
            2,
            "second compaction must have run"
        );
        assert_eq!(r.live_count(), MAX_HISTORY_ENTRIES);
        let mut ts_seq = Vec::new();
        r.iter_oldest_first(|_, e, _, _| ts_seq.push(e.timestamp));
        // Newest 32 of the 57 + 24 = 81 total appends: ts 1000+24-32..1000+24 tail,
        // i.e. the window is entirely within the second batch (24 < 32 though,
        // so it also reaches back into the first batch's tail).
        assert_eq!(ts_seq.len(), MAX_HISTORY_ENTRIES);
        assert_eq!(*ts_seq.last().unwrap(), 1000 + 23);
    }

    // ── Directory ─────────────────────────────────────────────────────────

    #[test]
    fn directory_finds_existing_and_claims_free() {
        let mut regions: Vec<HistoryRegion> = (0..4).map(|_| HistoryRegion::new_erased()).collect();

        let idx_a =
            find_or_claim_region(&mut regions, Kind::Dm, 0x11).expect("claims a free region");
        let idx_b = find_or_claim_region(&mut regions, Kind::GrpTxt, 0x22)
            .expect("claims a different region");
        assert_ne!(idx_a, idx_b);

        // Re-querying the same (kind, conv_hash) returns the same region,
        // without disturbing its contents.
        regions[idx_a]
            .append(&entry(5, b"hi"), false, false)
            .unwrap();
        let idx_a_again =
            find_or_claim_region(&mut regions, Kind::Dm, 0x11).expect("finds existing");
        assert_eq!(idx_a_again, idx_a);
        assert_eq!(regions[idx_a].live_count(), 1);
    }

    #[test]
    fn directory_full_returns_none() {
        let mut regions: Vec<HistoryRegion> = (0..2).map(|_| HistoryRegion::new_erased()).collect();
        find_or_claim_region(&mut regions, Kind::Dm, 1).unwrap();
        find_or_claim_region(&mut regions, Kind::Dm, 2).unwrap();
        // Both regions claimed by different conversations; a third doesn't fit.
        assert_eq!(find_or_claim_region(&mut regions, Kind::Dm, 3), None);
    }

    // ── Export aggregation (export-parity contract) ──────

    #[test]
    fn export_groups_by_conversation_oldest_first_globally_reindexed() {
        // Mirrors `firmware::history_store::HistoryStore::export_entries`'s
        // contract: entries are aggregated *grouped by conversation* (a
        // region's whole run before the next), oldest-first *within* each
        // region, under one running index across the whole export — NOT a
        // global timestamp-sorted merge (rejected: fragile when the RTC is
        // unsynced/zero). Conversation A claims its
        // region first but owns the *later* timestamps, and conversation B
        // claims second but owns the *earlier* timestamps, so a timestamp-merge
        // implementation would produce different output than this contract.
        let mut regions: Vec<HistoryRegion> = (0..4).map(|_| HistoryRegion::new_erased()).collect();

        let idx_a =
            find_or_claim_region(&mut regions, Kind::Dm, 0xA1).expect("claims region for A");
        regions[idx_a]
            .append(&entry(500, b"a-oldest"), false, false)
            .unwrap();
        regions[idx_a]
            .append(&entry(600, b"a-newest"), false, false)
            .unwrap();

        let idx_b =
            find_or_claim_region(&mut regions, Kind::GrpTxt, 0xB2).expect("claims region for B");
        regions[idx_b]
            .append(&entry(100, b"b-oldest"), false, false)
            .unwrap();
        regions[idx_b]
            .append(&entry(200, b"b-newest"), false, false)
            .unwrap();

        // Simulate the firmware store's `export_entries` traversal: visit
        // regions in directory (claim) order, oldest-first within each,
        // one running index across the whole stream.
        let mut got: Vec<(u8, u32)> = Vec::new();
        let mut idx: u8 = 0;
        for region in &regions {
            region.iter_oldest_first(|_, e, _, _| {
                got.push((idx, e.timestamp));
                idx += 1;
            });
        }

        // Grouped by conversation: all of A's entries (claimed first) precede
        // all of B's, even though B's timestamps are numerically smaller. A
        // strict timestamp-merge would instead yield 100, 200, 500, 600.
        assert_eq!(
            got,
            vec![(0, 500), (1, 600), (2, 100), (3, 200)],
            "export must group by conversation (region/claim order), not timestamp-merge"
        );
    }

    #[test]
    fn directory_survives_contact_index_shifts() {
        // A region is owned by (kind, conv_hash), not a table slot — so even
        // if the caller's contact list re-orders, the same hash still finds
        // the same region regardless of which directory slot it occupies.
        let mut regions: Vec<HistoryRegion> = (0..3).map(|_| HistoryRegion::new_erased()).collect();
        let idx = find_or_claim_region(&mut regions, Kind::Dm, 0x99).unwrap();
        regions[idx].append(&entry(1, b"x"), false, false).unwrap();

        // Simulate scanning in a different order by shuffling the slice.
        regions.swap(0, 2);
        let found = regions.iter().position(|r| {
            r.header()
                .map(|h| h.kind == Kind::Dm && h.conv_hash == 0x99)
                .unwrap_or(false)
        });
        assert!(found.is_some());
        assert_eq!(regions[found.unwrap()].live_count(), 1);
    }
}
