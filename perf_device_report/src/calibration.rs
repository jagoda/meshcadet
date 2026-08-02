// SPDX-License-Identifier: GPL-3.0-only
//! Bridges a parsed device log ([`crate::perf_log::ParsedLog`]) to
//! `perf_loop_model`'s re-calibration hook
//! ([`perf_loop_model::calibration::calibrate`]), per `docs/perf/
//! collection-kit.md` Part D's derivation table. This module performs
//! exactly the derivations that table specifies — a µs -> ms conversion
//! for the three directly-read fields, and the `cad` phase's mean minus
//! `perf_loop_model::sim::CAD_ACTIVE_MS` (floored at 0) for the fourth —
//! and nothing else; every field Part D marks "not directly instrumented"
//! has no path through this module at all, matching `perf_loop_model::
//! calibration::MeasuredConstants`'s own shape.

use perf_loop_model::calibration::{MeasuredConstants, MeasuredPhaseMs};
use perf_loop_model::sim::CAD_ACTIVE_MS;

use crate::perf_log::ParsedLog;

const US_PER_MS: f64 = 1000.0;

fn phase_ms(log: &ParsedLog, phase: &str) -> Option<MeasuredPhaseMs> {
    let r = log.latest_phase_window(phase)?;
    Some(MeasuredPhaseMs {
        mean_ms: r.mean / US_PER_MS,
        p95_ms: r.p95 / US_PER_MS,
        max_ms: r.max / US_PER_MS,
    })
}

/// Build [`MeasuredConstants`] from a Part C/D capture, per the exact
/// derivations `docs/perf/collection-kit.md` Part D's table specifies.
/// Fields with no usable window (no report, or every window read `n=0`)
/// come back `None` — this function never substitutes a guessed or
/// zero-filled value for a field it couldn't derive.
pub fn measured_constants_from_log(log: &ParsedLog) -> MeasuredConstants {
    let cad_spi_overhead_ms = log
        .latest_phase_window("cad")
        .map(|r| (r.mean / US_PER_MS - CAD_ACTIVE_MS).max(0.0));

    MeasuredConstants {
        ui_step: phase_ms(log, "ui_step"),
        cad_spi_overhead_ms,
        gps_poll: phase_ms(log, "gps"),
        battery_poll: phase_ms(log, "battery"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perf_log::parse;

    const CALIBRATION_LOG: &str = "\
PERF phase=gps: n=2 min=1500 mean=1800 max=2100 p95=2050
PERF phase=battery: n=1 min=650 mean=650 max=650 p95=650
PERF phase=cad: n=5 min=8500 mean=9200 max=10100 p95=9900
PERF phase=ui_step: n=50 min=20 mean=180 max=900 p95=520
";

    #[test]
    fn derives_all_four_reachable_fields_with_a_us_to_ms_conversion() {
        let parsed = parse(CALIBRATION_LOG);
        let measured = measured_constants_from_log(&parsed);

        let ui_step = measured.ui_step.expect("ui_step should be derivable");
        assert_eq!(ui_step.mean_ms, 0.180);
        assert_eq!(ui_step.p95_ms, 0.520);
        assert_eq!(ui_step.max_ms, 0.900);

        let gps = measured.gps_poll.expect("gps_poll should be derivable");
        assert_eq!(gps.mean_ms, 1.8);

        let battery = measured
            .battery_poll
            .expect("battery_poll should be derivable");
        assert_eq!(battery.mean_ms, 0.650);

        // cad mean 9.2 ms minus CAD_ACTIVE_MS (8.192 ms) = ~1.008 ms.
        let cad_overhead = measured
            .cad_spi_overhead_ms
            .expect("cad overhead should be derivable");
        assert!((cad_overhead - (9.2 - CAD_ACTIVE_MS)).abs() < 1e-9);
    }

    #[test]
    fn cad_overhead_floors_at_zero_when_measured_mean_is_below_cad_active_ms() {
        let log = "PERF phase=cad: n=3 min=8000 mean=8100 max=8200 p95=8150\n";
        let parsed = parse(log);
        let measured = measured_constants_from_log(&parsed);
        // 8.1 ms mean < CAD_ACTIVE_MS (8.192 ms) -> floored, not negative.
        assert_eq!(measured.cad_spi_overhead_ms, Some(0.0));
    }

    #[test]
    fn a_phase_with_no_window_at_all_is_none_not_zero() {
        let parsed = parse("PERF phase=ui_step: n=1 min=10 mean=10 max=10 p95=10\n");
        let measured = measured_constants_from_log(&parsed);
        assert_eq!(measured.gps_poll, None);
        assert_eq!(measured.battery_poll, None);
        assert_eq!(measured.cad_spi_overhead_ms, None);
    }

    #[test]
    fn a_phase_whose_only_window_is_all_zero_samples_is_none_not_zero() {
        let parsed = parse("PERF phase=gps: n=0 min=0 mean=0 max=0 p95=0\n");
        let measured = measured_constants_from_log(&parsed);
        // n=0 is "no samples" (collection-kit.md Part C), not a real
        // zero-cost reading -- must not be fed to the model as one.
        assert_eq!(measured.gps_poll, None);
    }

    #[test]
    fn feeds_cleanly_into_perf_loop_models_calibrate_hook() {
        let parsed = parse(CALIBRATION_LOG);
        let measured = measured_constants_from_log(&parsed);
        let (calibrated, report) = perf_loop_model::calibrate(
            perf_loop_model::LoopModelParams::documented_defaults(),
            &measured,
        );
        assert!(report.fully_calibrated());
        // Sanity: the calibrated ui_step corners are the log's own
        // mean/p95/max, not the documented [0.05, 5.0] range.
        assert_eq!(calibrated.ui_step.at(perf_loop_model::Corner::Low), 0.180);
    }
}
