// SPDX-License-Identifier: GPL-3.0-only
//! Curated emoji set for MeshCadet.
//!
//! # Scope
//! A curated set of 96 emoji (6 categories × 16), chosen for:
//! - Broad, universal recognisability
//! - Absence of violence, adult content, or ambiguous meaning
//! - Coverage of common positive reactions (happy, love, fun, nature)
//!
//! # The picker/render split (meshcadet-emoji-coverage D1)
//! `EMOJI_TABLE` (this file, the send-side picker) is a curated SUBSET of
//! the much larger renderable set (`gen_emoji_font.c`'s `EMOJI_CPS ∪
//! RENDER_EXTRA_CPS`, ~600 codepoints landed by
//! `meshcadet-emoji-render-set-curation`) — not the same thing. Every entry
//! below is drawn from that render set, so growing this table costs only
//! `&'static str` shortcode/label bytes, never a new glyph bitmap: the
//! codepoint is already rasterised into the bundled font one way or
//! another. `xtask::emoji_table_subset_mismatches` enforces
//! `EMOJI_TABLE ⊆ (EMOJI_CPS ∪ RENDER_EXTRA_CPS)` in code — a picker entry
//! whose codepoint isn't in that union renders BLANK on-device. The
//! original 40 entries additionally live in `EMOJI_CPS` itself (the
//! historical, still-valid way to add a picker entry); the 56 grown by
//! `meshcadet-emoji-picker-expansion` instead reuse already-rasterised
//! `RENDER_EXTRA_CPS` codepoints, at zero flash cost — see that mission's
//! dossier and the campaign plan's D1 for the full rationale.
//!
//! # Categories
//! [`EMOJI_CATEGORIES`] lists the 6 picker tabs, in display order. Every
//! [`EmojiEntry::category`] value is one of these 6 strings — pinned by the
//! `every_entry_category_is_a_known_category` test below. The firmware
//! picker (`firmware/src/ui/screens/compose.rs`'s `EmojiPickerGrid`) shows
//! one category's ~16 entries at a time behind a tab row; there is
//! deliberately no search UI here — the `:shortcode:` completion path
//! ([`shortcode_completions`]) already covers that interaction (campaign
//! D1: "why 96 and why tabs-not-search").
//!
//! # Wire format
//! Emoji are transmitted as UTF-8 code points — no escaping or shortcode syntax
//! on the wire.  `:shortcode:` syntax is a **compose-time** and
//! **display-time** convenience.  `expand_shortcodes` converts `:word:` tokens
//! in an outgoing message to their Unicode code point before the text is
//! encrypted and sent.  The receiver renders the UTF-8 string directly; the
//! shortcode is never transmitted.
//!
//! # Inbound normalization (display-time only)
//! Slint's `SoftwareRenderer` resolves a whole text run to a single bitmap
//! font with no per-glyph fallback and no grapheme clustering (see
//! `firmware/src/ui/platform.rs`'s `emoji_font` module doc). The bundled
//! font has no entries for VS16 (`U+FE0F`), skin-tone modifiers
//! (`U+1F3FB..FF`), or ZWJ (`U+200D`) — they are combining characters, not
//! glyphs, and adding them to the font would not help. An unmapped
//! codepoint doesn't just fail to paint; it still consumes a full
//! character cell of horizontal advance (see
//! `i-slint-renderer-software`'s `PixelFont::shape_text`), so e.g. Android's
//! `U+2764 U+FE0F` (❤️) renders as a heart followed by a visible blank
//! cell — even though `U+2764` alone is in the font. [`normalize_inbound`]
//! strips these combining characters before the text ever reaches the
//! renderer: it drops VS16 and skin-tone modifiers outright, and collapses
//! a ZWJ sequence to its lead scalar (`EmojiEntry::codepoint` — and every
//! other codepoint this crate hands to the renderer — is a single `char`,
//! not a grapheme cluster, so this is a deliberate degradation, not a
//! workaround to be fixed later). This is display-time only: it runs on
//! the RECEIVE/RENDER side, never on wire text, and never on the bytes
//! actually sent — an un-normalized peer or the Android companion is
//! unaffected.
//!
//! # no_std compatibility
//! This module is `no_std`-compatible: no heap allocation is required.
//! `expand_shortcodes` and `normalize_inbound` both write into a
//! caller-supplied output buffer (mirrors `crate::mention`'s send/receive
//! buffer-based shape).
//! The emoji table is a `const` slice; lookup is a linear scan (O(N), N=96,
//! fast enough for interactive compose — see the
//! `shortcode_completions_scan_stays_fast_at_n96` benchmark test below for a
//! measurement, not just the O(N) claim).

