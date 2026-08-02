// SPDX-License-Identifier: GPL-3.0-only
//! Ingest one or more pasted `meshcadet-perf-report` blocks from a file:
//! parse, archive under `docs/perf/device-reports/`, and — for any
//! `section: calibration` block — print the `perf_loop_model` calibration
//! this run derives.
//!
//! ```sh
//! cargo run -p perf_device_report --bin ingest_device_report -- <path-to-report-text>
//! ```
//!
//! This binary touches no serial device — its only input is a text file a
//! human already produced by pasting a collection-kit report-back block
//! (see `docs/perf/collection-kit.md` §9). By default it archives into
//! `docs/perf/device-reports/` relative to the current directory (run this
//! from the repo root); pass a second argument to archive somewhere else
//! (used by this tool's own tests, and useful for a dry run).

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use perf_device_report::archive::{archive_block, DEFAULT_ARCHIVE_DIR};
use perf_device_report::calibration::measured_constants_from_log;
use perf_device_report::perf_log;
use perf_device_report::report_block::{parse_report_blocks, Section};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some(input_path) = args.get(1) else {
        eprintln!(
            "usage: ingest_device_report <path-to-report-text> [archive-dir, default: {DEFAULT_ARCHIVE_DIR}]"
        );
        return ExitCode::FAILURE;
    };
    let archive_dir = PathBuf::from(
        args.get(2)
            .map(String::as_str)
            .unwrap_or(DEFAULT_ARCHIVE_DIR),
    );

    let text = match fs::read_to_string(input_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: could not read {input_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let blocks = parse_report_blocks(&text);
    if blocks.is_empty() {
        eprintln!("no `meshcadet-perf-report` block found in {input_path}");
        return ExitCode::FAILURE;
    }

    let mut had_error = false;
    for (i, result) in blocks.into_iter().enumerate() {
        let block = match result {
            Ok(b) => b,
            Err(e) => {
                eprintln!("block #{}: parse error: {e}", i + 1);
                had_error = true;
                continue;
            }
        };

        let log = perf_log::parse(&block.raw_serial_log);
        if log.looks_like_diagnostics_not_compiled() {
            eprintln!(
                "block #{} (build_ref={}): WARNING — gps/battery/rx_poll are all n=0 in every \
                 window; per collection-kit.md Part C this usually means the `diagnostics` \
                 feature did not actually compile in, not a real zero cost. Archiving anyway.",
                i + 1,
                block.build_ref
            );
        }
        if log.delivery.looks_like_reset_mid_capture() {
            eprintln!(
                "block #{} (build_ref={}): WARNING — {} `firmware build:` lines seen; per \
                 collection-kit.md Part C this means the device reset mid-capture. Archiving \
                 anyway.",
                i + 1,
                block.build_ref,
                log.delivery.firmware_boots
            );
        }

        match archive_block(&archive_dir, &block) {
            Ok(path) => println!(
                "block #{}: archived build_ref={} section={} -> {}",
                i + 1,
                block.build_ref,
                block.section,
                path.display()
            ),
            Err(e) => {
                eprintln!("block #{}: failed to archive: {e}", i + 1);
                had_error = true;
                continue;
            }
        }

        if block.section == Section::Calibration {
            let measured = measured_constants_from_log(&log);
            let (_calibrated, report) = perf_loop_model::calibrate(
                perf_loop_model::LoopModelParams::documented_defaults(),
                &measured,
            );
            println!(
                "  calibration (build_ref={}): ui_step={:?} cad_spi_overhead={:?} gps_poll={:?} \
                 battery_poll={:?}",
                block.build_ref,
                report.ui_step,
                report.cad_spi_overhead,
                report.gps_poll,
                report.battery_poll,
            );
            if !report.fully_calibrated() {
                println!(
                    "  NOTE: not every calibratable field had a usable window in this capture — \
                     the un-calibrated fields above stay at their documented SIMULATED range. \
                     Re-run this tool once a report covers them, or leave them ranged per Part \
                     D's own table (some fields are 'not directly instrumented' and never close \
                     through this hook)."
                );
            }
        }
    }

    if had_error {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
