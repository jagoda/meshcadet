// SPDX-License-Identifier: GPL-3.0-only
//! Flash-backed **per-conversation** history store, driving the
//! `protocol::history_region` codec directly over the dedicated `mc_hist`
//! raw data partition — plus the one-shot legacy-NVS migration.
//!
//! # Why not NVS anymore
//!
//! The prior design (see git history) kept the ~32-entry ring in the shared
//! 24 KB `nvs` partition, one NVS key per slot. That partition also holds
//! `mc_id`/`mc_cfg`, and — more fundamentally — a single ring buffer shared
//! by every conversation meant a chatty channel could evict a quiet contact's
//! entire history. This store instead reserves one physically independent
//! **region** per conversation (contact or channel) on the dedicated
//! `mc_hist` partition (256 KB, declared in `firmware/partitions.csv`), so
//! each of up to [`MAX_CONVERSATION_REGIONS`] (24 = 16 contacts + 8 channels)
//! conversations keeps its own newest-32 independently.
//!
//! # Why raw `esp_partition`, not a whole-region RAM model
//!
//! [`protocol::history_region::HistoryRegion`] models a full 2-sector region
//! in RAM for exhaustive host testing, but its fields are private by design
//! and offer no "load arbitrary flash bytes into me" constructor — it is a
//! forward-simulation tool, not a flash-reconstruction one (see that module's
//! doc comment). This store instead drives the *free* codec functions
//! (`encode_slot`/`decode_slot`, `encode_region_header`/`decode_region_header`,
//! `slot_offset`, `generation_is_newer`) directly against
//! [`esp_idf_svc::partition::EspPartition`] reads/writes/erases, using small
//! fixed-size scratch buffers (one header = 8 B, one slot = 72 B) rather than
//! materializing any sector (4 KB) or region (8 KB) wholesale — 24 regions ×
//! 8 KB would be 192 KB of RAM, unacceptable on an ESP32-S3.
//!
//! # Region layout on `mc_hist`
//!
//! Region `i` (`0..MAX_CONVERSATION_REGIONS`) occupies
//! `[i * REGION_SIZE, (i + 1) * REGION_SIZE)` of the partition; each region is
//! `SECTORS_PER_REGION` (2) consecutive `SECTOR_SIZE` (4096-byte, the ESP32-S3
//! native erase-sector size) sectors — exactly the codec's double-buffer
//! layout. `MAX_CONVERSATION_REGIONS * REGION_SIZE` = 192 KB fits inside the
//! 256 KB partition with headroom to spare.
//!
//! # Flash-safety: always erase before first write into a sector
//!
//! Unlike the host-side `HistoryRegion` model (which starts from a
//! guaranteed-erased `[0xFF; _]` buffer), a raw partition's *actual* prior
//! content on first use is not guaranteed erased — flashing a new partition
//! table does not erase the region physically unless the chip was blank.
//! [`decode_region_header`] already treats non-matching bytes as "unclaimed"
//! (same as truly-erased `0xFF`), so a garbage-but-not-`"MH"`-tagged region
//! is correctly detected as unclaimed — but flash can only clear bits (not
//! set them) without an erase, so this store unconditionally erases a
//! sector immediately before the first write lands in it (region claim, and
//! every compaction's spare sector), rather than trusting header inspection
//! alone to imply "already erased". This costs at most one redundant erase
//! cycle in the case where the sector genuinely was already clean —
//! negligible against claim (once per conversation, ever) and compaction
//! (once per ~24-32 appends).
//!
//! # One-shot legacy-NVS migration
//!
//! At construction, before anything else, this store reads the legacy
//! per-slot NVS history (`mc_hist` NVS namespace: `head`/`cnt`/`s00`..`s31`,
//! written by the pre-flash design) exactly once, bucketing each decoded
//! entry into its owning conversation region by `(msg_type, sender_hash)` —
//! `sender_hash` already *is* the conversation hash for both message kinds
//! (contact pubkey hash for `Dm`, channel hash for `GrpTxt` — see
//! `main.rs::append_history`'s callers) — via [`Self::append`]'s own
//! `is_ours = false` path (matching legacy blobs, which never carried an
//! `is_ours` bit; `decode_slot` documents that a legacy blob's reserved byte
//! `0x00` decodes to `is_ours = false`, so this is exactly byte-faithful).
//! A `migrated` marker (1-byte NVS blob) is then written into that same
//! legacy namespace so the migration never re-runs; the legacy keys
//! themselves are left in place (dormant), not erased.

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use esp_idf_svc::partition::EspPartition;
pub use esp_idf_svc::sys::EspError;

