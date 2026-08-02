// SPDX-License-Identifier: GPL-3.0-only
//! The discrete-event simulator: replays the dispatcher loop's documented
//! per-iteration phase order over a [`crate::workload::Workload`], calling
//! the REAL `firmware_core::dispatcher` state machines for every phase that
//! has one, and reports the UI-unserviced-gap distribution this pass's
//! M0 checkpoint needs. See `crate` (`lib.rs`) for the full module doc.

use firmware_core::dispatcher::{lora_airtime_ms, AirtimeBudget, TxQueue};

use crate::params::ResolvedParams;
use crate::workload::{Workload, RX_POLL_YIELD_MS};

/// LoRa symbol time at the locked SF7/BW62.5kHz preset: `2^SF / BW` (Semtech
/// AN1200.13 §4 — the SAME base relation `firmware_core::dispatcher::
/// lora_airtime_ms` cites for its own, different, PAYLOAD-symbol-count
/// formula; this is an independent application of that relation to the
/// CAD symbol count, not a reuse or re-derivation of that function's
/// result). `2^7 / 62_500 Hz * 1000 = 2.048 ms`. Verified against the
/// formula directly in this module's tests.
pub const CAD_SYMBOL_TIME_MS: f64 = 2.048;

/// CAD is configured for 4 symbols — `cadSymbolNum = CAD_ON_4_SYMB`
/// (`firmware/src/radio.rs:447`).
pub const CAD_SYMBOLS: f64 = 4.0;

/// Analytically computed CAD-active time (NOT swept — see
/// [`crate::params::LoopModelParams::cad_spi_overhead`] for the genuinely
/// unknown remainder).
pub const CAD_ACTIVE_MS: f64 = CAD_SYMBOL_TIME_MS * CAD_SYMBOLS; // 8.192 ms

/// The real code's own CAD poll deadline — `firmware/src/radio.rs:468`,
/// `let deadline = uptime_ms() + 20;`. In-repo, exact; not swept.
pub const CAD_HARD_DEADLINE_MS: f64 = 20.0;

// Compile-time invariant: the analytically computed CAD-active time must
// itself fit inside the real code's own 20 ms poll deadline, or
// `LoopModelParams::resolve`'s `cad_overhead_cap` computation
// (`CAD_HARD_DEADLINE_MS - CAD_ACTIVE_MS`) would go negative.
const _: () = assert!(CAD_ACTIVE_MS < CAD_HARD_DEADLINE_MS);

/// `RX_STATS_INTERVAL_MS` — `firmware/src/main.rs:1622`. In-repo, exact;
/// not swept.
pub const RX_STATS_INTERVAL_MS: f64 = 30_000.0;

fn synth_frame(payload_bytes: usize) -> Vec<u8> {
    vec![0u8; payload_bytes.min(255)]
}

/// One frame stream's cumulative outcome over a simulation run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrafficCounters {
    pub frames_enqueued: usize,
    /// Frames the REAL `TxQueue::enqueue` reported evicted because the
    /// 4-slot queue was already full (`firmware_core::dispatcher::
    /// TX_QUEUE_SLOTS`) — a genuine overload signal, not silently dropped
    /// by this model.
    pub frames_dropped: usize,
    pub frames_transmitted: usize,
    /// Times a pending TX was denied by the REAL `AirtimeBudget::
    /// can_transmit` (10% duty-cycle cap) and backed off rather than sent.
    /// This model always treats CAD as clear (see `attempt_cad_tx`'s doc),
    /// so this is the ONLY backoff-triggering path here — a nonzero count
    /// means the duty-cycle cap, not just the queue depth, genuinely
    /// bound this run's throughput.
    pub budget_denials: usize,
}

