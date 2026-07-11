// SPDX-License-Identifier: GPL-3.0-only
//! Battery status driver for the LilyGo T-Deck Plus — ADC voltage-divider read.
//!
//! # Hardware (feasibility check)
//!
//! The T-Deck Plus reports battery state through a **plain ADC voltage
//! divider**, not a fuel-gauge IC. There is no AXP192/AXP2101 power-management
//! chip on this board (unlike the T-Beam family, which has one — see
//! `meshcore-dev/MeshCore`'s `TBeamBoard.h`, `I2C_PMU_ADD 0x34`). Confirmed
//! against two independent upstream sources:
//! - LilyGo's own `examples/UnitTest/utilities.h`: `#define BOARD_BAT_ADC 4`.
//! - `meshcore-dev/MeshCore`'s `variants/lilygo_tdeck/TDeckBoard.h`:
//!   `#define PIN_VBAT_READ 4` / `#define ADC_MULTIPLIER (2.0f * 3.3f * 1000)`
//!   read via `analogRead(PIN_VBAT_READ)` at 12-bit resolution — i.e. exactly
//!   the plain-ADC path this module implements, not a PMU register read.
//!
//! | Signal | GPIO | Notes |
//! |--------|------|-------|
//! | Battery ADC | 4 | `BOARD_BAT_ADC` / `PIN_VBAT_READ` — ADC1 channel 3 on ESP32-S3 |
//!
//! GPIO4 is not claimed by any other peripheral in this firmware — SPI2
//! (40/41/38/12/9), I2C1 (18/8), UART1 (43/44), and the discrete GPIOs used
//! for reset/backlight/interrupts (see `docs/adr/0003-ui-toolkit.md`'s pin
//! table and `gps.rs`'s GPIO43/44 note) leave GPIO4 free. No collision.
//!
//! The pack is wired through a 2:1 resistor divider (LilyGo's own
//! `ADC_MULTIPLIER = 2.0 * 3.3 V`), so `Vbat = 2 * Vadc`. Reads are taken at
//! 12-bit resolution with 12 dB attenuation (~0–3.1 V ADC input range on the
//! S3) and averaged over [`BATTERY_SAMPLES`] samples to damp ADC noise,
//! mirroring the reference `getBattMilliVolts()` implementation.
//!
//! # No charge-status pin — "charging" is INFERRED, not read directly
//!
//! Unlike a fuel-gauge IC, a plain voltage divider carries no charge/discharge
//! signal, and — per a HIL bug report (2026-07-05,
//! charge-inflation) — there is also **no reachable external-power-present
//! signal** on this board to substitute for one. This was checked, not
//! assumed: LilyGo's own `examples/UnitTest/utilities.h` and
//! `meshcore-dev/MeshCore`'s `TDeckBoard.h` / `ESP32Board.h` (the same
//! upstream sources that established the ADC-only hardware fact above) define
//! no VBUS-detect, charge-status, or PMU pin for the T-Deck Plus anywhere —
//! only `BOARD_BAT_ADC`/`PIN_VBAT_READ` on GPIO4. The USB-Serial-JTAG
//! peripheral used for the host CLI is a *data*-presence signal (is a USB
//! host enumerated on the console endpoint), not a *power*-presence signal
//! (VBUS can be present — and charging can be happening — with no host
//! attached to the console at all, e.g. a wall-charger brick); it was
//! considered and rejected as a charging-status substitute for that reason.
//! So charging must be inferred from the ADC voltage alone, and the fix below
//! is a voltage-domain mitigation, not a new hardware signal.
//!
//! **2026-07-05 update:** a follow-on HIL capture (below) found that the ADC
//! voltage, inferred alone, still carries a reliable in-band proxy for
//! "external power present" — a raw reading physically impossible for a
//! battery. It is a proxy inferred from the same GPIO4 divider, not a new
//! pin, so the hardware fact above (no dedicated VBUS-detect pin) still
//! holds.
//!
//! ## The bug this module works around
//!
//! The pack's *terminal* voltage while on external power is elevated toward
//! the charger's ~4.2 V CC/CV setpoint well above the *open-circuit* voltage
//! the same true state of charge would show at rest — so a plain
//! voltage→percent map reads ~100% while actually charging, even when the
//! true SoC is much lower (confirmed HIL: reads 100% while charging, collapses
//! to the true ~36% the instant external power is removed and the pack
//! settles).
//!
//! An initial fix inferred "charging" from a *rise* in the live ADC voltage
//! above a resting baseline (latched with hysteresis so a later charger
//! float/CV plateau — where the voltage stops climbing but power stays
//! connected — didn't fall back to "not charging"). A follow-on HIL capture
//! (2026-07-05) then handed us a number the rise heuristic
//! could not use: **raw 4888 mV while plugged in** — a physical impossibility
//! for a single-cell pack (max ~4200 mV at full charge). That number proves
//! the ADC divider node reads the USB/charge rail directly whenever external
//! power is connected, not merely an IR-drop-elevated battery terminal. It
//! also explains why the rise trigger sometimes failed to engage even though
//! it looked correct on paper: it fires by comparing the live reading against
//! a **prior** resting-baseline sample, so on any poll where no such
//! below-ceiling prior sample exists yet (e.g. the very first sample taken
//! while already on power, with nothing to rise *from*), no delta is ever
//! seen, and the contaminated raw voltage leaks straight through as the
//! reported percent — exactly the observed "100% / not charging" report.
//!
//! ## Fix: freeze the percent basis, detect power via an impossible-voltage threshold
//!
//! [`BatteryDriver`] keeps a `settled_mv` value — the percent basis — that is
//! kept in lock-step with the live ADC reading **only while the live reading
//! is at or below [`EXTERNAL_POWER_MV_THRESHOLD`]**. [`BatteryStatus::percent`]
//! always derives from `settled_mv`, never the raw live voltage, so a
//! charge-rail-contaminated read never surfaces there. The moment a poll's
//! live voltage exceeds that threshold, external power is inferred present:
//! `settled_mv` freezes at whatever it last held (the last known good,
//! off-power SoC) and `charging` reports `true` (the fix
//! direction: hold the last valid unplugged SoC rather than the contaminated
//! live read). Unlike the superseded rise trigger, this is a **stateless,
//! per-poll** check against a fixed, physically-grounded ceiling — it needs
//! no delta from a prior sample, so it engages correctly on the very first
//! poll above the ceiling, holds unconditionally through a charger's
//! float/CV plateau (there is no "stopped rising" moment for a threshold
//! check to be confused by), and clears the instant the live voltage falls
//! back at/under the threshold (an actual unplug) — at which point
//! `settled_mv` resyncs to that fresh post-unplug reading, so any real
//! capacity gained during the charge session is picked up rather than the
//! basis staying frozen forever. See [`battery_poll_step`] for the exact,
//! host-tested state transition, and its test module for a full
//! plug/plateau/unplug regression matching the HIL report.
//!
//! This remains an honest best-effort heuristic, not fuel-gauge-grade truth:
//! a device that *boots* already attached to a charger has no prior off-power
//! sample to freeze `settled_mv` at, so `percent` will still show that first
//! contaminated reading until the pack is next seen at/under
//! [`EXTERNAL_POWER_MV_THRESHOLD`] (i.e. unplugged once). `charging`, however,
//! is now correctly reported `true` immediately in that case too, since the
//! threshold check needs no prior sample — a strict improvement over the
//! superseded rise trigger, which reported `false` in exactly that case. The
//! residual `percent` gap is a direct consequence of "no power-present signal
//! exists on this board other than the contaminated rail itself" above, and
//! closing it fully would require persisting a last-known-good SoC across
//! reboots — out of scope here.
//!
//! ## ADC calibration (2026-07-05 redirect) and the diagnostic `raw_mv` field
//!
//! The channel was originally opened with `AdcChannelConfig { attenuation:
//! DB_12, ..Default::default() }` — `..Default::default()` leaves
//! `calibration: Calibration::None` (esp-idf-hal 0.46's default), so
//! `AdcChannelDriver::read()` was converting raw counts to millivolts with
//! the *uncalibrated* piecewise-linear attenuation-curve table
//! (`DirectConverter`), not the ESP32-S3's factory eFuse curve-fitting
//! calibration (`esp_adc_cal`/`esp_adc`'s `Calibration::Curve` scheme, which
//! the S3 supports). The uncalibrated table is known to read low in the
//! upper part of the ADC's range — exactly where a near-full pack (~4.2 V,
//! ~2.1 V post 2:1 divider) sits — which was the prime suspect for a HIL
//! report of the gauge reading ~36% while the charge-complete LED indicated a
//! full pack. `Calibration::Curve` is now requested below so `read()` returns
//! the factory-curve-fit millivolts instead.
//!
//! To let that be verified with data instead of inferred, [`BatteryStatus`]
//! now carries a third field, `raw_mv`: the last live (post-divider, still
//! averaged over [`BATTERY_SAMPLES`]) ADC millivolt reading, updated on every
//! poll regardless of the charging latch above — i.e. it is NOT frozen at
//! `settled_mv` while charging, unlike `percent`. This is a deliberate,
//! temporary relaxation of the 2026-07-03 "expose only percent+charging"
//! scoping, for diagnosis only: `raw_mv` is wired into the host CLI's
//! `status` command (`protocol::provisioning::RspStatusPayload`) and this
//! module's own init/poll log lines, but neither the on-device admin-menu
//! screen (`ui/screens/admin_menu.rs::format_battery_display`) nor the
//! over-the-air telemetry RESPONSE (`main.rs::build_telemetry_response`)
//! reads this field — both consume only `percent`/`charging` from this same
//! struct, so raw mV never reaches the on-device UI or the air.
//!
//! ### Reconciliation with the charge-inflation "hold last unplugged SoC" fix
//!
//! The fix that landed [`battery_poll_step`]'s freeze/latch logic (just
//! above) and this ADC-calibration fix address two **different, independent**
//! mechanisms, not one bug seen from two angles:
//!
//! - **Calibration** (this section): a fixed measurement error in
//!   raw-counts→mV conversion, present at every sample regardless of charging
//!   state. Fixing it shifts every reading (charging or not) toward the true
//!   voltage.
//! - **Charge inflation** (`battery_poll_step`): even with perfectly
//!   calibrated mV, a pack's *terminal* voltage while a charge current is
//!   flowing sits above its *open-circuit* voltage for the same true state of
//!   charge (internal-resistance IR drop + the charger's own CC/CV
//!   regulation) — a real electrical effect, not a measurement artifact.
//!
//! So the freeze/latch logic is not superseded by the calibration fix and is
//! kept as-is: calibration corrects *what the ADC reports for a given pin
//! voltage*; the freeze/latch logic corrects *for the pin voltage itself
//! being elevated by charging*. Both can be true simultaneously (as the HIL
//! report's 36%-while-LED-off symptom suggests: an under-read pack that is
//! also genuinely below 100%). A raw-mV HIL capture (the
//! acceptance criterion for this fix) is what distinguishes, WITH DATA, how much
//! of the ~3624 mV reading was calibration error vs. genuine partial charge.
//! If that capture
//! shows the calibration fix alone now reports ~4200 mV on a charge-LED-off
//! pack, no further change to the freeze/latch logic is needed; if a
//! meaningful gap remains, that is new evidence for a follow-on fix, not
//! a reason to have preemptively removed working charge-inflation logic here.
//!
//! **Follow-on outcome (2026-07-05):**
//! the capture landed exactly the "meaningful gap" scenario flagged above, and
//! then some — unplugged, the calibration fix alone brought the reading to a
//! plausible ~4038 mV/82%; plugged in, `raw_mv` read 4888 mV, *above* the
//! physical single-cell ceiling entirely, meaning the divider is reading the
//! charge rail, not an IR-elevated battery terminal. The freeze/latch
//! *concept* was not superseded, but its *trigger* was: the rise-based
//! comparison was replaced with the impossible-voltage threshold check
//! described in the "Fix" section above — see that section and
//! [`battery_poll_step`] for the current mechanism.
//!
//! ## `held_raw_mv` — the last-unplugged raw reading, contamination-free (2026-07-05 follow-on)
//!
//! On this board USB carries BOTH the host CLI UART AND charge power, so
//! *any* CLI read is necessarily taken while the charger's contaminated
//! ~4.2-4.9 V rail is on the pin — `raw_mv` (above) can never show a
//! clean battery voltage while a cable is attached to read it.
//! `settled_mv` (the percent basis — see the "Fix" section above) is already
//! exactly that clean reading: it tracks the live voltage only while not
//! charging, and freezes at the last pre-charge value the instant a charge is
//! detected. [`BatteryStatus`] now exposes that basis directly, in
//! millivolts, as `held_raw_mv` — distinct from both `raw_mv` (live, rail-
//! contaminated while charging) and `percent` (the same basis, but lossy-
//! rounded through [`percent_from_millivolts`]). Reading `held_raw_mv` after
//! unplugging and replugging (to re-attach the CLI) surfaces the exact
//! millivolt figure the pack settled to before the charger went on — the
//! instrument needed to confirm or refute the full-scale anchor
//! below with real hardware data.
//!
//! ## Full-scale anchor: resting-voltage curve, not charging voltage (2026-07-05 follow-on)
//!
//! [`percent_from_millivolts`] used to be a straight line from
//! [`BATTERY_EMPTY_MV`] to [`BATTERY_FULL_MV`] (4200 mV) — but 4200 mV is the
//! charger's CC/CV *terminal* voltage, not a voltage a rested pack ever
//! reaches: a rested single-cell Li-ion/LiPo settles to roughly 4.10-4.15 V
//! at true 100% SoC. Anchoring the map at 4200 mV therefore capped every
//! rested-full pack at ~89-94%, structurally — confirmed HIL: ~82% unplugged
//! on a pack the charge-complete LED reported full. `percent_from_millivolts`
//! now interpolates over [`RESTING_SOC_CURVE`], a piecewise open-circuit-
//! voltage → SoC table anchored at [`RESTING_FULL_MV`] (4150 mV, the top of
//! the standard rested-full range) for 100%, keeping [`BATTERY_EMPTY_MV`]
//! (3300 mV) for 0%. The breakpoints approximate the well-known flat-middle /
//! steep-ends shape of a Li-ion discharge curve rather than a single straight
//! line, so mid-range readings track real pack behavior instead of a coarse
//! linear guess. `BATTERY_FULL_MV` is kept as a named constant purely to
//! document the charging terminal voltage referenced elsewhere in these
//! docs — it is no longer read by `percent_from_millivolts`.
//!
//! This is still a default curve, not a per-pack calibration: if a
//! `held_raw_mv` capture at a known-full (charge-LED-off) charge
//! state comes back suspiciously low (e.g. under ~4.0 V), that points at a
//! residual ADC under-read beyond this curve (the calibration fix moved
//! 36%→82% but may not be fully accurate) — the fix for that is a follow-on
//! ADC-calibration effort, not further lowering this anchor to paper over a
//! measurement error.

