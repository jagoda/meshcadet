// SPDX-License-Identifier: GPL-3.0-only
//! DM, ACK, GRP_TXT, and PATH-return codecs.
//!
//! Source references (@ dee3e26a):
//!   src/helpers/BaseChatMesh.cpp — ACK hash, onPeerDataRecv
//!   src/Mesh.cpp                 — createPathReturn
//!   Spec §4 (recon doc)
//!
//! Wire layouts:
//!
//!   DM payload (TXT_MSG / REQ / RESPONSE):
//!     [dest_hash(1)] [src_hash(1)] [HMAC(2)] [AES-128-ECB ciphertext]
//!
//!   TXT_MSG plaintext:
//!     [timestamp_le(4)] [txt_type_attempt(1)] [text...]
//!
//!   ACK payload (v1.15):
//!     [ack_hash(4)]
//!     where ack_hash = SHA256(timestamp_le(4) || txt_type_attempt(1) || text || sender_pub_key(32))[0:4]
//!
//!   GRP_TXT payload:
//!     [channel_hash(1)] [HMAC(2)] [AES-128-ECB ciphertext]
//!     channel_hash = SHA256(channel_secret)[0]
//!     ciphertext   = AES-128-ECB(channel_secret[0:16], timestamp_le(4)||txt_type_attempt(1)||text)
//!     HMAC         = HMAC-SHA256(channel_secret[0:32], ciphertext)[0:2]
//!
//!   PATH return payload (outer = same as DM):
//!     inner plaintext: [path_len(1)] [path(N)] [extra_type(1)] [extra...]
//!     extra_type 0x03 (ACK): [ack_hash(4)]
//!     extra_type 0xFF:       [4 random bytes] (no bundled ack)

use crate::crypto::{encrypt_then_mac_var, mac_then_decrypt_var, sha256, MacError};
use sha2::{Digest, Sha256};

// ── Error type ───────────────────────────────────────────────────────────────

/// Errors from codec encode/decode operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecError {
    /// MAC verification failed (tampered or wrong key).
    MacMismatch,
    /// Payload is shorter than the minimum for this type.
    TruncatedPayload,
    /// Ciphertext length is not a multiple of the AES block size (16).
    MisalignedCiphertext,
}

impl From<MacError> for CodecError {
    fn from(_: MacError) -> Self {
        CodecError::MacMismatch
    }
}

// ── DM encode / decode ───────────────────────────────────────────────────────

/// Encode a DM payload (TXT_MSG, REQ, or RESPONSE).
///
/// `plaintext` should already be formatted for the message type, e.g.:
/// - TXT_MSG: `[timestamp_le(4)] [txt_type_attempt(1)] [text...]`
///
/// Returns the number of bytes written to `out`.
/// `out` must be at least `2 + 2 + ceil_16(plaintext.len())` bytes.
pub fn encode_dm_payload(
    shared_secret: &[u8; 32],
    dest_hash: u8,
    src_hash: u8,
    plaintext: &[u8],
    out: &mut [u8],
) -> usize {
    let mut aes_key = [0u8; 16];
    aes_key.copy_from_slice(&shared_secret[..16]);
    out[0] = dest_hash;
    out[1] = src_hash;
    let mac_ct_len = encrypt_then_mac_var(&aes_key, shared_secret, plaintext, &mut out[2..]);
    2 + mac_ct_len
}

/// Decode a DM payload.
///
/// Returns `(dest_hash, src_hash, plaintext_len)` on success.
/// `plaintext_out` receives the decrypted bytes (length = ciphertext_len, zero-padded at end).
pub fn decode_dm_payload(
    shared_secret: &[u8; 32],
    payload: &[u8],
    plaintext_out: &mut [u8],
) -> Result<(u8, u8, usize), CodecError> {
    if payload.len() < 4 {
        // Minimum: dest_hash(1) + src_hash(1) + MAC(2) = 4 bytes
        return Err(CodecError::TruncatedPayload);
    }
    let dest_hash = payload[0];
    let src_hash = payload[1];
    let mac_and_ct = &payload[2..];
    if mac_and_ct.len() < 2 || !(mac_and_ct.len() - 2).is_multiple_of(16) {
        return Err(CodecError::MisalignedCiphertext);
    }
    let mut aes_key = [0u8; 16];
    aes_key.copy_from_slice(&shared_secret[..16]);
    let pt_len = mac_then_decrypt_var(&aes_key, shared_secret, mac_and_ct, plaintext_out)?;
    Ok((dest_hash, src_hash, pt_len))
}

// ── TXT_MSG plaintext helper ─────────────────────────────────────────────────

/// Encode a TXT_MSG plaintext: `[timestamp_le(4)] [txt_type_attempt(1)] [text...]`
/// Returns the number of bytes written.
pub fn encode_txt_msg_plaintext(
    timestamp: u32,
    txt_type: u8,
    attempt: u8,
    text: &[u8],
    out: &mut [u8],
) -> usize {
    let ts_bytes = timestamp.to_le_bytes();
    out[0] = ts_bytes[0];
    out[1] = ts_bytes[1];
    out[2] = ts_bytes[2];
    out[3] = ts_bytes[3];
    out[4] = (txt_type << 2) | (attempt & 0x03); // upper 6 bits = txt_type, lower 2 = attempt
    out[5..5 + text.len()].copy_from_slice(text);
    5 + text.len()
}

/// Decode a TXT_MSG plaintext (after AES decryption).
/// Returns `(timestamp, txt_type, attempt, text_start_offset)` or error.
pub fn decode_txt_msg_plaintext(
    plaintext: &[u8],
    plaintext_actual_len: usize,
) -> Result<(u32, u8, u8, usize), CodecError> {
    if plaintext_actual_len < 5 {
        return Err(CodecError::TruncatedPayload);
    }
    let timestamp = u32::from_le_bytes([plaintext[0], plaintext[1], plaintext[2], plaintext[3]]);
    let type_byte = plaintext[4];
    let txt_type = type_byte >> 2;
    let attempt = type_byte & 0x03;
    Ok((timestamp, txt_type, attempt, 5))
}

