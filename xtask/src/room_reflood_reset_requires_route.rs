// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard pinning `meshcadet-room-reflood-backoff-resets-
//! without-a-learned-route`'s fix: `apply_room_login_outcome`'s
//! `room.reflood_attempts = 0` reset must be gated on the session actually
//! having a learned route, never unconditional on "a login reply arrived".
//!
//! # Why this lives in `xtask` and not in `firmware`
//!
//! Same reason the other scanners in this crate do (see
//! `xtask::room_reflood_cadence`'s module doc): the `firmware` crate's
//! single `[[bin]]` target sets `harness = false`, so a `#[test]` inside
//! `firmware/src/main.rs` is type-checked but never EXECUTED by `cargo
//! test`. This module is the host-runnable equivalent, in the same
//! "plain text scanning, no esp toolchain" spirit.
//!
//! # The invariant being pinned
//!
//! `room_session::room_reflood_interval_ms`'s doc names exactly two reset
//! conditions for the reflood backoff epoch: a successful login reply, or an
//! inbound push. `apply_room_login_outcome` used to reset
//! `room.reflood_attempts` on EVERY login reply unconditionally — including
//! a direct `RESPONSE` datagram reply, which `decode_login_response_datagram`
//! always decodes with `out_path: None` (it teaches no route). A session
//! stuck at `out_path_len == 0` that receives such a reply would clear its
//! attempt counter anyway, so the very next scheduler tick re-floods a full
//! `ANON_REQ` login at the 30 s floor again — forever, with no escalation:
//! the exact airtime/regulatory-duty-cycle defect
//! `meshcadet-room-reflood-login-backoff` introduced this backoff to
//! prevent. The fix: only reset `reflood_attempts` when `out_path_len != 0`
//! (a route is actually known) AFTER `apply_login_outcome` has run.
//!
//! # Scope and honest limits
//!
//! Structural, not behavioural: it checks that the `room.reflood_attempts =
//! 0` statement inside `apply_room_login_outcome` sits inside a braced block
//! whose own `if` condition mentions `out_path_len` (or a future
//! `has_route()`-shaped rename) — not that the guard's runtime semantics are
//! correct, which is a plain boolean firmware-core's own reflood-cadence
//! tests already pin. It fails loud (a reported violation, never a silent
//! skip) if `apply_room_login_outcome` or the reset statement can't be
//! located at all, per this crate's "parse gap = NO-GO" doctrine.

use std::fs;
use std::path::Path;

use crate::{brace_spans, innermost_span, tokenize};

/// Path, relative to the repo root, of the file this module scans.
pub const MAIN_RS_REL_PATH: &str = "firmware/src/main.rs";

/// The exact reset statement this scanner guards.
const RESET_NEEDLE: &str = "room.reflood_attempts = 0";

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

/// The text of the smallest enclosing statement/block boundary immediately
/// before `open_brace_pos` — i.e. everything back to (but not including) the
/// nearest preceding `;`, `{`, or `}`. For a plain `if <cond> {`, this is
/// `if <cond>` (trimmed). Conditions in this file never themselves contain
/// `;`/`{`/`}`, so this is a safe, simple boundary to walk back to.
fn guard_condition_text(chars: &[char], open_brace_pos: usize) -> String {
    let mut start = open_brace_pos;
    while start > 0 {
        let c = chars[start - 1];
        if c == ';' || c == '{' || c == '}' {
            break;
        }
        start -= 1;
    }
    chars[start..open_brace_pos].iter().collect::<String>()
}

