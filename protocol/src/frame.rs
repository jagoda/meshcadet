// SPDX-License-Identifier: GPL-3.0-only
//! Shared RX frame-header parsing — locates the payload offset in a raw wire
//! frame, branching on route type so a transport-coded frame's extra header
//! bytes are skipped correctly.
//!
//! Source reference: `Packet::readFrom` / `writeTo` (`src/Packet.cpp` @
//! `companion-v1.17.1`, unchanged since `dee3e26a`). Wire framing:
//!
//! ```text
//! [header (1 B)] [transport_codes (4 B, optional)] [path_len (1 B)] [path (0-64 B)] [payload]
//! ```
//!
//! The transport-code bytes are present only when the header's route type is
//! `ROUTE_TYPE_TRANSPORT_FLOOD` (`0x00`) or `ROUTE_TYPE_TRANSPORT_DIRECT`
//! (`0x03`) — `Packet::hasTransportCodes()`. A naive parse that always
//! assumes `path_len` sits at byte offset 1 is correct for
//! `ROUTE_TYPE_FLOOD`/`DIRECT` (MeshCadet's own only-ever-emitted route
//! types) but wrong for a *received* transport-coded frame: `path_len`
//! actually sits at offset 5 there, so the naive parse reads the first
//! transport-code byte as `path_len`, producing a garbage path length and
//! misparsing the rest of the frame.
//!
//! # Why this lives in `protocol/`, not `firmware-core/`
//!
//! This parse is needed at two independent call sites that must agree
//! byte-for-byte: `firmware_core::rx_frame` (re-exports this module — see
//! its doc for the `firmware/src/main.rs::on_receive` history) and
//! `protocol::dedup::packet_payload_view` (dedup hashing must skip the SAME
//! bytes `on_receive` does, or a transport-coded frame's relayed copies hash
//! to different keys and duplicate suppression silently breaks — see
//! `dedup`'s module doc). `firmware-core` depends on `protocol`, never the
//! reverse, so the shared logic has to live on the `protocol` side of that
//! edge for `dedup.rs` to reach it without a layering violation.

use crate::header::{Header, PathLen};
use crate::RouteType;

/// Why a frame's header could not be parsed into a payload offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RxFrameError {
    /// Frame has zero bytes — no header byte to read.
    Empty,
    /// Frame is too short to contain the transport-code bytes its route type
    /// declares (if any) and the `path_len` byte that follows them.
    TooShortForPathLen { frame_len: usize, needed: usize },
    /// The `path_len` byte itself is malformed: reserved `hash_size == 4`, or
    /// `hop_count * hash_size` exceeds `protocol::constants::MAX_PATH_SIZE`
    /// (mirrors MeshCore's `Packet::isValidPathLen`).
    InvalidPathLen { path_len_byte: u8 },
    /// Frame is shorter than the path length it declares.
    TooShortForPath {
        frame_len: usize,
        payload_off: usize,
    },
}

/// A frame's header, parsed far enough to locate the payload slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RxFrameHeader {
    pub header_byte: u8,
    pub route_type: Option<RouteType>,
    pub has_transport_codes: bool,
    pub path_len_byte: u8,
    pub hash_size: u8,
    pub hop_count: u8,
    /// Byte offset into the original frame where the payload begins.
    pub payload_off: usize,
}

