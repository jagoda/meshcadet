// SPDX-License-Identifier: GPL-3.0-only
//! Assembles the full sensitivity sweep + dominance table into one text
//! report — printed by `src/bin/loop_model_report.rs` and captured verbatim
//! (labelled SIMULATED) into `docs/perf/perf-loop-model-baseline.md` (the M0
//! prediction) and, after `Topology::Split` was re-parameterised to the
//! as-built firmware, into `docs/perf/task-split-host-validation.md` (the M1
//! before/after) — the same "run the binary, paste the output" pattern
//! `ui_perf_bench` establishes for `docs/perf/ui-perf-baseline.md` §3, reused
//! milestone over milestone rather than re-derived per document.

use firmware_core::radio_wait::Dio1WaitKind;
use firmware_core::ui::idle_tick::ASLEEP_IDLE_TICK_MS;

use crate::params::{Corner, LoopModelParams};
use crate::sim::{
    dominance_check, simulate, simulate_split_ui_task, simulate_with_dio1_wait, DominanceVerdict,
    GapStats, SimResult, Topology,
};
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

/// One (corner, payload) row of the [`Dio1WaitKind::SpinPoll`] vs.
/// [`Dio1WaitKind::Notify`] comparison — `meshcadet-perf-radio-host-
/// validation`'s own headline table: quantifies "up to 1 ms of quantization
/// per DIO1 wait" (campaign plan, M2) directly against the SPLIT topology's
/// dispatcher/radio task, the metric plan §6 criterion 2 gates on ("the loop
/// model shows RX-poll cadence and CAD-attempt latency improved under UI
/// load").
pub struct Dio1WaitComparisonRow {
    pub corner: Corner,
    pub payload_bytes: usize,
    pub spin_poll: SimResult,
    pub notify: SimResult,
}

impl Dio1WaitComparisonRow {
    /// How much LONGER the spin-poll counterfactual's dispatcher-task
    /// iteration duration is than the shipped notify wait's, at this row's
    /// (corner, payload) point — always `>= 0`, per `apply_dio1_wait_
    /// quantization`'s own invariant (quantization can only add cost).
    pub fn dispatcher_longest_delta_ms(&self) -> f64 {
        self.spin_poll.dispatcher.longest_gap_ms - self.notify.dispatcher.longest_gap_ms
    }

    /// How much FASTER the shipped notify wait's dispatcher-task cadence is
    /// than the spin-poll counterfactual's, in Hz — the direct "CAD-attempt
    /// latency / RX-poll cadence improved" reading.
    pub fn dispatcher_service_hz_delta(&self) -> f64 {
        self.notify.dispatcher.service_hz - self.spin_poll.dispatcher.service_hz
    }
}

/// Run the [`Dio1WaitKind::SpinPoll`] (legacy, `tick_ms = 1`, the exact
/// removed-code behaviour) vs. [`Dio1WaitKind::Notify`] (shipped) comparison
/// across every (corner, payload) point in the headline sweep, against the
/// SPLIT topology (ADR-0012's as-built radio/dispatcher task — the topology
/// M2 actually runs on; `Topology::SingleLoop` is deliberately not swept
/// here since M2 landed after the M1 split, so "under UI load" now means
/// "concurrently with `ui_task` on the other core", not "in the same loop
/// as `ui.step()`").
///
/// **Deliberately isolated from `Workload::payload_sweep`'s GRP_TXT/room-
/// keepalive streams** (unlike every other table in this report) — those
/// have their own, payload-size-INDEPENDENT airtime and compete with the
/// swept inbound-DM/ACK frame for the same one-TX-per-iteration `TxQueue`
/// drain. Mixed in, a run's `longest_gap_ms` can end up dominated by
/// WHICHEVER stream happens to win a given (corner, payload) point's
/// scheduling — a real, separate effect, but not the DIO1-quantization
/// claim this table exists to isolate (same confound
/// `sim::tests::single_loop_gap_scales_monotonically_with_payload_size`
/// found and the same fix applied). Every other headline table in this
/// report answers "what is the overall gap distribution under realistic
/// mixed traffic" (already settled by earlier missions); this one answers
/// "what does the DIO1 wait choice, alone, change" — a different question,
/// deliberately isolated to answer it precisely.
pub fn dio1_wait_comparison_table() -> Vec<Dio1WaitComparisonRow> {
    dio1_wait_comparison_table_with_params(&LoopModelParams::documented_defaults())
}

