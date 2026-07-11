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
//! ADC1 read path (`BatteryDriver`), which needs actual hardware.
//! `pub use firmware_core::battery::*;` below re-exports the pure half so
//! every existing call site (`battery::BatteryStatus`,
//! `crate::battery::clamp_raw_mv_for_wire`, …) resolves unchanged. See
//! `docs/adr/0005-firmware-core-extraction.md`.

use std::rc::Rc;

use esp_idf_hal::adc::attenuation::DB_12;
use esp_idf_hal::adc::oneshot::config::{AdcChannelConfig, Calibration};
use esp_idf_hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_hal::adc::{ADCCH3, ADCU1, ADC1};
use esp_idf_hal::gpio::Gpio4;

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
/// iteration (unlike GPS, which drains a UART byte stream every tick).
const BATTERY_POLL_INTERVAL_MS: u64 = 2_000;

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
    /// charge-rail voltage never leaks into `status()`.
    settled_mv: u32,
    /// Charging (external-power-present) state — see [`battery_poll_step`].
    cached_charging: bool,
    /// Last live (post-divider, averaged) ADC millivolt reading — diagnostic
    /// only, updated unconditionally on every poll, never frozen by the
    /// charging latch. See module docs' "ADC calibration ... raw_mv" section.
    live_mv: u32,
    /// Uptime ms of the last ADC sample (poll throttling).
    last_poll_ms: u64,
}

impl<'d> BatteryDriver<'d> {
    /// Construct the battery driver from the ADC1 peripheral and GPIO4.
    ///
    /// Takes one initial sample immediately so `status()` returns real data
    /// from the first call rather than [`BatteryStatus::unknown`].  A failed
    /// initial read falls back to the empty-pack floor rather than failing
    /// firmware bring-up over a non-critical status readout.
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
    pub fn new(adc1: ADC1<'d>, pin: Gpio4<'d>, now_ms: u64) -> anyhow::Result<Self> {
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
        log::info!(
            "battery ADC initialised (curve-fitting calibration) — GPIO4 (ADC1_CH3), initial read {} mV ({}%)",
            initial_mv,
            percent_from_millivolts(initial_mv),
        );

        // Run the initial sample through the same state-transition logic as
        // every later poll (rather than assuming "not charging"), so a device
        // that happens to boot already on external power is correctly
        // flagged `charging: true` immediately instead of only on the next
        // poll — see module docs' "Fix" section residual-gap note for what
        // this can and cannot recover about `percent` in that boot case.
        let (settled_mv, cached_charging) = battery_poll_step(initial_mv, initial_mv);

        Ok(BatteryDriver {
            _adc: adc,
            chan,
            settled_mv,
            cached_charging,
            live_mv: initial_mv,
            last_poll_ms: now_ms,
        })
    }

    /// Poll the battery ADC — a throttled no-op between samples.
    ///
    /// Called on every dispatcher-loop iteration; only actually samples the
    /// ADC every [`BATTERY_POLL_INTERVAL_MS`]. Drives [`battery_poll_step`] to
    /// update the percent basis and the latched charging state.
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

                let was_charging = self.cached_charging;
                let (settled_mv, charging) = battery_poll_step(self.settled_mv, mv);
                self.settled_mv = settled_mv;
                self.cached_charging = charging;
                // Log the transition (not every poll) — the one field signal
                // that lets a HIL run be diagnosed after the fact without a
                // debugger: confirms whether a plug/unplug was actually seen
                // by this heuristic, and at what basis it froze/resynced.
                if charging != was_charging {
                    log::info!(
                        "battery charging state -> {} (live {} mV, percent basis now {} mV / {}%)",
                        charging,
                        mv,
                        settled_mv,
                        percent_from_millivolts(settled_mv),
                    );
                }
            }
            Err(e) => {
                log::warn!("battery ADC read failed: {:?} — keeping last known status", e);
            }
        }
    }

    /// Return the current battery status snapshot (percent + charging +
    /// diagnostic raw mV + held raw mV).
    ///
    /// `percent` is derived from `settled_mv` — the percent basis — never
    /// from the raw live voltage, so a charge-inflated read never surfaces
    /// here (see module docs). `raw_mv` IS the raw live voltage, unfrozen,
    /// for diagnosis (see module docs' "raw_mv" section). `held_raw_mv` is
    /// that same `settled_mv` basis, in millivolts rather than percent (see
    /// module docs' "`held_raw_mv`" section).
    pub fn status(&self) -> BatteryStatus {
        BatteryStatus {
            percent: percent_from_millivolts(self.settled_mv),
            charging: self.cached_charging,
            raw_mv: self.live_mv,
            held_raw_mv: self.settled_mv,
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
