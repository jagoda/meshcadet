// SPDX-License-Identifier: GPL-3.0-only
//! Message thread view screen — pure comet-on-notify predicate, the
//! plain-data message-row type, the model-builder that assembles it from
//! stored conversation history, and the @mention wrap/render glue around
//! `protocol::mention`.
//!
//! The `slint::slint!{}` view and the `MessageViewScreen` Rust wrapper stay
//! in `firmware/src/ui/screens/message_view.rs` (the wrapper depends on
//! Slint); only the plain-data pieces below move here so their tests
//! execute under `cargo test --workspace` (this crate is a detached,
//! cross-compiled workspace — see `Cargo.toml`'s doc comment — so a
//! `#[cfg(test)]` block written there would type-check but never run). See
//! `docs/adr/0005-firmware-core-extraction.md`.

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

/// Pure predicate: should an incoming message for `(hash, is_channel)` be
/// flagged unread, given the conversation the user is CURRENTLY viewing
/// (`active_convo`)? `false` only while that exact conversation is the one
/// open in a live `MessageView` — `UiRuntime::handle_event`'s IncomingDm/
/// IncomingGroupMsg branches both share this one gate.
///
/// # The invariant this depends on
///
/// `active_convo` must be `None` whenever no conversation is on screen —
/// `navigate_to_message_view` sets it, and `navigate_to_contact_list` (the
/// single choke point both PinEntry-cancel and MessageView's Back button
/// route through) clears it back to `None`. Before that clear existed,
/// `active_convo` stayed latched to whichever conversation was most
/// recently opened, so this predicate stayed permanently `false` for that
/// one (hash, is_channel) — suppressing its unread badge even long after
/// the user had navigated away.
pub fn incoming_message_is_unread(
    active_convo: Option<(u8, bool)>,
    hash: u8,
    is_channel: bool,
) -> bool {
    active_convo != Some((hash, is_channel))
}

/// A single message in the conversation thread.
#[derive(Clone, Debug)]
pub struct MessageItem {
    /// Message text (UTF-8, emoji already expanded from shortcodes, `@[name]`
    /// mention wire markup already flattened to a brackets-hidden `@name`
    /// display string — see [`render_mentions`]). For a received channel
    /// message with a parseable sender prefix, this is the body ALONE — the
    /// "<name>: " prefix has been split off into `from_name` (see below) so
    /// `MessageBubble` can render it bold.
    pub text: String,
    /// Sender name for a received channel message, sans the trailing `": "`
    /// delimiter — empty for DMs, sent messages, and channel messages with
    /// no parseable prefix. `MessageBubble` renders `"<from_name>:"` in bold
    /// beside the (normal-weight) body when this is non-empty, and falls
    /// back to a single plain-weight Text otherwise. Populated by
    /// [`build_message_items`].
    pub from_name: String,
    pub time_str: String,
    pub is_ours: bool,
    pub acked: bool,
    /// Highest `protocol::mention::MentionTier` found in `text`, as `i32` (0
    /// = none, 1 = other-node mention, 2 = self-mention). Drives
    /// `MessageBubble`'s tint — see that property's doc.
    pub mention_tier: i32,
}

/// Wrap `@name` occurrences in `text` into wire form `@[name]` against
/// `known` (see `protocol::mention::wrap_mentions`). The send-side half of
/// the @mention wrap (send) / render (receive) pair — `UiRuntime::
/// on_send_message` calls this before storing/sending a composed message.
/// Falls back to `text` verbatim on overflow of the internal scratch buffer
/// (matches `protocol::emoji::expand_shortcodes`'s call site's fallback
/// style) — a composed message longer than the scratch buffer is already
/// bounded well under it by the compose screen's own input limit.
pub fn wrap_outgoing_mentions(text: &str, known: &[&str]) -> String {
    let mut out = [0u8; 512];
    match protocol::mention::wrap_mentions(text.as_bytes(), known, &mut out) {
        Some(n) => String::from_utf8_lossy(&out[..n]).into_owned(),
        None => text.to_string(),
    }
}

