// SPDX-License-Identifier: GPL-3.0-only
//! Pure-logic halves of `firmware`'s Slint-backed screens, plus
//! `UiRuntime`'s own screen-agnostic pure helpers.
//!
//! Each screen's `slint::slint! { ... }` markup and its Rust-side wrapper
//! struct depend on Slint and stay in `firmware/src/ui/screens/`; only the
//! plain-data formatting helpers move here so their tests execute under
//! `cargo test --workspace`.
//!
//! `keyboard::key_text` is the one exception noted in `keyboard`'s own doc:
//! it feeds Slint's `Key`/`SharedString` types directly, so it stays behind
//! in `firmware/src/ui/keyboard.rs` (still compile-only) rather than pulling
//! `slint` into this crate's dependency graph — see that module's doc for
//! the full reclassification note.
//!
//! # `UiRuntime`'s own pure helpers
//!
//! [`MessageRecord`] (the plain conversation-history record `UiRuntime`
//! stores per contact/channel hash) and the two functions below that operate
//! directly on `HashMap<u8, Vec<MessageRecord>>` — `UiRuntime`'s shared
//! message store — move here from `firmware/src/ui/mod.rs` (rather than into
//! any single screen submodule) because they aren't owned by one screen:
//! `messages_insert_non_empty` seeds boot-restored history (consumed by both
//! `contact_list::build_contact_items` and `contact_list::build_channel_items`),
//! and `mark_last_unacked_outbound` backs the ✓→✓✓ delivered-indicator for
//! both DMs and channel messages. [`roll_selection`] is the trackball
//! Up/Down index arithmetic shared by `ContactList` and `AdminMenu`'s
//! trackball handlers — likewise not owned by either screen alone. See
//! `docs/adr/0005-firmware-core-extraction.md`.

pub mod admin_menu;
pub mod buzzer;
pub mod compose;
pub mod contact_list;
pub mod gps_status;
pub mod keyboard;
pub mod message_view;
pub mod signal_meter;
pub mod splash;
pub mod theme;
pub mod touch;

/// One stored message in a conversation — mirrors
/// `firmware::ui::MessageRecord` exactly. `ts_ms` is captured at every
/// construction site but not read by any pure helper here (no renderer
/// consumes it yet — see the firmware-side field's own doc); kept rather
/// than dropped for the same reason firmware keeps it.
#[derive(Clone, Debug)]
pub struct MessageRecord {
    pub text: String,
    pub is_ours: bool,
    pub acked: bool,
    #[allow(dead_code)]
    pub ts_ms: u64,
}

/// Insert `records` under `hash`, unless `records` is empty (a no-op skip,
/// not a clearing insert — see `UiRuntime::seed_conversation`'s doc for why
/// an empty conversation is left absent from the map rather than inserted as
/// `vec![]`).
///
/// Pulled out as a free function over a plain map — rather than a
/// `UiRuntime` method — purely so it's testable in isolation, same
/// "static function over plain data" pattern `contact_list::build_contact_items`/
/// `contact_list::build_channel_items` already use (those can't touch real
/// display/touch hardware in a test either).
pub fn messages_insert_non_empty(
    messages: &mut std::collections::HashMap<u8, Vec<MessageRecord>>,
    hash: u8,
    records: Vec<MessageRecord>,
) {
    if records.is_empty() {
        return;
    }
    messages.insert(hash, records);
}

/// Mark the most-recently-sent, still-unacked outbound `MessageRecord` to
/// `to_hash` as acked (✓ → ✓✓). Returns `true` if a record was found and
/// flipped, `false` if there was no matching pending outbound message.
///
/// Searches newest-first (`.rev()`) and stops at the first unacked outbound
/// hit — this is the "right message marked" invariant a confirmed-delivered
/// DM depends on: `main.rs`'s `pending_ack` tracks only ONE outstanding ack at
/// a time, for the most recently sent DM, so the newest unacked outbound
/// record in this contact's thread is always the one a live match refers to.
///
/// Pulled out as a free function over a plain map for the same reason as
/// `messages_insert_non_empty` above.
pub fn mark_last_unacked_outbound(
    messages: &mut std::collections::HashMap<u8, Vec<MessageRecord>>,
    to_hash: u8,
) -> bool {
    if let Some(msgs) = messages.get_mut(&to_hash) {
        for m in msgs.iter_mut().rev() {
            if m.is_ours && !m.acked {
                m.acked = true;
                return true;
            }
        }
    }
    false
}

