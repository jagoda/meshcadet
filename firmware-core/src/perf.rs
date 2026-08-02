// SPDX-License-Identifier: GPL-3.0-only
//! On-device superloop timing instrumentation — `--features diagnostics`
//! only. This is M0 of the `meshcadet-perf-rearchitecture` design: "the
//! walking skeleton is the *instrument*, not the optimization". The
//! design's whole premise (a task/core split fixes the dominant
//! radio-airtime-blocks-the-UI finding) currently rests on
//! `docs/perf/ui-perf-baseline.md`'s host-side
//! measurements plus static code reading — the device itself has never
//! reported a single timing number. This module is the pure-computation half
//! of closing that gap; the hardware-owning half (reading
//! `esp_timer_get_time()` around each phase, calling FreeRTOS's
//! `vTaskGetRunTimeStats()`, and logging the 30 s rollup) lives in
//! `firmware/src/main.rs` and `firmware/src/ui/mod.rs`. No behavior change:
//! every call site this module's types are threaded into is a read of a
//! monotonic clock around an already-existing operation, never a change to
//! what that operation does or when it runs.
//!
//! # Why a histogram instead of storing every sample
//!
//! An exact p95 needs every sample kept until the rollup closes. At this
//! loop's natural cadence (`RX_POLL_YIELD_MS` ≈ 5 ms when idle,
//! `main.rs`'s RX-poll doc) a 30 s window is several thousand iterations;
//! keeping every raw duration for six-plus phases would cost tens of KB —
//! real money against a main-task stack budget that has already overflowed
//! once in production (`firmware/sdkconfig.defaults`'s stack-budget comment).
//! [`Histogram`] instead buckets into `BUCKETS` power-of-two-width buckets
//! (128 B total, fixed, regardless of how many samples land in the window)
//! and reports the upper bound of whichever bucket the target percentile
//! falls in — a deliberately conservative (never-under-reports),
//! coarse-at-the-tail/fine-near-zero estimate. That is the right shape for
//! this campaign's actual question ("is the UI-starving tail dominated by
//! radio airtime, roughly how large, how often" — a go/no-go bound, not a
//! certified SLO number) and is exactly the same "honest proxy, not a
//! guarantee" framing [`crate::signal_tracker`] already uses for the signal
//! meter.
//!
//! # Units
//!
//! [`PhaseStats`]/[`Histogram`] are unit-agnostic — they just accumulate
//! `u32` durations. Each call site chooses microseconds or milliseconds
//! based on what that phase can actually resolve; see the doc on each
//! `main.rs`/`ui/mod.rs` call site for which it uses and why.

/// Number of histogram buckets. Bucket `0` holds exactly the value `0`;
/// bucket `i` (`1..=31`) holds `[2^(i-1), 2^i)`. 32 buckets span the full
/// `u32` range, so no value is ever out of range — the top bucket is simply
/// a catch-all for anything implausibly large (a wrapped/garbage clock read,
/// say), never a panic or a silently dropped sample.
#[cfg(feature = "diagnostics")]
const BUCKETS: usize = 32;

/// Fixed-memory streaming histogram used to approximate a percentile without
/// storing samples. See the module doc for the bucket shape and the
/// accuracy/memory trade-off this makes.
#[cfg(feature = "diagnostics")]
#[derive(Debug, Clone, Copy)]
struct Histogram {
    buckets: [u32; BUCKETS],
}

#[cfg(feature = "diagnostics")]
impl Histogram {
    fn new() -> Self {
        Self {
            buckets: [0; BUCKETS],
        }
    }

    /// Bucket index for `value`: `0` for exactly `0`, otherwise
    /// `floor(log2(value)) + 1`, clamped to the last bucket.
    ///
    /// `u32::leading_zeros()` conveniently already gives the right answer
    /// for `value == 0` too (`leading_zeros(0) == 32`, so `32 - 32 == 0`) —
    /// no separate zero-case branch needed.
    fn bucket_index(value: u32) -> usize {
        let idx = (32 - value.leading_zeros()) as usize;
        idx.min(BUCKETS - 1)
    }