use protocol::history::{
    HistoryEntry, HistoryMsgType, HISTORY_ENTRY_BLOB_LEN, MAX_HISTORY_ENTRIES,
    decode_entry_blob,
};
use protocol::history_region::{
    MAX_CONVERSATION_REGIONS, REGION_HEADER_LEN, REGION_SIZE, SECTOR_SIZE, SECTORS_PER_REGION,
    SLOTS_PER_SECTOR, RegionHeader,
    decode_region_header, decode_slot, encode_region_header, encode_slot, generation_is_newer,
    find_newest_ours_unacked, slot_offset,
};

/// Label of the dedicated raw data partition declared in `partitions.csv`.
const MC_HIST_PARTITION_LABEL: &str = "mc_hist";

// ── Legacy (pre-flash-store) NVS layout — read-only, migration source ────────

const LEGACY_NVS_NAMESPACE: &str = "mc_hist";
const LEGACY_KEY_HEAD: &str = "head";
const LEGACY_KEY_CNT: &str = "cnt";
/// One-shot migration done-marker, colocated in the legacy namespace itself
/// (no need to spend `mc_hist`-partition space on it, and it naturally lives
/// alongside the data it gates).
const LEGACY_KEY_MIGRATED: &str = "migrated";

/// Format a legacy per-slot NVS key as `sNN` into `buf`, mirroring the
/// pre-flash-store design exactly (same key scheme, so the same on-device
/// data written by prior firmware is readable here).
#[inline]
fn legacy_slot_key(slot: usize, buf: &mut [u8; 3]) -> &str {
    debug_assert!(slot < 100, "slot index out of range: {}", slot);
    buf[0] = b's';
    buf[1] = b'0' + (slot / 10) as u8;
    buf[2] = b'0' + (slot % 10) as u8;
    // SAFETY: buf contains only ASCII digits — valid UTF-8.
    core::str::from_utf8(buf).unwrap()
}

// ── HistoryStore ──────────────────────────────────────────────────────────────

/// Errors this store can return: either a raw flash/NVS `EspError`, or the
/// directory being full (every one of the [`MAX_CONVERSATION_REGIONS`]
/// regions is already claimed by a different conversation).
#[derive(Debug)]
pub enum HistoryStoreError {
    Esp(EspError),
    DirectoryFull,
}

impl From<EspError> for HistoryStoreError {
    fn from(e: EspError) -> Self {
        Self::Esp(e)
    }
}

impl std::fmt::Display for HistoryStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Esp(e) => write!(f, "flash/NVS error: {:?}", e),
            Self::DirectoryFull => write!(f, "conversation directory full (all {} regions claimed)", MAX_CONVERSATION_REGIONS),
        }
    }
}

impl std::error::Error for HistoryStoreError {}

/// Flash-backed per-conversation history store over the `mc_hist` partition.
pub struct HistoryStore {
    partition: EspPartition,
}

impl HistoryStore {
    /// Locate `mc_hist`, then perform the one-shot legacy-NVS migration if it
    /// hasn't already run. `nvs_partition` is the *legacy* NVS store on the
    /// default `nvs` partition — the migration's only reader, never touched
    /// again afterward.
    pub fn new(nvs_partition: EspNvsPartition<NvsDefault>) -> Result<Self, HistoryStoreError> {
        // SAFETY: exactly one `HistoryStore` (hence one `EspPartition` handle
        // for "mc_hist") is ever constructed, at boot, before the `HISTORY`
        // static is populated — see main.rs::run().
        let partition = unsafe { EspPartition::new(MC_HIST_PARTITION_LABEL) }?
            .unwrap_or_else(|| {
                panic!(
                    "partition \"{}\" not found — partitions.csv missing mc_hist row \
                     or stale flashed partition table",
                    MC_HIST_PARTITION_LABEL
                )
            });

        let mut store = Self { partition };
        store.migrate_legacy_nvs_once(&nvs_partition)?;
        Ok(store)
    }

