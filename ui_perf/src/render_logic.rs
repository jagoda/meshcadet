// SPDX-License-Identifier: GPL-3.0-only
//! Host port of `UiRuntime::build_message_items` / `UiRuntime::render_mentions`
//! (`firmware/src/ui/mod.rs:2477` / `:2514`) — the MessageView state-build hot
//! path. See the crate root doc for why this is a port rather than a call
//! into `firmware` directly, and the drift contract that comes with that.
//!
//! Both ported functions are IDENTICAL in algorithm and allocation shape to
//! the firmware originals: same `protocol::codec::parse_channel_text` /
//! `protocol::mention::split_mentions` calls, same `String`/`Vec`
//! construction order. The only difference is the output type: firmware's
//! `MessageItem` is a Slint-macro-generated struct (requires the Slint
//! compiler + `.slint` markup to produce); [`BenchMessageItem`] here is a
//! plain Rust struct with the identical field set, so this crate needs no
//! Slint dependency at all.

/// One stored message — mirrors `firmware::ui::MessageRecord`
/// (`firmware/src/ui/mod.rs:625`) exactly (same three fields read by
/// `build_message_items`; `ts_ms` omitted since the real struct's own doc
/// notes it is captured-but-unread by any current renderer).
#[derive(Clone, Debug)]
pub struct MessageRecord {
    pub text: String,
    pub is_ours: bool,
    pub acked: bool,
}

/// Mirrors `firmware::ui::screens::message_view::MessageItem` (the Slint
/// `MessageBubble` model row) field-for-field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchMessageItem {
    pub text: String,
    pub from_name: String,
    pub time_str: String,
    pub is_ours: bool,
    pub acked: bool,
    pub mention_tier: i32,
}

/// Port of `UiRuntime::render_mentions` (`firmware/src/ui/mod.rs:2514`).
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

/// Port of `UiRuntime::build_message_items` (`firmware/src/ui/mod.rs:2477`).
pub fn build_message_items(
    records: &[MessageRecord],
    is_channel: bool,
    self_name: &str,
    known: &[&str],
) -> Vec<BenchMessageItem> {
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
            BenchMessageItem {
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── Pinned against firmware/src/ui/mod.rs's own #[cfg(test)] fixtures ──
    // Each test name matches its firmware mirror 1:1 (see the source line
    // cited) so a future divergence is easy to trace back to what changed.

    #[test]
    fn build_message_items_splits_prefix_on_received_channel_message() {
        // mirrors firmware/src/ui/mod.rs:3295
        let records = vec![MessageRecord {
            text: "Alice: hello there".into(),
            is_ours: false,
            acked: false,
        }];
        let items = build_message_items(&records, true, "Self", &[]);
        assert_eq!(items[0].from_name, "Alice");
        assert_eq!(items[0].text, "hello there");
    }

    #[test]
    fn build_message_items_leaves_sent_channel_message_unprefixed() {
        // mirrors firmware/src/ui/mod.rs:3305
        let records = vec![MessageRecord {
            text: "Alice: hello there".into(),
            is_ours: true,
            acked: false,
        }];
        let items = build_message_items(&records, true, "Self", &[]);
        assert_eq!(items[0].from_name, "");
        assert_eq!(items[0].text, "Alice: hello there");
    }

    #[test]
    fn build_message_items_leaves_dm_message_unprefixed() {
        // mirrors firmware/src/ui/mod.rs:3318
        let records = vec![MessageRecord {
            text: "Alice: hello there".into(),
            is_ours: false,
            acked: false,
        }];
        let items = build_message_items(&records, false, "Self", &[]);
        assert_eq!(items[0].from_name, "");
        assert_eq!(items[0].text, "Alice: hello there");
    }

    #[test]
    fn build_message_items_falls_back_when_channel_text_has_no_prefix() {
        // mirrors firmware/src/ui/mod.rs:3331
        let records = vec![MessageRecord {
            text: "no delimiter here".into(),
            is_ours: false,
            acked: false,
        }];
        let items = build_message_items(&records, true, "Self", &[]);
        assert_eq!(items[0].from_name, "");
        assert_eq!(items[0].text, "no delimiter here");
    }

    #[test]
    fn render_mentions_flattens_brackets_and_reports_no_tier_for_plain_text() {
        // mirrors firmware/src/ui/mod.rs:3381
        let (text, tier) = render_mentions("just a plain message", "Bob", &[]);
        assert_eq!(text, "just a plain message");
        assert_eq!(tier, 0);
    }

    #[test]
    fn render_mentions_other_node_mention_is_tier_1() {
        // mirrors firmware/src/ui/mod.rs:3388
        let (text, tier) = render_mentions("hi @[Alice] there", "Bob", &[]);
        assert_eq!(text, "hi @Alice there");
        assert_eq!(tier, 1);
    }

    #[test]
    fn render_mentions_self_mention_is_tier_2_more_prominent_than_other() {
        // mirrors firmware/src/ui/mod.rs:3397
        let (text, tier) = render_mentions("hi @[Bob] there", "Bob", &[]);
        assert_eq!(text, "hi @Bob there");
        assert_eq!(tier, 2);
    }

    #[test]
    fn render_mentions_multiword_name_not_tokenized_on_space() {
        // mirrors firmware/src/ui/mod.rs:3405
        let (text, tier) =
            render_mentions("watch out @[Chicken Little] the sky is falling", "Rex", &[]);
        assert_eq!(text, "watch out @Chicken Little the sky is falling");
        assert_eq!(tier, 1);
    }

    #[test]
    fn build_message_items_renders_other_mention_in_received_channel_message_after_prefix_split() {
        // mirrors firmware/src/ui/mod.rs:3426
        let records = vec![MessageRecord {
            text: "Alice: hi @[Bob] check this out".into(),
            is_ours: false,
            acked: false,
        }];
        let items = build_message_items(&records, true, "Carol", &["Carol", "Bob", "Alice"]);
        assert_eq!(items[0].from_name, "Alice");
        assert_eq!(items[0].text, "hi @Bob check this out");
        assert_eq!(items[0].mention_tier, 1);
    }
}

/// Synthetic bench inputs — representative conversation shapes, not
/// exercised by the correctness tests above (those pin exact behavior on
/// small fixtures; these exist purely to give `ui_perf_bench` a realistic
/// distribution of plain/DM/channel/mention traffic at scale).
pub mod bench_fixtures {
    use super::MessageRecord;

    /// One synthetic conversation of `n` records, cycling through: a plain
    /// DM, a channel message with a sender prefix, and a channel message
    /// with a sender prefix AND an other-node mention — so every branch in
    /// `build_message_items`/`render_mentions` is exercised proportionally
    /// on every bench run, not just the cheapest path.
    pub fn conversation(n: usize) -> Vec<MessageRecord> {
        (0..n)
            .map(|i| match i % 3 {
                0 => MessageRecord {
                    text: format!(
                        "plain DM body number {i} with a little more text to size it realistically"
                    ),
                    is_ours: i % 2 == 0,
                    acked: true,
                },
                1 => MessageRecord {
                    text: format!("Alice: channel message number {i} reporting status normally"),
                    is_ours: false,
                    acked: false,
                },
                _ => MessageRecord {
                    text: format!("Bob: hey @[Carol] channel message number {i} needs your eyes"),
                    is_ours: false,
                    acked: false,
                },
            })
            .collect()
    }

    /// The known-names set matching `conversation`'s Alice/Bob/Carol cast.
    pub const KNOWN: &[&str] = &["Alice", "Bob", "Carol"];
}
