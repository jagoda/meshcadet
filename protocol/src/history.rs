// SPDX-License-Identifier: GPL-3.0-only
//! Firmware rotating history — codec and in-memory ring buffer.
//!
//! This module is `no_std`-safe and host-testable (features: none, hil).
//!
//! # NVS layout (firmware side)
//!
//! | NVS key  | Type     | Description |
//! |----------|----------|-------------|
//! | `head`   | u8 blob  | ring-buffer head pointer (next write slot) |
//! | `cnt`    | u8 blob  | how many valid entries (0..=MAX_HISTORY_ENTRIES) |
//! | `blob`   | 2304-byte blob | flat array of 32 × 72-byte encoded entries |
//!
//! # Wire export protocol (ADR-0002 extension)
//!
//! Host sends `FRAME_EXPORT_HISTORY` (0x71).
//! Device replies with N × `FRAME_RSP_HISTORY_ENTRY` (0x84), then
//! `FRAME_RSP_HISTORY_DONE` (0x85).  Entries are oldest-first.
//!
//! Each `FRAME_RSP_HISTORY_ENTRY` payload:
//!   - byte 0:      entry index (0-based, oldest = 0)
//!   - byte 1:      sender_hash (first byte of Ed25519 pubkey; for both
//!                  directions this is the *conversation* hash — contact hash
//!                  for a DM, channel hash for a GrpTxt — matching the
//!                  per-conversation region key, NOT the device's own hash
//!                  for outbound entries)
//!   - byte 2:      msg_type (0 = DM, 1 = GrpTxt)
//!   - bytes 3..6:  timestamp (u32 LE)
//!   - byte 7:      text_len
//!   - byte 8:      is_ours (0 = received, 1 = sent by this device — mirrors
//!                  `history_region::FLAG_IS_OURS`; added so `export-history`
//!                  can distinguish direction, since `sender_hash` alone
//!                  cannot)
//!   - bytes 9..:   text (text_len bytes, no NUL)
//!
//! Maximum payload = 9 + 64 = 73 bytes.

// No std dependencies — safe for no_std firmware.

/// Maximum UTF-8 text bytes stored per message (truncated at encode time).
pub const MAX_HISTORY_TEXT_LEN: usize = 64;

/// Maximum number of entries in the ring buffer.
pub const MAX_HISTORY_ENTRIES: usize = 32;

/// Bytes used to encode one entry in the NVS blob:
///   sender_hash(1) + msg_type(1) + timestamp(4) + text_len(1) + text(64) + reserved(1) = 72
pub const HISTORY_ENTRY_BLOB_LEN: usize = 72;

/// Total NVS blob size: 32 × 72 = 2304 bytes.
pub const HISTORY_BLOB_LEN: usize = MAX_HISTORY_ENTRIES * HISTORY_ENTRY_BLOB_LEN;

/// Maximum bytes in a `FRAME_RSP_HISTORY_ENTRY` payload:
///   index(1) + sender_hash(1) + msg_type(1) + timestamp(4) + text_len(1) +
///   is_ours(1) + text(64) = 73.
pub const MAX_RSP_HISTORY_ENTRY_PAYLOAD: usize = 73;

// ── HistoryMsgType ────────────────────────────────────────────────────────────

/// Message type tag stored with each history entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryMsgType {
    /// Direct message (unicast DM).
    Dm = 0,
    /// Group text (channel broadcast).
    GrpTxt = 1,
}

impl HistoryMsgType {
    pub(crate) fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Dm),
            1 => Some(Self::GrpTxt),
            _ => None,
        }
    }
}

// ── HistoryEntry ──────────────────────────────────────────────────────────────

/// One conversation history entry.
///
/// This struct is `Copy` so it can be stored in a stack ring buffer for tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    /// First byte of the sender's Ed25519 public key.
    pub sender_hash: u8,
    /// Message type.
    pub msg_type: HistoryMsgType,
    /// Unix timestamp (seconds, wraps at 2^32).
    pub timestamp: u32,
    /// Message text, UTF-8 (not NUL-terminated; only `text[..text_len]` is valid).
    pub text: [u8; MAX_HISTORY_TEXT_LEN],
    /// Number of valid bytes in `text`.
    pub text_len: u8,
}

// ── NVS blob codec ────────────────────────────────────────────────────────────

