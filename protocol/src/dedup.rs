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
//!
//! # Transport-coded frames
//!
//! `packet_payload_view` locates the payload via
//! [`crate::frame::parse_rx_frame_header`] — the SAME shared parse
//! `firmware_core::rx_frame` uses ahead of `on_receive`'s own dispatch — so a
//! `ROUTE_TYPE_TRANSPORT_FLOOD`/`ROUTE_TYPE_TRANSPORT_DIRECT` frame's 4
//! transport-code bytes between the header and `path_len` are skipped here
//! too. Before this, this function hand-rolled the same `payload_off = 2 +
//! hop_count * hash_size` arithmetic `on_receive` used to (and PR #170 fixed
//! there): correct for `ROUTE_TYPE_FLOOD`/`DIRECT`, but wrong for a
//! transport-coded frame, where it read a transport-code byte as `path_len`
//! and mis-sliced the payload — so every relay copy of ONE logical
//! transport-coded packet hashed to a DIFFERENT dedup key, defeating
//! duplicate suppression entirely for that route-type class. This function
//! is called on EVERY received frame (`DuplicateFilter`, ahead of the
//! allowlist gate), so the two call sites parsing the header differently was
//! a second, independent instance of the identical defect: any wire event
//! reachable through more than one parse site needs the SAME parse
//! consulted at each one, not a second hand-rolled copy that can drift.

use crate::crypto::sha256_2;
use crate::frame::parse_rx_frame_header;

/// Split a wire frame into `(payload_type, payload)`, skipping the 1-byte
/// header, the optional 4-byte transport-code field, and the variable path
/// field (`hash_size × hop_count` bytes) — see module doc.
///
/// `payload_type = (header >> 2) & 0x0F`.
///
/// Returns `None` when the frame is malformed: shorter than its declared
/// path, or the `path_len` byte itself is invalid (reserved `hash_size`, or
/// an oversize path) — [`parse_rx_frame_header`]'s error cases all fall back
/// to hashing the whole frame here, same as before this shared the parse.
pub fn packet_payload_view(frame: &[u8]) -> Option<(u8, &[u8])> {
    let header = parse_rx_frame_header(frame).ok()?;
    Some((
        (header.header_byte >> 2) & 0x0F,
        &frame[header.payload_off..],
    ))
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

    /// Build a `ROUTE_TYPE_TRANSPORT_FLOOD` frame: `header | transport_codes(4)
    /// | path_len | path | payload`, with `hops` 2-byte path entries appended.
    fn transport_flood_frame(hops: u8, payload: &[u8]) -> Vec<u8> {
        // header = GRP_TXT(0x05)<<2 | TRANSPORT_FLOOD(0x00) = 0x14
        let mut f = vec![0x14u8, 0xDE, 0xAD, 0xBE, 0xEF]; // transport codes
        f.push(0x40 | hops); // path_len: 2-byte hash, `hops` hops
        for h in 0..hops {
            f.push(0xA0 + h);
            f.push(0xB0 + h);
        }
        f.extend_from_slice(payload);
        f
    }

    /// REGRESSION (GAP 1, second site of PR #170's on_receive fix): a
    /// transport-coded (`ROUTE_TYPE_TRANSPORT_FLOOD`/`TRANSPORT_DIRECT`)
    /// frame has 4 transport-code bytes between the header and `path_len`.
    /// Before this fix, `packet_payload_view` hand-rolled `payload_off = 2 +
    /// hop_count * hash_size` with no transport-code branch, so it read the
    /// first transport-code byte as `path_len` and mis-sliced the frame —
    /// every relayed copy of ONE logical transport-coded packet hashed to a
    /// DIFFERENT dedup key, defeating duplicate suppression. The ordinary
    /// (non-transport-coded) flood control case
    /// (`dedup_key_invariant_under_path_mutation`) already collapsed
    /// correctly; this proves the transport-coded route types now do too.
    #[test]
    fn dedup_key_invariant_under_path_mutation_transport_coded() {
        let payload = b"\x6dchannel hello";
        let direct = transport_flood_frame(0, payload);
        let relay1 = transport_flood_frame(1, payload);
        let relay3 = transport_flood_frame(3, payload);

        assert_ne!(direct, relay1);
        assert_ne!(relay1, relay3);

        let k = packet_dedup_key(&direct);
        assert_eq!(
            packet_dedup_key(&relay1),
            k,
            "1-hop relay of a transport-coded frame must share key"
        );
        assert_eq!(
            packet_dedup_key(&relay3),
            k,
            "3-hop relay of a transport-coded frame must share key"
        );
        // Sanity: the payload view actually lands past the transport codes,
        // not on them (payload starts with 0x6d, the channel-text prefix).
        let (_, view) = packet_payload_view(&direct).unwrap();
        assert_eq!(view, payload);
    }

    #[test]
    fn dedup_key_malformed_frame_falls_back() {
        // path_len 0x41 claims 1 two-byte hop (needs ≥ 4 bytes) but frame is 3 B.
        let runt = [0x15u8, 0x41, 0x00];
        assert!(
            packet_payload_view(&runt).is_none(),
            "runt frame is malformed"
        );
        // Fallback still yields a stable key for byte-identical repeats.
        assert_eq!(packet_dedup_key(&runt), packet_dedup_key(&runt));
    }
}
