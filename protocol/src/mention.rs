// SPDX-License-Identifier: GPL-3.0-only
//! @-mention wrap / split — MeshCore wire convention `@[<name>]`.
//!
//! # Wire format
//! A mention travels on-air as `@[<name>]` — the brackets are part of the
//! wire text (mirrors `format_channel_text`'s `"<name>: "` sender prefix:
//! MeshCore-style plaintext markup embedded in the message body, not a
//! separate framed field). Users type and see `@<name>` — brackets are
//! never shown in any rendered view; they exist purely to delimit a name
//! that may itself contain whitespace (`@[Chicken Little]`), which is why
//! the parse below never tokenizes on a bare space.
//!
//! # Two entry points
//! - [`wrap_mentions`] — SEND side. Scans composed text for `@<name>` where
//!   `<name>` matches (longest-match) a caller-supplied set of known names,
//!   and rewrites it to `@[<name>]`. `no_std`, buffer-based (mirrors
//!   [`crate::emoji::expand_shortcodes`]).
//! - [`split_mentions`] — RECEIVE/RENDER side. An iterator over `MentionRun`s:
//!   alternating spans of plain text and mention names (brackets already
//!   stripped), each tagged with a [`MentionTier`] so the caller can style
//!   self-mentions more prominently than other-node mentions. `no_std`,
//!   allocation-free (borrows from the input `&str`).
//!
//! Both sides share one definition of "known name" matching: longest-match
//! against the caller's known-names set, with the token boundary rule (`@`
//! must start-of-text or be preceded by whitespace — excludes `a@b` email
//! shapes) and a terminator rule (the char immediately after the matched
//! name, if any, must not be alphanumeric — excludes false-positive prefix
//! matches like matching `"Bob"` out of `"@Bobby"` when only `"Bob"` is
//! known). Names containing `]` are unsupported (a documented abort
//! condition — no MeshCadet-known name is expected to contain one).

/// Append `src` to `out` starting at `w`, clamped to the buffer; returns the
/// new write cursor. Mirrors `codec::append_clamped` (private there; this is
/// a separate small copy rather than a cross-module `pub(crate)` reach, to
/// keep this module a self-contained mirror of `emoji.rs`'s style).
#[inline]
fn append_clamped(out: &mut [u8], w: usize, src: &[u8]) -> usize {
    let take = src.len().min(out.len().saturating_sub(w));
    out[w..w + take].copy_from_slice(&src[..take]);
    w + take
}

/// `true` when byte index `i` in `text` is a valid mention-start boundary:
/// start-of-text, or the preceding byte is ASCII whitespace. Excludes `a@b`
/// email shapes (preceded by `a`, not whitespace).
fn is_token_boundary(text: &str, i: usize) -> bool {
    i == 0 || text.as_bytes()[i - 1].is_ascii_whitespace()
}

/// Longest known name that `after` starts with, subject to the terminator
/// rule (see module doc). `known` order does not matter — every candidate is
/// checked and the longest winner is kept.
fn longest_match<'k>(after: &str, known: &[&'k str]) -> Option<&'k str> {
    let mut best: Option<&str> = None;
    for &name in known {
        if name.is_empty() || !after.starts_with(name) {
            continue;
        }
        let terminator_ok = match after[name.len()..].chars().next() {
            None => true,
            Some(c) => !c.is_alphanumeric(),
        };
        if terminator_ok && best.is_none_or(|b: &str| name.len() > b.len()) {
            best = Some(name);
        }
    }
    best
}

