// SPDX-License-Identifier: GPL-3.0-only
//! Battery status driver for the LilyGo T-Deck Plus — ADC voltage-divider read.
//!
//! The pure percent/charging-inference model (`BatteryStatus`,
//! `percent_from_millivolts`, `battery_poll_step`, and the calibration
//! constants) now lives in [`firmware_core::battery`] so its tests execute
//! under `cargo test --workspace` (this crate is a detached, cross-compiled
//! workspace — see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block
//! written here would type-check but never run); see that module's doc for
//! the full hardware-feasibility writeup, the charge-inflation bug/fix
//! history, and the ADC-calibration notes. This file keeps only the real
//! ADC1 read path (`BatteryDriver`) and the NVS `settled_mv` persistence,
//! both of which need actual hardware. `pub use firmware_core::battery::*;`
//! below re-exports the pure half so every existing call site
//! (`battery::BatteryStatus`, `crate::battery::clamp_raw_mv_for_wire`, …)
//! resolves unchanged. See `docs/adr/0005-firmware-core-extraction.md`.
//!
//! # NVS layout (`settled_mv` persistence — see `firmware_core::battery`'s
//! "(A)" doc section)
//!
//! | Namespace | Key        | Type | Contents                              |
//! |-----------|------------|------|-----------------------------------------|
//! | `mc_cfg`  | `batt_mv`  | u32  | Last CONFIRMED `settled_mv` (mV)       |
//!
//! Deliberately reuses `config_store`'s `mc_cfg` provisioning namespace (this
//! mission's acceptance line: "reuse the provisioning namespace") rather than
//! opening a new one — a plain typed scalar under its own key, same shape as
//! `advert_ts_store.rs`'s `mc_adv`/`ts` u32 counter, just filed under the
//! existing namespace instead of a fresh one.

use std::rc::Rc;

use esp_idf_hal::adc::attenuation::DB_12;
use esp_idf_hal::adc::oneshot::config::{AdcChannelConfig, Calibration};
use esp_idf_hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_hal::adc::{ADCCH3, ADCU1, ADC1};
use esp_idf_hal::gpio::Gpio4;
use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};

pub use firmware_core::battery::*;

// ── Tunables ──────────────────────────────────────────────────────────────────

/// ADC samples averaged per read (matches the reference `getBattMilliVolts()`'s
/// `BATTERY_SAMPLES`).
const BATTERY_SAMPLES: u32 = 8;

/// Voltage-divider ratio: `Vbat = DIVIDER_RATIO * Vadc` (LilyGo reference:
/// `ADC_MULTIPLIER = 2.0 * 3.3 * 1000`, i.e. a 2:1 divider).
const DIVIDER_RATIO: u32 = 2;

/// Minimum interval between ADC samples. Battery state changes slowly; there
/// is no reason to spend a multi-sample ADC read on every dispatcher-loop
/// iteration (unlike GPS, which drains a UART byte stream every tick). This
/// is the per-SAMPLE cadence fed into [`PeakWindowSampler`] — the
/// [`PEAK_WINDOW_MS`] (~30 s) peak-hold window sits above it (see module
/// docs / `firmware_core::battery`'s "(B)" section).
const BATTERY_POLL_INTERVAL_MS: u64 = 2_000;

// ── NVS (settled_mv persistence — see module doc's "NVS layout" section) ────

/// Reuses `config_store`'s provisioning namespace — see this file's module
/// doc. `config_store::NVS_NAMESPACE` is private to that module, so this is
/// its own copy of the same literal, not an import; keep the two in sync if
/// either namespace is ever renamed.
const NVS_NAMESPACE: &str = "mc_cfg";
const NVS_KEY_SETTLED_MV: &str = "batt_mv";

/// Load the last-persisted, CONFIRMED `settled_mv` from NVS.
///
/// Returns `None` on first boot, a missing key, or any NVS error (logged,
/// non-fatal) — [`seed_boot_state`] treats `None` as "no prior confirmed
/// basis," collapsing to the pre-persistence boot behavior.
fn load_persisted_settled_mv(nvs_partition: &EspNvsPartition<NvsDefault>) -> Option<u32> {
    let nvs = match EspNvs::new(nvs_partition.clone(), NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!(
                "battery: failed to open NVS namespace for settled_mv read ({:?})",
                e
            );
            return None;
        }
    };
    match nvs.get_u32(NVS_KEY_SETTLED_MV) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("battery: NVS settled_mv read failed ({:?})", e);
            None
        }
    }
}

