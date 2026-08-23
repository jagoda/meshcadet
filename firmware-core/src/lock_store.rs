// SPDX-License-Identifier: GPL-3.0-only
//! Screen-lock PIN store — blob codec for the dedicated `mc_lock` NVS
//! namespace.
//!
//! Plan D2 rejects folding the lock PIN into `ProvisionedConfig` (that would
//! have required a `CFG_VERSION` v0x04 bump, growing `MAX_BLOB_LEN` and
//! `size_of::<ProvisionedConfig>()`, and dragging the host CLI /
//! `site/provisioner/codec.js` blob decoders along with it) and rejects
//! folding it into `mc_rts` (that store's sole writer is the UI thread —
//! see `runtime_settings_store`'s doc — but the lock PIN is written by
//! `admin_server` over USB, a different thread). It gets its own namespace
//! instead, the same shape `advert_ts_store.rs` and `gps_baud_store.rs`
//! already use for other `admin_server`-owned single values (see
//! `checklists/meshcadet-firmware-dispatcher-stateful-feature.md`).
//!
//! This module is the pure byte-slice codec (`serialize`/`deserialize`)
//! only. The `EspNvs` read/write wrapper for NVS namespace `mc_lock`, key
//! `lock_blob` is a later phase's job (`firmware::lock_store` — phase 7 of
//! the `meshcadet-screen-lock` campaign) — it needs a real NVS partition and
//! will `pub use firmware_core::lock_store::*;` to re-export this pure half,
//! the same shape `runtime_settings_store`/`config_store` already use, so
//! its tests execute under `cargo test --workspace` (this crate is a
//! detached, cross-compiled workspace — see `Cargo.toml`'s doc comment — so
//! a `#[cfg(test)]` block written there would type-check but never run).
//! See `docs/adr/0005-firmware-core-extraction.md`.
//!
//! # Blob layout
//!
//! ```text
//! byte 0        version = 0x01
//! byte 1        pin_len   (0 ⇒ no PIN set; otherwise always LOCK_PIN_LEN)
//! bytes 2..2+N  pin       (LOCK_PIN_LEN bytes, zero-padded; N = LOCK_PIN_LEN)
//! ```
//!
//! Distinct from `pin_menu`'s admin PIN (`MAX_PIN_LEN` = 16, variable
//! length, constant-time-compared by `pin_menu::verify_pin`): the lock PIN
//! is always exactly `protocol::provisioning::LOCK_PIN_LEN` (4) ASCII-digit
//! bytes, enforced at the decode path by
//! `protocol::provisioning::decode_set_lock_pin` — this codec stores
//! whatever `LOCK_PIN_LEN`-byte value it's given without re-validating
//! content, mirroring how `config_store` stores the admin PIN it's handed.
//! The comparison itself (`ui::lock::attempt_unlock`) takes the result of
//! that comparison as a plain `bool`, so it stays decoupled from this
//! codec's storage shape entirely.

use protocol::provisioning::LOCK_PIN_LEN;

const VERSION: u8 = 0x01;
/// `version(1) + pin_len(1) + pin(LOCK_PIN_LEN)`.
pub const BLOB_LEN: usize = 2 + LOCK_PIN_LEN;

/// Serialize `(pin, pin_len)` into `out` (must be at least [`BLOB_LEN`]
/// bytes long). `pin_len == 0` means "no PIN set" — `pin`'s bytes are still
/// written (callers typically pass all-zero in that case) but are ignored on
/// read. Returns bytes written (always [`BLOB_LEN`]).
pub fn serialize(pin: &[u8; LOCK_PIN_LEN], pin_len: u8, out: &mut [u8]) -> usize {
    out[0] = VERSION;
    out[1] = pin_len;
    out[2..2 + LOCK_PIN_LEN].copy_from_slice(pin);
    BLOB_LEN
}

/// Verify an entered PIN attempt against the stored screen-lock PIN.
///
/// This is a SEPARATE comparison from `pin_menu::verify_pin` — the lock PIN
/// is a distinct secret from the admin-menu PIN (screen-lock plan, "the lock
/// PIN is verified against the boot-seeded lock PIN, not the admin PIN");
/// keeping this as its own function, sized to [`LOCK_PIN_LEN`] rather than
/// `pin_menu::MAX_PIN_LEN`, means the two PINs are never compared through a
/// shared fixed-width buffer where a defect in one could bleed into the
/// other. Constant-time, mirroring `pin_menu::verify_pin`'s discipline
/// exactly (no early return on mismatch, and any excess bytes in `entered`
/// still get OR'd in so a too-long attempt can't shortcut the comparison).
///
/// Returns `false` immediately if `stored_pin_len == 0` ("no lock PIN
/// configured" — mirrors `pin_menu::verify_pin`'s same convention).
pub fn verify(entered: &[u8], stored_pin: &[u8; LOCK_PIN_LEN], stored_pin_len: u8) -> bool {
    let slen = stored_pin_len as usize;
    if slen == 0 {
        return false;
    }
    let mut mismatch: u8 = if entered.len() != slen { 1 } else { 0 };
    for i in 0..LOCK_PIN_LEN {
        let a = if i < entered.len() { entered[i] } else { 0x00 };
        let b = stored_pin[i];
        mismatch |= a ^ b;
    }
    for &b in entered.iter().skip(slen) {
        mismatch |= b;
    }
    mismatch == 0
}

