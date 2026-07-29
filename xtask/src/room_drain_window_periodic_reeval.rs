// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard pinning
//! `meshcadet-room-drain-window-out-path-never-learned-fix`'s central fix in
//! `firmware/src/main.rs`'s room keep-alive scheduler.
//!
//! # Why this lives in `xtask` and not in `firmware`
//!
//! Same reason `room_reflood_cadence` and `room_aggregate_notification` do
//! (see their module docs): the `firmware` crate's single `[[bin]]` target
//! sets `harness = false`, so a `#[test]` inside `firmware/src/main.rs` is
//! type-checked but never EXECUTED by `cargo test`. This module is the
//! host-runnable equivalent, in the same "plain text scanning, no esp
//! toolchain" spirit — the behavioural half of this fix (does the stall
//! bound actually re-evaluate and flush correctly) is pinned by
//! `firmware_core::room_session`'s own `RoomSyncPhase::on_scheduler_tick`
//! tests; this scanner pins that `firmware::main`'s scheduler loop actually
//! WIRES that classifier in, unconditionally, on every pass.
//!
//! # The invariant being pinned
//!
//! `flight-manuals/library/deferral-bound-is-load-bearing.md` § Third
//! recurrence: `DRAIN_WINDOW_STALL_TIMEOUT_MS` had two closers before this
//! mission, both reachable only from INSIDE a handler for some other event
//! (a post arriving — `RoomSyncPhase::on_post_received`; a keep-alive-stall
//! detection — `RoomSyncPhase::note_closer_failed`, itself unreachable
//! without a learned `out_path`). A session whose `out_path` is never
//! learned at all, that absorbs exactly one post and no successor, never
//! re-hit either closer again.
//!
//! `RoomSyncPhase::on_scheduler_tick` is the fix — a periodic,
//! event-independent re-evaluation — but it is only a real fix if
//! `firmware::main`'s scheduler loop actually CALLS it on every pass, for
//! every logged-in room, BEFORE the `out_path_len == 0` branch (that branch
//! always `continue`s, so any call placed after it would never run for the
//! exact case this mission fixes — a route that is never learned). This
//! scanner pins both:
//!   - `room.sync_phase.on_scheduler_tick(...)` is called inside the
//!     scheduler loop, textually BEFORE the `out_path_len == 0` branch's own
//!     condition (i.e. unconditionally reached every pass, not nested inside
//!     either cadence branch).
//!   - the `Some(RoomNotification::Aggregate { .. })` arm that call feeds
//!     raises `UiEvent::RoomDrainComplete` — mirroring
//!     `room_aggregate_notification`'s guard for the other two call sites,
//!     so a future refactor can't silently turn this into a no-op the same
//!     way `meshcadet-room-post-no-notification`'s HIL capture caught once
//!     already.
//!
//! # Scope and honest limits
//!
//! Structural, not behavioural: it checks call placement and that the right
//! event constructor appears in the right arm, not that the timing math
//! itself is correct — that is `firmware_core::room_session`'s own job. It
//! fails loud (a reported violation, never a silent skip) if the loop or
//! either marker can't be located at all, per this crate's "parse gap =
//! NO-GO" doctrine. A legitimate refactor of the loop's shape will trip it —
//! that is the intended trade: teach this scanner the new shape, don't
//! suppress it.

use std::fs;
use std::path::Path;

use crate::{brace_spans, innermost_span, slice_chars, tokenize};

/// Path, relative to the repo root, of the file this module scans.
pub const MAIN_RS_REL_PATH: &str = "firmware/src/main.rs";

const TICK_CALL: &str = "on_scheduler_tick";
const REQUIRED_EVENT: &str = "UiEvent::RoomDrainComplete";

/// Char-index positions of every occurrence of `needle` in `hay`.
fn find_all(hay: &[char], needle: &str) -> Vec<usize> {
    let pat: Vec<char> = needle.chars().collect();
    if pat.is_empty() || hay.len() < pat.len() {
        return Vec::new();
    }
    (0..=hay.len() - pat.len())
        .filter(|&i| hay[i..i + pat.len()] == pat[..])
        .collect()
}

/// Walk forward from `start` to the next `{`, returning its char index —
/// `None` if the text runs out first.
fn next_open_brace(chars: &[char], start: usize) -> Option<usize> {
    (start..chars.len()).find(|&i| chars[i] == '{')
}