    // ── Region/sector addressing ─────────────────────────────────────────────

    fn region_offset(region: usize) -> usize {
        region * REGION_SIZE
    }

    fn sector_offset(region: usize, sector: usize) -> usize {
        Self::region_offset(region) + sector * SECTOR_SIZE
    }

    fn slot_abs_offset(region: usize, sector: usize, slot: usize) -> usize {
        Self::sector_offset(region, sector) + slot_offset(slot)
    }

    // ── Sector header I/O ────────────────────────────────────────────────────

    fn read_header(&mut self, region: usize, sector: usize) -> Result<Option<RegionHeader>, EspError> {
        let mut buf = [0u8; REGION_HEADER_LEN];
        self.partition.read(Self::sector_offset(region, sector), &mut buf)?;
        Ok(decode_region_header(&buf))
    }

    fn write_header(
        &mut self,
        region: usize,
        sector: usize,
        header: &RegionHeader,
    ) -> Result<(), EspError> {
        let bytes = encode_region_header(header);
        self.partition.write(Self::sector_offset(region, sector), &bytes)
    }

    fn erase_sector(&mut self, region: usize, sector: usize) -> Result<(), EspError> {
        self.partition.erase(Self::sector_offset(region, sector), SECTOR_SIZE)
    }

    /// Which sector is authoritative for `region`, and its header — same
    /// generation-resolution rule as `HistoryRegion::active_sector_index`
    /// (higher generation wins when both sectors carry a valid header).
    fn active_sector(&mut self, region: usize) -> Result<Option<(usize, RegionHeader)>, EspError> {
        let h0 = self.read_header(region, 0)?;
        let h1 = self.read_header(region, 1)?;
        Ok(match (h0, h1) {
            (Some(a), Some(b)) => {
                if generation_is_newer(b.generation, a.generation) {
                    Some((1, b))
                } else {
                    Some((0, a))
                }
            }
            (Some(a), None) => Some((0, a)),
            (None, Some(b)) => Some((1, b)),
            (None, None) => None,
        })
    }

    // ── Slot I/O ──────────────────────────────────────────────────────────────

    fn read_slot(
        &mut self,
        region: usize,
        sector: usize,
        slot: usize,
    ) -> Result<[u8; HISTORY_ENTRY_BLOB_LEN], EspError> {
        let mut buf = [0u8; HISTORY_ENTRY_BLOB_LEN];
        self.partition.read(Self::slot_abs_offset(region, sector, slot), &mut buf)?;
        Ok(buf)
    }

    fn write_slot(
        &mut self,
        region: usize,
        sector: usize,
        slot: usize,
        entry: &HistoryEntry,
        is_ours: bool,
        acked: bool,
    ) -> Result<(), EspError> {
        let bytes = encode_slot(entry, is_ours, acked);
        self.partition.write(Self::slot_abs_offset(region, sector, slot), &bytes)
    }

    /// Index of the first `0xFF`-erased slot in `sector` (or
    /// `SLOTS_PER_SECTOR` if full) — the write head. Reads one 72-byte slot
    /// at a time rather than the whole 4 KB sector.
    fn find_write_head(&mut self, region: usize, sector: usize) -> Result<usize, EspError> {
        for i in 0..SLOTS_PER_SECTOR {
            let blob = self.read_slot(region, sector, i)?;
            if blob.iter().all(|&b| b == 0xFF) {
                return Ok(i);
            }
        }
        Ok(SLOTS_PER_SECTOR)
    }

    // ── Directory ─────────────────────────────────────────────────────────────

