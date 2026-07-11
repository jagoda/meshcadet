// SPDX-License-Identifier: GPL-3.0-only
//! On-device admin-menu `RuntimeSettings` store — NVS-backed flash persistence.
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
//! # NVS layout
//!
//! | Namespace | Key        | Type | Contents                            |
//! |-----------|------------|------|--------------------------------------|
//! | `mc_rts`  | `rts_blob` | blob | Serialised `RuntimeSettings` (below) |
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
use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
pub use esp_idf_svc::sys::EspError;

const NVS_NAMESPACE: &str = "mc_rts";
const NVS_KEY_BLOB: &str = "rts_blob";
const VERSION: u8 = 0x01;
/// Pre-screen-sleep blob length (original layout, no timeout byte).
const OLD_BLOB_LEN: usize = 5 + MAX_CONTACTS;
/// Current blob length: original layout + 1 trailing `screen_sleep_timeout_s` byte.
const BLOB_LEN: usize = OLD_BLOB_LEN + 1;

/// Load the on-device `RuntimeSettings` from NVS.
///
/// `notif_defaults` is `(visual, audible)` from the admin's provisioning-time
/// `SET_NOTIF_DEFAULTS` command (`ProvisionedConfig::notif_defaults`). It seeds
/// the notification toggles ONLY on first boot (nothing saved yet in this
/// store) or if the stored blob fails to parse — a corrupt blob must not brick
/// the admin menu, it just falls back to a default. Once the admin-menu
/// toggle is used even once, the persisted `RuntimeSettings` blob is the
/// source of truth and `notif_defaults` is no longer consulted (mirrors the
/// on-device screen-sleep-timeout default precedent).
///
/// DEFECT FIX (host-command-audit): this previously always fell back to
/// `RuntimeSettings::default_enabled()` (visual=true, audible=true) — the
/// admin's `SET_NOTIF_DEFAULTS` value was persisted to `ProvisionedConfig`
/// but never read, so provisioning "notification defaults" had no effect on
/// a freshly-provisioned device that had never opened the admin menu.
pub fn load(
    nvs_partition: EspNvsPartition<NvsDefault>,
    notif_defaults: (bool, bool),
) -> Result<RuntimeSettings, EspError> {
    let fallback = || fallback_settings(notif_defaults);
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    let mut blob = [0u8; BLOB_LEN];
    match nvs.get_blob(NVS_KEY_BLOB, &mut blob)? {
        Some(bytes) => Ok(deserialize(bytes).unwrap_or_else(fallback)),
        None => Ok(fallback()),
    }
}

/// Pure helper: `RuntimeSettings::default_enabled()` with the notification
/// toggles overridden to the provisioned `(visual, audible)` defaults.
/// Factored out of [`load`] so the first-boot seeding contract is unit
/// testable without an NVS partition (mirrors [`serialize`]/[`deserialize`]
/// below).
fn fallback_settings(notif_defaults: (bool, bool)) -> RuntimeSettings {
    let mut s = RuntimeSettings::default_enabled();
    s.notif_visual = notif_defaults.0;
    s.notif_audible = notif_defaults.1;
    s
}

/// Persist `settings` to NVS (overwrites any previously stored blob).
pub fn save(nvs_partition: EspNvsPartition<NvsDefault>, settings: &RuntimeSettings) -> Result<(), EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    let mut blob = [0u8; BLOB_LEN];
    let n = serialize(settings, &mut blob);
    nvs.set_blob(NVS_KEY_BLOB, &blob[..n])?;
    Ok(())
}

fn serialize(s: &RuntimeSettings, out: &mut [u8]) -> usize {
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

fn deserialize(blob: &[u8]) -> Option<RuntimeSettings> {
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
// Pure functions over byte slices — no NVS/hardware required. Same caveat as
// `pin_menu.rs`/`ui/mod.rs`: this crate's `cargo test` type-checks but does not
// execute `#[cfg(test)]` blocks (see `ui/mod.rs` module docs), so these pin the
// intended contract for a future host-side extraction or hardware-in-the-loop
// run rather than a currently-executing regression guard.
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
