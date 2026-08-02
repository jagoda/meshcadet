// SPDX-License-Identifier: GPL-3.0-only
//! The re-calibration hook `meshcadet-perf-device-report-ingest` drives:
//! replace one or more of [`LoopModelParams`]' cited SIMULATED sensitivity
//! ranges with a real device MEASURED point, once `docs/perf/collection-
//! kit.md` Part D's derivation table produces one.
//!
//! This module owns no parsing of the raw serial log — that lives in the
//! `perf_device_report` crate (a separate root-workspace member), which
//! depends on this crate and calls [`calibrate`] with whatever it
//! extracted from a `PERF phase=<name>` rollup line. Keeping the split this
//! way means this crate's provenance story (every number SIMULATED unless
//! explicitly overridden here, see `crate::lib`'s module doc) never has to
//! know about the report-back text format, and the parser never has to
//! know how a `LoopModelParams` field resolves.
//!
//! **Only four fields are reachable through this hook** — exactly the ones
//! `docs/perf/collection-kit.md` Part D's table marks "Directly" or
//! "Derived" from a Part C capture: `ui_step`, `cad_spi_overhead`,
//! `gps_poll`, `battery_poll`. Every other field (`frame_encode`,
//! `wdt_pet`, `tx_timestamp_rebase`, `room_keepalive_sched_check`,
//! `drain_ui_command`, `periodic_stats`, `split_ui_idle_tick`,
//! `queue_handoff`) has no corresponding field on [`MeasuredConstants`] at
//! all — there is
//! structurally no way to calibrate them through this hook until a future
//! instrumentation change gives them a phase to read, which is Part D's own
//! table's honest answer for each of them today.
//!
//! **The output of [`calibrate`] is still fed into a SIMULATION.** Giving
//! `ui_step` a MEASURED point instead of a SIMULATED range does not make
//! `perf_loop_model`'s downstream `longest_gap_ms`/`p95_gap_ms`/etc. a
//! device measurement — those numbers are still produced by
//! [`crate::sim::simulate`], a host discrete-event model, and must still be
//! labelled SIMULATED wherever they are quoted (`meshcadet-perf-rearchitecture`
//! plan §6 criterion 6). Only the INPUT constant this hook replaces may be
//! labelled MEASURED — [`CalibrationReport`] exists so a caller renders
//! that distinction field-by-field instead of guessing.

use crate::params::{LoopModelParams, ParamRangeMs};

/// One measured phase's mean/p95/max, in milliseconds — the shape Part D's
/// table reads directly off a `PERF phase=<name>` rollup line (after the
/// caller performs a µs -> ms conversion; this hook does no unit
/// conversion of its own so it never has to guess which unit a caller
/// passed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasuredPhaseMs {
    pub mean_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

/// The measured constants Part D's derivation table can produce from one
/// Part C capture window. Every field is `Option` because a real report may
/// only cover some of them (e.g. a capture that never straddled a GPS
/// active window has no `gps_poll` reading worth trusting — see that
/// field's own note in `docs/perf/collection-kit.md` Part D). `None` means
/// "leave this field at its documented SIMULATED range," never "assume
/// zero" — this hook has no fallback-to-zero path anywhere.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MeasuredConstants {
    /// `ui_step` phase, mean/p95/max, ms.
    pub ui_step: Option<MeasuredPhaseMs>,
    /// The `cad` phase's mean, ALREADY reduced to overhead
    /// (`cad_mean_ms - crate::sim::CAD_ACTIVE_MS`, floored at 0 by the
    /// caller) — a single derived point, not a mean/p95/max triple, because
    /// Part D's table derives exactly one number here, not a distribution.
    /// This hook performs no subtraction itself: it stays honest about not
    /// silently depending on `sim::CAD_ACTIVE_MS` on the caller's behalf.
    pub cad_spi_overhead_ms: Option<f64>,
    /// `gps` phase, mean/p95/max, ms.
    pub gps_poll: Option<MeasuredPhaseMs>,
    /// `battery` phase, mean/p95/max, ms.
    pub battery_poll: Option<MeasuredPhaseMs>,
}

/// One field's calibration outcome — the honesty ledger [`calibrate`]
/// returns alongside the calibrated params, so a caller can never present
/// an unreplaced field as measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldProvenance {
    /// This field now holds a device-measured point ([`ParamRangeMs::
    /// measured`] or an equal-bound point, per field).
    Measured,
    /// This field is still `LoopModelParams::documented_defaults()`'s
    /// cited SIMULATED sensitivity range — [`MeasuredConstants`] carried
    /// `None` for it.
    SimulatedRange,
}

