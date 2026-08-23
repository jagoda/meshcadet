// SPDX-License-Identifier: GPL-3.0-only
//! On-device admin-menu `RuntimeSettings` store — blob codec.
//!
//! This module is the pure byte-slice codec (`serialize`/`deserialize`) plus
//! the first-boot-seeding helper (`fallback_settings`) for the on-device
//! admin-menu `RuntimeSettings` blob. The `EspNvs` read/write wrapper
//! (`load`/`save`) stays in `firmware::runtime_settings_store` — it needs a
//! real NVS partition — and re-exports this module via `pub use
//! firmware_core::runtime_settings_store::*;` so its tests execute under
//! `cargo test --workspace` (this crate is a detached, cross-compiled
//! workspace — see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block
//! written there would type-check but never run).
//! See `docs/adr/0005-firmware-core-extraction.md`.
//!
//! Deliberately a SEPARATE NVS namespace from `config_store`'s `ProvisionedConfig`
//! blob, and a separate Rust type from `pin_menu::RuntimeSettings` is owned only
//! here (not folded into `ProvisionedConfig`).
//!
//! # Why a separate store, not a field on `ProvisionedConfig`
//!
//! `ProvisionedConfig` is the admin_server thread's single mutable source of
//! truth — it is loaded once at boot, moved into the `admin_server` thread, and
//! every mutation (`SET_*` frames) is applied to that thread-local copy and
//! persisted from that same thread (see `admin_server.rs`). The on-device
//! admin-menu screen runs on the UI/main thread and has no access to that
//! moved copy. Round-tripping the on-device toggle through
//! `config_store::load_provisioned_config` / `save_provisioned_config` would
//! read-modify-write the SAME blob a second, independent thread already owns —
//! a write from one thread can be silently clobbered by a stale in-memory copy
//! flushed from the other. Giving `RuntimeSettings` its own namespace/blob
//! removes that race entirely: the UI thread is the sole writer here — screen-
//! lock config (plan D2) rides this exact mechanism, forwarded from
//! `admin_server` to the UI thread as a `UiEvent` for host/provisioner writes.
//!
//! # Blob layout
//!
//! Stored (by `firmware::runtime_settings_store::save`) under NVS namespace
//! `mc_rts`, key `rts_blob`. Current version `0x02` (bumped from `0x01` when
//! the screen-lock idle timeout was added):
//!
//! ```text
//! byte 0          version = 0x02
//! byte 1          notif_visual         (0/1)
//! byte 2          notif_audible        (0/1)
//! byte 3          contact_count        (0..=MAX_CONTACTS)
//! byte 4          lock_flags           (bitfield, see LOCK_* in protocol::provisioning)
//! bytes 5..5+N    contact_telemetry    (0/1 per slot, N = MAX_CONTACTS)
//! byte 5+N        screen_sleep_timeout_s (0..=120)
//! bytes 6+N..8+N  lock_timeout_s       (u16 little-endian, 15..=3600; ADDED in v0x02)
//! ```
//!
//! # Backward compatibility (v0x01 → v0x02: screen-lock idle timeout)
//!
//! A device already in the field with a `VERSION_V1` (`0x01`) blob stored
//! must not have its saved `notif_visual` / `notif_audible` /
//! `contact_telemetry` / `screen_sleep_timeout_s` prefs reset to defaults
//! just because firmware grew one more field — this is the same
//! field-device-data-loss concern the original screen-sleep-timeout addition
//! (still present below) already solved once. `deserialize` keeps a
//! dedicated `VERSION_V1` reader that accepts BOTH `V1_LEGACY_BLOB_LEN`
//! (pre-screen-sleep) and `V1_BLOB_LEN` (with the screen-sleep byte, no lock
//! timeout) lengths, defaulting ONLY `lock_timeout_s` (to
//! `LOCK_TIMEOUT_DEFAULT_S`) — never touching any field an old blob actually
//! stored. A `VERSION` (`0x02`) blob is always the full current length; a
//! v0x02 blob shorter than that is treated as corrupt (`None`), same as a
//! too-short v0x01 blob always has been.
//!
//! The `lock_flags` byte (position 4) already exists in both versions — the
//! screen-lock enable bit (`LOCK_SCREEN_ENABLE`, bit 0) rides that pre-
//! existing field, so no new flags byte was needed, only the new
//! `lock_timeout_s` field.