/// Encode a `HistoryEntry` into a 72-byte NVS blob.
///
/// Layout:
///   [0]      sender_hash
///   [1]      msg_type (0 = DM, 1 = GrpTxt)
///   [2..5]   timestamp (u32 LE)
///   [6]      text_len
///   [7..70]  text (64 bytes, zero-padded)
///   [71]     reserved / version (0x00)
pub fn encode_entry_blob(entry: &HistoryEntry, out: &mut [u8; HISTORY_ENTRY_BLOB_LEN]) {
    out[0] = entry.sender_hash;
    out[1] = entry.msg_type as u8;
    out[2] = (entry.timestamp & 0xFF) as u8;
    out[3] = ((entry.timestamp >> 8) & 0xFF) as u8;
    out[4] = ((entry.timestamp >> 16) & 0xFF) as u8;
    out[5] = ((entry.timestamp >> 24) & 0xFF) as u8;
    out[6] = entry.text_len;
    let tlen = entry.text_len as usize;
    out[7..7 + MAX_HISTORY_TEXT_LEN].copy_from_slice(&entry.text);
    // zero-pad any tail already from copy_from_slice; reserved byte
    let _ = tlen; // text is always MAX_HISTORY_TEXT_LEN bytes in the array
    out[71] = 0x00; // reserved
}

/// Decode a 72-byte NVS blob into a `HistoryEntry`.
///
/// Returns `None` if the msg_type byte is unrecognised.
pub fn decode_entry_blob(blob: &[u8; HISTORY_ENTRY_BLOB_LEN]) -> Option<HistoryEntry> {
    let sender_hash = blob[0];
    let msg_type = HistoryMsgType::from_u8(blob[1])?;
    let timestamp = (blob[2] as u32)
        | ((blob[3] as u32) << 8)
        | ((blob[4] as u32) << 16)
        | ((blob[5] as u32) << 24);
    let text_len = blob[6];
    let mut text = [0u8; MAX_HISTORY_TEXT_LEN];
    text.copy_from_slice(&blob[7..7 + MAX_HISTORY_TEXT_LEN]);
    Some(HistoryEntry { sender_hash, msg_type, timestamp, text, text_len })
}

// ── Wire export codec ─────────────────────────────────────────────────────────

/// Encode a `FRAME_RSP_HISTORY_ENTRY` payload.
///
/// Layout:
///   [0]      entry index (0-based, oldest = 0)
///   [1]      sender_hash
///   [2]      msg_type (0 = DM, 1 = GrpTxt)
///   [3..6]   timestamp (u32 LE)
///   [7]      text_len
///   [8]      is_ours (0 = received, 1 = sent by this device)
///   [9..]    text (text_len bytes)
///
/// `is_ours` is carried separately from `HistoryEntry` (mirroring
/// `history_region`'s slot codec, which also keeps it out of the entry
/// struct) so `export-history` can distinguish direction — `sender_hash`
/// alone cannot, since it is always the *conversation* hash for both
/// directions.
///
/// Returns the number of bytes written to `out`.
pub fn encode_rsp_history_entry(
    index: u8,
    entry: &HistoryEntry,
    is_ours: bool,
    out: &mut [u8],
) -> usize {
    let tlen = entry.text_len as usize;
    let total = 9 + tlen;
    if out.len() < total {
        return 0; // caller must provide sufficient buffer
    }
    out[0] = index;
    out[1] = entry.sender_hash;
    out[2] = entry.msg_type as u8;
    out[3] = (entry.timestamp & 0xFF) as u8;
    out[4] = ((entry.timestamp >> 8) & 0xFF) as u8;
    out[5] = ((entry.timestamp >> 16) & 0xFF) as u8;
    out[6] = ((entry.timestamp >> 24) & 0xFF) as u8;
    out[7] = entry.text_len;
    out[8] = is_ours as u8;
    out[9..9 + tlen].copy_from_slice(&entry.text[..tlen]);
    total
}

/// Decode a `FRAME_RSP_HISTORY_ENTRY` payload.
///
/// Returns `Some((index, entry, is_ours))` on success, or `None` if the
/// payload is too short or contains an unrecognised msg_type.
pub fn decode_rsp_history_entry(payload: &[u8]) -> Option<(u8, HistoryEntry, bool)> {
    if payload.len() < 9 {
        return None;
    }
    let index = payload[0];
    let sender_hash = payload[1];
    let msg_type = HistoryMsgType::from_u8(payload[2])?;
    let timestamp = (payload[3] as u32)
        | ((payload[4] as u32) << 8)
        | ((payload[5] as u32) << 16)
        | ((payload[6] as u32) << 24);
    let text_len = payload[7] as usize;
    let is_ours = payload[8] != 0;
    if payload.len() < 9 + text_len || text_len > MAX_HISTORY_TEXT_LEN {
        return None;
    }
    let mut text = [0u8; MAX_HISTORY_TEXT_LEN];
    text[..text_len].copy_from_slice(&payload[9..9 + text_len]);
    Some((index, HistoryEntry {
        sender_hash,
        msg_type,
        timestamp,
        text,
        text_len: text_len as u8,
    }, is_ours))
}

