// SPDX-License-Identifier: GPL-3.0-only
//! Screen-lock PIN store — `EspNvs` wrapper over NVS namespace `mc_lock`,
//! key `lock_blob`.
//!
//! The blob codec (`serialize`/`deserialize`) is a pure byte-slice pair with
//! no NVS dependency; it lives in [`firmware_core::lock_store`] so its tests
//! execute under `cargo test --workspace` (this crate is a detached,
//! cross-compiled workspace — see `Cargo.toml`'s doc comment — so a
//! `#[cfg(test)]` block written here would type-check but never run). This
//! file keeps the `EspNvs` read/write wrapper (`load`/`save`), which needs a
//! real NVS partition, and re-exports the pure half via `pub use
//! firmware_core::lock_store::*;` below — the same shim shape
//! `runtime_settings_store.rs` / `advert_ts_store.rs` already use. See
//! `docs/adr/0005-firmware-core-extraction.md`.
//!
//! # Why a dedicated store, not `ProvisionedConfig` or `mc_rts`
//!
//! Screen-lock plan D2 rejects folding the lock PIN into either existing
//! store:
//!
//! - **`ProvisionedConfig`** (`config_store.rs`) would need a `CFG_VERSION`
//!   `0x03` → `0x04` bump, growing `MAX_BLOB_LEN` and
//!   `size_of::<ProvisionedConfig>()` and dragging the host CLI's and
//!   `site/provisioner/codec.js`'s blob decoders along with it — the single
//!   largest cost item the campaign's plan explicitly rejects paying.
//! - **`mc_rts`** (`runtime_settings_store.rs`) is written by exactly one
//!   thread — the UI thread (see that module's doc for the race a second
//!   writer would open) — but the lock PIN is written by `admin_server` over
//!   USB, a different thread entirely.
//!
//! So the lock PIN gets its own namespace instead, following the exact
//! `advert_ts_store.rs` / `gps_baud_store.rs` shape every other
//! `admin_server`-owned single value already uses (see
//! `checklists/meshcadet-firmware-dispatcher-stateful-feature.md`). Unlike
//! those two (a plain `u32` via `get_u32`/`set_u32`), the lock PIN is a
//! small fixed-shape blob (version + length + digits), so this store uses
//! `get_blob`/`set_blob` — the same primitive `runtime_settings_store.rs`
//! uses for its own (larger) blob.
//!
//! `admin_server.rs`'s `FRAME_SET_LOCK_PIN` handler is this store's only
//! writer. `FRAME_QUERY_LOCK` is a reader (for the `pin_set` bit in
//! `RSP_LOCK`) — reads of `mc_lock` are unrestricted; the single-writer
//! invariant is about `mc_rts`, a different namespace entirely.

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
pub use esp_idf_svc::sys::EspError;
use protocol::provisioning::LOCK_PIN_LEN;

pub use firmware_core::lock_store::*;

const NVS_NAMESPACE: &str = "mc_lock";
const NVS_KEY_BLOB: &str = "lock_blob";

/// Load the persisted screen-lock PIN from NVS.
///
/// Returns `([0; LOCK_PIN_LEN], 0)` ("no PIN set") on first boot, a missing
/// key, a corrupt/unrecognised blob, or any NVS error (logged, non-fatal) —
/// mirrors `deserialize`'s own "treat as no PIN set" contract and
/// `config_store`'s `is_provisioned` gate precedent: a storage-layer failure
/// here degrades to "lock PIN not configured," never a panic or a stale/
/// garbage PIN.
pub fn load(nvs_partition: EspNvsPartition<NvsDefault>) -> ([u8; LOCK_PIN_LEN], u8) {
    let unset = ([0u8; LOCK_PIN_LEN], 0u8);
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!(
                "lock_store: failed to open NVS namespace ({:?}); no PIN set",
                e
            );
            return unset;
        }
    };
    let mut blob = [0u8; BLOB_LEN];
    match nvs.get_blob(NVS_KEY_BLOB, &mut blob) {
        Ok(Some(bytes)) => deserialize(bytes).unwrap_or(unset),
        Ok(None) => unset,
        Err(e) => {
            log::warn!("lock_store: NVS read failed ({:?}); no PIN set", e);
            unset
        }
    }
}

/// Persist `pin`/`pin_len` as the screen-lock PIN, overwriting any previous
/// value. The caller (`admin_server.rs`'s `FRAME_SET_LOCK_PIN` handler)
/// already has a decode-path-validated exactly-`LOCK_PIN_LEN`-ASCII-digit
/// PIN by the time this is called — this store trusts it without
/// re-validating content, mirroring how `config_store` stores the admin PIN
/// it's handed.
pub fn save(
    nvs_partition: EspNvsPartition<NvsDefault>,
    pin: &[u8; LOCK_PIN_LEN],
    pin_len: u8,
) -> Result<(), EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    let mut blob = [0u8; BLOB_LEN];
    let n = serialize(pin, pin_len, &mut blob);
    nvs.set_blob(NVS_KEY_BLOB, &blob[..n])?;
    Ok(())
}