/// Deserialize a stored lock-PIN blob. Returns `None` on a short/malformed
/// blob or unrecognised version (first boot with the namespace never
/// written, or a corrupt blob) — callers treat `None` as "no PIN set",
/// exactly like `config_store`'s `is_provisioned` gate treats a missing
/// config blob.
///
/// A `pin_len` outside `0..=LOCK_PIN_LEN` is clamped to `0` ("no PIN set")
/// rather than trusted — defensive against a corrupt blob claiming a PIN
/// length this codec's fixed-width layout can't actually hold.
pub fn deserialize(blob: &[u8]) -> Option<([u8; LOCK_PIN_LEN], u8)> {
    if blob.len() < BLOB_LEN || blob[0] != VERSION {
        return None;
    }
    let mut pin = [0u8; LOCK_PIN_LEN];
    pin.copy_from_slice(&blob[2..2 + LOCK_PIN_LEN]);
    let raw_len = blob[1];
    let pin_len = if (raw_len as usize) <= LOCK_PIN_LEN {
        raw_len
    } else {
        0
    };
    Some((pin, pin_len))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_pin_and_len() {
        let pin = *b"1234";
        let mut blob = [0u8; BLOB_LEN];
        let n = serialize(&pin, LOCK_PIN_LEN as u8, &mut blob);
        assert_eq!(n, BLOB_LEN);
        let (restored_pin, restored_len) = deserialize(&blob[..n]).expect("valid blob");
        assert_eq!(restored_pin, pin);
        assert_eq!(restored_len, LOCK_PIN_LEN as u8);
    }

    #[test]
    fn roundtrip_no_pin_set() {
        let pin = [0u8; LOCK_PIN_LEN];
        let mut blob = [0u8; BLOB_LEN];
        let n = serialize(&pin, 0, &mut blob);
        let (_, restored_len) = deserialize(&blob[..n]).expect("valid blob");
        assert_eq!(restored_len, 0, "pin_len 0 means no PIN configured");
    }

    #[test]
    fn distinct_pins_round_trip_distinctly() {
        let mut blob = [0u8; BLOB_LEN];
        let n = serialize(b"0000", LOCK_PIN_LEN as u8, &mut blob);
        assert_eq!(deserialize(&blob[..n]).unwrap().0, *b"0000");
        let n = serialize(b"9876", LOCK_PIN_LEN as u8, &mut blob);
        assert_eq!(deserialize(&blob[..n]).unwrap().0, *b"9876");
    }

    #[test]
    fn blob_too_short_returns_none() {
        let short = [VERSION; BLOB_LEN - 1];
        assert!(deserialize(&short).is_none());
    }

    #[test]
    fn empty_blob_returns_none() {
        assert!(deserialize(&[]).is_none());
    }

    #[test]
    fn wrong_version_returns_none() {
        let mut blob = [0u8; BLOB_LEN];
        blob[0] = 0xFF;
        blob[1] = LOCK_PIN_LEN as u8;
        assert!(deserialize(&blob).is_none());
    }

    /// Defensive: a corrupt blob claiming a `pin_len` this fixed-width
    /// layout cannot hold must not be trusted — treated as "no PIN set"
    /// rather than read out-of-bounds or silently truncated.
    #[test]
    fn out_of_range_pin_len_clamped_to_zero() {
        let mut blob = [0u8; BLOB_LEN];
        blob[0] = VERSION;
        blob[1] = 255;
        let (_, restored_len) = deserialize(&blob).expect("length prefix, not the blob, is bad");
        assert_eq!(restored_len, 0);
    }

    #[test]
    fn in_range_pin_len_values_all_accepted() {
        let mut blob = [0u8; BLOB_LEN];
        blob[0] = VERSION;
        for len in 0..=(LOCK_PIN_LEN as u8) {
            blob[1] = len;
            let (_, restored_len) = deserialize(&blob).expect("valid blob");
            assert_eq!(restored_len, len);
        }
    }

    // ── verify (lock-PIN comparison — distinct from pin_menu::verify_pin) ──

    #[test]
    fn verify_correct_pin_accepted() {
        assert!(verify(b"1234", b"1234", LOCK_PIN_LEN as u8));
    }

    #[test]
    fn verify_wrong_pin_rejected() {
        assert!(!verify(b"5678", b"1234", LOCK_PIN_LEN as u8));
    }

    #[test]
    fn verify_no_pin_set_always_rejected() {
        assert!(!verify(b"1234", &[0u8; LOCK_PIN_LEN], 0));
        assert!(!verify(b"", &[0u8; LOCK_PIN_LEN], 0));
    }

    #[test]
    fn verify_shorter_or_longer_entry_rejected() {
        assert!(!verify(b"123", b"1234", LOCK_PIN_LEN as u8));
        assert!(!verify(b"12345", b"1234", LOCK_PIN_LEN as u8));
    }

    #[test]
    fn verify_single_digit_mismatch_rejected() {
        assert!(!verify(b"1235", b"1234", LOCK_PIN_LEN as u8));
    }
}