// ── In-memory ring buffer (for host tests / firmware integration tests) ───────

/// An in-memory rotating ring buffer of `MAX_HISTORY_ENTRIES` `HistoryEntry` values.
///
/// Oldest entry ages out when the buffer is full (ring overwrite).
///
/// Used by host-side tests; the firmware uses `HistoryStore` (NVS-backed).
pub struct RingBuffer {
    slots: [HistoryEntry; MAX_HISTORY_ENTRIES],
    /// Index of next write slot (head).
    head: usize,
    /// Number of valid entries (saturates at MAX_HISTORY_ENTRIES).
    count: usize,
}

impl RingBuffer {
    /// Create an empty ring buffer.
    pub fn new() -> Self {
        let empty = HistoryEntry {
            sender_hash: 0,
            msg_type: HistoryMsgType::Dm,
            timestamp: 0,
            text: [0u8; MAX_HISTORY_TEXT_LEN],
            text_len: 0,
        };
        Self {
            slots: [empty; MAX_HISTORY_ENTRIES],
            head: 0,
            count: 0,
        }
    }

    /// Append an entry, overwriting the oldest if full.
    pub fn append(&mut self, entry: &HistoryEntry) {
        self.slots[self.head] = *entry;
        self.head = (self.head + 1) % MAX_HISTORY_ENTRIES;
        if self.count < MAX_HISTORY_ENTRIES {
            self.count += 1;
        }
    }

    /// Number of valid entries.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns `true` if the buffer contains no entries.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Iterate entries oldest-first, calling `f(index, entry)` for each.
    ///
    /// `index` is 0-based (oldest = 0).
    pub fn iter_oldest_first<F: FnMut(u8, &HistoryEntry)>(&self, mut f: F) {
        if self.count == 0 {
            return;
        }
        // Oldest slot: if not yet full, slot 0; otherwise head (next overwrite slot).
        let oldest_slot = if self.count < MAX_HISTORY_ENTRIES {
            0
        } else {
            self.head // head points at the slot about to be overwritten = oldest
        };
        for i in 0..self.count {
            let slot = (oldest_slot + i) % MAX_HISTORY_ENTRIES;
            f(i as u8, &self.slots[slot]);
        }
    }

    /// Retrieve entry by oldest-first index.  Returns `None` if out of range.
    pub fn get(&self, index: usize) -> Option<HistoryEntry> {
        if index >= self.count {
            return None;
        }
        let oldest_slot = if self.count < MAX_HISTORY_ENTRIES {
            0
        } else {
            self.head
        };
        let slot = (oldest_slot + index) % MAX_HISTORY_ENTRIES;
        Some(self.slots[slot])
    }
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn dm(sender: u8, ts: u32, text: &[u8]) -> HistoryEntry {
        let text_len = text.len().min(MAX_HISTORY_TEXT_LEN) as u8;
        let mut text_buf = [0u8; MAX_HISTORY_TEXT_LEN];
        text_buf[..text_len as usize].copy_from_slice(&text[..text_len as usize]);
        HistoryEntry {
            sender_hash: sender,
            msg_type: HistoryMsgType::Dm,
            timestamp: ts,
            text: text_buf,
            text_len,
        }
    }

    fn grp(sender: u8, ts: u32, text: &[u8]) -> HistoryEntry {
        let mut e = dm(sender, ts, text);
        e.msg_type = HistoryMsgType::GrpTxt;
        e
    }

    // ── NVS blob codec ────────────────────────────────────────────────────

    #[test]
    fn blob_roundtrip_dm() {
        let entry = dm(0xAB, 0xDEAD_BEEF, b"hello world");
        let mut blob = [0u8; HISTORY_ENTRY_BLOB_LEN];
        encode_entry_blob(&entry, &mut blob);
        let decoded = decode_entry_blob(&blob).expect("decode must succeed");
        assert_eq!(decoded.sender_hash, 0xAB);
        assert_eq!(decoded.msg_type, HistoryMsgType::Dm);
        assert_eq!(decoded.timestamp, 0xDEAD_BEEF);
        let text = &decoded.text[..decoded.text_len as usize];
        assert_eq!(text, b"hello world");
    }