/// Per-field provenance for the four fields this hook can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalibrationReport {
    pub ui_step: FieldProvenance,
    pub cad_spi_overhead: FieldProvenance,
    pub gps_poll: FieldProvenance,
    pub battery_poll: FieldProvenance,
}

impl CalibrationReport {
    /// True if every field this hook can reach was actually replaced —
    /// useful for a caller deciding whether a "calibrated baseline" claim
    /// is complete or partial.
    pub fn fully_calibrated(&self) -> bool {
        [
            self.ui_step,
            self.cad_spi_overhead,
            self.gps_poll,
            self.battery_poll,
        ]
        .iter()
        .all(|p| *p == FieldProvenance::Measured)
    }
}

/// Replace every field [`MeasuredConstants`] provides with a measured
/// point on top of `base`; every field it leaves `None` keeps `base`'s
/// existing range untouched (so a caller can chain [`calibrate`] calls
/// across multiple device reports, each filling in more fields, without
/// clobbering an earlier report's already-calibrated fields with a fresh
/// `documented_defaults()`).
pub fn calibrate(
    base: LoopModelParams,
    measured: &MeasuredConstants,
) -> (LoopModelParams, CalibrationReport) {
    let mut out = base;
    let mut report = CalibrationReport {
        ui_step: FieldProvenance::SimulatedRange,
        cad_spi_overhead: FieldProvenance::SimulatedRange,
        gps_poll: FieldProvenance::SimulatedRange,
        battery_poll: FieldProvenance::SimulatedRange,
    };

    if let Some(m) = measured.ui_step {
        out.ui_step = ParamRangeMs::measured(m.mean_ms, m.p95_ms, m.max_ms);
        report.ui_step = FieldProvenance::Measured;
    }
    if let Some(overhead_ms) = measured.cad_spi_overhead_ms {
        let v = overhead_ms.max(0.0);
        out.cad_spi_overhead = ParamRangeMs::new(v, v);
        report.cad_spi_overhead = FieldProvenance::Measured;
    }
    if let Some(m) = measured.gps_poll {
        out.gps_poll = ParamRangeMs::measured(m.mean_ms, m.p95_ms, m.max_ms);
        report.gps_poll = FieldProvenance::Measured;
    }
    if let Some(m) = measured.battery_poll {
        out.battery_poll = ParamRangeMs::measured(m.mean_ms, m.p95_ms, m.max_ms);
        report.battery_poll = FieldProvenance::Measured;
    }

    (out, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Corner;

    #[test]
    fn no_measurements_leaves_every_field_at_the_documented_range() {
        let base = LoopModelParams::documented_defaults();
        let (calibrated, report) = calibrate(base, &MeasuredConstants::default());

        assert_eq!(calibrated.ui_step, base.ui_step);
        assert_eq!(calibrated.cad_spi_overhead, base.cad_spi_overhead);
        assert_eq!(calibrated.gps_poll, base.gps_poll);
        assert_eq!(calibrated.battery_poll, base.battery_poll);
        assert_eq!(report.ui_step, FieldProvenance::SimulatedRange);
        assert_eq!(report.cad_spi_overhead, FieldProvenance::SimulatedRange);
        assert_eq!(report.gps_poll, FieldProvenance::SimulatedRange);
        assert_eq!(report.battery_poll, FieldProvenance::SimulatedRange);
        assert!(!report.fully_calibrated());

        // Fields this hook cannot reach at all are untouched too, by
        // construction (there is no field on `MeasuredConstants` for them).
        assert_eq!(calibrated.frame_encode, base.frame_encode);
        assert_eq!(calibrated.wdt_pet, base.wdt_pet);
    }

    #[test]
    fn ui_step_measurement_replaces_the_range_with_mean_p95_max() {
        let measured = MeasuredConstants {
            ui_step: Some(MeasuredPhaseMs {
                mean_ms: 0.180,
                p95_ms: 0.410,
                max_ms: 1.250,
            }),
            ..Default::default()
        };
        let (calibrated, report) = calibrate(LoopModelParams::documented_defaults(), &measured);

        assert_eq!(report.ui_step, FieldProvenance::Measured);
        assert_eq!(calibrated.ui_step.at(Corner::Low), 0.180);
        assert_eq!(calibrated.ui_step.at(Corner::Mid), 0.410);
        assert_eq!(calibrated.ui_step.at(Corner::High), 1.250);

        // The other three reachable fields are untouched.
        assert_eq!(report.cad_spi_overhead, FieldProvenance::SimulatedRange);
        assert_eq!(report.gps_poll, FieldProvenance::SimulatedRange);
        assert_eq!(report.battery_poll, FieldProvenance::SimulatedRange);
        assert!(!report.fully_calibrated());
    }

    #[test]
    fn cad_overhead_is_a_single_point_not_a_range() {
        let measured = MeasuredConstants {
            cad_spi_overhead_ms: Some(0.75),
            ..Default::default()
        };
        let (calibrated, report) = calibrate(LoopModelParams::documented_defaults(), &measured);

        assert_eq!(report.cad_spi_overhead, FieldProvenance::Measured);
        for corner in Corner::ALL {
            assert_eq!(calibrated.cad_spi_overhead.at(corner), 0.75);
        }
    }

    #[test]
    fn cad_overhead_negative_derivation_floors_at_zero() {
        // A capture where the CAD phase's mean happened to read below the
        // analytical CAD_ACTIVE_MS constant (measurement noise, or a very
        // fast bus) must never produce a negative SPI overhead.
        let measured = MeasuredConstants {
            cad_spi_overhead_ms: Some(-1.2),
            ..Default::default()
        };
        let (calibrated, _report) = calibrate(LoopModelParams::documented_defaults(), &measured);
        assert_eq!(calibrated.cad_spi_overhead.at(Corner::Low), 0.0);
        assert_eq!(calibrated.cad_spi_overhead.at(Corner::High), 0.0);
    }

    #[test]
    fn cad_overhead_never_pushes_the_cad_phase_past_the_real_deadline_once_resolved() {
        // Even a bogus/huge derived overhead is still capped by
        // `LoopModelParams::resolve`'s existing invariant (params.rs) —
        // calibration does not bypass it.
        let measured = MeasuredConstants {
            cad_spi_overhead_ms: Some(1_000.0),
            ..Default::default()
        };
        let (calibrated, _report) = calibrate(LoopModelParams::documented_defaults(), &measured);
        for corner in Corner::ALL {
            let r = calibrated.resolve(corner);
            let total = crate::sim::CAD_ACTIVE_MS + r.cad_spi_overhead;
            assert!(total <= crate::sim::CAD_HARD_DEADLINE_MS + 1e-9);
        }
    }

    #[test]
    fn all_four_reachable_fields_measured_reports_fully_calibrated() {
        let phase = MeasuredPhaseMs {
            mean_ms: 0.1,
            p95_ms: 0.2,
            max_ms: 0.3,
        };
        let measured = MeasuredConstants {
            ui_step: Some(phase),
            cad_spi_overhead_ms: Some(0.4),
            gps_poll: Some(phase),
            battery_poll: Some(phase),
        };
        let (_calibrated, report) = calibrate(LoopModelParams::documented_defaults(), &measured);
        assert!(report.fully_calibrated());
    }

    #[test]
    fn calibration_is_chainable_across_two_partial_reports() {
        // First report only closes ui_step ...
        let (after_first, _) = calibrate(
            LoopModelParams::documented_defaults(),
            &MeasuredConstants {
                ui_step: Some(MeasuredPhaseMs {
                    mean_ms: 0.2,
                    p95_ms: 0.3,
                    max_ms: 0.4,
                }),
                ..Default::default()
            },
        );
        // ... a second report closes battery_poll without a fresh
        // documented_defaults() call, so the first report's ui_step point
        // must survive.
        let (after_second, report) = calibrate(
            after_first,
            &MeasuredConstants {
                battery_poll: Some(MeasuredPhaseMs {
                    mean_ms: 0.05,
                    p95_ms: 0.08,
                    max_ms: 0.15,
                }),
                ..Default::default()
            },
        );
        assert_eq!(after_second.ui_step.at(Corner::Low), 0.2);
        assert_eq!(after_second.battery_poll.at(Corner::Low), 0.05);
        assert_eq!(report.battery_poll, FieldProvenance::Measured);
        // `report` only reflects THIS call's inputs, not history — the
        // caller composing multiple reports is responsible for merging
        // provenance across calls if it needs a cumulative picture.
        assert_eq!(report.ui_step, FieldProvenance::SimulatedRange);
    }
}
