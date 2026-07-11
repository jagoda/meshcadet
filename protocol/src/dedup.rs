// SPDX-License-Identifier: GPL-3.0-only
//! Inbound-packet duplicate-detection key.
//!
//! Source reference: `Packet::calculatePacketHash` (`src/Packet.cpp:41 @ dee3e26a`).
//!
//! MeshCore dedups packets by hashing only the IMMUTABLE part of the frame — the
//! payload type and the payload bytes — and deliberately EXCLUDES the 1-byte
//! header and the variable path field. A flood relay appends its own hash to the
//! path and bumps the hop count on every forward, so those bytes differ between
//! copies of one logical packet. Hashing the whole frame instead (the bug this
//! module fixes) gives each relayed copy a distinct key, so duplicates slip past
//! the seen-packet ring and are displayed/ACKed repeatedly.

use crate::crypto::sha256_2;

/// Split a wire frame into `(payload_type, payload)`, skipping the 1-byte header
/// and the variable path field (`hash_size × hop_count` bytes).
///
/// Frame layout: `header(1) | path_len(1) | path(hash_size × hop_count) | payload`.
/// `hash_size = (path_len >> 6) + 1`, `hop_count = path_len & 0x3F`,
/// `payload_type = (header >> 2) & 0x0F`.
///
/// Returns `None` when the frame is shorter than its declared path (malformed).
pub fn packet_payload_view(frame: &[u8]) -> Option<(u8, &[u8])> {
    if frame.len() < 2 {
        return None;
    }
    let header_byte = frame[0];
    let path_len_byte = frame[1];
    let hash_size = ((path_len_byte >> 6) + 1) as usize;
    let hop_count = (path_len_byte & 0x3F) as usize;
    let payload_off = 2 + hop_count * hash_size;
    if frame.len() < payload_off {
        return None;
    }
    Some(((header_byte >> 2) & 0x0F, &frame[payload_off..]))
}

/// 4-byte duplicate-detection key: `SHA-256(payload_type || payload)[0:4]`.
///
/// Computed over the immutable frame bytes so every flood-relayed copy of one
/// logical packet shares a key. Malformed frames (too short for their declared
/// path) fall back to hashing the whole frame, so byte-identical repeats still
/// dedup while distinct junk frames stay distinct.
pub fn packet_dedup_key(frame: &[u8]) -> [u8; 4] {
    let h = match packet_payload_view(frame) {
        Some((ptype, payload)) => sha256_2(&[ptype], payload),
        None => sha256_2(frame, &[]),
    };
    [h[0], h[1], h[2], h[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a flood GRP_TXT-style frame: `header | path_len | path | payload`,
    /// with `hops` 2-byte path entries appended (mirrors a flood relay).
    fn grp_frame(hops: u8, payload: &[u8]) -> Vec<u8> {
        // header = GRP_TXT(0x05)<<2 | FLOOD(0x01) = 0x15
        // path_len = (hash_size=2 → 0x40) | hop_count
        let mut f = vec![0x15u8, 0x40 | hops];
        for h in 0..hops {
            f.push(0xA0 + h); // hop hash byte 0
            f.push(0xB0 + h); // hop hash byte 1 (2-byte hashes)
        }
        f.extend_from_slice(payload);
        f
    }

    #[test]
    fn payload_view_skips_header_and_path() {
        let frame = grp_frame(2, b"hello"); // 2 hops → 4 path bytes
        let (ptype, payload) = packet_payload_view(&frame).unwrap();
        assert_eq!(ptype, 0x05, "GRP_TXT payload type");
        assert_eq!(payload, b"hello", "payload starts after header + path");
    }

    /// REGRESSION (ISSUE 2): the SAME logical packet relayed over different paths
    /// must produce the SAME dedup key. The path mutates on every flood hop, so
    /// hashing the whole frame let each copy through; hashing payload-only
    /// collapses them to one key.
    #[test]
    fn dedup_key_invariant_under_path_mutation() {
        let payload = b"\x6dchannel hello";
        let direct = grp_frame(0, payload); // as originated
        let relay1 = grp_frame(1, payload); // +1 relay hop
        let relay3 = grp_frame(3, payload); // +3 relay hops, longer path

        // Frames differ byte-for-byte…
        assert_ne!(direct, relay1);
        assert_ne!(relay1, relay3);
        // …but the dedup key is identical across all relayed copies.
        let k = packet_dedup_key(&direct);
        assert_eq!(packet_dedup_key(&relay1), k, "1-hop relay must share key");
        assert_eq!(packet_dedup_key(&relay3), k, "3-hop relay must share key");
    }

    #[test]
    fn dedup_key_distinct_payloads_differ() {
        let a = packet_dedup_key(&grp_frame(0, b"\x6dmessage one"));
        let b = packet_dedup_key(&grp_frame(0, b"\x6dmessage two"));
        assert_ne!(a, b, "distinct payloads must yield distinct keys");
    }

    /// The payload TYPE is part of the key: same payload bytes under GRP_TXT vs
    /// TXT_MSG must not collide (matches MeshCore's hash input).
    #[test]
    fn dedup_key_includes_payload_type() {
        let payload = [0x01u8, 0x02, 0x03, 0x04];
        let mut grp = vec![0x15u8, 0x40]; // GRP_TXT
        grp.extend_from_slice(&payload);
        let mut dm = vec![0x09u8, 0x40]; // TXT_MSG
        dm.extend_from_slice(&payload);
        assert_ne!(
            packet_dedup_key(&grp),
            packet_dedup_key(&dm),
            "different payload types must hash differently",
        );
    }

    #[test]
    fn dedup_key_malformed_frame_falls_back() {
        // path_len 0x41 claims 1 two-byte hop (needs ≥ 4 bytes) but frame is 3 B.
        let runt = [0x15u8, 0x41, 0x00];
        assert!(packet_payload_view(&runt).is_none(), "runt frame is malformed");
        // Fallback still yields a stable key for byte-identical repeats.
        assert_eq!(packet_dedup_key(&runt), packet_dedup_key(&runt));
    }
}
