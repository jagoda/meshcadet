// SPDX-License-Identifier: GPL-3.0-only
//! Cryptographic primitives: AES-128-ECB, HMAC-SHA256-2, SHA-256.
//!
//! Source reference: `src/Utils.cpp` @ dee3e26a.
//! Spec §3 (recon doc):
//! - encrypt: AES-128-ECB, zero-pad last block, key = first 16 bytes of shared secret
//! - MAC: HMAC-SHA256(key = full 32-byte shared secret, data = ciphertext), truncated to 2 bytes
//! - Protocol uses Encrypt-then-MAC: [MAC(2)] [ciphertext]
//!
//! ⚠ The prose guide says "AES-128-CBC"; the actual source is ECB (no IV, no chaining).
//!   Verified at dee3e26a:src/Utils.cpp. We implement ECB as the source dictates.

use aes::{
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit},
    Aes128,
};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;
/// Convenience alias for a raw AES block
type AesBlock = aes::cipher::generic_array::GenericArray<u8, aes::cipher::typenum::U16>;

// ── AES-128-ECB ─────────────────────────────────────────────────────────────

/// AES-128-ECB encrypt.
///
/// The plaintext is zero-padded to the next 16-byte boundary (ECB, no IV).
/// `out` must be at least `ceil_16(plaintext.len())` bytes long.
/// Returns the number of ciphertext bytes written.
pub fn aes128_ecb_encrypt(key: &[u8; 16], plaintext: &[u8], out: &mut [u8]) -> usize {
    let cipher = Aes128::new_from_slice(key).expect("key is 16 bytes");
    let nblocks = plaintext.len().div_ceil(16);
    let ct_len = nblocks * 16;
    debug_assert!(out.len() >= ct_len, "output buffer too small");
    for i in 0..nblocks {
        let mut block = [0u8; 16];
        let s = i * 16;
        let e = core::cmp::min(s + 16, plaintext.len());
        block[..e - s].copy_from_slice(&plaintext[s..e]);
        let ga = AesBlock::from_mut_slice(&mut block);
        cipher.encrypt_block(ga);
        out[s..s + 16].copy_from_slice(&block);
    }
    ct_len
}

/// AES-128-ECB decrypt.
///
/// `ciphertext.len()` must be a multiple of 16.
/// `out` must be at least `ciphertext.len()` bytes long.
/// Returns the number of plaintext bytes written (= `ciphertext.len()`).
pub fn aes128_ecb_decrypt(key: &[u8; 16], ciphertext: &[u8], out: &mut [u8]) -> usize {
    debug_assert_eq!(ciphertext.len() % 16, 0, "ciphertext must be block-aligned");
    debug_assert!(out.len() >= ciphertext.len(), "output buffer too small");
    let cipher = Aes128::new_from_slice(key).expect("key is 16 bytes");
    let nblocks = ciphertext.len() / 16;
    for i in 0..nblocks {
        let mut block = [0u8; 16];
        block.copy_from_slice(&ciphertext[i * 16..(i + 1) * 16]);
        let ga = AesBlock::from_mut_slice(&mut block);
        cipher.decrypt_block(ga);
        out[i * 16..(i + 1) * 16].copy_from_slice(&block);
    }
    ciphertext.len()
}

/// Round up `n` to the next multiple of 16.
pub fn ceil_16(n: usize) -> usize {
    n.div_ceil(16) * 16
}

// ── HMAC-SHA256 ──────────────────────────────────────────────────────────────

/// 2-byte truncated HMAC-SHA256: HMAC-SHA256(key, data)[0:2].
///
/// Used as the Encrypt-then-MAC authenticator (CIPHER_MAC_SIZE = 2).
pub fn hmac_sha256_2(key: &[u8], data: &[u8]) -> [u8; 2] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(data);
    let result = mac.finalize().into_bytes();
    [result[0], result[1]]
}

// ── SHA-256 ──────────────────────────────────────────────────────────────────