    /// Find the region already claimed by `(kind, conv_hash)`, without
    /// claiming a new one — `None` if this conversation has no flash-side
    /// history yet.
    ///
    /// Pulled out of `find_or_claim_region`'s pass 1 so [`Self::mark_last_ours_acked`]
    /// can look up an existing region without the claim side-effect a fresh
    /// (never-sent-to) conversation would otherwise trigger.
    fn find_claimed_region(
        &mut self,
        kind: HistoryMsgType,
        conv_hash: u8,
    ) -> Result<Option<usize>, EspError> {
        for region in 0..MAX_CONVERSATION_REGIONS {
            if let Some((_, header)) = self.active_sector(region)? {
                if header.kind == kind && header.conv_hash == conv_hash {
                    return Ok(Some(region));
                }
            }
        }
        Ok(None)
    }

    /// Find the region already claimed by `(kind, conv_hash)`, or claim the
    /// first unclaimed region for it. `None` only if every region is claimed
    /// by a *different* conversation (directory full).
    fn find_or_claim_region(
        &mut self,
        kind: HistoryMsgType,
        conv_hash: u8,
    ) -> Result<Option<usize>, EspError> {
        // Pass 1: does this conversation already own a region?
        if let Some(region) = self.find_claimed_region(kind, conv_hash)? {
            return Ok(Some(region));
        }
        // Pass 2: claim the first unclaimed region.
        for region in 0..MAX_CONVERSATION_REGIONS {
            if self.active_sector(region)?.is_none() {
                self.claim_region(region, kind, conv_hash)?;
                return Ok(Some(region));
            }
        }
        Ok(None)
    }

    /// Claim an (assumed-unclaimed) region: erase both sectors — see module
    /// docs on why this store never trusts "decodes as unclaimed" to imply
    /// "physically erased" — then commit a generation-0 header to sector 0.
    fn claim_region(
        &mut self,
        region: usize,
        kind: HistoryMsgType,
        conv_hash: u8,
    ) -> Result<(), EspError> {
        self.erase_sector(region, 0)?;
        self.erase_sector(region, 1)?;
        self.write_header(region, 0, &RegionHeader { kind, conv_hash, generation: 0 })
    }

    // ── Compaction ────────────────────────────────────────────────────────────

    /// Compact `region`'s active (full) sector into its spare: copy the live
    /// newest-`MAX_HISTORY_ENTRIES` entries, **entries first, header last**
    /// (see `protocol::history_region` module docs — this ordering is what
    /// makes a torn compaction recoverable by generation on next boot). The
    /// spare is unconditionally erased first (module docs: never trust
    /// "decodes as unclaimed" as a proxy for "physically erased").
    fn compact(&mut self, region: usize, active: usize, header: RegionHeader) -> Result<(), EspError> {
        // Gather the live newest-<=32 entries from the active sector before
        // erasing anything.
        let head = self.find_write_head(region, active)?;
        let start = head.saturating_sub(MAX_HISTORY_ENTRIES);
        let mut live: [Option<(HistoryEntry, bool, bool)>; MAX_HISTORY_ENTRIES] =
            [None; MAX_HISTORY_ENTRIES];
        let mut n = 0;
        for slot in start..head {
            let blob = self.read_slot(region, active, slot)?;
            if let Some(triple) = decode_slot(&blob) {
                live[n] = Some(triple);
                n += 1;
            }
        }

        self.rewrite_region(region, active, header, &live[..n])
    }

