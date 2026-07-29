// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard pinning `meshcadet-room-post-no-notification`'s
//! central fix in `firmware/src/main.rs`'s `handle_room_push_frame`.
//!
//! # Why this lives in `xtask` and not in `firmware`
//!
//! Same reason `room_reflood_cadence` and `ui_event_parity` do (see their
//! module docs): the `firmware` crate's single `[[bin]]` target sets
//! `harness = false`, so a `#[test]` inside `firmware/src/main.rs` is
//! type-checked but never EXECUTED by `cargo test`. This module is the
//! host-runnable equivalent, in the same "plain text scanning, no esp
//! toolchain" spirit.
//!
//! # The invariant being pinned
//!
//! `handle_room_push_frame` classifies every genuinely-new incoming post via
//! `firmware_core::room_session::RoomSyncPhase::on_push_outcome`, which CAN
//! return `RoomNotification::Aggregate { count }` — not only from
//! `meshcadet-room-drain-window-never-closes-no-notify`'s stall-timeout
//! force-close (already true before this mission), but now also from
//! `meshcadet-room-post-no-notification`'s `RoomSyncPhase::note_closer_failed`
//! short-circuit. Before this mission's fix, the `match notification { … }`
//! arm for `RoomNotification::Aggregate` inside `handle_room_push_frame` was
//! a no-op — its own comment claimed the variant was unreachable from this
//! call site, which was already false the moment the stall-timeout
//! force-close landed. The classifier correctly flipped its internal state
//! to "closed" every time; the badge/tone/blink consequence was dropped on
//! the floor right here. Nothing in `firmware_core::room_session`'s own test
//! suite could ever have caught this — those tests pin the CLASSIFIER's
//! return value, not what `main.rs` then does with it, exactly the gap
//! this fix's investigation turned up ("assert the consequence, not just
//! the classifier's return value").
//!
//! This scanner pins that the `Aggregate` arm raises
//! `UiEvent::RoomDrainComplete` (the same event `handle_ack`'s
//! keep-alive-triggered close already correctly raises), so a future
//! refactor can't silently reintroduce the no-op arm.
//!
//! # Scope and honest limits
//!
//! Structural, not behavioural: it checks that the right event constructor
//! appears inside the right arm, not that its fields are correct — that is
//! `firmware_core::room_session`'s own job for the classifier, and
//! `xtask::ui_event_parity`'s for what the UI then does with the event once
//! raised. Fails loud (a reported violation, never a silent skip) if the
//! function or arm cannot be located at all, per this crate's "parse gap =
//! NO-GO" doctrine. A legitimate refactor of the arm's shape will trip it —
//! that is the intended trade: teach this scanner the new shape, don't
//! suppress it.

use std::fs;
use std::path::Path;

use crate::{brace_spans, innermost_span, slice_chars, tokenize};

/// Path, relative to the repo root, of the file this module scans.
pub const MAIN_RS_REL_PATH: &str = "firmware/src/main.rs";

const FN_MARKER: &str = "fn handle_room_push_frame";
const ARM_MARKER: &str = "RoomNotification::Aggregate";
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

