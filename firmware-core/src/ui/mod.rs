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
pub mod battery_indicator;
pub mod buzzer;
pub mod compose;
pub mod contact_list;
pub mod gps_status;
pub mod idle_tick;
pub mod keyboard;
pub mod lock;
pub mod message_view;
pub mod signal_meter;
pub mod splash;
pub mod theme;
pub mod touch;
pub mod ui_task_boundary;

/// Delivery status of an outbound (`is_ours: true`) [`MessageRecord`] — the
/// tri-state checkmark model `firmware`'s `MessageView` renders as grey
/// (`Pending`) / blue (`Acked`) / red (`Undelivered`), the third state this
/// mission adds alongside the pre-existing grey/blue pair. Meaningless for
/// an inbound (`is_ours: false`) record (the renderer only shows the
/// checkmark `if is_ours` — `firmware/src/ui/screens/message_view.rs`); kept
/// at `Acked` for those by convention (matches the pre-tri-state `acked:
/// true` default every inbound-record construction site already used) so
/// there is exactly one representation, never an `Option`-wrapped one only
/// half the records populate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryState {
    /// Sent, no ACK yet and its deadline hasn't passed — grey check.
    Pending,
    /// ACKed — blue check.
    Acked,
    /// No ACK arrived before its deadline, or its frame was evicted from the
    /// TX queue before ever reaching the wire — red check.
    Undelivered,
}