    #[test]
    fn blob_roundtrip_grptxt() {
        let entry = grp(0x01, 99, b"group msg");
        let mut blob = [0u8; HISTORY_ENTRY_BLOB_LEN];
        encode_entry_blob(&entry, &mut blob);
        let decoded = decode_entry_blob(&blob).unwrap();
        assert_eq!(decoded.msg_type, HistoryMsgType::GrpTxt);
        assert_eq!(&decoded.text[..decoded.text_len as usize], b"group msg");
    }

    #[test]
    fn blob_invalid_msg_type_returns_none() {
        let mut blob = [0u8; HISTORY_ENTRY_BLOB_LEN];
        blob[1] = 0xFF; // unrecognised msg_type
        assert!(decode_entry_blob(&blob).is_none());
    }

    #[test]
    fn blob_max_text_roundtrip() {
        let text = [b'X'; MAX_HISTORY_TEXT_LEN];
        let entry = dm(0x01, 1, &text);
        let mut blob = [0u8; HISTORY_ENTRY_BLOB_LEN];
        encode_entry_blob(&entry, &mut blob);
        let decoded = decode_entry_blob(&blob).unwrap();
        assert_eq!(decoded.text_len as usize, MAX_HISTORY_TEXT_LEN);
        assert_eq!(&decoded.text[..], &text[..]);
    }

    // ── Wire export codec ─────────────────────────────────────────────────

    #[test]
    fn rsp_entry_roundtrip() {
        let entry = dm(0xCC, 1234567, b"wire msg");
        let mut payload = [0u8; MAX_RSP_HISTORY_ENTRY_PAYLOAD + 1];
        let plen = encode_rsp_history_entry(3, &entry, false, &mut payload);
        assert_eq!(plen, 9 + 8); // header(9) + text(8)
        let (idx, decoded, is_ours) = decode_rsp_history_entry(&payload[..plen]).expect("decode");
        assert_eq!(idx, 3);
        assert_eq!(decoded.sender_hash, 0xCC);
        assert_eq!(decoded.timestamp, 1234567);
        assert_eq!(&decoded.text[..decoded.text_len as usize], b"wire msg");
        assert!(!is_ours);
    }

    #[test]
    fn rsp_entry_is_ours_round_trips_true() {
        let entry = dm(0x67, 42, b"sent by me");
        let mut payload = [0u8; MAX_RSP_HISTORY_ENTRY_PAYLOAD + 1];
        let plen = encode_rsp_history_entry(0, &entry, true, &mut payload);
        let (_, _, is_ours) = decode_rsp_history_entry(&payload[..plen]).expect("decode");
        assert!(is_ours, "is_ours=true must survive the wire round-trip");
    }

    #[test]
    fn rsp_entry_too_short_returns_none() {
        assert!(decode_rsp_history_entry(&[0u8; 8]).is_none());
        assert!(decode_rsp_history_entry(&[]).is_none());
    }

    #[test]
    fn rsp_entry_bad_msg_type_returns_none() {
        let mut payload = [0u8; 10];
        payload[2] = 0xFF; // bad msg_type
        payload[7] = 0;    // text_len = 0
        assert!(decode_rsp_history_entry(&payload).is_none());
    }

    #[test]
    fn rsp_entry_text_len_overflow_returns_none() {
        let mut payload = [0u8; 10];
        payload[2] = 0; // DM
        payload[7] = 2; // text_len = 2, but only 1 byte follows
        assert!(decode_rsp_history_entry(&payload[..9 + 1]).is_none());
    }

    // ── RingBuffer ────────────────────────────────────────────────────────

    #[test]
    fn ring_buffer_starts_empty() {
        let rb = RingBuffer::new();
        assert_eq!(rb.len(), 0);
        assert!(rb.is_empty());
        assert!(rb.get(0).is_none());
    }

    #[test]
    fn ring_buffer_append_and_get() {
        let mut rb = RingBuffer::new();
        let e = dm(0x01, 100, b"test");
        rb.append(&e);
        assert_eq!(rb.len(), 1);
        let got = rb.get(0).expect("index 0 must exist");
        assert_eq!(got.sender_hash, 0x01);
        assert_eq!(got.timestamp, 100);
    }