use crate::pin_menu::{RuntimeSettings, MAX_CONTACTS, SCREEN_SLEEP_DEFAULT_S, SCREEN_SLEEP_MAX_S};
use protocol::provisioning::{LOCK_TIMEOUT_DEFAULT_S, LOCK_TIMEOUT_MAX_S, LOCK_TIMEOUT_MIN_S};

/// Current blob version. Bumped `0x01` → `0x02` when `lock_timeout_s` was
/// added — see the module-level "Backward compatibility" note for why the
/// version tag moved this time (unlike the screen-sleep-timeout addition,
/// which stayed additive-within-`0x01`).
const VERSION: u8 = 0x02;
/// The prior blob version, still accepted on read (never written).
const VERSION_V1: u8 = 0x01;

/// Pre-screen-sleep `VERSION_V1` blob length (original layout, no timeout
/// byte, no lock-timeout field).
const V1_LEGACY_BLOB_LEN: usize = 5 + MAX_CONTACTS;
/// `VERSION_V1` blob length once `screen_sleep_timeout_s` was appended
/// (still version `0x01` — that addition was additive-within-version).
const V1_BLOB_LEN: usize = V1_LEGACY_BLOB_LEN + 1;
/// Current (`VERSION` / `0x02`) blob length: the v0x01 layout plus a
/// trailing 2-byte little-endian `lock_timeout_s`.
pub const BLOB_LEN: usize = V1_BLOB_LEN + 2;

/// Pure helper: `RuntimeSettings::default_enabled()` with the notification
/// toggles overridden to the provisioned `(visual, audible)` defaults.
/// Factored out of `firmware::runtime_settings_store::load` so the
/// first-boot seeding contract is unit testable without an NVS partition
/// (mirrors [`serialize`]/[`deserialize`] below).
pub fn fallback_settings(notif_defaults: (bool, bool)) -> RuntimeSettings {
    let mut s = RuntimeSettings::default_enabled();
    s.notif_visual = notif_defaults.0;
    s.notif_audible = notif_defaults.1;
    s
}

pub fn serialize(s: &RuntimeSettings, out: &mut [u8]) -> usize {
    out[0] = VERSION;
    out[1] = s.notif_visual as u8;
    out[2] = s.notif_audible as u8;
    out[3] = s.contact_count;
    out[4] = s.lock_flags;
    for i in 0..MAX_CONTACTS {
        out[5 + i] = s.contact_telemetry[i] as u8;
    }
    out[V1_LEGACY_BLOB_LEN] = s.screen_sleep_timeout_s.min(SCREEN_SLEEP_MAX_S);
    let lock_timeout_s = s
        .lock_timeout_s
        .clamp(LOCK_TIMEOUT_MIN_S, LOCK_TIMEOUT_MAX_S);
    let [lo, hi] = lock_timeout_s.to_le_bytes();
    out[V1_BLOB_LEN] = lo;
    out[V1_BLOB_LEN + 1] = hi;
    BLOB_LEN
}

/// Parse the fields common to every version (bytes `0..V1_LEGACY_BLOB_LEN`),
/// given `blob.len() >= V1_LEGACY_BLOB_LEN` already holds. Callers layer
/// their own version-specific fields (screen-sleep timeout, lock timeout) on
/// top of the returned `RuntimeSettings::default_enabled()` base.
fn parse_common_fields(blob: &[u8]) -> RuntimeSettings {
    let mut s = RuntimeSettings::default_enabled();
    s.notif_visual = blob[1] != 0;
    s.notif_audible = blob[2] != 0;
    s.contact_count = blob[3];
    s.lock_flags = blob[4];
    for i in 0..MAX_CONTACTS {
        s.contact_telemetry[i] = blob[5 + i] != 0;
    }
    s
}