    /// Exclusive-upper-bound-minus-one representative value for `idx` — the
    /// largest value that bucket could actually hold. Reporting this (rather
    /// than the bucket's lower bound) is the "never under-report the tail"
    /// half of the honest-proxy framing in the module doc.
    fn bucket_representative(idx: usize) -> u32 {
        if idx == 0 {
            0
        } else {
            // `idx` is clamped to `BUCKETS - 1` == 31, so `1u64 << idx` is at
            // most `1u64 << 31`, comfortably inside `u64` — no overflow.
            (((1u64) << idx) - 1) as u32
        }
    }

    fn record(&mut self, value: u32) {
        let idx = Self::bucket_index(value);
        self.buckets[idx] = self.buckets[idx].saturating_add(1);
    }

    /// Approximate the `p`-th quantile (`p` in `[0.0, 1.0]`). Returns `0` if
    /// no samples have been recorded yet.
    fn percentile(&self, p: f64) -> u32 {
        let total: u64 = self.buckets.iter().map(|&c| c as u64).sum();
        if total == 0 {
            return 0;
        }
        // `.max(1)` — even p=0.0 should land in the first non-empty bucket,
        // not report a phantom "below everything" 0.
        let threshold = ((p * total as f64).ceil() as u64).max(1);
        let mut cumulative = 0u64;
        for (idx, &count) in self.buckets.iter().enumerate() {
            cumulative += count as u64;
            if cumulative >= threshold {
                return Self::bucket_representative(idx);
            }
        }
        // Unreachable in practice (the loop above always finds `threshold`
        // once `cumulative` reaches `total`, and `threshold <= total`) — kept
        // as a safe fallback rather than an `unreachable!()` panic, since
        // this reads into a hot dispatcher-loop-adjacent path.
        Self::bucket_representative(BUCKETS - 1)
    }
}

/// A read-only rollup of one [`PhaseStats`] accumulator's window: sample
/// count, min, mean, max, and an approximate p95 — all in whatever unit the
/// accumulator was fed (see the module doc's "Units" section).
#[cfg(feature = "diagnostics")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhaseSnapshot {
    pub count: u32,
    pub min: u32,
    pub mean: u32,
    pub max: u32,
    pub p95: u32,
}

/// Rolling min/mean/max/p95 accumulator for one superloop phase (or one
/// latency measurement — the shape is identical either way). `record()` is
/// the only mutation; `snapshot()` reads without resetting; the caller
/// resets by discarding and constructing a fresh one (matches the existing
/// `rx_done_count = 0;`-style reset idiom at `main.rs`'s 30 s rollup site).
#[cfg(feature = "diagnostics")]
#[derive(Debug, Clone, Copy)]
pub struct PhaseStats {
    count: u32,
    sum: u64,
    min: u32,
    max: u32,
    hist: Histogram,
}

#[cfg(feature = "diagnostics")]
impl Default for PhaseStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "diagnostics")]
impl PhaseStats {
    pub fn new() -> Self {
        Self {
            count: 0,
            sum: 0,
            min: u32::MAX,
            max: 0,
            hist: Histogram::new(),
        }
    }

    pub fn record(&mut self, duration: u32) {
        self.count += 1;
        self.sum += duration as u64;
        if duration < self.min {
            self.min = duration;
        }
        if duration > self.max {
            self.max = duration;
        }
        self.hist.record(duration);
    }

    pub fn snapshot(&self) -> PhaseSnapshot {
        if self.count == 0 {
            return PhaseSnapshot::default();
        }
        PhaseSnapshot {
            count: self.count,
            min: self.min,
            mean: (self.sum / self.count as u64) as u32,
            max: self.max,
            p95: self.hist.percentile(0.95),
        }
    }
}

