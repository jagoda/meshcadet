// SPDX-License-Identifier: GPL-3.0-only
//! Parses the raw serial-console capture pasted into a [`crate::
//! report_block::ReportBlock`]'s `raw_serial_log` field. Every line shape
//! here is quoted verbatim from `docs/perf/collection-kit.md` Part C/E/G —
//! see each parser function's doc for its exact source citation.
//!
//! This module does no unit conversion (µs vs ms) and no cross-window
//! aggregation — a Part C capture logs one rollup block per phase every
//! 30 s, so a single log commonly contains several [`Rollup`]s for the
//! same phase (an idle window, a navigation-event window, ...). Collapsing
//! those into one number is a scenario-dependent judgement call (D1 wants
//! two SPECIFIC windows compared, not an average of all of them) that this
//! parser deliberately leaves to the caller — see [`ParsedLog::phase_
//! windows`] and [`ParsedLog::latest_phase_window`].

use std::collections::HashMap;

/// One `PERF <label>: n=<count> min=<v> mean=<v> max=<v> p95=<v>` line —
/// covers `PERF phase=<name>: ...` (Part C, µs), `PERF rx-notice-latency:
/// ...` (Part C, µs), and `PERF input-to-first-paint: ...` (Part D10, ms).
/// Units are NOT tracked here — the caller knows which parser function it
/// called and therefore which unit applies (see each parsing entry point's
/// doc).
#[derive(Debug, Clone, PartialEq)]
pub struct Rollup {
    pub label: String,
    pub n: u64,
    pub min: f64,
    pub mean: f64,
    pub max: f64,
    pub p95: f64,
}

impl Rollup {
    /// For a `phase=<name>` label, the bare phase name (`gps`, `cad`,
    /// `ui_step`, ...). `None` for a non-phase rollup (`rx-notice-latency`,
    /// `input-to-first-paint`).
    pub fn phase_name(&self) -> Option<&str> {
        self.label.strip_prefix("phase=")
    }
}

/// `PERF ui-starvation: cumulative=<ms> longest=<ms> (window=<n>s)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiStarvation {
    pub cumulative_ms: f64,
    pub longest_ms: f64,
    pub window_s: f64,
}

/// `PERF core-utilization: core0=<pct|n/a> core1=<pct|n/a>`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoreUtilization {
    pub core0_pct: Option<f64>,
    pub core1_pct: Option<f64>,
}

/// `PERF heap-internal: free=<bytes> min_ever=<bytes>` (Part C; ADR-0012
/// D-H, `ui-perf-baseline.md` §9.1). `MALLOC_CAP_INTERNAL` only — this is
/// free internal-SRAM heap, not total free heap, and PSRAM headroom is not
/// tracked here at all. `free` is the instantaneous reading at this 30 s
/// tick; `min_ever` is the lifetime low-water mark since boot
/// (`heap_caps_get_minimum_free_size`), so a transient squeeze that
/// recovered between rollup windows is still visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapInternal {
    pub free_bytes: u64,
    pub min_ever_bytes: u64,
}

/// One stack high-water-mark sample (Part E). The one-shot UI-navigation
/// samples only ever report `free_bytes`; the periodic task/server samples
/// additionally report a budget and headroom percentage.
#[derive(Debug, Clone, PartialEq)]
pub struct StackHwmSample {
    pub label: String,
    pub free_bytes: u64,
    pub total_bytes: Option<u64>,
    pub peak_bytes: Option<u64>,
    pub headroom_pct: Option<f64>,
}

/// Line-prefix counters relevant to D5 (delivery success rate) and to
/// detecting a mid-capture reset — see `docs/perf/collection-kit.md` Part
/// C's "A failed run looks like ..." note and Part G's D5 procedure. This
/// crate reports the raw counts only: attributing a `TX:` line to a
/// specific peer (needed for a true sent/ACKed rate) is not recoverable
/// from this line shape alone — collection-kit.md's own D5 procedure
/// counts by hand from context for that reason, and this parser does not
/// pretend otherwise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeliveryCounters {
    pub tx_lines: u64,
    pub ack_received: u64,
    pub rx_done: u64,
    pub rx_dm: u64,
    pub rx_grp_txt: u64,
    pub cad_busy: u64,
    pub tx_retry: u64,
    /// `firmware build: <sha>` boot-banner occurrences. More than one in a
    /// single capture is Part C's own definition of "a failed run" (the
    /// device reset mid-capture).
    pub firmware_boots: u64,
}

