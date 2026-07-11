// SPDX-License-Identifier: GPL-3.0-only
//! Header byte and path_len encoding.
//!
//! Spec §1–§2 (src/Packet.h @ dee3e26a).

use crate::{constants, PayloadType, RouteType};

/// The 1-byte MeshCore packet header.
///
/// Bit layout:
/// ```text
/// Bit:  7  6  5  4  3  2  1  0
///       [VER  ][  PAYLOAD TYPE  ][RT]
/// Mask: 0xC0       0x3C           0x03
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header(pub u8);

impl Header {
    /// Construct a version-0 header from route type and payload type.
    pub fn new(route: RouteType, payload_type: PayloadType) -> Self {
        Header((payload_type as u8) << constants::PH_TYPE_SHIFT | (route as u8))
    }

    /// Route type from bits [1:0].
    pub fn route_type(self) -> Option<RouteType> {
        match self.0 & constants::PH_ROUTE_MASK {
            0x00 => Some(RouteType::TransportFlood),
            0x01 => Some(RouteType::Flood),
            0x02 => Some(RouteType::Direct),
            0x03 => Some(RouteType::TransportDirect),
            _ => None,
        }
    }

    /// Payload type from bits [5:2].
    pub fn payload_type(self) -> Option<PayloadType> {
        match (self.0 >> constants::PH_TYPE_SHIFT) & 0x0F {
            0x00 => Some(PayloadType::Req),
            0x01 => Some(PayloadType::Response),
            0x02 => Some(PayloadType::TxtMsg),
            0x03 => Some(PayloadType::Ack),
            0x04 => Some(PayloadType::Advert),
            0x05 => Some(PayloadType::GrpTxt),
            0x06 => Some(PayloadType::GrpData),
            0x07 => Some(PayloadType::AnonReq),
            0x08 => Some(PayloadType::Path),
            0x09 => Some(PayloadType::Trace),
            0x0A => Some(PayloadType::Multipart),
            0x0B => Some(PayloadType::Control),
            0x0F => Some(PayloadType::RawCustom),
            _ => None,
        }
    }

    /// Payload version from bits [7:6].  Only `0` (PAYLOAD_VER_1) is deployed.
    pub fn version(self) -> u8 {
        (self.0 >> constants::PH_VER_SHIFT) & 0x03
    }
}

/// The `path_len` byte: bits[7:6] = (hash_size - 1), bits[5:0] = hop_count.
///
/// Accessors mirror `src/Packet.h`:
/// ```text
/// getPathHashSize()  = (path_len >> 6) + 1
/// getPathHashCount() =  path_len & 63
/// getPathByteLen()   =  getPathHashCount() * getPathHashSize()
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathLen(pub u8);

impl PathLen {
    /// Construct from `hash_size` in [1, 4] and `hop_count` in [0, 63].
    /// Returns `None` if either argument is out of range.
    pub fn new(hash_size: u8, hop_count: u8) -> Option<Self> {
        if !(1..=4).contains(&hash_size) || hop_count > 63 {
            return None;
        }
        Some(PathLen(((hash_size - 1) << 6) | hop_count))
    }

    /// Per-hop hash size in bytes: `(raw >> 6) + 1`.
    pub fn hash_size(self) -> u8 {
        (self.0 >> 6) + 1
    }

    /// Hop count: `raw & 63`.
    pub fn hop_count(self) -> u8 {
        self.0 & 63
    }