// ── ACK hash (v1.15) ─────────────────────────────────────────────────────────

/// Compute the 4-byte ACK hash (v1.15).
///
/// `ack_hash = SHA256(timestamp_le(4) || txt_type_attempt(1) || text || sender_pub_key(32))[0:4]`
///
/// Source: `BaseChatMesh.cpp` line 221–222 @ dee3e26a.
/// ⚠ v1.16 changed this to 6 bytes; build against dee3e26 for 4-byte behaviour.
pub fn compute_ack_hash(
    timestamp: u32,
    txt_type_attempt: u8,
    text: &[u8],
    sender_pub_key: &[u8; 32],
) -> [u8; 4] {
    let ts = timestamp.to_le_bytes();
    let mut h = Sha256::new();
    h.update(ts);
    h.update([txt_type_attempt]);
    h.update(text);
    h.update(sender_pub_key);
    let result = h.finalize();
    [result[0], result[1], result[2], result[3]]
}

// ── GRP_TXT encode / decode ──────────────────────────────────────────────────

/// Channel hash from a 32-byte channel secret: `SHA256(channel_secret)[0]`.
///
/// This is the 256-bit-secret convention. For 128-bit channels use
/// [`channel_hash_var`] with the 16-byte secret slice.
pub fn channel_hash(channel_secret: &[u8; 32]) -> u8 {
    channel_hash_var(channel_secret)
}

/// Channel hash for a variable-length channel secret: `SHA256(secret)[0]`.
///
/// MeshCore hashes the channel over exactly `secret_len` bytes (recon doc §9):
///   - 256-bit channel: pass the full 32-byte secret  → `SHA256(secret)[0]`
///   - 128-bit channel: pass `secret[0:16]`           → `SHA256(secret[0:16])[0]`
///     (`BaseChatMesh.cpp:896`). The caller selects the convention by slicing.
pub fn channel_hash_var(channel_secret: &[u8]) -> u8 {
    sha256(channel_secret)[0]
}

/// Encode a GRP_TXT payload.
///
/// Wire layout: `[channel_hash(1)] [HMAC(2)] [ciphertext]`
/// plaintext = `[timestamp_le(4)] [txt_type_attempt(1)] [text...]`
///
/// `out` must be at least `1 + 2 + ceil_16(5 + text.len())` bytes.
/// Returns total bytes written.
pub fn encode_grp_txt(
    channel_secret: &[u8; 32],
    timestamp: u32,
    txt_type: u8,
    attempt: u8,
    text: &[u8],
    out: &mut [u8],
) -> usize {
    encode_grp_txt_var(channel_secret, timestamp, txt_type, attempt, text, out)
}

/// Encode a GRP_TXT payload for a variable-length channel secret.
///
/// Pass the 32-byte secret for a 256-bit channel, or `secret[0:16]` for a
/// 128-bit channel (this fixes both the channel hash AND the HMAC key length).
/// `channel_secret.len()` must be >= 16 (the AES-128 key is `secret[0:16]`).
pub fn encode_grp_txt_var(
    channel_secret: &[u8],
    timestamp: u32,
    txt_type: u8,
    attempt: u8,
    text: &[u8],
    out: &mut [u8],
) -> usize {
    // Build plaintext on stack (max text size in a single packet << 256)
    let mut pt_buf = [0u8; 256];
    let pt_len = encode_txt_msg_plaintext(timestamp, txt_type, attempt, text, &mut pt_buf);

    let mut aes_key = [0u8; 16];
    aes_key.copy_from_slice(&channel_secret[..16]);

    out[0] = channel_hash_var(channel_secret);
    let mac_ct_len =
        encrypt_then_mac_var(&aes_key, channel_secret, &pt_buf[..pt_len], &mut out[1..]);
    1 + mac_ct_len
}

// ── Channel sender-name prefix (MeshCore convention) ──────────────────────────
//
// A channel (GRP_TXT) packet carries no per-sender addressing, so MeshCore
// attributes a message by prepending the sender's node name to the text body:
//
//   BaseChatMesh::sendGroupMessage (BaseChatMesh.cpp:480 @ dee3e26a):
//     sprintf((char *) &temp[5], "%s: ", sender_name);   // "<name>: <msg>"
//
// The delimiter is exactly a colon followed by a single space (`": "`), placed
// between the name and the body. The receiver displays the whole `<name>: <msg>`
// string; the companion app parses the name off the front. MeshCadet must emit
// the identical layout so companions attribute its channel messages correctly.

/// The MeshCore channel sender-name delimiter: a colon followed by one space.
pub const CHANNEL_NAME_DELIM: &[u8] = b": ";

/// Append `src` to `out` starting at `w`, clamped to the buffer; returns the new
/// write cursor. Shared by [`format_channel_text`].
#[inline]
fn append_clamped(out: &mut [u8], w: usize, src: &[u8]) -> usize {
    let take = src.len().min(out.len().saturating_sub(w));
    out[w..w + take].copy_from_slice(&src[..take]);
    w + take
}

/// Format a channel text body with the MeshCore sender-name prefix:
/// `<sender_name>: <body>`.
///
/// Mirrors `BaseChatMesh::sendGroupMessage` byte-for-byte (name, then `": "`,
/// then body). Writes into `out` (clamped to its length) and returns the number
/// of bytes written. The returned bytes are the plaintext *text* region only —
/// the caller still wraps it with the 5-byte `[timestamp][txt_type]` header via
/// [`encode_grp_txt_var`].
pub fn format_channel_text(sender_name: &[u8], body: &[u8], out: &mut [u8]) -> usize {
    let w = append_clamped(out, 0, sender_name);
    let w = append_clamped(out, w, CHANNEL_NAME_DELIM);
    append_clamped(out, w, body)
}

