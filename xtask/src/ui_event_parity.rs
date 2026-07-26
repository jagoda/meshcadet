// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard for the room-post notification-surface contract
//! in `firmware/src/ui/mod.rs`'s `UiRuntime::handle_event`.
//!
//! # Why this lives in `xtask` and not in `firmware`
//!
//! Same reason the glyph harness does (see this crate's module doc): the
//! `firmware` crate's single `[[bin]]` target sets `harness = false`, so
//! `cargo test` only *type-checks* its `#[cfg(test)]` blocks and never
//! executes one — a fact `firmware/src/ui/mod.rs` documents at its own
//! `mod tests`. A `#[test]` asserting what `handle_event` does therefore
//! cannot exist in that crate. This module is the host-runnable equivalent,
//! in the same "plain text scanning, no esp toolchain" spirit.
//!
//! # The invariant being pinned
//!
//! Milestone 2 of the room-server work (`meshcadet-room-firmware-post-and-
//! notify`, Phase D) split one incoming room push into three distinct
//! notification-surface behaviours, and the whole point of the split is that
//! a **live** room post is indistinguishable from a channel message:
//!
//! | `UiEvent` arm      | appends content | bumps unread | fires notification |
//! |--------------------|-----------------|--------------|--------------------|
//! | `IncomingGroupMsg` | yes             | yes          | yes                |
//! | `RoomPostLive`     | yes             | yes          | yes                |
//! | `RoomPostDrained`  | yes             | **no**       | **no**             |
//! | `RoomDrainComplete`| **no**          | yes          | yes                |
//!
//! `RoomPostLive`'s row is required to equal `IncomingGroupMsg`'s row — that
//! equality *is* the "full parity with the channel path" claim, which until
//! this guard existed was asserted only by a code comment.
//!
//! The other three rows matter just as much in the other direction: if
//! `RoomPostDrained` ever grows a `notif.fire`, the 32-post login backlog
//! goes back to storming the notification tray — the exact defect Phase D
//! was written to prevent, and one that `firmware_core::room_session`'s own
//! tests cannot catch, because they pin the *classifier* (which
//! `RoomNotification` a push maps to), not what the UI then does with it.
//!
//! # Scope and honest limits
//!
//! This is a **structural** check, not a behavioural one: it asserts that
//! each arm's body does or does not perform each of three recognisable
//! actions. It cannot prove the actions are *correct* (that the unread count
//! lands on the right key, say) — only that no arm silently loses or gains
//! one. It deliberately fails loud (a reported violation, never a silent
//! skip) if an arm cannot be located or parsed at all, per this crate's
//! "parse gap = NO-GO" doctrine.

use std::fs;
use std::path::Path;

use crate::{brace_spans, innermost_span, slice_chars, tokenize};

/// Path, relative to the repo root, of the file this module scans.
pub const UI_MOD_REL_PATH: &str = "firmware/src/ui/mod.rs";

/// What one `UiEvent` match arm does to the notification surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationEffects {
    /// Pushes a `MessageRecord` into `self.messages` — the post's text
    /// reaches the thread / list preview.
    pub appends_content: bool,
    /// Increments `self.unread`, gated by `incoming_message_is_unread`.
    pub bumps_unread: bool,
    /// Fires the notification model (`notif.fire(NotifEvent::…)`) — buzzer /
    /// LED / wake, i.e. an actual user interruption.
    pub fires_notification: bool,
}

impl std::fmt::Display for NotificationEffects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "append={} unread={} notify={}",
            self.appends_content, self.bumps_unread, self.fires_notification
        )
    }
}

/// The contract table from this module's doc comment. Ordered so
/// `IncomingGroupMsg` (the reference behaviour) comes first.
const EXPECTED: &[(&str, NotificationEffects)] = &[
    (
        "IncomingGroupMsg",
        NotificationEffects {
            appends_content: true,
            bumps_unread: true,
            fires_notification: true,
        },
    ),
    (
        "RoomPostLive",
        NotificationEffects {
            appends_content: true,
            bumps_unread: true,
            fires_notification: true,
        },
    ),
    (
        "RoomPostDrained",
        NotificationEffects {
            appends_content: true,
            bumps_unread: false,
            fires_notification: false,
        },
    ),
    (
        "RoomDrainComplete",
        NotificationEffects {
            appends_content: false,
            bumps_unread: true,
            fires_notification: true,
        },
    ),
];

/// The two arms that must behave identically — the parity claim itself.
const PARITY_PAIR: (&str, &str) = ("RoomPostLive", "IncomingGroupMsg");

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

fn contains(hay: &str, needle: &str) -> bool {
    hay.contains(needle)
}

