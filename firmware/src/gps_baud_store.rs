// SPDX-License-Identifier: GPL-3.0-only
//! Persisted GPS UART baud-rate cache — NVS-backed.
//!
//! By design (full
//! auto-detect): probing all of [`gps::GPS_BAUD_CANDIDATES`](crate::gps::GPS_BAUD_CANDIDATES)
//! costs up to `candidates.len() * BAUD_PROBE_WINDOW_MS` (~3.3 s worst case for
//! three candidates) — acceptable once, but not worth paying on every boot
//! once the unit's actual rate is known. The detected rate is persisted here
//! after a successful probe so subsequent boots can attempt the cached rate
//! directly; [`gps::GpsDriver::new`](crate::gps::GpsDriver::new) only falls
//! back to the full multi-candidate probe if the cached rate turns out to be
//! stale (module swapped, reconfigured, or genuinely silent) — see that
//! function's doc for the self-healing re-probe path.
//!
//! # NVS layout
//!
//! | Namespace | Key    | Type | Contents                          |
//! |-----------|--------|------|------------------------------------|
//! | `mc_gps`  | `baud` | u32  | Last-detected working baud rate    |
//!
//! A plain typed `u32` (via `EspNvs::get_u32`/`set_u32`) is used instead of a
//! hand-rolled blob layout — the value is a single scalar with no versioning
//! or backward-compatibility concerns (unlike `runtime_settings_store`'s
//! growing struct).

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};

const NVS_NAMESPACE: &str = "mc_gps";
const NVS_KEY_BAUD: &str = "baud";

/// Load the cached GPS baud rate from NVS, if a previous boot successfully
/// probed and persisted one. Returns `None` on first boot (key absent) or on
/// any NVS error (logged, non-fatal — the caller falls back to a full probe).
pub fn load_cached_baud(nvs_partition: EspNvsPartition<NvsDefault>) -> Option<u32> {
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!("GPS: baud cache — failed to open NVS namespace ({:?}); no cached rate", e);
            return None;
        }
    };
    match nvs.get_u32(NVS_KEY_BAUD) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("GPS: baud cache — NVS read failed ({:?}); no cached rate", e);
            None
        }
    }
}

/// Persist `baud` as the cached GPS baud rate (overwrites any previous
/// value). Failure is logged and non-fatal — a lost cache write just costs
/// a full re-probe on the next boot, not correctness.
pub fn save_cached_baud(nvs_partition: EspNvsPartition<NvsDefault>, baud: u32) {
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!("GPS: baud cache — failed to open NVS namespace for write ({:?})", e);
            return;
        }
    };
    if let Err(e) = nvs.set_u32(NVS_KEY_BAUD, baud) {
        log::warn!("GPS: baud cache — NVS write failed ({:?}); rate will be re-probed next boot", e);
    }
}
