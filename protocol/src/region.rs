// SPDX-License-Identifier: GPL-3.0-only
//! MeshCore region scoping (transport codes) — outbound wire helpers.
//!
//! Source reference: MeshCore `companion-v1.18.0` (`81dbae5`),
//! `src/helpers/RegionMap.cpp` and `src/helpers/TransportKeyStore.cpp`, plus
//! the companion/room send sites in `examples/companion_radio/MyMesh.cpp`.
//! The receive side already lives in [`crate::frame`] — this module is the
//! *writer* mirror of the transport-code branch that parser implements.
//!
//! # Scoped flood frame layout
//!
//! ```text
//! [header=(ptype<<2)|route] [code0 lo,hi] [code1 lo,hi] [path_len] [path] [payload]
//! ```
//!
//! The two 16-bit codes are present **only** for
//! `ROUTE_TYPE_TRANSPORT_FLOOD` (`0x00`) and `ROUTE_TYPE_TRANSPORT_DIRECT`
//! (`0x03`). Every other route type (`FLOOD`, `DIRECT`) stays unsuffixed, so
//! an unconfigured device's emissions remain byte-identical to the existing
//! unscoped path ([`write_unscoped_flood_frame`]).
//!
//! # Codes
//!
//! `code0 = HMAC-SHA256(region_key, payload_type_byte || payload)[0:2]`,
//! interpreted as a little-endian `u16` and remapped so the reserved values
//! `0x0000` and `0xFFFF` never appear on the wire (`0x0000 -> 0x0001`,
//! `0xFFFF -> 0xFFFE`; `TransportKeyStore.cpp` `calcTransportCode`). The MAC
//! is keyed on the **payload**, not the frame identity, and must be computed
//! once per distinct payload: never recompute per retry or per hop.
//!
//! `code1` is `0` at every upstream send site (`MyMesh.cpp`'s
//! `sendFloodScoped` companion/room paths: `codes[1] = 0`). It is a reserved
//! "home/return region" slot, not a hop index; v1 emits `0`.
//!
//! # Region keys and names
//!
//! A public region's key is `SHA256("#" + bare_name)[0:16]`
//! (`RegionMap::getTransportKeysFor` prepends `#` for a bare name;
//! `TransportKeyStore::getAutoKeyFor` hashes the canonical name). A name
//! supplied with a leading `#` keeps it.
//!
//! Name rules mirror upstream `RegionMap::is_name_char` and the 31-byte
//! `RegionEntry::name` array: at most [`REGION_NAME_MAX`] bytes; every byte
//! must be `-`, `$`, `#`, an ASCII digit, or `>= 'A'` (which, as upstream's
//! comment notes, also admits UTF-8 continuation bytes). A **leading** `$`
//! marks a private region. Upstream's private keystore is unimplemented
//! (`loadKeysFor` returns no keys), so v1 rejects it loudly rather than
//! silently provisioning a region that can never derive a key.
//!
//! # Why `{0, 0}` is forbidden
//!
//! Upstream treats transport-code `code0 == 0` as the share/"send to
//! nowhere" marker: `RegionMap::findMatch` short-circuits
//! `transport_codes[0] == 0` straight to the wildcard region, ignoring its
//! `DENY` flags. Encoding "no region" as `TRANSPORT_FLOOD{0,0}` would
//! therefore look scoped while actually broadcasting to every region — the
//! exact silent-wrong-scope failure this feature exists to prevent. "No
//! region configured" must emit plain [`RouteType::Flood`] instead.

use crate::crypto::{hmac_sha256_2_parts, sha256};
use crate::header::{Header, PathLen};
use crate::{PayloadType, RouteType};

/// Maximum region-name length in bytes (upstream `RegionEntry::name[31]`).
pub const REGION_NAME_MAX: usize = 31;

/// Error from region-name validation or key derivation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionNameError {
    /// A region name may not be empty (an empty provisioning payload is the
    /// explicit "clear region" command, not a name).
    Empty,
    /// Name is longer than [`REGION_NAME_MAX`] bytes.
    TooLong { len: usize },
    /// A leading `$` marks an upstream *private* region, whose hardware
    /// keystore is unimplemented upstream. Rejected for v1 rather than
    /// provisioned as a public region under a private-looking name.
    PrivateUnsupported,
    /// A byte outside upstream `is_name_char`'s accepted set.
    InvalidByte { index: usize, byte: u8 },
}

/// `true` if `byte` is accepted by upstream `RegionMap::is_name_char`.
///
/// Kept byte-for-byte equivalent to the upstream predicate rather than
/// tightened locally: `>= 'A'` deliberately admits lowercase ASCII and any
/// byte `>= 0x80` (upstream's "accented characters"), and `_`, `[`, `]`,
/// `~` come along with that range.
pub fn is_region_name_byte(byte: u8) -> bool {
    byte == b'-' || byte == b'$' || byte == b'#' || byte.is_ascii_digit() || byte >= b'A'
}