/// One stored message in a conversation — mirrors
/// `firmware::ui::MessageRecord` exactly. `ts_ms` is captured at every
/// construction site but not read by any pure helper here (no renderer
/// consumes it yet — see the firmware-side field's own doc); kept rather
/// than dropped for the same reason firmware keeps it.
#[derive(Clone, Debug)]
pub struct MessageRecord {
    pub text: String,
    pub is_ours: bool,
    pub delivery: DeliveryState,
    /// The wire ACK hash this outbound send is waiting on, if it is a DM or
    /// room post tracked by the dispatcher's outstanding-sends table —
    /// `None` for every inbound record, and for an outbound record this
    /// table doesn't track (a channel/GRP_TXT send, still resolved via
    /// [`mark_last_unacked_outbound`]'s heuristic below; or a DM/room-post
    /// bubble not yet tagged — see `firmware/src/ui/mod.rs`'s
    /// `UiEvent::DmQueued` doc).
    ///
    /// Exists so [`mark_delivery_by_ack_hash`] can flip the EXACT record a
    /// given outstanding-sends table entry refers to, rather than guessing
    /// via `mark_last_unacked_outbound`'s "newest pending" heuristic — that
    /// heuristic silently picks the wrong record once more than one DM can
    /// be outstanding to the same contact at once and their ACKs resolve
    /// out of order, which is exactly the concurrency this mission
    /// introduces (the single-slot `PendingAck` it replaces made that
    /// ambiguity physically unreachable before now).
    pub ack_hash: Option<[u8; 4]>,
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

/// Mark the most-recently-sent, still-pending outbound `MessageRecord` to
/// `to_hash` as acked (✓ → ✓✓). Returns `true` if a record was found and
/// flipped, `false` if there was no matching pending outbound message.
///
/// Searches newest-first (`.rev()`) and stops at the first `Pending` outbound
/// hit. Channel/GRP_TXT sends (this function's only remaining caller —
/// `UiEvent::ChannelAcked`, see its doc) stay on this "newest pending"
/// heuristic rather than [`mark_delivery_by_ack_hash`]'s exact match: a
/// broadcast has no wire ACK hash to correlate against at all (implicit
/// "ack" = hearing our own transmission repeated back — see
/// `main.rs::PendingChannelAck`'s doc), and — Channel/GRP_TXT sends are out
/// of this mission's Objective scope — still only ever track one outstanding
/// send at a time (`main.rs`'s single-slot `PendingChannelAck`), so the
/// heuristic's "right message marked" assumption still holds for them
/// exactly as it always did.
///
/// Pulled out as a free function over a plain map for the same reason as
/// `messages_insert_non_empty` above.
pub fn mark_last_unacked_outbound(
    messages: &mut std::collections::HashMap<u8, Vec<MessageRecord>>,
    to_hash: u8,
) -> bool {
    if let Some(msgs) = messages.get_mut(&to_hash) {
        for m in msgs.iter_mut().rev() {
            if m.is_ours && m.delivery == DeliveryState::Pending {
                m.delivery = DeliveryState::Acked;
                return true;
            }
        }
    }
    false
}

/// Flip the outbound `MessageRecord` to `to_hash` whose `ack_hash` exactly
/// matches `ack_hash` to `new_state` — the DM/room-post counterpart of
/// `mark_last_unacked_outbound`'s heuristic, used once a contact can have
/// more than one send outstanding at a time (this mission's whole point).
/// Returns `true` if a record was found and flipped.
///
/// Exact match over the "newest pending" guess: if the SECOND of two
/// outstanding DMs to the same contact is the one that gets ACKed first, the
/// heuristic above would flip the wrong (newest) record — this looks up the
/// specific record `ack_hash` was recorded on
/// (`UiEvent::DmQueued`/`UiEvent::RoomPostSent`, see their docs) instead.
pub fn mark_delivery_by_ack_hash(
    messages: &mut std::collections::HashMap<u8, Vec<MessageRecord>>,
    to_hash: u8,
    ack_hash: [u8; 4],
    new_state: DeliveryState,
) -> bool {
    if let Some(msgs) = messages.get_mut(&to_hash) {
        for m in msgs.iter_mut() {
            if m.is_ours && m.ack_hash == Some(ack_hash) {
                m.delivery = new_state;
                return true;
            }
        }
    }
    false
}

/// Tag the OLDEST outbound `MessageRecord` to `to_hash` that has no
/// `ack_hash` yet with `ack_hash` — how a DM's optimistically-rendered
/// bubble (pushed by `on_send_message` before the dispatcher has even built
/// the wire frame, let alone computed its ACK hash) later learns the hash
/// [`mark_delivery_by_ack_hash`] will resolve it by by (`UiEvent::DmQueued`,
/// raised once the dispatcher's `SendDm` handler computes it).
///
/// Oldest-first (NOT `.rev()`, unlike `mark_last_unacked_outbound`) is the
/// correct search order here: `on_send_message` always pushes its optimistic
/// bubble synchronously before queuing the matching `SendDm` command, and
/// both the command channel (UI → dispatcher) and the event channel
/// (dispatcher → UI) this tag arrives on preserve FIFO order, so untagged
/// records and their `DmQueued` events are always in the same relative
/// order — tagging the oldest untagged one first is what keeps two
/// back-to-back DMs to the same contact each getting their OWN hash rather
/// than both racing for the newest slot.
pub fn tag_oldest_untagged_outbound(
    messages: &mut std::collections::HashMap<u8, Vec<MessageRecord>>,
    to_hash: u8,
    ack_hash: [u8; 4],
) -> bool {
    if let Some(msgs) = messages.get_mut(&to_hash) {
        for m in msgs.iter_mut() {
            if m.is_ours && m.ack_hash.is_none() && m.delivery == DeliveryState::Pending {
                m.ack_hash = Some(ack_hash);
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
                delivery: DeliveryState::Acked,
                ack_hash: None,
                ts_ms: 0,
            },
            MessageRecord {
                text: "outbound restored".into(),
                is_ours: true,
                delivery: DeliveryState::Acked,
                ack_hash: None,
                ts_ms: 0,
            },
        ];
        messages_insert_non_empty(&mut messages, 0x55, records);

        let seeded = messages.get(&0x55).expect("conversation must be seeded");
        assert_eq!(seeded.len(), 2);
        assert_eq!(seeded[0].text, "inbound restored");
        assert!(!seeded[0].is_ours);
        assert!(
            seeded[0].delivery == DeliveryState::Acked,
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
                    delivery: DeliveryState::Pending,
                    ack_hash: None,
                    ts_ms: 0,
                },
                MessageRecord {
                    text: "second".into(),
                    is_ours: true,
                    delivery: DeliveryState::Pending,
                    ack_hash: None,
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
            msgs[0].delivery == DeliveryState::Pending,
            "the older unacked message must be left alone"
        );
        assert!(
            msgs[1].delivery == DeliveryState::Acked,
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
                    delivery: DeliveryState::Acked,
                    ack_hash: None,
                    ts_ms: 0,
                },
                MessageRecord {
                    text: "their reply".into(),
                    is_ours: false,
                    delivery: DeliveryState::Pending,
                    ack_hash: None,
                    ts_ms: 1,
                },
            ],
        );

        let marked = mark_last_unacked_outbound(&mut messages, 0x42);

