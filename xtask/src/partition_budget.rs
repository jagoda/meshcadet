// SPDX-License-Identifier: GPL-3.0-only
//! Recomputes the actual firmware app-image size from a FRESH release build
//! and diffs it against the committed baseline
//! (`firmware/app-image-budget-baseline.txt`), instead of trusting a
//! hand-written "measured" figure frozen in a `firmware/partitions.csv`
//! comment.
//!
//! # The defect this closes
//!
//! `firmware/partitions.csv` used to carry a comment — "espflash reports
//! 2,359,440 bytes ... ~3.75 MB headroom (measured, see above)" — that had
//! no recompute trigger. It silently decayed 2.72 MB stale as the tree
//! grew, and an entire multi-phase campaign plan
//! (`meshcadet-emoji-coverage`) budgeted every decision against it before a
//! checkpoint's own direct measurement caught the drift (see
//! `meshcadet-emoji-font-upgrade-checkpoint-20260802-140922469`). A
//! "measured" figure in a comment reads as trustworthy precisely because it
//! claims to be derived, and nothing about a prose comment fires when the
//! underlying quantity moves — a mechanical recompute-and-diff is what
//! actually catches that.
//!
//! # Why this is NOT part of the default `xtask` battery or `cargo test`
//!
//! Every other check in this crate (`verify-glyphs`,
//! `verify-font-table-counts`, `room_session_erase`, ...) is a plain-text
//! scan over already-checked-out source — no extra toolchain required,
//! cheap enough to run on every `cargo test --workspace` in the fast host
//! lane (`.github/workflows/ci.yml`'s `test` job). This check is
//! different: it runs an actual `cargo build --release` for
//! `xtensa-esp32s3-espidf` plus an `esptool elf2image` pass
//! (`firmware/scripts/measure-app-image-size.sh`), which requires the `esp`
//! rustup toolchain + ESP-IDF sysroot to be bootstrapped and takes minutes,
//! not milliseconds. Folding it into the default battery would break every
//! plain `cargo run -p xtask --bin xtask` / `cargo test --workspace`
//! invocation on a machine without that toolchain (which is most of them —
//! see `firmware/check-all-features.sh`'s own prerequisites comment).
//!
//! Invoke it explicitly instead:
//!
//! ```sh
//! cargo run -p xtask --bin xtask -- verify-partition-budget
//! ```
//!
//! This is wired into `.github/workflows/ci.yml`'s `firmware` job (which
//! already bootstraps the `esp` toolchain for `check-all-features.sh`) as
//! its own step, and CONTRIBUTING.md's "Flash-budget changes" section
//! documents it as a required pre-campaign step for any plan that budgets
//! flash headroom.
//!
//! # What "drift" means here
//!
//! The committed baseline (`firmware/app-image-budget-baseline.txt`) is not
//! a target to hit — it is last known-good measurement, bumped
//! deliberately whenever a real, intentional change moves the app image
//! (new UI assets, new glyph coverage, a dependency upgrade). A drift past
//! [`DRIFT_THRESHOLD_PCT`] in *either* direction fails loudly: growth means
//! the budget assumption downstream campaigns plan against just moved
//! (exactly the incident this guard exists to catch); shrinkage past the
//! threshold is equally worth a human look (did an asset silently stop
//! being embedded?).

use std::path::Path;
use std::process::Command;

/// The committed baseline this check diffs the fresh measurement against.
pub const BASELINE_REL_PATH: &str = "firmware/app-image-budget-baseline.txt";

/// Where the `factory` partition's real size (the actual budget ceiling)
/// comes from — parsed from partition-table CSV data, never from a comment.
pub const PARTITIONS_CSV_REL_PATH: &str = "firmware/partitions.csv";

/// The script that does the actual build + measurement (see its own header
/// doc for the full rationale).
pub const MEASURE_SCRIPT_REL_PATH: &str = "firmware/scripts/measure-app-image-size.sh";

/// Fail past this much drift, in percent, in either direction.
pub const DRIFT_THRESHOLD_PCT: f64 = 5.0;

/// Result of a `verify-partition-budget` run: a freshly measured app-image
/// size, diffed against the committed baseline and the partition's actual
/// capacity.
#[derive(Debug)]
pub struct Report {
    pub measured_bytes: u64,
    pub baseline_bytes: u64,
    pub drift_pct: f64,
    pub factory_partition_bytes: u64,
    pub headroom_bytes: i64,
    pub over_threshold: bool,
}

