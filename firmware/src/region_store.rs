// SPDX-License-Identifier: GPL-3.0-only
//! Region-name store — `EspNvs` wrapper over NVS namespace `mc_region`,
//! key `name`, plus the boot-time scope derivation the radio loop holds.
//!
//! # NVS layout
//!
//! | Namespace   | Key    | Type | Contents                                        |
//! |-------------|--------|------|-------------------------------------------------|
//! | `mc_region` | `name` | blob | MeshCore region name, ≤ [`REGION_NAME_MAX`] bytes, unpadded. Absent or empty ⇒ no region configured |
//!
//! # Why a dedicated store, not `ProvisionedConfig`
//!
//! ADR-0015 D2 makes the region a provisioning-time, fleet-level setting, but
//! that does not make it part of the provisioning *blob*. Folding
//! `name[31] + name_len` into `ProvisionedConfig` would need a `CFG_VERSION`
//! `0x03` → `0x04` bump, growing `MAX_BLOB_LEN` and
//! `size_of::<ProvisionedConfig>()` and dragging the host CLI's and
//! `site/provisioner/codec.js`'s blob decoders (plus ADR-0002 §7's
//! forward-migration contract and its tests) along with it — the same cost the
//! screen-lock PIN explicitly refused to pay (see `lock_store.rs`'s module doc,
//! which this store mirrors). A dedicated namespace is additive: an old
//! firmware ignores the key, a new one defaults to "no region" when it is
//! absent, and nothing about the blob format moves.
//!
//! # Writers
//!
//! `provisioning_server.rs` (`FRAME_SET_REGION`, first-boot path) and
//! `admin_server.rs` (`FRAME_SET_REGION`, post-boot path) are the only writers.
//! Both persist **immediately** on a validated frame — mirroring
//! `identity_store`'s device name, not `ProvisionedConfig`'s staged
//! commit — and both go through [`save`], which takes the fixed-size padded
//! form so an over-length name is unrepresentable at the persistence boundary
//! (ADR-0015 D4: a truncated name derives a different key and therefore a
//! different region, so it must be refused, never clamped).
//!
//! # Readers
//!
//! - Boot: [`load_scope`] → the emission scope `main.rs`'s radio loop holds
//!   for its whole lifetime ([`firmware_core::region_scope::RegionScope`]).
//!   Derived ONCE per boot, so a `SET_REGION` applies on air only after a
//!   reboot — the same boot-snapshot posture as the provisioned
//!   channel/contact set, and required here because a frame's transport code
//!   must stay byte-identical across its retries.
//! - `FRAME_QUERY_REGION` in both servers: reads the raw name fresh via
//!   [`load_name`] on every call (no cached copy to go stale, same rationale
//!   as `admin_server`'s `FRAME_QUERY_LOCK` reading `mc_rts` live).
//!
//! The pure half (name validation, key derivation, the fail-closed scope
//! state machine) lives in [`firmware_core::region_scope`] /
//! [`firmware_core::region_admin`] so its tests execute under
//! `cargo test --workspace`; this file keeps only the NVS read/write, which
//! needs a real partition (this crate is a detached, cross-compiled workspace
//! — see `Cargo.toml`'s doc comment). Same shim shape as `lock_store.rs` /
//! `config_store.rs`.

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
pub use esp_idf_svc::sys::EspError;
use protocol::region::REGION_NAME_MAX;

use firmware_core::region_scope::RegionScope;

const NVS_NAMESPACE: &str = "mc_region";
const NVS_KEY_NAME: &str = "name";

/// Load the persisted region name from NVS.
///
/// Returns `(name, name_len)` zero-padded to [`REGION_NAME_MAX`], the same
/// convention `identity_store::load_name` uses for the device display name.
/// `name_len == 0` means no region is configured (first boot, or an explicit
/// clear via `FRAME_SET_REGION` with an empty name).
///
/// A read *error* is deliberately propagated, not swallowed into "no region":
/// the caller must fail closed (see [`load_scope`] and
/// [`firmware_core::region_scope::boot_scope`]), because silently reporting
/// "unconfigured" for an unreadable store would turn a region-managed device
/// into an unscoped flooder — the exact failure ADR-0015 D4 forbids. This
/// also covers the corrupt-store case where the blob is longer than
/// [`REGION_NAME_MAX`]: `get_blob` reports `ESP_ERR_NVS_INVALID_LENGTH` rather
/// than truncating it into `buf`, so an over-length name can never be keyed
/// off a prefix.
pub fn load_name(
    nvs_partition: EspNvsPartition<NvsDefault>,
) -> Result<([u8; REGION_NAME_MAX], u8), EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    let mut buf = [0u8; REGION_NAME_MAX];
    let len = match nvs.get_blob(NVS_KEY_NAME, &mut buf)? {
        Some(bytes) => bytes.len(),
        None => 0,
    };
    Ok((buf, len as u8))
}

/// Persist a validated region name, overwriting any previous value.
///
/// Mirrors `identity_store::set_name`: the caller
/// (`region_admin::handle_set_region` via either USB-serial server) has
/// already validated the name, and this takes the fixed-shape padded form so
/// truncation is unrepresentable.
///
/// `name_len == 0` is the explicit **clear** command. It is implemented as an
/// NVS *erase*, not a zero-length blob (ESP-IDF's zero-length blob behaviour
/// is unspecified, and an absent key is exactly what [`load_name`] reports as
/// `name_len == 0`), so a later [`load_scope`] resolves to unscoped emission.
/// Clearing an already-clear region succeeds without touching flash.
pub fn save(
    nvs_partition: EspNvsPartition<NvsDefault>,
    name: &[u8; REGION_NAME_MAX],
    name_len: u8,
) -> Result<(), EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    let n = (name_len as usize).min(REGION_NAME_MAX);
    if n == 0 {
        // `EspNvs::remove` reports whether the key was actually present;
        // "already absent" (`Ok(false)`) is precisely the post-clear state we
        // want, so the flag is discarded rather than treated as a failure. A
        // real NVS fault still propagates, so the caller answers `RSP_ERROR`
        // instead of acking a clear that did not land.
        return nvs.remove(NVS_KEY_NAME).map(|_was_present| ());
    }
    nvs.set_blob(NVS_KEY_NAME, &name[..n])
}

/// Derive this boot's region emission scope from flash.
///
/// FAIL CLOSED (ADR-0015 D4): an unreadable store or an underivable name both
/// resolve to [`RegionScope::Unusable`], never to [`RegionScope::Unscoped`].
/// The caller (`main.rs`) logs the reason once at boot and suppresses flood
/// emission while the state holds.
pub fn load_scope(nvs_partition: EspNvsPartition<NvsDefault>) -> RegionScope {
    match load_name(nvs_partition) {
        Ok((name, name_len)) => firmware_core::region_scope::boot_scope(Some(
            &name[..(name_len as usize).min(REGION_NAME_MAX)],
        )),
        Err(e) => {
            log::error!(
                "region_store: NVS read failed ({:?}) — region scope unknown, failing closed",
                e
            );
            firmware_core::region_scope::boot_scope(None)
        }
    }
}