impl DeliveryCounters {
    pub fn looks_like_reset_mid_capture(&self) -> bool {
        self.firmware_boots > 1
    }
}

/// Everything this module can extract from one raw serial-log capture.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedLog {
    pub rollups: Vec<Rollup>,
    pub ui_starvation: Vec<UiStarvation>,
    pub core_utilization: Vec<CoreUtilization>,
    pub heap_internal: Vec<HeapInternal>,
    pub stack_hwm: Vec<StackHwmSample>,
    pub delivery: DeliveryCounters,
}

impl ParsedLog {
    /// Every rollup window recorded for `phase` (e.g. `"gps"`, `"ui_step"`,
    /// `"cad"`), in chronological (log) order.
    pub fn phase_windows(&self, phase: &str) -> Vec<&Rollup> {
        self.rollups
            .iter()
            .filter(|r| r.phase_name() == Some(phase))
            .collect()
    }

    /// The chronologically LAST window recorded for `phase` that actually
    /// carried samples (`n > 0`) — a defensible, simple default for "the
    /// one number to calibrate against" when the caller has no
    /// scenario-specific window selection to make. `n=0` windows are
    /// skipped per Part C's own instruction: `n=0` is "no samples," not a
    /// real zero-cost reading. Returns `None` if `phase` never reported a
    /// window with any samples.
    pub fn latest_phase_window(&self, phase: &str) -> Option<&Rollup> {
        self.phase_windows(phase)
            .into_iter()
            .rev()
            .find(|r| r.n > 0)
    }

    /// The chronologically LAST heap-internal reading, if any — the same
    /// "last window is the one number to report" convention as
    /// [`Self::latest_phase_window`].
    pub fn latest_heap_internal(&self) -> Option<&HeapInternal> {
        self.heap_internal.last()
    }

    /// Part C's own definition of a run where the `diagnostics` feature
    /// did not actually compile in: `gps`/`battery`/`rx_poll` are supposed
    /// to run every iteration unconditionally, so EVERY window reading
    /// `n=0` for all three (when at least one window exists) is the
    /// documented failure signature, not a coincidence.
    pub fn looks_like_diagnostics_not_compiled(&self) -> bool {
        let always_running = ["gps", "battery", "rx_poll"];
        let mut saw_any_window = false;
        for phase in always_running {
            let windows = self.phase_windows(phase);
            if windows.is_empty() {
                continue;
            }
            saw_any_window = true;
            if windows.iter().any(|r| r.n > 0) {
                return false;
            }
        }
        saw_any_window
    }
}

/// Parse everything this module recognizes out of `raw_log`. Unrecognized
/// lines (ordinary log noise) are silently skipped — this is a targeted
/// extractor, not a strict-grammar log validator.
pub fn parse(raw_log: &str) -> ParsedLog {
    let mut parsed = ParsedLog::default();

    for line in raw_log.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("PERF phase=") {
            if let Some(r) = parse_named_rollup(&format!("phase={rest}")) {
                parsed.rollups.push(r);
            }
        } else if let Some(rest) = t.strip_prefix("PERF rx-notice-latency:") {
            if let Some(r) = parse_rollup_fields("rx-notice-latency", rest) {
                parsed.rollups.push(r);
            }
        } else if let Some(rest) = t.strip_prefix("PERF input-to-first-paint:") {
            if let Some(r) = parse_rollup_fields("input-to-first-paint", rest) {
                parsed.rollups.push(r);
            }
        } else if let Some(rest) = t.strip_prefix("PERF ui-starvation:") {
            if let Some(s) = parse_ui_starvation(rest) {
                parsed.ui_starvation.push(s);
            }
        } else if let Some(rest) = t.strip_prefix("PERF core-utilization:") {
            if let Some(c) = parse_core_utilization(rest) {
                parsed.core_utilization.push(c);
            }
        } else if let Some(rest) = t.strip_prefix("PERF heap-internal:") {
            if let Some(h) = parse_heap_internal(rest) {
                parsed.heap_internal.push(h);
            }
        } else if let Some(s) = parse_stack_hwm_line(t) {
            parsed.stack_hwm.push(s);
        } else {
            count_delivery_line(t, &mut parsed.delivery);
        }
    }

    parsed
}

/// `phase=<name>: n=.. min=.. mean=.. max=.. p95=..` — `label` is already
/// `phase=<name>`.
fn parse_named_rollup(label_and_rest: &str) -> Option<Rollup> {
    let (label, rest) = label_and_rest.split_once(':')?;
    parse_rollup_fields(label, rest)
}

