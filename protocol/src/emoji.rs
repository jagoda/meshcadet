// SPDX-License-Identifier: GPL-3.0-only
//! Curated emoji set for MeshCadet.
//!
//! # Scope
//! A fixed set of 40 emoji chosen for:
//! - Broad, universal recognisability
//! - Absence of violence, adult content, or ambiguous meaning
//! - Coverage of common positive reactions (happy, love, fun, nature)
//!
//! # Wire format
//! Emoji are transmitted as UTF-8 code points — no escaping or shortcode syntax
//! on the wire.  `:shortcode:` syntax is a **compose-time** and
//! **display-time** convenience.  `expand_shortcodes` converts `:word:` tokens
//! in an outgoing message to their Unicode code point before the text is
//! encrypted and sent.  The receiver renders the UTF-8 string directly; the
//! shortcode is never transmitted.
//!
//! # no_std compatibility
//! This module is `no_std`-compatible: no heap allocation is required.
//! `expand_shortcodes` writes into a caller-supplied output buffer.
//! The emoji table is a `const` slice; lookup is a linear scan (O(N), N=40,
//! fast enough for interactive compose).

/// One entry in the curated emoji table.
#[derive(Clone, Copy, Debug)]
pub struct EmojiEntry {
    /// Slack-style shortcode, without the surrounding `:` delimiters.
    pub shortcode: &'static str,
    /// Unicode scalar value.
    pub codepoint: char,
    /// Short human-readable label for the emoji picker grid.
    pub label: &'static str,
}

/// The canonical 40-entry curated emoji set.
///
/// Broadly recognisable.  No violence, adult content, or ambiguous sentiment.
pub const EMOJI_TABLE: &[EmojiEntry] = &[
    // ── Faces ────────────────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "smile",
        codepoint: '😊',
        label: "Smile",
    },
    EmojiEntry {
        shortcode: "laugh",
        codepoint: '😂',
        label: "Laugh",
    },
    EmojiEntry {
        shortcode: "wink",
        codepoint: '😉',
        label: "Wink",
    },
    EmojiEntry {
        shortcode: "cool",
        codepoint: '😎',
        label: "Cool",
    },
    // BUG FIX: U+1F914 (🤔) is
    // outside the coverage of the bundled `NotoEmoji-Regular.ttf` (it only
    // covers emoji through ~Unicode 8.0; 🤔/🤗 are Unicode 9.0 additions) —
    // `firmware/gen_emoji_font.c`'s build-time no-blank-glyph check
    // fails the build on it. Swapped for 😕 (U+1F615,
    // confirmed present in the bundled font), the closest
    // "hmm/not sure" analog available. Shortcode/label unchanged.
    EmojiEntry {
        shortcode: "think",
        codepoint: '😕',
        label: "Hmm",
    },
    EmojiEntry {
        shortcode: "wow",
        codepoint: '😲',
        label: "Wow",
    },
    EmojiEntry {
        shortcode: "sleepy",
        codepoint: '😴',
        label: "Sleepy",
    },
    EmojiEntry {
        shortcode: "silly",
        codepoint: '😜',
        label: "Silly",
    },
    EmojiEntry {
        shortcode: "happy",
        codepoint: '😁',
        label: "Happy",
    },
    EmojiEntry {
        shortcode: "sad",
        codepoint: '😢',
        label: "Sad",
    },
    // ── Gestures ─────────────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "wave",
        codepoint: '👋',
        label: "Wave",
    },
    EmojiEntry {
        shortcode: "thumbsup",
        codepoint: '👍',
        label: "Thumbs Up",
    },
    EmojiEntry {
        shortcode: "clap",
        codepoint: '👏',
        label: "Clap",
    },
    EmojiEntry {
        shortcode: "highfive",
        codepoint: '🙏',
        label: "High Five",
    },
    EmojiEntry {
        shortcode: "fist",
        codepoint: '✊',
        label: "Fist Bump",
    },
    EmojiEntry {
        shortcode: "point",
        codepoint: '👆',
        label: "Point Up",
    },
    EmojiEntry {
        shortcode: "ok",
        codepoint: '👌',
        label: "OK",
    },
    // ── Love / Feelings ──────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "heart",
        codepoint: '❤',
        label: "Heart",
    },
    // BUG FIX: same font-coverage
    // gap as "think" above — U+1F917 (🤗) is a Unicode 9.0 addition absent
    // from the bundled emoji font. Swapped for 😘 (U+1F618, confirmed
    // present), the closest available affectionate face. Shortcode/label
    // unchanged.
    EmojiEntry {
        shortcode: "hug",
        codepoint: '😘',
        label: "Hug",
    },
    EmojiEntry {
        shortcode: "sparkles",
        codepoint: '✨',
        label: "Sparkles",
    },
    EmojiEntry {
        shortcode: "star",
        codepoint: '⭐',
        label: "Star",
    },
    EmojiEntry {
        shortcode: "rainbow",
        codepoint: '🌈',
        label: "Rainbow",
    },
    // ── Nature ───────────────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "sun",
        codepoint: '☀',
        label: "Sun",
    },
    EmojiEntry {
        shortcode: "moon",
        codepoint: '🌙',
        label: "Moon",
    },
    EmojiEntry {
        shortcode: "cloud",
        codepoint: '⛅',
        label: "Cloud",
    },
    EmojiEntry {
        shortcode: "flower",
        codepoint: '🌸',
        label: "Flower",
    },
    EmojiEntry {
        shortcode: "tree",
        codepoint: '🌲',
        label: "Tree",
    },
    EmojiEntry {
        shortcode: "leaf",
        codepoint: '🍃',
        label: "Leaf",
    },
    EmojiEntry {
        shortcode: "dog",
        codepoint: '🐶',
        label: "Dog",
    },
    EmojiEntry {
        shortcode: "cat",
        codepoint: '🐱',
        label: "Cat",
    },
    EmojiEntry {
        shortcode: "rabbit",
        codepoint: '🐰',
        label: "Rabbit",
    },
    // ── Objects / Fun ────────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "music",
        codepoint: '🎵',
        label: "Music",
    },
    EmojiEntry {
        shortcode: "game",
        codepoint: '🎮',
        label: "Game",
    },
    EmojiEntry {
        shortcode: "ball",
        codepoint: '⚽',
        label: "Ball",
    },
    EmojiEntry {
        shortcode: "cake",
        codepoint: '🎂',
        label: "Cake",
    },
    EmojiEntry {
        shortcode: "pizza",
        codepoint: '🍕',
        label: "Pizza",
    },
    EmojiEntry {
        shortcode: "rocket",
        codepoint: '🚀',
        label: "Rocket",
    },
    EmojiEntry {
        shortcode: "fire",
        codepoint: '🔥',
        label: "Fire",
    },
    // ── Communication ────────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "radio",
        codepoint: '📻',
        label: "Radio",
    },
    EmojiEntry {
        shortcode: "check",
        codepoint: '✅',
        label: "Done",
    },
];

