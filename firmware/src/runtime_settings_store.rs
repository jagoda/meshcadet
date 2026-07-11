// SPDX-License-Identifier: GPL-3.0-only
//! On-device admin-menu `RuntimeSettings` store — NVS-backed flash persistence.
//!
//! The blob codec (`serialize`/`deserialize`) and the first-boot-seeding
//! helper (`fallback_settings`) are pure byte-slice functions with no NVS
//! dependency; they now live in [`firmware_core::runtime_settings_store`] so
//! their tests execute under `cargo test --workspace` (this crate is a
//! detached, cross-compiled workspace — see `Cargo.toml`'s doc comment — so
//! a `#[cfg(test)]` block written here would type-check but never run). See
//! that module's doc for the blob layout and backward-compatibility notes.
//! This file keeps the `EspNvs` read/write wrapper (`load`/`save`), which
//! needs a real NVS partition. `pub use firmware_core::runtime_settings_
//! store::*;` below re-exports the pure half so every existing call site
//! resolves unchanged. See `docs/adr/0005-firmware-core-extraction.md`.

use crate::pin_menu::RuntimeSettings;
use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
pub use esp_idf_svc::sys::EspError;

pub use firmware_core::runtime_settings_store::*;

const NVS_NAMESPACE: &str = "mc_rts";
const NVS_KEY_BLOB: &str = "rts_blob";

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

/// Persist `settings` to NVS (overwrites any previously stored blob).
pub fn save(nvs_partition: EspNvsPartition<NvsDefault>, settings: &RuntimeSettings) -> Result<(), EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    let mut blob = [0u8; BLOB_LEN];
    let n = serialize(settings, &mut blob);
    nvs.set_blob(NVS_KEY_BLOB, &blob[..n])?;
    Ok(())
}