/// SEND side: rewrite `@<name>` → `@[<name>]` for every `<name>` in `input`
/// that longest-matches an entry of `known` at a token boundary.
///
/// `known` is the caller's known-names set (contacts ∪ this node's own name
/// — a self-mention should match too). An `@word` that matches nothing in `known`
/// is left verbatim (not wrapped) — the "leave `@foo` alone" acceptance
/// criterion.
///
/// Returns the number of bytes written to `out`, or `None` if `input` is not
/// valid UTF-8 or `out` is too small to hold the result (same contract shape
/// as [`crate::emoji::expand_shortcodes`]).
///
/// # Example
/// ```
/// # use protocol::mention::wrap_mentions;
/// let mut out = [0u8; 64];
/// let n = wrap_mentions(b"hi @Alice!", &["Alice", "Bob"], &mut out).unwrap();
/// assert_eq!(&out[..n], b"hi @[Alice]!");
/// ```
pub fn wrap_mentions(input: &[u8], known: &[&str], out: &mut [u8]) -> Option<usize> {
    let text = core::str::from_utf8(input).ok()?;
    let bytes = text.as_bytes();
    let mut w = 0usize;
    let mut cursor = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'@' && is_token_boundary(text, i) {
            if let Some(name) = longest_match(&text[i + 1..], known) {
                w = append_clamped(out, w, &bytes[cursor..i]);
                w = append_clamped(out, w, b"@[");
                w = append_clamped(out, w, name.as_bytes());
                w = append_clamped(out, w, b"]");
                i += 1 + name.len();
                cursor = i;
                continue;
            }
        }
        i += 1;
    }
    w = append_clamped(out, w, &bytes[cursor..]);
    // append_clamped silently truncates rather than signalling overflow;
    // detect it the same way expand_shortcodes' explicit bounds checks do,
    // so a too-small `out` is a `None`, not a silently truncated wire write.
    if w < required_len(text, known) {
        return None;
    }
    Some(w)
}

/// Recompute the exact wrapped length of `text` under `known` — used by
/// [`wrap_mentions`] purely to detect output-buffer overflow (see its doc).
/// Not `pub`: an internal double-pass rather than threading an overflow flag
/// through `append_clamped`.
fn required_len(text: &str, known: &[&str]) -> usize {
    let bytes = text.as_bytes();
    let mut needed = 0usize;
    let mut cursor = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'@' && is_token_boundary(text, i) {
            if let Some(name) = longest_match(&text[i + 1..], known) {
                needed += i - cursor;
                needed += 2 + name.len() + 1; // "@[" + name + "]"
                i += 1 + name.len();
                cursor = i;
                continue;
            }
        }
        i += 1;
    }
    needed += bytes.len() - cursor;
    needed
}

/// Tier of a mention run — drives render prominence. Ordered so the
/// "highest tier wins" reduction (a message with both a self- and an
/// other-node mention renders as self-tier) is a plain `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MentionTier {
    /// Not a mention — ordinary message text.
    Plain = 0,
    /// A mention of some other node's name.
    Other = 1,
    /// A mention of THIS node's own name — renders more prominently than
    /// [`MentionTier::Other`].
    SelfMention = 2,
}

/// One run of display text with its mention tier. For `Plain`, `text` is a
/// verbatim slice of the input (may span multiple words). For `Other` /
/// `SelfMention`, `text` is the bare name (brackets AND the leading `@`
/// already stripped) — callers render a mention by prepending `@` to `text`
/// themselves (kept out of this borrowed-`&str` run so this module stays
/// alloc-free; see module doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MentionRun<'a> {
    pub text: &'a str,
    pub tier: MentionTier,
}

/// Locate the next mention in `text` (bracketed `@[name]`, the authoritative
/// wire form, or a best-effort bare `@name` matching `known`), scanning past
/// non-matching `@` occurrences (e.g. `a@b`, or an unrecognised `@word`).
///
/// Returns `(start_byte, wire_end_byte_exclusive, tier, name)` for the first
/// match, or `None` if `text` has no mention.
fn find_next_mention<'a>(
    text: &'a str,
    self_name: &str,
    known: &[&str],
) -> Option<(usize, usize, MentionTier, &'a str)> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'@' && is_token_boundary(text, i) {
            let after = &text[i + 1..];
            if let Some(stripped) = after.strip_prefix('[') {
                if let Some(close_rel) = stripped.find(']') {
                    let name = &stripped[..close_rel];
                    let wire_end = i + 1 + 1 + close_rel + 1; // '@' + '[' + name + ']'
                    let tier = if !name.is_empty() && name == self_name {
                        MentionTier::SelfMention
                    } else {
                        MentionTier::Other
                    };
                    return Some((i, wire_end, tier, name));
                }
                // Unterminated '@[': not a well-formed mention — keep
                // scanning past this '@' rather than treating '[' as plain.
            } else if let Some(matched) = longest_match(after, known) {
                // Bare (unbracketed) mention — brackets are authoritative,
                // so this is best-effort and always tiers as `Other`, even
                // when `matched == self_name` (see module doc).
                // Slice the matched length out of `after` (borrowed from
                // `text`, lifetime `'a`) rather than returning `matched`
                // itself (borrowed from `known`, an unrelated lifetime).
                let name = &after[..matched.len()];
                let wire_end = i + 1 + name.len();
                return Some((i, wire_end, MentionTier::Other, name));
            }
        }
        i += 1;
    }
    None
}