pub fn deserialize(blob: &[u8]) -> Option<RuntimeSettings> {
    if blob.is_empty() {
        return None;
    }
    match blob[0] {
        VERSION => {
            // Current version is always the full length — a short v0x02
            // blob is corrupt, not a legitimate in-field-upgrade shape (that
            // shape is exactly what the VERSION_V1 arm below exists for).
            if blob.len() < BLOB_LEN {
                return None;
            }
            let mut s = parse_common_fields(blob);
            s.screen_sleep_timeout_s = blob[V1_LEGACY_BLOB_LEN].min(SCREEN_SLEEP_MAX_S);
            let raw = u16::from_le_bytes([blob[V1_BLOB_LEN], blob[V1_BLOB_LEN + 1]]);
            s.lock_timeout_s = raw.clamp(LOCK_TIMEOUT_MIN_S, LOCK_TIMEOUT_MAX_S);
            Some(s)
        }
        VERSION_V1 => {
            // Accept both legacy v0x01 lengths so an in-field upgrade never
            // resets previously-saved notif/telemetry/screen-sleep prefs —
            // see the module-level "Backward compatibility" note. Only the
            // NEW field (lock_timeout_s, which v0x01 never had) is defaulted.
            if blob.len() < V1_LEGACY_BLOB_LEN {
                return None;
            }
            let mut s = parse_common_fields(blob);
            s.screen_sleep_timeout_s = if blob.len() >= V1_BLOB_LEN {
                blob[V1_LEGACY_BLOB_LEN].min(SCREEN_SLEEP_MAX_S)
            } else {
                // Old-length blob predates this field — fall back to the documented default.
                SCREEN_SLEEP_DEFAULT_S
            };
            s.lock_timeout_s = LOCK_TIMEOUT_DEFAULT_S;
            Some(s)
        }
        _ => None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Pure functions over byte slices — no NVS/hardware required. These now
// EXECUTE under `cargo test --workspace` (this module lives in `firmware-
// core`, a root-workspace member — see `Cargo.toml`'s doc comment — unlike
// the detached `firmware/` workspace these tests used to type-check-only in).
#[cfg(test)]
mod tests {
    use super::*;

    // ── fallback_settings (first-boot notif-defaults seeding) ───────────────

    /// DEFECT-FIX acceptance: a freshly-provisioned device (no runtime-settings
    /// blob saved yet) must seed its notification toggles from the admin's
    /// provisioning-time `SET_NOTIF_DEFAULTS` value, not a hardcoded true/true.
    #[test]
    fn fallback_settings_uses_provisioned_notif_defaults() {
        let s = fallback_settings((false, true));
        assert!(!s.notif_visual);
        assert!(s.notif_audible);
    }

    #[test]
    fn fallback_settings_both_off() {
        let s = fallback_settings((false, false));
        assert!(!s.notif_visual);
        assert!(!s.notif_audible);
    }

    /// Every other field still comes from `RuntimeSettings::default_enabled()`
    /// — only the two notif toggles are overridden.
    #[test]
    fn fallback_settings_preserves_other_defaults() {
        let s = fallback_settings((false, false));
        assert_eq!(s.contact_count, 0);
        assert_eq!(s.lock_flags, 0);
        assert_eq!(s.screen_sleep_timeout_s, SCREEN_SLEEP_DEFAULT_S);
        assert_eq!(s.lock_timeout_s, LOCK_TIMEOUT_DEFAULT_S);
        for &v in &s.contact_telemetry {
            assert!(!v);
        }
    }

    #[test]
    fn roundtrip_preserves_screen_sleep_timeout() {
        let mut s = RuntimeSettings::default_enabled();
        s.screen_sleep_timeout_s = 45;
        let mut blob = [0u8; BLOB_LEN];
        let n = serialize(&s, &mut blob);
        let restored = deserialize(&blob[..n]).expect("valid blob");
        assert_eq!(restored.screen_sleep_timeout_s, 45);
    }

    #[test]
    fn roundtrip_preserves_zero_sentinel() {
        let mut s = RuntimeSettings::default_enabled();
        s.screen_sleep_timeout_s = 0;
        let mut blob = [0u8; BLOB_LEN];
        let n = serialize(&s, &mut blob);
        let restored = deserialize(&blob[..n]).expect("valid blob");
        assert_eq!(restored.screen_sleep_timeout_s, 0);
    }

    // ── lock_timeout_s round-trip (v0x02) ───────────────────────────────────

    #[test]
    fn roundtrip_preserves_lock_timeout() {
        let mut s = RuntimeSettings::default_enabled();
        s.lock_timeout_s = 900;
        let mut blob = [0u8; BLOB_LEN];
        let n = serialize(&s, &mut blob);
        assert_eq!(n, BLOB_LEN);
        assert_eq!(
            blob[0], VERSION,
            "serialize always writes the current version"
        );
        let restored = deserialize(&blob[..n]).expect("valid blob");
        assert_eq!(restored.lock_timeout_s, 900);
    }

    #[test]
    fn roundtrip_preserves_lock_timeout_bounds() {
        let mut s = RuntimeSettings::default_enabled();
        s.lock_timeout_s = LOCK_TIMEOUT_MIN_S;
        let mut blob = [0u8; BLOB_LEN];
        let n = serialize(&s, &mut blob);
        assert_eq!(
            deserialize(&blob[..n]).unwrap().lock_timeout_s,
            LOCK_TIMEOUT_MIN_S
        );

        s.lock_timeout_s = LOCK_TIMEOUT_MAX_S;
        let n = serialize(&s, &mut blob);
        assert_eq!(
            deserialize(&blob[..n]).unwrap().lock_timeout_s,
            LOCK_TIMEOUT_MAX_S
        );
    }

    /// Acceptance: an old-length blob (pre-screen-sleep firmware) must not
    /// reset `notif_visual`/`notif_audible` to defaults — it must fall back
    /// ONLY the new field to `SCREEN_SLEEP_DEFAULT_S`, preserving everything
    /// that old blob actually stored.
    #[test]
    fn old_length_blob_preserves_existing_fields_and_defaults_timeout() {
        let mut old_blob = [0u8; V1_LEGACY_BLOB_LEN];
        old_blob[0] = VERSION_V1;
        old_blob[1] = 0; // notif_visual = false (non-default, proves it round-trips)
        old_blob[2] = 1; // notif_audible = true
        old_blob[3] = 2; // contact_count
        old_blob[4] = 0x07; // lock_flags
        old_blob[5] = 1; // contact_telemetry[0] = true

        let restored = deserialize(&old_blob).expect("old-length blob must still parse");
        assert!(!restored.notif_visual);
        assert!(restored.notif_audible);
        assert_eq!(restored.contact_count, 2);
        assert_eq!(restored.lock_flags, 0x07);
        assert!(restored.contact_telemetry[0]);
        assert_eq!(restored.screen_sleep_timeout_s, SCREEN_SLEEP_DEFAULT_S);
        assert_eq!(
            restored.lock_timeout_s, LOCK_TIMEOUT_DEFAULT_S,
            "v0x01 never had this field — must default, never zero/garbage"
        );
    }

    /// Acceptance (this mission's hard constraint): a v0x01 blob that DOES
    /// carry the screen-sleep byte (the mid-length shape, `V1_BLOB_LEN`) must
    /// preserve that saved screen-sleep value too — only `lock_timeout_s`,
    /// the genuinely new field, gets defaulted. This is the template test
    /// named in the mission's Hard constraints, extended one version further.
    #[test]
    fn v1_blob_with_screen_sleep_preserves_it_and_defaults_lock_timeout() {
        let mut v1_blob = [0u8; V1_BLOB_LEN];
        v1_blob[0] = VERSION_V1;
        v1_blob[1] = 1; // notif_visual = true
        v1_blob[2] = 0; // notif_audible = false (non-default, proves it round-trips)
        v1_blob[3] = 5; // contact_count
        v1_blob[4] = 0x01; // lock_flags = LOCK_SCREEN_ENABLE already set
        v1_blob[5 + 2] = 1; // contact_telemetry[2] = true
        v1_blob[V1_LEGACY_BLOB_LEN] = 90; // screen_sleep_timeout_s = 90 (non-default)

        let restored = deserialize(&v1_blob).expect("v0x01 blob with screen-sleep must parse");
        assert!(restored.notif_visual);
        assert!(!restored.notif_audible);
        assert_eq!(restored.contact_count, 5);
        assert_eq!(restored.lock_flags, 0x01);
        assert!(restored.contact_telemetry[2]);
        assert_eq!(
            restored.screen_sleep_timeout_s, 90,
            "a field the OLD version already carried must round-trip untouched"
        );
        assert_eq!(
            restored.lock_timeout_s, LOCK_TIMEOUT_DEFAULT_S,
            "only the genuinely new field defaults"
        );
    }

    #[test]
    fn blob_too_short_returns_none() {
        let short = [VERSION_V1; V1_LEGACY_BLOB_LEN - 1];
        assert!(deserialize(&short).is_none());
    }

    /// A v0x02-tagged blob shorter than the full current length is corrupt,
    /// not a legitimate migration shape (unlike a short v0x01 blob) — v0x02
    /// is only ever written at full length by this crate's own `serialize`.
    #[test]
    fn v2_tagged_blob_shorter_than_full_length_returns_none() {
        let mut short = [0u8; BLOB_LEN - 1];
        short[0] = VERSION;
        assert!(deserialize(&short).is_none());
    }

    #[test]
    fn wrong_version_returns_none() {
        let mut blob = [0u8; BLOB_LEN];
        blob[0] = 0xFF;
        assert!(deserialize(&blob).is_none());
    }

    #[test]
    fn empty_blob_returns_none() {
        assert!(deserialize(&[]).is_none());
    }

    #[test]
    fn deserialize_clamps_out_of_range_timeout_byte() {
        // Defensive: a corrupt/rogue blob byte above 120 must not silently
        // load an out-of-spec timeout.
        let mut blob = [0u8; BLOB_LEN];
        blob[0] = VERSION;
        blob[V1_LEGACY_BLOB_LEN] = 200;
        let restored = deserialize(&blob).expect("valid blob");
        assert_eq!(restored.screen_sleep_timeout_s, SCREEN_SLEEP_MAX_S);
    }

    #[test]
    fn deserialize_clamps_out_of_range_lock_timeout() {
        // Same defensive posture for the new field: a corrupt/rogue
        // out-of-bound u16 must not silently load an out-of-spec value.
        let mut blob = [0u8; BLOB_LEN];
        blob[0] = VERSION;
        let [lo, hi] = 50_000u16.to_le_bytes();
        blob[V1_BLOB_LEN] = lo;
        blob[V1_BLOB_LEN + 1] = hi;
        let restored = deserialize(&blob).expect("valid blob");
        assert_eq!(restored.lock_timeout_s, LOCK_TIMEOUT_MAX_S);

        let [lo, hi] = 3u16.to_le_bytes();
        blob[V1_BLOB_LEN] = lo;
        blob[V1_BLOB_LEN + 1] = hi;
        let restored = deserialize(&blob).expect("valid blob");
        assert_eq!(restored.lock_timeout_s, LOCK_TIMEOUT_MIN_S);
    }
}