/// Extract the body of the `UiEvent::<variant> { … } => { BODY }` match arm
/// from already-tokenized (comment- and string-blanked) source.
///
/// Returns `Err` rather than `None`-and-carry-on for every ambiguity: zero
/// hits, more than one hit, or a hit that isn't followed by `=> {`. A doc
/// comment mentioning `[UiEvent::RoomDrainComplete]` is not a hit, because
/// `masked` has had every comment body blanked to spaces before we look.
fn arm_body(masked: &str, variant: &str) -> Result<String, String> {
    let chars: Vec<char> = masked.chars().collect();
    let hits = find_all(&chars, &format!("UiEvent::{variant}"));
    match hits.len() {
        1 => {}
        0 => {
            return Err(format!(
                "{UI_MOD_REL_PATH}: no `UiEvent::{variant}` match arm found — the arm was \
                 renamed or deleted, or this scanner needs updating"
            ))
        }
        n => {
            return Err(format!(
                "{UI_MOD_REL_PATH}: {n} occurrences of `UiEvent::{variant}` in code (expected \
                 exactly one match arm) — this scanner cannot tell which is the arm"
            ))
        }
    }
    let start = hits[0];

    // Walk forward to the arm's `=> {`. A `;` before it means we landed on a
    // statement, not a match pattern.
    let mut i = start;
    while i + 1 < chars.len() {
        if chars[i] == ';' {
            return Err(format!(
                "{UI_MOD_REL_PATH}: `UiEvent::{variant}` is not followed by a `=>` match arm"
            ));
        }
        if chars[i] == '=' && chars[i + 1] == '>' {
            break;
        }
        i += 1;
    }
    // Then to the `{` that opens the arm body.
    let mut open = i;
    while open < chars.len() && chars[open] != '{' {
        open += 1;
    }
    if open >= chars.len() {
        return Err(format!(
            "{UI_MOD_REL_PATH}: `UiEvent::{variant} => …` has no braced body this scanner can \
             delimit"
        ));
    }

    let spans = brace_spans(masked);
    let (o, c) = innermost_span(&spans, open + 1).ok_or_else(|| {
        format!("{UI_MOD_REL_PATH}: unbalanced braces around the `UiEvent::{variant}` arm")
    })?;
    if o != open {
        return Err(format!(
            "{UI_MOD_REL_PATH}: could not delimit the `UiEvent::{variant}` arm body"
        ));
    }
    Ok(slice_chars(masked, o + 1, c))
}

/// Classify one arm body's notification-surface actions.
///
/// Matching is on the two identifiers that carry each action's *meaning*
/// rather than one long formatted call expression, so a rustfmt line-wrap
/// can't turn a real behaviour change into a false pass (or a reformat into
/// a false failure).
fn effects_of(body: &str) -> NotificationEffects {
    NotificationEffects {
        appends_content: contains(body, "self.messages") && contains(body, "MessageRecord"),
        bumps_unread: contains(body, "incoming_message_is_unread")
            && contains(body, "unread.entry("),
        fires_notification: contains(body, "notif.fire(") && contains(body, "NotifEvent::"),
    }
}

/// Scan already-read source text and return every contract violation.
/// Split from [`check`] so the tests can drive it with synthetic sources.
pub fn check_source(src: &str) -> Vec<String> {
    let masked = tokenize(src).masked;
    let mut violations = Vec::new();
    let mut found: Vec<(&str, NotificationEffects)> = Vec::new();

    for (variant, expected) in EXPECTED {
        match arm_body(&masked, variant) {
            Err(e) => violations.push(e),
            Ok(body) => {
                let actual = effects_of(&body);
                if actual != *expected {
                    violations.push(format!(
                        "{UI_MOD_REL_PATH}: `UiEvent::{variant}` arm does [{actual}] but the \
                         Phase D notification contract requires [{expected}]"
                    ));
                }
                found.push((variant, actual));
            }
        }
    }

    // The parity claim itself, asserted against what the two arms ACTUALLY
    // do — not against `EXPECTED` — so it still fails if someone "fixes" a
    // regression by editing both the code and the table above.
    let (live, reference) = PARITY_PAIR;
    let lookup = |name: &str| found.iter().find(|(v, _)| *v == name).map(|(_, e)| *e);
    if let (Some(l), Some(r)) = (lookup(live), lookup(reference)) {
        if l != r {
            violations.push(format!(
                "{UI_MOD_REL_PATH}: `UiEvent::{live}` [{l}] is NOT at parity with \
                 `UiEvent::{reference}` [{r}] — a live room post must be indistinguishable \
                 from a channel message on the notification surface"
            ));
        }
    }

    violations
}