impl Report {
    /// Human-readable summary, independent of pass/fail — always report the
    /// real numbers, not just a verdict (same "fails loud, reports fully"
    /// spirit as this crate's other checks).
    pub fn summary(&self) -> String {
        format!(
            "measured {measured} B ({measured_mib:.2} MiB) vs. baseline {baseline} B \
             ({baseline_mib:.2} MiB) — drift {drift:+.2}% (threshold ±{threshold:.1}%). \
             factory partition {factory} B ({factory_mib:.2} MiB); headroom {headroom} B \
             ({headroom_mib:.2} MiB).",
            measured = self.measured_bytes,
            measured_mib = mib(self.measured_bytes as f64),
            baseline = self.baseline_bytes,
            baseline_mib = mib(self.baseline_bytes as f64),
            drift = self.drift_pct,
            threshold = DRIFT_THRESHOLD_PCT,
            factory = self.factory_partition_bytes,
            factory_mib = mib(self.factory_partition_bytes as f64),
            headroom = self.headroom_bytes,
            headroom_mib = mib(self.headroom_bytes as f64),
        )
    }
}

fn mib(bytes: f64) -> f64 {
    bytes / (1024.0 * 1024.0)
}

/// Runs the measurement script, reads the committed baseline and the
/// `factory` partition's real capacity, and computes drift.
///
/// Returns `Err` for an OPERATIONAL failure (build didn't run, baseline
/// file missing/malformed, `partitions.csv` unparseable) — never for "the
/// budget drifted", which is a valid, fully-reported `Ok(Report)` with
/// `over_threshold: true`.
pub fn check(repo_root: &Path) -> Result<Report, String> {
    let measured_bytes = measure_app_image_size(repo_root)?;
    let baseline_bytes = read_baseline(repo_root)?;
    let factory_partition_bytes = read_factory_partition_size(repo_root)?;

    if baseline_bytes == 0 {
        return Err(format!(
            "{BASELINE_REL_PATH}: baseline is 0 bytes — cannot compute drift"
        ));
    }

    Ok(compute_report(
        measured_bytes,
        baseline_bytes,
        factory_partition_bytes,
    ))
}

/// The pure arithmetic half of [`check`] — split out from the I/O (build
/// subprocess, file reads) so the drift/threshold/headroom math is
/// unit-testable without the `esp` cross-toolchain `check`'s callers
/// require. Caller must have already ruled out `baseline_bytes == 0`.
fn compute_report(
    measured_bytes: u64,
    baseline_bytes: u64,
    factory_partition_bytes: u64,
) -> Report {
    let drift_pct =
        ((measured_bytes as f64 - baseline_bytes as f64) / baseline_bytes as f64) * 100.0;
    let headroom_bytes = factory_partition_bytes as i64 - measured_bytes as i64;

    Report {
        measured_bytes,
        baseline_bytes,
        drift_pct,
        factory_partition_bytes,
        headroom_bytes,
        over_threshold: drift_pct.abs() > DRIFT_THRESHOLD_PCT,
    }
}