/// Persist `mv` as the last CONFIRMED `settled_mv`, overwriting any previous
/// value. Callers MUST only call this when [`should_persist_settled_mv`]
/// says to (never for an unconfirmed basis — see module doc). A failed write
/// here is logged and non-fatal; it only risks the NEXT boot restoring a
/// stale (but never poisoned) prior value.
fn persist_settled_mv(nvs_partition: &EspNvsPartition<NvsDefault>, mv: u32) {
    match EspNvs::new(nvs_partition.clone(), NVS_NAMESPACE, true) {
        Ok(nvs) => {
            if let Err(e) = nvs.set_u32(NVS_KEY_SETTLED_MV, mv) {
                log::warn!("battery: NVS settled_mv write failed ({:?})", e);
            }
        }
        Err(e) => log::warn!(
            "battery: failed to open NVS namespace for settled_mv write ({:?})",
            e
        ),
    }
}

// ── BatteryDriver ─────────────────────────────────────────────────────────────

/// Polling ADC-based battery driver for the T-Deck Plus.
///
/// Constructed once in `main.rs::run()` and polled on every dispatcher-loop
/// iteration (cheap no-op between [`BATTERY_POLL_INTERVAL_MS`] samples). Owns
/// the ADC1 unit peripheral for the lifetime of the running system.
pub struct BatteryDriver<'d> {
    // The ADC unit is shared with the channel driver via `Rc` (channel holds a
    // clone) rather than borrowed, so both can live in this struct without a
    // self-referential lifetime.
    _adc: Rc<AdcDriver<'d, ADCU1>>,
    chan: AdcChannelDriver<'d, ADCCH3<ADCU1>, Rc<AdcDriver<'d, ADCU1>>>,

    /// Percent basis — see [`battery_poll_step`]. Kept in lock-step with the
    /// live voltage while off external power; frozen at its last good value
    /// while [`EXTERNAL_POWER_MV_THRESHOLD`] is exceeded so the contaminated
    /// charge-rail voltage never leaks into `status()`. Fed a windowed PEAK
    /// (see `peak_sampler` below), not the raw per-poll reading — see
    /// `firmware_core::battery`'s "(B)" doc section.
    settled_mv: u32,
    /// Charging (external-power-present) state — see [`battery_poll_step`].
    cached_charging: bool,
    /// Whether `settled_mv` is a trustworthy basis to persist — see
    /// [`advance_settled_confirmed`] and `firmware_core::battery`'s "(A)"
    /// doc section.
    confirmed: bool,
    /// Coarse bucket, computed directly from `settled_mv`/`cached_charging`
    /// with millivolt hysteresis — no percent-domain state survives here as
    /// of 2026-08-22 (`meshcadet-battery-three-state-pipeline`; see
    /// [`battery_level_bucket`] and `firmware_core::battery`'s "Three-state
    /// voltage-domain bucket" doc section).
    level: BatteryLevel,
    /// Peak-over-window sampler feeding `settled_mv`'s update cadence — see
    /// [`PeakWindowSampler`] and `firmware_core::battery`'s "(B)" doc
    /// section.
    peak_sampler: PeakWindowSampler,
    /// Last live (post-divider, averaged) ADC millivolt reading — diagnostic
    /// only, updated unconditionally on every poll, never frozen by the
    /// charging latch and never filtered by the peak-window sampler above.
    /// See module docs' "ADC calibration ... raw_mv" section.
    live_mv: u32,
    /// This boot's raw, un-peak-held ADC seed sample (`new`'s own
    /// `initial_mv`) — fixed for the life of the boot. Added 2026-08-22 as a
    /// HIL capture probe (`BatteryStatus::boot_mv`'s own field doc) — the
    /// discriminator between "inrush sag" and "the pack really is low."
    boot_mv: u32,
    /// Uptime ms of the last ADC sample (poll throttling).
    last_poll_ms: u64,

    /// NVS partition handle for `settled_mv` persistence — see module doc's
    /// "NVS layout" section. Cheap to hold (reference-counted handle, same
    /// pattern as `gps::GpsDriver`'s own `nvs_partition` field).
    nvs_partition: EspNvsPartition<NvsDefault>,
    /// Last `settled_mv` value actually written to flash (`None` if never
    /// written this boot AND nothing was restored at construction).
    last_persisted_mv: Option<u32>,
    /// Uptime ms of the last successful NVS write — write-wear-bound
    /// bookkeeping for [`should_persist_settled_mv`].
    last_persist_ms: u64,
}