    /// Carry `entries` (already the live newest-<=32 set, oldest-first) into
    /// `region`'s spare sector and promote it over `active` — the common tail
    /// shared by [`Self::compact`] (verbatim carry-over, triggered when the
    /// active sector fills) and [`Self::mark_last_ours_acked`] (carry-over
    /// with one entry's ack bit corrected, triggered by a live ack event).
    /// Same power-loss-safe sequencing either way: spare erased, entries
    /// written, header committed LAST (atomic hand-off from next boot's
    /// perspective — see module docs), old sector erased only once the new
    /// one is authoritative.
    fn rewrite_region(
        &mut self,
        region: usize,
        active: usize,
        header: RegionHeader,
        entries: &[Option<(HistoryEntry, bool, bool)>],
    ) -> Result<(), EspError> {
        let spare = 1 - active;

        self.erase_sector(region, spare)?;
        for (i, triple) in entries.iter().enumerate() {
            if let Some((entry, is_ours, acked)) = triple {
                self.write_slot(region, spare, i, entry, *is_ours, *acked)?;
            }
        }

        let new_header = RegionHeader {
            kind: header.kind,
            conv_hash: header.conv_hash,
            generation: header.generation.wrapping_add(1),
        };
        self.write_header(region, spare, &new_header)?;

        // The old sector is now stale; erasing it promptly (rather than
        // deferring, which the codec also permits) keeps this store's
        // invariant simple: an unclaimed-or-stale sector is always found
        // already erased by the next claim/compaction that touches it.
        self.erase_sector(region, active)?;

        Ok(())
    }