/// Shared `n=.. min=.. mean=.. max=.. p95=..` field parser, used by every
/// `PERF` rollup-shaped line regardless of its label.
fn parse_rollup_fields(label: &str, rest: &str) -> Option<Rollup> {
    let fields = key_value_fields(rest);
    Some(Rollup {
        label: label.to_string(),
        n: fields.get("n")?.parse().ok()?,
        min: fields.get("min")?.parse().ok()?,
        mean: fields.get("mean")?.parse().ok()?,
        max: fields.get("max")?.parse().ok()?,
        p95: fields.get("p95")?.parse().ok()?,
    })
}

/// `cumulative=<ms> longest=<ms> (window=<n>s)`.
fn parse_ui_starvation(rest: &str) -> Option<UiStarvation> {
    let fields = key_value_fields(rest);
    let window_s = fields
        .get("window")
        .map(|v| v.trim_end_matches('s'))
        .and_then(|v| v.parse::<f64>().ok())?;
    Some(UiStarvation {
        cumulative_ms: fields.get("cumulative")?.parse().ok()?,
        longest_ms: fields.get("longest")?.parse().ok()?,
        window_s,
    })
}

/// `core0=<pct|n/a> core1=<pct|n/a>`.
fn parse_core_utilization(rest: &str) -> Option<CoreUtilization> {
    let fields = key_value_fields(rest);
    let parse_pct = |v: &str| -> Option<f64> {
        if v == "n/a" {
            None
        } else {
            v.trim_end_matches('%').parse::<f64>().ok()
        }
    };
    Some(CoreUtilization {
        core0_pct: fields.get("core0").and_then(|v| parse_pct(v)),
        core1_pct: fields.get("core1").and_then(|v| parse_pct(v)),
    })
}

/// `free=<bytes> min_ever=<bytes>`.
fn parse_heap_internal(rest: &str) -> Option<HeapInternal> {
    let fields = key_value_fields(rest);
    Some(HeapInternal {
        free_bytes: fields.get("free")?.parse().ok()?,
        min_ever_bytes: fields.get("min_ever")?.parse().ok()?,
    })
}

/// Splits `rest` into `key=value` pairs on whitespace, tolerating a
/// trailing parenthesised remark like `(window=30s)` by stripping stray
/// `(`/`)` characters before splitting on `=`.
fn key_value_fields(rest: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for tok in rest.split_whitespace() {
        let cleaned = tok.trim_matches(|c| c == '(' || c == ')');
        if let Some((k, v)) = cleaned.split_once('=') {
            out.insert(k.to_string(), v.to_string());
        }
    }
    out
}

/// Recognizes every Part E stack-HWM line shape:
/// - `main-task: stack HWM: <free> B free / <total> B total = <peak> B peak (<pct>% headroom)`
/// - `admin_server: stack HWM: ...` / `prov_server: stack HWM: ...` (same shape)
/// - `ui: navigate_to_pin_entry stack HWM: <free> B free` (free-only, no budget)
/// - `ui: navigate_to_admin_menu stack HWM: <free> B free` (free-only, no budget)
fn parse_stack_hwm_line(line: &str) -> Option<StackHwmSample> {
    let (label, rest) = line.split_once("stack HWM:")?;
    let label = label.trim().trim_end_matches(':').to_string();
    let rest = rest.trim();

    // `<free> B free` is common to every shape; everything after it is
    // optional (the two UI one-shot samples stop there).
    let free_bytes = rest
        .split_whitespace()
        .next()
        .and_then(|v| v.parse::<u64>().ok())?;

    if !rest.contains(" total") {
        return Some(StackHwmSample {
            label,
            free_bytes,
            total_bytes: None,
            peak_bytes: None,
            headroom_pct: None,
        });
    }

    // "<free> B free / <total> B total = <peak> B peak (<pct>% headroom)"
    let after_slash = rest.split_once('/')?.1;
    let total_bytes = after_slash.split_whitespace().next()?.parse::<u64>().ok();
    let after_eq = after_slash.split_once('=')?.1;
    let peak_bytes = after_eq.split_whitespace().next()?.parse::<u64>().ok();
    let headroom_pct = after_eq
        .split_once('(')
        .and_then(|(_, tail)| tail.split('%').next())
        .and_then(|v| v.parse::<f64>().ok());

    Some(StackHwmSample {
        label,
        free_bytes,
        total_bytes,
        peak_bytes,
        headroom_pct,
    })
}