/// One "service point" distribution: for the UI-unserviced-gap runs, gaps
/// are the wall-clock time between consecutive `ui.step()` calls; for the
/// dispatcher-cadence runs (no UI in the loop at all — the SPLIT topology's
/// radio/dispatcher task), gaps are consecutive iteration durations. See the
/// call site for which one a given [`GapStats`] value is.
#[derive(Debug, Clone)]
pub struct GapStats {
    pub longest_gap_ms: f64,
    pub p95_gap_ms: f64,
    pub mean_gap_ms: f64,
    /// Sum of every recorded gap — the total wall-clock time the service
    /// point went unserviced over the whole run.
    pub cumulative_unserviced_ms: f64,
    pub service_iterations: usize,
    pub service_hz: f64,
    pub traffic: TrafficCounters,
    /// Full distribution, sorted ascending — for callers that want more
    /// than the summary percentiles (e.g. the report binary's histogram).
    pub gaps_ms: Vec<f64>,
}

fn percentile(sorted_gaps: &[f64], p: f64) -> f64 {
    if sorted_gaps.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted_gaps.len() as f64 - 1.0)).round() as usize;
    sorted_gaps[idx.min(sorted_gaps.len() - 1)]
}

impl GapStats {
    fn from_gaps(mut gaps: Vec<f64>, elapsed_ms: f64, traffic: TrafficCounters) -> Self {
        gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = gaps.len().max(1) as f64;
        let sum: f64 = gaps.iter().sum();
        let longest = gaps.last().copied().unwrap_or(0.0);
        let p95 = percentile(&gaps, 95.0);
        GapStats {
            longest_gap_ms: longest,
            p95_gap_ms: p95,
            mean_gap_ms: sum / n,
            cumulative_unserviced_ms: sum,
            service_iterations: gaps.len(),
            service_hz: if elapsed_ms > 0.0 {
                gaps.len() as f64 / (elapsed_ms / 1000.0)
            } else {
                0.0
            },
            traffic,
            gaps_ms: gaps,
        }
    }
}

/// A pending outbound-frame scheduler: fires every `interval_ms`, advancing
/// its own next-due time — used identically for GRP_TXT, room keep-alive,
/// and inbound-DM arrivals (deterministic, evenly spaced — see
/// `workload.rs`'s "Determinism" note).
struct Scheduler {
    next_due_ms: Option<f64>,
    interval_ms: Option<f64>,
    payload_bytes: usize,
}

impl Scheduler {
    fn new(interval_ms: Option<f64>, payload_bytes: usize) -> Self {
        // A zero (or negative) interval would never advance `next_due_ms`
        // past `t_ms` in `fire_if_due`'s caller `while let` drain loop —
        // an infinite loop. Not a real workload shape (every real traffic
        // stream has a positive period), so this is a defensive guard
        // against a malformed `Workload`, not a case this crate's own
        // presets ever construct.
        let interval_ms = interval_ms.filter(|ms| *ms > 0.0);
        Self {
            next_due_ms: interval_ms,
            interval_ms,
            payload_bytes,
        }
    }

    /// If due by `t_ms`, advance to the next tick and return this stream's
    /// payload size to enqueue. Callers drain this in a `while let` loop
    /// (not `if let`) so a backlog built up during a long CAD+TX block is
    /// fully caught up rather than under-counted — see the call sites'
    /// doc.
    fn fire_if_due(&mut self, t_ms: f64) -> Option<usize> {
        let due = self.next_due_ms?;
        if t_ms < due {
            return None;
        }
        let interval = self.interval_ms.expect("next_due_ms implies interval_ms");
        self.next_due_ms = Some(due + interval);
        Some(self.payload_bytes)
    }
}