/// Scan already-read source text and return every contract violation.
/// Split from [`check`] so the tests can drive it with synthetic sources.
pub fn check_source(src: &str) -> Vec<String> {
    let masked = tokenize(src).masked;
    let chars: Vec<char> = masked.chars().collect();
    let spans = brace_spans(&masked);

    let fn_hits = find_all(&chars, FN_MARKER);
    let fn_pos = match fn_hits.len() {
        1 => fn_hits[0],
        0 => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: no `{FN_MARKER}` found — the room push handler was \
                 renamed or moved, or this scanner needs updating"
            )]
        }
        n => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: {n} occurrences of `{FN_MARKER}` (expected exactly one) — \
                 this scanner cannot tell which is the function"
            )]
        }
    };
    let Some(fn_open) = (fn_pos..chars.len()).find(|&i| chars[i] == '{') else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: `{FN_MARKER}` has no braced body this scanner can delimit"
        )];
    };
    let Some((fo, fc)) = innermost_span(&spans, fn_open + 1).filter(|&(o, _)| o == fn_open) else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: could not brace-match `{FN_MARKER}`'s body"
        )];
    };

    let arm_hits: Vec<usize> = find_all(&chars[fo..fc], ARM_MARKER)
        .into_iter()
        .map(|rel| fo + rel)
        .collect();
    let arm_pos = match arm_hits.len() {
        1 => arm_hits[0],
        0 => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: no `{ARM_MARKER}` match arm found inside `{FN_MARKER}` — \
                 the arm was renamed or deleted, or this scanner needs updating"
            )]
        }
        n => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: {n} occurrences of `{ARM_MARKER}` inside `{FN_MARKER}` \
                 (expected exactly one) — this scanner cannot tell which is the arm"
            )]
        }
    };
    // Walk forward to the arm's `=>` FIRST, not to the next `{` — the
    // variant's own destructuring pattern (`RoomNotification::Aggregate {
    // count }`) has a brace of its own that a naive "next `{`" search would
    // mistake for the arm body, delimiting the pattern's `{ count }` instead
    // (mirrors `xtask::ui_event_parity::arm_body`'s identical two-step walk).
    let Some(fat_arrow) =
        (arm_pos..fc).find(|&i| chars[i] == '=' && chars.get(i + 1) == Some(&'>'))
    else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: `{ARM_MARKER}` is not followed by a `=>` match arm"
        )];
    };
    let Some(arm_open) = (fat_arrow..fc).find(|&i| chars[i] == '{') else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: `{ARM_MARKER} … =>` has no braced arm body this scanner can \
             delimit"
        )];
    };
    let Some((ao, ac)) = innermost_span(&spans, arm_open + 1).filter(|&(o, _)| o == arm_open)
    else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: could not brace-match the `{ARM_MARKER}` arm body"
        )];
    };

    let body = slice_chars(&masked, ao + 1, ac);
    if body.contains(REQUIRED_EVENT) {
        Vec::new()
    } else {
        vec![format!(
            "{MAIN_RS_REL_PATH}: the `{ARM_MARKER}` match arm inside `{FN_MARKER}` no longer \
             raises `{REQUIRED_EVENT}` — this reintroduces the meshcadet-room-post-no-\
             notification defect: the drain window closes internally (the classifier's own \
             state flips to \"not draining\") but nobody is ever told — no badge, no tone, no \
             blink"
        )]
    }
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

/// Marker for `main.rs`'s OTHER call site of `RoomSyncPhase::
/// note_closer_failed` — the keep-alive-stall-invalidation branch inside the
/// room keep-alive scheduler. Unlike `handle_room_push_frame`'s call site
/// (scanned by [`check_source`] above), this one carries no post of its own;
/// `meshcadet-room-post-still-no-notify-hil` taught `note_closer_failed`
/// itself to return `Some(Aggregate { count })` directly whenever a backlog
/// was already silently absorbed at the moment the closer is confirmed
/// dead — a HIL capture ("Test 6") proved the alternative (deferring the
/// flush to a next post that never arrived) swallows the notification
/// forever, not merely delays it. This scanner pins that this call site's
/// `if let Some(Aggregate)` body raises the same `UiEvent::RoomDrainComplete`
/// [`check_source`] already pins for the per-post call site, so a future
/// refactor of this branch can't silently drop it again.
const CLOSER_FAILED_CALL_MARKER: &str = "note_closer_failed()";