/// Scan already-read source text and return every contract violation.
/// Split from [`check`] so the tests can drive it with synthetic sources.
pub fn check_source(src: &str) -> Vec<String> {
    let masked = tokenize(src).masked;
    let chars: Vec<char> = masked.chars().collect();
    let spans = brace_spans(&masked);

    // Same disambiguation `room_reflood_cadence` uses: `firmware/src/main.rs`
    // has more than one `for room in room_runtime.iter_mut()` loop; `if
    // !room.login_sent` appears exactly once, as this loop's first
    // statement.
    let loop_needle = "for room in room_runtime.iter_mut() {";
    let loop_hits = find_all(&chars, loop_needle);
    if loop_hits.is_empty() {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: no `{loop_needle}` loop found — the room keep-alive scheduler \
             was renamed/restructured, or this scanner needs updating"
        )];
    }
    let marker = "if !room.login_sent";
    let marker_hits = find_all(&chars, marker);
    let marker_pos = match marker_hits.len() {
        1 => marker_hits[0],
        0 => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: no `{marker}` guard found — this scanner can no longer \
                 disambiguate the scheduler loop from the other \
                 `room_runtime.iter_mut()` loops"
            )]
        }
        n => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: {n} occurrences of `{marker}` (expected exactly one) — \
                 this scanner cannot disambiguate the scheduler loop"
            )]
        }
    };
    let Some(loop_pos) = loop_hits.iter().filter(|&&p| p < marker_pos).max().copied() else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: no `{loop_needle}` loop precedes the `{marker}` guard — this \
             scanner cannot locate the scheduler loop"
        )];
    };
    let loop_open = loop_pos + loop_needle.chars().count() - 1;
    let Some((lo, lc)) = innermost_span(&spans, loop_open + 1).filter(|&(o, _)| o == loop_open)
    else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: could not brace-match the room keep-alive scheduler loop body"
        )];
    };

    let mut violations = Vec::new();

    let branch_needle = "out_path_len == 0";
    let branch_hits: Vec<usize> = find_all(&chars[lo..lc], branch_needle)
        .into_iter()
        .map(|rel| lo + rel)
        .collect();
    let branch_cond_pos = match branch_hits.len() {
        1 => branch_hits[0],
        0 => {
            violations.push(format!(
                "{MAIN_RS_REL_PATH}: could not locate the `out_path_len == 0` re-flood branch \
                 inside the scheduler loop"
            ));
            return violations;
        }
        n => {
            violations.push(format!(
                "{MAIN_RS_REL_PATH}: {n} occurrences of `out_path_len == 0` inside the \
                 scheduler loop (expected exactly one) — this scanner cannot tell which is the \
                 re-flood branch"
            ));
            return violations;
        }
    };

    let tick_hits: Vec<usize> = find_all(&chars[lo..lc], TICK_CALL)
        .into_iter()
        .map(|rel| lo + rel)
        .collect();
    let tick_pos = match tick_hits.len() {
        1 => tick_hits[0],
        0 => {
            violations.push(format!(
                "{MAIN_RS_REL_PATH}: no `{TICK_CALL}` call found inside the scheduler loop — the \
                 periodic, event-independent drain-window re-evaluation \
                 (`RoomSyncPhase::on_scheduler_tick`) is missing; a session whose `out_path` is \
                 never learned that absorbs exactly one post and no successor will lose that \
                 post's notification forever (see \
                 flight-manuals/library/deferral-bound-is-load-bearing.md § Third recurrence)"
            ));
            return violations;
        }
        n => {
            violations.push(format!(
                "{MAIN_RS_REL_PATH}: {n} occurrences of `{TICK_CALL}` inside the scheduler loop \
                 (expected exactly one) — this scanner cannot tell which is the periodic \
                 re-evaluation call"
            ));
            return violations;
        }
    };

    if tick_pos >= branch_cond_pos {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: `{TICK_CALL}` is called AFTER the `out_path_len == 0` check — \
             that branch always `continue`s, so a call placed after it never runs for a room \
             whose `out_path` is never learned, which is exactly the case this mission fixes. \
             The call must precede the `out_path_len == 0` branch so it runs unconditionally, \
             every scheduler pass, for every logged-in room"
        ));
    }

    // The `if let Some(...) = ...on_scheduler_tick(...) { ... }` body: the
    // call itself is the RHS of the `if let`'s `=`, so the next `{` after
    // the call's own closing paren is that `if let`'s body brace — there is
    // no earlier brace between them (the pattern's own `{ count }`, if any,
    // sits BEFORE the call, on the LHS of `=`).
    let Some(arm_open) = next_open_brace(&chars[..lc], tick_pos) else {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: `{TICK_CALL}`'s result is not consumed by a braced `if let` \
             body this scanner can delimit"
        ));
        return violations;
    };
    let Some((ao, ac)) = innermost_span(&spans, arm_open + 1).filter(|&(o, _)| o == arm_open)
    else {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: could not brace-match the `{TICK_CALL}` consumer's body"
        ));
        return violations;
    };

    let body = slice_chars(&masked, ao + 1, ac);
    if !body.contains(REQUIRED_EVENT) {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: the `{TICK_CALL}` consumer body no longer raises \
             `{REQUIRED_EVENT}` — this reintroduces the exact defect this mission fixes: the \
             classifier correctly force-closes the drain window on its own periodic tick, but \
             nobody is ever told — no badge, no tone, no blink"
        ));
    }

    violations
}