    #[test]
    fn ring_buffer_oldest_first_order() {
        let mut rb = RingBuffer::new();
        for i in 0..5 {
            rb.append(&dm(i, i as u32 * 100, b"msg"));
        }
        let mut ts_seq = Vec::new();
        rb.iter_oldest_first(|_, e| ts_seq.push(e.timestamp));
        assert_eq!(ts_seq, vec![0, 100, 200, 300, 400]);
    }

    #[test]
    fn ring_buffer_get_oldest_first() {
        let mut rb = RingBuffer::new();
        for i in 0..5u8 {
            rb.append(&dm(i, i as u32 * 10, b"x"));
        }
        assert_eq!(rb.get(0).unwrap().timestamp, 0);
        assert_eq!(rb.get(4).unwrap().timestamp, 40);
        assert!(rb.get(5).is_none());
    }

    /// Acceptance: history rotates at bound — MAX_HISTORY_ENTRIES + 1 appends
    /// results in exactly MAX_HISTORY_ENTRIES entries, oldest discarded.
    #[test]
    fn ring_buffer_rotation_at_bound() {
        let mut rb = RingBuffer::new();
        // Fill to capacity.
        for i in 0..MAX_HISTORY_ENTRIES as u8 {
            rb.append(&dm(i, i as u32, b"msg"));
        }
        assert_eq!(rb.len(), MAX_HISTORY_ENTRIES);

        // One more: oldest (ts=0) is overwritten.
        rb.append(&dm(0xFF, MAX_HISTORY_ENTRIES as u32, b"new"));
        assert_eq!(rb.len(), MAX_HISTORY_ENTRIES, "count must not exceed MAX_HISTORY_ENTRIES");

        // New oldest has timestamp = 1 (the entry with ts=0 was overwritten).
        let oldest = rb.get(0).unwrap();
        assert_eq!(oldest.timestamp, 1, "oldest must have been rotated out");

        // Newest is the one we just appended.
        let newest = rb.get(MAX_HISTORY_ENTRIES - 1).unwrap();
        assert_eq!(newest.timestamp, MAX_HISTORY_ENTRIES as u32);
    }

    #[test]
    fn ring_buffer_fill_exactly_max_no_rotation() {
        let mut rb = RingBuffer::new();
        for i in 0..MAX_HISTORY_ENTRIES as u8 {
            rb.append(&dm(i, i as u32 * 5, b"x"));
        }
        assert_eq!(rb.len(), MAX_HISTORY_ENTRIES);
        // First entry still oldest (no rotation yet).
        assert_eq!(rb.get(0).unwrap().timestamp, 0);
    }

    #[test]
    fn ring_buffer_multiple_rotations() {
        let mut rb = RingBuffer::new();
        let total = MAX_HISTORY_ENTRIES * 3 + 7;
        for i in 0..total {
            rb.append(&dm(0, i as u32, b"x"));
        }
        assert_eq!(rb.len(), MAX_HISTORY_ENTRIES);
        // Oldest timestamp = total - MAX_HISTORY_ENTRIES
        let expected_oldest_ts = (total - MAX_HISTORY_ENTRIES) as u32;
        assert_eq!(rb.get(0).unwrap().timestamp, expected_oldest_ts);
        assert_eq!(rb.get(MAX_HISTORY_ENTRIES - 1).unwrap().timestamp, (total - 1) as u32);
    }

    /// Acceptance: export intact — encode/decode each entry round-trips.
    #[test]
    fn export_roundtrip_all_entries() {
        let mut rb = RingBuffer::new();
        let entries = [
            dm(0x01, 100, b"first"),
            grp(0x02, 200, b"second"),
            dm(0x03, 300, b"third"),
        ];
        for e in &entries {
            rb.append(e);
        }

        let mut exported: Vec<(u8, HistoryEntry)> = Vec::new();
        rb.iter_oldest_first(|idx, e| {
            let mut pbuf = [0u8; MAX_RSP_HISTORY_ENTRY_PAYLOAD + 1];
            let plen = encode_rsp_history_entry(idx, e, false, &mut pbuf);
            let (got_idx, got_entry, _is_ours) = decode_rsp_history_entry(&pbuf[..plen]).unwrap();
            exported.push((got_idx, got_entry));
        });

        assert_eq!(exported.len(), 3);
        assert_eq!(exported[0].0, 0);
        assert_eq!(exported[0].1.timestamp, 100);
        assert_eq!(exported[1].0, 1);
        assert_eq!(exported[1].1.msg_type, HistoryMsgType::GrpTxt);
        assert_eq!(exported[2].0, 2);
        assert_eq!(&exported[2].1.text[..exported[2].1.text_len as usize], b"third");
    }
}