// This module is pure Rust with no ADC/hardware dependency — see
// `firmware::battery` for `BatteryDriver` (the real ADC1 read path), which
// stays in the firmware crate and re-exports the pure helpers/`BatteryStatus`
// below via a `pub use firmware_core::battery::*;` shim. See
// `docs/adr/0005-firmware-core-extraction.md`.
//
// `BATTERY_SAMPLES`/`DIVIDER_RATIO` (the ADC-sampling tunables) and
// `BATTERY_POLL_INTERVAL_MS` stay in `firmware::battery` alongside
// `BatteryDriver` — they only matter to the real ADC read path.

// ── Tunables ──────────────────────────────────────────────────────────────────

/// Empty-pack cutoff in millivolts → 0%. Conservative single-cell Li-ion
/// "empty under light load" figure.
pub const BATTERY_EMPTY_MV: u32 = 3300;

/// Charging terminal (CC/CV setpoint) millivolts. Standard single-cell
/// Li-ion/LiPo full-*charge* voltage — kept as a named constant purely to
/// document that figure for the rest of this module's docs. **Not** the
/// percent gauge's 100% anchor: see [`RESTING_FULL_MV`] and the module docs'
/// "Full-scale anchor" section for why a rested pack never reaches this
/// voltage and anchoring here structurally under-reads a full battery.
/// `#[allow(dead_code)]`: no longer read by any non-test production code
/// (only by this module's own `#[cfg(test)]` regressions) now that
/// `percent_from_millivolts` anchors on `RESTING_FULL_MV` instead — kept
/// `pub` anyway as reference documentation for the charging-voltage figure.
#[allow(dead_code)]
pub const BATTERY_FULL_MV: u32 = 4200;