/// Read `firmware/src/ui/mod.rs` under `repo_root` and return every
/// contract violation. Empty vec == the contract holds.
pub fn check(repo_root: &Path) -> Vec<String> {
    let path = repo_root.join(UI_MOD_REL_PATH);
    match fs::read_to_string(&path) {
        Ok(src) => check_source(&src),
        Err(e) => vec![format!("reading {}: {e}", path.display())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual guard: the shipped `firmware/src/ui/mod.rs` honours the
    /// Phase D notification contract, including `RoomPostLive`'s parity with
    /// `IncomingGroupMsg`.
    #[test]
    fn room_notification_surface_contract_holds() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "room notification-surface contract violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    /// A minimal stand-in for `handle_event`'s four arms, used by the
    /// mutation tests below to prove this scanner has teeth.
    fn synthetic(live_fires_notification: bool, drained_fires_notification: bool) -> String {
        let live_notif = if live_fires_notification {
            "self.notif.fire(NotifEvent::IncomingGroupMsg, now_ms, self.screen_asleep);"
        } else {
            ""
        };
        let drained_notif = if drained_fires_notification {
            "self.notif.fire(NotifEvent::IncomingGroupMsg, now_ms, self.screen_asleep);"
        } else {
            ""
        };
        format!(
            r#"
            // A doc mention of [`UiEvent::RoomPostLive`] must not count as an arm.
            match ev {{
                UiEvent::IncomingGroupMsg {{ channel_hash, text }} => {{
                    self.messages.entry(channel_hash).or_default().push(MessageRecord {{ text }});
                    if incoming_message_is_unread(self.active_convo, channel_hash, true) {{
                        *self.unread.entry(channel_hash).or_insert(0) += 1;
                    }}
                    self.notif.fire(NotifEvent::IncomingGroupMsg, now_ms, self.screen_asleep);
                }}
                UiEvent::RoomPostLive {{ room_hash, text }} => {{
                    self.messages.entry(room_hash).or_default().push(MessageRecord {{ text }});
                    if incoming_message_is_unread(self.active_convo, room_hash, true) {{
                        *self.unread.entry(room_hash).or_insert(0) += 1;
                    }}
                    {live_notif}
                }}
                UiEvent::RoomPostDrained {{ room_hash, text }} => {{
                    // deliberately NO unread bump and NO notif.fire here
                    self.messages.entry(room_hash).or_default().push(MessageRecord {{ text }});
                    {drained_notif}
                }}
                UiEvent::RoomDrainComplete {{ room_hash, count }} => {{
                    if incoming_message_is_unread(self.active_convo, room_hash, true) {{
                        *self.unread.entry(room_hash).or_insert(0) += count;
                    }}
                    self.notif.fire(NotifEvent::IncomingGroupMsg, now_ms, self.screen_asleep);
                }}
            }}
            "#
        )
    }

    #[test]
    fn synthetic_baseline_is_clean() {
        assert_eq!(check_source(&synthetic(true, false)), Vec::<String>::new());
    }

    /// The mutation this guard exists for: `RoomPostLive` silently stops
    /// notifying, and a live room post becomes invisible while the device is
    /// asleep. Must be caught twice over — the contract row AND the parity
    /// comparison.
    #[test]
    fn dropping_the_live_arms_notification_is_caught() {
        let violations = check_source(&synthetic(false, false));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("RoomPostLive` arm does")),
            "expected a contract-row violation, got {violations:?}"
        );
        assert!(
            violations.iter().any(|v| v.contains("NOT at parity with")),
            "expected a parity violation, got {violations:?}"
        );
    }

    /// The mutation in the other direction: the drain window starts
    /// notifying per post again, i.e. the 32-post backlog storms the tray.
    #[test]
    fn a_notification_leaking_into_the_drain_arm_is_caught() {
        let violations = check_source(&synthetic(true, true));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("RoomPostDrained` arm does")),
            "expected a contract-row violation, got {violations:?}"
        );
    }

    /// Parse gaps fail loud rather than passing silently.
    #[test]
    fn a_missing_arm_is_a_violation_not_a_silent_pass() {
        let violations = check_source("match ev { UiEvent::IncomingGroupMsg { } => { } }");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no `UiEvent::RoomPostLive` match arm found")),
            "expected a missing-arm violation, got {violations:?}"
        );
    }

    /// A comment or doc-link mention of a variant must not be mistaken for
    /// its match arm (the tokenizer blanks comment bodies first).
    #[test]
    fn comment_mentions_are_not_counted_as_arms() {
        let src = synthetic(true, false);
        assert!(
            src.contains("[`UiEvent::RoomPostLive`]"),
            "fixture must actually contain a doc-comment mention"
        );
        assert_eq!(check_source(&src), Vec::<String>::new());
    }
}
