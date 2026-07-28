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
//! | `RoomPostSent`     | yes             | **no**       | **no**             |
//! | `RoomPostRefused`  | yes             | **no**       | **no**             |
//!
//! The last two rows are `meshcadet-room-post-refusal-surface`'s own —
//! confirmation/refusal of a room post the user just sent themselves is
//! content the sender already knows about, not an *incoming* interruption:
//! it must append (so the user sees the outcome) but never bump unread or
//! fire the notification model. Pinning both here means the "non-alarming"
//! half of that mission's Objective is mutation-tested for free by this
//! table's existing generic machinery, on top of the dedicated sibling guard
//! below for the phantom-bubble half.
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
//!
//! # Sibling guard: no phantom "sent" bubble for a refused room post
//!
//! `meshcadet-room-post-refusal-surface` added a second, unrelated
//! invariant pinned in this same module (same file under scan, same
//! "structural scan, no esp toolchain" rationale):
//! `UiRuntime::on_send_message`'s room-post branch — the one that queues
//! `UiCommand::SendRoomPost` — must never itself construct a
//! `MessageRecord`. A room post can be refused post-hoc by the dispatcher's
//! monotonic-timestamp gate (`main.rs`'s handling of that command,
//! `room_session::encode_room_post_checked`); pushing an optimistic bubble
//! before that gate runs left a phantom "sent" message with nothing behind
//! it on the refusal path. See
//! [`check_no_optimistic_room_post_bubble`]'s doc for the mechanics.
//!
//! The corollary of failing loud is that a legitimate refactor — hoisting an
//! arm's body into a helper function, say — will trip it. That is the
//! intended trade: a false alarm you resolve by teaching this scanner the new
//! shape is strictly better than a guard that quietly stops looking. Update
//! the effect table and the matchers together when the arms move.

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
    (
        "RoomPostSent",
        NotificationEffects {
            appends_content: true,
            bumps_unread: false,
            fires_notification: false,
        },
    ),
    (
        "RoomPostRefused",
        NotificationEffects {
            appends_content: true,
            bumps_unread: false,
            fires_notification: false,
        },
    ),
];

/// The two arms that must behave identically — the parity claim itself.
const PARITY_PAIR: (&str, &str) = ("RoomPostLive", "IncomingGroupMsg");

/// `meshcadet-room-session-state-to-ui`'s F1 fix: a room's session
/// permission, learned at runtime (e.g. a Guest→ReadWrite login), only ever
/// reaches the UI if this arm re-registers it. Before that mission,
/// `register_room` was called exactly once, at boot, off the resumed
/// session — a later permission upgrade left `room_permissions` stuck at
/// the stale boot-time value with no code path able to correct it. This is
/// a narrower, single-call-presence check rather than a full effects table
/// (unlike [`EXPECTED`] above) because the arm has exactly one job; the same
/// "structural scan over an un-host-testable crate" rationale as the rest of
/// this module applies.
const ROOM_PERMISSION_UPDATED_VARIANT: &str = "RoomPermissionUpdated";
const ROOM_PERMISSION_UPDATED_REQUIRED_CALL: &str = "self.register_room(";

/// `meshcadet-room-post-refusal-surface`'s regression guard: a room post can
/// be refused post-hoc by the dispatcher's monotonic-timestamp gate
/// (`main.rs`'s handling of `UiCommand::SendRoomPost`,
/// `room_session::encode_room_post_checked`) — reachable roughly a coin-flip
/// of the time on a GPS-unsynced boot (see that mission's Objective for the
/// full reachability argument). Before the fix, `on_send_message` pushed an
/// optimistic "sent" `MessageRecord` unconditionally, before that gate ever
/// ran, so a refusal left a phantom bubble: transmitted nowhere, persisted
/// nowhere, gone on reboot with no explanation. The fix moved the bubble to
/// only ever be rendered once the dispatcher confirms the encode actually
/// succeeded (`UiEvent::RoomPostSent`); this pins that `on_send_message`'s
/// room-post branch — the block that queues `UiCommand::SendRoomPost` —
/// never itself constructs a `MessageRecord` ahead of that confirmation.
const ON_SEND_MESSAGE_FN_MARKER: &str = "fn on_send_message";
const ROOM_POST_COMMAND_MARKER: &str = "UiCommand::SendRoomPost";