/// RECEIVE/RENDER side: an allocation-free iterator over the `MentionRun`s
/// of `wire` text — alternating `Plain` spans and mention names, brackets
/// already stripped. See [`split_mentions`].
pub struct MentionRuns<'a> {
    text: &'a str,
    self_name: &'a str,
    known: &'a [&'a str],
    pos: usize,
}

impl<'a> Iterator for MentionRuns<'a> {
    type Item = MentionRun<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.text.len() {
            return None;
        }
        let rest = &self.text[self.pos..];
        match find_next_mention(rest, self.self_name, self.known) {
            Some((0, wire_end, tier, name)) => {
                self.pos += wire_end;
                Some(MentionRun { text: name, tier })
            }
            Some((start, ..)) => {
                self.pos += start;
                Some(MentionRun {
                    text: &rest[..start],
                    tier: MentionTier::Plain,
                })
            }
            None => {
                self.pos = self.text.len();
                Some(MentionRun {
                    text: rest,
                    tier: MentionTier::Plain,
                })
            }
        }
    }
}

/// Split `wire` text into an ordered sequence of [`MentionRun`]s.
///
/// `self_name` is this node's own display name (drives the `SelfMention` vs
/// `Other` tier decision on the authoritative bracketed form); `known` is
/// the best-effort name set consulted for unbracketed `@name` occurrences
/// (see [`find_next_mention`]'s doc — brackets are authoritative).
///
/// # Example
/// ```
/// # use protocol::mention::{split_mentions, MentionTier};
/// let runs: Vec<_> = split_mentions("hi @[Chicken Little] how are you", "Rex", &[]).collect();
/// assert_eq!(runs[0].text, "hi ");
/// assert_eq!(runs[0].tier, MentionTier::Plain);
/// assert_eq!(runs[1].text, "Chicken Little");
/// assert_eq!(runs[1].tier, MentionTier::Other);
/// assert_eq!(runs[2].text, " how are you");
/// ```
pub fn split_mentions<'a>(
    wire: &'a str,
    self_name: &'a str,
    known: &'a [&'a str],
) -> MentionRuns<'a> {
    MentionRuns {
        text: wire,
        self_name,
        known,
        pos: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── wrap_mentions ────────────────────────────────────────────────────

    #[test]
    fn wrap_mentions_wraps_known_name() {
        let mut out = [0u8; 64];
        let n = wrap_mentions(b"hi @Alice!", &["Alice", "Bob"], &mut out).unwrap();
        assert_eq!(&out[..n], b"hi @[Alice]!");
    }

    #[test]
    fn wrap_mentions_leaves_unknown_name_verbatim() {
        let mut out = [0u8; 64];
        let n = wrap_mentions(b"hi @foo!", &["Alice", "Bob"], &mut out).unwrap();
        assert_eq!(&out[..n], b"hi @foo!");
    }

    #[test]
    fn wrap_mentions_rejects_email_shape() {
        // '@' preceded by a non-whitespace char ('a') is not a token
        // boundary — must not wrap even if "b" happened to be a known name.
        let mut out = [0u8; 64];
        let n = wrap_mentions(b"a@b", &["b"], &mut out).unwrap();
        assert_eq!(&out[..n], b"a@b");
    }

    #[test]
    fn wrap_mentions_handles_multiword_name() {
        let mut out = [0u8; 64];
        let n = wrap_mentions(
            b"tell @Chicken Little the sky is falling",
            &["Chicken Little"],
            &mut out,
        )
        .unwrap();
        assert_eq!(&out[..n], b"tell @[Chicken Little] the sky is falling");
    }

    #[test]
    fn wrap_mentions_longest_match_wins() {
        // Both "Bob" and "Bobby" known; "@Bobby" must wrap as a whole, not
        // as "@[Bob]by".
        let mut out = [0u8; 64];
        let n = wrap_mentions(b"hey @Bobby", &["Bob", "Bobby"], &mut out).unwrap();
        assert_eq!(&out[..n], b"hey @[Bobby]");
    }

    #[test]
    fn wrap_mentions_short_name_not_matched_as_prefix_of_longer_word() {
        // Only "Bob" known but the text says "@Bobby" — "Bob" is a prefix
        // but the terminator rule (next char 'b' is alphanumeric) rejects
        // it, so nothing wraps.
        let mut out = [0u8; 64];
        let n = wrap_mentions(b"hey @Bobby", &["Bob"], &mut out).unwrap();
        assert_eq!(&out[..n], b"hey @Bobby");
    }

    #[test]
    fn wrap_mentions_start_of_text_boundary() {
        let mut out = [0u8; 64];
        let n = wrap_mentions(b"@Alice hi", &["Alice"], &mut out).unwrap();
        assert_eq!(&out[..n], b"@[Alice] hi");
    }

    #[test]
    fn wrap_mentions_none_on_output_overflow() {
        let mut out = [0u8; 4];
        assert!(wrap_mentions(b"hi @Alice!", &["Alice"], &mut out).is_none());
    }

    // ── split_mentions ───────────────────────────────────────────────────

    #[test]
    fn split_mentions_bracketed_other() {
        let runs: Vec<_> = split_mentions("hi @[Alice] there", "Bob", &[]).collect();
        assert_eq!(runs.len(), 3);
        assert_eq!(
            runs[0],
            MentionRun {
                text: "hi ",
                tier: MentionTier::Plain
            }
        );
        assert_eq!(
            runs[1],
            MentionRun {
                text: "Alice",
                tier: MentionTier::Other
            }
        );
        assert_eq!(
            runs[2],
            MentionRun {
                text: " there",
                tier: MentionTier::Plain
            }
        );
    }

    #[test]
    fn split_mentions_bracketed_self_is_more_prominent_tier() {
        let runs: Vec<_> = split_mentions("hi @[Bob] there", "Bob", &[]).collect();
        assert_eq!(
            runs[1],
            MentionRun {
                text: "Bob",
                tier: MentionTier::SelfMention
            }
        );
        assert!(MentionTier::SelfMention > MentionTier::Other);
    }

    #[test]
    fn split_mentions_multiword_name_not_tokenized_on_space() {
        let runs: Vec<_> =
            split_mentions("@[Chicken Little] the sky is falling", "Rex", &[]).collect();
        assert_eq!(
            runs[0],
            MentionRun {
                text: "Chicken Little",
                tier: MentionTier::Other
            }
        );
        assert_eq!(runs[1].text, " the sky is falling");
    }

    #[test]
    fn split_mentions_no_mention_is_single_plain_run() {
        let runs: Vec<_> = split_mentions("just a plain message", "Bob", &[]).collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0],
            MentionRun {
                text: "just a plain message",
                tier: MentionTier::Plain
            }
        );
    }

    #[test]
    fn split_mentions_bare_mention_is_best_effort_other_even_for_self_name() {
        // Brackets are authoritative — an unbracketed "@Bob" tiers as
        // `Other`, never `SelfMention`, even when "Bob" is self_name.
        let runs: Vec<_> = split_mentions("hi @Bob there", "Bob", &["Bob"]).collect();
        assert_eq!(
            runs[1],
            MentionRun {
                text: "Bob",
                tier: MentionTier::Other
            }
        );
    }

    #[test]
    fn split_mentions_unknown_bare_at_word_stays_plain() {
        let runs: Vec<_> = split_mentions("hi @nobody there", "Bob", &["Alice"]).collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].tier, MentionTier::Plain);
    }

    #[test]
    fn split_mentions_round_trip_hides_brackets() {
        let mut wire_buf = [0u8; 64];
        let n = wrap_mentions(
            b"a mention of @Chicken Little arrives",
            &["Chicken Little"],
            &mut wire_buf,
        )
        .unwrap();
        let wire = core::str::from_utf8(&wire_buf[..n]).unwrap();
        assert_eq!(wire, "a mention of @[Chicken Little] arrives");

        let mut display = String::new();
        for run in split_mentions(wire, "Rex", &[]) {
            if run.tier == MentionTier::Plain {
                display.push_str(run.text);
            } else {
                display.push('@');
                display.push_str(run.text);
            }
        }
        assert_eq!(display, "a mention of @Chicken Little arrives");
        assert!(!display.contains('['));
        assert!(!display.contains(']'));
    }

    #[test]
    fn split_mentions_empty_text_yields_no_runs() {
        assert_eq!(split_mentions("", "Bob", &[]).count(), 0);
    }

    // ── Adversarial / untrusted-input safety ────────────────────────────
    //
    // `split_mentions` runs on wire text from remote mesh peers — untrusted
    // input this node did not compose. Post-green review criterion #6
    // (security & input validation): these pin "never panics, degrades to
    // plain text" for the malformed shapes a hostile or buggy peer could
    // send, rather than relying on code-reading alone.

    #[test]
    fn split_mentions_unterminated_bracket_does_not_panic_and_stays_plain() {
        let runs: Vec<_> = split_mentions("hi @[Alice never closes", "Bob", &[]).collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].tier, MentionTier::Plain);
        assert_eq!(runs[0].text, "hi @[Alice never closes");
    }

    #[test]
    fn split_mentions_empty_bracket_is_a_mention_with_empty_name() {
        let runs: Vec<_> = split_mentions("hi @[] there", "Bob", &[]).collect();
        assert_eq!(
            runs[1],
            MentionRun {
                text: "",
                tier: MentionTier::Other
            }
        );
    }

    #[test]
    fn split_mentions_lone_at_at_end_of_text_does_not_panic() {
        let runs: Vec<_> = split_mentions("trailing @", "Bob", &[]).collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].tier, MentionTier::Plain);
        assert_eq!(runs[0].text, "trailing @");
    }

    #[test]
    fn split_mentions_bracket_immediately_at_end_of_text_does_not_panic() {
        let runs: Vec<_> = split_mentions("trailing @[", "Bob", &[]).collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].tier, MentionTier::Plain);
    }

    #[test]
    fn split_mentions_adjacent_mentions_with_no_gap() {
        let runs: Vec<_> = split_mentions("@[Alice]@[Bob]", "Bob", &[]).collect();
        assert_eq!(runs.len(), 2);
        assert_eq!(
            runs[0],
            MentionRun {
                text: "Alice",
                tier: MentionTier::Other
            }
        );
        assert_eq!(
            runs[1],
            MentionRun {
                text: "Bob",
                tier: MentionTier::SelfMention
            }
        );
    }

    #[test]
    fn split_mentions_unicode_name_inside_brackets_does_not_panic() {
        // Non-ASCII display names (accented / multi-byte characters) must
        // slice at valid UTF-8 boundaries.
        let runs: Vec<_> = split_mentions("hi @[Zoë Müller] there", "Bob", &[]).collect();
        assert_eq!(
            runs[1],
            MentionRun {
                text: "Zoë Müller",
                tier: MentionTier::Other
            }
        );
    }

    #[test]
    fn split_mentions_bracket_containing_only_whitespace_does_not_panic() {
        let runs: Vec<_> = split_mentions("hi @[   ] there", "Bob", &[]).collect();
        assert_eq!(runs[1].tier, MentionTier::Other);
        assert_eq!(runs[1].text, "   ");
    }

    #[test]
    fn wrap_mentions_rejects_invalid_utf8_input() {
        let mut out = [0u8; 64];
        // Lone continuation byte — never valid UTF-8 on its own.
        let invalid = [0x40u8, 0x80u8, 0x41u8]; // '@', 0x80, 'A'
        assert!(wrap_mentions(&invalid, &["A"], &mut out).is_none());
    }
}