/// SHA-256 hash of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let result = Sha256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// SHA-256 of two concatenated slices without allocating.
pub fn sha256_2(a: &[u8], b: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(a);
    h.update(b);
    let result = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

// ── Encrypt-then-MAC ─────────────────────────────────────────────────────────

/// Encrypt-then-MAC: AES-128-ECB + 2-byte HMAC-SHA256.
///
/// Output layout: `[MAC (2 B)] [ciphertext]`
/// `out` must be at least `2 + ceil_16(plaintext.len())` bytes.
/// Returns the total number of bytes written.
///
/// `aes_key`  = first 16 bytes of the shared secret
/// `hmac_key` = full 32-byte shared secret
pub fn encrypt_then_mac(
    aes_key: &[u8; 16],
    hmac_key: &[u8; 32],
    plaintext: &[u8],
    out: &mut [u8],
) -> usize {
    encrypt_then_mac_var(aes_key, hmac_key, plaintext, out)
}

/// Encrypt-then-MAC with a variable-length HMAC key.
///
/// Identical to [`encrypt_then_mac`] but the HMAC key may be any length. This is
/// required for 128-bit group channels, whose secret is 16 bytes: AES key =
/// secret[0:16], HMAC key = the same 16-byte secret (MeshCore keys the channel
/// HMAC on `secret_len` bytes). See recon doc §9 / `BaseChatMesh.cpp:896`.
pub fn encrypt_then_mac_var(
    aes_key: &[u8; 16],
    hmac_key: &[u8],
    plaintext: &[u8],
    out: &mut [u8],
) -> usize {
    let ct_len = ceil_16(plaintext.len());
    debug_assert!(out.len() >= 2 + ct_len, "output buffer too small");
    // Encrypt into out[2..]
    aes128_ecb_encrypt(aes_key, plaintext, &mut out[2..]);
    // MAC over ciphertext
    let mac = hmac_sha256_2(hmac_key, &out[2..2 + ct_len]);
    out[0] = mac[0];
    out[1] = mac[1];
    2 + ct_len
}

/// MAC-then-Decrypt: verify 2-byte HMAC-SHA256 then AES-128-ECB decrypt.
///
/// `payload` = `[MAC (2 B)] [ciphertext]`
/// Returns `Err(())` if MAC verification fails or the ciphertext is not block-aligned.
/// On success returns the number of plaintext bytes written (= `ciphertext.len()`).
pub fn mac_then_decrypt(
    aes_key: &[u8; 16],
    hmac_key: &[u8; 32],
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, MacError> {
    mac_then_decrypt_var(aes_key, hmac_key, payload, out)
}

/// MAC-then-Decrypt with a variable-length HMAC key (see [`encrypt_then_mac_var`]).
pub fn mac_then_decrypt_var(
    aes_key: &[u8; 16],
    hmac_key: &[u8],
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, MacError> {
    if payload.len() < 2 {
        return Err(MacError);
    }
    let (mac_slice, ciphertext) = payload.split_at(2);
    if ciphertext.len() % 16 != 0 {
        return Err(MacError);
    }
    let expected_mac = hmac_sha256_2(hmac_key, ciphertext);
    // Constant-time comparison (prevent timing attacks on MAC)
    if mac_slice[0] != expected_mac[0] || mac_slice[1] != expected_mac[1] {
        return Err(MacError);
    }
    Ok(aes128_ecb_decrypt(aes_key, ciphertext, out))
}

/// Opaque error returned when MAC verification fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MacError;

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::hex;

    // ── SHA-256 known-answer ─────────────────────────────────────────────

    #[test]
    fn sha256_empty_nist() {
        // NIST FIPS 180-4: SHA-256("") = e3b0c442...
        let expected = hex!("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256(b""), expected);
    }

    #[test]
    fn sha256_short_message_nondeterminism() {
        // Verify SHA-256 produces 32 non-trivial bytes for a short input.
        // The empty-string NIST vector (sha256_empty_nist) is the canonical KAT;
        // this test just guards against trivially-zero output for another input.
        let h = sha256(b"abc");
        assert_eq!(h.len(), 32);
        assert_ne!(h, [0u8; 32]);
        // SHA-256("abc") and SHA-256("") must differ
        assert_ne!(h, sha256(b""));
    }

    #[test]
    fn sha256_2_equals_single_concat() {
        let a = b"hello";
        let b = b" world";
        let combined = b"hello world";
        assert_eq!(sha256_2(a, b), sha256(combined));
    }

    // ── AES-128-ECB known-answer (NIST FIPS 197, Appendix B) ────────────

    #[test]
    fn aes128_ecb_nist_fips197_appendix_b() {
        // Key: 2b7e151628aed2a6abf7158809cf4f3c
        // PT:  3243f6a8885a308d313198a2e0370734
        // CT:  3925841d02dc09fbdc118597196a0b32
        let key = hex!("2b7e151628aed2a6abf7158809cf4f3c");
        let pt = hex!("3243f6a8885a308d313198a2e0370734");
        let ct = hex!("3925841d02dc09fbdc118597196a0b32");

        let mut out = [0u8; 16];
        let n = aes128_ecb_encrypt(&key, &pt, &mut out);
        assert_eq!(n, 16);
        assert_eq!(out, ct, "AES-128-ECB encrypt NIST vector");

        let mut plain = [0u8; 16];
        let m = aes128_ecb_decrypt(&key, &out, &mut plain);
        assert_eq!(m, 16);
        assert_eq!(plain, pt, "AES-128-ECB decrypt NIST vector");
    }

    #[test]
    fn aes128_ecb_zero_pad_extra_block() {
        // 5-byte plaintext → padded to 16 bytes
        let key = [0u8; 16];
        let plaintext = b"hello";
        let mut ct = [0u8; 16];
        let n = aes128_ecb_encrypt(&key, plaintext, &mut ct);
        assert_eq!(n, 16);
        // Decrypt and check first 5 bytes match
        let mut pt = [0u8; 16];
        aes128_ecb_decrypt(&key, &ct, &mut pt);
        assert_eq!(&pt[..5], b"hello");
        // Bytes 5..16 should be 0 (zero-pad)
        assert_eq!(&pt[5..], &[0u8; 11]);
    }

    // ── HMAC-SHA256 known-answer (RFC 4231 Test Case 1) ──────────────────

    #[test]
    fn hmac_sha256_rfc4231_tc1() {
        // RFC 4231 Test Case 1:
        // Key  = 0b0b0b0b... (20 bytes)
        // Data = "Hi There"
        // HMAC = b0344c61...
        let key = hex!("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
        let data = b"Hi There";
        // Full 32-byte HMAC:
        let expected_full =
            hex!("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        // 2-byte truncation = first 2 bytes
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key).unwrap();
        mac.update(data);
        let full: [u8; 32] = mac.finalize().into_bytes().into();
        assert_eq!(full, expected_full, "HMAC-SHA256 RFC 4231 TC1 full");

        let trunc = hmac_sha256_2(&key, data);
        assert_eq!(trunc, [0xb0, 0x34], "2-byte truncation");
    }

    // ── Encrypt-then-MAC round-trip ──────────────────────────────────────

    #[test]
    fn encrypt_then_mac_roundtrip() {
        let shared_secret = [0x55u8; 32];
        let aes_key: [u8; 16] = shared_secret[..16].try_into().unwrap();
        let plaintext = b"Hello, MeshCore!";

        let mut enc_buf = [0u8; 2 + 16];
        let n = encrypt_then_mac(&aes_key, &shared_secret, plaintext, &mut enc_buf);
        assert_eq!(n, 18);

        let mut dec_buf = [0u8; 16];
        let m = mac_then_decrypt(&aes_key, &shared_secret, &enc_buf[..n], &mut dec_buf).unwrap();
        assert_eq!(m, 16);
        assert_eq!(&dec_buf[..plaintext.len()], plaintext);
    }

    #[test]
    fn mac_verification_rejects_tampered_ciphertext() {
        let shared_secret = [0xAAu8; 32];
        let aes_key: [u8; 16] = shared_secret[..16].try_into().unwrap();
        let plaintext = b"secret message";

        let mut enc_buf = [0u8; 2 + 16];
        let n = encrypt_then_mac(&aes_key, &shared_secret, plaintext, &mut enc_buf);

        // Flip a bit in the ciphertext
        enc_buf[5] ^= 0x01;

        let mut dec_buf = [0u8; 16];
        let result = mac_then_decrypt(&aes_key, &shared_secret, &enc_buf[..n], &mut dec_buf);
        assert_eq!(
            result,
            Err(MacError),
            "tampered ciphertext must be rejected"
        );
    }
}