/// Scan already-read source text and return every contract violation for the
/// [`CLOSER_FAILED_CALL_MARKER`] call site. Split from [`check_closer_failed_
/// wiring`] so tests can drive it with synthetic sources, mirroring
/// [`check_source`]'s own split.
pub fn check_closer_failed_wiring_source(src: &str) -> Vec<String> {
    let masked = tokenize(src).masked;
    let chars: Vec<char> = masked.chars().collect();
    let spans = brace_spans(&masked);

    let call_hits = find_all(&chars, CLOSER_FAILED_CALL_MARKER);
    let call_pos = match call_hits.len() {
        1 => call_hits[0],
        0 => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: no `{CLOSER_FAILED_CALL_MARKER}` call found — the \
                 keep-alive-stall-invalidation call site was renamed, removed, or moved, or \
                 this scanner needs updating"
            )]
        }
        n => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: {n} occurrences of `{CLOSER_FAILED_CALL_MARKER}` \
                 (expected exactly one) — this scanner cannot tell which is the call site"
            )]
        }
    };
    // Walk forward from the call (not backward from the pattern) — the
    // `if let Some(RoomNotification::Aggregate { count })` pattern preceding
    // this call has a brace of its own (`{ count }`) that a naive nearest-
    // brace search could mistake for the body open; the call itself has no
    // braces, so the first `{` found AFTER it is unambiguously the `if let`
    // body (mirrors `check_source`'s identical `fat_arrow`-then-`{` walk).
    let Some(body_open) = (call_pos..chars.len()).find(|&i| chars[i] == '{') else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: `{CLOSER_FAILED_CALL_MARKER}` has no braced `if let` body this \
             scanner can delimit"
        )];
    };
    let Some((bo, bc)) = innermost_span(&spans, body_open + 1).filter(|&(o, _)| o == body_open)
    else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: could not brace-match the `{CLOSER_FAILED_CALL_MARKER}` `if \
             let` body"
        )];
    };

    let body = slice_chars(&masked, bo + 1, bc);
    if body.contains(REQUIRED_EVENT) {
        Vec::new()
    } else {
        vec![format!(
            "{MAIN_RS_REL_PATH}: the `{CLOSER_FAILED_CALL_MARKER}` call site no longer raises \
             `{REQUIRED_EVENT}` when it returns `Some(Aggregate)` — this reintroduces the \
             meshcadet-room-post-still-no-notify-hil defect: a backlog already absorbed when \
             the closer is confirmed dead is flushed internally (the classifier's own state \
             flips to \"not draining\") but nobody is ever told — no badge, no tone, no blink"
        )]
    }
}

