// SPDX-License-Identifier: GPL-3.0-only
//! Assembles the full sensitivity sweep + dominance table into one text
//! report — printed by `src/bin/loop_model_report.rs` and captured verbatim
//! (labelled SIMULATED) into `docs/perf/perf-loop-model-baseline.md` (the M0
//! prediction) and, after `Topology::Split` was re-parameterised to the
//! as-built firmware, into `docs/perf/task-split-host-validation.md` (the M1
//! before/after) — the same "run the binary, paste the output" pattern
//! `ui_perf_bench` establishes for `docs/perf/ui-perf-baseline.md` §3, reused
//! milestone over milestone rather than re-derived per document.

use crate::params::{Corner, LoopModelParams};
use crate::sim::{dominance_check, simulate, DominanceVerdict, SimResult, Topology};
use crate::workload::Workload;

/// The headline payload-size sweep: "10 B ACK-shaped through 255 B".
pub const PAYLOAD_SWEEP_BYTES: [usize; 4] = [10, 40, 100, 255];

/// Simulated duration for the headline sweep — 3 minutes of virtual time,
/// long enough to cover the payload-sweep scenario's fastest traffic stream
/// (inbound DM every 5 s) many times over. The room keep-alive's real
/// 300 000 ms cadence never fires within a single run at this duration,
/// which is intentional: the headline sweep's dominant traffic is the
/// inbound-DM/ACK stream the payload axis varies, not the keep-alive
/// background traffic — the dominance table below still exercises the
/// keep-alive/GRP_TXT scheduling paths via `Workload::payload_sweep`'s
/// non-idle streams, just not necessarily to a keep-alive firing within
/// this particular window.
pub const SIM_DURATION_MS: f64 = 180_000.0;

/// One (topology, corner, payload) simulation row.
pub struct SweepRow {
    pub corner: Corner,
    pub result: SimResult,
}

/// Every combination — 3 corners x 4 payload sizes x 2 topologies = 24 runs,
/// against `LoopModelParams::documented_defaults()`. See
/// [`full_sweep_with_params`] to re-run against a calibrated
/// [`LoopModelParams`] (`crate::calibration::calibrate`'s output) instead.
pub fn full_sweep() -> Vec<SweepRow> {
    full_sweep_with_params(&LoopModelParams::documented_defaults())
}

/// Same sweep as [`full_sweep`], against caller-supplied `params` — the
/// hook a device-report re-calibration re-run uses, per `crate`'s "The
/// re-calibration hook" doc section.
pub fn full_sweep_with_params(params: &LoopModelParams) -> Vec<SweepRow> {
    let mut out = Vec::new();
    for corner in Corner::ALL {
        let r = params.resolve(corner);
        for payload_bytes in PAYLOAD_SWEEP_BYTES {
            let workload = Workload::payload_sweep(payload_bytes);
            for topology in [Topology::SingleLoop, Topology::Split] {
                out.push(SweepRow {
                    corner,
                    result: simulate(topology, &r, &workload, payload_bytes, SIM_DURATION_MS),
                });
            }
        }
    }
    out
}

/// This pass's own abort/reroute question: does radio-TX blocking dominate
/// the UI-unserviced gap? Evaluated at every corner x payload-size
/// combination, against `LoopModelParams::documented_defaults()`. See
/// [`dominance_table_with_params`] for the calibrated-re-run hook.
pub fn dominance_table() -> Vec<DominanceVerdict> {
    dominance_table_with_params(&LoopModelParams::documented_defaults())
}

/// Same table as [`dominance_table`], against caller-supplied `params`.
pub fn dominance_table_with_params(params: &LoopModelParams) -> Vec<DominanceVerdict> {
    let mut out = Vec::new();
    for corner in Corner::ALL {
        for payload_bytes in PAYLOAD_SWEEP_BYTES {
            out.push(dominance_check(params, payload_bytes, corner));
        }
    }
    out
}