/// Validate a user-supplied region name (leading `#` optional).
///
/// Does **not** canonicalize; see [`canonical_region_name`]. Empty input is
/// rejected here because at the provisioning layer emptiness means "clear",
/// which is handled by the frame encoder rather than by naming.
pub fn validate_region_name(name: &[u8]) -> Result<(), RegionNameError> {
    if name.is_empty() {
        return Err(RegionNameError::Empty);
    }
    if name.len() > REGION_NAME_MAX {
        return Err(RegionNameError::TooLong { len: name.len() });
    }
    if name[0] == b'$' {
        return Err(RegionNameError::PrivateUnsupported);
    }
    for (index, &byte) in name.iter().enumerate() {
        if !is_region_name_byte(byte) {
            return Err(RegionNameError::InvalidByte { index, byte });
        }
    }
    Ok(())
}

/// Write the canonical public-region name into `out`, returning its length.
///
/// A name with a leading `#` is kept as-is; a bare name gets one `#`
/// prepended, mirroring `RegionMap::getTransportKeysFor`'s implicit
/// hashtag-region branch. `out` must be [`REGION_NAME_MAX`] + 1 bytes.
pub fn canonical_region_name(
    name: &[u8],
    out: &mut [u8; REGION_NAME_MAX + 1],
) -> Result<usize, RegionNameError> {
    validate_region_name(name)?;
    if name[0] == b'#' {
        out[..name.len()].copy_from_slice(name);
        Ok(name.len())
    } else {
        out[0] = b'#';
        out[1..1 + name.len()].copy_from_slice(name);
        Ok(1 + name.len())
    }
}

/// Derive a public region's 16-byte transport key:
/// `SHA256("#" + bare_name)[0:16]`.
///
/// The name may be supplied with or without the leading `#`; both forms
/// produce the same key. A private (`$`-prefixed) or malformed name is
/// rejected — callers must surface that as a configuration error, never fall
/// back to unscoped emission (fail closed).
pub fn region_key(name: &[u8]) -> Result<[u8; 16], RegionNameError> {
    let mut canonical = [0u8; REGION_NAME_MAX + 1];
    let len = canonical_region_name(name, &mut canonical)?;
    let digest = sha256(&canonical[..len]);
    let mut key = [0u8; 16];
    key.copy_from_slice(&digest[..16]);
    Ok(key)
}

/// Apply upstream's reserved-code remap to a raw 2-byte HMAC prefix.
pub fn remap_transport_code(raw: u16) -> u16 {
    match raw {
        0x0000 => 0x0001,
        0xFFFF => 0xFFFE,
        other => other,
    }
}

/// Compute `code0` for a payload: HMAC-SHA256(key, type-byte || payload)[0:2]
/// as a little-endian `u16`, with the reserved-value remap applied.
///
/// `payload` must be the **wire payload** (the bytes after `path_len`/path)
/// and `payload_type_byte` is the raw header payload-type nibble (e.g.
/// `0x02` for `TXT_MSG`), not the packed header byte.
pub fn transport_code(key: &[u8; 16], payload_type_byte: u8, payload: &[u8]) -> u16 {
    let mac = hmac_sha256_2_parts(key, &[payload_type_byte], payload);
    remap_transport_code(u16::from_le_bytes(mac))
}

/// Build the exact four transport-code bytes for a scoped flood frame:
/// `[code0 lo, code0 hi, 0, 0]`.
pub fn scoped_transport_codes(key: &[u8; 16], payload_type_byte: u8, payload: &[u8]) -> [u8; 4] {
    let code0 = transport_code(key, payload_type_byte, payload);
    let code1 = 0u16;
    let mut out = [0u8; 4];
    out[..2].copy_from_slice(&code0.to_le_bytes());
    out[2..].copy_from_slice(&code1.to_le_bytes());
    out
}

/// Write a scoped flood frame:
/// `[header] [code0] [0,0] [path_len] [path] [payload]`.
///
/// Returns the total bytes written. The caller supplies the path bytes
/// already encoded by `MeshCore`'s path rules; `path.len()` must equal
/// `path_len.hop_count() * path_len.hash_size()` (checked in debug builds).
pub fn write_scoped_flood_frame(
    out: &mut [u8],
    payload_type: PayloadType,
    key: &[u8; 16],
    path_len: PathLen,
    path: &[u8],
    payload: &[u8],
) -> usize {
    let path_bytes = path_len.hop_count() as usize * path_len.hash_size() as usize;
    debug_assert_eq!(path.len(), path_bytes, "path slice does not match path_len");
    let header = Header::new(RouteType::TransportFlood, payload_type).0;
    let codes = scoped_transport_codes(key, payload_type as u8, payload);
    let mut at = 0;
    out[at] = header;
    at += 1;
    out[at..at + 4].copy_from_slice(&codes);
    at += 4;
    out[at] = path_len.0;
    at += 1;
    out[at..at + path_bytes].copy_from_slice(&path[..path_bytes]);
    at += path_bytes;
    out[at..at + payload.len()].copy_from_slice(payload);
    at + payload.len()
}

