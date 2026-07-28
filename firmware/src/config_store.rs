// SPDX-License-Identifier: GPL-3.0-only
//! Firmware-side provisioning config store — NVS-backed flash persistence.
//!
//! The blob codec (`serialize_config`/`deserialize_config`) and every
//! provisioned-config struct (`Contact`/`Channel`/`RoomExtra`/
//! `ProvisionedConfig`) are pure byte-slice/in-memory functions with no NVS
//! dependency; they now live in [`firmware_core::config_store`] so their
//! tests execute under `cargo test --workspace` (this crate is a detached,
//! cross-compiled workspace — see `Cargo.toml`'s doc comment — so a
//! `#[cfg(test)]` block written here would type-check but never run). See
//! that module's doc for the blob layout, the v0x02→v0x03 forward-migration
//! contract, and the `MAX_BLOB_LEN` budget. This file keeps the `EspNvs`
//! read/write wrapper, which needs a real NVS partition. `pub use
//! firmware_core::config_store::*;` below re-exports the pure half so every
//! existing call site resolves unchanged. See
//! `docs/adr/0005-firmware-core-extraction.md`.
//!
//! # NVS layout
//!
//! | Namespace | Key          | Type  | Contents                               |
//! |-----------|--------------|-------|----------------------------------------|
//! | `mc_cfg`  | `prov`       | u8    | 0 = unprovisioned, 1 = provisioned    |
//! | `mc_cfg`  | `cfg_blob`   | blob  | Serialised `ProvisionedConfig` binary  |
//!
//! The `prov` flag is written to `1` only by [`mark_provisioned`] /
//! [`save_provisioned_config`], which persists the config blob first.  Reads at
//! boot check the flag first; a missing or zero flag means UNPROVISIONED.

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
pub use esp_idf_svc::sys::EspError;

pub use firmware_core::config_store::*;

// ── NVS keys ─────────────────────────────────────────────────────────────────

const NVS_NAMESPACE: &str = "mc_cfg";
const NVS_KEY_PROV_FLAG: &str = "prov";
const NVS_KEY_CFG_BLOB: &str = "cfg_blob";

// ── Public API ─────────────────────────────────────────────────────────────────

/// Check whether this device has been provisioned (flash flag is set).
///
/// Does NOT load the full config blob; suitable for the boot gate.
pub fn is_provisioned(nvs_partition: EspNvsPartition<NvsDefault>) -> Result<bool, EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    Ok(nvs.get_u8(NVS_KEY_PROV_FLAG)?.unwrap_or(0) == 1)
}

/// Load the provisioned config from NVS.
///
/// Returns `Ok(None)` if the device is unprovisioned (or if the blob is
/// missing / corrupt — in which case reprovisioning is required).
/// Returns `Ok(Some(config))` if the device is provisioned and the blob
/// deserialises without error. Accepts both the current ([`CFG_VERSION`])
/// and the pre-room-server ([`CFG_VERSION_V2`]) blob format — see
/// [`firmware_core::config_store`]'s "Forward migration" doc section;
/// [`deserialize_config`] itself dispatches on the version byte.
pub fn load_provisioned_config(
    nvs_partition: EspNvsPartition<NvsDefault>,
) -> Result<Option<ProvisionedConfig>, EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;

    // Check the provisioned flag first.
    if nvs.get_u8(NVS_KEY_PROV_FLAG)?.unwrap_or(0) != 1 {
        return Ok(None);
    }

    // Read the config blob. Heap-allocated (not a `[0u8; MAX_BLOB_LEN]` stack
    // array): `MAX_BLOB_LEN` is 3544 B (see `firmware_core::config_store`'s
    // "Blob size budget" doc) — see the `boot-pthread-stack-overflow-fix`
    // mission for why a buffer this size has no business living on a
    // constrained pthread stack (`admin_server`/`prov_server` both call
    // through here).
    let mut blob = vec![0u8; MAX_BLOB_LEN].into_boxed_slice();
    match nvs.get_blob(NVS_KEY_CFG_BLOB, &mut blob)? {
        Some(bytes)
            if !bytes.is_empty() && (bytes[0] == CFG_VERSION || bytes[0] == CFG_VERSION_V2) =>
        {
            match deserialize_config(bytes) {
                Some(cfg) => Ok(Some(cfg)),
                None => {
                    log::warn!("config_store: blob deserialization failed — treating as unprovisioned");
                    Ok(None)
                }
            }
        }
        Some(bytes) => {
            log::warn!(
                "config_store: blob version mismatch or truncated ({} bytes, version=0x{:02x}) — reprovisioning required",
                bytes.len(),
                bytes.first().copied().unwrap_or(0xFF)
            );
            Ok(None)
        }
        None => {
            // Provisioned flag was set but blob is missing — inconsistent NVS state.
            log::warn!("config_store: prov flag set but cfg_blob absent — treating as unprovisioned");
            Ok(None)
        }
    }
}

/// Save `config` to NVS and set the provisioned flag.
///
/// This is the atomic commit step: the blob is written first, then the flag.
/// If the blob write fails the flag remains unset (UNPROVISIONED state
/// is preserved — a correct invariant). Always persists in the CURRENT
/// (`CFG_VERSION`) format — a device that loaded a migrated v0x02 blob and
/// is then re-saved (e.g. after any admin edit) upgrades forward permanently
/// on this call.
pub fn save_provisioned_config(
    nvs_partition: EspNvsPartition<NvsDefault>,
    config: &ProvisionedConfig,
) -> Result<(), EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;

    // Serialise the config. Heap-allocated — see `load_provisioned_config`'s
    // matching comment: this is called from deep in the `admin_server`/
    // `prov_server` thread call chain (`run` → `handle_frame` →
    // `persist_or_rollback`/`persist_setting` → here), on top of whatever
    // those threads already have resident, so a 3544 B stack array here was
    // the dominant transient contributor to the boot-time stack overflow.
    let mut blob = vec![0u8; MAX_BLOB_LEN].into_boxed_slice();
    let blob_len = serialize_config(config, &mut blob);

    // Write the blob first, then the flag (atomicity property).
    nvs.set_blob(NVS_KEY_CFG_BLOB, &blob[..blob_len])?;
    nvs.set_u8(NVS_KEY_PROV_FLAG, 1)?;

    log::info!(
        "config_store: provisioning committed — {} contacts, {} channels, {} rooms, blob {} bytes",
        config.contact_count,
        config.channel_count,
        config.room_count,
        blob_len,
    );
    Ok(())
}

/// Clear the provisioned flag (and optionally the blob) — for factory reset.
///
/// After this call, `is_provisioned` returns `false` and the next boot enters
/// the UNPROVISIONED state.
///
/// No caller today — there is no factory-reset trigger anywhere in the
/// firmware (no menu row, no host command) yet. Kept as the primitive that
/// feature will need.
#[allow(dead_code)]
pub fn clear_provisioned_flag(nvs_partition: EspNvsPartition<NvsDefault>) -> Result<(), EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    nvs.set_u8(NVS_KEY_PROV_FLAG, 0)?;
    log::info!("config_store: provisioned flag cleared (factory reset path)");
    Ok(())
}