fn corner_label(c: Corner) -> &'static str {
    match c {
        Corner::Low => "low",
        Corner::Mid => "mid",
        Corner::High => "high",
    }
}

fn topology_label(t: Topology) -> &'static str {
    match t {
        Topology::SingleLoop => "single-loop (current)",
        Topology::Split => "split (as-built M1)",
    }
}

/// Render the full report as plain text — the exact bytes that get pasted
/// into `docs/perf/perf-loop-model-baseline.md`, labelled SIMULATED.
pub fn render_text_report() -> String {
    render_text_report_with_params(&LoopModelParams::documented_defaults())
}

/// Same report as [`render_text_report`], against caller-supplied `params`.
///
/// **Every number this prints is still SIMULATED**, even when `params`
/// carries one or more device-MEASURED points from `crate::calibration::
/// calibrate` — `longest_gap_ms`/`p95_gap_ms`/etc. are always
/// `crate::sim::simulate`'s output, a host model, never a device reading.
/// A caller embedding this text into a doc must still tag the OUTPUT rows
/// SIMULATED and separately cite which INPUT constants were MEASURED (e.g.
/// via `crate::calibration::CalibrationReport`) — this function does not
/// do that labelling for the caller, by design, so it can never silently
/// launder a calibrated input into a "measured" output claim.
pub fn render_text_report_with_params(params: &LoopModelParams) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    // Milestone-neutral header: this same report shape is now reused by
    // both the M0 baseline (`docs/perf/perf-loop-model-baseline.md`) and the
    // M1 as-built re-run (`docs/perf/task-split-host-validation.md`), and
    // will be again for M2 — the milestone label belongs to the caller
    // embedding this text, not hardcoded here.
    writeln!(out, "=== perf_loop_model — SIMULATED report ===").unwrap();
    writeln!(
        out,
        "no device, no HIL, no QEMU — host discrete-event model over real firmware-core state machines"
    )
    .unwrap();
    writeln!(out).unwrap();

    writeln!(
        out,
        "-- dominance check (abort/reroute condition): does a single radio-TX"
    )
    .unwrap();
    writeln!(
        out,
        "   block, alone, exceed the WORST UI-unserviced gap achievable with ZERO radio"
    )
    .unwrap();
    writeln!(out, "   traffic at all? --").unwrap();
    writeln!(
        out,
        "{:<8} {:>10} {:>14} {:>20} {:>10} {:>10}",
        "corner", "payload_B", "airtime_ms", "idle_floor_gap_ms", "ratio_x", "dominates"
    )
    .unwrap();
    for v in dominance_table_with_params(params) {
        writeln!(
            out,
            "{:<8} {:>10} {:>14} {:>20.3} {:>10.1} {:>10}",
            corner_label(v.corner),
            v.payload_bytes,
            v.airtime_ms,
            v.idle_floor_longest_gap_ms,
            v.ratio,
            v.dominates,
        )
        .unwrap();
    }
    writeln!(out).unwrap();

    writeln!(out, "-- UI-unserviced-gap sweep (headline metric) --").unwrap();
    writeln!(
        out,
        "{:<24} {:<8} {:>10} {:>14} {:>12} {:>12} {:>16} {:>12}",
        "topology",
        "corner",
        "payload_B",
        "longest_ms",
        "p95_ms",
        "mean_ms",
        "cumul_unsvc_ms",
        "service_hz"
    )
    .unwrap();
    // Computed once and reused for both tables below — each `SweepRow` runs
    // a full discrete-event simulation, so recomputing it a second time for
    // the cadence table would silently double this function's total work
    // for no reason.
    let sweep = full_sweep_with_params(params);

    for row in &sweep {
        let r = &row.result;
        writeln!(
            out,
            "{:<24} {:<8} {:>10} {:>14.2} {:>12.2} {:>12.2} {:>16.1} {:>12.2}",
            topology_label(r.topology),
            corner_label(row.corner),
            r.payload_bytes,
            r.ui.longest_gap_ms,
            r.ui.p95_gap_ms,
            r.ui.mean_gap_ms,
            r.ui.cumulative_unserviced_ms,
            r.ui.service_hz,
        )
        .unwrap();
    }
    writeln!(out).unwrap();

    writeln!(
        out,
        "-- radio/dispatcher-task cadence (same loop as UI for single-loop; the"
    )
    .unwrap();
    writeln!(
        out,
        "   decoupled radio/dispatcher task under the split topology) --"
    )
    .unwrap();
    writeln!(
        out,
        "{:<24} {:<8} {:>10} {:>14} {:>12} {:>12}",
        "topology", "corner", "payload_B", "longest_ms", "p95_ms", "iter_hz"
    )
    .unwrap();
    for row in &sweep {
        let r = &row.result;
        writeln!(
            out,
            "{:<24} {:<8} {:>10} {:>14.2} {:>12.2} {:>12.2}",
            topology_label(r.topology),
            corner_label(row.corner),
            r.payload_bytes,
            r.dispatcher.longest_gap_ms,
            r.dispatcher.p95_gap_ms,
            r.dispatcher.service_hz,
        )
        .unwrap();
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_sweep_has_every_combination() {
        let rows = full_sweep();
        assert_eq!(rows.len(), 3 * PAYLOAD_SWEEP_BYTES.len() * 2);
    }

    #[test]
    fn dominance_table_has_every_combination() {
        let rows = dominance_table();
        assert_eq!(rows.len(), 3 * PAYLOAD_SWEEP_BYTES.len());
    }

    #[test]
    fn every_dominance_row_dominates() {
        // The report's own headline claim: across the FULL sensitivity
        // range (all 3 corners) and every payload size (including the
        // smallest, hardest-to-dominate 10 B ACK), radio-TX blocking
        // dominates. If this ever fails, M0's checkpoint verdict changes
        // and this report's own text must say so plainly, not silently.
        for v in dominance_table() {
            assert!(
                v.dominates,
                "corner={:?} payload={} should dominate (airtime={} ms, floor={} ms)",
                v.corner, v.payload_bytes, v.airtime_ms, v.idle_floor_longest_gap_ms,
            );
        }
    }

    #[test]
    fn render_text_report_is_non_empty_and_mentions_simulated() {
        let text = render_text_report();
        assert!(text.contains("SIMULATED"));
        assert!(text.contains("dominates"));
    }

    #[test]
    fn with_params_entry_points_match_the_default_entry_points_at_documented_defaults() {
        // The re-calibration hook's `_with_params` variants must be
        // strictly more general than the no-arg ones, not a parallel
        // reimplementation that can drift from them.
        let defaults = LoopModelParams::documented_defaults();
        assert_eq!(full_sweep().len(), full_sweep_with_params(&defaults).len());
        assert_eq!(
            dominance_table().len(),
            dominance_table_with_params(&defaults).len()
        );
        assert_eq!(
            render_text_report(),
            render_text_report_with_params(&defaults)
        );
    }

    #[test]
    fn calibrated_params_change_the_ui_unserviced_gap_sweep() {
        // A calibrated ui_step far above the documented range's high bound
        // should visibly move the headline sweep's numbers — otherwise the
        // hook would be wired to nothing.
        let (calibrated, _report) = crate::calibration::calibrate(
            LoopModelParams::documented_defaults(),
            &crate::calibration::MeasuredConstants {
                ui_step: Some(crate::calibration::MeasuredPhaseMs {
                    mean_ms: 4.0,
                    p95_ms: 8.0,
                    max_ms: 12.0,
                }),
                ..Default::default()
            },
        );
        let baseline_text = render_text_report();
        let calibrated_text = render_text_report_with_params(&calibrated);
        assert_ne!(baseline_text, calibrated_text);
    }
}