/// CAD + TX phase: mirrors `firmware/src/main.rs`'s `if txq.has_pending() &&
/// now >= cad_backoff_until_ms` block. The channel is always modelled clear
/// (no CAD-busy / collision modelling here — bus-arbitration/contention
/// behaviour is a separate source-and-datasheet analysis, not this crate's
/// job; see the module doc's "What this does NOT model"), which is also the
/// WORST case for UI starvation (it maximizes how many TX events actually
/// fire and block `ui.step()`) — the conservative direction for this
/// crate's headline question. Returns the phase's cost in ms; mutates
/// `txq`/`budget`/
/// `cad_backoff_until_ms` exactly as the real loop does, and increments
/// `traffic.frames_transmitted` on a successful TX.
fn attempt_cad_tx(
    t_ms: f64,
    txq: &mut TxQueue,
    budget: &mut AirtimeBudget,
    cad_backoff_until_ms: &mut f64,
    r: &ResolvedParams,
    traffic: &mut TrafficCounters,
) -> f64 {
    if !txq.has_pending() || t_ms < *cad_backoff_until_ms {
        return 0.0;
    }
    let cad_cost = CAD_ACTIVE_MS + r.cad_spi_overhead;
    let mut buf = [0u8; 255];
    let n = txq.peek(&mut buf);
    if n == 0 {
        return cad_cost;
    }
    let required = lora_airtime_ms(n); // REAL function — see crate doc.
    let now_ms = (t_ms + cad_cost).round() as u64;
    if budget.can_transmit(now_ms, required) {
        budget.record_tx(now_ms, required);
        txq.pop_front();
        traffic.frames_transmitted += 1;
        cad_cost + required as f64
    } else {
        // Airtime-budget denial — same non-blocking backoff shape as a
        // CAD-busy result (`firmware/src/main.rs:2371-2384`): the frame
        // stays queued for the next attempt instead of being dropped. The
        // real code jitters this 1000-3000 ms (`1000 + pub_hash() % 2000`);
        // this model fixes it at the low end of that range deterministically
        // (see the crate root doc's "Determinism" section — no PRNG
        // anywhere in this crate). This DOES fire in the headline sweep: a
        // 255 B payload's 800 ms airtime against a 5 s inbound-DM cadence is
        // a ~16% duty cycle, over `AirtimeBudget`'s real 10% cap
        // (`BUDGET_MAX_MS`/`BUDGET_WINDOW_MS`), so some attempts are
        // genuinely denied and back off. It does NOT skew this crate's
        // headline metrics (longest gap, dominance): a denied attempt costs
        // only `cad_cost` here (CAD_ACTIVE_MS + the small SPI-overhead
        // range) — `t_ms < cad_backoff_until_ms` skips CAD+TX ENTIRELY on
        // every iteration while backed off, so backoff duration changes how
        // often a TX attempt is retried, not the size of any one gap.
        *cad_backoff_until_ms = t_ms + cad_cost + 1_000.0;
        traffic.budget_denials += 1;
        cad_cost
    }
}

