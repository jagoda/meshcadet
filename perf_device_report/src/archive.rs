// SPDX-License-Identifier: GPL-3.0-only
//! Archives a parsed [`crate::report_block::ReportBlock`] under
//! `docs/perf/device-reports/` — see that directory's own `README.md` for
//! the schema this module implements. One archived file per report block,
//! named for build ref / section / payload / ui-load / date so a later
//! reader (or a later ingest run re-deriving a calibration) can find the
//! right one without re-parsing every file in the directory.
//!
//! This module is pure text-formatting + `std::fs` — it takes a directory
//! `Path` rather than assuming `docs/perf/device-reports/` itself, so its
//! tests run against a `tempfile`-free scratch directory (`std::env::
//! temp_dir()` + a unique suffix) instead of ever touching the real repo
//! tree.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::report_block::ReportBlock;

/// The default archive location, relative to the repo root — the
/// `docs/perf/` directory this crate's own `Cargo.toml` description and
/// `docs/perf/device-reports/README.md` both cite.
pub const DEFAULT_ARCHIVE_DIR: &str = "docs/perf/device-reports";

/// The archived filename for `block` — `<archive_stem>.md`.
pub fn archive_filename(block: &ReportBlock) -> String {
    format!("{}.md", block.archive_stem())
}

/// Render `block` back into its exact report-back shape, with one
/// provenance header line on top recording when/what archived it. The
/// body is otherwise byte-identical to what a correct §9 paste looks like,
/// so a human can eyeball an archived file next to the original paste.
pub fn render_archive_entry(block: &ReportBlock) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "<!-- archived by perf_device_report; MEASURED (device, {}, {}) -->\n",
        block.build_ref, block.capture_date
    ));
    out.push_str("```meshcadet-perf-report\n");
    out.push_str(&format!("kit_version: {}\n", block.kit_version));
    out.push_str(&format!("build_ref: {}\n", block.build_ref));
    out.push_str(&format!("capture_date: {}\n", block.capture_date));
    out.push_str(&format!("section: {}\n", block.section));
    out.push_str(&format!("payload_bytes: {}\n", payload_bytes_field(block)));
    out.push_str(&format!("ui_load: {}\n", ui_load_field(block)));
    out.push_str(&format!(
        "peer_present: {}\n",
        if block.peer_present { "yes" } else { "no" }
    ));
    out.push_str(&format!(
        "notes: {}\n",
        block.notes.as_deref().unwrap_or("")
    ));
    out.push_str("--- raw-serial-log ---\n");
    out.push_str(&block.raw_serial_log);
    out.push('\n');
    out.push_str("--- end-raw-serial-log ---\n```\n");
    out
}

fn ui_load_field(block: &ReportBlock) -> &'static str {
    use crate::report_block::UiLoad;
    match block.ui_load {
        UiLoad::Idle => "idle",
        UiLoad::Navigating => "navigating",
        UiLoad::NotApplicable => "n/a",
    }
}