    /// Total path field byte length: `hop_count x hash_size`.
    pub fn path_byte_len(self) -> u8 {
        self.hop_count() * self.hash_size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Header ──────────────────────────────────────────────────────────────

    #[test]
    fn header_flood_txt_msg_is_0x09() {
        // Spec §8: outbound flood TXT_MSG header = (TXT_MSG<<2)|FLOOD = (2<<2)|1 = 0x09
        let h = Header::new(RouteType::Flood, PayloadType::TxtMsg);
        assert_eq!(h.0, 0x09);
    }

    #[test]
    fn header_direct_ack_is_0x0e() {
        // Spec §8: direct ACK header = (ACK<<2)|DIRECT = (3<<2)|2 = 0x0E
        let h = Header::new(RouteType::Direct, PayloadType::Ack);
        assert_eq!(h.0, 0x0E);
    }

    #[test]
    fn header_flood_ack_is_0x0d() {
        // (3<<2)|1 = 0x0D
        let h = Header::new(RouteType::Flood, PayloadType::Ack);
        assert_eq!(h.0, 0x0D);
    }

    #[test]
    fn header_roundtrip_all_types() {
        let cases = [
            (RouteType::Flood, PayloadType::TxtMsg),
            (RouteType::Flood, PayloadType::GrpTxt),
            (RouteType::Direct, PayloadType::Ack),
            (RouteType::Direct, PayloadType::Path),
            (RouteType::Flood, PayloadType::Advert),
            (RouteType::TransportFlood, PayloadType::AnonReq),
        ];
        for (rt, pt) in cases {
            let h = Header::new(rt, pt);
            assert_eq!(h.route_type(), Some(rt), "rt={:?} pt={:?}", rt, pt);
            assert_eq!(h.payload_type(), Some(pt), "rt={:?} pt={:?}", rt, pt);
            assert_eq!(h.version(), 0);
        }
    }

    // ── PathLen ─────────────────────────────────────────────────────────────

    #[test]
    fn path_len_0x42_known_answer() {
        // Known-answer (spec §2.2): 0x42 = 0b01_000010
        //   bits[7:6]=01 → hash_size = 2
        //   bits[5:0]=2  → hop_count = 2
        //   path_byte_len = 2 × 2 = 4
        let pl = PathLen(0x42);
        assert_eq!(pl.hash_size(), 2, "hash_size for 0x42");
        assert_eq!(pl.hop_count(), 2, "hop_count for 0x42");
        assert_eq!(pl.path_byte_len(), 4, "path_byte_len for 0x42");
    }

    #[test]
    fn path_len_constructor_matches_raw() {
        let pl = PathLen::new(2, 2).unwrap();
        assert_eq!(pl.0, 0x42, "new(2,2) should produce 0x42");
    }

    #[test]
    fn path_len_zero_hops_two_byte() {
        // 2-byte hash, 0 hops = 0x40
        let pl = PathLen::new(2, 0).unwrap();
        assert_eq!(pl.0, 0x40);
        assert_eq!(pl.path_byte_len(), 0);
    }

    #[test]
    fn path_len_spec_table_examples() {
        // All rows from spec §2.2
        assert_eq!(PathLen(0x00).hash_size(), 1);
        assert_eq!(PathLen(0x00).hop_count(), 0);
        assert_eq!(PathLen(0x00).path_byte_len(), 0);

        assert_eq!(PathLen(0x05).hash_size(), 1);
        assert_eq!(PathLen(0x05).hop_count(), 5);
        assert_eq!(PathLen(0x05).path_byte_len(), 5);

        assert_eq!(PathLen(0x45).hash_size(), 2);
        assert_eq!(PathLen(0x45).hop_count(), 5);
        assert_eq!(PathLen(0x45).path_byte_len(), 10);

        assert_eq!(PathLen(0x8A).hash_size(), 3);
        assert_eq!(PathLen(0x8A).hop_count(), 10);
        assert_eq!(PathLen(0x8A).path_byte_len(), 30);
    }

    #[test]
    fn path_len_new_rejects_bad_inputs() {
        assert!(PathLen::new(0, 0).is_none()); // hash_size=0 invalid
        assert!(PathLen::new(5, 0).is_none()); // hash_size=5 invalid
        assert!(PathLen::new(1, 64).is_none()); // hop_count=64 invalid
        assert!(PathLen::new(2, 32).is_some()); // max valid for 2-byte mode
    }
}