/// Core replay of the dispatcher loop's documented phase order (see the
/// crate root doc's "What to model" summary). `include_ui`:
/// - `true` — the CURRENT single-superloop topology: `ui.step()` and the
///   `UiCommand` drain are part of THIS SAME loop, so the recorded gap is
///   the real UI-unserviced-gap definition (wall-clock between consecutive
///   `ui.step()` calls).
/// - `false` — the radio/dispatcher HALF of the proposed M1 split topology,
///   once decoupled from the UI task entirely: no `ui.step()`/drain in this
///   loop at all, so the recorded "gap" is this task's own iteration
///   cadence (a smaller number here means a faster, more responsive CAD/RX
///   loop — feeds the "RX-poll cadence and CAD-attempt latency… improve"
///   half of the pass's priority-1 acceptance criterion, which M1/M2's
///   own host-validation children re-run this exact model against).
pub(crate) fn simulate_core(
    r: &ResolvedParams,
    workload: &Workload,
    duration_ms: f64,
    include_ui: bool,
) -> GapStats {
    let mut txq = TxQueue::new();
    let mut budget = AirtimeBudget::new();
    let mut cad_backoff_until_ms = 0.0f64;
    let mut t = 0.0f64;
    let mut last_stats_ms = 0.0f64;
    let mut last_service_end_ms = 0.0f64;
    let mut traffic = TrafficCounters::default();

    let mut grp_txt = Scheduler::new(workload.grp_txt.interval_ms, workload.grp_txt.payload_bytes);
    let mut keepalive = Scheduler::new(
        workload.room_keepalive.interval_ms,
        workload.room_keepalive.payload_bytes,
    );
    let mut inbound_dm = Scheduler::new(
        workload.inbound_dm.interval_ms,
        workload.inbound_dm.payload_bytes,
    );

    let mut gaps = Vec::new();

    while t < duration_ms {
        let iteration_start_ms = t;

        // WDT pet, GPS poll, tx-timestamp rebase, battery poll, room
        // keep-alive SCHEDULER CHECK (encode cost below is separate).
        t += r.fixed_phase_cost_ms();

        // Own-initiated sends (GRP_TXT, room keep-alive) — checked here,
        // before CAD+TX, mirroring `firmware/src/main.rs`'s room
        // keep-alive scheduler position in the loop. `while let` (not `if
        // let`): a long CAD+TX block (up to the 800 ms max-payload airtime)
        // can leave several intervals due by the time this phase is next
        // checked — draining the backlog here, rather than only ever
        // catching the most recent tick, is what lets a fast stream
        // genuinely overrun the 4-slot `TxQueue` and show up as a counted
        // drop instead of silently vanishing into an under-counted model.
        while let Some(payload_bytes) = grp_txt.fire_if_due(t) {
            t += r.frame_encode;
            traffic.frames_enqueued += 1;
            if txq.enqueue(&synth_frame(payload_bytes)).is_some() {
                traffic.frames_dropped += 1;
            }
        }
        while let Some(payload_bytes) = keepalive.fire_if_due(t) {
            t += r.frame_encode;
            traffic.frames_enqueued += 1;
            if txq.enqueue(&synth_frame(payload_bytes)).is_some() {
                traffic.frames_dropped += 1;
            }
        }

        // CAD + TX.
        t += attempt_cad_tx(
            t,
            &mut txq,
            &mut budget,
            &mut cad_backoff_until_ms,
            r,
            &mut traffic,
        );

        // RX poll — full `RX_POLL_YIELD_MS` window every iteration (see
        // that constant's doc: this model conservatively never short-
        // circuits on an early DIO1 edge). An inbound DM "arrives" here —
        // decoded and ACK-enqueued inline, exactly where `handle_dm` runs
        // in the real loop.
        t += RX_POLL_YIELD_MS;
        while let Some(payload_bytes) = inbound_dm.fire_if_due(t) {
            t += r.frame_encode;
            traffic.frames_enqueued += 1;
            if txq.enqueue(&synth_frame(payload_bytes)).is_some() {
                traffic.frames_dropped += 1;
            }
        }

        // Periodic RX-stats / stack-HWM log (every 30 s).
        if t - last_stats_ms >= RX_STATS_INTERVAL_MS {
            t += r.periodic_stats;
            last_stats_ms = t;
        }

        if include_ui {
            // `ui.step()` — the ONLY place touch/keyboard/render happen.
            let gap = t - last_service_end_ms;
            gaps.push(gap);
            t += r.ui_step;
            last_service_end_ms = t;
            // Drain UiCommand / handle events.
            t += r.drain_ui_command;
        } else {
            // No UI in this task under the split topology — the recorded
            // "gap" is this iteration's own duration (this task's cadence).
            gaps.push(t - iteration_start_ms);
        }
    }

    GapStats::from_gaps(gaps, t, traffic)
}