/// Read `firmware/src/main.rs` under `repo_root` and return every contract
/// violation for the [`CLOSER_FAILED_CALL_MARKER`] call site. Empty vec ==
/// the contract holds.
pub fn check_closer_failed_wiring(repo_root: &Path) -> Vec<String> {
    let path = repo_root.join(MAIN_RS_REL_PATH);
    match fs::read_to_string(&path) {
        Ok(src) => check_closer_failed_wiring_source(&src),
        Err(e) => vec![format!("reading {}: {e}", path.display())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual guard: the shipped `firmware/src/main.rs` raises
    /// `UiEvent::RoomDrainComplete` from the `Aggregate` arm.
    #[test]
    fn room_aggregate_notification_wiring_holds() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "room aggregate-notification wiring violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    const WIRED_BASELINE: &str = r#"
        fn handle_room_push_frame() {
            match notification {
                RoomNotification::None => {}
                RoomNotification::Live => {}
                RoomNotification::Aggregate { count } => {
                    ui_events.push(UiEvent::RoomPostDrained {
                        room_hash: room.hash,
                        text: display_text,
                    });
                    ui_events.push(UiEvent::RoomDrainComplete {
                        room_hash: room.hash,
                        count,
                    });
                }
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

    /// Models the EXACT pre-fix defect: the `Aggregate` arm does nothing.
    #[test]
    fn synthetic_no_op_arm_is_caught() {
        let synthetic = r#"
            fn handle_room_push_frame() {
                match notification {
                    RoomNotification::None => {}
                    RoomNotification::Live => {}
                    RoomNotification::Aggregate { .. } => {
                        // Never produced by on_push_outcome — unreachable here.
                    }
                }
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no longer raises `UiEvent::RoomDrainComplete`")),
            "a no-op Aggregate arm must be caught: {violations:?}"
        );
    }

    #[test]
    fn a_missing_function_is_a_violation_not_a_silent_pass() {
        let violations = check_source("fn main() {}");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no `fn handle_room_push_frame` found")),
            "{violations:?}"
        );
    }

    #[test]
    fn a_missing_arm_is_a_violation_not_a_silent_pass() {
        let synthetic = r#"
            fn handle_room_push_frame() {
                match notification {
                    RoomNotification::None => {}
                    RoomNotification::Live => {}
                }
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no `RoomNotification::Aggregate` match arm found")),
            "{violations:?}"
        );
    }

    /// A comment mentioning the required event must not be mistaken for the
    /// real call (the tokenizer blanks comment bodies first).
    #[test]
    fn comment_mentions_are_not_counted() {
        let synthetic = r#"
            fn handle_room_push_frame() {
                match notification {
                    RoomNotification::None => {}
                    RoomNotification::Live => {}
                    RoomNotification::Aggregate { .. } => {
                        // TODO: raise UiEvent::RoomDrainComplete here.
                    }
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

    // ── `note_closer_failed()`'s OWN call site (the keep-alive-stall-
    // invalidation branch, no post of its own) ─────────────────────────────

    /// The actual guard: the shipped `firmware/src/main.rs` raises
    /// `UiEvent::RoomDrainComplete` from the `note_closer_failed()` call
    /// site's `if let Some(Aggregate)` body too.
    #[test]
    fn closer_failed_wiring_holds() {
        let violations = check_closer_failed_wiring(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "closer-failed aggregate-notification wiring violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    const CLOSER_FAILED_WIRED_BASELINE: &str = r#"
        fn main() {
            if let Some(RoomNotification::Aggregate { count }) =
                room.sync_phase.note_closer_failed()
            {
                if let Some(ref mut ui) = ui_opt {
                    ui.post_event(UiEvent::RoomDrainComplete {
                        room_hash: room.hash,
                        count,
                    });
                }
            }
        }
    "#;

    #[test]
    fn synthetic_closer_failed_wired_baseline_is_clean() {
        assert!(
            check_closer_failed_wiring_source(CLOSER_FAILED_WIRED_BASELINE).is_empty(),
            "{:?}",
            check_closer_failed_wiring_source(CLOSER_FAILED_WIRED_BASELINE)
        );
    }

    /// Models the exact regression this guard exists to catch: the call
    /// happens, the classifier correctly flips state, and nobody is told.
    #[test]
    fn synthetic_closer_failed_no_op_body_is_caught() {
        let synthetic = r#"
            fn main() {
                if let Some(RoomNotification::Aggregate { count }) =
                    room.sync_phase.note_closer_failed()
                {
                    // Never wired to the UI — the regression this scanner exists to catch.
                }
            }
        "#;
        let violations = check_closer_failed_wiring_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no longer raises `UiEvent::RoomDrainComplete`")),
            "a no-op closer-failed body must be caught: {violations:?}"
        );
    }

    #[test]
    fn closer_failed_missing_call_is_a_violation_not_a_silent_pass() {
        let violations = check_closer_failed_wiring_source("fn main() {}");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no `note_closer_failed()` call found")),
            "{violations:?}"
        );
    }

    /// A comment mentioning the required event must not be mistaken for the
    /// real call.
    #[test]
    fn closer_failed_comment_mentions_are_not_counted() {
        let synthetic = r#"
            fn main() {
                if let Some(RoomNotification::Aggregate { count }) =
                    room.sync_phase.note_closer_failed()
                {
                    // TODO: raise UiEvent::RoomDrainComplete here.
                }
            }
        "#;
        let violations = check_closer_failed_wiring_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no longer raises `UiEvent::RoomDrainComplete`")),
            "a comment-only mention must not satisfy the guard: {violations:?}"
        );
    }
}