/// Scan already-read source text and return every contract violation. Split
/// from [`check`] so the tests can drive it with synthetic sources.
pub fn check_source(src: &str) -> Vec<String> {
    let masked = tokenize(src).masked;
    let chars: Vec<char> = masked.chars().collect();
    let spans = brace_spans(&masked);

    let fn_needle = "fn apply_room_login_outcome(";
    let fn_hits = find_all(&chars, fn_needle);
    let fn_pos = match fn_hits.len() {
        1 => fn_hits[0],
        0 => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: no `{fn_needle}` found — `apply_room_login_outcome` was \
                 renamed/restructured, or this scanner needs updating"
            )]
        }
        n => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: {n} occurrences of `{fn_needle}` (expected exactly one) — \
                 this scanner cannot disambiguate the function"
            )]
        }
    };

    let Some(fn_open) = next_open_brace(&chars, fn_pos) else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: could not find `apply_room_login_outcome`'s opening brace"
        )];
    };
    let Some((bo, bc)) = spans.iter().find(|&&(o, _)| o == fn_open).copied() else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: could not brace-match `apply_room_login_outcome`'s body"
        )];
    };

    let mut violations = Vec::new();

    let reset_hits: Vec<usize> = find_all(&chars[bo..bc], RESET_NEEDLE)
        .into_iter()
        .map(|rel| bo + rel)
        .collect();
    let reset_pos = match reset_hits.len() {
        1 => reset_hits[0],
        0 => {
            violations.push(format!(
                "{MAIN_RS_REL_PATH}: could not locate `{RESET_NEEDLE}` inside \
                 `apply_room_login_outcome` — the reflood backoff reset was removed or reshaped; \
                 this scanner can no longer confirm it is route-gated"
            ));
            return violations;
        }
        n => {
            violations.push(format!(
                "{MAIN_RS_REL_PATH}: {n} occurrences of `{RESET_NEEDLE}` inside \
                 `apply_room_login_outcome` (expected exactly one) — this scanner cannot \
                 disambiguate which is the reflood backoff reset"
            ));
            return violations;
        }
    };

    let Some((io, ic)) = innermost_span(&spans, reset_pos) else {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: `{RESET_NEEDLE}` at char {reset_pos} is not inside any braced \
             block at all — unreachable given it must at least sit inside the function body"
        ));
        return violations;
    };

    if io == bo && ic == bc {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: `{RESET_NEEDLE}` resets UNCONDITIONALLY on every login reply — \
             this is the exact `meshcadet-room-reflood-backoff-resets-without-a-learned-route` \
             regression: a direct RESPONSE reply (`decode_login_response_datagram`) always \
             decodes `out_path: None` and teaches no route, so an unconditional reset here \
             defeats the backoff forever for a session stuck at `out_path_len == 0`"
        ));
        return violations;
    }

    let cond = guard_condition_text(&chars, io);
    if !cond.contains("out_path_len") && !cond.contains("has_route") {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: `{RESET_NEEDLE}` is gated on `{}`, which does not reference \
             `out_path_len` or `has_route(..)` — this scanner cannot confirm the reset is \
             actually gated on the session having a learned route rather than some unrelated \
             condition",
            cond.trim(),
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

    /// The actual guard: the shipped `firmware/src/main.rs` only resets the
    /// reflood backoff epoch when a route is actually known.
    #[test]
    fn reflood_reset_is_route_gated() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "reflood-reset route-gating contract violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    const GATED_BASELINE: &str = r#"
        fn apply_room_login_outcome(
            room: &mut RoomRuntime,
            outcome: &room_session::RoomLoginOutcome,
        ) {
            room.session.apply_login_outcome(outcome);
            room.keep_alive_stall.reset();
            if room.session.out_path_len != 0 {
                room.reflood_attempts = 0;
            }
            room.resync_pending = true;
        }
    "#;

    #[test]
    fn synthetic_gated_baseline_is_clean() {
        assert!(
            check_source(GATED_BASELINE).is_empty(),
            "{:?}",
            check_source(GATED_BASELINE)
        );
    }

    #[test]
    fn synthetic_unconditional_reset_is_caught() {
        // Models the EXACT pre-fix defect: unconditional reset on every
        // login reply, including a direct RESPONSE that teaches no route.
        let synthetic = r#"
            fn apply_room_login_outcome(
                room: &mut RoomRuntime,
                outcome: &room_session::RoomLoginOutcome,
            ) {
                room.session.apply_login_outcome(outcome);
                room.keep_alive_stall.reset();
                room.reflood_attempts = 0;
                room.resync_pending = true;
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("resets UNCONDITIONALLY")),
            "an unconditional reset must be caught: {violations:?}"
        );
    }

    #[test]
    fn synthetic_reset_gated_on_an_unrelated_condition_is_caught() {
        // The reset IS inside a braced block, but the guard doesn't
        // reference route-known-ness at all — a different bug shape, still
        // must not pass.
        let synthetic = r#"
            fn apply_room_login_outcome(
                room: &mut RoomRuntime,
                outcome: &room_session::RoomLoginOutcome,
            ) {
                room.session.apply_login_outcome(outcome);
                if outcome.server_ts != 0 {
                    room.reflood_attempts = 0;
                }
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("does not reference `out_path_len` or `has_route(..)`")),
            "a reset gated on something other than route-known-ness must be caught: \
             {violations:?}"
        );
    }

    #[test]
    fn synthetic_has_route_gate_is_accepted() {
        // Forward-compatible: a future `has_route()`-shaped rename of the
        // route-known predicate must also satisfy this guard.
        let synthetic = r#"
            fn apply_room_login_outcome(
                room: &mut RoomRuntime,
                outcome: &room_session::RoomLoginOutcome,
            ) {
                room.session.apply_login_outcome(outcome);
                if room.session.has_route() {
                    room.reflood_attempts = 0;
                }
            }
        "#;
        assert!(
            check_source(synthetic).is_empty(),
            "{:?}",
            check_source(synthetic)
        );
    }

    #[test]
    fn a_missing_function_is_a_violation_not_a_silent_pass() {
        let violations = check_source("fn main() {}");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no `fn apply_room_login_outcome(` found")),
            "{violations:?}"
        );
    }

    #[test]
    fn comment_mentions_of_the_reset_are_not_counted() {
        // A doc comment mentioning the reset statement's own text must not
        // itself be mistaken for a second occurrence — masked
        // comments/strings are blanked before scanning.
        let synthetic = r#"
            fn apply_room_login_outcome(
                room: &mut RoomRuntime,
                outcome: &room_session::RoomLoginOutcome,
            ) {
                room.session.apply_login_outcome(outcome);
                // Previously `room.reflood_attempts = 0` unconditionally —
                // see the mission doc for why that was wrong.
                if room.session.out_path_len != 0 {
                    room.reflood_attempts = 0;
                }
            }
        "#;
        assert!(
            check_source(synthetic).is_empty(),
            "{:?}",
            check_source(synthetic)
        );
    }

    #[test]
    fn a_missing_reset_statement_is_a_violation_not_a_silent_pass() {
        let synthetic = r#"
            fn apply_room_login_outcome(
                room: &mut RoomRuntime,
                outcome: &room_session::RoomLoginOutcome,
            ) {
                room.session.apply_login_outcome(outcome);
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("could not locate `room.reflood_attempts = 0`")),
            "{violations:?}"
        );
    }
}
