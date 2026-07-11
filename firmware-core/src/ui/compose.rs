// SPDX-License-Identifier: GPL-3.0-only
//! Compose screen — pure shortcode-prefix scan for the `:shortcode:` emoji
//! autocomplete, the Return-to-send guard, and the post-send navigation
//! deferral timing.
//!
//! The `slint::slint!{}` view and the `ComposeScreen` Rust wrapper stay in
//! `firmware/src/ui/screens/compose.rs` (they depend on Slint); only the
//! plain-data helpers below move here so their tests execute under `cargo
//! test --workspace` (this crate is a detached, cross-compiled workspace —
//! see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block written there
//! would type-check but never run). See
//! `docs/adr/0005-firmware-core-extraction.md`.

/// Return the in-progress `:shortcode` token: the text after the last `:` in
/// `text`, provided that tail contains only shortcode characters (ASCII
/// alphanumerics and `_`).  An empty tail (a just-typed trailing `:`) is a
/// valid prefix and matches every shortcode.  Returns `None` when there is no
/// `:` or when the tail has already been terminated (e.g. by whitespace or a
/// closing `:`), so the autocomplete bar is hidden outside shortcode entry.
pub fn current_shortcode_prefix(text: &str) -> Option<&str> {
    let colon = text.rfind(':')?;
    let tail = &text[colon + 1..];
    if tail.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(tail)
    } else {
        None
    }
}

/// Decide whether a Return keypress in the Compose draft should trigger Send.
///
/// Returns `true` whenever `draft` has non-whitespace content — the same
/// intent as the Send button, which sends whatever text is present. Returns
/// `false` for empty or whitespace-only drafts, an explicit guard against
/// empty sends: `UiRuntime::step()` treats a `false` result as a total no-op
/// (no send, no navigation, and — because Return is intercepted before
/// `key_text` dispatch — no newline inserted either), rather than falling
/// back to the button's behavior of silently discarding the draft and still
/// navigating back to MessageView.
///
/// Pure (no Slint/hardware dependency) so this acceptance-critical decision
/// (empty/whitespace must not send) is host-testable in isolation.
pub fn compose_return_should_send(draft: &str) -> bool {
    !draft.trim().is_empty()
}

/// Pure predicate: has `UiRuntime::step()`'s deferred post-send Compose →
/// MessageView navigation deadline (`UiRuntime::
/// deferred_message_view_nav_at_ms`) elapsed?
///
/// Extracted from `step()`'s hardware-bound body so this timing edge —
/// unarmed, armed-but-not-yet-due, and armed-and-due (including the exact
/// boundary) — is covered by a host-native unit test independent of the
/// display/touch stack. The deferral itself exists so the Send button's
/// `RocketOnSend` one-shot has time to actually render on the still-live
/// Compose screen before it gets torn down by the screen swap to
/// MessageView.
pub fn send_nav_deferral_elapsed(deferred_at_ms: Option<u64>, now_ms: u64) -> bool {
    matches!(deferred_at_ms, Some(at_ms) if now_ms >= at_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_colon_opens_with_empty_prefix() {
        assert_eq!(current_shortcode_prefix("hello :"), Some(""));
    }

    #[test]
    fn partial_shortcode_is_the_prefix() {
        assert_eq!(current_shortcode_prefix("hi :sm"), Some("sm"));
        assert_eq!(current_shortcode_prefix(":rock"), Some("rock"));
    }

    #[test]
    fn closed_or_absent_shortcode_yields_none() {
        // Closed by whitespace after the token.
        assert_eq!(current_shortcode_prefix(":smile "), None);
        // No colon at all.
        assert_eq!(current_shortcode_prefix("plain text"), None);
        // A completed :shortcode: -> tail after last ':' is empty => Some(""),
        // which is acceptable (re-opens for a new shortcode after the colon).
    }

    // ── compose_return_should_send ────────────────────────────────────────
    //
    // Acceptance-critical: Return sends non-empty drafts; empty/whitespace-only
    // drafts must NOT send (an explicit guard-against-empty-sends requirement).

    #[test]
    fn non_empty_draft_sends_on_return() {
        assert!(compose_return_should_send("hi"));
        assert!(compose_return_should_send("  hi  ")); // surrounding whitespace is fine
        assert!(compose_return_should_send("a"));
        assert!(compose_return_should_send(":smile:"));
    }

    #[test]
    fn empty_or_whitespace_only_draft_does_not_send() {
        assert!(!compose_return_should_send(""));
        assert!(!compose_return_should_send(" "));
        assert!(!compose_return_should_send("   \t  "));
        assert!(!compose_return_should_send("\n"));
    }

    // ── send_nav_deferral_elapsed ─────────────────────────────────────────
    //
    // Regression guard: RocketOnSend's one-shot must have its full 400ms window to
    // render on the still-live Compose screen before step() swaps to
    // MessageView.

    #[test]
    fn no_deferral_armed_never_elapses() {
        assert!(!send_nav_deferral_elapsed(None, 0));
        assert!(!send_nav_deferral_elapsed(None, u64::MAX));
    }

    #[test]
    fn deferral_not_yet_elapsed_before_the_deadline() {
        assert!(!send_nav_deferral_elapsed(Some(1_000), 999));
    }

    #[test]
    fn deferral_elapsed_at_and_past_the_deadline() {
        // Exactly-at-deadline counts as elapsed (`>=`), not just strictly past.
        assert!(send_nav_deferral_elapsed(Some(1_000), 1_000));
        assert!(send_nav_deferral_elapsed(Some(1_000), 1_500));
    }
}