/// Rested (open-circuit), not charging, millivolts → 100%. The top of the
/// standard ~4.10-4.15 V rested-full range for a single-cell Li-ion/LiPo —
/// see module docs' "Full-scale anchor" section. This, not
/// [`BATTERY_FULL_MV`], is what [`percent_from_millivolts`] anchors 100% at.
pub const RESTING_FULL_MV: u32 = 4150;

/// Piecewise open-circuit-voltage → state-of-charge breakpoints for a
/// resting (non-charging) single-cell Li-ion/LiPo pack, approximating the
/// well-known flat-middle / steep-ends shape of a Li-ion discharge curve —
/// see module docs' "Full-scale anchor" section. `(millivolts, percent)`,
/// strictly increasing in both columns; [`percent_from_millivolts`] linearly
/// interpolates between adjacent points.
const RESTING_SOC_CURVE: &[(u32, u8)] = &[
    (BATTERY_EMPTY_MV, 0),
    (3_500, 5),
    (3_600, 10),
    (3_700, 20),
    (3_750, 30),
    (3_800, 42),
    (3_850, 55),
    (3_900, 67),
    (3_950, 77),
    (4_000, 85),
    (4_050, 91),
    (4_100, 96),
    (RESTING_FULL_MV, 100),
];

/// Millivolt ceiling above which a reading is physically impossible for a
/// single-cell Li-ion/LiPo pack and therefore reliably indicates the ADC
/// divider node is reading the USB/charge rail rather than the pack itself —
/// i.e. external power is present. Set ~700 mV above [`BATTERY_FULL_MV`]:
/// wide enough that it is never brushed by ADC noise or a genuinely
/// overcharged/out-of-spec cell, but well below the HIL-observed on-power
/// reading of 4888 mV (see module docs' "Fix" section) that this constant is
/// calibrated against. Unlike the superseded rise/drop hysteresis pair this
/// replaces, a single absolute ceiling needs no history: it is a stateless,
/// per-poll check (see [`battery_poll_step`]).
pub const EXTERNAL_POWER_MV_THRESHOLD: u32 = 4300;

