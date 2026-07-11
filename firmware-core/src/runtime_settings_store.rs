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
//! removes that race entirely: the UI thread is the sole writer here.
//!
//! # Blob layout
//!
//! Stored (by `firmware::runtime_settings_store::save`) under NVS namespace
//! `mc_rts`, key `rts_blob`:
//!
//! ```text
//! byte 0          version = 0x01
//! byte 1          notif_visual         (0/1)
//! byte 2          notif_audible        (0/1)
//! byte 3          contact_count        (0..=MAX_CONTACTS)
//! byte 4          lock_flags
//! bytes 5..5+N    contact_telemetry    (0/1 per slot, N = MAX_CONTACTS)
//! byte 5+N        screen_sleep_timeout_s (0..=120; ADDED, see below)
//! ```
//!
//! # Backward compatibility (screen-sleep timeout field)
//!
//! The `screen_sleep_timeout_s` byte was appended AFTER the original v0.01
//! layout rather than inserted, and the version tag was deliberately left at
//! `0x01` — the field is additive. A device already in the field with an
//! old (shorter) blob stored must not have its saved `notif_visual` /
//! `notif_audible` / `contact_telemetry` prefs reset to defaults just because
//! firmware grew one more field; `deserialize` accepts BOTH the old
//! (`OLD_BLOB_LEN`) and new (`BLOB_LEN`) lengths, defaulting the timeout to
//! `SCREEN_SLEEP_DEFAULT_S` when reading an old-length blob.

use crate::pin_menu::{RuntimeSettings, MAX_CONTACTS, SCREEN_SLEEP_DEFAULT_S, SCREEN_SLEEP_MAX_S};

const VERSION: u8 = 0x01;
/// Pre-screen-sleep blob length (original layout, no timeout byte).
const OLD_BLOB_LEN: usize = 5 + MAX_CONTACTS;
/// Current blob length: original layout + 1 trailing `screen_sleep_timeout_s` byte.
pub const BLOB_LEN: usize = OLD_BLOB_LEN + 1;

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
    out[OLD_BLOB_LEN] = s.screen_sleep_timeout_s.min(SCREEN_SLEEP_MAX_S);
    BLOB_LEN
}

pub fn deserialize(blob: &[u8]) -> Option<RuntimeSettings> {
    // Accept both the old (pre-screen-sleep) and current blob lengths so an
    // in-field upgrade doesn't reset previously-saved notif/telemetry prefs —
    // see the module-level "Backward compatibility" note.
    if blob.len() < OLD_BLOB_LEN || blob[0] != VERSION {
        return None;
    }
    let mut s = RuntimeSettings::default_enabled();
    s.notif_visual = blob[1] != 0;
    s.notif_audible = blob[2] != 0;
    s.contact_count = blob[3];
    s.lock_flags = blob[4];
    for i in 0..MAX_CONTACTS {
        s.contact_telemetry[i] = blob[5 + i] != 0;
    }
    s.screen_sleep_timeout_s = if blob.len() >= BLOB_LEN {
        blob[OLD_BLOB_LEN].min(SCREEN_SLEEP_MAX_S)
    } else {
        // Old-length blob predates this field — fall back to the documented default.
        SCREEN_SLEEP_DEFAULT_S
    };
    Some(s)
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

    /// Acceptance: an old-length blob (pre-screen-sleep firmware) must not
    /// reset `notif_visual`/`notif_audible` to defaults — it must fall back
    /// ONLY the new field to `SCREEN_SLEEP_DEFAULT_S`, preserving everything
    /// that old blob actually stored.
    #[test]
    fn old_length_blob_preserves_existing_fields_and_defaults_timeout() {
        let mut old_blob = [0u8; OLD_BLOB_LEN];
        old_blob[0] = VERSION;
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
    }

    #[test]
    fn blob_too_short_returns_none() {
        let short = [VERSION; OLD_BLOB_LEN - 1];
        assert!(deserialize(&short).is_none());
    }

    #[test]
    fn wrong_version_returns_none() {
        let mut blob = [0u8; BLOB_LEN];
        blob[0] = 0xFF;
        assert!(deserialize(&blob).is_none());
    }

    #[test]
    fn deserialize_clamps_out_of_range_timeout_byte() {
        // Defensive: a corrupt/rogue blob byte above 120 must not silently
        // load an out-of-spec timeout.
        let mut blob = [0u8; BLOB_LEN];
        blob[0] = VERSION;
        blob[OLD_BLOB_LEN] = 200;
        let restored = deserialize(&blob).expect("valid blob");
        assert_eq!(restored.screen_sleep_timeout_s, SCREEN_SLEEP_MAX_S);
    }
}