/// Look up an emoji entry by shortcode.
///
/// Returns `None` if the shortcode is not in the curated set.
///
/// # Example
/// ```
/// # use protocol::emoji::lookup_shortcode;
/// let entry = lookup_shortcode("smile").unwrap();
/// assert_eq!(entry.codepoint, '😊');
/// ```
pub fn lookup_shortcode(code: &str) -> Option<&'static EmojiEntry> {
    EMOJI_TABLE.iter().find(|e| e.shortcode == code)
}

/// Expand all `:shortcode:` tokens in `input` and write the result into `out`.
///
/// Returns the number of bytes written to `out`, or `None` if `out` is too
/// small to hold the result.  Unrecognised shortcodes (not in `EMOJI_TABLE`)
/// are passed through literally (`:unknown:` → `:unknown:`).
///
/// This function is `no_std`-compatible: no heap allocation is performed.
///
/// # Example
/// ```
/// # use protocol::emoji::expand_shortcodes;
/// let mut out = [0u8; 64];
/// let n = expand_shortcodes(b"Hello :smile: world!", &mut out).unwrap();
/// let result = core::str::from_utf8(&out[..n]).unwrap();
/// assert!(result.contains('\u{1F60A}'));
/// ```
pub fn expand_shortcodes(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut in_pos = 0usize;
    let mut out_pos = 0usize;

    while in_pos < input.len() {
        if input[in_pos] == b':' {
            // Search for the closing ':'.
            let start = in_pos + 1;
            let mut end = start;
            while end < input.len() && input[end] != b':' && input[end] != b' ' {
                end += 1;
            }
            if end < input.len() && input[end] == b':' && end > start {
                // We have a candidate shortcode in input[start..end].
                let code = core::str::from_utf8(&input[start..end]).unwrap_or("");
                if let Some(entry) = lookup_shortcode(code) {
                    // Encode the code point as UTF-8 into out.
                    let mut cp_buf = [0u8; 4];
                    let encoded = entry.codepoint.encode_utf8(&mut cp_buf);
                    let encoded_bytes = encoded.as_bytes();
                    if out_pos + encoded_bytes.len() > out.len() {
                        return None; // output buffer exhausted
                    }
                    out[out_pos..out_pos + encoded_bytes.len()].copy_from_slice(encoded_bytes);
                    out_pos += encoded_bytes.len();
                    in_pos = end + 1; // skip past the closing ':'
                    continue;
                }
            }
            // Not a recognised shortcode — emit the ':' literally.
            if out_pos >= out.len() {
                return None;
            }
            out[out_pos] = b':';
            out_pos += 1;
            in_pos += 1;
        } else {
            if out_pos >= out.len() {
                return None;
            }
            out[out_pos] = input[in_pos];
            out_pos += 1;
            in_pos += 1;
        }
    }
    Some(out_pos)
}