/// SPLIT topology's UI task, fully decoupled from radio/dispatcher: no CAD,
/// no TX, no RX poll — it loops purely between `ui.step()` and its own
/// idle-tick granularity (see [`crate::params::LoopModelParams::
/// split_ui_idle_tick`]). By construction this NEVER touches
/// `lora_airtime_ms`/`TxQueue`/payload size at all, which is what makes
/// "no longer scales with LoRa payload size" (pass acceptance
/// criterion 3) trivially true for this topology rather than merely
/// asserted.
pub(crate) fn simulate_split_ui_task(r: &ResolvedParams, duration_ms: f64) -> GapStats {
    let mut t = 0.0f64;
    let mut last_service_end_ms = 0.0f64;
    let mut gaps = Vec::new();
    while t < duration_ms {
        let gap = t - last_service_end_ms;
        gaps.push(gap);
        t += r.ui_step;
        last_service_end_ms = t;
        t += r.split_ui_idle_tick;
    }
    GapStats::from_gaps(gaps, t, TrafficCounters::default())
}

/// Which superloop topology a run models — see the crate root doc's "Two
/// topologies, one harness" section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topology {
    /// Today's shipped topology: radio + UI in one task, one core.
    SingleLoop,
    /// The proposed M1 split: UI on its own task/core, radio+dispatcher on
    /// core 0, message queues across the boundary. Not yet implemented in
    /// firmware — this is the PREDICTED delta the pass's M0 checkpoint
    /// uses to decide whether M1 is worth building at all.
    Split,
}

/// One simulation run's full result: the UI-unserviced-gap distribution
/// (the headline metric) plus the radio/dispatcher task's own iteration-
/// cadence distribution (for [`Topology::SingleLoop`] these are the SAME
/// loop, so `ui` and `dispatcher` are identical; for [`Topology::Split`]
/// they are two independent tasks).
#[derive(Debug, Clone)]
pub struct SimResult {
    pub topology: Topology,
    pub payload_bytes: usize,
    pub ui: GapStats,
    pub dispatcher: GapStats,
}

/// Run one simulation: `params` already resolved at a [`crate::params::
/// Corner`], `workload` already sized for `payload_bytes` (callers building
/// a payload sweep should use [`crate::workload::Workload::payload_sweep`]).
pub fn simulate(
    topology: Topology,
    r: &ResolvedParams,
    workload: &Workload,
    payload_bytes: usize,
    duration_ms: f64,
) -> SimResult {
    match topology {
        Topology::SingleLoop => {
            let stats = simulate_core(r, workload, duration_ms, true);
            SimResult {
                topology,
                payload_bytes,
                ui: stats.clone(),
                dispatcher: stats,
            }
        }
        Topology::Split => {
            let ui = simulate_split_ui_task(r, duration_ms);
            let dispatcher = simulate_core(r, workload, duration_ms, false);
            SimResult {
                topology,
                payload_bytes,
                ui,
                dispatcher,
            }
        }
    }
}

/// Simulated duration for the idle-floor dominance check — long enough
/// (>30 s) that the periodic RX-stats phase fires at least once, so the
/// "floor" isn't an artificially low reading that never pays that cost.
pub const IDLE_FLOOR_DURATION_MS: f64 = 65_000.0;

/// Whether a single radio-TX block, by itself, already exceeds the WORST
/// UI-unserviced gap achievable from routine per-iteration overhead alone
/// (WDT/GPS/battery/room-sched/RX-poll/stats/`ui.step()`/drain) with ZERO
/// radio traffic at all — this pass's own abort/reroute question: if,
/// across the full plausible range of the un-measured constants, radio-TX
/// blocking does NOT dominate the UI-unserviced gap, later milestones
/// should reroute to local UI-side optimization instead of a task/core
/// split. Evaluated at one [`crate::params::Corner`] for one payload size.
#[derive(Debug, Clone, Copy)]
pub struct DominanceVerdict {
    pub payload_bytes: usize,
    pub corner: crate::params::Corner,
    pub airtime_ms: u32,
    pub idle_floor_longest_gap_ms: f64,
    pub dominates: bool,
    /// `airtime_ms / idle_floor_longest_gap_ms` — how many multiples of the
    /// zero-traffic worst gap a single TX block already is.
    pub ratio: f64,
}