/// `meshcadet-room-readonly-refusal-surface-v2`'s regression guard: the
/// defense-in-depth read-only recheck inside `on_send_message` (`if let
/// Some(&can_post) = self.room_permissions.get(&hash) { if !can_post { … }
/// }`) used to drop the composed post with only a `log::warn!` and a bare
/// `return` — silently losing the user's typed text, the exact failure mode
/// `UiEvent::RoomPostRefused` exists to prevent (per its own doc comment).
/// This pins that the `!can_post` arm raises `RoomPostRefused` via
/// `post_event` instead of returning silently.
const ROOM_PERMISSIONS_GET_MARKER: &str = "self.room_permissions.get(&hash)";
const ROOM_POST_REFUSED_EVENT_MARKER: &str = "UiEvent::RoomPostRefused";
const POST_EVENT_CALL_MARKER: &str = "self.post_event(";

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

/// Is the `UiEvent::<variant>` occurrence starting at `end` (the index right
/// after the variant name) a match-arm PATTERN (`UiEvent::Foo { .. } => {`
/// or, for a unit variant, `UiEvent::Foo => {`), as opposed to a value
/// CONSTRUCTION expression (`UiEvent::Foo { .. }` used to build a value, e.g.
/// inside `self.post_event(UiEvent::Foo { .. })`)? Both share the identical
/// `UiEvent::<variant> { .. }` prefix, so `find_all` alone cannot tell them
/// apart — this walks past the (possibly braced) pattern and checks whether
/// `=>` immediately follows, which only a match arm has.
fn is_match_arm_hit(chars: &[char], end: usize, spans: &[(usize, usize)]) -> bool {
    let mut i = end;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    if i < chars.len() && chars[i] == '{' {
        match innermost_span(spans, i + 1) {
            Some((o, c)) if o == i => {
                let mut j = c + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                j + 1 < chars.len() && chars[j] == '=' && chars[j + 1] == '>'
            }
            _ => false,
        }
    } else {
        i + 1 < chars.len() && chars[i] == '=' && chars[i + 1] == '>'
    }
}