impl<'d> BatteryDriver<'d> {
    /// Construct the battery driver from the ADC1 peripheral and GPIO4.
    ///
    /// Takes one initial sample immediately so `status()` returns real data
    /// from the first call rather than [`BatteryStatus::unknown`]. A failed
    /// initial read falls back to the empty-pack floor rather than failing
    /// firmware bring-up over a non-critical status readout.
    ///
    /// Restores a persisted `settled_mv` from NVS (see module doc's "NVS
    /// layout" section) and seeds boot state via [`seed_boot_state`] — this
    /// is what closes the documented boot-while-plugged gap (see
    /// `firmware_core::battery`'s "(A)" doc section): a device that boots
    /// already on external power reports the last known GOOD off-power
    /// reading, not the raw contaminated first sample, whenever a prior
    /// confirmed value exists on flash.
    ///
    /// Channel construction itself (below) still propagates its error via
    /// `?`, matching this crate's boot-sequence convention for every other
    /// peripheral (see the call site in `main.rs::run()`). Requesting
    /// `Calibration::Curve` adds one more way that `?` can fire — scheme
    /// creation fails if the SoC's ADC calibration eFuse were unprogrammed —
    /// but Espressif programs that eFuse at the factory on all production
    /// ESP32-S3 silicon, so this is not considered a realistic field risk on
    /// this hardware; a same-boot fallback to `Calibration::None` was
    /// considered and rejected because `AdcChannelDriver::new` consumes
    /// `pin` by value, so a first attempt's failure cannot hand it back for
    /// a second attempt with the pin type available at this call site.
    pub fn new(
        adc1: ADC1<'d>,
        pin: Gpio4<'d>,
        now_ms: u64,
        nvs_partition: EspNvsPartition<NvsDefault>,
    ) -> anyhow::Result<Self> {
        let adc = Rc::new(
            AdcDriver::new(adc1).map_err(|e| anyhow::anyhow!("battery ADC unit init: {:?}", e))?,
        );
        // `calibration: Calibration::Curve` requests the ESP32-S3's factory
        // eFuse curve-fitting scheme instead of the default `Calibration::None`
        // (uncalibrated piecewise-linear attenuation table), which reads low
        // near the top of the ADC's range — see module docs' "ADC calibration"
        // section for the HIL report this was diagnosed against.
        let config = AdcChannelConfig {
            attenuation: DB_12,
            calibration: Calibration::Curve,
            ..Default::default()
        };
        let mut chan = AdcChannelDriver::new(adc.clone(), pin, &config)
            .map_err(|e| anyhow::anyhow!("battery ADC channel init: {:?}", e))?;

        let initial_mv = read_battery_mv(&mut chan).unwrap_or(BATTERY_EMPTY_MV);

        let persisted_mv = load_persisted_settled_mv(&nvs_partition);
        let (settled_mv, cached_charging, confirmed) = seed_boot_state(persisted_mv, initial_mv);
        let level = battery_level_bucket(BatteryLevel::Unknown, settled_mv, cached_charging);

        log::info!(
            "battery ADC initialised (curve-fitting calibration) — GPIO4 (ADC1_CH3), initial read {} mV, restored settled_mv {:?} from NVS, basis now {} mV ({}%, {})",
            initial_mv,
            persisted_mv,
            settled_mv,
            percent_from_millivolts(settled_mv),
            if cached_charging { "charging" } else { "not charging" },
        );

        Ok(BatteryDriver {
            _adc: adc,
            chan,
            settled_mv,
            cached_charging,
            confirmed,
            level,
            peak_sampler: PeakWindowSampler::new(now_ms, initial_mv),
            live_mv: initial_mv,
            // Provisional boot seed — see `BatteryStatus::boot_mv`'s doc.
            // The first closed peak window below unconditionally overwrites
            // `settled_mv`/`level`, regardless of what this reads.
            boot_mv: initial_mv,
            last_poll_ms: now_ms,
            nvs_partition,
            // A restored value is, by construction, already durably on
            // flash: treat it as "just persisted" so construction doesn't
            // immediately re-write the identical value back.
            last_persisted_mv: persisted_mv,
            last_persist_ms: now_ms,
        })
    }