    /// Flip the acked bit for the newest still-pending (`is_ours && !acked`)
    /// outbound entry in `(kind, conv_hash)`'s region, persisting a live ack
    /// event so it
    /// survives a power-cycle. Returns `true` if an entry was found and
    /// flipped, `false` if the conversation has no region yet or has no
    /// pending outbound entry (nothing to do — not an error).
    ///
    /// # Why this can't be a simple in-place byte update
    ///
    /// NOR flash program operations can only clear bits (`1 -> 0`); setting a
    /// bit back to `1` needs an erase. `encode_slot`'s flags byte is fully
    /// committed at append time — a still-pending send already has
    /// `FLAG_ACKED` programmed to `0` — so flipping it to `1` later needs
    /// exactly the erase-then-rewrite cycle `compact` already performs for a
    /// full sector. This method forces that cycle early (the region is very
    /// likely nowhere near full), carrying every other live entry through
    /// unchanged via the shared [`Self::rewrite_region`] tail.
    pub fn mark_last_ours_acked(
        &mut self,
        kind: HistoryMsgType,
        conv_hash: u8,
    ) -> Result<bool, HistoryStoreError> {
        let Some(region) = self.find_claimed_region(kind, conv_hash)? else {
            return Ok(false);
        };
        let (active, header) = self
            .active_sector(region)?
            .expect("find_claimed_region only returns regions with a valid active sector");

        let head = self.find_write_head(region, active)?;
        let start = head.saturating_sub(MAX_HISTORY_ENTRIES);
        let mut live: [Option<(HistoryEntry, bool, bool)>; MAX_HISTORY_ENTRIES] =
            [None; MAX_HISTORY_ENTRIES];
        let mut n = 0;
        for slot in start..head {
            let blob = self.read_slot(region, active, slot)?;
            if let Some(triple) = decode_slot(&blob) {
                live[n] = Some(triple);
                n += 1;
            }
        }

        // Selection logic delegated to `protocol::find_newest_ours_unacked` —
        // a pure, host-testable function (this store itself only
        // cross-compiles for the esp target and cannot run `cargo test`
        // directly) — same "right message marked" invariant as
        // `ui::mark_last_unacked_outbound`.
        let Some(idx) = find_newest_ours_unacked(&live[..n]) else {
            return Ok(false);
        };
        if let Some((entry, is_ours, _)) = live[idx] {
            live[idx] = Some((entry, is_ours, true));
        }

        self.rewrite_region(region, active, header, &live[..n])?;
        Ok(true)
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Append one entry to its owning conversation region, keyed by
    /// `(kind, conv_hash)`. Compacts first if the active sector is full.
    /// Fails with `DirectoryFull` if `(kind, conv_hash)` is new and every
    /// region already belongs to a different conversation.
    pub fn append_conversation(
        &mut self,
        kind: HistoryMsgType,
        conv_hash: u8,
        entry: &HistoryEntry,
        is_ours: bool,
        acked: bool,
    ) -> Result<(), HistoryStoreError> {
        let region = self
            .find_or_claim_region(kind, conv_hash)?
            .ok_or(HistoryStoreError::DirectoryFull)?;

        let (mut active, mut header) =
            self.active_sector(region)?.expect("just found-or-claimed above");
        let mut head = self.find_write_head(region, active)?;
        if head >= SLOTS_PER_SECTOR {
            self.compact(region, active, header)?;
            let (new_active, new_header) =
                self.active_sector(region)?.expect("compact() just committed a header");
            active = new_active;
            header = new_header;
            head = self.find_write_head(region, active)?;
        }
        let _ = header; // only needed to drive compaction above
        self.write_slot(region, active, head, entry, is_ours, acked)?;
        Ok(())
    }

    /// Append one entry using `entry.msg_type`/`entry.sender_hash` as the
    /// conversation key — the shape the legacy-NVS migration's carried-over
    /// entries already come in (`sender_hash` already carries the
    /// conversation hash for both `Dm` and `GrpTxt`). `is_ours` is always
    /// `false` here since migrated legacy entries predate the `is_ours` bit
    /// (byte-compatible: a legacy blob's reserved byte was always `0x00`).
    /// `acked` is likewise always `true` here — legacy entries predate the
    /// ack bit too (same byte-faithful `0x00` reserved byte), and `acked` is
    /// only ever rendered for `is_ours` entries, which this path never
    /// produces, so the value is inert either way; `true` documents "no
    /// pending ACK to model" rather than an arbitrary default.
    /// `main.rs::append_history` (both RX and TX call sites) calls
    /// [`Self::append_conversation`]
    /// directly instead so it can pass a real `is_ours`/`acked`.
    pub fn append(&mut self, entry: &HistoryEntry) -> Result<(), HistoryStoreError> {
        self.append_conversation(entry.msg_type, entry.sender_hash, entry, false, true)
    }

    /// Load every claimed conversation's live entries (oldest-first, newest
    /// ~[`MAX_HISTORY_ENTRIES`] retained per the ring-bound/compaction
    /// invariant `append_conversation` already enforces), grouped by
    /// conversation — `(kind, conv_hash, entries)`, one tuple per claimed
    /// region, regions visited in directory (claim) order. Each entry keeps
    /// its decoded `is_ours` bit (unlike [`Self::export_entries`], which
    /// discards it — that method serves the wire-export path, which doesn't
    /// need it).
    ///
    /// Driving API for UI boot hydrate: `main.rs::run()` calls this once,
    /// right after this store is constructed, and feeds each conversation's
    /// entries to `UiRuntime::seed_conversation` so the contact/channel list
    /// previews and the conversation view reflect restored history from the
    /// very first frame — not only after a live send/receive.
    ///
    /// Each entry's ack/delivery bit is now also forwarded
    /// rather than the caller assuming a blanket "delivered" default — that
    /// assumption is exactly the bug this fixes: a power-cycle must
    /// restore the same ✓/✓✓ checkmark the message showed before reboot, not
    /// a hardcoded "always delivered".
    pub fn load_all_conversations(
        &mut self,
    ) -> Result<Vec<(HistoryMsgType, u8, Vec<(HistoryEntry, bool, bool)>)>, EspError> {
        let mut out = Vec::new();
        for region in 0..MAX_CONVERSATION_REGIONS {
            let Some((active, header)) = self.active_sector(region)? else { continue };
            let head = self.find_write_head(region, active)?;
            let start = head.saturating_sub(MAX_HISTORY_ENTRIES);
            let mut entries = Vec::with_capacity(head - start);
            for slot in start..head {
                let blob = self.read_slot(region, active, slot)?;
                if let Some(triple) = decode_slot(&blob) {
                    entries.push(triple);
                }
            }
            out.push((header.kind, header.conv_hash, entries));
        }
        Ok(out)
    }

    /// Call `cb(index, entry, is_ours)` for every live entry across every
    /// claimed conversation region, oldest-first *within* each region,
    /// regions visited in directory (claim) order. `index` is a running
    /// 0-based counter across the whole export stream.
    ///
    /// This satisfies the export-parity contract directly:
    /// entries are aggregated *grouped by conversation* (one region's full
    /// run before the next), oldest-first within each region, globally
    /// re-indexed — not a timestamp-merge across conversations (rejected:
    /// fragile when the RTC is unsynced/zero). `admin_server.rs`'s
    /// `FRAME_EXPORT_HISTORY` handler streams this unchanged over the wire;
    /// see `protocol::history_region`'s host-testable
    /// `export_groups_by_conversation_oldest_first_globally_reindexed` test
    /// for a pure-model regression guard on this exact ordering (this store
    /// itself only cross-compiles for the esp target, so it cannot run in
    /// `cargo test` directly).
    ///
    /// `is_ours` is
    /// now forwarded rather than discarded — `sender_hash` alone cannot tell
    /// a conversation's inbound entries from its outbound ones (it is always
    /// the conversation hash, never the device's own hash, for either
    /// direction), so the wire export needs the flag explicitly to let
    /// `export-history` render direction.
    ///
    /// `index` is `u8`, matching the unchanged wire payload's 1-byte index
    /// field (`protocol::history::encode_rsp_history_entry`); it saturates
    /// (rather than wraps) past 255 total entries — the host CLI never reads
    /// this byte back (`Session::export_history` decodes and discards it,
    /// rebuilding its own display index from receive order in
    /// `host/src/main.rs`), so saturation here does not corrupt host output
    /// even for a device with all 24 conversation regions near-full.
    ///
    /// The slot's ack/delivery bit is decoded but
    /// deliberately NOT forwarded to `cb` — the wire export frame
    /// (`FRAME_RSP_HISTORY_ENTRY`) has no ack field and this does not
    /// extend it (out of scope: only the on-device flash slot format and UI
    /// hydrate need the bit; `export-history` is host-side, unaffected).
    pub fn export_entries<F: FnMut(u8, &HistoryEntry, bool)>(
        &mut self,
        mut cb: F,
    ) -> Result<u8, EspError> {
        let mut idx: u8 = 0;
        for region in 0..MAX_CONVERSATION_REGIONS {
            let Some((active, _header)) = self.active_sector(region)? else { continue };
            let head = self.find_write_head(region, active)?;
            let start = head.saturating_sub(MAX_HISTORY_ENTRIES);
            for slot in start..head {
                let blob = self.read_slot(region, active, slot)?;
                if let Some((entry, is_ours, _acked)) = decode_slot(&blob) {
                    cb(idx, &entry, is_ours);
                    idx = idx.saturating_add(1);
                }
            }
        }
        Ok(idx)
    }

    /// Erase every sector of every conversation region, wiping ALL persisted
    /// history — every DM contact and channel conversation, both inbound and
    /// outbound `HistoryEntry` regardless of `is_ours` — unconditionally.
    ///
    /// Unlike [`Self::claim_region`]'s erase (only the two sectors backing one
    /// about-to-be-claimed region), this walks every one of the
    /// [`MAX_CONVERSATION_REGIONS`] regions and erases both of its sectors
    /// regardless of current claim state: a region already claimed by some
    /// conversation, a region mid-compaction, and a never-claimed region are
    /// all erased identically. Builds on the same private [`Self::erase_sector`]
    /// primitive every other flash-mutating path in this store already uses.
    ///
    /// This store caches no directory/index in RAM — every query
    /// ([`Self::active_sector`], [`Self::find_write_head`], etc.) re-reads
    /// flash headers on demand — so there is no separate in-memory index/head
    /// state to reset beyond the erase itself: the very next `active_sector`
    /// call on any region reads back `(None, None)` (both sectors decode as
    /// unclaimed), exactly as it would for a factory-fresh partition, and
    /// [`Self::load_all_conversations`] / [`Self::export_entries`] therefore
    /// return zero conversations / zero entries immediately after this call
    /// returns, no reboot required to see that specific effect. (The *UI's*
    /// in-memory `messages`/`unread` maps are a separate, main-thread-owned
    /// concern this store has no reach into — see the `CLEAR_HISTORY` ADR-0002
    /// amendment and `admin_server`'s handler doc comment for the
    /// reboot-required-for-the-UI design decision.)
    ///
    /// # Cost and failure behavior
    ///
    /// `MAX_CONVERSATION_REGIONS * SECTORS_PER_REGION` (48) individual 4 KB
    /// sector erases, each a real (non-trivial: tens of milliseconds on this
    /// hardware) NOR-flash operation — the caller (`admin_server`) holds the
    /// shared `HISTORY` mutex for the entire loop, so a concurrent
    /// main-thread `append_conversation` (a live DM/channel receive) blocks
    /// for the duration. This is an accepted, bounded stall on a rare,
    /// deliberate admin-initiated action, not a hot path — chunking the
    /// erase to release the lock between sectors would add real complexity
    /// for no correctness benefit and was not justified here.
    /// If `erase_sector` fails partway through (real flash fault), the loop
    /// returns early via `?` leaving some regions erased and others not; this
    /// is safe to retry — every region is erased unconditionally regardless
    /// of its current claim state, so re-issuing `clear-history` finishes
    /// erasing whatever the first attempt did not reach, it does not need to
    /// know where the first attempt stopped.
    pub fn clear_all(&mut self) -> Result<(), EspError> {
        for region in 0..MAX_CONVERSATION_REGIONS {
            for sector in 0..SECTORS_PER_REGION {
                self.erase_sector(region, sector)?;
            }
        }
        Ok(())
    }

    // ── Legacy-NVS migration (one-shot) ──────────────────────────────────────

    /// Read the legacy per-slot NVS history exactly once and bucket every
    /// decoded entry into its owning conversation region, then mark
    /// migration done. No-op (fast, single 1-byte NVS read) on every boot
    /// after the first.
    fn migrate_legacy_nvs_once(
        &mut self,
        nvs_partition: &EspNvsPartition<NvsDefault>,
    ) -> Result<(), HistoryStoreError> {
        let nvs = EspNvs::new(nvs_partition.clone(), LEGACY_NVS_NAMESPACE, true)?;

        let mut marker = [0u8; 1];
        if nvs.get_blob(LEGACY_KEY_MIGRATED, &mut marker)?.is_some() && marker[0] != 0 {
            return Ok(()); // already migrated on a prior boot
        }

        let (head, cnt) = Self::read_legacy_meta(&nvs)?;
        let count = cnt as usize;
        let oldest_slot = if count < MAX_HISTORY_ENTRIES { 0usize } else { head as usize };

        for i in 0..count {
            let slot = (oldest_slot + i) % MAX_HISTORY_ENTRIES;
            let mut key_buf = [0u8; 3];
            let key = legacy_slot_key(slot, &mut key_buf);
            let mut raw = [0u8; HISTORY_ENTRY_BLOB_LEN];
            if nvs.get_blob(key, &mut raw)?.is_some() {
                if let Some(entry) = decode_entry_blob(&raw) {
                    // Legacy blobs never carried an `is_ours` bit (reserved
                    // byte was always 0x00) — `is_ours = false` is exactly
                    // byte-faithful to what was actually recorded.
                    if let Err(e) = self.append(&entry) {
                        log::warn!("history migration: append failed for slot {}: {:?}", slot, e);
                    }
                }
            }
        }

        nvs.set_blob(LEGACY_KEY_MIGRATED, &[1u8])?;
        log::info!("history migration: {} legacy entr{} migrated into per-conversation regions",
            count, if count == 1 { "y" } else { "ies" });

        Ok(())
    }

    fn read_legacy_meta(nvs: &EspNvs<NvsDefault>) -> Result<(u8, u8), EspError> {
        let mut buf = [0u8; 1];
        let head = match nvs.get_blob(LEGACY_KEY_HEAD, &mut buf)? {
            Some(_) => buf[0].min((MAX_HISTORY_ENTRIES - 1) as u8),
            None => 0,
        };
        let cnt = match nvs.get_blob(LEGACY_KEY_CNT, &mut buf)? {
            Some(_) => buf[0].min(MAX_HISTORY_ENTRIES as u8),
            None => 0,
        };
        Ok((head, cnt))
    }
}