/// Extract the body of the `UiEvent::<variant> { … } => { BODY }` match arm
/// from already-tokenized (comment- and string-blanked) source.
///
/// Returns `Err` rather than `None`-and-carry-on for every ambiguity: zero
/// hits, more than one hit, or a hit that isn't followed by `=> {`. A doc
/// comment mentioning `[UiEvent::RoomDrainComplete]` is not a hit, because
/// `masked` has had every comment body blanked to spaces before we look. A
/// value-construction expression elsewhere in the file (e.g.
/// `self.post_event(UiEvent::RoomPostRefused { .. })`) is likewise not a hit —
/// see [`is_match_arm_hit`].
fn arm_body(masked: &str, variant: &str) -> Result<String, String> {
    let chars: Vec<char> = masked.chars().collect();
    let all_hits = find_all(&chars, &format!("UiEvent::{variant}"));
    let spans = brace_spans(masked);
    let variant_end_offset = format!("UiEvent::{variant}").chars().count();
    let hits: Vec<usize> = all_hits
        .iter()
        .copied()
        .filter(|&start| is_match_arm_hit(&chars, start + variant_end_offset, &spans))
        .collect();
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

/// Extract the full body of a named function (`fn <name_marker>(...) { BODY
/// }`) from already-tokenized (comment/string-blanked) source. Same
/// ambiguity handling as [`arm_body`] above: exactly one hit, or fail loud
/// rather than guess.
fn fn_body(masked: &str, fn_name_marker: &str) -> Result<String, String> {
    let chars: Vec<char> = masked.chars().collect();
    let hits = find_all(&chars, fn_name_marker);
    match hits.len() {
        1 => {}
        0 => {
            return Err(format!(
                "{UI_MOD_REL_PATH}: no `{fn_name_marker}` found — the function was renamed, \
                 moved, or deleted, and this scanner needs updating"
            ))
        }
        n => {
            return Err(format!(
                "{UI_MOD_REL_PATH}: {n} occurrences of `{fn_name_marker}` (expected exactly \
                 one) — this scanner cannot tell which is the function"
            ))
        }
    }
    let start = hits[0];
    let mut open = start;
    while open < chars.len() && chars[open] != '{' {
        open += 1;
    }
    if open >= chars.len() {
        return Err(format!(
            "{UI_MOD_REL_PATH}: `{fn_name_marker}` has no braced body this scanner can delimit"
        ));
    }
    let spans = brace_spans(masked);
    let (o, c) = innermost_span(&spans, open + 1)
        .ok_or_else(|| format!("{UI_MOD_REL_PATH}: unbalanced braces around `{fn_name_marker}`"))?;
    if o != open {
        return Err(format!(
            "{UI_MOD_REL_PATH}: could not delimit `{fn_name_marker}`'s body"
        ));
    }
    Ok(slice_chars(masked, o + 1, c))
}

/// `meshcadet-room-post-refusal-surface`'s regression guard — see
/// [`ON_SEND_MESSAGE_FN_MARKER`]'s doc for the invariant. Locates
/// `on_send_message`'s body, finds the single `UiCommand::SendRoomPost`
/// command push inside it, and asserts the innermost brace block enclosing
/// that push (the room-post branch itself) never constructs a
/// `MessageRecord`.
fn check_no_optimistic_room_post_bubble(masked: &str) -> Vec<String> {
    let body = match fn_body(masked, ON_SEND_MESSAGE_FN_MARKER) {
        Err(e) => return vec![e],
        Ok(b) => b,
    };
    let body_chars: Vec<char> = body.chars().collect();
    let hits = find_all(&body_chars, ROOM_POST_COMMAND_MARKER);
    if hits.len() != 1 {
        return vec![format!(
            "{UI_MOD_REL_PATH}: expected exactly one `{ROOM_POST_COMMAND_MARKER}` inside \
             `on_send_message` (found {}) — this scanner cannot locate the room-post branch",
            hits.len()
        )];
    }
    let spans = brace_spans(&body);
    match innermost_span(&spans, hits[0]) {
        None => vec![format!(
            "{UI_MOD_REL_PATH}: could not delimit the room-post branch enclosing \
             `{ROOM_POST_COMMAND_MARKER}` inside `on_send_message`"
        )],
        Some((o, c)) => {
            let branch = slice_chars(&body, o + 1, c);
            if branch.contains("MessageRecord") {
                vec![format!(
                    "{UI_MOD_REL_PATH}: `on_send_message`'s room-post branch constructs a \
                     `MessageRecord` ahead of the dispatcher's send-eligibility checks — this \
                     reintroduces the phantom-sent-bubble defect fixed by \
                     meshcadet-room-post-refusal-surface (a refused room post must leave no \
                     record in `self.messages`)"
                )]
            } else {
                Vec::new()
            }
        }
    }
}

/// `meshcadet-room-readonly-refusal-surface-v2`'s regression guard — see
/// [`ROOM_PERMISSIONS_GET_MARKER`]'s doc for the invariant. Locates
/// `on_send_message`'s body, finds the single `self.room_permissions.get(&hash)`
/// re-check inside it, and asserts the innermost brace block enclosing that
/// call (the `if let Some(&can_post) = …` guard, including its nested `if
/// !can_post` arm) raises `UiEvent::RoomPostRefused` through `self.post_event(`
/// rather than only logging and returning.
fn check_readonly_guard_surfaces_refusal(masked: &str) -> Vec<String> {
    let body = match fn_body(masked, ON_SEND_MESSAGE_FN_MARKER) {
        Err(e) => return vec![e],
        Ok(b) => b,
    };
    let body_chars: Vec<char> = body.chars().collect();
    let hits = find_all(&body_chars, ROOM_PERMISSIONS_GET_MARKER);
    if hits.len() != 1 {
        return vec![format!(
            "{UI_MOD_REL_PATH}: expected exactly one `{ROOM_PERMISSIONS_GET_MARKER}` inside \
             `on_send_message` (found {}) — this scanner cannot locate the read-only \
             defense-in-depth guard",
            hits.len()
        )];
    }
    // `ROOM_PERMISSIONS_GET_MARKER` sits in the `if let … = <marker> {` guard's
    // CONDITION, i.e. before that block's own opening brace — unlike
    // `check_no_optimistic_room_post_bubble`'s marker, which sits inside the
    // branch it delimits. Walk forward to that brace first, then delimit the
    // block it opens (which encloses the nested `if !can_post { … }` arm).
    let mut open = hits[0];
    while open < body_chars.len() && body_chars[open] != '{' {
        open += 1;
    }
    if open >= body_chars.len() {
        return vec![format!(
            "{UI_MOD_REL_PATH}: `{ROOM_PERMISSIONS_GET_MARKER}` inside `on_send_message` has no \
             braced guard block this scanner can delimit"
        )];
    }
    let spans = brace_spans(&body);
    match innermost_span(&spans, open + 1) {
        None => vec![format!(
            "{UI_MOD_REL_PATH}: could not delimit the read-only guard block enclosing \
             `{ROOM_PERMISSIONS_GET_MARKER}` inside `on_send_message`"
        )],
        Some((o, c)) if o == open => {
            let branch = slice_chars(&body, o + 1, c);
            if branch.contains(ROOM_POST_REFUSED_EVENT_MARKER)
                && branch.contains(POST_EVENT_CALL_MARKER)
            {
                Vec::new()
            } else {
                vec![format!(
                    "{UI_MOD_REL_PATH}: `on_send_message`'s read-only defense-in-depth guard \
                     no longer raises `{ROOM_POST_REFUSED_EVENT_MARKER}` via \
                     `{POST_EVENT_CALL_MARKER}` — a room-post send blocked here would silently \
                     drop the user's typed text with no on-screen explanation, reintroducing the \
                     meshcadet-room-readonly-refusal-surface-v2 defect"
                )]
            }
        }
        Some(_) => vec![format!(
            "{UI_MOD_REL_PATH}: could not delimit the read-only guard block enclosing \
             `{ROOM_PERMISSIONS_GET_MARKER}` inside `on_send_message`"
        )],
    }
}

/// Classify one arm body's notification-surface actions.
///
/// Matching is on the two identifiers that carry each action's *meaning*
/// rather than one long formatted call expression, so a rustfmt line-wrap
/// can't turn a real behaviour change into a false pass (or a reformat into
/// a false failure).
fn effects_of(body: &str) -> NotificationEffects {
    NotificationEffects {
        appends_content: body.contains("self.messages") && body.contains("MessageRecord"),
        bumps_unread: body.contains("incoming_message_is_unread") && body.contains("unread.entry("),
        fires_notification: body.contains("notif.fire(") && body.contains("NotifEvent::"),
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

    // F1's session-state-reaches-the-UI contract: see
    // `ROOM_PERMISSION_UPDATED_VARIANT`'s doc.
    match arm_body(&masked, ROOM_PERMISSION_UPDATED_VARIANT) {
        Err(e) => violations.push(e),
        Ok(body) => {
            if !body.contains(ROOM_PERMISSION_UPDATED_REQUIRED_CALL) {
                violations.push(format!(
                    "{UI_MOD_REL_PATH}: `UiEvent::{ROOM_PERMISSION_UPDATED_VARIANT}` arm does \
                     not call `register_room` — a room's runtime-learned session permission \
                     (e.g. a Guest→ReadWrite login) would no longer reach the UI, reintroducing \
                     the `meshcadet-room-session-state-to-ui` F1 defect"
                ));
            }
        }
    }

    // `meshcadet-room-post-refusal-surface`'s regression guard: no
    // room-post send path may push a `MessageRecord` ahead of the
    // dispatcher's send-eligibility checks — see
    // `check_no_optimistic_room_post_bubble`'s doc.
    violations.extend(check_no_optimistic_room_post_bubble(&masked));

    // `meshcadet-room-readonly-refusal-surface-v2`'s regression guard: the
    // read-only defense-in-depth recheck must surface `RoomPostRefused`
    // rather than silently dropping the composed text — see
    // `check_readonly_guard_surfaces_refusal`'s doc.
    violations.extend(check_readonly_guard_surfaces_refusal(&masked));

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

    /// A minimal stand-in for `on_send_message`'s room-post branch, used by
    /// [`synthetic`] below to feed [`check_no_optimistic_room_post_bubble`]'s
    /// and [`check_readonly_guard_surfaces_refusal`]'s mutation tests.
    /// `phantom_bubble: true` reproduces the original
    /// `meshcadet-room-post-refusal-surface` defect (an optimistic
    /// `MessageRecord` pushed before `UiCommand::SendRoomPost` is queued).
    /// `readonly_guard_silent: true` reproduces the
    /// `meshcadet-room-readonly-refusal-surface-v2` defect (the read-only
    /// recheck logs and returns with no `RoomPostRefused`).
    fn synthetic_on_send_message(phantom_bubble: bool, readonly_guard_silent: bool) -> String {
        let phantom_push = if phantom_bubble {
            r#"self.messages.entry(hash).or_default().push(MessageRecord { text: text.clone(), is_ours: true, acked: false, ts_ms: 0 });"#
        } else {
            ""
        };
        let readonly_arm = if readonly_guard_silent {
            r#"log::warn!("ui: compose send blocked — room 0x{:02x} is read-only", hash);
                    return;"#
        } else {
            r#"log::warn!("ui: compose send blocked — room 0x{:02x} is read-only", hash);
                    self.post_event(UiEvent::RoomPostRefused {
                        room_hash: hash,
                        reason: "this room is now read-only for your session".to_string(),
                    });
                    return;"#
        };
        format!(
            r#"
            fn on_send_message(&mut self, hash: u8, is_channel: bool, raw_text: String) {{
                if let Some(&can_post) = self.room_permissions.get(&hash) {{
                    if !can_post {{
                        {readonly_arm}
                    }}
                }}
                if self.room_permissions.contains_key(&hash) {{
                    {phantom_push}
                    self.commands.push(UiCommand::SendRoomPost {{ room_hash: hash, text }});
                }} else {{
                    self.messages.entry(hash).or_default().push(MessageRecord {{
                        text: text.clone(), is_ours: true, acked: false, ts_ms: 0,
                    }});
                    self.commands.push(UiCommand::SendDm {{ to_hash: hash, text }});
                }}
            }}
            "#
        )
    }

    /// A minimal stand-in for `handle_event`'s five arms plus
    /// `on_send_message`'s room-post branch, used by the mutation tests
    /// below to prove this scanner has teeth. `room_post_phantom_bubble` and
    /// `readonly_guard_silent` drive [`synthetic_on_send_message`] — both
    /// `false` everywhere except the dedicated mutation test for each guard.
    fn synthetic(
        live_fires_notification: bool,
        drained_fires_notification: bool,
        registers_permission: bool,
        room_post_phantom_bubble: bool,
        readonly_guard_silent: bool,
    ) -> String {
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
        let register_call = if registers_permission {
            "self.register_room(room_hash, can_post);"
        } else {
            ""
        };
        let on_send_message =
            synthetic_on_send_message(room_post_phantom_bubble, readonly_guard_silent);
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
                UiEvent::RoomPermissionUpdated {{ room_hash, can_post }} => {{
                    {register_call}
                }}
                UiEvent::RoomPostSent {{ room_hash, text }} => {{
                    // Confirmation of the user's own send: append only, no
                    // unread bump, no notif.fire — see [`EXPECTED`]'s doc.
                    self.messages.entry(room_hash).or_default().push(MessageRecord {{ text }});
                }}
                UiEvent::RoomPostRefused {{ room_hash, reason }} => {{
                    // Refusal of the user's own send: append only, no
                    // unread bump, no notif.fire — see [`EXPECTED`]'s doc.
                    self.messages.entry(room_hash).or_default().push(MessageRecord {{ text: reason }});
                }}
            }}

            {on_send_message}
            "#
        )
    }

    #[test]
    fn synthetic_baseline_is_clean() {
        assert_eq!(
            check_source(&synthetic(true, false, true, false, false)),
            Vec::<String>::new()
        );
    }

    /// The mutation this sibling guard exists for: `on_send_message`'s
    /// room-post branch pushes an optimistic `MessageRecord` before
    /// `UiCommand::SendRoomPost` is even queued — the exact
    /// `meshcadet-room-post-refusal-surface` defect (a refusal downstream
    /// leaves a phantom "sent" bubble with nothing behind it).
    #[test]
    fn optimistic_room_post_bubble_is_caught() {
        let violations = check_source(&synthetic(true, false, true, true, false));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("constructs a `MessageRecord` ahead of")),
            "expected the phantom-bubble violation, got {violations:?}"
        );
    }

    /// The mutation this guard exists for: `RoomPostLive` silently stops
    /// notifying, and a live room post becomes invisible while the device is
    /// asleep. Must be caught twice over — the contract row AND the parity
    /// comparison.
    #[test]
    fn dropping_the_live_arms_notification_is_caught() {
        let violations = check_source(&synthetic(false, false, true, false, false));
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
        let violations = check_source(&synthetic(true, true, true, false, false));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("RoomPostDrained` arm does")),
            "expected a contract-row violation, got {violations:?}"
        );
    }

    /// `meshcadet-room-session-state-to-ui`'s F1 mutation: `RoomPermissionUpdated`
    /// stops calling `register_room`, silently reintroducing "session
    /// upgrade never reaches the UI".
    #[test]
    fn dropping_the_register_room_call_is_caught() {
        let violations = check_source(&synthetic(true, false, false, false, false));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("RoomPermissionUpdated` arm does not call `register_room`")),
            "expected a register_room-missing violation, got {violations:?}"
        );
    }

    /// The mutation `meshcadet-room-readonly-refusal-surface-v2` exists for:
    /// the read-only defense-in-depth recheck inside `on_send_message` logs
    /// and returns silently, dropping the user's composed text with no
    /// on-screen trace instead of raising `UiEvent::RoomPostRefused`.
    #[test]
    fn silent_readonly_guard_drop_is_caught() {
        let violations = check_source(&synthetic(true, false, true, false, true));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no longer raises `UiEvent::RoomPostRefused`")),
            "expected the silent-drop violation, got {violations:?}"
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
        let src = synthetic(true, false, true, false, false);
        assert!(
            src.contains("[`UiEvent::RoomPostLive`]"),
            "fixture must actually contain a doc-comment mention"
        );
        assert_eq!(check_source(&src), Vec::<String>::new());
    }
}