/// Read `firmware/src/main.rs` under `repo_root` and return every contract
/// violation. Empty vec == the contract holds.
pub fn check(repo_root: &Path) -> Vec<String> {
    let path = repo_root.join(MAIN_RS_REL_PATH);
    match fs::read_to_string(&path) {
        Ok(src) => check_source(&src),
        Err(e) => vec![format!("reading {}: {e}", path.display())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual guard: the shipped `firmware/src/main.rs` wires the
    /// periodic re-evaluation in, unconditionally, before the reflood
    /// branch, and raises the notification event on a force-close.
    #[test]
    fn drain_window_periodic_reeval_wiring_holds() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "drain-window periodic re-evaluation wiring violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    const WIRED_BASELINE: &str = r#"
        for room in room_runtime.iter_mut() {
            if !room.login_sent {
                continue;
            }

            if let Some(room_session::RoomNotification::Aggregate { count }) =
                room.sync_phase.on_scheduler_tick(now)
            {
                if let Some(ref mut ui) = ui_opt {
                    ui.post_event(ui::UiEvent::RoomDrainComplete { room_hash: room.hash, count });
                }
            }

            if room.session.out_path_len == 0 {
                let interval = room_session::room_reflood_interval_ms(
                    room.reflood_attempts,
                    ROOM_REFLOOD_INITIAL_BACKOFF_MS,
                    ROOM_REFLOOD_BACKOFF_CEILING_MS,
                );
                continue;
            }
        }
    "#;

    #[test]
    fn synthetic_wired_baseline_is_clean() {
        assert!(
            check_source(WIRED_BASELINE).is_empty(),
            "{:?}",
            check_source(WIRED_BASELINE)
        );
    }

    /// Models a plausible-but-wrong "fix": the tick is wired AFTER the
    /// reflood branch (dead for the out-path-never-learned case, since that
    /// branch always `continue`s).
    #[test]
    fn synthetic_tick_after_reflood_branch_is_caught() {
        let synthetic = r#"
            for room in room_runtime.iter_mut() {
                if !room.login_sent {
                    continue;
                }

                if room.session.out_path_len == 0 {
                    let interval = room_session::room_reflood_interval_ms(
                        room.reflood_attempts,
                        ROOM_REFLOOD_INITIAL_BACKOFF_MS,
                        ROOM_REFLOOD_BACKOFF_CEILING_MS,
                    );
                    continue;
                }

                if let Some(room_session::RoomNotification::Aggregate { count }) =
                    room.sync_phase.on_scheduler_tick(now)
                {
                    ui.post_event(ui::UiEvent::RoomDrainComplete { room_hash: room.hash, count });
                }
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("called AFTER the `out_path_len == 0` check")),
            "{violations:?}"
        );
    }

    /// Models the no-op arm defect: the tick's result is computed but
    /// dropped on the floor.
    #[test]
    fn synthetic_no_op_arm_is_caught() {
        let synthetic = r#"
            for room in room_runtime.iter_mut() {
                if !room.login_sent {
                    continue;
                }

                if let Some(room_session::RoomNotification::Aggregate { count }) =
                    room.sync_phase.on_scheduler_tick(now)
                {
                    // Dropped on the floor — never reaches the UI.
                }

                if room.session.out_path_len == 0 {
                    continue;
                }
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no longer raises `UiEvent::RoomDrainComplete`")),
            "{violations:?}"
        );
    }

    #[test]
    fn synthetic_missing_tick_call_is_caught() {
        let synthetic = r#"
            for room in room_runtime.iter_mut() {
                if !room.login_sent {
                    continue;
                }
                if room.session.out_path_len == 0 {
                    continue;
                }
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no `on_scheduler_tick` call found")),
            "{violations:?}"
        );
    }

    #[test]
    fn a_missing_loop_is_a_violation_not_a_silent_pass() {
        let violations = check_source("fn main() {}");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no `for room in room_runtime.iter_mut() {` loop found")),
            "{violations:?}"
        );
    }

    /// A comment mentioning the required event must not be mistaken for the
    /// real call (the tokenizer blanks comment bodies first).
    #[test]
    fn comment_mentions_are_not_counted() {
        let synthetic = r#"
            for room in room_runtime.iter_mut() {
                if !room.login_sent {
                    continue;
                }

                if let Some(room_session::RoomNotification::Aggregate { count }) =
                    room.sync_phase.on_scheduler_tick(now)
                {
                    // TODO: raise UiEvent::RoomDrainComplete here.
                }

                if room.session.out_path_len == 0 {
                    continue;
                }
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no longer raises `UiEvent::RoomDrainComplete`")),
            "a comment-only mention must not satisfy the guard: {violations:?}"
        );
    }
}
