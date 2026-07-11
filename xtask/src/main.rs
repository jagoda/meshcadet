// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p xtask -- verify-glyphs` — human-runnable entry point for the
//! glyph-coverage harness (see `xtask::check`'s doc for the full design).
//! The same check also runs as a `cargo test` (`xtask::tests::
//! glyph_coverage_is_complete`), which is what CI / every downstream change
//! actually gates on; this binary exists for a quick manual re-check with a
//! human-readable report and a nonzero exit code on failure.

use std::process::ExitCode;

fn main() -> ExitCode {
    let repo_root = xtask::repo_root_from_manifest_dir();
    let violations = xtask::check(&repo_root);
    if violations.is_empty() {
        println!("xtask verify-glyphs: OK — every (codepoint, size) used in firmware/src/ui/screens/*.rs is covered.");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "xtask verify-glyphs: FAILED — {} violation(s):",
            violations.len()
        );
        for v in &violations {
            eprintln!("  - {v}");
        }
        ExitCode::FAILURE
    }
}
