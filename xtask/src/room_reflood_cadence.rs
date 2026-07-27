// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard pinning `meshcadet-room-reflood-login-backoff`'s
//! fix in `firmware/src/main.rs`'s room keep-alive scheduler.
//!
//! # Why this lives in `xtask` and not in `firmware`
//!
//! Same reason the glyph harness and the notification-surface guard do (see
//! `xtask::ui_event_parity`'s module doc): the `firmware` crate's single
//! `[[bin]]` target sets `harness = false`, so a `#[test]` inside
//! `firmware/src/main.rs` is type-checked but never EXECUTED by `cargo
//! test`. This module is the host-runnable equivalent, in the same "plain
//! text scanning, no esp toolchain" spirit.
//!
//! # The invariant being pinned
//!
//! FINDING B of `meshcadet-room-reflood-login-backoff`'s Objective: the room
//! keep-alive scheduler's `for room in room_runtime.iter_mut()` loop has TWO
//! branches that must gate on TWO INDEPENDENT cadences —
//!   - `out_path_len == 0` (no learned route): must gate on
//!     `room_session::room_reflood_interval_ms`'s own, backed-off cadence,
//!     and must NEVER reference `ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS`,
//!     `room_session::room_keep_alive_interval_ms`, or `is_draining`.
//!   - the route-direct keep-alive branch (everything else in the loop):
//!     must still gate on `room_session::room_keep_alive_interval_ms` /
//!     `ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS`, unchanged.
//!
//! Before this mission's fix both branches shared ONE gate, keyed on
//! `ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS` (15 s) whenever
//! `firmware_core::room_session::RoomSyncPhase::is_draining()` was true —
//! which is FOREVER for a room whose server never answers (a keep-alive ACK
//! is the only thing that ever closes that drain window). The result: an
//! offline/out-of-range/decommissioned room server got re-flooded a full
//! `ANON_REQ` login every 15 s, forever, with no backoff and no cap — a
//! flood frame every relaying node in the mesh rebroadcasts, so this was an
//! airtime/regulatory-duty-cycle defect, not merely a battery one. A
//! per-diff review of the commit that introduced
//! `ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS` could not have caught this: it
//! reasoned correctly about the route-direct keep-alive branch it was
//! written for, and had no reason to notice an unrelated branch quietly
//! shared its gate. This scanner pins the two branches structurally apart
//! so a future change can't silently re-couple them again.
//!
//! # Scope and honest limits
//!
//! Structural, not behavioural: it checks which identifiers appear inside
//! each branch, not that the gate values are numerically sane — that
//! numeric behaviour is what `firmware_core::room_session`'s own `#[test]`s
//! for `room_reflood_interval_ms` / `room_keep_alive_interval_ms` pin. It
//! fails loud (a reported violation, never a silent skip) if the scheduler
//! loop or either branch can't be located at all, per this crate's
//! "parse gap = NO-GO" doctrine. A legitimate refactor of the loop's shape
//! will trip it — that is the intended trade: teach this scanner the new
//! shape, don't suppress it.

use std::fs;
use std::path::Path;

use crate::{brace_spans, innermost_span, slice_chars, tokenize};

/// Path, relative to the repo root, of the file this module scans.
pub const MAIN_RS_REL_PATH: &str = "firmware/src/main.rs";

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

