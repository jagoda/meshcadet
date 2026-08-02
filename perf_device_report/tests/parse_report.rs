// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end tests over `tests/fixtures/*.md` — SYNTHETIC report-back text
//! (never a real device capture, see each fixture's own header comment)
//! that exercises the full parse -> archive / calibrate pipeline the way
//! `src/bin/ingest_device_report.rs` drives it, without touching the real
//! `docs/perf/device-reports/` directory.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use perf_device_report::archive::archive_block;
use perf_device_report::calibration::measured_constants_from_log;
use perf_device_report::perf_log;
use perf_device_report::report_block::{parse_report_blocks, Section, UiLoad};

const BASELINE: &str = include_str!("fixtures/sample-baseline-report.md");
const CALIBRATION: &str = include_str!("fixtures/sample-calibration-report.md");
const TWO_DEVICE_SWEEP: &str = include_str!("fixtures/sample-two-device-delivery-sweep.md");

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dir() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "perf_device_report-integration-test-{}-{}",
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

#[test]
fn baseline_fixture_parses_and_reports_two_ui_step_windows() {
    let results = parse_report_blocks(BASELINE);
    assert_eq!(results.len(), 1);
    let block = results[0].clone().expect("fixture should parse cleanly");
    assert_eq!(block.section, Section::Baseline);
    assert_eq!(block.build_ref, "fixture01");

    let log = perf_log::parse(&block.raw_serial_log);
    assert!(!log.looks_like_diagnostics_not_compiled());
    assert!(!log.delivery.looks_like_reset_mid_capture());

    let ui_step_windows = log.phase_windows("ui_step");
    assert_eq!(ui_step_windows.len(), 2);
    // The navigation window's max is visibly higher than the idle window's
    // — collection-kit.md Part C step 2's own expectation, and exactly why
    // the parser exposes every window rather than collapsing them.
    assert!(ui_step_windows[1].max > ui_step_windows[0].max);
}

#[test]
fn baseline_fixture_archives_cleanly_into_a_scratch_dir() {
    let dir = scratch_dir();
    let block = parse_report_blocks(BASELINE)[0].clone().unwrap();
    let path = archive_block(&dir, &block).unwrap();
    assert!(path.exists());
    let archived_text = fs::read_to_string(&path).unwrap();
    // The archived copy must itself re-parse as the identical block.
    let reparsed = parse_report_blocks(&archived_text);
    assert_eq!(reparsed.len(), 1);
    assert_eq!(reparsed[0].as_ref().unwrap(), &block);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn calibration_fixture_derives_a_usable_measured_constants_set() {
    let block = parse_report_blocks(CALIBRATION)[0].clone().unwrap();
    assert_eq!(block.section, Section::Calibration);

    let log = perf_log::parse(&block.raw_serial_log);
    let measured = measured_constants_from_log(&log);

    assert!(measured.ui_step.is_some());
    assert!(measured.gps_poll.is_some());
    assert!(measured.battery_poll.is_some());
    assert!(measured.cad_spi_overhead_ms.is_some());

    let (calibrated, report) = perf_loop_model::calibrate(
        perf_loop_model::LoopModelParams::documented_defaults(),
        &measured,
    );
    assert!(report.fully_calibrated());

    // The calibrated params must still simulate cleanly through the
    // existing perf_loop_model report machinery — this is the "loop model
    // exposes a hook" contract exercised end to end, not just type-checked.
    let text = perf_loop_model::report::render_text_report_with_params(&calibrated);
    assert!(text.contains("SIMULATED"));
}

#[test]
fn two_device_sweep_fixture_parses_both_blocks_and_shows_the_idle_vs_navigating_delta() {
    let results = parse_report_blocks(TWO_DEVICE_SWEEP);
    assert_eq!(results.len(), 2);
    let idle = results[0].clone().unwrap();
    let navigating = results[1].clone().unwrap();
    assert_eq!(idle.ui_load, UiLoad::Idle);
    assert_eq!(navigating.ui_load, UiLoad::Navigating);
    assert_eq!(idle.payload_bytes, navigating.payload_bytes);

    let idle_log = perf_log::parse(&idle.raw_serial_log);
    let nav_log = perf_log::parse(&navigating.raw_serial_log);

    let idle_rx_notice = idle_log
        .rollups
        .iter()
        .find(|r| r.label == "rx-notice-latency")
        .unwrap();
    let nav_rx_notice = nav_log
        .rollups
        .iter()
        .find(|r| r.label == "rx-notice-latency")
        .unwrap();
    // D6's own headline comparison: RX-notice latency, idle vs. UI-active.
    assert!(nav_rx_notice.mean > idle_rx_notice.mean);

    // D4's headline number: ui-starvation `longest` under load.
    assert_eq!(idle_log.ui_starvation[0].longest_ms, 0.0);
    assert!(nav_log.ui_starvation[0].longest_ms > 0.0);
}

#[test]
fn archiving_the_two_device_sweep_produces_two_distinct_files() {
    let dir = scratch_dir();
    let results = parse_report_blocks(TWO_DEVICE_SWEEP);
    let mut paths = Vec::new();
    for r in results {
        let block = r.unwrap();
        paths.push(archive_block(&dir, &block).unwrap());
    }
    assert_eq!(paths.len(), 2);
    assert_ne!(paths[0], paths[1]);

    let index = fs::read_to_string(dir.join("INDEX.md")).unwrap();
    let row_count = index
        .lines()
        .filter(|line| line.starts_with("| fixture03 "))
        .count();
    assert_eq!(row_count, 2);
    fs::remove_dir_all(&dir).ok();
}
