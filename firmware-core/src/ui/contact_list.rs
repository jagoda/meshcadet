// SPDX-License-Identifier: GPL-3.0-only
//! Contact / channel list screen — pure display-string formatting + the
//! plain-data channel-entry type.
//!
//! The `slint::slint!{}` view and the `ContactListScreen` Rust wrapper
//! (along with `ContactItem`, its DM-side twin) stay in
//! `firmware/src/ui/screens/contact_list.rs` — they depend on Slint; only
//! the plain-data pieces below move here so their tests execute under
//! `cargo test --workspace` (this crate is a detached, cross-compiled
//! workspace — see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block
//! written there would type-check but never run). `ChannelItem` moves too
//! even though it carries no test of its own: it is plain data with no
//! Slint dependency, and the follow-on `firmware-core-extract-ui-runtime`
//! increment needs it in this crate for `UiRuntime::build_channel_items`.
//! See `docs/adr/0005-firmware-core-extraction.md`.

/// Format an unread count for badge display: exact digits at ≤9, capped to
/// `"9+"` above — the single clamp rule shared by every unread badge
/// `ContactListScreen` renders (per-row `ContactRow`/channel-row badges AND
/// the two tab-bar aggregate badges in `set_contacts`/`set_channels`). Kept
/// as one pure, unit-testable function rather than four independently-
/// inlined copies of the same ternary, so the "9+" rule can't drift between
/// them.
pub fn format_unread_badge(count: i32) -> String {
    if count > 9 {
        "9+".to_string()
    } else {
        count.to_string()
    }
}

/// Whether `total` is a genuine increase over a previously-observed
/// baseline — the comet-on-notify predicate `ContactListScreen::
/// maybe_fire_notify` applies. `prev: None` means "no baseline recorded
/// yet" (the screen's very first `set_contacts`/`set_channels` call after
/// construction, i.e. the initial populate) and always returns `false`, so
/// merely navigating into a screen that already has unread messages never
/// fires the comet — only a call that pushes the total past a previously-
/// recorded value does. Pulled out as a pure function (mirrors
/// `format_unread_badge` above) so the baseline/increase rule is
/// host-testable independent of the Slint side-effect it gates.
pub fn unread_total_increased(prev: Option<i32>, total: i32) -> bool {
    matches!(prev, Some(p) if total > p)
}

/// A channel entry.
#[derive(Clone, Debug)]
pub struct ChannelItem {
    pub name: String,
    pub preview: String,
    pub time_str: String,
    pub unread: i32,
    pub hash: u8,
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Regression guard: the per-tab aggregate badges (`contacts_unread_total`/
// `channels_unread_total` and their `_str` counterparts, set in
// `set_contacts`/`set_channels` on the firmware side) had zero direct test
// coverage before this — the Slint root component can't be constructed
// off-device (no display backend in a host/CI test run), so the
// arithmetic/formatting was previously only exercised by hand on hardware.
// Pulling the shared clamp rule out to `format_unread_badge` makes at least
// that seam host-testable.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_unread_badge_exact_for_zero_through_nine() {
        assert_eq!(format_unread_badge(0), "0");
        assert_eq!(format_unread_badge(1), "1");
        assert_eq!(format_unread_badge(9), "9");
    }

    #[test]
    fn format_unread_badge_caps_above_nine() {
        assert_eq!(format_unread_badge(10), "9+");
        assert_eq!(format_unread_badge(42), "9+");
    }

    // Regression guard: the comet-on-notify baseline/increase predicate,
    // pulled out to a pure function for the same "Slint root can't be
    // constructed off-device" reason the badge tests above document.
    #[test]
    fn unread_total_increased_false_with_no_baseline() {
        // The initial populate (`navigate_to_contact_list` /
        // `refresh_contact_list_lists`) must never fire the comet, even if
        // the freshly-shown screen already has unread messages.
        assert!(!unread_total_increased(None, 0));
        assert!(!unread_total_increased(None, 5));
    }

    #[test]
    fn unread_total_increased_true_on_genuine_increase() {
        assert!(unread_total_increased(Some(0), 1));
        assert!(unread_total_increased(Some(2), 3));
    }

    #[test]
    fn unread_total_increased_false_on_same_or_decrease() {
        assert!(!unread_total_increased(Some(3), 3));
        assert!(!unread_total_increased(Some(3), 2));
        assert!(!unread_total_increased(Some(3), 0));
    }
}