fn measure_app_image_size(repo_root: &Path) -> Result<u64, String> {
    let script = repo_root.join(MEASURE_SCRIPT_REL_PATH);
    let output = Command::new("bash")
        .arg(&script)
        .current_dir(repo_root.join("firmware"))
        .output()
        .map_err(|e| format!("failed to run {}: {e}", script.display()))?;

    if !output.status.success() {
        return Err(format!(
            "{} exited with {}:\n{}",
            script.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let last_line = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or_else(|| format!("{}: produced no stdout output", script.display()))?;
    last_line.parse::<u64>().map_err(|e| {
        format!(
            "{}: could not parse final stdout line '{last_line}' as a byte count: {e}",
            script.display()
        )
    })
}

fn read_baseline(repo_root: &Path) -> Result<u64, String> {
    let path = repo_root.join(BASELINE_REL_PATH);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .ok_or_else(|| format!("{}: no non-comment baseline line found", path.display()))?;
    line.parse::<u64>().map_err(|e| {
        format!(
            "{}: could not parse '{line}' as a byte count: {e}",
            path.display()
        )
    })
}

/// Reads the `factory` partition's Size field directly from
/// `firmware/partitions.csv`'s DATA row — never from the file's prose
/// comments, which is exactly the trust-without-recompute failure mode this
/// whole check exists to close.
fn read_factory_partition_size(repo_root: &Path) -> Result<u64, String> {
    let path = repo_root.join(PARTITIONS_CSV_REL_PATH);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;

    for line in content.lines() {
        let trimmed = line.trim();
        // Same comment convention gen_esp32part.py itself uses (and this
        // file's own header documents): '#' as the first non-whitespace
        // character marks a comment line.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = trimmed.split(',').map(str::trim).collect();
        if fields.first() == Some(&"factory") {
            let size_field = fields.get(4).ok_or_else(|| {
                format!(
                    "{}: `factory` row has no Size field: {line}",
                    path.display()
                )
            })?;
            return parse_csv_size(size_field).ok_or_else(|| {
                format!(
                    "{}: could not parse `factory` Size field '{size_field}'",
                    path.display()
                )
            });
        }
    }

    Err(format!("{}: no `factory` row found", path.display()))
}

fn parse_csv_size(field: &str) -> Option<u64> {
    let f = field.trim();
    if let Some(hex) = f.strip_prefix("0x").or_else(|| f.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        f.parse::<u64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_and_decimal_csv_sizes() {
        assert_eq!(parse_csv_size("0x600000"), Some(0x600000));
        assert_eq!(parse_csv_size("6291456"), Some(6291456));
        assert_eq!(parse_csv_size("  0x600000  "), Some(0x600000));
        assert_eq!(parse_csv_size("not-a-size"), None);
    }

    #[test]
    fn reads_factory_partition_size_from_real_partitions_csv() {
        // Exercises the parser against the actual committed file — proves
        // it reads the DATA row, not the comment prose above it (which
        // documents a different, and deliberately stale-until-corrected,
        // set of numbers in its own text).
        let repo_root = crate::repo_root_from_manifest_dir();
        let bytes = read_factory_partition_size(&repo_root)
            .expect("firmware/partitions.csv must have a parseable `factory` row");
        assert_eq!(bytes, 0x600000, "factory partition is documented as 6 MB");
    }

    #[test]
    fn reads_committed_baseline() {
        let repo_root = crate::repo_root_from_manifest_dir();
        let baseline =
            read_baseline(&repo_root).expect("firmware/app-image-budget-baseline.txt must parse");
        assert!(baseline > 0, "baseline must be a positive byte count");
    }

    #[test]
    fn zero_drift_is_not_over_threshold() {
        let report = compute_report(1_000_000, 1_000_000, 6_000_000);
        assert_eq!(report.drift_pct, 0.0);
        assert!(!report.over_threshold);
        assert_eq!(report.headroom_bytes, 5_000_000);
    }

    #[test]
    fn drift_exactly_at_threshold_is_not_over() {
        // Boundary is strict (`> DRIFT_THRESHOLD_PCT`), not `>=` — a measurement
        // that lands exactly on the threshold must still pass.
        let baseline = 1_000_000u64;
        let measured = baseline + (baseline as f64 * DRIFT_THRESHOLD_PCT / 100.0) as u64;
        let report = compute_report(measured, baseline, 6_000_000);
        assert!(
            !report.over_threshold,
            "drift {} at exactly the {}% threshold must not fail",
            report.drift_pct, DRIFT_THRESHOLD_PCT
        );
    }

    #[test]
    fn growth_past_threshold_fails() {
        // +6% growth — past the 5% threshold — is exactly the incident this
        // guard exists to catch (a stale-low baseline masking real growth).
        let report = compute_report(1_060_000, 1_000_000, 6_000_000);
        assert!(report.over_threshold);
        assert!(report.drift_pct > DRIFT_THRESHOLD_PCT);
    }

    #[test]
    fn shrinkage_past_threshold_also_fails() {
        // Drift is checked in BOTH directions — an app image that shrank
        // more than the threshold is equally worth a human look (did an
        // asset silently stop being embedded?).
        let report = compute_report(900_000, 1_000_000, 6_000_000);
        assert!(report.over_threshold);
        assert!(report.drift_pct < -DRIFT_THRESHOLD_PCT);
    }

    #[test]
    fn headroom_can_go_negative_when_over_partition_capacity() {
        // The `factory` partition is a hard ceiling — if a measured image
        // ever exceeds it, headroom must report negative, not saturate at
        // zero (a saturating value would silently hide "this won't flash").
        let report = compute_report(6_500_000, 6_500_000, 6_000_000);
        assert_eq!(report.headroom_bytes, -500_000);
    }
}