        assert!(
            !marked,
            "no unacked OUTBOUND message exists — an inbound record must never be flipped"
        );
        assert!(messages[&0x42][0].delivery == DeliveryState::Acked);
        assert!(
            messages[&0x42][1].delivery == DeliveryState::Pending,
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
                delivery: DeliveryState::Pending,
                ack_hash: None,
                ts_ms: 0,
            }],
        );
        messages.insert(
            0x20,
            vec![MessageRecord {
                text: "to bob".into(),
                is_ours: true,
                delivery: DeliveryState::Pending,
                ack_hash: None,
                ts_ms: 0,
            }],
        );

        let marked = mark_last_unacked_outbound(&mut messages, 0x10);

        assert!(marked);
        assert!(
            messages[&0x10][0].delivery == DeliveryState::Acked,
            "the addressed contact's message is marked"
        );
        assert!(
            messages[&0x20][0].delivery == DeliveryState::Pending,
            "an unrelated contact's pending message must be untouched"
        );
    }

    #[test]
    fn unknown_contact_hash_is_a_no_op() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        let marked = mark_last_unacked_outbound(&mut messages, 0x99);
        assert!(!marked);
    }

    // ── mark_delivery_by_ack_hash / tag_oldest_untagged_outbound ───────────
    //
    // Regression guard for this mission's whole point: once more than one DM
    // can be outstanding to the same contact at once, resolving by exact
    // `ack_hash` (not `mark_last_unacked_outbound`'s "newest pending" guess)
    // is what lets each ACK flip the RIGHT record, even out of send order.

    #[test]
    fn mark_delivery_by_ack_hash_flips_the_exact_record_even_out_of_order() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(
            0x42,
            vec![
                MessageRecord {
                    text: "first".into(),
                    is_ours: true,
                    delivery: DeliveryState::Pending,
                    ack_hash: Some([1, 1, 1, 1]),
                    ts_ms: 0,
                },
                MessageRecord {
                    text: "second".into(),
                    is_ours: true,
                    delivery: DeliveryState::Pending,
                    ack_hash: Some([2, 2, 2, 2]),
                    ts_ms: 1,
                },
            ],
        );

        // The SECOND DM's ack arrives first — out of send order.
        let flipped =
            mark_delivery_by_ack_hash(&mut messages, 0x42, [2, 2, 2, 2], DeliveryState::Acked);
        assert!(flipped);
        let msgs = &messages[&0x42];
        assert_eq!(
            msgs[0].delivery,
            DeliveryState::Pending,
            "the FIRST DM's own ack hasn't arrived — it must stay pending, \
             not get flipped by the second DM's ack (the bug a newest-pending \
             heuristic would introduce here)"
        );
        assert_eq!(msgs[1].delivery, DeliveryState::Acked);
    }

    #[test]
    fn mark_delivery_by_ack_hash_no_match_is_a_no_op() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(
            0x42,
            vec![MessageRecord {
                text: "first".into(),
                is_ours: true,
                delivery: DeliveryState::Pending,
                ack_hash: Some([1, 1, 1, 1]),
                ts_ms: 0,
            }],
        );
        let flipped =
            mark_delivery_by_ack_hash(&mut messages, 0x42, [9, 9, 9, 9], DeliveryState::Acked);
        assert!(!flipped);
        assert_eq!(messages[&0x42][0].delivery, DeliveryState::Pending);
    }

    #[test]
    fn mark_delivery_by_ack_hash_can_mark_undelivered_too() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(
            0x42,
            vec![MessageRecord {
                text: "first".into(),
                is_ours: true,
                delivery: DeliveryState::Pending,
                ack_hash: Some([1, 1, 1, 1]),
                ts_ms: 0,
            }],
        );
        let flipped = mark_delivery_by_ack_hash(
            &mut messages,
            0x42,
            [1, 1, 1, 1],
            DeliveryState::Undelivered,
        );
        assert!(flipped);
        assert_eq!(messages[&0x42][0].delivery, DeliveryState::Undelivered);
    }

    #[test]
    fn tag_oldest_untagged_outbound_tags_the_oldest_first() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(
            0x42,
            vec![
                MessageRecord {
                    text: "first".into(),
                    is_ours: true,
                    delivery: DeliveryState::Pending,
                    ack_hash: None,
                    ts_ms: 0,
                },
                MessageRecord {
                    text: "second".into(),
                    is_ours: true,
                    delivery: DeliveryState::Pending,
                    ack_hash: None,
                    ts_ms: 1,
                },
            ],
        );

        // Two DMs queued back-to-back — the dispatcher raises `DmQueued`
        // once per send, in send order, tagging the oldest untagged record
        // each time.
        assert!(tag_oldest_untagged_outbound(
            &mut messages,
            0x42,
            [1, 1, 1, 1]
        ));
        assert!(tag_oldest_untagged_outbound(
            &mut messages,
            0x42,
            [2, 2, 2, 2]
        ));

        let msgs = &messages[&0x42];
        assert_eq!(msgs[0].ack_hash, Some([1, 1, 1, 1]));
        assert_eq!(msgs[1].ack_hash, Some([2, 2, 2, 2]));
    }

    #[test]
    fn tag_oldest_untagged_outbound_skips_already_tagged_records() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(
            0x42,
            vec![MessageRecord {
                text: "first".into(),
                is_ours: true,
                delivery: DeliveryState::Pending,
                ack_hash: Some([1, 1, 1, 1]),
                ts_ms: 0,
            }],
        );
        assert!(
            !tag_oldest_untagged_outbound(&mut messages, 0x42, [2, 2, 2, 2]),
            "no untagged record exists — must not overwrite an existing tag"
        );
        assert_eq!(messages[&0x42][0].ack_hash, Some([1, 1, 1, 1]));
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