/// Increments whichever [`DeliveryCounters`] field `line` matches, per
/// Part G's cited line prefixes. A line matches at most one counter.
fn count_delivery_line(line: &str, counters: &mut DeliveryCounters) {
    if line.starts_with("firmware build:") {
        counters.firmware_boots += 1;
    } else if line.starts_with("TX error:") {
        counters.tx_retry += 1;
    } else if line.starts_with("TX:") {
        counters.tx_lines += 1;
    } else if line.starts_with("ACK received:") {
        counters.ack_received += 1;
    } else if line.starts_with("RX RxDone:") {
        counters.rx_done += 1;
    } else if line.starts_with("RX DM from") {
        counters.rx_dm += 1;
    } else if line.starts_with("RX GRP_TXT") {
        counters.rx_grp_txt += 1;
    } else if line.starts_with("CAD: channel busy") {
        counters.cad_busy += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_LOG: &str = "\
firmware build: a1b2c3d
identity ready: pub_hash=0x12, pubkey=deadbeef
PERF phase=gps: n=1 min=800 mean=800 max=800 p95=800
PERF phase=battery: n=1 min=120 mean=120 max=120 p95=120
PERF phase=cad: n=0 min=0 mean=0 max=0 p95=0
PERF phase=tx: n=0 min=0 mean=0 max=0 p95=0
PERF phase=rx_poll: n=30 min=2 mean=3 max=6 p95=5
PERF phase=ui_step: n=120 min=10 mean=45 max=310 p95=180
PERF rx-notice-latency: n=0 min=0 mean=0 max=0 p95=0
PERF ui-starvation: cumulative=12 longest=8 (window=30s)
PERF input-to-first-paint: n=3 min=90 mean=140 max=210 p95=205
PERF core-utilization: core0=4.2 core1=n/a
PERF heap-internal: free=182304 min_ever=170112
main-task: stack HWM: 18432 B free / 49152 B total = 30720 B peak (37.5% headroom)
ui: navigate_to_pin_entry stack HWM: 15000 B free
ui: navigate_to_admin_menu stack HWM: 14500 B free
admin_server: stack HWM: 9000 B free / 12288 B total = 3288 B peak (73.2% headroom)
prov_server: stack HWM: 6000 B free / 8192 B total = 2192 B peak (73.2% headroom)
TX: 10 bytes, 83ms airtime
ACK received: matches last-sent DM
RX RxDone: 10 bytes, rssi=-42dBm snr=9dB (raw 1/1)
RX DM from 0x12 ...
RX GRP_TXT from 0x34 ...
CAD: channel busy, deferring retry 40ms
TX error: retained for retry in 200ms
";

    #[test]
    fn parses_every_phase_rollup() {
        let parsed = parse(SAMPLE_LOG);
        let phases: Vec<&str> = parsed
            .rollups
            .iter()
            .filter_map(|r| r.phase_name())
            .collect();
        assert!(phases.contains(&"gps"));
        assert!(phases.contains(&"battery"));
        assert!(phases.contains(&"cad"));
        assert!(phases.contains(&"tx"));
        assert!(phases.contains(&"rx_poll"));
        assert!(phases.contains(&"ui_step"));

        let ui_step = parsed.latest_phase_window("ui_step").unwrap();
        assert_eq!(ui_step.n, 120);
        assert_eq!(ui_step.mean, 45.0);
        assert_eq!(ui_step.p95, 180.0);
        assert_eq!(ui_step.max, 310.0);
    }

    #[test]
    fn latest_phase_window_skips_zero_sample_windows() {
        let parsed = parse(SAMPLE_LOG);
        // `cad` only has one window and it's n=0 -> no usable window.
        assert!(parsed.latest_phase_window("cad").is_none());
        // But the window is still recorded for inspection.
        assert_eq!(parsed.phase_windows("cad").len(), 1);
    }

    #[test]
    fn parses_rx_notice_latency_as_a_non_phase_rollup() {
        let parsed = parse(SAMPLE_LOG);
        let r = parsed
            .rollups
            .iter()
            .find(|r| r.label == "rx-notice-latency")
            .unwrap();
        assert_eq!(r.n, 0);
        assert_eq!(r.phase_name(), None);
    }

    #[test]
    fn parses_input_to_first_paint() {
        let parsed = parse(SAMPLE_LOG);
        let r = parsed
            .rollups
            .iter()
            .find(|r| r.label == "input-to-first-paint")
            .unwrap();
        assert_eq!(r.n, 3);
        assert_eq!(r.mean, 140.0);
    }

    #[test]
    fn parses_ui_starvation() {
        let parsed = parse(SAMPLE_LOG);
        assert_eq!(parsed.ui_starvation.len(), 1);
        assert_eq!(parsed.ui_starvation[0].cumulative_ms, 12.0);
        assert_eq!(parsed.ui_starvation[0].longest_ms, 8.0);
        assert_eq!(parsed.ui_starvation[0].window_s, 30.0);
    }

    #[test]
    fn parses_core_utilization_with_an_na_core() {
        let parsed = parse(SAMPLE_LOG);
        assert_eq!(parsed.core_utilization.len(), 1);
        assert_eq!(parsed.core_utilization[0].core0_pct, Some(4.2));
        assert_eq!(parsed.core_utilization[0].core1_pct, None);
    }

    #[test]
    fn parses_heap_internal() {
        let parsed = parse(SAMPLE_LOG);
        assert_eq!(parsed.heap_internal.len(), 1);
        assert_eq!(parsed.heap_internal[0].free_bytes, 182304);
        assert_eq!(parsed.heap_internal[0].min_ever_bytes, 170112);
        assert_eq!(
            parsed.latest_heap_internal(),
            Some(&HeapInternal {
                free_bytes: 182304,
                min_ever_bytes: 170112
            })
        );
    }

    #[test]
    fn latest_heap_internal_is_none_when_absent() {
        assert_eq!(ParsedLog::default().latest_heap_internal(), None);
    }

    #[test]
    fn parses_all_five_stack_hwm_shapes() {
        let parsed = parse(SAMPLE_LOG);
        assert_eq!(parsed.stack_hwm.len(), 5);

        let main = parsed
            .stack_hwm
            .iter()
            .find(|s| s.label == "main-task")
            .unwrap();
        assert_eq!(main.free_bytes, 18432);
        assert_eq!(main.total_bytes, Some(49152));
        assert_eq!(main.peak_bytes, Some(30720));
        assert_eq!(main.headroom_pct, Some(37.5));

        let pin_entry = parsed
            .stack_hwm
            .iter()
            .find(|s| s.label == "ui: navigate_to_pin_entry")
            .unwrap();
        assert_eq!(pin_entry.free_bytes, 15000);
        assert_eq!(pin_entry.total_bytes, None);
    }

    #[test]
    fn counts_delivery_lines() {
        let parsed = parse(SAMPLE_LOG);
        assert_eq!(parsed.delivery.tx_lines, 1);
        assert_eq!(parsed.delivery.ack_received, 1);
        assert_eq!(parsed.delivery.rx_done, 1);
        assert_eq!(parsed.delivery.rx_dm, 1);
        assert_eq!(parsed.delivery.rx_grp_txt, 1);
        assert_eq!(parsed.delivery.cad_busy, 1);
        assert_eq!(parsed.delivery.tx_retry, 1);
        assert_eq!(parsed.delivery.firmware_boots, 1);
        assert!(!parsed.delivery.looks_like_reset_mid_capture());
    }

    #[test]
    fn detects_a_reset_mid_capture() {
        let log = format!("{SAMPLE_LOG}\nfirmware build: a1b2c3d\nidentity ready: pub_hash=0x12, pubkey=deadbeef\n");
        let parsed = parse(&log);
        assert_eq!(parsed.delivery.firmware_boots, 2);
        assert!(parsed.delivery.looks_like_reset_mid_capture());
    }

    #[test]
    fn detects_diagnostics_not_compiled_when_every_always_on_phase_is_always_zero() {
        let broken_log = "\
PERF phase=gps: n=0 min=0 mean=0 max=0 p95=0
PERF phase=battery: n=0 min=0 mean=0 max=0 p95=0
PERF phase=rx_poll: n=0 min=0 mean=0 max=0 p95=0
";
        let parsed = parse(broken_log);
        assert!(parsed.looks_like_diagnostics_not_compiled());
    }

    #[test]
    fn a_correct_run_does_not_look_like_diagnostics_not_compiled() {
        let parsed = parse(SAMPLE_LOG);
        assert!(!parsed.looks_like_diagnostics_not_compiled());
    }

    #[test]
    fn empty_log_does_not_look_like_diagnostics_not_compiled() {
        // No windows at all is "nothing captured yet," not "diagnostics
        // missing" — the two failure modes need different operator advice.
        let parsed = parse("");
        assert!(!parsed.looks_like_diagnostics_not_compiled());
    }
}
