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
//! - **verify-lock-gate** — `UiRuntime::step()`'s keyboard/trackball input
//!   blocks check `self.locked` before any branch that reads
//!   `self.active_screen` directly, so the screen-lock overlay's retained
//!   underlying screen never leaks input while locked (see
//!   `xtask::lock_gate`'s doc).
//! - **verify-lock-integrity-fixes** — deep-review pass 1's F2 (`trip_lock`
//!   fails CLOSED on a `LockScreen::new()` construction failure) and F1
//!   (`FRAME_SET_LOCK_PIN` live-forwards a `UiEvent::LockPinChanged` to the
//!   UI thread instead of only taking effect at the next boot) — see
//!   `xtask::lock_integrity_fixes`'s doc.
//! - **verify-power-provenance** — ADR-0014 D2's estimate-labelling rule,
//!   mechanized: every power-current figure (`mA`/`µA`/`uA`) anywhere under
//!   `docs/` carries one of the three D2 provenance tags
//!   (`[DATASHEET]`/`[ESTIMATE]`/`[MEASURED]`) nearby (see
//!   `xtask::power_provenance`'s doc).
//! - **verify-render-asleep-gate** — `UiRuntime::step()`'s render section
//!   never reaches `render_if_needed` without `render_gate(self.screen_asleep)`
//!   guarding it first (meshcadet-power-optimization Phase 5 — see
//!   `xtask::render_asleep_gate`'s doc).
//! - **verify-pm-apb-lock-gate** — every SPI2 transaction the radio driver
//!   issues, and the GPS driver's whole UART ACTIVE window, is bracketed by
//!   an `ESP_PM_APB_FREQ_MAX` lock acquire/release pair (meshcadet-power-
//!   optimization Phase 7 — see `xtask::pm_apb_lock_gate`'s doc).
//! - **verify-slint-thread-affinity** — no file under `firmware/src/`
//!   outside the `firmware/src/ui/`/`firmware/src/ui_task.rs` boundary names
//!   `UiRuntime`, `slint::`, or `i_slint*` (ADR-0012 R8 — see
//!   `xtask::slint_thread_affinity`'s doc).
//! - **verify-ci-filter-coverage** — every root Cargo workspace member is
//!   explicitly wired into `.github/workflows/ci.yml`'s `changes` job path
//!   filter, under `full:` or `host:` (see `xtask::ci_filter_coverage`'s
//!   doc).
//!
//! All also run as `cargo test`s. That used to be unconditionally "what CI
//! actually gates on" and this binary a mere convenience for a human's
//! quick manual re-check — it no longer is: `.github/workflows/ci.yml`'s
//! `firmware` job runs `cargo run -p xtask --bin xtask` directly (not
//! `cargo test`) as its own CI gate on firmware-only PRs, precisely because
//! the `test` job (where these run as `#[test]`s) is skipped for those
//! diffs. Believing this binary was purely a manual convenience — the exact
//! belief this doc comment stated — is what let that firmware-only-PR gap
//! open in the first place (deep-review pass 3 F4). Treat every check
//! above as CI-gating either way: via `cargo test` on a host/full-lane PR,
//! or via this binary directly on a firmware-only one. It runs every check
//! in the battery and reports all of them before exiting, rather than
//! short-circuiting on the first — a full pass (manual or CI) should
//! surface everything in one go.
//!
//! One check is deliberately NOT part of that default battery and is NOT a
//! `cargo test`, because it needs the `esp` cross-toolchain and takes
//! minutes rather than milliseconds — it must be named explicitly:
//!
//! - **verify-partition-budget** — recomputes the actual firmware app-image
//!   size from a fresh release build and diffs it against the committed
//!   baseline, failing loudly past a drift threshold (see
//!   `xtask::partition_budget`'s doc). Run with:
//!   `cargo run -p xtask --bin xtask -- verify-partition-budget`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let repo_root = xtask::repo_root_from_manifest_dir();

    // `verify-partition-budget` requires the `esp` cross-toolchain and a
    // multi-minute release build (see xtask::partition_budget's module doc
    // for why it can't just join the battery below) — dispatch it alone,
    // rather than unconditionally running it on every plain `cargo run -p
    // xtask --bin xtask`, which would break on any machine without that
    // toolchain bootstrapped.
    if std::env::args().any(|a| a == "verify-partition-budget") {
        return match xtask::partition_budget::check(&repo_root) {
            Ok(report) => {
                if report.over_threshold {
                    eprintln!(
                        "xtask verify-partition-budget: FAILED — {}",
                        report.summary()
                    );
                    ExitCode::FAILURE
                } else {
                    println!("xtask verify-partition-budget: OK — {}", report.summary());
                    ExitCode::SUCCESS
                }
            }
            Err(e) => {
                eprintln!("xtask verify-partition-budget: ERROR — {e}");
                ExitCode::FAILURE
            }
        };
    }

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

    let lock_gate = xtask::lock_gate::check(&repo_root);
    if lock_gate.is_empty() {
        println!(
            "xtask verify-lock-gate: OK — {}'s keyboard/trackball input blocks gate on \
             `self.locked` before any active-screen-dependent branch.",
            xtask::lock_gate::UI_MOD_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-lock-gate: FAILED — {} violation(s):",
            lock_gate.len()
        );
        for v in &lock_gate {
            eprintln!("  - {v}");
        }
    }

    let lock_integrity_fixes = xtask::lock_integrity_fixes::check(&repo_root);
    if lock_integrity_fixes.is_empty() {
        println!(
            "xtask verify-lock-integrity-fixes: OK — trip_lock fails closed (F2) and \
             FRAME_SET_LOCK_PIN live-forwards to the UI thread (F1)."
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-lock-integrity-fixes: FAILED — {} violation(s):",
            lock_integrity_fixes.len()
        );
        for v in &lock_integrity_fixes {
            eprintln!("  - {v}");
        }
    }

    let power_provenance = xtask::power_provenance::check(&repo_root);
    if power_provenance.is_empty() {
        println!(
            "xtask verify-power-provenance: OK — every power-current figure under docs/ \
             carries a [DATASHEET]/[ESTIMATE]/[MEASURED] provenance tag nearby (ADR-0014 D2)."
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-power-provenance: FAILED — {} violation(s):",
            power_provenance.len()
        );
        for v in &power_provenance {
            eprintln!("  - {v}");
        }
    }

    let render_asleep_gate = xtask::render_asleep_gate::check(&repo_root);
    if render_asleep_gate.is_empty() {
        println!(
            "xtask verify-render-asleep-gate: OK — {}'s render section is gated on \
             `render_gate(self.screen_asleep)` before `render_if_needed`.",
            xtask::render_asleep_gate::UI_MOD_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-render-asleep-gate: FAILED — {} violation(s):",
            render_asleep_gate.len()
        );
        for v in &render_asleep_gate {
            eprintln!("  - {v}");
        }
    }

    let pm_apb_lock_gate = xtask::pm_apb_lock_gate::check(&repo_root);
    if pm_apb_lock_gate.is_empty() {
        println!(
            "xtask verify-pm-apb-lock-gate: OK — the radio's two SPI2 funnel points \
             (write_cmd/spi_transfer, {}) and the GPS ACTIVE window ({}) are bracketed by an \
             ESP_PM_APB_FREQ_MAX lock. This does NOT cover the ST7789 display controller, which \
             also shares SPI2, nor the GT911/keyboard I2C or LEDC backlight timer — see ADR-0014 \
             D4.3/D8 for why that coverage is deliberately partial.",
            xtask::pm_apb_lock_gate::RADIO_REL_PATH,
            xtask::pm_apb_lock_gate::GPS_REL_PATH
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-pm-apb-lock-gate: FAILED — {} violation(s):",
            pm_apb_lock_gate.len()
        );
        for v in &pm_apb_lock_gate {
            eprintln!("  - {v}");
        }
    }

    let slint_thread_affinity = xtask::slint_thread_affinity::check(&repo_root);
    if slint_thread_affinity.is_empty() {
        println!(
            "xtask verify-slint-thread-affinity: OK — no file outside firmware/src/ui/ or \
             firmware/src/ui_task.rs names UiRuntime, slint::, or i_slint* (ADR-0012 R8)."
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-slint-thread-affinity: FAILED — {} violation(s):",
            slint_thread_affinity.len()
        );
        for v in &slint_thread_affinity {
            eprintln!("  - {v}");
        }
    }

    let ci_filter_coverage = xtask::ci_filter_coverage::check(&repo_root);
    if ci_filter_coverage.is_empty() {
        println!(
            "xtask verify-ci-filter-coverage: OK — every root Cargo workspace member has an \
             explicit entry in ci.yml's `full:`/`host:` path filter."
        );
    } else {
        ok = false;
        eprintln!(
            "xtask verify-ci-filter-coverage: FAILED — {} violation(s):",
            ci_filter_coverage.len()
        );
        for v in &ci_filter_coverage {
            eprintln!("  - {v}");
        }
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
