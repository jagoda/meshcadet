// SPDX-License-Identifier: GPL-3.0-only
//! MeshCore v1.15 wire protocol — shared by MeshCadet firmware and the host CLI.
//!
//! This crate is the byte-exact port of the MeshCore v1.15.0 (`dee3e26a`) wire
//! format: packet framing, header/route/payload-type fields, 2-byte path-hash
//! encoding, the crypto/identity primitives (Ed25519 → X25519 ECDH, AES-128-ECB,
//! 2-byte HMAC-SHA256 MAC), the DM + ACK codec, and symmetric channel encryption.
//!
//! The authoritative spec is the upstream MeshCore v1.15.0 firmware source
//! (`dee3e26a`) this crate is ported from.
//! Interop is a HARD requirement — every byte on the air must match the deployed
//! mesh. See `docs/adr/0001-charter.md`.
//!
//! # Module map
//!
//! | Module | Contents |
//! |--------|----------|
//! | `constants`  | Wire constants mirrored from `src/MeshCore.h` |
//! | `header`     | `Header` (1-byte packet header) and `PathLen` (path_len encoding) |
//! | `crypto`     | AES-128-ECB, HMAC-SHA256-2, SHA-256, encrypt-then-MAC |
//! | `identity`   | `Identity` (Ed25519 keypair), X25519 ECDH shared secret |
//! | `advert`     | Self-advert "biz card" builder (signed ADVERT) + `meshcore://` URI rendering |
//! | `codec`      | DM, ACK, GRP_TXT, PATH-return encode/decode |
//! | `policy`     | `PolicyFilter` — allowlist-only DM filter + telemetry gate |
//! | `telemetry`  | GPS telemetry: bespoke `?loc` text codec + MeshCore-native REQ/RESPONSE (companion-app) codec |
//! | `history`    | Rotating history codec + in-memory ring buffer + wire export protocol |
//! | `history_region` | Per-conversation flash region format: header, append/compaction ring, directory |

#![cfg_attr(not(test), no_std)]

// ── Wire constants ────────────────────────────────────────────────────────────

/// MeshCore wire constants, mirrored from `src/MeshCore.h` @ `dee3e26a`.
pub mod constants {
    /// Max application payload bytes in a packet.
    pub const MAX_PACKET_PAYLOAD: usize = 184;
    /// Max path field length in bytes.
    pub const MAX_PATH_SIZE: usize = 64;
    /// Max transmission unit (whole frame ceiling).
    pub const MAX_TRANS_UNIT: usize = 255;
    /// AES-128 key size (first 16 bytes of the 32-byte ECDH/channel secret).
    pub const CIPHER_KEY_SIZE: usize = 16;
    /// AES block size.
    pub const CIPHER_BLOCK_SIZE: usize = 16;
    /// Truncated HMAC-SHA256 MAC size prepended to ciphertext.
    pub const CIPHER_MAC_SIZE: usize = 2;
    /// Ed25519 public key / node identity size.
    pub const PUB_KEY_SIZE: usize = 32;
    /// Ed25519 private key size (seed = 32 B; expanded = 64 B).
    pub const PRV_KEY_SIZE: usize = 64;
    /// Ed25519 signature size.
    pub const SIGNATURE_SIZE: usize = 64;
    /// Max advert app-data bytes.
    pub const MAX_ADVERT_DATA_SIZE: usize = 32;
    /// Truncated packet-hash size used for duplicate detection.
    pub const MAX_HASH_SIZE: usize = 8;

    // ── Header bit layout (src/Packet.h) ─────────────────────────────────
    pub const PH_ROUTE_MASK: u8 = 0x03;
    pub const PH_TYPE_SHIFT: u8 = 2;
    pub const PH_VER_SHIFT: u8 = 6;
}

// ── Enums ────────────────────────────────────────────────────────────────────

/// Route types (header bits 0–1). See recon doc §1.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteType {
    TransportFlood = 0x00,
    Flood = 0x01,
    Direct = 0x02,
    TransportDirect = 0x03,
}

/// Payload types (header bits 2–5). See recon doc §1.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadType {
    Req = 0x00,
    Response = 0x01,
    TxtMsg = 0x02,
    Ack = 0x03,
    Advert = 0x04,
    GrpTxt = 0x05,
    GrpData = 0x06,
    AnonReq = 0x07,
    Path = 0x08,
    Trace = 0x09,
    Multipart = 0x0A,
    Control = 0x0B,
    RawCustom = 0x0F,
}

// ── Sub-modules ───────────────────────────────────────────────────────────────