/// One 30 s window's worth of superloop instrumentation — one [`PhaseStats`]
/// per named phase from the M0 objective (GPS poll, battery poll, CAD, TX,
/// RX-poll, `ui.step()`'s own duration, plus the RX-notice latency proxy),
/// and the UI-starvation counters. The input-to-first-paint latency is a
/// separate metric and lives in [`crate::ui`]'s `UiRuntime` instead (see
/// that module's `input_paint_stats`/`take_input_paint_stats` — this file
/// has no dependency on Slint or the UI module) and is folded into the same
/// log line by the `main.rs` call site, not by this struct.
///
/// Fields are `pub` so the owning call site can `record()`/`snapshot()`
/// each phase directly without a forwarding method per phase — this struct
/// is a plain aggregate, the same shape convention `gps::GpsStatus`/
/// `battery::BatteryStatus` already use for their own plain-data structs.
#[cfg(feature = "diagnostics")]
pub struct PerfRollup {
    pub gps: PhaseStats,
    pub battery: PhaseStats,
    pub cad: PhaseStats,
    pub tx: PhaseStats,
    pub rx_poll: PhaseStats,
    /// `ui.step()`'s own call duration — distinct from the input-to-first-
    /// paint latency tracked by `UiRuntime::input_paint_stats` (that one
    /// measures from an input event to the next render; this one measures
    /// only the `step()` call itself). See `main.rs`'s touch-UI-step call
    /// site for where this is recorded.
    pub ui_step: PhaseStats,
    /// Proxy for "how long after a frame was actually ready did the
    /// dispatcher notice it" — see `main.rs`'s RX-poll call site doc for the
    /// exact (deliberately honest-proxy, not hardware-timestamped) definition.
    pub rx_notice: PhaseStats,
    /// Cumulative milliseconds, across the whole window, during which
    /// `ui.step()` was NOT the thing running — i.e. the sum, over every
    /// iteration, of `(iteration wall clock up to and including the
    /// ui.step() call) - (that call's own duration)`. See `main.rs`'s
    /// dispatcher-loop call site for the exact accounting and why this is
    /// the right definition (`ui.step()` itself runs every iteration
    /// unconditionally in the current single-loop design — see the
    /// `meshcadet-perf-rearchitecture` design's §1 — so
    /// "did not run" is a statement about UI *responsiveness*, i.e. how much
    /// of the window the UI thread spent doing something other than UI
    /// work, not about whether the call happened at all).
    pub ui_starvation_cumulative_ms: u32,
    /// The single largest one-iteration starvation gap in the window — the
    /// number that would spike to LoRa airtime (tens to ~800 ms per
    /// `docs/perf/ui-perf-baseline.md` §4) if the campaign's §2 dominant
    /// finding holds on real hardware.
    pub ui_starvation_longest_ms: u32,
}

#[cfg(feature = "diagnostics")]
impl Default for PerfRollup {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "diagnostics")]
impl PerfRollup {
    pub fn new() -> Self {
        Self {
            gps: PhaseStats::new(),
            battery: PhaseStats::new(),
            cad: PhaseStats::new(),
            tx: PhaseStats::new(),
            rx_poll: PhaseStats::new(),
            ui_step: PhaseStats::new(),
            rx_notice: PhaseStats::new(),
            ui_starvation_cumulative_ms: 0,
            ui_starvation_longest_ms: 0,
        }
    }

    /// Fold one iteration's starvation gap (milliseconds) into the window's
    /// running cumulative sum and longest-gap high-water mark.
    pub fn record_ui_starvation(&mut self, gap_ms: u32) {
        self.ui_starvation_cumulative_ms = self.ui_starvation_cumulative_ms.saturating_add(gap_ms);
        if gap_ms > self.ui_starvation_longest_ms {
            self.ui_starvation_longest_ms = gap_ms;
        }
    }
}

/// Parse the fixed ASCII table FreeRTOS's `vTaskGetRunTimeStats()` writes
/// (`CONFIG_FREERTOS_GENERATE_RUN_TIME_STATS` +
/// `CONFIG_FREERTOS_USE_STATS_FORMATTING_FUNCTIONS` — see
/// `firmware/sdkconfig.defaults`) and return the reported percentage for the
/// named task/row, if present.
///
/// Table format (whitespace-separated, one task per line):
/// `<name> <abs-time> <percent>%`. Parsed defensively line-by-line: a blank
/// line, an unrecognized line, or a missing task all just fail to match
/// rather than aborting the rest of the table — this reads a string a
/// hardware/FreeRTOS-version boundary owns the exact shape of, which this
/// crate cannot control or fully anticipate every variant of.
#[cfg(feature = "diagnostics")]
pub fn parse_task_percent(stats: &str, task_name: &str) -> Option<u8> {
    for line in stats.lines() {
        let mut fields = line.split_whitespace();
        let name = match fields.next() {
            Some(n) => n,
            None => continue,
        };
        if name != task_name {
            continue;
        }
        let _abs_time = fields.next();
        let percent_field = match fields.next() {
            Some(f) => f,
            None => continue,
        };
        let digits: String = percent_field
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(pct) = digits.parse::<u8>() {
            return Some(pct);
        }
    }
    None
}