/// The header/index text for `payload_bytes` — `n/a` or the bare integer,
/// the same shape §9 itself uses (NOT [`crate::report_block::ReportBlock::
/// archive_stem`]'s compact `10B`/`na` filename shape — that one needs to
/// be filesystem-friendly and unambiguous in a `--`-joined stem; this one
/// needs to round-trip back through [`crate::report_block::parse_report_
/// blocks`]).
fn payload_bytes_field(block: &ReportBlock) -> String {
    block
        .payload_bytes
        .map(|n| n.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

/// Write `block`'s archive entry into `dir` (creating it if needed) and
/// append one row to `dir/INDEX.md` (creating that with a header row if
/// it doesn't exist yet). Returns the path of the file just written.
///
/// Archiving the SAME `(build_ref, section, payload_bytes, ui_load,
/// capture_date)` tuple twice overwrites the archived report file (a
/// re-ingest of a corrected paste is expected to replace, not duplicate)
/// but still appends a fresh `INDEX.md` row — a human skimming the index
/// sees every ingest event, even a re-ingest of the same run.
pub fn archive_block(dir: &Path, block: &ReportBlock) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(archive_filename(block));
    fs::write(&path, render_archive_entry(block))?;
    append_index_row(dir, block)?;
    Ok(path)
}

const INDEX_HEADER: &str = "\
# Device report archive index

One row per `perf_device_report` ingest run. Generated/appended
mechanically — do not hand-edit rows, only the text above this table.

| build_ref | section | payload_bytes | ui_load | capture_date | closes | file |
|---|---|---|---|---|---|---|
";

fn append_index_row(dir: &Path, block: &ReportBlock) -> io::Result<()> {
    let index_path = dir.join("INDEX.md");
    if !index_path.exists() {
        fs::write(&index_path, INDEX_HEADER)?;
    }
    let row = format!(
        "| {} | {} | {} | {} | {} | {} | {} |\n",
        block.build_ref,
        block.section,
        payload_bytes_field(block),
        ui_load_field(block),
        block.capture_date,
        block.section.closes_predicates().join(", "),
        archive_filename(block),
    );
    let mut existing = fs::read_to_string(&index_path)?;
    existing.push_str(&row);
    fs::write(&index_path, existing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report_block::parse_report_blocks;
    use std::sync::atomic::{AtomicU64, Ordering};

    const SAMPLE: &str = "\
```meshcadet-perf-report
kit_version: 1
build_ref: a1b2c3d
capture_date: 2026-08-02
section: baseline
payload_bytes: n/a
ui_load: n/a
peer_present: no
notes: synthetic fixture, not a real capture
--- raw-serial-log ---
PERF phase=gps: n=1 min=10 mean=10 max=10 p95=10
--- end-raw-serial-log ---
```
";

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A fresh scratch directory per test, never the real repo tree.
    fn scratch_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "perf_device_report-archive-test-{}-{}",
            std::process::id(),
            n
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn sample_block() -> ReportBlock {
        parse_report_blocks(SAMPLE)[0].clone().unwrap()
    }

    #[test]
    fn archive_filename_matches_the_report_blocks_archive_stem() {
        let block = sample_block();
        assert_eq!(
            archive_filename(&block),
            format!("{}.md", block.archive_stem())
        );
    }

    #[test]
    fn render_archive_entry_round_trips_through_the_parser() {
        let block = sample_block();
        let rendered = render_archive_entry(&block);
        let reparsed = parse_report_blocks(&rendered);
        assert_eq!(reparsed.len(), 1);
        let reparsed_block = reparsed[0].clone().expect("archived entry should re-parse");
        assert_eq!(reparsed_block, block);
    }

    #[test]
    fn archive_block_writes_the_file_and_creates_the_index() {
        let dir = scratch_dir();
        let block = sample_block();
        let path = archive_block(&dir, &block).unwrap();
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap(), archive_filename(&block).as_str());

        let index = fs::read_to_string(dir.join("INDEX.md")).unwrap();
        assert!(index.contains("a1b2c3d"));
        assert!(index.contains("baseline"));
        assert!(index.contains("D1"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn archiving_twice_overwrites_the_file_but_appends_two_index_rows() {
        let dir = scratch_dir();
        let block = sample_block();
        archive_block(&dir, &block).unwrap();
        archive_block(&dir, &block).unwrap();

        let index = fs::read_to_string(dir.join("INDEX.md")).unwrap();
        let row_count = index
            .lines()
            .filter(|line| line.starts_with("| a1b2c3d "))
            .count();
        assert_eq!(row_count, 2);

        // Still exactly one archived .md file for this stem (plus INDEX.md).
        let md_files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            .collect();
        assert_eq!(md_files.len(), 2); // the archived report + INDEX.md

        fs::remove_dir_all(&dir).ok();
    }
}
