// SPDX-License-Identifier: GPL-3.0-only
//! Self-advert "biz card": a MeshCore-wire-compatible signed ADVERT packet
//! encoding this node's identity, ready to hand to a companion app or peer
//! node exactly as MeshCore's own `card` REPL command does.
//!
//! Source reference: `src/Mesh.cpp::createSelfAdvert()` (sign) /
//! `Mesh.cpp::onRecvPacket()` (verify) / `src/helpers/BaseChatMesh.cpp`
//! (`onAdvertRecv`, `hasName()` gate) @ `07a3ca9e`.
//!
//! # Wire format
//!
//! ```text
//! [header(1)=0x11] [path_len(1)=0x00] [pubkey(32)] [timestamp(4 LE)] [signature(64)] [appdata(1..32)]
//! ```
//!
//! - `header = 0x11`: `VER 0 | PAYLOAD_TYPE_ADVERT(0x04)<<2 | ROUTE_TYPE_FLOOD(0x01)`.
//!   Deliberately FLOOD, not `TRANSPORT_FLOOD` (`0x10`, what the upstream `card`
//!   example actually emits): `Packet::readFrom()` only consumes the 4-byte
//!   transport-code prefix when the route type calls for it, and those bytes
//!   are never initialized in a freshly-built `Packet` — emitting
//!   `TRANSPORT_FLOOD` here would round-trip stale memory through the wire.
//!   `BaseChatMesh::onAdvertRecv()` independently forces the saved copy of an
//!   advert to `ROUTE_TYPE_FLOOD` before it can be re-shared, confirming FLOOD
//!   is the correct route type for a standalone card.
//! - `signature` is Ed25519 over `pubkey || timestamp_le || appdata` **only**
//!   — the header and path_len bytes are never signed (`Mesh.cpp::sign` at
//!   packet-build time, `Mesh.cpp::onRecvPacket` at verify time both build the
//!   signed message from those three fields alone).
//! - `appdata = [flags(1)] [name(variable)]`, `flags = 0x81`
//!   (`ADV_TYPE_CHAT(0x01) | ADV_NAME_MASK(0x80)`, from
//!   `src/helpers/AdvertDataHelpers.h`). No lat/lon: this project has no GPS
//!   telemetry, and omitting the field is a deliberate privacy stance, not an
//!   oversight.
//! - A name is mandatory: `BaseChatMesh::onAdvertRecv()` drops any advert that
//!   fails `AdvertDataParser::hasName()`. [`build_self_advert_card`] asserts
//!   on an empty name rather than silently emitting a card no peer will keep;
//!   callers with no persisted name must pass a pub_hash-derived label.
//!
//! # no_std / no heap
//!
//! Every function here writes into a caller-supplied buffer; there is no
//! allocation.

use crate::identity::Identity;
use crate::{constants, header::Header, PayloadType, RouteType};

// ── Card layout ─────────────────────────────────────────────────────────────

const OFFSET_PUBKEY: usize = 2;
const OFFSET_TIMESTAMP: usize = OFFSET_PUBKEY + constants::PUB_KEY_SIZE; // 34
const OFFSET_SIGNATURE: usize = OFFSET_TIMESTAMP + 4; // 38
const OFFSET_APPDATA: usize = OFFSET_SIGNATURE + constants::SIGNATURE_SIZE; // 102

/// Maximum length of an encoded self-advert card:
/// `header(1) + path_len(1) + pubkey(32) + timestamp(4) + signature(64) + appdata(32)`.
pub const MAX_ADVERT_CARD_LEN: usize = OFFSET_APPDATA + constants::MAX_ADVERT_DATA_SIZE;

/// Maximum name length embeddable in appdata: `MAX_ADVERT_DATA_SIZE(32) - flags(1)`.
pub const MAX_ADVERT_NAME_LEN: usize = constants::MAX_ADVERT_DATA_SIZE - 1;

/// Advert app-data flags for a named chat node with no lat/lon:
/// `ADV_TYPE_CHAT(0x01) | ADV_NAME_MASK(0x80)` (`src/helpers/AdvertDataHelpers.h`).
pub const ADVERT_APPDATA_FLAGS: u8 = 0x81;