/// Given the char-index of a `{`, return the (open, close) span it delimits
/// — `None` if `open_brace_pos` doesn't actually open one of `spans`, or the
/// braces are unbalanced around it.
fn brace_body(spans: &[(usize, usize)], open_brace_pos: usize) -> Option<(usize, usize)> {
    let (o, c) = innermost_span(spans, open_brace_pos + 1)?;
    if o == open_brace_pos {
        Some((o, c))
    } else {
        None
    }
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

    // `firmware/src/main.rs` has THREE `for room in room_runtime.iter_mut()`
    // loops (boot-time login, this scheduler, `handle_ack`) — disambiguate
    // via `if !room.login_sent`, a marker that appears EXACTLY once in the
    // whole file and only ever as this loop's first statement (the boot
    // login loop's equivalent guard is the un-negated `if room.login_sent`).
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
            "{MAIN_RS_REL_PATH}: no `{loop_needle}` loop precedes the `{marker}` guard — \
             this scanner cannot locate the scheduler loop"
        )];
    };
    let loop_open = loop_pos + loop_needle.chars().count() - 1;
    let Some((lo, lc)) = brace_body(&spans, loop_open) else {
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
    let Some(branch_open) = next_open_brace(&chars[..lc], branch_cond_pos) else {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: `out_path_len == 0` has no braced body this scanner can delimit"
        ));
        return violations;
    };
    let Some((bo, bc)) = brace_body(&spans, branch_open) else {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: could not brace-match the `out_path_len == 0` re-flood branch"
        ));
        return violations;
    };

    let reflood_body = slice_chars(&masked, bo + 1, bc);
    let rest_of_loop = format!(
        "{}{}",
        slice_chars(&masked, lo + 1, bo),
        slice_chars(&masked, bc + 1, lc)
    );

    if !reflood_body.contains("room_reflood_interval_ms") {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: the `out_path_len == 0` re-flood branch no longer gates on \
             `room_session::room_reflood_interval_ms` — it must use its own decoupled, \
             backed-off cadence, not an ungated (or re-coupled) reflood"
        ));
    }
    if reflood_body.contains("ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS")
        || reflood_body.contains("room_keep_alive_interval_ms")
        || reflood_body.contains("is_draining")
    {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: the `out_path_len == 0` re-flood branch references the \
             drain/routine keep-alive cadence (`ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS` / \
             `room_keep_alive_interval_ms` / `is_draining`) — this is FINDING B's exact \
             regression: an offline room server would again be re-flooded every 15s forever, \
             with no backoff, because the reflood branch shares the route-direct keep-alive's \
             drain-cadence gate"
        ));
    }

    // Sanity check the OTHER branch is still there, still on its own
    // cadence — if this ever fails, either the scanner is stale or the
    // route-direct branch's cadence was itself accidentally deleted (a
    // different, real regression this scanner should also surface rather
    // than silently pass).
    if !rest_of_loop.contains("room_keep_alive_interval_ms")
        || !rest_of_loop.contains("ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS")
    {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: the route-direct keep-alive branch (the rest of the scheduler \
             loop) no longer gates on `room_session::room_keep_alive_interval_ms` / \
             `ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS` — this scanner can no longer confirm the two \
             cadences are actually distinct branches"
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

    /// The actual guard: the shipped `firmware/src/main.rs` keeps the
    /// re-flood cadence decoupled from the drain/routine one.
    #[test]
    fn reflood_cadence_decoupling_holds() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "reflood-cadence decoupling violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    const DECOUPLED_BASELINE: &str = r#"
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
                if now.saturating_sub(room.last_reflood_ms) < interval {
                    continue;
                }
                txq.enqueue(&frame[..n]);
                continue;
            }
            let interval = room_session::room_keep_alive_interval_ms(
                room.last_keep_alive_ms,
                room.sync_phase.is_draining(),
                ROOM_FIRST_KEEP_ALIVE_DELAY_MS,
                ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS,
                ROOM_KEEP_ALIVE_INTERVAL_MS,
            );
            if now.saturating_sub(room.last_keep_alive_ms) < interval {
                continue;
            }
        }
    "#;

    #[test]
    fn synthetic_decoupled_baseline_is_clean() {
        assert!(
            check_source(DECOUPLED_BASELINE).is_empty(),
            "{:?}",
            check_source(DECOUPLED_BASELINE)
        );
    }

    #[test]
    fn synthetic_recoupled_regression_is_caught() {
        // Models the EXACT pre-fix defect: the re-flood branch shares the
        // drain-cadence gate instead of using its own.
        let synthetic = r#"
            for room in room_runtime.iter_mut() {
                if !room.login_sent {
                    continue;
                }
                let interval = room_session::room_keep_alive_interval_ms(
                    room.last_keep_alive_ms,
                    room.sync_phase.is_draining(),
                    ROOM_FIRST_KEEP_ALIVE_DELAY_MS,
                    ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS,
                    ROOM_KEEP_ALIVE_INTERVAL_MS,
                );
                if now.saturating_sub(room.last_keep_alive_ms) < interval {
                    continue;
                }
                room.last_keep_alive_ms = now;
                if room.session.out_path_len == 0 {
                    txq.enqueue(&frame[..n]);
                    continue;
                }
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no longer gates on `room_session::room_reflood_interval_ms`")),
            "a re-flood branch that inherits its ONLY gate from the shared drain-cadence check \
             above it (the exact pre-fix shape — the branch itself has no cadence of its own) \
             must be caught: {violations:?}"
        );
    }

    #[test]
    fn synthetic_reflood_branch_that_reimports_the_drain_cadence_directly_is_caught() {
        // A DIFFERENT, more overt re-coupling: the reflood branch calls
        // `room_reflood_interval_ms` (so the first check above would pass)
        // but ALSO references the drain-cadence identifiers directly inside
        // its own body — still a coupling regression, and a distinct defect
        // shape from the "no gate at all" one above.
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
                    if room.sync_phase.is_draining() && interval < ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS {
                        continue;
                    }
                    continue;
                }
                let interval = room_session::room_keep_alive_interval_ms(
                    room.last_keep_alive_ms,
                    room.sync_phase.is_draining(),
                    ROOM_FIRST_KEEP_ALIVE_DELAY_MS,
                    ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS,
                    ROOM_KEEP_ALIVE_INTERVAL_MS,
                );
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("FINDING B's exact regression")),
            "a re-flood branch that directly references the drain-cadence identifiers inside \
             its own body must be caught: {violations:?}"
        );
    }

    #[test]
    fn synthetic_missing_reflood_gate_is_caught() {
        // An ungated reflood (no cadence function at all) must also fail —
        // not just a re-coupled one.
        let synthetic = r#"
            for room in room_runtime.iter_mut() {
                if !room.login_sent {
                    continue;
                }
                if room.session.out_path_len == 0 {
                    txq.enqueue(&frame[..n]);
                    continue;
                }
                let interval = room_session::room_keep_alive_interval_ms(
                    room.last_keep_alive_ms,
                    room.sync_phase.is_draining(),
                    ROOM_FIRST_KEEP_ALIVE_DELAY_MS,
                    ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS,
                    ROOM_KEEP_ALIVE_INTERVAL_MS,
                );
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no longer gates on `room_session::room_reflood_interval_ms`")),
            "an ungated reflood branch must be caught: {violations:?}"
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

    #[test]
    fn comment_mentions_of_the_coupled_identifiers_are_not_counted() {
        // A doc comment mentioning the OLD coupled identifiers inside the
        // re-flood branch must not itself trip the guard — only CODE usage
        // should (masked comments/strings are blanked before scanning).
        let synthetic = r#"
            for room in room_runtime.iter_mut() {
                if !room.login_sent {
                    continue;
                }
                if room.session.out_path_len == 0 {
                    // Deliberately NOT `ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS` /
                    // `room_keep_alive_interval_ms` / `is_draining` — see doc.
                    let interval = room_session::room_reflood_interval_ms(
                        room.reflood_attempts,
                        ROOM_REFLOOD_INITIAL_BACKOFF_MS,
                        ROOM_REFLOOD_BACKOFF_CEILING_MS,
                    );
                    if now.saturating_sub(room.last_reflood_ms) < interval {
                        continue;
                    }
                    continue;
                }
                let interval = room_session::room_keep_alive_interval_ms(
                    room.last_keep_alive_ms,
                    room.sync_phase.is_draining(),
                    ROOM_FIRST_KEEP_ALIVE_DELAY_MS,
                    ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS,
                    ROOM_KEEP_ALIVE_INTERVAL_MS,
                );
            }
        "#;
        assert!(
            check_source(synthetic).is_empty(),
            "{:?}",
            check_source(synthetic)
        );
    }
}