/// Write an unscoped flood frame:
/// [`header`] [`path_len`] [`path`] [`payload`].
///
/// This is the exact shape MeshCadet emits today and the required emission
/// when no region is configured. No transport-code bytes are present.
pub fn write_unscoped_flood_frame(
    out: &mut [u8],
    payload_type: PayloadType,
    path_len: PathLen,
    path: &[u8],
    payload: &[u8],
) -> usize {
    let path_bytes = path_len.hop_count() as usize * path_len.hash_size() as usize;
    debug_assert_eq!(path.len(), path_bytes, "path slice does not match path_len");
    let header = Header::new(RouteType::Flood, payload_type).0;
    out[0] = header;
    out[1] = path_len.0;
    out[2..2 + path_bytes].copy_from_slice(&path[..path_bytes]);
    let payload_off = 2 + path_bytes;
    out[payload_off..payload_off + payload.len()].copy_from_slice(payload);
    2 + path_bytes + payload.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::parse_rx_frame_header;

    // ── Name validation matrix ───────────────────────────────────────────

    #[test]
    fn accepts_upstream_name_chars_and_leading_hash() {
        for name in [
            &b"seattle"[..],
            b"#seattle",
            b"bay-area",
            b"zone-1",
            b"A",
            b"abc_DEF",
            b"~",
        ] {
            assert_eq!(validate_region_name(name), Ok(()), "{name:?}");
        }
    }

    #[test]
    fn accepts_utf8_multibyte_names_like_upstream() {
        // Every UTF-8 continuation/lead byte is >= 'A' (0x41), so upstream's
        // byte predicate accepts accented names ("accept ... accented
        // characters"). 31-byte ceiling still applies.
        let name = "Zürich".as_bytes();
        assert_eq!(validate_region_name(name), Ok(()));
    }

    #[test]
    fn rejects_empty_name() {
        assert_eq!(validate_region_name(b""), Err(RegionNameError::Empty));
    }

    #[test]
    fn rejects_names_longer_than_31_bytes() {
        let ok = [b'A'; REGION_NAME_MAX];
        assert_eq!(validate_region_name(&ok), Ok(()));
        let too_long = [b'A'; REGION_NAME_MAX + 1];
        assert_eq!(
            validate_region_name(&too_long),
            Err(RegionNameError::TooLong {
                len: REGION_NAME_MAX + 1
            })
        );
    }

    #[test]
    fn rejects_each_disallowed_byte_outside_upstream_set() {
        for &byte in b" .@:;/?\\*+()[]{}!\"'&%=\x00\x1f" {
            // Filter bytes that upstream actually accepts: only assert the
            // ones we mean to reject.
            if is_region_name_byte(byte) {
                continue;
            }
            let name = [b'A', byte];
            assert_eq!(
                validate_region_name(&name),
                Err(RegionNameError::InvalidByte { index: 1, byte })
            );
        }
    }

    #[test]
    fn rejects_leading_dollar_private_region() {
        assert_eq!(
            validate_region_name(b"$secret"),
            Err(RegionNameError::PrivateUnsupported)
        );
        // Non-leading '$' is an ordinary allowed name byte upstream.
        assert_eq!(validate_region_name(b"a$b"), Ok(()));
    }

    // ── Key derivation known-answer ──────────────────────────────────────
    //
    // Independently derived with Python:
    //   hashlib.sha256(b"#seattle").hexdigest()[:32]
    //   == "ef627a9bbbb549347fdb76bf0cd3bc14"
    // This pins the canonical "#"+bare concatenation and the [0:16] truncation.

    #[test]
    fn region_key_known_answer_bare_and_hash_forms_match() {
        let expected = [
            0xef, 0x62, 0x7a, 0x9b, 0xbb, 0xb5, 0x49, 0x34, 0x7f, 0xdb, 0x76, 0xbf, 0x0c, 0xd3,
            0xbc, 0x14,
        ];
        assert_eq!(region_key(b"seattle"), Ok(expected));
        assert_eq!(region_key(b"#seattle"), Ok(expected));
        // A different name must not collide with the above in this fixture.
        assert_ne!(region_key(b"bay-area"), Ok(expected));
    }

    #[test]
    fn canonical_name_prepends_hash_only_when_bare() {
        let mut out = [0u8; REGION_NAME_MAX + 1];
        assert_eq!(canonical_region_name(b"seattle", &mut out), Ok(8));
        assert_eq!(&out[..8], b"#seattle");
        assert_eq!(canonical_region_name(b"#seattle", &mut out), Ok(8));
        assert_eq!(&out[..8], b"#seattle");
        // Longest bare name still fits: '#' + 31 bytes.
        let longest = [b'A'; REGION_NAME_MAX];
        assert_eq!(
            canonical_region_name(&longest, &mut out),
            Ok(REGION_NAME_MAX + 1)
        );
        assert_eq!(out[0], b'#');
    }

    // ── Transport-code known-answer + remap ─────────────────────────────

    #[test]
    fn transport_code_known_answer() {
        // Independently derived with Python:
        //   key = hashlib.sha256(b"#seattle").digest()[:16]
        //   hmac.new(key, bytes([0x02]) + b"hello", sha256).digest()[:2]
        //   == b"\xf5\x7c" -> little-endian u16 0x7cf5.
        let key = region_key(b"seattle").unwrap();
        assert_eq!(transport_code(&key, 0x02, b"hello"), 0x7cf5);
        assert_eq!(
            scoped_transport_codes(&key, 0x02, b"hello"),
            [0xf5, 0x7c, 0x00, 0x00]
        );
    }

    #[test]
    fn transport_code_changes_with_payload_and_type() {
        let key = region_key(b"seattle").unwrap();
        let a = transport_code(&key, 0x02, b"hello");
        let b = transport_code(&key, 0x02, b"hello!");
        let c = transport_code(&key, 0x04, b"hello");
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn reserved_codes_are_remapped() {
        assert_eq!(remap_transport_code(0x0000), 0x0001);
        assert_eq!(remap_transport_code(0xFFFF), 0xFFFE);
        assert_eq!(remap_transport_code(0x0001), 0x0001);
        assert_eq!(remap_transport_code(0xFFFE), 0xFFFE);
        assert_eq!(remap_transport_code(0x1234), 0x1234);
    }

    // ── Frame byte layout ────────────────────────────────────────────────

    #[test]
    fn scoped_frame_layout_matches_rx_parser() {
        let key = region_key(b"seattle").unwrap();
        let payload = b"hello";
        let mut frame = [0u8; 64];
        let n = write_scoped_flood_frame(
            &mut frame,
            PayloadType::TxtMsg,
            &key,
            PathLen(0),
            &[],
            payload,
        );
        // header + 4 codes + path_len + payload
        assert_eq!(n, 1 + 4 + 1 + payload.len());
        assert_eq!(frame[0], (PayloadType::TxtMsg as u8) << 2);
        assert_eq!(&frame[1..5], &[0xf5, 0x7c, 0x00, 0x00]);
        assert_eq!(frame[5], 0x00);
        assert_eq!(&frame[6..n], payload);

        let parsed = parse_rx_frame_header(&frame[..n]).expect("RX parser accepts writer output");
        assert!(parsed.has_transport_codes);
        assert_eq!(parsed.route_type, Some(RouteType::TransportFlood));
        assert_eq!(parsed.payload_off, 6);
        assert_eq!(&frame[parsed.payload_off..n], payload);
    }

    #[test]
    fn scoped_frame_with_two_hop_path_roundtrips_through_rx_parser() {
        let key = region_key(b"bay-area").unwrap();
        let payload = b"xy";
        let path = [0x11, 0x22, 0x33, 0x44];
        let mut frame = [0u8; 64];
        let n = write_scoped_flood_frame(
            &mut frame,
            PayloadType::GrpTxt,
            &key,
            PathLen(0x42), // 2-byte hashes, 2 hops
            &path,
            payload,
        );
        assert_eq!(n, 1 + 4 + 1 + path.len() + payload.len());
        let parsed = parse_rx_frame_header(&frame[..n]).unwrap();
        assert_eq!(parsed.hash_size, 2);
        assert_eq!(parsed.hop_count, 2);
        assert_eq!(parsed.payload_off, 10);
        assert_eq!(&frame[parsed.payload_off..n], payload);
    }

    #[test]
    fn unconfigured_emission_is_plain_flood_with_no_codes() {
        let payload = b"hello";
        let mut frame = [0u8; 32];
        let n =
            write_unscoped_flood_frame(&mut frame, PayloadType::TxtMsg, PathLen(0), &[], payload);
        // Header route bits are FLOOD (0x01), NOT TRANSPORT_FLOOD (0x00).
        assert_eq!(frame[0], ((PayloadType::TxtMsg as u8) << 2) | 0x01);
        assert_eq!(n, 1 + 1 + payload.len());
        assert_eq!(frame[1], 0x00); // path_len at offset 1, not a code byte
        assert_eq!(&frame[2..n], payload);
        let parsed = parse_rx_frame_header(&frame[..n]).unwrap();
        assert!(!parsed.has_transport_codes);
        assert_eq!(parsed.payload_off, 2);
    }
}