/// Find the largest `end <= max_bytes` that lands on a UTF-8 character
/// boundary of `s`, so truncation never splits a codepoint.
fn floor_char_boundary(s: &str, max_bytes: usize) -> usize {
    if max_bytes >= s.len() {
        return s.len();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Build this identity's self-advert "biz card" into `out`. Returns the
/// number of bytes written (`<=` [`MAX_ADVERT_CARD_LEN`]).
///
/// `name` is truncated to [`MAX_ADVERT_NAME_LEN`] bytes on a UTF-8 character
/// boundary if longer. `out` must be at least [`MAX_ADVERT_CARD_LEN`] bytes.
///
/// # Panics
///
/// Panics if `name` is empty — every peer's `onAdvertRecv()` drops a
/// nameless advert (see module docs), so building one is always a caller
/// bug; callers with no persisted device name must synthesize a
/// pub_hash-derived label before calling this.
pub fn build_self_advert_card(
    identity: &Identity,
    timestamp: u32,
    name: &str,
    out: &mut [u8],
) -> usize {
    assert!(
        !name.is_empty(),
        "advert card name must not be empty (peers drop nameless adverts; \
         pass a pub_hash-derived label instead)"
    );

    let name_len = floor_char_boundary(name, MAX_ADVERT_NAME_LEN);
    let name_bytes = &name.as_bytes()[..name_len];
    let appdata_len = 1 + name_bytes.len();

    // Signed message = pubkey || timestamp_le || appdata (NOT header/path_len).
    let mut msg = [0u8; constants::PUB_KEY_SIZE + 4 + constants::MAX_ADVERT_DATA_SIZE];
    msg[..constants::PUB_KEY_SIZE].copy_from_slice(&identity.pubkey);
    msg[constants::PUB_KEY_SIZE..constants::PUB_KEY_SIZE + 4]
        .copy_from_slice(&timestamp.to_le_bytes());
    let msg_appdata =
        &mut msg[constants::PUB_KEY_SIZE + 4..constants::PUB_KEY_SIZE + 4 + appdata_len];
    msg_appdata[0] = ADVERT_APPDATA_FLAGS;
    msg_appdata[1..].copy_from_slice(name_bytes);
    let msg_len = constants::PUB_KEY_SIZE + 4 + appdata_len;

    let signature = identity.sign(&msg[..msg_len]);

    out[0] = Header::new(RouteType::Flood, PayloadType::Advert).0;
    out[1] = 0x00; // path_len: no hops yet — this is a freshly-built card.
    out[OFFSET_PUBKEY..OFFSET_TIMESTAMP].copy_from_slice(&identity.pubkey);
    out[OFFSET_TIMESTAMP..OFFSET_SIGNATURE].copy_from_slice(&timestamp.to_le_bytes());
    out[OFFSET_SIGNATURE..OFFSET_APPDATA].copy_from_slice(&signature);
    out[OFFSET_APPDATA] = ADVERT_APPDATA_FLAGS;
    out[OFFSET_APPDATA + 1..OFFSET_APPDATA + appdata_len].copy_from_slice(name_bytes);

    OFFSET_APPDATA + appdata_len
}

// ── URI rendering ────────────────────────────────────────────────────────────

/// URI scheme prefix MeshCore uses for a shareable card
/// (`Serial.print("meshcore://")` in the upstream `card` REPL command, and
/// the `import`/QR-scan side that consumes it).
pub const CARD_URI_SCHEME: &str = "meshcore://";

/// Maximum length of a card URI: the scheme prefix plus 2 lowercase hex
/// characters per card byte.
pub const MAX_CARD_URI_LEN: usize = CARD_URI_SCHEME.len() + 2 * MAX_ADVERT_CARD_LEN;

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Render `card` as a `meshcore://<lowercase-hex>` URI into `out`. Returns
/// the number of bytes written; `out[..n]` is ASCII, so it is always valid
/// UTF-8 and may be viewed as `&str` via `core::str::from_utf8`.
///
/// `out` must be at least `CARD_URI_SCHEME.len() + 2 * card.len()` bytes
/// (at most [`MAX_CARD_URI_LEN`] for a card no longer than
/// [`MAX_ADVERT_CARD_LEN`]).
pub fn card_to_uri(card: &[u8], out: &mut [u8]) -> usize {
    let scheme_len = CARD_URI_SCHEME.len();
    out[..scheme_len].copy_from_slice(CARD_URI_SCHEME.as_bytes());
    for (i, &byte) in card.iter().enumerate() {
        out[scheme_len + 2 * i] = HEX_DIGITS[(byte >> 4) as usize];
        out[scheme_len + 2 * i + 1] = HEX_DIGITS[(byte & 0x0F) as usize];
    }
    scheme_len + 2 * card.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    fn seed(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    #[test]
    fn golden_vector_self_advert_card() {
        // Fixed seed + fixed timestamp + fixed name -> byte-exact card.
        // Regenerated once from this implementation and hardcoded here as
        // the regression anchor (any future change to the wire format,
        // signing message, or flags must be a deliberate, reviewed edit to
        // this vector).
        let identity = Identity::from_seed(seed(0x01));
        let mut out = [0u8; MAX_ADVERT_CARD_LEN];
        let n = build_self_advert_card(&identity, 0x0102_0304, "Cadet", &mut out);

        assert_eq!(out[0], 0x11, "header: VER0 | ADVERT<<2 | FLOOD");
        assert_eq!(out[1], 0x00, "path_len: no hops on a freshly-built card");
        assert_eq!(&out[2..34], &identity.pubkey, "pubkey field");
        assert_eq!(
            &out[34..38],
            &0x0102_0304u32.to_le_bytes(),
            "timestamp field, little-endian"
        );
        let appdata = &out[102..n];
        assert_eq!(
            appdata[0], 0x81,
            "appdata flags: ADV_TYPE_CHAT | ADV_NAME_MASK"
        );
        assert_eq!(&appdata[1..], b"Cadet", "appdata name");
        assert_eq!(
            n,
            102 + 1 + 5,
            "total length: header..signature + flags + name"
        );
        assert_eq!(n, 108);

        let expected: [u8; 108] = [
            0x11, 0x00, 0x8a, 0x88, 0xe3, 0xdd, 0x74, 0x09, 0xf1, 0x95, 0xfd, 0x52, 0xdb, 0x2d,
            0x3c, 0xba, 0x5d, 0x72, 0xca, 0x67, 0x09, 0xbf, 0x1d, 0x94, 0x12, 0x1b, 0xf3, 0x74,
            0x88, 0x01, 0xb4, 0x0f, 0x6f, 0x5c, 0x04, 0x03, 0x02, 0x01, 0x03, 0x47, 0xc2, 0x75,
            0x80, 0xf4, 0xc2, 0x31, 0xcc, 0x6d, 0x81, 0x85, 0xc4, 0xaf, 0x41, 0xaa, 0xdc, 0x5c,
            0x19, 0xef, 0x5b, 0xd8, 0x19, 0x4a, 0xad, 0xe1, 0x23, 0xf7, 0xa9, 0x0c, 0x92, 0x60,
            0xae, 0xe5, 0x63, 0x1e, 0xf6, 0x79, 0x2d, 0x32, 0xe6, 0xda, 0xf7, 0x11, 0xdf, 0x44,
            0x37, 0x9b, 0xfa, 0x7a, 0x59, 0x84, 0xb0, 0xe7, 0x71, 0xe3, 0xef, 0x42, 0x6c, 0xdc,
            0xd2, 0xb9, 0x4b, 0x0e, 0x81, 0x43, 0x61, 0x64, 0x65, 0x74,
        ];
        assert_eq!(&out[..n], &expected[..], "byte-exact golden vector");
    }

    #[test]
    fn round_trip_parses_and_verifies() {
        let identity = Identity::from_seed(seed(0x22));
        let timestamp: u32 = 1_720_000_000;
        let name = "Field Node";

        let mut out = [0u8; MAX_ADVERT_CARD_LEN];
        let n = build_self_advert_card(&identity, timestamp, name, &mut out);
        let card = &out[..n];

        assert_eq!(card[0], 0x11);
        assert_eq!(card[1], 0x00);

        let pubkey: [u8; 32] = card[2..34].try_into().unwrap();
        assert_eq!(pubkey, identity.pubkey);

        let parsed_timestamp = u32::from_le_bytes(card[34..38].try_into().unwrap());
        assert_eq!(parsed_timestamp, timestamp);

        let signature_bytes: [u8; 64] = card[38..102].try_into().unwrap();
        let appdata = &card[102..n];
        assert_eq!(appdata[0], ADVERT_APPDATA_FLAGS);
        let parsed_name = core::str::from_utf8(&appdata[1..]).unwrap();
        assert_eq!(parsed_name, name);

        // The signature covers pubkey || timestamp_le || appdata ONLY —
        // never the header or path_len bytes.
        let mut msg = [0u8; 32 + 4 + 32];
        msg[..32].copy_from_slice(&pubkey);
        msg[32..36].copy_from_slice(&parsed_timestamp.to_le_bytes());
        msg[36..36 + appdata.len()].copy_from_slice(appdata);
        let msg_len = 36 + appdata.len();

        let verifying_key = VerifyingKey::from_bytes(&pubkey).unwrap();
        let signature = Signature::from_bytes(&signature_bytes);
        assert!(
            verifying_key.verify(&msg[..msg_len], &signature).is_ok(),
            "signature must verify over pubkey||timestamp||appdata"
        );
    }

    #[test]
    fn name_truncation_is_utf8_safe_at_the_31_byte_edge() {
        // 30 ASCII bytes + one 4-byte codepoint = 34 bytes. Truncating to 31
        // bytes lands mid-codepoint (byte offsets 31..34 are NOT char
        // boundaries of the trailing emoji) — the truncator must back off to
        // the last real boundary (30), not slice through it.
        let mut name = std::string::String::new();
        for _ in 0..30 {
            name.push('a');
        }
        name.push('\u{1F600}'); // 4-byte UTF-8 codepoint
        assert_eq!(name.len(), 34);

        let identity = Identity::from_seed(seed(0x33));
        let mut out = [0u8; MAX_ADVERT_CARD_LEN];
        let n = build_self_advert_card(&identity, 42, &name, &mut out);

        let appdata = &out[102..n];
        assert_eq!(appdata[0], ADVERT_APPDATA_FLAGS);
        let truncated_name = &appdata[1..];
        assert!(
            core::str::from_utf8(truncated_name).is_ok(),
            "truncated name must remain valid UTF-8"
        );
        assert_eq!(
            truncated_name.len(),
            30,
            "must back off before the split codepoint, not include a partial one"
        );
        assert_eq!(truncated_name, "a".repeat(30).as_bytes());
    }

    #[test]
    fn name_within_limit_is_not_truncated() {
        let identity = Identity::from_seed(seed(0x44));
        let mut out = [0u8; MAX_ADVERT_CARD_LEN];
        let n = build_self_advert_card(&identity, 1, "short", &mut out);
        let appdata = &out[102..n];
        assert_eq!(&appdata[1..], b"short");
    }

    #[test]
    fn name_exactly_at_the_limit_is_kept_whole() {
        let identity = Identity::from_seed(seed(0x55));
        let name = "a".repeat(MAX_ADVERT_NAME_LEN);
        let mut out = [0u8; MAX_ADVERT_CARD_LEN];
        let n = build_self_advert_card(&identity, 1, &name, &mut out);
        let appdata = &out[102..n];
        assert_eq!(appdata.len() - 1, MAX_ADVERT_NAME_LEN);
        assert_eq!(&appdata[1..], name.as_bytes());
    }

    #[test]
    #[should_panic(expected = "name must not be empty")]
    fn empty_name_is_rejected() {
        let identity = Identity::from_seed(seed(0x66));
        let mut out = [0u8; MAX_ADVERT_CARD_LEN];
        build_self_advert_card(&identity, 0, "", &mut out);
    }

    #[test]
    fn max_card_length_is_134_and_fits_one_provisioning_frame() {
        assert_eq!(MAX_ADVERT_CARD_LEN, 134);
        // FRAME_RSP_ADVERT carries the card as its whole payload with no
        // inner length field — it must comfortably fit the fixed-size
        // buffers firmware/host use for a single provisioning frame (both
        // sides currently size these at 512 B; see admin_server.rs /
        // session.rs).
        const {
            assert!(MAX_ADVERT_CARD_LEN + crate::provisioning::FRAME_OVERHEAD <= 512);
        }
    }

    #[test]
    fn card_to_uri_is_meshcore_scheme_plus_lowercase_hex() {
        let card = [0x00u8, 0x11, 0xAB, 0xFF];
        let mut out = [0u8; MAX_CARD_URI_LEN];
        let n = card_to_uri(&card, &mut out);
        let uri = core::str::from_utf8(&out[..n]).unwrap();
        assert_eq!(uri, "meshcore://0011abff");
    }

    #[test]
    fn card_to_uri_round_trips_a_built_card() {
        let identity = Identity::from_seed(seed(0x77));
        let mut card_buf = [0u8; MAX_ADVERT_CARD_LEN];
        let n = build_self_advert_card(&identity, 99, "Alice", &mut card_buf);

        let mut uri_buf = [0u8; MAX_CARD_URI_LEN];
        let uri_len = card_to_uri(&card_buf[..n], &mut uri_buf);
        let uri = core::str::from_utf8(&uri_buf[..uri_len]).unwrap();

        assert!(uri.starts_with(CARD_URI_SCHEME));
        assert_eq!(uri.len(), CARD_URI_SCHEME.len() + 2 * n);
        assert!(uri[CARD_URI_SCHEME.len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }
}