/// Split an inbound channel text body into `(sender_name, body)` on the first
/// `": "` delimiter.
///
/// Returns `(Some(name), body)` when the MeshCore prefix is present, or
/// `(None, full_text)` when it is absent (best-effort display of malformed or
/// prefix-less channel text). Splitting on the FIRST delimiter matches the
/// companion: a body that itself contains `": "` keeps the trailing colon in the
/// body, exactly as MeshCore's display does.
pub fn parse_channel_text(text: &[u8]) -> (Option<&[u8]>, &[u8]) {
    if text.len() >= CHANNEL_NAME_DELIM.len() {
        let last = text.len() - CHANNEL_NAME_DELIM.len();
        for i in 0..=last {
            if &text[i..i + CHANNEL_NAME_DELIM.len()] == CHANNEL_NAME_DELIM {
                return (Some(&text[..i]), &text[i + CHANNEL_NAME_DELIM.len()..]);
            }
        }
    }
    (None, text)
}

/// GRP_TXT decoded fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrpTxtFields {
    pub timestamp: u32,
    pub txt_type: u8,
    pub attempt: u8,
    /// Start offset of text bytes within the `plaintext_buf` passed to `decode_grp_txt`.
    pub text_offset: usize,
    /// Length of text bytes.
    pub text_len: usize,
}

/// Decode a GRP_TXT payload.
///
/// `plaintext_buf` must be at least `ceil_16(payload.len() - 3)` bytes.
/// On success fills `plaintext_buf` with the decrypted bytes and returns field offsets.
pub fn decode_grp_txt(
    channel_secret: &[u8; 32],
    payload: &[u8],
    plaintext_buf: &mut [u8],
) -> Result<GrpTxtFields, CodecError> {
    decode_grp_txt_var(channel_secret, payload, plaintext_buf)
}

/// Decode a GRP_TXT payload for a variable-length channel secret.
///
/// Pass the 32-byte secret for a 256-bit channel, or `secret[0:16]` for a
/// 128-bit channel. The channel-hash byte (`payload[0]`) is left to the caller
/// to verify against [`channel_hash_var`] of the same slice.
pub fn decode_grp_txt_var(
    channel_secret: &[u8],
    payload: &[u8],
    plaintext_buf: &mut [u8],
) -> Result<GrpTxtFields, CodecError> {
    // Minimum: channel_hash(1) + MAC(2) + at least one AES block(16) = 19 bytes
    if payload.len() < 3 {
        return Err(CodecError::TruncatedPayload);
    }
    let _ch = payload[0]; // channel_hash (caller may verify against channel_hash_var(secret))
    let mac_and_ct = &payload[1..];
    if mac_and_ct.len() < 2 || !(mac_and_ct.len() - 2).is_multiple_of(16) {
        return Err(CodecError::MisalignedCiphertext);
    }
    let mut aes_key = [0u8; 16];
    aes_key.copy_from_slice(&channel_secret[..16]);
    let pt_len = mac_then_decrypt_var(&aes_key, channel_secret, mac_and_ct, plaintext_buf)?;
    let (timestamp, txt_type, attempt, text_offset) =
        decode_txt_msg_plaintext(plaintext_buf, pt_len)?;
    Ok(GrpTxtFields {
        timestamp,
        txt_type,
        attempt,
        text_offset,
        text_len: pt_len.saturating_sub(text_offset),
    })
}

// ── PATH-return decode ────────────────────────────────────────────────────────

/// Extra payload bundled inside a PATH return packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathExtra {
    /// A 4-byte ACK hash bundled with the returned path.
    Ack([u8; 4]),
    /// No meaningful extra (extra_type = 0xFF, 4 random bytes).
    None,
}

/// Decoded PATH-return packet inner plaintext.
#[derive(Clone, Copy, Debug)]
pub struct ReturnPath {
    /// The `path_len` byte (same encoding as the outer packet path_len).
    pub path_len_byte: u8,
    /// Path bytes (hop_count × hash_size, up to 64 bytes).
    pub path: [u8; 64],
    /// Number of valid bytes in `path`.
    pub path_byte_count: usize,
    /// Bundled extra payload.
    pub extra: PathExtra,
}

/// Decode a PATH-return payload (outer DM envelope has already been stripped).
///
/// `plaintext` is the AES-decrypted inner payload (from the outer DM decode).
/// `pt_len` is the actual plaintext length (ignoring zero-padding).
pub fn decode_path_return_plaintext(
    plaintext: &[u8],
    pt_len: usize,
) -> Result<ReturnPath, CodecError> {
    // Minimum: path_len(1) + extra_type(1) = 2 bytes (0 hop path + dummy extra)
    if pt_len < 2 {
        return Err(CodecError::TruncatedPayload);
    }
    let path_len_byte = plaintext[0];
    let hash_size = ((path_len_byte >> 6) + 1) as usize;
    let hop_count = (path_len_byte & 63) as usize;
    let path_byte_count = hop_count * hash_size;
    if pt_len < 1 + path_byte_count + 1 {
        return Err(CodecError::TruncatedPayload);
    }
    let mut path = [0u8; 64];
    path[..path_byte_count].copy_from_slice(&plaintext[1..1 + path_byte_count]);

    let extra_offset = 1 + path_byte_count;
    let extra_type = plaintext[extra_offset] & 0x0F; // lower 4 bits = payload type

    let extra = if extra_type == 0x03 {
        // PAYLOAD_TYPE_ACK: next 4 bytes are the ack_hash
        if pt_len < extra_offset + 1 + 4 {
            return Err(CodecError::TruncatedPayload);
        }
        let mut ack = [0u8; 4];
        ack.copy_from_slice(&plaintext[extra_offset + 1..extra_offset + 5]);
        PathExtra::Ack(ack)
    } else {
        // 0xFF or anything else: no meaningful extra
        PathExtra::None
    };

    Ok(ReturnPath {
        path_len_byte,
        path,
        path_byte_count,
        extra,
    })
}

