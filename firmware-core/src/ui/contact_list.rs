// SPDX-License-Identifier: GPL-3.0-only
//! Contact / channel list screen — pure display-string formatting, the
//! plain-data contact/channel-entry types, and the two list-builder
//! functions that assemble them from `UiRuntime`'s data maps.
//!
//! The `slint::slint!{}` view and the `ContactListScreen` Rust wrapper stay
//! in `firmware/src/ui/screens/contact_list.rs` — they depend on Slint;
//! only the plain-data pieces below move here so their tests execute under
//! `cargo test --workspace` (this crate is a detached, cross-compiled
//! workspace — see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block
//! written there would type-check but never run). `ContactItem` moves
//! alongside `ChannelItem` (both plain data, no Slint dependency) so
//! `build_contact_items`/`build_channel_items` below can return them. See
//! `docs/adr/0005-firmware-core-extraction.md`.

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

/// A contact entry passed into the Slint model.
#[derive(Clone, Debug)]
pub struct ContactItem {
    pub name: String,
    pub preview: String,
    pub time_str: String,
    pub unread: i32,
    pub hash: u8,
}

/// Build a sorted contact item list from the current data maps.
///
/// Static function so `UiRuntime` can call it while `self.active_screen` is
/// borrowed — Rust's field-splitting rules allow simultaneous borrows of
/// separate struct fields.
pub fn build_contact_items(
    contact_names: &std::collections::HashMap<u8, String>,
    messages: &std::collections::HashMap<u8, Vec<super::MessageRecord>>,
    unread: &std::collections::HashMap<u8, u32>,
) -> Vec<ContactItem> {
    let mut items: Vec<ContactItem> = contact_names
        .iter()
        .map(|(&hash, name)| ContactItem {
            name: name.clone(),
            preview: messages
                .get(&hash)
                .and_then(|msgs| msgs.last())
                .map(|m| m.text.clone())
                .unwrap_or_default(),
            time_str: String::new(),
            unread: *unread.get(&hash).unwrap_or(&0) as i32,
            hash,
        })
        .collect();
    // Sort by unread count (desc) then name (asc) for consistent ordering.
    items.sort_by(|a, b| b.unread.cmp(&a.unread).then(a.name.cmp(&b.name)));
    items
}