/// Flatten `body`'s `@[name]` wire markup into a brackets-hidden `@name`
/// display string, and compute the highest `protocol::mention::MentionTier`
/// present, returned as `i32` (see [`MessageItem::mention_tier`]'s doc for
/// the tier code meaning). The receive-side half of the wrap (send) / render
/// (receive) pair — see [`wrap_outgoing_mentions`].
pub fn render_mentions(body: &str, self_name: &str, known: &[&str]) -> (String, i32) {
    use protocol::mention::MentionTier;
    let mut display = String::with_capacity(body.len());
    let mut tier = MentionTier::Plain;
    for run in protocol::mention::split_mentions(body, self_name, known) {
        if run.tier == MentionTier::Plain {
            display.push_str(run.text);
        } else {
            display.push('@');
            display.push_str(run.text);
        }
        if run.tier > tier {
            tier = run.tier;
        }
    }
    (display, tier as i32)
}

/// Build the MessageView model rows from stored message records.
///
/// For received channel messages (`is_channel && !m.is_ours`), the stored
/// text carries MeshCore's inline `"<name>: <msg>"` sender prefix (see
/// `protocol::parse_channel_text`, and `main.rs::handle_grp_txt` which
/// stores the raw prefixed text unmodified). This splits it into
/// `from_name` (the sender, sans delimiter) and a body so the Slint
/// `MessageBubble` can render the name+colon in bold and the body at
/// normal weight. DMs and sent messages never carry this prefix and pass
/// the whole text through as the body with `from_name` empty — which is
/// also the signal `MessageBubble` uses to fall back to plain, single-run
/// rendering, so the prefix split is scoped to received channel messages
/// only.
///
/// The body (post prefix-split) is then run through [`render_mentions`]
/// (`self_name`/`known` — same known-names set [`wrap_outgoing_mentions`]
/// matches against on send) to flatten `@[name]` wire markup into a
/// brackets-hidden `@name` display string, and to compute `mention_tier`.
/// Applied uniformly to sent and received messages (a self-composed mention
/// highlights too) — mentions are not channel-scoped.
pub fn build_message_items(
    records: &[super::MessageRecord],
    is_channel: bool,
    self_name: &str,
    known: &[&str],
) -> Vec<MessageItem> {
    records
        .iter()
        .map(|m| {
            let (from_name, body) = if is_channel && !m.is_ours {
                match protocol::parse_channel_text(m.text.as_bytes()) {
                    (Some(name), body) => (
                        String::from_utf8_lossy(name).into_owned(),
                        String::from_utf8_lossy(body).into_owned(),
                    ),
                    (None, _) => (String::new(), m.text.clone()),
                }
            } else {
                (String::new(), m.text.clone())
            };
            let (text, mention_tier) = render_mentions(&body, self_name, known);
            MessageItem {
                text,
                from_name,
                time_str: String::new(),
                is_ours: m.is_ours,
                acked: m.acked,
                mention_tier,
            }
        })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Regression guard: the comet-on-notify baseline/increase predicate, pulled
// out to a pure function for the same "Slint root can't be constructed
// off-device" reason `contact_list`'s equivalent test module documents.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::MessageRecord;

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

    // ── incoming_message_is_unread ─────────────────────────────────────────
    //
    // Regression guard: the "don't flag unread while this exact thread is
    // open" gate must suppress ONLY the currently-open conversation, and
    // must NOT stay latched to a conversation that is no longer open (i.e.
    // once `navigate_to_contact_list` has cleared `active_convo` back to
    // `None`).

    #[test]
    fn no_active_convo_is_always_unread() {
        // `None` — either never opened a conversation, or (the bug this fix
        // addresses) properly cleared on return to ContactList — must never
        // suppress the badge.
        assert!(incoming_message_is_unread(None, 0x20, false));
        assert!(incoming_message_is_unread(None, 0x20, true));
    }

    #[test]
    fn matching_open_convo_suppresses_unread() {
        // The exact (hash, is_channel) pair currently open in MessageView —
        // the message lands directly in the live view, so it must not also
        // flag the badge.
        assert!(!incoming_message_is_unread(
            Some((0x20, false)),
            0x20,
            false
        ));
        assert!(!incoming_message_is_unread(Some((0x20, true)), 0x20, true));
    }

    #[test]
    fn different_hash_or_kind_stays_unread_even_with_an_active_convo() {
        // A different contact/channel — or the same hash under the other
        // kind (DM hash 0x20 vs. channel hash 0x20 are different threads) —
        // must still flag unread while some OTHER convo is open.
        assert!(incoming_message_is_unread(Some((0x20, false)), 0x30, false));
        assert!(incoming_message_is_unread(Some((0x20, false)), 0x20, true));
        assert!(incoming_message_is_unread(Some((0x20, true)), 0x20, false));
    }

    // ── build_message_items — channel sender-prefix split ────────────────
    //
    // Regression guard: pins which records get their MeshCore
    // "<name>: <msg>" wire text split into (from_name, text) — the signal
    // `MessageBubble` uses to bold the sender prefix — and which pass `text`
    // through verbatim with an empty `from_name` (the fallback to plain
    // rendering).

    #[test]
    fn build_message_items_splits_prefix_on_received_channel_message() {
        let records = vec![MessageRecord {
            text: "Alice: hello there".into(),
            is_ours: false,
            acked: false,
            ts_ms: 0,
        }];
        let items = build_message_items(&records, /* is_channel */ true, "Self", &[]);
        assert_eq!(items[0].from_name, "Alice");
        assert_eq!(items[0].text, "hello there");
    }

    #[test]
    fn build_message_items_leaves_sent_channel_message_unprefixed() {
        // Sent messages store the raw compose text (no MeshCore name prefix
        // — see `UiRuntime::on_send_message`), and must render exactly as
        // before this fix regardless of channel-ness: no split, empty
        // from_name.
        let records = vec![MessageRecord {
            text: "Alice: hello there".into(),
            is_ours: true,
            acked: false,
            ts_ms: 0,
        }];
        let items = build_message_items(&records, /* is_channel */ true, "Self", &[]);
        assert_eq!(items[0].from_name, "");
        assert_eq!(items[0].text, "Alice: hello there");
    }

    #[test]
    fn build_message_items_leaves_dm_message_unprefixed() {
        // DMs never carry the channel wire-text delimiter, even if their
        // literal text happens to contain "name: " — is_channel=false is the
        // guard, not a text-shape heuristic.
        let records = vec![MessageRecord {
            text: "Alice: hello there".into(),
            is_ours: false,
            acked: false,
            ts_ms: 0,
        }];
        let items = build_message_items(&records, /* is_channel */ false, "Self", &[]);
        assert_eq!(items[0].from_name, "");
        assert_eq!(items[0].text, "Alice: hello there");
    }

    #[test]
    fn build_message_items_falls_back_when_channel_text_has_no_prefix() {
        // Malformed/prefix-less channel text (no "<name>: " delimiter) passes
        // through verbatim rather than mis-splitting on the wrong bytes.
        let records = vec![MessageRecord {
            text: "no delimiter here".into(),
            is_ours: false,
            acked: false,
            ts_ms: 0,
        }];
        let items = build_message_items(&records, /* is_channel */ true, "Self", &[]);
        assert_eq!(items[0].from_name, "");
        assert_eq!(items[0].text, "no delimiter here");
    }

    #[test]
    fn build_message_items_empty_sender_name_falls_back_to_plain_body() {
        // Pathological wire text (never emitted by real MeshCore senders,
        // whose sender_name is never empty) with an EMPTY name before the
        // delimiter: `parse_channel_text` still reports `Some("")`, which
        // collapses to `from_name == ""` here — the same signal
        // `MessageBubble` treats as "no attribution", so it falls back to
        // plain rendering. Documented rather than special-cased: the
        // delimiter itself is dropped from the displayed body in this
        // corner case (accepted known limitation).
        let records = vec![MessageRecord {
            text: ": hello".into(),
            is_ours: false,
            acked: false,
            ts_ms: 0,
        }];
        let items = build_message_items(&records, /* is_channel */ true, "Self", &[]);
        assert_eq!(items[0].from_name, "");
        assert_eq!(items[0].text, "hello");
    }

    // ── @mentions — wrap (send) / render (receive) ────────────────────────
    //
    // Pins the two Rust-side seams around `protocol::mention` (itself
    // unit-tested in `protocol/src/mention.rs`): `wrap_outgoing_mentions`
    // (send-side glue) and `render_mentions`/`build_message_items`
    // (receive-side glue — flattened display text + `mention_tier`).

    #[test]
    fn wrap_outgoing_mentions_wraps_known_name() {
        let out = wrap_outgoing_mentions("hi @Alice!", &["Alice", "Bob"]);
        assert_eq!(out, "hi @[Alice]!");
    }

    #[test]
    fn wrap_outgoing_mentions_leaves_unknown_name_verbatim() {
        let out = wrap_outgoing_mentions("hi @nobody!", &["Alice", "Bob"]);
        assert_eq!(out, "hi @nobody!");
    }

    #[test]
    fn render_mentions_flattens_brackets_and_reports_no_tier_for_plain_text() {
        let (text, tier) = render_mentions("just a plain message", "Bob", &[]);
        assert_eq!(text, "just a plain message");
        assert_eq!(tier, 0);
    }

    #[test]
    fn render_mentions_other_node_mention_is_tier_1() {
        let (text, tier) = render_mentions("hi @[Alice] there", "Bob", &[]);
        assert_eq!(text, "hi @Alice there");
        assert!(!text.contains('['));
        assert!(!text.contains(']'));
        assert_eq!(tier, 1);
    }

    #[test]
    fn render_mentions_self_mention_is_tier_2_more_prominent_than_other() {
        let (text, tier) = render_mentions("hi @[Bob] there", "Bob", &[]);
        assert_eq!(text, "hi @Bob there");
        assert_eq!(tier, 2);
        assert!(tier > 1); // self-mention outranks an other-node mention
    }

    #[test]
    fn render_mentions_multiword_name_not_tokenized_on_space() {
        // A real-world example: "Chicken Little" contains a space — the
        // delimiter must not be mistaken for a word boundary.
        let (text, tier) =
            render_mentions("watch out @[Chicken Little] the sky is falling", "Rex", &[]);
        assert_eq!(text, "watch out @Chicken Little the sky is falling");
        assert_eq!(tier, 1);
    }

    #[test]
    fn build_message_items_renders_self_mention_in_received_dm() {
        let records = vec![MessageRecord {
            text: "hey @[Bob] check this out".into(),
            is_ours: false,
            acked: false,
            ts_ms: 0,
        }];
        let items = build_message_items(&records, /* is_channel */ false, "Bob", &["Bob"]);
        assert_eq!(items[0].text, "hey @Bob check this out");
        assert_eq!(items[0].mention_tier, 2);
    }

    #[test]
    fn build_message_items_renders_other_mention_in_received_channel_message_after_prefix_split() {
        // Sender-prefix split (from_name) and mention flattening both apply
        // to the same received channel message, in sequence: prefix comes
        // off first, then the body is scanned for mentions — the two
        // features this render rework couples on purpose. This node's own
        // name is "Carol" — the mention is of "Bob", a different node, so it
        // must tier as `Other` (1), not `SelfMention`.
        let records = vec![MessageRecord {
            text: "Alice: hi @[Bob] check this out".into(),
            is_ours: false,
            acked: false,
            ts_ms: 0,
        }];
        let items = build_message_items(
            &records,
            /* is_channel */ true,
            "Carol",
            &["Carol", "Bob", "Alice"],
        );
        assert_eq!(items[0].from_name, "Alice");
        assert_eq!(items[0].text, "hi @Bob check this out");
        assert_eq!(items[0].mention_tier, 1);
    }

    #[test]
    fn build_message_items_mention_free_text_is_tier_zero_unchanged() {
        let records = vec![MessageRecord {
            text: "no mentions here".into(),
            is_ours: false,
            acked: false,
            ts_ms: 0,
        }];
        let items = build_message_items(&records, /* is_channel */ false, "Bob", &["Bob"]);
        assert_eq!(items[0].text, "no mentions here");
        assert_eq!(items[0].mention_tier, 0);
    }

    #[test]
    fn build_message_items_sent_message_mentions_also_render() {
        // Mentions are not receive-only: a self-composed mention (already
        // wire-wrapped by `UiRuntime::on_send_message`) must render
        // identically through the same one code path.
        let records = vec![MessageRecord {
            text: "hi @[Alice] it's Bob".into(),
            is_ours: true,
            acked: false,
            ts_ms: 0,
        }];
        let items = build_message_items(
            &records,
            /* is_channel */ false,
            "Bob",
            &["Bob", "Alice"],
        );
        assert_eq!(items[0].text, "hi @Alice it's Bob");
        assert_eq!(items[0].mention_tier, 1);
    }
}