/// Parse `frame`'s header far enough to locate the payload, per MeshCore's
/// wire framing (see module doc). Branches on route type so a
/// `ROUTE_TYPE_TRANSPORT_FLOOD`/`ROUTE_TYPE_TRANSPORT_DIRECT` frame's 4
/// transport-code bytes are skipped correctly rather than misread as the
/// start of `path_len`. Also rejects a malformed `path_len` byte early via
/// [`PathLen::is_valid`].
pub fn parse_rx_frame_header(frame: &[u8]) -> Result<RxFrameHeader, RxFrameError> {
    let Some(&header_byte) = frame.first() else {
        return Err(RxFrameError::Empty);
    };
    let route_type = Header(header_byte).route_type();
    let has_transport_codes = matches!(
        route_type,
        Some(RouteType::TransportFlood) | Some(RouteType::TransportDirect)
    );
    let path_len_idx = 1 + if has_transport_codes { 4 } else { 0 };
    if frame.len() <= path_len_idx {
        return Err(RxFrameError::TooShortForPathLen {
            frame_len: frame.len(),
            needed: path_len_idx + 1,
        });
    }
    let path_len_byte = frame[path_len_idx];
    let path_len = PathLen(path_len_byte);
    if !path_len.is_valid() {
        return Err(RxFrameError::InvalidPathLen { path_len_byte });
    }
    let hash_size = path_len.hash_size();
    let hop_count = path_len.hop_count();
    let path_bytes = hop_count as usize * hash_size as usize;
    let payload_off = path_len_idx + 1 + path_bytes;
    if frame.len() < payload_off {
        return Err(RxFrameError::TooShortForPath {
            frame_len: frame.len(),
            payload_off,
        });
    }
    Ok(RxFrameHeader {
        header_byte,
        route_type,
        has_transport_codes,
        path_len_byte,
        hash_size,
        hop_count,
        payload_off,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PayloadType;

    fn flood_txt_frame() -> Vec<u8> {
        // header=(TXT_MSG<<2)|FLOOD, path_len=0x00 (zero hops), 3 payload bytes.
        let header = Header::new(RouteType::Flood, PayloadType::TxtMsg).0;
        vec![header, 0x00, 0xAA, 0xBB, 0xCC]
    }

    #[test]
    fn empty_frame_is_rejected() {
        assert_eq!(parse_rx_frame_header(&[]), Err(RxFrameError::Empty));
    }

    #[test]
    fn ordinary_flood_frame_parses_unchanged() {
        let frame = flood_txt_frame();
        let parsed = parse_rx_frame_header(&frame).expect("should parse");
        assert!(!parsed.has_transport_codes);
        assert_eq!(parsed.route_type, Some(RouteType::Flood));
        assert_eq!(parsed.payload_off, 2);
        assert_eq!(&frame[parsed.payload_off..], &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn direct_frame_with_path_parses_unchanged() {
        // header=(TXT_MSG<<2)|DIRECT, path_len=0x42 (2-byte hash, 2 hops) =>
        // 4 path bytes, then payload.
        let header = Header::new(RouteType::Direct, PayloadType::TxtMsg).0;
        let frame = vec![header, 0x42, 0x11, 0x22, 0x33, 0x44, 0xAA, 0xBB];
        let parsed = parse_rx_frame_header(&frame).expect("should parse");
        assert!(!parsed.has_transport_codes);
        assert_eq!(parsed.hash_size, 2);
        assert_eq!(parsed.hop_count, 2);
        assert_eq!(parsed.payload_off, 6);
        assert_eq!(&frame[parsed.payload_off..], &[0xAA, 0xBB]);
    }

    #[test]
    fn transport_flood_frame_skips_4_transport_code_bytes() {
        // header=(TXT_MSG<<2)|TRANSPORT_FLOOD (route bits 0b00).
        let header = ((PayloadType::TxtMsg as u8) << 2) | (RouteType::TransportFlood as u8);
        // [header][transport_codes x4][path_len=0x00][payload...]
        let frame = vec![header, 0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0xAA, 0xBB];
        let parsed = parse_rx_frame_header(&frame).expect("should parse");
        assert!(parsed.has_transport_codes);
        assert_eq!(parsed.route_type, Some(RouteType::TransportFlood));
        assert_eq!(parsed.path_len_byte, 0x00);
        // Before the fix, payload_off was computed as if path_len sat at
        // offset 1 (reading transport-code byte 0xDE as path_len instead).
        assert_eq!(parsed.payload_off, 6);
        assert_eq!(&frame[parsed.payload_off..], &[0xAA, 0xBB]);
    }

    #[test]
    fn transport_direct_frame_with_path_skips_transport_codes_and_reads_path() {
        // header=(ACK<<2)|TRANSPORT_DIRECT (route bits 0b11).
        let header = ((PayloadType::Ack as u8) << 2) | (RouteType::TransportDirect as u8);
        // [header][transport_codes x4][path_len=0x42 (2-byte hash, 2 hops)][path x4][payload]
        let frame = vec![
            header, 0x01, 0x02, 0x03, 0x04, // transport codes
            0x42, // path_len
            0x11, 0x22, 0x33, 0x44, // path (2 hops x 2-byte hash)
            0xAA, 0xBB, // payload
        ];
        let parsed = parse_rx_frame_header(&frame).expect("should parse");
        assert!(parsed.has_transport_codes);
        assert_eq!(parsed.route_type, Some(RouteType::TransportDirect));
        assert_eq!(parsed.hash_size, 2);
        assert_eq!(parsed.hop_count, 2);
        assert_eq!(parsed.payload_off, 10);
        assert_eq!(&frame[parsed.payload_off..], &[0xAA, 0xBB]);
    }

    #[test]
    fn transport_frame_too_short_for_transport_codes_and_path_len_is_rejected() {
        let header = ((PayloadType::TxtMsg as u8) << 2) | (RouteType::TransportFlood as u8);
        // Only 4 bytes total: header + 3 of the 4 transport-code bytes — no
        // room for the path_len byte at all.
        let frame = vec![header, 0xDE, 0xAD, 0xBE];
        assert_eq!(
            parse_rx_frame_header(&frame),
            Err(RxFrameError::TooShortForPathLen {
                frame_len: 4,
                needed: 6,
            })
        );
    }

    #[test]
    fn invalid_path_len_reserved_hash_size_is_rejected() {
        let header = Header::new(RouteType::Flood, PayloadType::TxtMsg).0;
        // path_len bits[7:6] = 0b11 -> hash_size = 4, reserved.
        let frame = vec![header, 0xC0, 0xAA];
        assert_eq!(
            parse_rx_frame_header(&frame),
            Err(RxFrameError::InvalidPathLen {
                path_len_byte: 0xC0
            })
        );
    }

    #[test]
    fn invalid_path_len_oversize_path_is_rejected() {
        let header = Header::new(RouteType::Direct, PayloadType::TxtMsg).0;
        // hash_size=2, hop_count=63 -> 126 bytes, exceeds MAX_PATH_SIZE (64).
        let frame = vec![header, 0x7F, 0xAA];
        assert_eq!(
            parse_rx_frame_header(&frame),
            Err(RxFrameError::InvalidPathLen {
                path_len_byte: 0x7F
            })
        );
    }

    #[test]
    fn frame_shorter_than_declared_path_is_rejected() {
        let header = Header::new(RouteType::Direct, PayloadType::TxtMsg).0;
        // path_len=0x42 (2-byte hash, 2 hops) needs 4 path bytes; only 2 given.
        let frame = vec![header, 0x42, 0x11, 0x22];
        assert_eq!(
            parse_rx_frame_header(&frame),
            Err(RxFrameError::TooShortForPath {
                frame_len: 4,
                payload_off: 6,
            })
        );
    }
}