    /// Poll the battery ADC — a throttled no-op between samples.
    ///
    /// Called on every dispatcher-loop iteration; only actually samples the
    /// ADC every [`BATTERY_POLL_INTERVAL_MS`]. Each sample is fed through
    /// [`PeakWindowSampler`]; [`battery_poll_step`] (and everything derived
    /// from it — `settled_mv`, `charging`, `confirmed`, `level`, NVS
    /// persistence) only actually updates once a ~30 s peak window closes —
    /// see `firmware_core::battery`'s "(B)" doc section.
    pub fn poll(&mut self, now_ms: u64) {
        if now_ms.saturating_sub(self.last_poll_ms) < BATTERY_POLL_INTERVAL_MS {
            return;
        }
        self.last_poll_ms = now_ms;

        match read_battery_mv(&mut self.chan) {
            Ok(mv) => {
                self.live_mv = mv;
                // Diagnostic-only raw-mV trace (see module docs' "raw_mv"
                // section) — feature-gated because at
                // BATTERY_POLL_INTERVAL_MS this would otherwise spam the
                // production log every 2 s. Primary capture path is the host
                // CLI `status` command (`BatteryStatus::raw_mv`); this is a
                // secondary path for a `--features diagnostics` HIL build.
                #[cfg(feature = "diagnostics")]
                log::info!("battery raw read: {} mV", mv);

                let Some(window_peak_mv) = self.peak_sampler.sample(now_ms, mv) else {
                    // Still accumulating within the current ~30s window —
                    // nothing else updates this poll.
                    return;
                };

                let was_charging = self.cached_charging;
                let (settled_mv, charging, confirmed, level) = battery_window_close_step(
                    self.settled_mv,
                    self.level,
                    self.confirmed,
                    window_peak_mv,
                );
                self.settled_mv = settled_mv;
                self.cached_charging = charging;
                self.confirmed = confirmed;
                self.level = level;

                // Log the transition (not every window) — the one field
                // signal that lets a HIL run be diagnosed after the fact
                // without a debugger: confirms whether a plug/unplug was
                // actually seen by this heuristic, and at what basis it
                // froze/resynced.
                if charging != was_charging {
                    log::info!(
                        "battery charging state -> {} (window peak {} mV, percent basis now {} mV / {}%)",
                        charging,
                        window_peak_mv,
                        settled_mv,
                        percent_from_millivolts(settled_mv),
                    );
                }

                // Bounded-write-wear NVS persist — see module doc's "NVS
                // layout" section and `firmware_core::battery`'s "(A)"
                // section for the exact policy.
                let ms_since_last_persist = now_ms.saturating_sub(self.last_persist_ms);
                if should_persist_settled_mv(
                    self.last_persisted_mv,
                    settled_mv,
                    ms_since_last_persist,
                    self.confirmed,
                ) {
                    persist_settled_mv(&self.nvs_partition, settled_mv);
                    self.last_persisted_mv = Some(settled_mv);
                    self.last_persist_ms = now_ms;
                }
            }
            Err(e) => {
                log::warn!("battery ADC read failed: {:?} — keeping last known status", e);
            }
        }
    }

    /// Return the current battery status snapshot (percent + charging +
    /// diagnostic raw mV + held raw mV + boot mV + confirmed + coarse level
    /// bucket).
    ///
    /// `percent` is a pure, stateless `percent_from_millivolts(settled_mv)`
    /// as of 2026-08-22 — no percent-domain filter sits between the basis
    /// and this field anymore (see `firmware_core::battery`'s "Three-state
    /// voltage-domain bucket" doc section), so a charge-inflated read still
    /// never surfaces here (that protection is `battery_poll_step`'s
    /// freeze/latch, unchanged). `raw_mv` IS the raw live voltage, unfrozen
    /// and unfiltered by the peak-window sampler, for diagnosis (see module
    /// docs' "raw_mv" section). `held_raw_mv` is the underlying `settled_mv`
    /// basis in millivolts — the SAME basis `percent`/`level` derive from.
    /// `boot_mv` is this boot's raw seed sample, fixed for the boot's
    /// lifetime. `confirmed` is the trust latch. `level` is the coarse
    /// voltage-domain bucket.
    pub fn status(&self) -> BatteryStatus {
        BatteryStatus {
            percent: percent_from_millivolts(self.settled_mv),
            charging: self.cached_charging,
            raw_mv: self.live_mv,
            held_raw_mv: self.settled_mv,
            boot_mv: self.boot_mv,
            confirmed: self.confirmed,
            level: self.level,
        }
    }
}

/// Sample the battery ADC channel [`BATTERY_SAMPLES`] times and return the
/// averaged pack voltage in millivolts (post divider-scaling).
fn read_battery_mv<'d>(
    chan: &mut AdcChannelDriver<'d, ADCCH3<ADCU1>, Rc<AdcDriver<'d, ADCU1>>>,
) -> anyhow::Result<u32> {
    let mut acc: u32 = 0;
    for _ in 0..BATTERY_SAMPLES {
        let adc_mv = chan
            .read()
            .map_err(|e| anyhow::anyhow!("battery ADC sample: {:?}", e))? as u32;
        acc += adc_mv;
    }
    Ok((acc / BATTERY_SAMPLES) * DIVIDER_RATIO)
}