/// Same table as [`dio1_wait_comparison_table`], against caller-supplied
/// `params`.
pub fn dio1_wait_comparison_table_with_params(
    params: &LoopModelParams,
) -> Vec<Dio1WaitComparisonRow> {
    let mut out = Vec::new();
    for corner in Corner::ALL {
        let r = params.resolve(corner);
        for payload_bytes in PAYLOAD_SWEEP_BYTES {
            let workload = Workload {
                inbound_dm: crate::workload::TrafficStream::every(5_000.0, payload_bytes),
                grp_txt: crate::workload::TrafficStream::disabled(),
                room_keepalive: crate::workload::TrafficStream::disabled(),
            };
            let spin_poll = simulate_with_dio1_wait(
                Topology::Split,
                &r,
                &workload,
                payload_bytes,
                SIM_DURATION_MS,
                Dio1WaitKind::SpinPoll { tick_ms: 1 },
            );
            let notify = simulate_with_dio1_wait(
                Topology::Split,
                &r,
                &workload,
                payload_bytes,
                SIM_DURATION_MS,
                Dio1WaitKind::Notify,
            );
            out.push(Dio1WaitComparisonRow {
                corner,
                payload_bytes,
                spin_poll,
                notify,
            });
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

/// One row of [`asleep_tick_comparison`]'s awake-vs-asleep-idle comparison.
pub struct AsleepTickRow {
    pub label: &'static str,
    pub split_ui_idle_tick_ms: f64,
    pub result: GapStats,
}

/// meshcadet-power-optimization Phase 5 (idle-screen enabler) — [SIM]-tagged
/// software-observable proxy for the adaptive asleep tick, per the plan of
/// record: re-runs `simulate_split_ui_task` (the SPLIT topology's own
/// `ui_task` service-gap model, `Corner::High`) with `split_ui_idle_tick`
/// widened from the documented AWAKE ceiling (`UI_TICK_MS` = 16ms) to the
/// Phase 5 asleep-and-idle ceiling
/// (`firmware_core::ui::idle_tick::ASLEEP_IDLE_TICK_MS`).
///
/// Still SIMULATED, never MEASURED (no device/HIL/QEMU path — crate root
/// doc) — a caller embedding this must keep the `[SIM]` tag, exactly like
/// every other number this crate produces.
pub fn asleep_tick_comparison(duration_ms: f64) -> Vec<AsleepTickRow> {
    let awake_r = LoopModelParams::documented_defaults().resolve(Corner::High);
    let awake = simulate_split_ui_task(&awake_r, duration_ms);

    let mut asleep_r = awake_r;
    asleep_r.split_ui_idle_tick = ASLEEP_IDLE_TICK_MS as f64;
    let asleep = simulate_split_ui_task(&asleep_r, duration_ms);

    vec![
        AsleepTickRow {
            label: "awake (UI_TICK_MS ceiling)",
            split_ui_idle_tick_ms: awake_r.split_ui_idle_tick,
            result: awake,
        },
        AsleepTickRow {
            label: "Phase 5 asleep-idle (ASLEEP_IDLE_TICK_MS ceiling)",
            split_ui_idle_tick_ms: asleep_r.split_ui_idle_tick,
            result: asleep,
        },
    ]
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
    writeln!(out).unwrap();

    writeln!(
        out,
        "-- DIO1 wait comparison (meshcadet-perf-radio-host-validation): legacy"
    )
    .unwrap();
    writeln!(
        out,
        "   1 ms-tick spin-poll (removed) vs. shipped notify wait, SPLIT dispatcher task --"
    )
    .unwrap();
    writeln!(
        out,
        "{:<8} {:>10} {:>16} {:>16} {:>16} {:>16} {:>16} {:>16}",
        "corner",
        "payload_B",
        "spinpoll_long_ms",
        "notify_long_ms",
        "delta_ms",
        "spinpoll_hz",
        "notify_hz",
        "hz_delta"
    )
    .unwrap();
    for row in dio1_wait_comparison_table_with_params(params) {
        writeln!(
            out,
            "{:<8} {:>10} {:>16.3} {:>16.3} {:>16.3} {:>16.2} {:>16.2} {:>16.2}",
            corner_label(row.corner),
            row.payload_bytes,
            row.spin_poll.dispatcher.longest_gap_ms,
            row.notify.dispatcher.longest_gap_ms,
            row.dispatcher_longest_delta_ms(),
            row.spin_poll.dispatcher.service_hz,
            row.notify.dispatcher.service_hz,
            row.dispatcher_service_hz_delta(),
        )
        .unwrap();
    }
    writeln!(out).unwrap();

    writeln!(
        out,
        "-- ui_task idle-tick comparison — meshcadet-power-optimization Phase 5 [SIM] --"
    )
    .unwrap();
    writeln!(
        out,
        "{:<48} {:>18} {:>12} {:>12} {:>12} {:>12}",
        "scenario", "idle_tick_ceil_ms", "longest_ms", "p95_ms", "mean_ms", "service_hz"
    )
    .unwrap();
    for row in asleep_tick_comparison(SIM_DURATION_MS) {
        writeln!(
            out,
            "{:<48} {:>18.1} {:>12.2} {:>12.2} {:>12.2} {:>12.2}",
            row.label,
            row.split_ui_idle_tick_ms,
            row.result.longest_gap_ms,
            row.result.p95_gap_ms,
            row.result.mean_gap_ms,
            row.result.service_hz,
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
    fn dio1_wait_comparison_table_has_every_combination() {
        let rows = dio1_wait_comparison_table();
        assert_eq!(rows.len(), 3 * PAYLOAD_SWEEP_BYTES.len());
    }

    #[test]
    fn dio1_wait_comparison_never_shows_spin_poll_faster_than_notify() {
        // The report's own headline claim for M2: removing the
        // quantization can only ever help (or be a wash), never hurt — see
        // `sim::apply_dio1_wait_quantization`'s own invariant.
        for row in dio1_wait_comparison_table() {
            assert!(
                row.dispatcher_longest_delta_ms() >= -1e-9,
                "corner {:?} payload {}: spin-poll's longest dispatcher gap ({}) should \
                 never be smaller than notify's ({})",
                row.corner,
                row.payload_bytes,
                row.spin_poll.dispatcher.longest_gap_ms,
                row.notify.dispatcher.longest_gap_ms,
            );
        }
    }

    #[test]
    fn render_text_report_mentions_the_dio1_wait_comparison() {
        let text = render_text_report();
        assert!(text.contains("DIO1 wait comparison"));
    }

    /// meshcadet-power-optimization Phase 5's own regression guard: the
    /// asleep-idle ceiling must genuinely slow `ui_task`'s own service rate
    /// relative to the awake ceiling — if `ASLEEP_IDLE_TICK_MS` were ever
    /// set at or below `UI_TICK_MS` by mistake, this fails loudly instead of
    /// silently reporting a no-op "improvement". Also the reproduction site
    /// for the `[SIM]`-tagged figure the plan of record asks Phase 5 to
    /// report: run `cargo test -p perf_loop_model asleep_idle_tick -- \
    /// --nocapture` to see the two printed lines.
    #[test]
    fn asleep_idle_tick_reduces_ui_task_service_rate_vs_awake() {
        let rows = asleep_tick_comparison(SIM_DURATION_MS);
        assert_eq!(rows.len(), 2);
        let awake = &rows[0];
        let asleep = &rows[1];

        for row in [awake, asleep] {
            println!(
                "[SIM] ui_task idle-tick comparison — {}: idle_tick_ceiling={:.1}ms \
                 longest={:.2}ms p95={:.2}ms mean={:.2}ms service_hz={:.2}",
                row.label,
                row.split_ui_idle_tick_ms,
                row.result.longest_gap_ms,
                row.result.p95_gap_ms,
                row.result.mean_gap_ms,
                row.result.service_hz,
            );
        }

        assert!(
            asleep.result.mean_gap_ms > awake.result.mean_gap_ms,
            "asleep-idle mean gap ({:.2}ms) should exceed the awake mean gap ({:.2}ms)",
            asleep.result.mean_gap_ms,
            awake.result.mean_gap_ms,
        );
        assert!(
            asleep.result.service_hz < awake.result.service_hz,
            "Phase 5's whole point is fewer ui_task iterations/sec while asleep and idle: \
             asleep {:.2}Hz should be less than awake {:.2}Hz",
            asleep.result.service_hz,
            awake.result.service_hz,
        );
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
    fn render_text_report_mentions_the_asleep_idle_tick_comparison() {
        let text = render_text_report();
        assert!(text.contains("ui_task idle-tick comparison"));
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