/// Extract all shortcodes present in a UTF-8 byte slice.
///
/// Returns the number of matches written into `found` (a caller-supplied
/// buffer of shortcode `str` references).  Use this in the compose screen
/// to show completion candidates as the user types.
pub fn shortcode_completions(prefix: &str, found: &mut [&'static str]) -> usize {
    let mut count = 0;
    for entry in EMOJI_TABLE {
        if count >= found.len() {
            break;
        }
        if entry.shortcode.starts_with(prefix) {
            found[count] = entry.shortcode;
            count += 1;
        }
    }
    count
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_forty_entries() {
        assert_eq!(EMOJI_TABLE.len(), 40);
    }

    #[test]
    fn all_shortcodes_are_unique() {
        for (i, a) in EMOJI_TABLE.iter().enumerate() {
            for (j, b) in EMOJI_TABLE.iter().enumerate() {
                if i != j {
                    assert_ne!(
                        a.shortcode, b.shortcode,
                        "duplicate shortcode: {}",
                        a.shortcode
                    );
                }
            }
        }
    }

    #[test]
    fn lookup_known_shortcodes() {
        assert_eq!(lookup_shortcode("smile").unwrap().codepoint, '😊');
        assert_eq!(lookup_shortcode("heart").unwrap().codepoint, '❤');
        assert_eq!(lookup_shortcode("rocket").unwrap().codepoint, '🚀');
        assert_eq!(lookup_shortcode("wave").unwrap().codepoint, '👋');
        assert_eq!(lookup_shortcode("thumbsup").unwrap().codepoint, '👍');
    }

    #[test]
    fn lookup_unknown_shortcode_returns_none() {
        assert!(lookup_shortcode("unknown").is_none());
        assert!(lookup_shortcode("").is_none());
    }

    #[test]
    fn expand_shortcodes_basic() {
        let mut out = [0u8; 64];
        let n = expand_shortcodes(b"hi :smile:", &mut out).unwrap();
        let s = core::str::from_utf8(&out[..n]).unwrap();
        assert!(s.starts_with("hi "));
        assert!(s.contains('😊'));
    }

    #[test]
    fn expand_shortcodes_unknown_passes_through() {
        let mut out = [0u8; 64];
        let n = expand_shortcodes(b":unknownthing: ok", &mut out).unwrap();
        let s = core::str::from_utf8(&out[..n]).unwrap();
        assert!(s.starts_with(':'));
        assert!(s.contains("ok"));
    }

    #[test]
    fn expand_shortcodes_multiple() {
        let mut out = [0u8; 128];
        let n = expand_shortcodes(b":heart: you :rocket:", &mut out).unwrap();
        let s = core::str::from_utf8(&out[..n]).unwrap();
        assert!(s.contains('❤'));
        assert!(s.contains("you"));
        assert!(s.contains('🚀'));
    }

    #[test]
    fn expand_shortcodes_no_shortcodes() {
        let input = b"hello world";
        let mut out = [0u8; 64];
        let n = expand_shortcodes(input, &mut out).unwrap();
        assert_eq!(&out[..n], input);
    }

    #[test]
    fn shortcode_completions_prefix() {
        let mut found = [""; 10];
        let n = shortcode_completions("s", &mut found);
        assert!(n > 0);
        for &sc in &found[..n] {
            assert!(
                sc.starts_with('s'),
                "completion {sc:?} doesn't start with 's'"
            );
        }
    }

    #[test]
    fn shortcode_completions_no_match() {
        let mut found = [""; 10];
        let n = shortcode_completions("zzz", &mut found);
        assert_eq!(n, 0);
    }
}