// ── BatteryStatus — the ONE shared representation ────────────────────────────

/// Battery status: charge percentage, charging state, and (diagnostic-only)
/// raw millivolts.
///
/// `percent`/`charging` are the two fields originally scoped in
/// (2026-07-03); `raw_mv` is a temporary, diagnosis-only relaxation of that
/// scoping added 2026-07-05 for the ADC-calibration investigation — see this
/// module's "ADC calibration ... raw_mv" doc section.
///
/// This is the single representation wired into all three consumers: the
/// radio telemetry RESPONSE (`main.rs::build_telemetry_response`), the host
/// `status` command (via `protocol::provisioning::RspStatusPayload`), and the
/// on-device admin-menu display — so all three report the same numbers by
/// construction rather than three independent reads/formats. Only the host
/// `status` command reads `raw_mv`; the other two consume solely
/// `percent`/`charging`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryStatus {
    /// Charge percentage, `0..=100`.
    pub percent: u8,
    /// `true` if the pack is inferred to be charging (see module docs).
    pub charging: bool,
    /// Last live (post-divider, averaged) ADC millivolt reading — diagnostic
    /// only, NOT frozen by the charging latch that `percent` is (see module
    /// docs' "ADC calibration ... raw_mv" section). Surfaced via the host CLI
    /// `status` command; deliberately not read by the on-device admin-menu
    /// screen or the over-the-air telemetry RESPONSE.
    pub raw_mv: u32,
    /// Last known non-charge-inflated ("resting") millivolt reading — the
    /// same `settled_mv` basis `percent` is derived from, but exposed as raw
    /// millivolts instead of a lossy-rounded percentage. Unlike `raw_mv`,
    /// this is frozen while charging (contamination-free by construction) —
    /// see module docs' "`held_raw_mv`" section. Surfaced via the host CLI
    /// `status` command only, same scoping as `raw_mv`.
    pub held_raw_mv: u32,
}

