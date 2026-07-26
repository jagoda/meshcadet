// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p xtask --bin xtask` — human-runnable entry point for the
//! host-side static
//! checks over `firmware/`, a crate whose own `#[cfg(test)]` blocks are
//! type-checked but never executed (`harness = false`):
//!
//! - **verify-glyphs** — every (codepoint, font-size) pair used in a screen's
//!   Slint literal is registered and rasterised (see `xtask::check`'s doc).
//! - **verify-ui-event-parity** — the room-post notification-surface contract
//!   in `UiRuntime::handle_event` (see `xtask::ui_event_parity`'s doc).
//!
//! Both also run as `cargo test`s, which is what CI / every downstream change
//! actually gates on; this binary exists for a quick manual re-check with a
//! human-readable report and a nonzero exit code on failure. It runs BOTH
//! checks and reports both before exiting, rather than short-circuiting on
//! the first — a manual re-check should surface everything in one pass.

use std::process::ExitCode;

fn main() -> ExitCode {
    let repo_root = xtask::repo_root_from_manifest_dir();
    let mut ok = true;

    let glyph = xtask::check(&repo_root);
    if glyph.is_empty() {
        println!("xtask verify-glyphs: OK — every (codepoint, size) used in firmware/src/ui/screens/*.rs is covered.");
    } else {
        ok = false;
        eprintln!(
            "xtask verify-glyphs: FAILED — {} violation(s):",
            glyph.len()
        );
        for v in &glyph {
            eprintln!("  - {v}");
        }
    }

    let parity = xtask::ui_event_parity::check(&repo_root);
    if parity.is_empty() {
        println!(
            "xtask verify-ui-event-parity: OK — {}'s room notification-surface contract holds.",
            xtask::ui_event_parity::UI_MOD_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-ui-event-parity: FAILED — {} violation(s):",
            parity.len()
        );
        for v in &parity {
            eprintln!("  - {v}");
        }
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
