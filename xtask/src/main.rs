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
//! - **verify-font-table-counts** — each `gen_emoji_font.c` `#define N_*`
//!   matches its paired array's real element count (see
//!   `xtask::font_table_count_mismatches`'s doc).
//! - **verify-room-session-erase** — `admin_server.rs`'s `ADD_ROOM`/`DEL_ROOM`
//!   arms erase the room's dedicated NVS session store (see
//!   `xtask::room_session_erase`'s doc).
//! - **verify-reflood-cadence-decoupling** — the room keep-alive scheduler's
//!   re-flood-login branch gates on its own, backed-off cadence rather than
//!   the route-direct keep-alive's drain/routine one (see
//!   `xtask::room_reflood_cadence`'s doc).
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

    let count_mismatches =
        xtask::font_table_count_mismatches(&repo_root.join("firmware/gen_emoji_font.c"));
    if count_mismatches.is_empty() {
        println!(
            "xtask verify-font-table-counts: OK — every gen_emoji_font.c #define N_* matches its paired array's element count."
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-font-table-counts: FAILED — {} mismatch(es):",
            count_mismatches.len()
        );
        for m in &count_mismatches {
            eprintln!("  - {m}");
        }
    }

    let session_erase = xtask::room_session_erase::check(&repo_root);
    if session_erase.is_empty() {
        println!(
            "xtask verify-room-session-erase: OK — {}'s ADD_ROOM/DEL_ROOM arms erase the \
             dedicated room session store.",
            xtask::room_session_erase::ADMIN_SERVER_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-room-session-erase: FAILED — {} violation(s):",
            session_erase.len()
        );
        for v in &session_erase {
            eprintln!("  - {v}");
        }
    }

    let reflood_cadence = xtask::room_reflood_cadence::check(&repo_root);
    if reflood_cadence.is_empty() {
        println!(
            "xtask verify-reflood-cadence-decoupling: OK — {}'s re-flood-login branch stays on its own cadence.",
            xtask::room_reflood_cadence::MAIN_RS_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-reflood-cadence-decoupling: FAILED — {} violation(s):",
            reflood_cadence.len()
        );
        for v in &reflood_cadence {
            eprintln!("  - {v}");
        }
    }

    let aggregate_notification = xtask::room_aggregate_notification::check(&repo_root);
    if aggregate_notification.is_empty() {
        println!(
            "xtask verify-room-aggregate-notification: OK — {}'s `RoomNotification::Aggregate` \
             arm raises `UiEvent::RoomDrainComplete`.",
            xtask::room_aggregate_notification::MAIN_RS_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-room-aggregate-notification: FAILED — {} violation(s):",
            aggregate_notification.len()
        );
        for v in &aggregate_notification {
            eprintln!("  - {v}");
        }
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