pub mod advert;
pub mod codec;
pub mod crypto;
pub mod dedup;
pub mod emoji;
pub mod header;
pub mod history;
pub mod history_region;
pub mod identity;
pub mod mention;
pub mod policy;
pub mod provisioning;
pub mod staging;
pub mod telemetry;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use advert::{
    build_self_advert_card, card_to_uri, ADVERT_APPDATA_FLAGS, CARD_URI_SCHEME,
    MAX_ADVERT_CARD_LEN, MAX_ADVERT_NAME_LEN, MAX_CARD_URI_LEN,
};
pub use codec::{
    channel_hash, channel_hash_var, compute_ack_hash, decode_dm_payload, decode_grp_txt,
    decode_grp_txt_var, decode_path_return, decode_txt_msg_plaintext, encode_dm_payload,
    encode_grp_txt, encode_grp_txt_var, encode_txt_msg_plaintext, format_channel_text,
    parse_channel_text, CodecError, GrpTxtFields, PathExtra, ReturnPath, CHANNEL_NAME_DELIM,
};
pub use crypto::{
    aes128_ecb_decrypt, aes128_ecb_encrypt, ceil_16, encrypt_then_mac, encrypt_then_mac_var,
    hmac_sha256_2, mac_then_decrypt, mac_then_decrypt_var, sha256, sha256_2, MacError,
};
pub use dedup::{packet_dedup_key, packet_payload_view};
pub use header::{Header, PathLen};
pub use history::{
    decode_entry_blob, decode_rsp_history_entry, encode_entry_blob, encode_rsp_history_entry,
    HistoryEntry, HistoryMsgType, RingBuffer, HISTORY_BLOB_LEN, HISTORY_ENTRY_BLOB_LEN,
    MAX_HISTORY_ENTRIES, MAX_HISTORY_TEXT_LEN, MAX_RSP_HISTORY_ENTRY_PAYLOAD,
};
pub use history_region::{
    decode_region_header, decode_slot, encode_region_header, encode_slot, find_newest_ours_unacked,
    find_or_claim_region, find_write_head, generation_is_newer, slot_offset, HistoryRegion,
    RegionError, RegionHeader, FLAG_IS_OURS, MAX_CONVERSATION_REGIONS, REGION_HEADER_LEN,
    REGION_SIZE, SECTORS_PER_REGION, SECTOR_SIZE, SLOTS_PER_SECTOR,
};
pub use identity::Identity;
pub use mention::{split_mentions, wrap_mentions, MentionRun, MentionRuns, MentionTier};
pub use policy::PolicyFilter;
pub use telemetry::{
    decode_telemetry_response,
    decode_telemetry_response_lpp,
    encode_no_fix_response,
    encode_telemetry_response,
    encode_telemetry_response_lpp,
    is_no_fix_response,
    is_telemetry_req,
    is_telemetry_request,
    parse_telemetry_req,
    // MeshCore-native companion-app telemetry REQ/RESPONSE codec.
    TelemetryReq,
    TelemetryResponse,
    TelemetryResponseLpp,
    MAX_RESPONSE_LEN,
    MAX_TELEMETRY_RESPONSE_LEN,
    REQ_TYPE_GET_TELEMETRY_DATA,
    TELEMETRY_REQUEST_MAGIC,
    TELEM_CHANNEL_SELF,
};

// ── Top-level tests (smoke + regression) ─────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_constants_match_meshcore_v1_15() {
        assert_eq!(constants::MAX_TRANS_UNIT, 255);
        assert_eq!(constants::CIPHER_KEY_SIZE, 16);
        assert_eq!(constants::PUB_KEY_SIZE, 32);
        assert_eq!(constants::CIPHER_MAC_SIZE, 2);
        // Wire frame ceiling: 1 + 4 + 1 + 64 + 184 = 254 < 255
        const {
            assert!(
                1 + 4 + 1 + constants::MAX_PATH_SIZE + constants::MAX_PACKET_PAYLOAD
                    < constants::MAX_TRANS_UNIT
            );
        }
    }

    #[test]
    fn header_field_encoding_roundtrips() {
        // A flood TXT_MSG header byte = (TXT_MSG << 2) | FLOOD = 0x09 (recon §8).
        let header: u8 =
            (PayloadType::TxtMsg as u8) << constants::PH_TYPE_SHIFT | (RouteType::Flood as u8);
        assert_eq!(header, 0x09);
        assert_eq!(header & constants::PH_ROUTE_MASK, RouteType::Flood as u8);
        assert_eq!(
            (header >> constants::PH_TYPE_SHIFT) & 0x0F,
            PayloadType::TxtMsg as u8
        );
    }

    #[test]
    fn end_to_end_dm_with_ecdh() {
        // Full E2E: two identities → ECDH → encode DM → decode DM
        let alice = Identity::from_seed([0xAAu8; 32]);
        let bob = Identity::from_seed([0xBBu8; 32]);

        let shared_a = alice.ecdh_shared_secret(&bob.pubkey);
        let shared_b = bob.ecdh_shared_secret(&alice.pubkey);
        assert_eq!(shared_a, shared_b, "ECDH must be symmetric");

        let mut pt_buf = [0u8; 64];
        let pt_len = encode_txt_msg_plaintext(0xDEAD_BEEF, 0, 0, b"hi bob", &mut pt_buf);

        let mut dm_buf = [0u8; 256];
        let dm_len = encode_dm_payload(
            &shared_a,
            bob.pub_hash(),
            alice.pub_hash(),
            &pt_buf[..pt_len],
            &mut dm_buf,
        );

        let mut dec_buf = [0u8; 256];
        let (dest, src, _) = decode_dm_payload(&shared_b, &dm_buf[..dm_len], &mut dec_buf).unwrap();
        assert_eq!(dest, bob.pub_hash());
        assert_eq!(src, alice.pub_hash());

        let (ts, _, _, off) = decode_txt_msg_plaintext(&dec_buf, pt_len).unwrap();
        assert_eq!(ts, 0xDEAD_BEEF);
        assert_eq!(&dec_buf[off..off + 6], b"hi bob");
    }
}