/// Pure index-math for a trackball Up/Down roll, shared by
/// `UiRuntime::handle_trackball_contact_list` and
/// `UiRuntime::handle_trackball_admin_menu`: move `current` toward the top
/// (`up: true`, decrement) or bottom (`up: false`, increment) of a
/// `0..=max_idx` list.
///
/// - `current < 0` means "no highlight yet" (the `-1` sentinel documented on
///   `contact_list_selected`/`admin_menu_selected`): the FIRST roll in
///   either direction always lands on row `0`, matching "roll highlights a
///   contact/channel" — the first roll picks the top row rather than needing
///   an extra roll to establish a starting point.
/// - `max_idx < 0` means an empty list (nothing to highlight): always returns
///   `-1` regardless of direction or `current`, so a caller can treat a
///   negative result as "no-op, no valid row" uniformly.
/// - Otherwise clamps to `0..=max_idx` — rolling off either end holds at that
///   end rather than wrapping (a wrap would let a roll silently jump from the
///   last row back to the first, easy to trigger by accident on a physical
///   trackball and surprising for the target audience).
pub fn roll_selection(current: i32, max_idx: i32, up: bool) -> i32 {
    if max_idx < 0 {
        return -1;
    }
    if current < 0 {
        0
    } else if up {
        (current - 1).max(0)
    } else {
        (current + 1).min(max_idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── messages_insert_non_empty — boot-hydrate seeding core ───────────────

    #[test]
    fn messages_insert_non_empty_seeds_restored_history() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        let records = vec![
            MessageRecord {
                text: "inbound restored".into(),
                is_ours: false,
                acked: true,
                ts_ms: 0,
            },
            MessageRecord {
                text: "outbound restored".into(),
                is_ours: true,
                acked: true,
                ts_ms: 0,
            },
        ];
        messages_insert_non_empty(&mut messages, 0x55, records);

        let seeded = messages.get(&0x55).expect("conversation must be seeded");
        assert_eq!(seeded.len(), 2);
        assert_eq!(seeded[0].text, "inbound restored");
        assert!(!seeded[0].is_ours);
        assert!(
            seeded[0].acked,
            "restored records must never show perpetual pending"
        );
        assert!(seeded[1].is_ours);
    }

    #[test]
    fn messages_insert_non_empty_skips_empty_conversation() {
        // An empty conversation (no history stored) must hydrate to empty —
        // i.e. leave the key absent — not insert `vec![]`, so a caller can
        // still tell "never messaged" apart from "seeded but list happened
        // to be empty" if that distinction ever matters, and so previews
        // read via `messages.get(&hash).and_then(|m| m.last())` behave
        // identically either way (both `None`).
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages_insert_non_empty(&mut messages, 0x77, Vec::new());
        assert!(!messages.contains_key(&0x77));
    }

    // ── mark_last_unacked_outbound — live ACK → ✓✓ indicator ────────────────

    #[test]
    fn marks_the_newest_unacked_outbound_message() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(
            0x42,
            vec![
                MessageRecord {
                    text: "first".into(),
                    is_ours: true,
                    acked: false,
                    ts_ms: 0,
                },
                MessageRecord {
                    text: "second".into(),
                    is_ours: true,
                    acked: false,
                    ts_ms: 1,
                },
            ],
        );

        let marked = mark_last_unacked_outbound(&mut messages, 0x42);

        assert!(
            marked,
            "an unacked outbound message must be found and marked"
        );
        let msgs = &messages[&0x42];
        assert!(
            !msgs[0].acked,
            "the older unacked message must be left alone"
        );
        assert!(
            msgs[1].acked,
            "the most recently sent unacked message is the one the ack refers to"
        );
    }

    #[test]
    fn does_not_re_ack_an_already_acked_message_or_touch_inbound_records() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(
            0x42,
            vec![
                MessageRecord {
                    text: "outbound already delivered".into(),
                    is_ours: true,
                    acked: true,
                    ts_ms: 0,
                },
                MessageRecord {
                    text: "their reply".into(),
                    is_ours: false,
                    acked: false,
                    ts_ms: 1,
                },
            ],
        );

        let marked = mark_last_unacked_outbound(&mut messages, 0x42);

        assert!(
            !marked,
            "no unacked OUTBOUND message exists — an inbound record must never be flipped"
        );
        assert!(messages[&0x42][0].acked);
        assert!(
            !messages[&0x42][1].acked,
            "inbound records are never acked by this path"
        );
    }

    #[test]
    fn does_not_touch_a_different_contacts_conversation() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(
            0x10,
            vec![MessageRecord {
                text: "to alice".into(),
                is_ours: true,
                acked: false,
                ts_ms: 0,
            }],
        );
        messages.insert(
            0x20,
            vec![MessageRecord {
                text: "to bob".into(),
                is_ours: true,
                acked: false,
                ts_ms: 0,
            }],
        );

        let marked = mark_last_unacked_outbound(&mut messages, 0x10);

        assert!(marked);
        assert!(
            messages[&0x10][0].acked,
            "the addressed contact's message is marked"
        );
        assert!(
            !messages[&0x20][0].acked,
            "an unrelated contact's pending message must be untouched"
        );
    }

    #[test]
    fn unknown_contact_hash_is_a_no_op() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        let marked = mark_last_unacked_outbound(&mut messages, 0x99);
        assert!(!marked);
    }

    // ── roll_selection ───────────────────────────────────────────────────

    #[test]
    fn first_roll_from_no_selection_lands_on_top_row_either_direction() {
        assert_eq!(
            roll_selection(-1, 3, true),
            0,
            "first Up roll starts at row 0"
        );
        assert_eq!(
            roll_selection(-1, 3, false),
            0,
            "first Down roll also starts at row 0"
        );
    }

    #[test]
    fn empty_list_never_produces_a_valid_index() {
        assert_eq!(roll_selection(-1, -1, true), -1);
        assert_eq!(roll_selection(-1, -1, false), -1);
    }

    #[test]
    fn roll_up_decrements_and_floors_at_zero() {
        assert_eq!(roll_selection(2, 3, true), 1);
        assert_eq!(
            roll_selection(0, 3, true),
            0,
            "already at the top row — holds, no wrap"
        );
    }

    #[test]
    fn roll_down_increments_and_ceilings_at_max_idx() {
        assert_eq!(roll_selection(1, 3, false), 2);
        assert_eq!(
            roll_selection(3, 3, false),
            3,
            "already at the bottom row — holds, no wrap"
        );
    }

    #[test]
    fn single_row_list_holds_at_zero_both_directions() {
        assert_eq!(roll_selection(0, 0, true), 0);
        assert_eq!(roll_selection(0, 0, false), 0);
    }
}