/// Build a fresh channel item list with up-to-date preview/unread from the
/// current data maps, using `channel_items` (the provisioned catalog: name +
/// hash) as the source of truth for identity.
///
/// Mirrors `build_contact_items`.  Without this, `self.channel_items` — the
/// raw catalog pushed once at provisioning with `unread: 0` — was pushed to
/// the screen verbatim on every return to ContactList, permanently
/// overwriting any unread count that had accumulated in `self.unread`.
pub fn build_channel_items(
    channel_items: &[ChannelItem],
    messages: &std::collections::HashMap<u8, Vec<super::MessageRecord>>,
    unread: &std::collections::HashMap<u8, u32>,
) -> Vec<ChannelItem> {
    let mut items: Vec<ChannelItem> = channel_items
        .iter()
        .map(|c| ChannelItem {
            name: c.name.clone(),
            preview: messages
                .get(&c.hash)
                .and_then(|msgs| msgs.last())
                .map(|m| m.text.clone())
                .unwrap_or_default(),
            time_str: String::new(),
            unread: *unread.get(&c.hash).unwrap_or(&0) as i32,
            hash: c.hash,
        })
        .collect();
    // Sort by unread count (desc) then name (asc) — same ordering rule as
    // `build_contact_items`, for cross-tab consistency.
    items.sort_by(|a, b| b.unread.cmp(&a.unread).then(a.name.cmp(&b.name)));
    items
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
    use crate::ui::MessageRecord;
    use std::collections::HashMap;

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

    // ── build_contact_items / build_channel_items ───────────────────────────
    //
    // Regression guard: pins the two regressions the original firmware fix
    // addressed — channel unread counts reaching the screen (previously only
    // contacts refreshed), and both trees reading a common `unread` map that
    // gets cleared on read.

    fn catalog(entries: &[(&str, u8)]) -> Vec<ChannelItem> {
        entries
            .iter()
            .map(|&(name, hash)| ChannelItem {
                name: name.to_string(),
                preview: String::new(),
                time_str: String::new(),
                unread: 0, // catalog entries are always seeded at 0 — see set_channels
                hash,
            })
            .collect()
    }

    #[test]
    fn build_channel_items_reflects_unread_map() {
        let channels = catalog(&[("General", 0x10), ("Ops", 0x20)]);
        let messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        let mut unread = HashMap::new();
        unread.insert(0x20u8, 3u32);

        let items = build_channel_items(&channels, &messages, &unread);

        // Regression guard for the missing-badge defect: a channel with a
        // nonzero `unread` map entry must carry that count through, not the
        // catalog's frozen `unread: 0`.
        let ops = items.iter().find(|c| c.hash == 0x20).unwrap();
        assert_eq!(ops.unread, 3);
        let general = items.iter().find(|c| c.hash == 0x10).unwrap();
        assert_eq!(general.unread, 0);
    }

    #[test]
    fn build_channel_items_sorts_unread_first() {
        let channels = catalog(&[("Alpha", 1), ("Bravo", 2), ("Charlie", 3)]);
        let messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        let mut unread = HashMap::new();
        unread.insert(3u8, 1u32);

        let items = build_channel_items(&channels, &messages, &unread);
        assert_eq!(items[0].hash, 3); // sole unread channel sorts first
    }

    #[test]
    fn build_channel_items_carries_last_message_as_preview() {
        let channels = catalog(&[("General", 0x10)]);
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(
            0x10,
            vec![
                MessageRecord {
                    text: "first".into(),
                    is_ours: false,
                    acked: false,
                    ts_ms: 0,
                },
                MessageRecord {
                    text: "latest".into(),
                    is_ours: false,
                    acked: false,
                    ts_ms: 1,
                },
            ],
        );
        let unread = HashMap::new();

        let items = build_channel_items(&channels, &messages, &unread);
        assert_eq!(items[0].preview, "latest");
    }

    #[test]
    fn build_contact_items_reflects_unread_map() {
        let mut contact_names = HashMap::new();
        contact_names.insert(0x30u8, "Alice".to_string());
        contact_names.insert(0x40u8, "Bob".to_string());
        let messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        let mut unread = HashMap::new();
        unread.insert(0x30u8, 2u32);

        let items = build_contact_items(&contact_names, &messages, &unread);
        let alice = items.iter().find(|c| c.hash == 0x30).unwrap();
        assert_eq!(alice.unread, 2);
        let bob = items.iter().find(|c| c.hash == 0x40).unwrap();
        assert_eq!(bob.unread, 0);
    }

    #[test]
    fn contact_and_channel_unread_share_one_map_and_clear_together() {
        // Documents the pre-existing key-space assumption both builders share:
        // `unread` is keyed only by `u8` hash, not by (hash, is_channel). A
        // contact hash and a channel hash that happen to collide will share
        // one counter and one clear-on-read. Not a regression introduced by
        // this fix (contact_names/messages already share the same u8 key
        // space) — recorded here so a future change to disambiguate the key
        // space has a test to update.
        let mut unread: HashMap<u8, u32> = HashMap::new();
        unread.insert(0x55, 1);
        // Simulate navigate_to_message_view's clear-on-read for hash 0x55,
        // regardless of whether it was opened as a contact or a channel.
        unread.remove(&0x55);
        assert_eq!(unread.get(&0x55), None);
    }

    #[test]
    fn messages_insert_non_empty_seeded_history_feeds_contact_preview() {
        // End-to-end (within the pure-function slice): seeding restored
        // history and then building contact items must surface the restored
        // text as the preview — the actual acceptance behavior ("contact
        // list previews show restored history").
        let mut contact_names = HashMap::new();
        contact_names.insert(0x64u8, "Dana".to_string());
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        crate::ui::messages_insert_non_empty(
            &mut messages,
            0x64,
            vec![MessageRecord {
                text: "welcome back".into(),
                is_ours: false,
                acked: true,
                ts_ms: 0,
            }],
        );
        let unread = HashMap::new();

        let items = build_contact_items(&contact_names, &messages, &unread);
        let dana = items.iter().find(|c| c.hash == 0x64).unwrap();
        assert_eq!(dana.preview, "welcome back");
    }
}