/// Derive per-core utilization from a `vTaskGetRunTimeStats()` table: each
/// ESP32-S3 SMP idle task (`IDLE0`/`IDLE1`) is pinned to exactly one core by
/// FreeRTOS's own construction, so `100 - idle%` for that named row IS that
/// core's utilization — no separate core-affinity lookup is needed. Returns
/// `(core0_pct, core1_pct)`; either is `None` if its idle-task row was not
/// found (an unexpected table shape, or the Kconfig prerequisites are
/// somehow not actually active) rather than a fabricated number.
#[cfg(feature = "diagnostics")]
pub fn per_core_utilization_pct(stats: &str) -> (Option<u8>, Option<u8>) {
    let core0 = parse_task_percent(stats, "IDLE0").map(|idle| 100u8.saturating_sub(idle));
    let core1 = parse_task_percent(stats, "IDLE1").map(|idle| 100u8.saturating_sub(idle));
    (core0, core1)
}

#[cfg(all(test, feature = "diagnostics"))]
mod tests {
    use super::*;

    // ── Histogram / PhaseStats ──────────────────────────────────────────────

    #[test]
    fn bucket_index_zero_is_bucket_zero() {
        assert_eq!(Histogram::bucket_index(0), 0);
    }

    #[test]
    fn bucket_index_powers_of_two_start_a_new_bucket() {
        assert_eq!(Histogram::bucket_index(1), 1);
        assert_eq!(Histogram::bucket_index(2), 2);
        assert_eq!(Histogram::bucket_index(3), 2);
        assert_eq!(Histogram::bucket_index(4), 3);
        assert_eq!(Histogram::bucket_index(7), 3);
        assert_eq!(Histogram::bucket_index(8), 4);
    }

    #[test]
    fn bucket_index_never_exceeds_last_bucket() {
        assert_eq!(Histogram::bucket_index(u32::MAX), BUCKETS - 1);
        assert_eq!(Histogram::bucket_index(1 << 31), BUCKETS - 1);
    }

    #[test]
    fn bucket_representative_is_never_below_the_bucket_lower_bound() {
        for idx in 0..BUCKETS {
            let repr = Histogram::bucket_representative(idx);
            if idx > 0 {
                let lower = 1u64 << (idx - 1);
                assert!(repr as u64 >= lower, "idx={idx} repr={repr} lower={lower}");
            }
        }
    }

    #[test]
    fn empty_histogram_percentile_is_zero() {
        let h = Histogram::new();
        assert_eq!(h.percentile(0.95), 0);
    }

    #[test]
    fn uniform_distribution_p95_lands_in_the_high_tail() {
        let mut h = Histogram::new();
        for v in 1..=100u32 {
            h.record(v);
        }
        // The true 95th percentile is 95; the bucket containing 95 covers
        // [64, 128), so this is the loosest correct bound this coarse a
        // histogram can promise.
        let p95 = h.percentile(0.95);
        assert!(p95 >= 95, "p95={p95}");
        assert!(p95 < 128, "p95={p95}");
    }

    #[test]
    fn all_zero_samples_report_p95_zero() {
        let mut h = Histogram::new();
        for _ in 0..50 {
            h.record(0);
        }
        assert_eq!(h.percentile(0.95), 0);
    }

    #[test]
    fn rare_high_outliers_are_visible_past_the_5_percent_mark() {
        // 95 fast samples + 5 slow outliers (5% of 100) — the p95 threshold
        // (ceil(0.95*100)=95) sits exactly on the boundary, so the outliers
        // must NOT be absorbed into the low bucket.
        let mut h = Histogram::new();
        for _ in 0..95 {
            h.record(1);
        }
        for _ in 0..5 {
            h.record(800);
        }
        let p95 = h.percentile(0.95);
        assert!(p95 <= 1, "p95 should still read the low cluster, got {p95}");
    }