impl BatteryStatus {
    /// Status before the first ADC sample has been taken (device just booted).
    pub const fn unknown() -> Self {
        BatteryStatus {
            percent: 0,
            charging: false,
            raw_mv: 0,
            held_raw_mv: 0,
        }
    }
}

// ── Pure helpers (host-testable, no ADC dependency) ──────────────────────────

/// Clamp a `raw_mv` reading to the `battery_raw_mv: u16` wire field
/// (`protocol::provisioning::RspStatusPayload`).
///
/// Saturating, not truncating: a real pack voltage is always well under
/// `u16::MAX` millivolts, so a legitimate reading is never affected. This
/// only guards against a corrupt/overflowed ADC sample silently wrapping
/// into a small, plausible-looking wrong value on the wire instead of
/// pinning at the (visibly implausible) ceiling.
pub fn clamp_raw_mv_for_wire(raw_mv: u32) -> u16 {
    raw_mv.min(u16::MAX as u32) as u16
}

/// Map a battery-pack millivolt reading to a `0..=100` percentage.
///
/// Piecewise-linear interpolation over [`RESTING_SOC_CURVE`], clamped to
/// `[BATTERY_EMPTY_MV, RESTING_FULL_MV]` — anchors 100% at a realistic
/// rested-full voltage rather than the charger's terminal voltage (see module
/// docs' "Full-scale anchor" section). This assumes `mv` is a resting
/// (non-charging) reading, same as the old linear map; [`BatteryDriver`]
/// only ever calls this on `settled_mv`, which the freeze/latch logic (see
/// [`battery_poll_step`]) guarantees is non-charge-inflated.
pub fn percent_from_millivolts(mv: u32) -> u8 {
    if mv <= BATTERY_EMPTY_MV {
        return 0;
    }
    if mv >= RESTING_FULL_MV {
        return 100;
    }
    for window in RESTING_SOC_CURVE.windows(2) {
        let (lo_mv, lo_pct) = window[0];
        let (hi_mv, hi_pct) = window[1];
        if mv <= hi_mv {
            let span_mv = (hi_mv - lo_mv) as u64;
            let span_pct = (hi_pct - lo_pct) as u64;
            let offset = (mv - lo_mv) as u64;
            return (lo_pct as u64 + (offset * span_pct) / span_mv) as u8;
        }
    }
    // Unreachable: the `mv >= RESTING_FULL_MV` guard above already handles
    // everything at/beyond the curve's last breakpoint. Kept as a safe
    // fallback rather than a `panic!`/`unreachable!` for a non-critical
    // status readout.
    100
}

