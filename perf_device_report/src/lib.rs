// SPDX-License-Identifier: GPL-3.0-only
//! Ingest path for `docs/perf/collection-kit.md` §9's report-back format —
//! the child of `meshcadet-perf-rearchitecture`'s campaign this crate was
//! built for is `meshcadet-perf-device-report-ingest`.
//!
//! # What a report-back block is
//!
//! A human operator runs the collection kit against a flashed T-Deck Plus
//! (never this container — no HIL, no serial device, ever touches this
//! crate) and pastes back one or more `meshcadet-perf-report` blocks: a
//! small header (build ref, capture date, which kit section, payload/UI-
//! load axis) followed by the raw, unmodified serial console capture. See
//! [`report_block`]'s module doc for the exact shape.
//!
//! # The three things this crate does
//!
//! 1. **Parse** ([`report_block::parse_report_blocks`]) the header +
//!    delimited raw-log shape into a [`report_block::ReportBlock`], and
//!    ([`perf_log::parse`]) the `PERF ...` / stack-HWM / TX-RX-CAD lines
//!    inside the raw log into structured [`perf_log::ParsedLog`] data.
//! 2. **Archive** ([`archive::archive_block`]) a parsed block under
//!    `docs/perf/device-reports/` — see that directory's `README.md` for
//!    the schema, and `archive`'s module doc for the file-naming and
//!    index-appending rules.
//! 3. **Calibrate** ([`calibration::measured_constants_from_log`]) — for a
//!    `section: calibration` block, derive the four
//!    `perf_loop_model::calibration::MeasuredConstants` fields
//!    `docs/perf/collection-kit.md` Part D's table specifies, ready to
//!    hand to `perf_loop_model::calibrate`.
//!
//! # What this crate deliberately does NOT do
//!
//! - **No device I/O of any kind.** No serial port, no `espflash`, no
//!   `/dev/ttyACM0` — this crate's whole job starts AFTER a human has
//!   already pasted a capture back as text, per the campaign's 2026-08-02
//!   no-HIL ruling.
//! - **No inventing numbers.** Every parser function returns `None`/an
//!   error for a field it cannot derive from what's actually in the text —
//!   there is no fallback-to-zero or fallback-to-estimate path anywhere in
//!   this crate. See [`perf_log::ParsedLog::latest_phase_window`] and
//!   [`calibration::measured_constants_from_log`]'s own docs for exactly
//!   which cases return `None`.
//! - **No MEASURED/SIMULATED mislabeling.** [`archive::render_archive_
//!   entry`] tags an archived block MEASURED (it IS a device reading);
//!   `perf_loop_model::report::render_text_report_with_params`'s own doc
//!   is explicit that feeding a MEASURED point into that crate's simulator
//!   does NOT make the simulator's OUTPUT numbers measured — this crate
//!   does not paper over that distinction anywhere it touches text a human
//!   might paste into a doc.
//!
//! # Host-testable, no device required
//!
//! Every test in this crate runs against synthetic fixture text (`tests/
//! fixtures/*.md`, or literal strings in each module's own `#[cfg(test)]`
//! block) — never a real device capture. `cargo test -p perf_device_report`
//! runs on the host, in CI, with no T-Deck attached, same as every other
//! host-workspace member.

pub mod archive;
pub mod calibration;
pub mod perf_log;
pub mod report_block;
