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
//! - **verify-emoji-render-subset** — every `protocol::emoji::EMOJI_TABLE`
//!   (picker) codepoint is present in `gen_emoji_font.c`'s renderable set,
//!   `EMOJI_CPS ∪ RENDER_EXTRA_CPS` (see `xtask::emoji_table_subset_mismatches`'s
//!   doc and the crate's module doc, "The picker/render split").
//! - **verify-room-session-erase** — `admin_server.rs`'s `ADD_ROOM`/`DEL_ROOM`
//!   arms erase the room's dedicated NVS session store (see
//!   `xtask::room_session_erase`'s doc).
//! - **verify-reflood-cadence-decoupling** — the room keep-alive scheduler's
//!   re-flood-login branch gates on its own, backed-off cadence rather than
//!   the route-direct keep-alive's drain/routine one (see
//!   `xtask::room_reflood_cadence`'s doc).
//! - **verify-room-post-history-timestamp** — the room-post send path's
//!   local outbound-echo history entry is sourced from the trusted wall
//!   clock, never the room's wire nonce (see
//!   `xtask::room_post_history_timestamp`'s doc).
//! - **verify-room-closer-failed-notification** — `main.rs`'s keep-alive-
//!   stall-invalidation call site of `note_closer_failed()` raises
//!   `UiEvent::RoomDrainComplete` whenever it returns `Some(Aggregate)` (see
//!   `xtask::room_aggregate_notification::check_closer_failed_wiring`'s doc).
//! - **verify-room-watermark-persist** — every
//!   `<room>.session.record_sent_timestamp(..)` call site in
//!   `firmware/src/main.rs`, enumerated by shape rather than a fixed list,
//!   is followed by a `save_room_session` for that same room (see
//!   `xtask::room_watermark_persist`'s doc).
//! - **verify-room-drain-window-periodic-reeval** — the room keep-alive
//!   scheduler's periodic, event-independent drain-window re-evaluation
//!   (`RoomSyncPhase::on_scheduler_tick`) is wired in unconditionally, before
//!   the re-flood branch, and raises `UiEvent::RoomDrainComplete` on a
//!   force-close (see `xtask::room_drain_window_periodic_reeval`'s doc).
//! - **verify-reflood-reset-requires-route** — `apply_room_login_outcome`
//!   only resets the reflood backoff epoch when a route is actually known,
//!   never on "a login reply arrived" alone (see
//!   `xtask::room_reflood_reset_requires_route`'s doc).
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

    let subset_mismatches =
        xtask::emoji_table_subset_mismatches(&repo_root.join("firmware/gen_emoji_font.c"));
    if subset_mismatches.is_empty() {
        println!(
            "xtask verify-emoji-render-subset: OK — every protocol::emoji::EMOJI_TABLE codepoint \
             is present in gen_emoji_font.c's EMOJI_CPS ∪ RENDER_EXTRA_CPS."
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-emoji-render-subset: FAILED — {} violation(s):",
            subset_mismatches.len()
        );
        for v in &subset_mismatches {
            eprintln!("  - {v}");
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

    let closer_failed_notification =
        xtask::room_aggregate_notification::check_closer_failed_wiring(&repo_root);
    if closer_failed_notification.is_empty() {
        println!(
            "xtask verify-room-closer-failed-notification: OK — {}'s `note_closer_failed()` \
             call site raises `UiEvent::RoomDrainComplete` when it returns `Some(Aggregate)`.",
            xtask::room_aggregate_notification::MAIN_RS_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-room-closer-failed-notification: FAILED — {} violation(s):",
            closer_failed_notification.len()
        );
        for v in &closer_failed_notification {
            eprintln!("  - {v}");
        }
    }

    let history_ts = xtask::room_post_history_timestamp::check(&repo_root);
    if history_ts.is_empty() {
        println!(
            "xtask verify-room-post-history-timestamp: OK — {}'s room-post local outbound echo \
             is sourced from the trusted wall clock, never the wire nonce.",
            xtask::room_post_history_timestamp::MAIN_RS_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-room-post-history-timestamp: FAILED — {} violation(s):",
            history_ts.len()
        );
        for v in &history_ts {
            eprintln!("  - {v}");
        }
    }

    let watermark_persist = xtask::room_watermark_persist::check(&repo_root);
    if watermark_persist.is_empty() {
        println!(
            "xtask verify-room-watermark-persist: OK — every `record_sent_timestamp` call site \
             in {} is followed by a `save_room_session` for the same room.",
            xtask::room_watermark_persist::MAIN_RS_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-room-watermark-persist: FAILED — {} violation(s):",
            watermark_persist.len()
        );
        for v in &watermark_persist {
            eprintln!("  - {v}");
        }
    }

    let periodic_reeval = xtask::room_drain_window_periodic_reeval::check(&repo_root);
    if periodic_reeval.is_empty() {
        println!(
            "xtask verify-room-drain-window-periodic-reeval: OK — {}'s scheduler loop \
             re-evaluates the drain-window stall bound on its own periodic tick and raises \
             `UiEvent::RoomDrainComplete` on a force-close.",
            xtask::room_drain_window_periodic_reeval::MAIN_RS_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-room-drain-window-periodic-reeval: FAILED — {} violation(s):",
            periodic_reeval.len()
        );
        for v in &periodic_reeval {
            eprintln!("  - {v}");
        }
    }

    let reflood_reset_route_gated = xtask::room_reflood_reset_requires_route::check(&repo_root);
    if reflood_reset_route_gated.is_empty() {
        println!(
            "xtask verify-reflood-reset-requires-route: OK — {}'s `apply_room_login_outcome` \
             only resets the reflood backoff epoch when a route is actually known.",
            xtask::room_reflood_reset_requires_route::MAIN_RS_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-reflood-reset-requires-route: FAILED — {} violation(s):",
            reflood_reset_route_gated.len()
        );
        for v in &reflood_reset_route_gated {
            eprintln!("  - {v}");
        }
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
