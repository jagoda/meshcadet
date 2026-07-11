// SPDX-License-Identifier: GPL-3.0-only
//! Message thread view screen — pure comet-on-notify predicate.
//!
//! The `slint::slint!{}` view, the `MessageViewScreen` Rust wrapper, and
//! `MessageItem` stay in `firmware/src/ui/screens/message_view.rs` (the
//! wrapper depends on Slint; `MessageItem` is only ever constructed
//! alongside a `set_messages` call there); only the plain-data predicate
//! below moves here so its tests execute under `cargo test --workspace`
//! (this crate is a detached, cross-compiled workspace — see `Cargo.toml`'s
//! doc comment — so a `#[cfg(test)]` block written there would type-check
//! but never run). See `docs/adr/0005-firmware-core-extraction.md`.

/// Whether `total` (a count of received, `!is_ours` messages in the current
/// thread) is a genuine increase over a previously-observed baseline — the
/// comet-on-notify predicate `MessageViewScreen::set_messages` applies.
/// `prev: None` means "no baseline recorded yet" (this instance's very first
/// `set_messages` call, i.e. the initial populate on navigate-in) and always
/// returns `false`, so merely opening a thread that already has received
/// messages never fires the comet — only a call that pushes the received
/// count past a previously-recorded value does. Mirrors
/// `contact_list::unread_total_increased` (same shape, different underlying
/// quantity).
pub fn received_total_increased(prev: Option<i32>, total: i32) -> bool {
    matches!(prev, Some(p) if total > p)
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Regression guard: the comet-on-notify baseline/increase predicate, pulled
// out to a pure function for the same "Slint root can't be constructed
// off-device" reason `contact_list`'s equivalent test module documents.
#[cfg(test)]
mod tests {
    use super::received_total_increased;

    #[test]
    fn received_total_increased_false_with_no_baseline() {
        // The initial populate (`navigate_to_message_view`) must never fire
        // the comet, even if the freshly-opened thread already has received
        // messages.
        assert!(!received_total_increased(None, 0));
        assert!(!received_total_increased(None, 5));
    }

    #[test]
    fn received_total_increased_true_on_genuine_increase() {
        assert!(received_total_increased(Some(0), 1));
        assert!(received_total_increased(Some(2), 3));
    }

    #[test]
    fn received_total_increased_false_on_same_or_decrease() {
        // Same count (e.g. an Ack-only refresh, which never appends a new
        // received record) must not re-fire the comet.
        assert!(!received_total_increased(Some(3), 3));
        assert!(!received_total_increased(Some(3), 2));
        assert!(!received_total_increased(Some(3), 0));
    }
}