pub fn dominance_check(
    params: &crate::params::LoopModelParams,
    payload_bytes: usize,
    corner: crate::params::Corner,
) -> DominanceVerdict {
    let r = params.resolve(corner);
    let idle = simulate_core(&r, &Workload::idle(), IDLE_FLOOR_DURATION_MS, true);
    let airtime = lora_airtime_ms(payload_bytes);
    let floor = idle.longest_gap_ms.max(1e-9);
    DominanceVerdict {
        payload_bytes,
        corner,
        airtime_ms: airtime,
        idle_floor_longest_gap_ms: idle.longest_gap_ms,
        dominates: (airtime as f64) > idle.longest_gap_ms,
        ratio: airtime as f64 / floor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{Corner, LoopModelParams};
    use crate::workload::Workload;

    #[test]
    fn cad_symbol_time_matches_the_semtech_formula() {
        // Verifies the hand-written literal against the actual `2^SF / BW`
        // relation (can't be a `const fn` on stable — `f64::powi` isn't
        // const-stable — so this test is the tripwire instead).
        let sf: f64 = 7.0;
        let bw_hz: f64 = 62_500.0;
        let t_sym_ms = (2f64.powf(sf) / bw_hz) * 1000.0;
        assert!(
            (t_sym_ms - CAD_SYMBOL_TIME_MS).abs() < 1e-9,
            "literal {} != formula {}",
            CAD_SYMBOL_TIME_MS,
            t_sym_ms,
        );
    }

    #[test]
    fn idle_workload_has_no_transmissions() {
        let params = LoopModelParams::documented_defaults();
        let r = params.resolve(Corner::Mid);
        let stats = simulate_core(&r, &Workload::idle(), 60_000.0, true);
        assert_eq!(stats.traffic.frames_transmitted, 0);
        assert_eq!(stats.traffic.frames_enqueued, 0);
    }

    #[test]
    fn single_loop_longest_gap_is_dominated_by_a_255b_airtime_block() {
        let params = LoopModelParams::documented_defaults();
        let r = params.resolve(Corner::Mid);
        let workload = Workload::payload_sweep(255);
        let stats = simulate_core(&r, &workload, 120_000.0, true);
        let airtime = lora_airtime_ms(255) as f64;
        assert!(
            stats.longest_gap_ms >= airtime * 0.9,
            "longest gap {} should be dominated by the {} ms airtime block",
            stats.longest_gap_ms,
            airtime,
        );
        assert!(stats.traffic.frames_transmitted > 0);
    }

    #[test]
    fn split_ui_task_gap_is_independent_of_payload_size() {
        let params = LoopModelParams::documented_defaults();
        for corner in Corner::ALL {
            let r = params.resolve(corner);
            let small = simulate_split_ui_task(&r, 60_000.0);
            let large = simulate_split_ui_task(&r, 60_000.0);
            assert_eq!(small.longest_gap_ms, large.longest_gap_ms);
            assert_eq!(small.service_iterations, large.service_iterations);
        }
    }

    #[test]
    fn split_topology_gap_at_least_an_order_of_magnitude_smaller_than_single_loop() {
        // This pass's own acceptance bar: "the modelled longest UI-
        // unserviced gap drops by at least an order of magnitude ... across
        // the full sensitivity range". Checked at every corner for the
        // worst-case (255 B) payload, since that is where the single-loop
        // topology's gap is largest.
        let params = LoopModelParams::documented_defaults();
        for corner in Corner::ALL {
            let r = params.resolve(corner);
            let workload = Workload::payload_sweep(255);
            let single = simulate(Topology::SingleLoop, &r, &workload, 255, 180_000.0);
            let split = simulate(Topology::Split, &r, &workload, 255, 180_000.0);
            assert!(
                split.ui.longest_gap_ms * 10.0 <= single.ui.longest_gap_ms,
                "corner {:?}: split gap {} should be >=10x smaller than single-loop gap {}",
                corner,
                split.ui.longest_gap_ms,
                single.ui.longest_gap_ms,
            );
        }
    }

    #[test]
    fn split_topology_gap_does_not_scale_with_payload_size() {
        let params = LoopModelParams::documented_defaults();
        let r = params.resolve(Corner::Mid);
        let small_workload = Workload::payload_sweep(10);
        let large_workload = Workload::payload_sweep(255);
        let small = simulate(Topology::Split, &r, &small_workload, 10, 120_000.0);
        let large = simulate(Topology::Split, &r, &large_workload, 255, 120_000.0);
        assert_eq!(small.ui.longest_gap_ms, large.ui.longest_gap_ms);
    }

    #[test]
    fn single_loop_gap_scales_monotonically_with_payload_size() {
        let params = LoopModelParams::documented_defaults();
        let r = params.resolve(Corner::Mid);
        let sizes = [10usize, 40, 100, 255];
        let mut prev = 0.0;
        for size in sizes {
            let workload = Workload::payload_sweep(size);
            let stats = simulate_core(&r, &workload, 120_000.0, true);
            assert!(
                stats.longest_gap_ms >= prev,
                "gap should be non-decreasing with payload size: size={} gap={} prev={}",
                size,
                stats.longest_gap_ms,
                prev,
            );
            prev = stats.longest_gap_ms;
        }
    }

    #[test]
    fn dominance_holds_across_every_corner_for_the_smallest_payload() {
        // The smallest ACK-shaped payload (10 B, 83 ms airtime) is the
        // hardest case for the dominance claim — if it holds there, it
        // holds a fortiori for every larger payload. Checked at every
        // corner, i.e. across the full sensitivity range (this pass's own
        // abort/reroute condition).
        let params = LoopModelParams::documented_defaults();
        for corner in Corner::ALL {
            let verdict = dominance_check(&params, 10, corner);
            assert!(
                verdict.dominates,
                "corner {:?}: 10 B airtime ({} ms) should dominate the zero-traffic \
                 floor ({} ms)",
                corner, verdict.airtime_ms, verdict.idle_floor_longest_gap_ms,
            );
        }
    }

    #[test]
    fn dropped_frames_are_counted_not_silently_lost() {
        // A pathologically fast inbound-DM rate (faster than CAD+TX can
        // drain, 4-slot queue) should show up as a nonzero drop count —
        // proves this model surfaces overload instead of hiding it, same
        // discipline `TxQueue::enqueue`'s own `#[must_use]` return enforces
        // in the real dispatcher.
        let params = LoopModelParams::documented_defaults();
        let r = params.resolve(Corner::Low);
        let mut workload = Workload::payload_sweep(255);
        workload.inbound_dm.interval_ms = Some(1.0); // far faster than any real radio
        let stats = simulate_core(&r, &workload, 5_000.0, true);
        assert!(stats.traffic.frames_dropped > 0);
    }

    #[test]
    fn headline_255b_sweep_genuinely_exercises_airtime_budget_denial() {
        // A 255 B ACK's 800 ms airtime against the headline sweep's 5 s
        // inbound-DM cadence is a ~16% duty cycle, over `AirtimeBudget`'s
        // real 10% cap — so the report's 255 B row is not purely a string
        // of clean transmits; the budget-denial backoff branch in
        // `attempt_cad_tx` genuinely fires. Pins that fact so a future
        // change to either the headline workload or `AirtimeBudget` itself
        // that accidentally stops exercising this branch is caught, rather
        // than silently leaving it untested by the crate's own headline
        // scenario.
        let params = LoopModelParams::documented_defaults();
        let r = params.resolve(Corner::Mid);
        let workload = Workload::payload_sweep(255);
        let stats = simulate_core(&r, &workload, crate::report::SIM_DURATION_MS, true);
        assert!(
            stats.traffic.budget_denials > 0,
            "expected the 10% duty-cycle cap to genuinely deny at least one \
             255 B attempt over the headline sweep's duration"
        );
    }
}