/// One poll-cycle charging/percent-basis state transition (host-testable, no
/// ADC dependency) — the exact logic [`BatteryDriver::poll`] drives.
///
/// - `settled_mv` is the current percent basis: the last known
///   off-power/valid voltage. [`BatteryStatus::percent`] is always derived
///   from this, never from the raw live voltage.
/// - `live_mv` is this poll's fresh ADC reading.
///
/// Returns the updated `(settled_mv, charging)`.
///
/// This is a **stateless** decision — it needs no charging flag or peak
/// tracker carried in from the previous poll, unlike the rise/drop hysteresis
/// pair this superseded. Whether external power is present is decided fresh
/// every poll, purely from `live_mv` against [`EXTERNAL_POWER_MV_THRESHOLD`],
/// a fixed physical ceiling — so it engages correctly even on the very first
/// poll that is over the ceiling (no prior baseline needed), stays correctly
/// latched through a charger's float/CV plateau (nothing can "stop rising"
/// for a threshold check to misread), and clears the instant the live
/// voltage falls back to a battery-plausible reading. See the module docs'
/// "Fix" section for the rationale, and this module's test suite for a full
/// plug-in / plateau / unplug regression matching the HIL bug report.
pub fn battery_poll_step(settled_mv: u32, live_mv: u32) -> (u32, bool) {
    if live_mv > EXTERNAL_POWER_MV_THRESHOLD {
        // External power present: the raw reading is contaminated by the
        // charge rail. Hold the percent basis at its last known good value
        // rather than let the impossible voltage leak into `percent`.
        return (settled_mv, true);
    }
    // Off external power (or a plausible, uncontaminated reading): the live
    // voltage IS the percent basis.
    (live_mv, false)
}