/// Full PATH-return packet decode: outer DM envelope + inner path plaintext.
pub fn decode_path_return(
    shared_secret: &[u8; 32],
    payload: &[u8],
    plaintext_buf: &mut [u8; 256],
) -> Result<(u8, u8, ReturnPath), CodecError> {
    let (dest_hash, src_hash, pt_len) = decode_dm_payload(shared_secret, payload, plaintext_buf)?;
    let rp = decode_path_return_plaintext(plaintext_buf, pt_len)?;
    Ok((dest_hash, src_hash, rp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::ceil_16;
    use crate::identity::Identity;

    fn make_pair() -> (Identity, Identity) {
        let a = Identity::from_seed([0x01u8; 32]);
        let b = Identity::from_seed([0x02u8; 32]);
        (a, b)
    }

    // ── ACK hash known-answer ────────────────────────────────────────────

    #[test]
    fn ack_hash_known_answer() {
        // Fixed inputs → fixed SHA256 → fixed 4-byte truncation.
        // timestamp=1, txt_type_attempt=0, text=b"hi", sender_pub_key=[0;32]
        let ts: u32 = 1;
        let tta: u8 = 0;
        let text = b"hi";
        let spk = [0u8; 32];
        let hash = compute_ack_hash(ts, tta, text, &spk);

        // Independently compute expected: SHA256(01_00_00_00 || 00 || "hi" || [0;32])
        let mut h = Sha256::new();
        h.update([0x01, 0x00, 0x00, 0x00]); // timestamp LE
        h.update([0x00]); // txt_type_attempt
        h.update(b"hi");
        h.update([0u8; 32]);
        let full = h.finalize();
        assert_eq!(hash, [full[0], full[1], full[2], full[3]]);
    }

    #[test]
    fn ack_hash_is_4_bytes_v1_15() {
        // Regression: v1.15 ACK is 4 bytes, NOT 6 (that's v1.16)
        let hash = compute_ack_hash(0xDEAD_BEEF, 0x05, b"test", &[0xAAu8; 32]);
        assert_eq!(hash.len(), 4, "ACK hash must be exactly 4 bytes (v1.15)");
    }

    #[test]
    fn ack_hash_changes_with_different_inputs() {
        let h1 = compute_ack_hash(1, 0, b"msg", &[0u8; 32]);
        let h2 = compute_ack_hash(2, 0, b"msg", &[0u8; 32]);
        let h3 = compute_ack_hash(1, 0, b"msg", &[1u8; 32]);
        assert_ne!(h1, h2, "different timestamp");
        assert_ne!(h1, h3, "different sender pubkey");
    }

    // ── DM → ACK hash agreement (firmware loop invariant) ────────────────

    /// Trim AES-ECB zero-padding to the C-string text length (mirrors the
    /// firmware `c_str` helper used before computing the ACK hash).
    fn c_str_len(buf: &[u8]) -> usize {
        buf.iter().position(|&b| b == 0).unwrap_or(buf.len())
    }

    #[test]
    fn dm_then_ack_hash_agrees_across_two_nodes() {
        // Invariant: the ACK hash the ORIGINATOR expects (keyed on its own
        // pubkey, over the exact text it sent) equals the ACK hash the RESPONDER
        // computes after decrypt + zero-padding trim. This is what closes the
        // firmware DM+ACK loop; if it ever diverges the loop silently breaks.
        let originator = Identity::from_seed([0x11u8; 32]); // bench seed A
        let responder = Identity::from_seed([0x42u8; 32]); // bench seed B
        let shared_o = originator.ecdh_shared_secret(&responder.pubkey);
        let shared_r = responder.ecdh_shared_secret(&originator.pubkey);

        let text = b"hi from meshcadet";
        let ts: u32 = 12_345;
        let type_byte: u8 = 0; // txt_type=0, attempt=0

        // Originator computes the ACK it expects back (keyed on ITS pubkey).
        let expected = compute_ack_hash(ts, type_byte, text, &originator.pubkey);

        // Originator encodes the DM.
        let mut pt = [0u8; 64];
        let pt_len = encode_txt_msg_plaintext(ts, 0, 0, text, &mut pt);
        let mut dm = [0u8; 256];
        let dm_len = encode_dm_payload(
            &shared_o,
            responder.pub_hash(),
            originator.pub_hash(),
            &pt[..pt_len],
            &mut dm,
        );

        // Responder decrypts and recovers ts / type / text (padding trimmed).
        let mut dec = [0u8; 256];
        let (_dest, _src, dec_len) = decode_dm_payload(&shared_r, &dm[..dm_len], &mut dec).unwrap();
        let rx_ts = u32::from_le_bytes([dec[0], dec[1], dec[2], dec[3]]);
        let rx_type = dec[4];
        let rx_text_region = &dec[5..dec_len];
        let rx_text = &rx_text_region[..c_str_len(rx_text_region)];
        assert_eq!(rx_ts, ts);
        assert_eq!(rx_type, type_byte);
        assert_eq!(rx_text, text);

        // Responder computes the ACK keyed on the ORIGINATOR's pubkey.
        let responder_ack = compute_ack_hash(rx_ts, rx_type, rx_text, &originator.pubkey);
        assert_eq!(
            responder_ack, expected,
            "DM+ACK hash must agree across nodes"
        );
    }

    // ── DM encode/decode round-trip ──────────────────────────────────────

    #[test]
    fn dm_round_trip_txt_msg() {
        let (a, b) = make_pair();
        let shared = a.ecdh_shared_secret(&b.pubkey);

        // Encode TXT_MSG plaintext
        let mut pt_buf = [0u8; 64];
        let pt_len = encode_txt_msg_plaintext(0x0102_0304, 0x00, 0, b"hello mesh", &mut pt_buf);

        // Encode DM payload
        let mut dm_buf = [0u8; 256];
        let dm_len = encode_dm_payload(
            &shared,
            b.pub_hash(),
            a.pub_hash(),
            &pt_buf[..pt_len],
            &mut dm_buf,
        );

        // Decode
        let mut dec_buf = [0u8; 256];
        let (dest, src, actual_pt_len) =
            decode_dm_payload(&shared, &dm_buf[..dm_len], &mut dec_buf).unwrap();

        assert_eq!(dest, b.pub_hash());
        assert_eq!(src, a.pub_hash());
        assert_eq!(actual_pt_len, ceil_16(pt_len));

        // Verify plaintext fields
        let (ts, txt_type, attempt, text_off) = decode_txt_msg_plaintext(&dec_buf, pt_len).unwrap();
        assert_eq!(ts, 0x0102_0304);
        assert_eq!(txt_type, 0x00);
        assert_eq!(attempt, 0);
        assert_eq!(&dec_buf[text_off..text_off + 10], b"hello mesh");
    }

    #[test]
    fn dm_wrong_key_fails_mac() {
        let (a, b) = make_pair();
        let shared_ab = a.ecdh_shared_secret(&b.pubkey);
        let shared_wrong = [0xFFu8; 32]; // wrong key

        let mut pt_buf = [0u8; 32];
        let pt_len = encode_txt_msg_plaintext(1, 0, 0, b"secret", &mut pt_buf);

        let mut dm_buf = [0u8; 256];
        let dm_len = encode_dm_payload(
            &shared_ab,
            b.pub_hash(),
            a.pub_hash(),
            &pt_buf[..pt_len],
            &mut dm_buf,
        );

        let mut dec_buf = [0u8; 256];
        let result = decode_dm_payload(&shared_wrong, &dm_buf[..dm_len], &mut dec_buf);
        assert_eq!(result, Err(CodecError::MacMismatch));
    }

    // ── Anti-replay: DM payload determinism (regression guard) ───────────
    //
    // MeshCore dedups inbound packets by SHA-256(payload_type || payload) over a
    // 160-entry ring and the companion app dedups per (contact, timestamp). AES-128
    // -ECB is deterministic (no IV/nonce), so a DM payload is a pure function of
    // (timestamp, text). A firmware that timestamps with seconds-since-BOOT replays
    // a byte-identical packet sequence after every reflash, and the node silently
    // drops the replays — the originated-DM-dropped defect. This guard pins the
    // mechanism: identical inputs ⇒ identical wire bytes, and a varied timestamp
    // base ⇒ distinct wire bytes (the fix: seed the timestamp per-boot).
    #[test]
    fn dm_payload_replays_unless_timestamp_varies() {
        let (a, b) = make_pair();
        let shared = a.ecdh_shared_secret(&b.pubkey);
        let text = b"hi from meshcadet";

        let encode_at = |ts: u32, out: &mut [u8; 256]| -> usize {
            let mut pt = [0u8; 64];
            let pt_len = encode_txt_msg_plaintext(ts, 0, 0, text, &mut pt);
            encode_dm_payload(&shared, b.pub_hash(), a.pub_hash(), &pt[..pt_len], out)
        };

        // Two boots that both reach "30 s since boot" reproduce the SAME timestamp
        // and therefore the SAME wire bytes — the node treats the second as a
        // replay and drops it.
        let mut boot1 = [0u8; 256];
        let n1 = encode_at(30, &mut boot1);
        let mut boot2 = [0u8; 256];
        let n2 = encode_at(30, &mut boot2);
        assert_eq!(
            (n1, &boot1[..n1]),
            (n2, &boot2[..n2]),
            "AES-ECB determinism: identical (timestamp,text) ⇒ identical packet (replay)"
        );

        // Seeding the timestamp with a per-boot base makes the packet unique, which
        // is exactly what defeats the mesh/app replay filter.
        let mut seeded = [0u8; 256];
        let ns = encode_at(0xA1B2_C3D4u32.wrapping_add(30), &mut seeded);
        assert_ne!(
            &seeded[..ns],
            &boot1[..n1],
            "a per-boot timestamp base must produce a distinct, never-before-seen packet"
        );
    }

    // ── GRP_TXT known-answer + round-trip ────────────────────────────────

    #[test]
    fn grp_txt_channel_hash_known() {
        // channel_hash = SHA256(channel_secret)[0]
        let secret = [0x42u8; 32];
        let expected = sha256(&secret)[0];
        assert_eq!(channel_hash(&secret), expected);
    }

    #[test]
    fn grp_txt_round_trip() {
        let channel_secret = [0x77u8; 32];

        let mut enc_buf = [0u8; 256];
        let n = encode_grp_txt(
            &channel_secret,
            0xDEAD,
            0x00,
            0,
            b"channel msg",
            &mut enc_buf,
        );

        // First byte must be the channel hash
        assert_eq!(enc_buf[0], channel_hash(&channel_secret));

        let mut pt_buf = [0u8; 256];
        let fields = decode_grp_txt(&channel_secret, &enc_buf[..n], &mut pt_buf).unwrap();

        assert_eq!(fields.timestamp, 0xDEAD);
        assert_eq!(fields.txt_type, 0x00);
        assert_eq!(fields.attempt, 0);
        assert_eq!(
            &pt_buf[fields.text_offset..fields.text_offset + fields.text_len],
            b"channel msg"
        );
    }

    #[test]
    fn grp_txt_wrong_secret_fails_mac() {
        let channel_secret = [0x11u8; 32];
        let wrong_secret = [0x22u8; 32];

        let mut enc_buf = [0u8; 256];
        let n = encode_grp_txt(&channel_secret, 1, 0, 0, b"test", &mut enc_buf);

        let mut pt_buf = [0u8; 256];
        let result = decode_grp_txt(&wrong_secret, &enc_buf[..n], &mut pt_buf);
        assert_eq!(result, Err(CodecError::MacMismatch));
    }

    #[test]
    fn grp_txt_128bit_channel_hash_variant() {
        // 128-bit channel: hash = SHA256(secret[0:16])[0], distinct from the
        // 256-bit convention SHA256(secret)[0]. Recon doc §9 / BaseChatMesh.cpp:896.
        let secret = [0x42u8; 32];
        let secret16 = &secret[..16];
        assert_eq!(channel_hash_var(secret16), sha256(secret16)[0]);
        // The two conventions differ for a non-uniform secret.
        let mut s2 = [0u8; 32];
        for (i, b) in s2.iter_mut().enumerate() {
            *b = i as u8;
        }
        assert_ne!(channel_hash_var(&s2[..16]), channel_hash_var(&s2[..]));
    }

    #[test]
    fn grp_txt_128bit_round_trip() {
        // Encode + decode under a 16-byte (128-bit) channel secret: AES key,
        // HMAC key, and channel hash all use exactly the 16-byte slice.
        let secret = [0x99u8; 32];
        let key16 = &secret[..16];

        let mut enc_buf = [0u8; 256];
        let n = encode_grp_txt_var(key16, 0xCAFE, 0x00, 0, b"128bit chan", &mut enc_buf);

        // Channel hash byte must be the 128-bit-convention hash.
        assert_eq!(enc_buf[0], channel_hash_var(key16));

        let mut pt_buf = [0u8; 256];
        let fields = decode_grp_txt_var(key16, &enc_buf[..n], &mut pt_buf).unwrap();
        assert_eq!(fields.timestamp, 0xCAFE);
        assert_eq!(
            &pt_buf[fields.text_offset..fields.text_offset + fields.text_len],
            b"128bit chan"
        );

        // A 256-bit-keyed decode of a 128-bit-encoded frame must fail the MAC
        // (HMAC key length differs), proving the conventions are not interchangeable.
        let mut pt_buf2 = [0u8; 256];
        let result = decode_grp_txt(&secret, &enc_buf[..n], &mut pt_buf2);
        assert_eq!(result, Err(CodecError::MacMismatch));
    }

    // ── Channel sender-name prefix (MeshCore interop) ─────────────────────

    /// Known-answer: the formatted text must be byte-identical to MeshCore's
    /// `sprintf("%s: ", sender_name)` + body. The companion parses this exact
    /// layout to attribute a channel message; any drift in delimiter/spacing
    /// silently breaks attribution (the original defect: no prefix at all).
    #[test]
    fn channel_text_format_known_answer() {
        let mut out = [0u8; 64];
        let n = format_channel_text(b"Alice", b"hello", &mut out);
        assert_eq!(&out[..n], b"Alice: hello");
    }

    #[test]
    fn channel_text_format_parse_round_trip() {
        let mut out = [0u8; 64];
        let n = format_channel_text(b"MeshCadet-AB", b"hi there", &mut out);
        let (name, body) = parse_channel_text(&out[..n]);
        assert_eq!(name, Some(&b"MeshCadet-AB"[..]));
        assert_eq!(body, b"hi there");
    }

    #[test]
    fn channel_text_parse_no_prefix_is_passthrough() {
        // A body with no "<name>: " prefix must display verbatim, not vanish.
        let (name, body) = parse_channel_text(b"no delimiter here");
        assert_eq!(name, None);
        assert_eq!(body, b"no delimiter here");
    }

    #[test]
    fn channel_text_splits_on_first_delim() {
        // A body that itself contains ": " keeps the trailing colon in the body,
        // matching the companion's first-delimiter split.
        let mut out = [0u8; 64];
        let n = format_channel_text(b"Bob", b"time: 5pm", &mut out);
        assert_eq!(&out[..n], b"Bob: time: 5pm");
        let (name, body) = parse_channel_text(&out[..n]);
        assert_eq!(name, Some(&b"Bob"[..]));
        assert_eq!(body, b"time: 5pm");
    }

    /// End-to-end wire check: a prefixed channel text survives encode → decode
    /// → parse with name and body intact — the exact path a companion exercises
    /// when MeshCadet sends and when MeshCadet receives.
    #[test]
    fn channel_text_prefix_survives_grp_txt_round_trip() {
        let channel_secret = [0x6du8; 32];
        let mut text = [0u8; 64];
        let text_len = format_channel_text(b"MeshCadet-7F", b"weather is good", &mut text);

        let mut enc = [0u8; 256];
        let n = encode_grp_txt(&channel_secret, 0xABCD, 0, 0, &text[..text_len], &mut enc);

        let mut pt = [0u8; 256];
        let fields = decode_grp_txt(&channel_secret, &enc[..n], &mut pt).unwrap();
        let rx = &pt[fields.text_offset..fields.text_offset + fields.text_len];
        let end = rx.iter().position(|&b| b == 0).unwrap_or(rx.len());
        let (name, body) = parse_channel_text(&rx[..end]);
        assert_eq!(name, Some(&b"MeshCadet-7F"[..]));
        assert_eq!(body, b"weather is good");
    }

    #[test]
    fn channel_text_format_clamps_to_buffer() {
        // Truncation must never panic or overflow the output buffer.
        let mut out = [0u8; 8];
        let n = format_channel_text(b"LongName", b"and a long body", &mut out);
        assert_eq!(n, 8);
        assert_eq!(&out[..n], b"LongName");
    }

    // ── PATH-return decode ────────────────────────────────────────────────

    #[test]
    fn path_return_decode_with_ack_bundle() {
        // Construct a PATH return plaintext manually:
        // path_len=0x42 (2B hash, 2 hops), path=[0xAA,0xBB,0xCC,0xDD],
        // extra_type=0x03 (ACK), ack_hash=[1,2,3,4]
        let mut pt = [0u8; 64];
        pt[0] = 0x42; // path_len
        pt[1] = 0xAA;
        pt[2] = 0xBB; // hop 0 hash
        pt[3] = 0xCC;
        pt[4] = 0xDD; // hop 1 hash
        pt[5] = 0x03; // extra_type = ACK
        pt[6] = 0x01;
        pt[7] = 0x02;
        pt[8] = 0x03;
        pt[9] = 0x04; // ack_hash

        let rp = decode_path_return_plaintext(&pt, 10).unwrap();
        assert_eq!(rp.path_len_byte, 0x42);
        assert_eq!(rp.path_byte_count, 4);
        assert_eq!(&rp.path[..4], &[0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(rp.extra, PathExtra::Ack([1, 2, 3, 4]));
    }

    #[test]
    fn path_return_decode_no_ack() {
        // path_len=0x40 (2B hash, 0 hops), extra_type=0xFF + 4 random bytes
        let mut pt = [0u8; 8];
        pt[0] = 0x40; // path_len: 0 hops
        pt[1] = 0xFF; // extra_type = dummy
        pt[2..6].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);

        let rp = decode_path_return_plaintext(&pt, 6).unwrap();
        assert_eq!(rp.path_byte_count, 0);
        assert_eq!(rp.extra, PathExtra::None);
    }

    #[test]
    fn path_return_full_packet_round_trip() {
        let (a, b) = make_pair();
        let shared = b.ecdh_shared_secret(&a.pubkey); // b sends PATH to a

        // Build a PATH inner plaintext
        let mut inner_pt = [0u8; 16];
        inner_pt[0] = 0x42; // path_len: 2B-hash, 2 hops
        inner_pt[1] = 0x11;
        inner_pt[2] = 0x22; // hop 0
        inner_pt[3] = 0x33;
        inner_pt[4] = 0x44; // hop 1
        inner_pt[5] = 0x03; // extra = ACK
        inner_pt[6] = 0xAA;
        inner_pt[7] = 0xBB;
        inner_pt[8] = 0xCC;
        inner_pt[9] = 0xDD;
        let inner_len = 10;

        // Wrap in DM envelope (b→a)
        let mut pkt = [0u8; 256];
        let pkt_len = encode_dm_payload(
            &shared,
            a.pub_hash(),
            b.pub_hash(),
            &inner_pt[..inner_len],
            &mut pkt,
        );

        // Decode
        let mut dec_buf = [0u8; 256];
        let (dest, _src, rp) = decode_path_return(&shared, &pkt[..pkt_len], &mut dec_buf).unwrap();
        assert_eq!(dest, a.pub_hash());
        assert_eq!(rp.path_byte_count, 4);
        assert_eq!(&rp.path[..4], &[0x11, 0x22, 0x33, 0x44]);
        assert_eq!(rp.extra, PathExtra::Ack([0xAA, 0xBB, 0xCC, 0xDD]));
    }

    // ── UI send path (acceptance criterion: DM and GRP_TXT roundtrip) ────────
    //
    // These tests verify the codec invariants that `build_ui_dm` and
    // `build_ui_grp_txt` in firmware/src/main.rs rely on.  The firmware
    // functions are thin wrappers; the encode→decode roundtrip is the true gate.

    /// Simulate `build_ui_dm`: encode a UI-originated DM and verify the
    /// recipient can decrypt it and recover the original text + ACK hash.
    ///
    /// Acceptance criterion: "Composing and sending a DM from the touch UI
    /// enqueues a real encrypted DM frame to a provisioned contact."
    #[test]
    fn ui_send_dm_roundtrip() {
        let sender = Identity::from_seed([0x11u8; 32]); // "our" device
        let receiver = Identity::from_seed([0x42u8; 32]); // provisioned contact

        let shared_tx = sender.ecdh_shared_secret(&receiver.pubkey);
        let shared_rx = receiver.ecdh_shared_secret(&sender.pubkey);

        let text = b"hello from the compose screen";
        let timestamp: u32 = 0xDEAD_BEEF;
        let type_byte: u8 = 0;

        // ── Encode (mirrors build_ui_dm) ──────────────────────────────────
        let mut pt_buf = [0u8; 128];
        let pt_len = encode_txt_msg_plaintext(timestamp, 0, 0, text, &mut pt_buf);

        let mut dm_buf = [0u8; 253]; // frame buf minus 2-byte header
        let dm_len = encode_dm_payload(
            &shared_tx,
            receiver.pub_hash(),
            sender.pub_hash(),
            &pt_buf[..pt_len],
            &mut dm_buf,
        );

        // ACK hash computed by sender at transmit time.
        let expected_ack = compute_ack_hash(timestamp, type_byte, text, &sender.pubkey);

        // ── Decode (mirrors handle_dm) ────────────────────────────────────
        let mut dec_buf = [0u8; 256];
        let (dest_hash, src_hash, dec_len) =
            decode_dm_payload(&shared_rx, &dm_buf[..dm_len], &mut dec_buf).unwrap();

        assert_eq!(
            dest_hash,
            receiver.pub_hash(),
            "dest hash must route to receiver"
        );
        assert_eq!(src_hash, sender.pub_hash(), "src hash must identify sender");
        assert!(dec_len >= 5, "plaintext must include timestamp + type byte");

        let rx_ts = u32::from_le_bytes([dec_buf[0], dec_buf[1], dec_buf[2], dec_buf[3]]);
        let rx_type = dec_buf[4];
        let text_region = &dec_buf[5..dec_len];
        // Trim AES zero-padding (mirrors firmware c_str helper).
        let text_end = text_region
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(text_region.len());
        let rx_text = &text_region[..text_end];

        assert_eq!(rx_ts, timestamp, "timestamp must survive encrypt/decrypt");
        assert_eq!(rx_type, type_byte, "type byte must survive encrypt/decrypt");
        assert_eq!(rx_text, text, "text must survive encrypt/decrypt");

        // Receiver computes ACK keyed on SENDER pubkey (v1.15 §7.1).
        let rx_ack = compute_ack_hash(rx_ts, rx_type, rx_text, &sender.pubkey);
        assert_eq!(
            rx_ack, expected_ack,
            "ACK hash must agree — closes the DM+ACK loop"
        );
    }

    /// Simulate `build_ui_grp_txt`: encode a UI-originated group message and
    /// verify that a node with the channel secret can decrypt it.
    ///
    /// Acceptance criterion: "Group-message send enqueues a real GRP_TXT frame
    /// on the provisioned channel."
    #[test]
    fn ui_send_grp_txt_roundtrip() {
        let channel_secret = [0x6du8; 32]; // 'm' — same as HIL_TEST_CHANNEL_SECRET
        let text = b"hi from the group compose screen";
        let timestamp: u32 = 0xCAFE_BABE;

        // ── Encode (mirrors build_ui_grp_txt) ────────────────────────────
        let mut frame_payload = [0u8; 253];
        let payload_len =
            encode_grp_txt_var(&channel_secret, timestamp, 0, 0, text, &mut frame_payload);
        assert!(payload_len > 0, "encode must produce non-empty payload");

        // Channel hash must be the first byte of the payload.
        let ch = frame_payload[0];
        let expected_ch = channel_hash_var(&channel_secret);
        assert_eq!(
            ch, expected_ch,
            "channel hash in payload must match the channel secret"
        );

        // ── Decode (mirrors handle_grp_txt) ──────────────────────────────
        let mut pt_buf = [0u8; 256];
        let fields =
            decode_grp_txt_var(&channel_secret, &frame_payload[..payload_len], &mut pt_buf)
                .expect("GRP_TXT decode must succeed");

        assert_eq!(
            fields.timestamp, timestamp,
            "timestamp must survive encrypt/decrypt"
        );
        let rx_text = &pt_buf[fields.text_offset..fields.text_offset + fields.text_len];
        // Trim AES zero-padding.
        let text_end = rx_text
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(rx_text.len());
        assert_eq!(
            &rx_text[..text_end],
            text,
            "text must survive encrypt/decrypt"
        );
    }

    /// Policy guard: an outbound DM to an unknown contact must be silently
    /// suppressed — the send path checks `policy.contact_pubkey(hash)` before
    /// encoding any frame.
    ///
    /// Acceptance criterion: "Policy invariants preserved (allowlist-only, no advert)."
    #[test]
    fn ui_send_unknown_contact_returns_none_pubkey() {
        use crate::PolicyFilter;
        let mut policy = PolicyFilter::new();
        // Register only contact 0x11; attempt to send to 0x42.
        let known_pk = {
            let mut pk = [0u8; 32];
            pk[0] = 0x11;
            pk
        };
        policy.add_contact(&known_pk, false);

        // Unknown hash → None → the send arm logs warn and enqueues nothing.
        assert!(
            policy.contact_pubkey(0x42).is_none(),
            "contact_pubkey must return None for unknown hash — enforces allowlist-only TX"
        );
        // Known hash → Some → the send arm proceeds to build_ui_dm.
        assert!(
            policy.contact_pubkey(0x11).is_some(),
            "contact_pubkey must return Some for a provisioned contact"
        );
    }

    /// Wire-interop guard for the private-channel TX/RX defect.
    ///
    /// Stock MeshCore (`Utils::encryptThenMAC` @ dee3e26a) ALWAYS keys the
    /// channel HMAC on `PUB_KEY_SIZE` (32) bytes of the channel secret buffer,
    /// regardless of the channel's logical key length. MeshCadet keys a 128-bit
    /// channel's HMAC on the 16-byte secret slice. These interoperate ONLY because
    /// (a) a 128-bit channel zero-fills `secret[16..32]`, and (b) HMAC right-pads
    /// any sub-block key to the 64-byte block size — so a 16-byte key and a
    /// `[key || 16 zero bytes]` 32-byte key hash to the identical padded key.
    ///
    /// This is exactly the path the fix wires up: a provisioned 128-bit channel
    /// (`key_len = 16`, `secret[16..32] == 0`). The test proves a frame MeshCadet
    /// encodes with the 16-byte key is byte-accepted by a peer applying MeshCore's
    /// fixed 32-byte HMAC convention. If anyone "simplifies" the HMAC keying and
    /// breaks this equivalence, channel interop silently dies again.
    #[test]
    fn grp_txt_128bit_interops_with_meshcore_32byte_hmac_convention() {
        // A provisioned 128-bit channel: 16-byte key in [0..16], zero-padded.
        let mut secret32 = [0u8; 32];
        secret32[..16].copy_from_slice(&[0xA3u8; 16]);
        let key16 = &secret32[..16];

        // MeshCadet encodes with the 16-byte slice (its 128-bit convention).
        let mut frame = [0u8; 256];
        let n = encode_grp_txt_var(key16, 0x1234_5678, 0, 0, b"channel hello", &mut frame);

        // A stock-MeshCore peer keys the HMAC on the full 32-byte (zero-padded)
        // secret buffer. Decoding under that 32-byte key must still succeed.
        let mut pt_buf = [0u8; 256];
        let fields = decode_grp_txt_var(&secret32, &frame[..n], &mut pt_buf)
            .expect("MeshCore 32-byte-HMAC convention must accept MeshCadet's 16-byte-keyed frame");
        assert_eq!(fields.timestamp, 0x1234_5678);
        let rx = &pt_buf[fields.text_offset..fields.text_offset + fields.text_len];
        let end = rx.iter().position(|&b| b == 0).unwrap_or(rx.len());
        assert_eq!(&rx[..end], b"channel hello");
    }

    /// Channel identity == secret: a frame encoded for one provisioned channel
    /// must NOT decode under a different channel secret, and the on-air
    /// `channel_hash` must differ. This is the invariant the production defect
    /// violated — the device used a hardcoded `[0x6d;32]` secret instead of the
    /// provisioned one, so its channel_hash never matched the real channel (RX
    /// gate dropped every inbound packet; companions ignored every outbound one).
    #[test]
    fn grp_txt_wrong_channel_secret_breaks_hash_and_decode() {
        let provisioned = [0x11u8; 32];
        let hardcoded = [0x6du8; 32]; // mirrors the old HIL_TEST_CHANNEL_SECRET

        assert_ne!(
            channel_hash_var(&provisioned),
            channel_hash_var(&hardcoded),
            "distinct channel secrets must yield distinct on-air channel hashes",
        );

        let mut frame = [0u8; 256];
        let n = encode_grp_txt(&provisioned, 1, 0, 0, b"real channel", &mut frame);

        let mut pt = [0u8; 256];
        assert_eq!(
            decode_grp_txt(&hardcoded, &frame[..n], &mut pt),
            Err(CodecError::MacMismatch),
            "a frame for the provisioned channel must not decode under the wrong secret",
        );
    }

    /// Advert-guard: `PolicyFilter::is_advert_type` must classify TXT_MSG (0x02)
    /// and GRP_TXT (0x05) as non-advert so the TX path allows them through.
    ///
    /// Acceptance criterion: "Policy invariants preserved … no advert."
    #[test]
    fn ui_send_frame_types_pass_advert_guard() {
        use crate::PolicyFilter;
        // UI send produces TXT_MSG (0x02) for DMs and GRP_TXT (0x05) for group.
        assert!(
            !PolicyFilter::is_advert_type(0x02),
            "TXT_MSG (DM) must NOT be classified as advert"
        );
        assert!(
            !PolicyFilter::is_advert_type(0x05),
            "GRP_TXT must NOT be classified as advert"
        );
    }
}
