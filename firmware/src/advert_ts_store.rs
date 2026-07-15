// SPDX-License-Identifier: GPL-3.0-only
//! Persisted self-advert timestamp — NVS-backed anti-replay counter.
//!
//! MeshCadet has no RTC: `tx_epoch_base` in `main.rs` is a per-boot
//! `esp_random()` value, fine for the existing DM/channel traffic (only ever
//! compared against itself, never persisted) but unusable for an advert's
//! `timestamp` field — a receiving peer replay-guards an incoming advert on
//! `timestamp <= from->last_advert_timestamp` already on file for that
//! contact (`BaseChatMesh.cpp:124`), so a fresh random value on every boot
//! would make a re-share (e.g. after a device rename) silently fail to
//! update the peer's contact. This store keeps a durable,
//! monotonically-increasing counter across reboots;
//! `firmware_core::advert::next_advert_timestamp` combines it with the
//! host-supplied wall-clock hint carried in the `QUERY_ADVERT` payload.
//!
//! # NVS layout
//!
//! | Namespace | Key  | Type | Contents                              |
//! |-----------|------|------|-----------------------------------------|
//! | `mc_adv`  | `ts` | u32  | Last self-advert-card timestamp issued |
//!
//! Same "plain typed scalar, its own namespace" shape as
//! `gps_baud_store.rs`'s cached baud rate — no versioning or
//! backward-compatibility concerns for a single `u32` counter.

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};

const NVS_NAMESPACE: &str = "mc_adv";
const NVS_KEY_TS: &str = "ts";

/// Load the last-issued self-advert timestamp from NVS.
///
/// Returns `0` (never issued) on first boot, on a missing key, or on any NVS
/// error (logged, non-fatal) —
/// [`firmware_core::advert::next_advert_timestamp`] treats `0` as "no prior
/// card" rather than a real timestamp, so this fallback can never regress
/// the anti-replay sequence below the host-time floor.
pub fn load_last_advert_ts(nvs_partition: EspNvsPartition<NvsDefault>) -> u32 {
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!(
                "advert_ts_store: failed to open NVS namespace ({:?}); starting from 0",
                e
            );
            return 0;
        }
    };
    match nvs.get_u32(NVS_KEY_TS) {
        Ok(v) => v.unwrap_or(0),
        Err(e) => {
            log::warn!("advert_ts_store: NVS read failed ({:?}); starting from 0", e);
            0
        }
    }
}

/// Persist `ts` as the last-issued self-advert timestamp, overwriting any
/// previous value.
///
/// Callers MUST write this BEFORE sending the `FRAME_RSP_ADVERT` reply that
/// carries `ts` (see `admin_server.rs`'s `FRAME_QUERY_ADVERT` handler) — that
/// ordering means a crash between the two cannot regress monotonicity on the
/// next boot: worst case the host times out and retries, generating a fresh,
/// still-strictly-increasing card. A failed write here is logged and
/// non-fatal; it only risks a future card reusing a lower timestamp than an
/// already-sent one, not a wire-format or signature defect.
pub fn save_last_advert_ts(nvs_partition: EspNvsPartition<NvsDefault>, ts: u32) {
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!(
                "advert_ts_store: failed to open NVS namespace for write ({:?})",
                e
            );
            return;
        }
    };
    if let Err(e) = nvs.set_u32(NVS_KEY_TS, ts) {
        log::warn!(
            "advert_ts_store: NVS write failed ({:?}); next card may reuse a lower timestamp",
            e
        );
    }
}