// ── Tests ─────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_raw_mv_for_wire_passes_through_realistic_readings() {
        assert_eq!(clamp_raw_mv_for_wire(0), 0);
        assert_eq!(clamp_raw_mv_for_wire(3624), 3624);
        assert_eq!(clamp_raw_mv_for_wire(4200), 4200);
    }

    #[test]
    fn clamp_raw_mv_for_wire_saturates_instead_of_wrapping() {
        // A corrupt/overflowed sample must saturate at u16::MAX, not silently
        // wrap into a small, plausible-looking wrong value (e.g. `as u16`
        // truncation of 65_536 would wrap to 0 — indistinguishable from "no
        // reading yet").
        assert_eq!(clamp_raw_mv_for_wire(u16::MAX as u32 + 1), u16::MAX);
        assert_eq!(clamp_raw_mv_for_wire(u32::MAX), u16::MAX);
    }

    #[test]
    fn percent_clamps_at_floor_and_ceiling() {
        assert_eq!(percent_from_millivolts(0), 0);
        assert_eq!(percent_from_millivolts(BATTERY_EMPTY_MV), 0);
        assert_eq!(percent_from_millivolts(BATTERY_EMPTY_MV - 100), 0);
        assert_eq!(percent_from_millivolts(RESTING_FULL_MV), 100);
        assert_eq!(percent_from_millivolts(RESTING_FULL_MV + 500), 100);
        // The OLD charging-voltage anchor (4200 mV) must now also read 100% —
        // it is above the new resting-full anchor, not below it — but must
        // NOT be required to reach it exactly (see "full-scale anchor" bug:
        // that was the whole defect being fixed).
        assert_eq!(percent_from_millivolts(BATTERY_FULL_MV), 100);
    }

    #[test]
    fn percent_full_rested_pack_reads_at_or_near_100() {
        // The original HIL bug report: a rested-full pack (charge LED off)
        // must read ~100%, not the ~82-94% the old 3300-4200 linear map
        // structurally capped it at. Standard rested-full range is ~4.10-4.15V.
        for mv in [4_100u32, 4_120, 4_150] {
            let p = percent_from_millivolts(mv);
            assert!(
                p >= 90,
                "expected >=90% at {mv}mV (rested-full range), got {p}%"
            );
        }
    }

    #[test]
    fn percent_midpoint_of_curve_breakpoints_matches_table() {
        // Spot-check a handful of the RESTING_SOC_CURVE breakpoints directly
        // — this is a resting open-circuit-voltage curve, not a straight
        // line, so "half the mV range" is no longer "half the percent range"
        // (that was the old, since-retired, linear-map assumption).
        assert_eq!(percent_from_millivolts(3_500), 5);
        assert_eq!(percent_from_millivolts(3_800), 42);
        assert_eq!(percent_from_millivolts(4_000), 85);
    }

    #[test]
    fn percent_interpolates_between_breakpoints() {
        // Halfway between the (3750, 30) and (3800, 42) breakpoints must read
        // halfway between 30% and 42%.
        let mid_mv = (3_750 + 3_800) / 2;
        assert_eq!(percent_from_millivolts(mid_mv), 36);
    }

    #[test]
    fn percent_monotonic_in_millivolts() {
        // A higher voltage must never map to a lower percentage — a monotonicity
        // violation would show up as a battery reading that "drains" while
        // physically charging.
        let mut last = percent_from_millivolts(BATTERY_EMPTY_MV);
        let mut mv = BATTERY_EMPTY_MV;
        while mv <= RESTING_FULL_MV {
            let p = percent_from_millivolts(mv);
            assert!(p >= last, "percent decreased at {mv}mV: {p}% < {last}%");
            last = p;
            mv += 37; // odd step to exercise non-round intermediate values
        }
    }

    #[test]
    fn unknown_status_is_zero_percent_not_charging() {
        let s = BatteryStatus::unknown();
        assert_eq!(s.percent, 0);
        assert!(!s.charging);
        assert_eq!(s.raw_mv, 0);
        assert_eq!(s.held_raw_mv, 0);
    }

    // ── battery_poll_step ────────────────────────────────────────────────

    #[test]
    fn resting_basis_tracks_live_voltage_when_flat() {
        // Off external power, voltage unchanged: the live voltage IS the basis.
        let (settled, charging) = battery_poll_step(3700, 3700);
        assert_eq!(settled, 3700);
        assert!(!charging);
    }

    #[test]
    fn resting_basis_follows_slow_natural_discharge() {
        // A slowly falling voltage (normal discharge) must not be mistaken
        // for anything but a resting pack, and the basis must follow it down
        // rather than sticking at an old higher reading.
        let (settled, charging) = battery_poll_step(3700, 3680);
        assert_eq!(settled, 3680);
        assert!(!charging);
    }

    #[test]
    fn reading_at_threshold_is_still_battery_plausible() {
        // The boundary itself is inclusive of a real reading (not yet
        // "impossible"): must not be misread as on-power.
        let (settled, charging) = battery_poll_step(3624, EXTERNAL_POWER_MV_THRESHOLD);
        assert!(!charging);
        assert_eq!(settled, EXTERNAL_POWER_MV_THRESHOLD);
    }

    #[test]
    fn reading_one_mv_over_threshold_is_already_impossible() {
        // Pins down the exact `>` (not `>=`) boundary from the other side:
        // one mV over the ceiling must already flip to on-power, freezing the
        // basis rather than tracking the now-impossible live reading.
        let settled_before = EXTERNAL_POWER_MV_THRESHOLD;
        let (settled, charging) =
            battery_poll_step(settled_before, EXTERNAL_POWER_MV_THRESHOLD + 1);
        assert!(
            charging,
            "one mV over the ceiling must already be treated as external power present"
        );
        assert_eq!(
            settled, settled_before,
            "basis must freeze, not adopt the just-over-ceiling reading"
        );
    }

    #[test]
    fn reading_above_threshold_holds_last_basis_and_reports_power() {
        // This is the exact HIL data point: raw 4888 mV, far above the
        // physical single-cell ceiling — must hold the prior basis, not track
        // the impossible live voltage.
        let settled_before = 3_775; // 36% on RESTING_SOC_CURVE, the last valid unplugged reading
        let (settled, charging) = battery_poll_step(settled_before, 4888);
        assert!(
            charging,
            "an impossible-for-a-battery reading means external power is present"
        );
        assert_eq!(
            settled, settled_before,
            "percent basis must hold the last known good value, not the contaminated live voltage"
        );
    }

    #[test]
    fn engages_on_the_very_first_poll_with_no_prior_history() {
        // Unlike the superseded rise trigger (which needs a delta from a
        // prior sample and so misses a device that boots already on power),
        // the threshold check needs no history: even seeded with itself as
        // "prior settled", an above-threshold live reading is flagged.
        let (_settled, charging) = battery_poll_step(4888, 4888);
        assert!(
            charging,
            "must detect power-present on the very first sample, no prior baseline required"
        );
    }

    #[test]
    fn boot_already_on_power_is_flagged_charging_immediately() {
        // Mirrors exactly the call BatteryDriver::new makes with its initial
        // sample: a device that boots already attached to a charger must be
        // flagged `charging: true` from that very first sample, not leak the
        // contaminated first read through as a false 100% while reporting
        // "not charging".
        let initial_mv = 4888;
        let (settled, charging) = battery_poll_step(initial_mv, initial_mv);
        assert!(
            charging,
            "boot-on-power must be detected on the initial sample"
        );
        // `percent` still shows the contaminated first reading in this edge
        // case (no prior off-power sample exists to freeze at) — a documented
        // residual gap (see module docs' "Fix" section) — but `charging`
        // being correct immediately is the strict improvement over the
        // superseded rise trigger, which required a delta to ever see this.
        assert_eq!(settled, initial_mv);
    }

    #[test]
    fn holds_basis_indefinitely_while_above_threshold_no_plateau_confusion() {
        // The old rate-of-rise heuristic reported "not charging" once voltage
        // stopped climbing at the charger's CV plateau. The threshold check
        // has nothing to "stop rising" — repeated polls at the same
        // above-threshold voltage must keep reporting charging.
        let settled_before = 3_775; // 36%
        let mut settled = settled_before;
        let mut charging;
        for _ in 0..10 {
            (settled, charging) = battery_poll_step(settled, 4888);
            assert!(
                charging,
                "must not drop to 'not charging' while still above the ceiling"
            );
            assert_eq!(
                settled, settled_before,
                "basis must stay frozen the whole time on power"
            );
        }
    }

    #[test]
    fn drop_back_under_threshold_ends_power_and_resyncs_basis() {
        let settled_before = 3_775; // frozen pre-plug basis (36%)
        let (settled, charging) = battery_poll_step(settled_before, 3_900); // unplugged, pack settled a bit higher
        assert!(
            !charging,
            "a reading back under the ceiling means external power is gone"
        );
        assert_eq!(
            settled, 3_900,
            "basis must resync to the fresh post-unplug reading, not stay frozen at the stale pre-plug value"
        );
    }

    #[test]
    fn full_plug_unplug_cycle_never_reports_a_false_100_percent() {
        // End-to-end regression for the original HIL report: a known ~36%
        // pack is plugged in, raw voltage jumps to the observed
        // battery-impossible 4888 mV, holds there for a while, and is later
        // unplugged.
        let resting_mv = 3_775; // an exact 36% on RESTING_SOC_CURVE
        assert_eq!(percent_from_millivolts(resting_mv), 36);

        let mut settled = resting_mv;
        let mut charging;

        // Plug in: raw voltage jumps to the charge-rail reading.
        (settled, charging) = battery_poll_step(settled, 4888);
        assert!(charging);
        assert_eq!(
            percent_from_millivolts(settled),
            36,
            "must not read 100% the instant external power is detected"
        );

        // Hold on power for a while — must keep reporting charging AND must
        // never read 100%.
        for _ in 0..10 {
            (settled, charging) = battery_poll_step(settled, 4888);
            assert!(
                charging,
                "must not drop to 'not charging' while still on power"
            );
            assert_eq!(
                percent_from_millivolts(settled),
                36,
                "must stay at the true SoC while on power"
            );
        }

        // Unplug: raw voltage falls back to a battery-plausible reading.
        (settled, charging) = battery_poll_step(settled, 3_850);
        assert!(!charging, "must detect the unplug");
        let unplugged_percent = percent_from_millivolts(settled);
        assert_eq!(
            unplugged_percent, 55,
            "must resync to the fresh post-unplug reading's true SoC, got {unplugged_percent}%"
        );
    }
}
