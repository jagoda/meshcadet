// SPDX-License-Identifier: GPL-3.0-only
//! Persisted self-advert timestamp — NVS-backed anti-replay counter.
//!
//! The ESP32-S3 itself has no battery-backed RTC: `tx_epoch_base` in
//! `main.rs` starts each boot as a random `esp_random()` value and — once
//! `gps` GPS-syncs the system clock — is rebased every dispatcher-loop tick
//! onto the real GPS wall-clock time (see `main.rs`'s dispatcher loop, right
//! after `gps.poll`). The GPS shield's own GNSS module DOES carry a
//! battery-backed RTC and can supply that sync within seconds of boot from a
//! pre-fix sentence (see `gps`'s module doc "Clock sync" section) rather than
//! only once a real fix lands outdoors — but that RTC-derived sync is marked
//! unverified and is still just wall-clock time to `tx_epoch_base`, not a
//! durable record of what THIS device has issued before: it is not
//! guaranteed accurate to the second (a drifted or stale RTC content is
//! exactly as plausible as a genuine one to `gps`'s plausibility gate), and
//! it is not persisted by `gps` itself either way. So `tx_epoch_base` is
//! fine for the existing DM/channel traffic (only ever compared against
//! itself, never persisted) but unusable as-is for an advert's `timestamp`
//! field — it carries no memory of the highest timestamp this device has
//! EVER issued a card with, GPS/RTC-synced or not. A receiving peer
//! replay-guards an incoming advert on
//! `timestamp <= from->last_advert_timestamp` already on file for that
//! contact (`BaseChatMesh.cpp:124`), so a value that can regress across a
//! reboot (a random reseed, or a fresh device power-on before GPS has
//! re-synced) would make a re-share (e.g. after a device rename) silently
//! fail to update the peer's contact. This store keeps a durable,
//! monotonically-increasing counter across reboots instead;
//! `firmware_core::advert::next_advert_timestamp` combines it with the
//! host-supplied wall-clock hint carried in the `QUERY_ADVERT` payload —
//! independent of both `tx_epoch_base` and GPS sync, since advert generation
//! is a USB-only, host-driven path with its own (typically more available,
//! network-synced) time source.
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