    #[test]
    fn outliers_dominate_once_past_the_95th_sample() {
        let mut h = Histogram::new();
        for _ in 0..94 {
            h.record(1);
        }
        for _ in 0..6 {
            h.record(800);
        }
        // threshold = ceil(0.95*100) = 95; cumulative through the low
        // cluster is 94 < 95, so the 95th sample (the first outlier) decides
        // the result — the outlier bucket ([512,1024)) must be reported.
        let p95 = h.percentile(0.95);
        assert!(p95 >= 512, "p95={p95}");
    }

    #[test]
    fn phase_stats_empty_snapshot_is_all_zero() {
        let stats = PhaseStats::new();
        assert_eq!(stats.snapshot(), PhaseSnapshot::default());
    }

    #[test]
    fn phase_stats_tracks_min_mean_max() {
        let mut stats = PhaseStats::new();
        for v in [10, 20, 30, 40] {
            stats.record(v);
        }
        let snap = stats.snapshot();
        assert_eq!(snap.count, 4);
        assert_eq!(snap.min, 10);
        assert_eq!(snap.max, 40);
        assert_eq!(snap.mean, 25);
    }

    #[test]
    fn phase_stats_single_sample_all_fields_equal_it() {
        // 7 == 2^3 - 1, a bucket's exact upper bound, so the histogram's
        // conservative-representative p95 lands on the same value as
        // min/mean/max — unlike an arbitrary value (see the module doc's
        // "never under-report the tail" framing), this is the one shape
        // where a single-sample histogram is exact rather than merely
        // bounded.
        let mut stats = PhaseStats::new();
        stats.record(7);
        let snap = stats.snapshot();
        assert_eq!(snap.count, 1);
        assert_eq!(snap.min, 7);
        assert_eq!(snap.mean, 7);
        assert_eq!(snap.max, 7);
        assert_eq!(snap.p95, 7);
    }

    // ── PerfRollup ────────────────────────────────────────────────────────

    #[test]
    fn ui_starvation_accumulates_and_tracks_longest() {
        let mut rollup = PerfRollup::new();
        rollup.record_ui_starvation(5);
        rollup.record_ui_starvation(800);
        rollup.record_ui_starvation(3);
        assert_eq!(rollup.ui_starvation_cumulative_ms, 808);
        assert_eq!(rollup.ui_starvation_longest_ms, 800);
    }

    #[test]
    fn fresh_rollup_has_zeroed_starvation() {
        let rollup = PerfRollup::new();
        assert_eq!(rollup.ui_starvation_cumulative_ms, 0);
        assert_eq!(rollup.ui_starvation_longest_ms, 0);
    }

    // ── parse_task_percent / per_core_utilization_pct ────────────────────────

    const SAMPLE_TABLE: &str = "\
IDLE0\t\t155000\t\t45%\r
IDLE1\t\t160000\t\t47%\r
main\t\t20000\t\t6%\r
Tmr Svc\t\t500\t\t0%\r
";

    #[test]
    fn parses_a_known_task_row() {
        assert_eq!(parse_task_percent(SAMPLE_TABLE, "IDLE0"), Some(45));
        assert_eq!(parse_task_percent(SAMPLE_TABLE, "IDLE1"), Some(47));
        assert_eq!(parse_task_percent(SAMPLE_TABLE, "main"), Some(6));
    }

    #[test]
    fn unknown_task_name_is_none() {
        assert_eq!(parse_task_percent(SAMPLE_TABLE, "nonexistent"), None);
    }

    #[test]
    fn blank_lines_do_not_abort_the_scan() {
        let table = "\n\nIDLE0\t\t1\t\t99%\n\n";
        assert_eq!(parse_task_percent(table, "IDLE0"), Some(99));
    }

    #[test]
    fn empty_table_is_none() {
        assert_eq!(parse_task_percent("", "IDLE0"), None);
    }

    #[test]
    fn malformed_row_missing_percent_field_is_none() {
        let table = "IDLE0\t\t155000\n";
        assert_eq!(parse_task_percent(table, "IDLE0"), None);
    }

    #[test]
    fn per_core_utilization_is_the_complement_of_idle() {
        assert_eq!(per_core_utilization_pct(SAMPLE_TABLE), (Some(55), Some(53)));
    }

    #[test]
    fn missing_idle_rows_report_none_not_a_fabricated_number() {
        let table = "main\t\t20000\t\t6%\n";
        assert_eq!(per_core_utilization_pct(table), (None, None));
    }
}