/// One entry in the curated emoji table.
#[derive(Clone, Copy, Debug)]
pub struct EmojiEntry {
    /// Slack-style shortcode, without the surrounding `:` delimiters.
    pub shortcode: &'static str,
    /// Unicode scalar value.
    pub codepoint: char,
    /// Short human-readable label for the emoji picker grid.
    pub label: &'static str,
    /// Picker tab this entry appears under — one of [`EMOJI_CATEGORIES`].
    pub category: &'static str,
}

/// The 6 emoji-picker category tabs, in display order (`firmware/src/ui/
/// screens/compose.rs`'s `EmojiPickerGrid` tab row reads this list). Every
/// [`EmojiEntry::category`] in [`EMOJI_TABLE`] must be one of these strings.
pub const EMOJI_CATEGORIES: &[&str] = &["Faces", "Gestures", "Hearts", "Nature", "Fun", "Objects"];

/// The canonical 96-entry curated emoji set (6 categories × 16 —
/// `meshcadet-emoji-picker-expansion`, campaign D1). The first 40 entries
/// are the original picker set (also members of `gen_emoji_font.c`'s
/// `EMOJI_CPS`); the 56 appended after them were grown out of the
/// render-only set (`RENDER_EXTRA_CPS`) at zero additional glyph-bitmap
/// cost — see the module doc's "picker/render split" section.
///
/// Broadly recognisable.  No violence, adult content, or ambiguous sentiment.
pub const EMOJI_TABLE: &[EmojiEntry] = &[
    // ── Faces (16) ───────────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "smile",
        codepoint: '😊',
        label: "Smile",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "laugh",
        codepoint: '😂',
        label: "Laugh",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "wink",
        codepoint: '😉',
        label: "Wink",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "cool",
        codepoint: '😎',
        label: "Cool",
        category: "Faces",
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
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "wow",
        codepoint: '😲',
        label: "Wow",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "sleepy",
        codepoint: '😴',
        label: "Sleepy",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "silly",
        codepoint: '😜',
        label: "Silly",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "happy",
        codepoint: '😁',
        label: "Happy",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "sad",
        codepoint: '😢',
        label: "Sad",
        category: "Faces",
    },
    // The 6 below are new (meshcadet-emoji-picker-expansion), drawn from
    // `RENDER_EXTRA_CPS`'s "Faces & emotions" section.
    EmojiEntry {
        shortcode: "loveeyes",
        codepoint: '😍',
        label: "Love Eyes",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "party",
        codepoint: '🥳',
        label: "Party",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "cry",
        codepoint: '😭',
        label: "Crying",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "crazy",
        codepoint: '🤪',
        label: "Crazy",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "please",
        codepoint: '🥺',
        label: "Puppy Eyes",
        category: "Faces",
    },
    EmojiEntry {
        shortcode: "angel",
        codepoint: '😇',
        label: "Angel",
        category: "Faces",
    },
    // ── Gestures (16) ────────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "wave",
        codepoint: '👋',
        label: "Wave",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "thumbsup",
        codepoint: '👍',
        label: "Thumbs Up",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "clap",
        codepoint: '👏',
        label: "Clap",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "highfive",
        codepoint: '🙏',
        label: "High Five",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "fist",
        codepoint: '✊',
        label: "Fist Bump",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "point",
        codepoint: '👆',
        label: "Point Up",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "ok",
        codepoint: '👌',
        label: "OK",
        category: "Gestures",
    },
    // The 9 below are new (meshcadet-emoji-picker-expansion), drawn from
    // `RENDER_EXTRA_CPS`'s "People & Body / hand-*" sections.
    EmojiEntry {
        shortcode: "thumbsdown",
        codepoint: '👎',
        label: "Thumbs Down",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "muscle",
        codepoint: '💪',
        label: "Muscle",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "handshake",
        codepoint: '🤝',
        label: "Handshake",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "crossedfingers",
        codepoint: '🤞',
        label: "Fingers Crossed",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "callme",
        codepoint: '🤙',
        label: "Call Me",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "rockon",
        codepoint: '🤘',
        label: "Rock On",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "yay",
        codepoint: '🙌',
        label: "Yay",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "selfie",
        codepoint: '🤳',
        label: "Selfie",
        category: "Gestures",
    },
    EmojiEntry {
        shortcode: "pointright",
        codepoint: '👉',
        label: "Point Right",
        category: "Gestures",
    },
    // ── Hearts (16) ──────────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "heart",
        codepoint: '❤',
        label: "Heart",
        category: "Hearts",
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
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "sparkles",
        codepoint: '✨',
        label: "Sparkles",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "star",
        codepoint: '⭐',
        label: "Star",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "rainbow",
        codepoint: '🌈',
        label: "Rainbow",
        category: "Hearts",
    },
    // The 11 below are new (meshcadet-emoji-picker-expansion), drawn from
    // `RENDER_EXTRA_CPS`'s "Hearts" / "Smileys & Emotion / heart" sections.
    EmojiEntry {
        shortcode: "yellowheart",
        codepoint: '💛',
        label: "Yellow Heart",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "greenheart",
        codepoint: '💚',
        label: "Green Heart",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "blueheart",
        codepoint: '💙',
        label: "Blue Heart",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "purpleheart",
        codepoint: '💜',
        label: "Purple Heart",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "orangeheart",
        codepoint: '🧡',
        label: "Orange Heart",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "twohearts",
        codepoint: '💕',
        label: "Two Hearts",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "glowheart",
        codepoint: '💖',
        label: "Glowing Heart",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "brokenheart",
        codepoint: '💔',
        label: "Broken Heart",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "kiss",
        codepoint: '💋',
        label: "Kiss",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "hundred",
        codepoint: '💯',
        label: "100",
        category: "Hearts",
    },
    EmojiEntry {
        shortcode: "loveletter",
        codepoint: '💌',
        label: "Love Letter",
        category: "Hearts",
    },
    // ── Nature (16) ──────────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "sun",
        codepoint: '☀',
        label: "Sun",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "moon",
        codepoint: '🌙',
        label: "Moon",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "cloud",
        codepoint: '⛅',
        label: "Cloud",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "flower",
        codepoint: '🌸',
        label: "Flower",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "tree",
        codepoint: '🌲',
        label: "Tree",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "leaf",
        codepoint: '🍃',
        label: "Leaf",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "dog",
        codepoint: '🐶',
        label: "Dog",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "cat",
        codepoint: '🐱',
        label: "Cat",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "rabbit",
        codepoint: '🐰',
        label: "Rabbit",
        category: "Nature",
    },
    // The 7 below are new (meshcadet-emoji-picker-expansion), drawn from
    // `RENDER_EXTRA_CPS`'s "Animals" / "Animals & Nature" sections.
    EmojiEntry {
        shortcode: "panda",
        codepoint: '🐼',
        label: "Panda",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "fox",
        codepoint: '🦊',
        label: "Fox",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "lion",
        codepoint: '🦁',
        label: "Lion",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "koala",
        codepoint: '🐨',
        label: "Koala",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "unicorn",
        codepoint: '🦄',
        label: "Unicorn",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "butterfly",
        codepoint: '🦋',
        label: "Butterfly",
        category: "Nature",
    },
    EmojiEntry {
        shortcode: "clover",
        codepoint: '🍀',
        label: "Clover",
        category: "Nature",
    },
    // ── Fun (16) ─────────────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "music",
        codepoint: '🎵',
        label: "Music",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "game",
        codepoint: '🎮',
        label: "Game",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "ball",
        codepoint: '⚽',
        label: "Ball",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "cake",
        codepoint: '🎂',
        label: "Cake",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "pizza",
        codepoint: '🍕',
        label: "Pizza",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "rocket",
        codepoint: '🚀',
        label: "Rocket",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "fire",
        codepoint: '🔥',
        label: "Fire",
        category: "Fun",
    },
    // The 9 below are new (meshcadet-emoji-picker-expansion), drawn from
    // `RENDER_EXTRA_CPS`'s "Food & drink" / "Activities" sections.
    EmojiEntry {
        shortcode: "apple",
        codepoint: '🍎',
        label: "Apple",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "banana",
        codepoint: '🍌',
        label: "Banana",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "cookie",
        codepoint: '🍪',
        label: "Cookie",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "donut",
        codepoint: '🍩',
        label: "Donut",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "trophy",
        codepoint: '🏆',
        label: "Trophy",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "basketball",
        codepoint: '🏀',
        label: "Basketball",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "bowling",
        codepoint: '🎳',
        label: "Bowling",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "balloon",
        codepoint: '🎈',
        label: "Balloon",
        category: "Fun",
    },
    EmojiEntry {
        shortcode: "gift",
        codepoint: '🎁',
        label: "Gift",
        category: "Fun",
    },
    // ── Objects (16) ─────────────────────────────────────────────────────────
    EmojiEntry {
        shortcode: "radio",
        codepoint: '📻',
        label: "Radio",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "check",
        codepoint: '✅',
        label: "Done",
        category: "Objects",
    },
    // The 14 below are new (meshcadet-emoji-picker-expansion), drawn from
    // `RENDER_EXTRA_CPS`'s "Smileys & Emotion / face-costume", "emotion",
    // "Activities", and "Travel & Places / place-map" sections.
    EmojiEntry {
        shortcode: "ghost",
        codepoint: '👻',
        label: "Ghost",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "robot",
        codepoint: '🤖',
        label: "Robot",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "alien",
        codepoint: '👽',
        label: "Alien",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "poop",
        codepoint: '💩',
        label: "Poop",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "thoughtballoon",
        codepoint: '💭',
        label: "Thought Bubble",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "speechballoon",
        codepoint: '💬',
        label: "Speech Bubble",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "compass",
        codepoint: '🧭',
        label: "Compass",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "globe",
        codepoint: '🌍',
        label: "Globe",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "crystalball",
        codepoint: '🔮',
        label: "Crystal Ball",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "puzzle",
        codepoint: '🧩',
        label: "Puzzle",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "teddybear",
        codepoint: '🧸',
        label: "Teddy Bear",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "paintpalette",
        codepoint: '🎨',
        label: "Palette",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "fireworks",
        codepoint: '🎆',
        label: "Fireworks",
        category: "Objects",
    },
    EmojiEntry {
        shortcode: "umbrella",
        codepoint: '☔',
        label: "Umbrella",
        category: "Objects",
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

/// Variation Selector-16 — forces the *emoji* presentation of the preceding
/// base character (as opposed to VS15, `U+FE0E`, which forces text
/// presentation). Android routinely sends this after single-codepoint emoji
/// that also have a text glyph, e.g. `U+2764 U+FE0F` for ❤️. Not a glyph in
/// its own right — see module doc.
const VARIATION_SELECTOR_16: char = '\u{FE0F}';

/// Zero Width Joiner — glues adjacent emoji into one logical pictograph
/// (family/couple/profession sequences). Not a glyph in its own right — see
/// module doc.
const ZERO_WIDTH_JOINER: char = '\u{200D}';

/// `true` for the five Fitzpatrick skin-tone modifiers `U+1F3FB..=U+1F3FF`.
/// Not a glyph in its own right — see module doc.
fn is_skin_tone_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// Normalize inbound (received) text for display: drop VS16, drop skin-tone
/// modifiers, and collapse ZWJ sequences to their lead scalar. See the
/// module doc's "Inbound normalization" section for why this exists and why
/// it runs on the RECEIVE/RENDER side only.
///
/// Returns the number of bytes written to `out`, or `None` if `input` is not
/// valid UTF-8 or `out` is too small to hold the result (same contract shape
/// as [`expand_shortcodes`] and [`crate::mention::wrap_mentions`]).
///
/// Text that carries none of the three combining characters above (the
/// common case — plain ASCII, or a bare single-codepoint emoji) passes
/// through unchanged.
///
/// # Example
/// ```
/// # use protocol::emoji::normalize_inbound;
/// // Android's ❤️ is U+2764 U+FE0F — normalizes to a single U+2764.
/// let mut out = [0u8; 16];
/// let n = normalize_inbound("\u{2764}\u{FE0F}".as_bytes(), &mut out).unwrap();
/// assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "\u{2764}");
/// ```
pub fn normalize_inbound(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let text = core::str::from_utf8(input).ok()?;
    let mut w = 0usize;
    // Set after a ZWJ is dropped: the NEXT scalar that would otherwise be
    // emitted is the glyph the ZWJ joined onto the run so far, so it is
    // dropped too — collapsing the whole joined sequence to its lead
    // scalar. A run of `G1 ZWJ G2 ZWJ G3` (e.g. 👨‍👩‍👧) drops both ZWJs
    // AND both G2/G3, since each ZWJ re-arms this flag before the next
    // glyph is reached.
    let mut drop_next_glyph = false;
    for c in text.chars() {
        if c == VARIATION_SELECTOR_16 || is_skin_tone_modifier(c) {
            continue; // never occupies a cell of its own; always dropped
        }
        if c == ZERO_WIDTH_JOINER {
            drop_next_glyph = true;
            continue;
        }
        if drop_next_glyph {
            drop_next_glyph = false;
            continue;
        }
        let mut cp_buf = [0u8; 4];
        let encoded = c.encode_utf8(&mut cp_buf);
        let encoded_bytes = encoded.as_bytes();
        if w + encoded_bytes.len() > out.len() {
            return None; // output buffer exhausted
        }
        out[w..w + encoded_bytes.len()].copy_from_slice(encoded_bytes);
        w += encoded_bytes.len();
    }
    Some(w)
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
    fn table_has_96_entries() {
        assert_eq!(EMOJI_TABLE.len(), 96);
    }

    /// D1's "6 categories × 16" shape — pins the exact per-tab count the
    /// firmware picker relies on (`meshcadet-emoji-picker-expansion`'s
    /// acceptance: "the emoji picker offers ~96 entries with category
    /// tabs").
    #[test]
    fn every_category_has_sixteen_entries() {
        for &category in EMOJI_CATEGORIES {
            let count = EMOJI_TABLE
                .iter()
                .filter(|e| e.category == category)
                .count();
            assert_eq!(
                count, 16,
                "category {category:?} has {count} entries, want 16"
            );
        }
    }

    /// Every entry's `category` must be one of the 6 known tabs — a typo'd
    /// category string would silently drop that entry from every picker tab
    /// (never rendered, never reachable), with no build-time signal.
    #[test]
    fn every_entry_category_is_a_known_category() {
        for entry in EMOJI_TABLE {
            assert!(
                EMOJI_CATEGORIES.contains(&entry.category),
                "entry {:?} has unknown category {:?}",
                entry.shortcode,
                entry.category
            );
        }
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

    /// `meshcadet-emoji-picker-expansion` acceptance: "shortcode_completions
    /// still returns sane results at the larger N" — pins a prefix that now
    /// has MULTIPLE matches only because of the picker's growth from 40 to
    /// 96 entries ("think" existed before; "thumbsup"/"thumbsdown"/
    /// "thoughtballoon" are new), through the compose screen's real
    /// 5-slot autocomplete buffer (`ComposeScreen::refresh_completions`'s
    /// `[""; 5]`) — sane means: no duplicates, every result actually
    /// matches the prefix, and the buffer isn't overrun.
    #[test]
    fn shortcode_completions_multiple_matches_at_n96() {
        let mut found = [""; 5];
        let n = shortcode_completions("th", &mut found);
        assert_eq!(
            n,
            4,
            "expected exactly 4 \"th\"-prefixed shortcodes at N=96: think, thumbsup, \
             thumbsdown, thoughtballoon — got {:?}",
            &found[..n]
        );
        for &sc in &found[..n] {
            assert!(
                sc.starts_with("th"),
                "completion {sc:?} doesn't start with \"th\""
            );
        }
        // No duplicates.
        for i in 0..n {
            for j in (i + 1)..n {
                assert_ne!(found[i], found[j], "duplicate completion {:?}", found[i]);
            }
        }
    }

    /// The compose-time cost claim (module doc: "linear scan, O(N), N=96,
    /// fast enough for interactive compose") as a MEASUREMENT, not just an
    /// assertion — `meshcadet-emoji-picker-expansion`'s acceptance calls
    /// this out explicitly ("say so with a measurement rather than an
    /// assertion") because the table just grew 2.4x (40 -> 96). Runs the
    /// same scan `refresh_completions` (`firmware/src/ui/screens/
    /// compose.rs`) drives on every physical keystroke while the
    /// `:shortcode:` autocomplete bar is open, at the now-current N=96,
    /// with a generous threshold: interactive UI latency budgets are
    /// usually drawn around ~16ms (one frame); this asserts three orders of
    /// magnitude under that, so a debug-profile host run (slower than the
    /// firmware's release/opt-level=\"z\" xtensa build) still passes with
    /// wide margin. `std::hint::black_box` prevents the optimizer from
    /// const-folding the loop away.
    #[test]
    fn shortcode_completions_scan_stays_fast_at_n96() {
        use std::hint::black_box;
        use std::time::Instant;

        assert_eq!(EMOJI_TABLE.len(), 96, "recalibrate this test if N changes");

        // Warm up (branch predictor, cache lines) before the timed pass.
        for _ in 0..1_000 {
            let mut found = [""; 5];
            black_box(shortcode_completions(black_box("th"), &mut found));
        }

        let iters = 100_000u32;
        let start = Instant::now();
        for _ in 0..iters {
            let mut found = [""; 5];
            black_box(shortcode_completions(black_box("th"), &mut found));
        }
        let elapsed = start.elapsed();
        let ns_per_scan = elapsed.as_nanos() as f64 / f64::from(iters);

        // 100 microseconds per single 96-entry linear scan is an extremely
        // generous ceiling (three orders of magnitude above the low-hundreds-
        // of-nanoseconds this actually measures on a dev host) — the point
        // is to fail loudly if this ever regresses to something
        // algorithmically worse (e.g. an accidental O(N^2)), not to pin a
        // tight bound that would make this test flaky on a loaded CI runner.
        assert!(
            ns_per_scan < 100_000.0,
            "shortcode_completions over N={} took {ns_per_scan:.1}ns/scan — expected well under \
             100,000ns (100us) for a linear scan this small",
            EMOJI_TABLE.len(),
        );
    }

    #[test]
    fn shortcode_completions_no_match() {
        let mut found = [""; 10];
        let n = shortcode_completions("zzz", &mut found);
        assert_eq!(n, 0);
    }

    // ── normalize_inbound ────────────────────────────────────────────────

    #[test]
    fn normalize_inbound_drops_vs16_on_heart() {
        // Android's ❤️ is U+2764 U+FE0F — the exact live defect this
        // mission fixes (renders as heart + blank cell otherwise).
        let mut out = [0u8; 16];
        let n = normalize_inbound("\u{2764}\u{FE0F}".as_bytes(), &mut out).unwrap();
        let s = core::str::from_utf8(&out[..n]).unwrap();
        assert_eq!(s, "\u{2764}");
        assert_eq!(s.chars().count(), 1);
    }

    #[test]
    fn normalize_inbound_drops_skin_tone_modifier() {
        // 👍🏽 = U+1F44D (thumbs up) U+1F3FD (medium skin tone) → 👍.
        let mut out = [0u8; 16];
        let n = normalize_inbound("\u{1F44D}\u{1F3FD}".as_bytes(), &mut out).unwrap();
        let s = core::str::from_utf8(&out[..n]).unwrap();
        assert_eq!(s, "\u{1F44D}");
    }

    #[test]
    fn normalize_inbound_collapses_zwj_family_to_lead_glyph() {
        // 👨‍👩‍👧 = man ZWJ woman ZWJ girl → man alone (the deliberate
        // degradation this mission's Notes document — EmojiEntry::codepoint
        // is a single `char`, not a grapheme cluster).
        let mut out = [0u8; 32];
        let n = normalize_inbound(
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}".as_bytes(),
            &mut out,
        )
        .unwrap();
        let s = core::str::from_utf8(&out[..n]).unwrap();
        assert_eq!(s, "\u{1F468}");
    }

    #[test]
    fn normalize_inbound_bare_emoji_passes_through_unchanged() {
        let mut out = [0u8; 16];
        let n = normalize_inbound("\u{1F600}".as_bytes(), &mut out).unwrap();
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "\u{1F600}");
    }

    #[test]
    fn normalize_inbound_plain_ascii_passes_through_unchanged() {
        let mut out = [0u8; 64];
        let input = b"hello world, no emoji here!";
        let n = normalize_inbound(input, &mut out).unwrap();
        assert_eq!(&out[..n], input);
    }

    #[test]
    fn normalize_inbound_mixed_text_and_decorated_emoji() {
        // A realistic mixed message: plain text, a VS16-decorated emoji,
        // more text.
        let mut out = [0u8; 64];
        let n = normalize_inbound("nice catch \u{2764}\u{FE0F} !".as_bytes(), &mut out).unwrap();
        let s = core::str::from_utf8(&out[..n]).unwrap();
        assert_eq!(s, "nice catch \u{2764} !");
    }

    #[test]
    fn normalize_inbound_none_on_output_overflow() {
        let mut out = [0u8; 1];
        assert!(normalize_inbound("\u{2764}\u{FE0F}".as_bytes(), &mut out).is_none());
    }

    #[test]
    fn normalize_inbound_rejects_invalid_utf8_input() {
        let mut out = [0u8; 16];
        let invalid = [0x80u8, 0x41u8]; // lone continuation byte, never valid UTF-8
        assert!(normalize_inbound(&invalid, &mut out).is_none());
    }

    // ── normalize_inbound — adversarial / untrusted-input safety ──────────
    //
    // `normalize_inbound` runs on wire text from remote mesh peers —
    // untrusted input this node did not compose. Post-green review
    // criterion #6 (security & input validation) / #8 (test depth): these
    // pin "never panics, degrades sensibly" for malformed combining-
    // character sequences a hostile or buggy peer could send, matching
    // `crate::mention`'s own "Adversarial / untrusted-input safety" test
    // section for the identical threat model.

    #[test]
    fn normalize_inbound_leading_zwj_with_no_preceding_glyph_drops_the_next_one_too() {
        // A ZWJ with nothing before it to join is malformed, but the flag
        // it sets is unconditional: the very next scalar is still treated
        // as "the thing this ZWJ joins" and dropped. Degrades to empty
        // output, not a panic.
        let mut out = [0u8; 16];
        let n = normalize_inbound("\u{200D}\u{1F600}".as_bytes(), &mut out).unwrap();
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "");
    }

    #[test]
    fn normalize_inbound_trailing_zwj_with_nothing_after_it_does_not_panic() {
        let mut out = [0u8; 16];
        let n = normalize_inbound("\u{1F600}\u{200D}".as_bytes(), &mut out).unwrap();
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "\u{1F600}");
    }

    #[test]
    fn normalize_inbound_consecutive_zwj_still_drops_only_one_following_glyph_set() {
        // G1 ZWJ ZWJ G2 — the second ZWJ re-arms the same flag rather than
        // stacking two drops; only ONE following glyph (G2) is consumed.
        let mut out = [0u8; 32];
        let n =
            normalize_inbound("\u{1F468}\u{200D}\u{200D}\u{1F469}".as_bytes(), &mut out).unwrap();
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "\u{1F468}");
    }

    #[test]
    fn normalize_inbound_vs16_between_zwj_and_next_glyph_does_not_clear_the_drop_flag() {
        // Malformed shape: G1 ZWJ VS16 G2. VS16 is checked (and dropped)
        // BEFORE the "was the previous char a ZWJ" check, so it does NOT
        // count as "the glyph the ZWJ joined" — the drop flag survives
        // past it and still consumes G2. Pins the deliberate branch
        // ordering in `normalize_inbound`, not an accident of it.
        let mut out = [0u8; 32];
        let n =
            normalize_inbound("\u{1F468}\u{200D}\u{FE0F}\u{1F469}".as_bytes(), &mut out).unwrap();
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "\u{1F468}");
    }

    #[test]
    fn normalize_inbound_stray_leading_vs16_with_no_preceding_base_is_dropped_harmlessly() {
        let mut out = [0u8; 16];
        let n = normalize_inbound("\u{FE0F}\u{1F600}".as_bytes(), &mut out).unwrap();
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "\u{1F600}");
    }

    #[test]
    fn normalize_inbound_all_combining_characters_degrades_to_empty_output() {
        // VS16 + ZWJ + skin-tone modifier, no base glyph anywhere — must
        // degrade to an empty string, never panic or underflow.
        let mut out = [0u8; 16];
        let n = normalize_inbound("\u{FE0F}\u{200D}\u{1F3FD}".as_bytes(), &mut out).unwrap();
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "");
    }
}
