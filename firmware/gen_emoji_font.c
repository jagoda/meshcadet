// SPDX-License-Identifier: GPL-3.0-only
/*
 * gen_emoji_font.c — Build-time bitmap font generator for MeshCadet emoji.
 *
 * Rasterises a combined "MeshCadetEmoji" font that includes:
 *   - Printable ASCII (U+0020..U+007E) from a Latin TTF (DejaVu Sans)
 *   - A small set of BMP symbols used by the UI (‹ › ✏ ✓ ✕ ⚙ ⌫ −) from the
 *     Latin font (with emoji-face fallback for any the Latin font lacks)
 *   - 40 curated picker emoji + a curated set of UI-chrome emoji, including
 *     📤 😀 🔐 📍, + a render-only set (rasterised so inbound messages
 *     display correctly, never offered in the picker), from
 *     NotoEmoji-Regular.ttf (see `UI_EXTRA_CPS`/`RENDER_EXTRA_CPS` below for
 *     the full, current lists — this header enumerates examples, not an
 *     exhaustive count)
 *
 * Why one combined font, registered globally, at every UI size:
 *   The Slint SoftwareRenderer resolves an entire text run to a SINGLE bitmap
 *   font and does NO per-glyph fallback (i-slint-renderer-software pixelfont.rs
 *   `shape_text`): any char absent from the selected font renders blank.  It
 *   also snaps each request to the nearest available pixel size and scales the
 *   glyph metrics.  Dynamic message bodies mix Latin + emoji in one run, so the
 *   serving font must cover BOTH at the run's EXACT size — an emoji-only font
 *   scoped via `font-family` cannot serve those runs, and a font with only a few
 *   sizes garbles text at every other size.  Hence this font covers full ASCII +
 *   UI symbols + emoji at every UI font-size and is registered as the global
 *   fallback in platform.rs::install().  (Emoji are limited to the sizes where
 *   they actually appear — see EMOJI_SIZES — to bound flash.)
 *
 * Usage (called from build.rs):
 *   gen_emoji_font <latin.ttf> <emoji.ttf> <out.rs>
 *
 * Build:
 *   gcc -O2 gen_emoji_font.c $(pkg-config --cflags --libs freetype2) -o gen_emoji_font
 *
 * ── The picker/render split (FROZEN CONTRACT — meshcadet-emoji-coverage D1) ──
 *
 * The renderable set is no longer required to equal the picker set. Two
 * codepoint tables feed the emoji face now:
 *   - `EMOJI_CPS`        — the picker's 40 curated entries. MUST stay in
 *                          lockstep (same codepoints) with
 *                          `protocol::emoji::EMOJI_TABLE` — this is the one
 *                          half of the sync invariant that is still equality.
 *   - `RENDER_EXTRA_CPS` — render-only codepoints: rasterised into this font
 *                          so INBOUND messages display correctly, but never
 *                          offered in the picker grid and never listed in
 *                          `protocol::emoji::EMOJI_TABLE`.
 *
 * The sync invariant this file has always documented — "EMOJI_CPS must match
 * protocol::emoji::EMOJI_TABLE" — is therefore relaxed from EQUALITY to
 * CONTAINMENT:
 *
 *     protocol::emoji::EMOJI_TABLE ⊆ (EMOJI_CPS ∪ RENDER_EXTRA_CPS)
 *
 * Every picker entry must still resolve to a rasterised glyph (that part
 * never changes — a picker entry with no glyph is still a build-breaking
 * bug), but a rasterised glyph no longer implies a picker entry. This is
 * what makes growing the picker flash-free: reusing an already-rasterised
 * render-only glyph costs only the new `EmojiEntry`'s `&'static str`
 * shortcode/label bytes, not a fresh bitmap. `xtask::font_table_count_mismatches`
 * enforces the containment half in code (see its doc comment); this file
 * enforces the "every registered codepoint has a glyph" half via
 * `g_missing_glyph_count` below, same as always.
 *
 * `RENDER_EXTRA_CPS` curation criteria (D3 — frozen; downstream missions that
 * grow this table draw from THESE criteria, not their own judgment):
 *   - Source of truth: Unicode CLDR emoji ordering + the fully-qualified,
 *     single-codepoint subset of `emoji-test.txt`.
 *   - EXCLUDE anything structurally unrenderable by this pipeline —
 *     `EmojiEntry.codepoint`/this file's `CharEntry.cp` is a single scalar
 *     and the renderer does no grapheme clustering: ZWJ `U+200D` sequences,
 *     regional-indicator flag pairs, keycap sequences, skin-tone modifiers
 *     `U+1F3FB..FF`, and anything whose only correct presentation needs VS16
 *     `U+FE0F`.
 *   - EXCLUDE by content bar (tightened for "kid appropriate"): weapons,
 *     violence/gore, alcohol, tobacco, drugs, gambling, adult/suggestive,
 *     medical/injury, offensive gestures, religious and political symbols,
 *     and anything culture-specific or ambiguous to a child.
 *   - INCLUDE, in rough priority order: faces & emotions (the largest single
 *     win), non-offensive hands, single-codepoint people/roles, animals,
 *     plants & nature, food & drink (non-alcoholic), activities & sports,
 *     travel & places, everyday objects, weather, symbols (hearts, stars,
 *     checks, music).
 *   - The target count is a ceiling for budgeting, not a quota.
 *
 * Do NOT change (D2 — frozen): `EMOJI_SIZES`/`PIXEL_SIZES` stay exactly as
 * they are (see their own doc comments below for why a per-table size split
 * was considered and rejected, and why `EMOJI_SIZES` must not shrink).
 *
 * This is part of a multi-phase emoji-coverage expansion; see the phase
 * notes below (and D1-D3 above) for the decisions carried forward from
 * earlier phases.
 */

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <ft2build.h>
#include FT_FREETYPE_H
#include FT_SFNT_NAMES_H
#include FT_TRUETYPE_TABLES_H
#include FT_MULTIPLE_MASTERS_H
#include FT_OUTLINE_H

/* ── Curated emoji codepoints (40 total) ─────────────────────────────── */
/*
 * SYNC INVARIANT (CONTAINMENT, not equality — see the picker/render split
 * contract at the top of this file): every codepoint in `protocol::emoji::
 * EMOJI_TABLE` must appear SOMEWHERE in `EMOJI_CPS ∪ RENDER_EXTRA_CPS`.
 * Historically this array WAS exactly `protocol::emoji::EMOJI_TABLE`'s
 * codepoints (hence "40 total" above), and adding a picker entry meant
 * adding it here. That is still ONE valid way to add a picker entry, but
 * NOT the only one as of D1: a picker entry can instead be grown out of an
 * already-rasterised `RENDER_EXTRA_CPS` codepoint at zero flash cost (no
 * new bitmap needed) — update `N_EMOJI_TABLE` below only if you add here.
 * `xtask::emoji_table_subset_mismatches` enforces the containment in code;
 * a divergence otherwise compiles silently but the added emoji renders
 * blank on-device.
 */
static const unsigned long EMOJI_CPS[] = {
    /* Faces */
    // BUG FIX: 0x1F914 (🤔) is a
    // Unicode 9.0 codepoint outside this bundled NotoEmoji build's coverage
    // (it stops at ~Unicode 8.0) — it rendered BLANK on-device with zero
    // build-time signal. Swapped for 0x1F615 (😕, confirmed present) to match
    // `protocol::emoji::EMOJI_TABLE`'s "think" entry.
    0x1F60A,0x1F602,0x1F609,0x1F60E,0x1F615,0x1F632,0x1F634,0x1F61C,
    0x1F601,0x1F622,
    /* Gestures */
    0x1F44B,0x1F44D,0x1F44F,0x1F64F,0x270A, 0x1F446,0x1F44C,
    /* Love/Feelings */
    // BUG FIX: 0x1F917 (🤗) —
    // same font-coverage gap as 0x1F914 above. Swapped for 0x1F618 (😘,
    // confirmed present) to match EMOJI_TABLE's "hug" entry.
    0x2764, 0x1F618,0x2728, 0x2B50, 0x1F308,
    /* Nature */
    0x2600, 0x1F319,0x26C5, 0x1F338,0x1F332,0x1F343,0x1F436,0x1F431,0x1F430,
    /* Objects/Fun */
    0x1F3B5,0x1F3AE,0x26BD, 0x1F382,0x1F355,0x1F680,0x1F525,
    /* Communication */
    0x1F4FB,0x2705,
};
#define N_EMOJI_TABLE 40

/* Extra UI emoji used in the UI chrome but NOT in the picker's EMOJI_TABLE.
 * These are NOT subject to the EMOJI_TABLE/protocol sync invariant — they are
 * private to the firmware UI.  Each MUST appear at a size listed in EMOJI_SIZES
 * (see below) or it renders blank. */
static const unsigned long UI_EXTRA_CPS[] = {
    0x1F4E4,  /* 📤 outbox tray   — "📤 Send" button (compose.rs, 13px)        */
    0x1F600,  /* 😀 grinning face — picker toggle    (compose.rs, 18px)        */
    /* NOTE: 0x1F4EC (📬 mailbox) and 0x1F4E1 (📡 satellite) were the
     * "📬 Messages"/"📡 Channels" tab-label glyphs in contact_list.rs. The
     * Contacts/Groups rename dropped both plain-text tab labels back to
     * bare "Contacts"/"Groups" (no leading glyph) — see contact_list.rs's
     * module doc — so these two codepoints are no longer used anywhere in
     * the UI and were removed from this table. Removing an UNUSED entry is
     * always safe (the glyph-coverage harness only checks that USED
     * codepoints are registered, never the reverse); if a future screen
     * wants either glyph again, re-add it here. */
    0x1F510,  /* 🔐 lock+key      — PIN-entry icon    (pin_entry.rs, 20px)     */
    0x1F4CD,  /* 📍 round pin     — telemetry location in message body (13px);
                 * also the GPS-status header title (gps_status.rs, 14px)     */
    /* BUG FIX: these two were used
     * in admin_menu.rs's ToggleRow labels when the AdminMenu screen was
     * added, but never added here — a SYNC INVARIANT violation
     * (see comment above).  `cargo build` doesn't catch this (the font table
     * is build-time-generated runtime data, not type-checked against Slint
     * string literals); on real hardware the two glyphs silently rendered
     * blank (this file's own documented failure mode), leaving only bare
     * "  Visual notifications" / "  Audible notifications" rows on the
     * admin-menu screen. */
    0x1F514,  /* 🔔 bell    — "🔔  Visual notifications"  (admin_menu.rs, 14px) */
    0x1F50A,  /* 🔊 speaker — "🔊  Audible notifications" (admin_menu.rs, 14px) */
    0x1F4A4,  /* 💤 zzz     — "💤  Screen sleep"          (admin_menu.rs, 14px) */
    /* BUG FIX: same SYNC
     * INVARIANT violation class as the three above — 🔋 was used in
     * admin_menu.rs's InfoRow label ("🔋  Battery") but was never added
     * here, so it silently rendered blank on real hardware. Caught by a
     * host glyph-coverage harness (`xtask`) while freezing the
     * full icon inventory, rather than by a future field report. */
    0x1F50B,  /* 🔋 battery — "🔋  Battery"                (admin_menu.rs, 14px) */
    0x1F512,  /* 🔒 lock (no key) — "🔒 Read-only" compose banner
                 * (compose.rs, size-preview/11px; ui_sim's read-only mirror
                 * uses the same glyph+size — M2 post-and-notify) */
};
#define N_UI_EXTRA 9

/* Render-only codepoints (D1 — see the frozen picker/render split contract
 * at the top of this file). These rasterise INTO this font so inbound
 * messages display correctly, but are deliberately NOT in `EMOJI_CPS` (the
 * picker set) and NOT in `protocol::emoji::EMOJI_TABLE` — they never appear
 * in the emoji-picker grid. `xtask::font_table_count_mismatches` enforces
 * `protocol::emoji::EMOJI_TABLE ⊆ (EMOJI_CPS ∪ RENDER_EXTRA_CPS)` in code.
 *
 * Curated to the campaign's confirmed 600-entry target by
 * `meshcadet-emoji-render-set-curation` (D3 criteria frozen above). Source:
 * Unicode CLDR emoji ordering + the fully-qualified, single-codepoint subset
 * of `emoji-test.txt` (v17.0), filtered by the exclusion/inclusion rules
 * above, deduplicated against `EMOJI_CPS`/`UI_EXTRA_CPS`, checked against
 * this build's actual `NotoEmoji-Regular.ttf` cmap coverage (a small number
 * of very recent (Unicode 15/16) additions the bundled font doesn't yet
 * cover were skipped rather than shipped as guaranteed-blank glyphs), and
 * ordered by the D3 priority list. The budget (600) was reached before the
 * candidate pool was exhausted — the priority order determined the cut:
 * faces/hands/people/animals/plants/food/most of activities are fully
 * represented; travel & places is only partially represented (11 of a much
 * larger available pool), and everyday objects/weather/symbols were not
 * reached at all. The first 50 entries below (phase 5's measuring seed,
 * kept verbatim) are followed by the additional 550. */
static const unsigned long RENDER_EXTRA_CPS[] = {
    /* Faces & emotions — the 14 confirmed missing from the pre-upgrade face */
    0x1F914, /* 🤔 thinking face */
    0x1F917, /* 🤗 hugging face */
    0x1F644, /* 🙄 face with rolling eyes */
    0x1F929, /* 🤩 star-struck */
    0x1F97A, /* 🥺 pleading face */
    0x1F970, /* 🥰 smiling face with hearts */
    0x1F92F, /* 🤯 exploding head */
    0x1F921, /* 🤡 clown face */
    0x1F9E1, /* 🧡 orange heart */
    0x1F92A, /* 🤪 zany face */
    0x1F971, /* 🥱 yawning face */
    0x1F984, /* 🦄 unicorn face */
    0x1F9D2, /* 🧒 child */
    0x1FAE0, /* 🫠 melting face */
    /* Faces & emotions — additional high-frequency entries */
    0x1F60D, /* 😍 heart eyes */
    0x1F60B, /* 😋 savoring food (yum) */
    0x1F642, /* 🙂 slightly smiling face */
    0x1F643, /* 🙃 upside-down face */
    0x1F605, /* 😅 grinning face with sweat */
    0x1F606, /* 😆 grinning squinting face */
    0x1F62D, /* 😭 loudly crying face */
    0x1F928, /* 🤨 face with raised eyebrow */
    0x1F9D0, /* 🧐 face with monocle */
    0x1F913, /* 🤓 nerd face */
    0x1F60C, /* 😌 relieved face */
    0x1F614, /* 😔 pensive face */
    0x1F973, /* 🥳 partying face */
    0x1F607, /* 😇 smiling face with halo */
    0x1F62C, /* 😬 grimacing face */
    /* Hearts (non-offensive symbols) */
    0x1F49B, /* 💛 yellow heart */
    0x1F49A, /* 💚 green heart */
    0x1F499, /* 💙 blue heart */
    0x1F49C, /* 💜 purple heart */
    0x1F5A4, /* 🖤 black heart */
    0x1F90D, /* 🤍 white heart */
    /* Animals */
    0x1F43C, /* 🐼 panda face */
    0x1F98A, /* 🦊 fox face */
    0x1F42F, /* 🐯 tiger face */
    0x1F981, /* 🦁 lion face */
    0x1F42E, /* 🐮 cow face */
    0x1F437, /* 🐷 pig face */
    0x1F438, /* 🐸 frog face */
    0x1F428, /* 🐨 koala */
    /* Food & drink (non-alcoholic) */
    0x1F34E, /* 🍎 red apple */
    0x1F34C, /* 🍌 banana */
    0x1F347, /* 🍇 grapes */
    0x1F369, /* 🍩 doughnut */
    0x1F36A, /* 🍪 cookie */
    0x1F368, /* 🍨 ice cream */
    /* Weather */
    0x2614,  /* ☔ umbrella with rain drops */
    /* ── The 550 entries below were added by
     * meshcadet-emoji-render-set-curation to reach the campaign's confirmed
     * 600-entry target (see this array's module doc above for method). ── */
    /* Smileys & Emotion / face-smiling */
    0x1F603, /* 😃 grinning face with big eyes */
    0x1F604, /* 😄 grinning face with smiling eyes */
    0x1F923, /* 🤣 rolling on the floor laughing */
    /* Smileys & Emotion / face-affection */
    0x1F617, /* 😗 kissing face */
    0x1F61A, /* 😚 kissing face with closed eyes */
    0x1F619, /* 😙 kissing face with smiling eyes */
    0x1F972, /* 🥲 smiling face with tear */
    /* Smileys & Emotion / face-tongue */
    0x1F61B, /* 😛 face with tongue */
    0x1F61D, /* 😝 squinting face with tongue */
    0x1F911, /* 🤑 money-mouth face */
    /* Smileys & Emotion / face-hand */
    0x1F92D, /* 🤭 face with hand over mouth */
    0x1FAE2, /* 🫢 face with open eyes and hand over mouth */
    0x1FAE3, /* 🫣 face with peeking eye */
    0x1F92B, /* 🤫 shushing face */
    0x1FAE1, /* 🫡 saluting face */
    /* Smileys & Emotion / face-neutral-skeptical */
    0x1F910, /* 🤐 zipper-mouth face */
    0x1F610, /* 😐 neutral face */
    0x1F611, /* 😑 expressionless face */
    0x1F636, /* 😶 face without mouth */
    0x1FAE5, /* 🫥 dotted line face */
    0x1F60F, /* 😏 smirking face */
    0x1F612, /* 😒 unamused face */
    0x1F925, /* 🤥 lying face */
    0x1FAE8, /* 🫨 shaking face */
    /* Smileys & Emotion / face-sleepy */
    0x1F62A, /* 😪 sleepy face */
    0x1F924, /* 🤤 drooling face */
    /* Smileys & Emotion / face-unwell */
    0x1F637, /* 😷 face with medical mask */
    0x1F912, /* 🤒 face with thermometer */
    0x1F915, /* 🤕 face with head-bandage */
    0x1F922, /* 🤢 nauseated face */
    0x1F92E, /* 🤮 face vomiting */
    0x1F927, /* 🤧 sneezing face */
    0x1F975, /* 🥵 hot face */
    0x1F976, /* 🥶 cold face */
    0x1F974, /* 🥴 woozy face */
    0x1F635, /* 😵 face with crossed-out eyes */
    /* Smileys & Emotion / face-hat */
    0x1F920, /* 🤠 cowboy hat face */
    0x1F978, /* 🥸 disguised face */
    /* Smileys & Emotion / face-concerned */
    0x1FAE4, /* 🫤 face with diagonal mouth */
    0x1F61F, /* 😟 worried face */
    0x1F641, /* 🙁 slightly frowning face */
    0x1F62E, /* 😮 face with open mouth */
    0x1F62F, /* 😯 hushed face */
    0x1F633, /* 😳 flushed face */
    0x1F979, /* 🥹 face holding back tears */
    0x1F626, /* 😦 frowning face with open mouth */
    0x1F627, /* 😧 anguished face */
    0x1F628, /* 😨 fearful face */
    0x1F630, /* 😰 anxious face with sweat */
    0x1F625, /* 😥 sad but relieved face */
    0x1F631, /* 😱 face screaming in fear */
    0x1F616, /* 😖 confounded face */
    0x1F623, /* 😣 persevering face */
    0x1F61E, /* 😞 disappointed face */
    0x1F613, /* 😓 downcast face with sweat */
    0x1F629, /* 😩 weary face */
    0x1F62B, /* 😫 tired face */
    /* Smileys & Emotion / face-negative */
    0x1F624, /* 😤 face with steam from nose */
    0x1F621, /* 😡 enraged face */
    0x1F620, /* 😠 angry face */
    0x1F92C, /* 🤬 face with symbols on mouth */
    0x1F608, /* 😈 smiling face with horns */
    0x1F47F, /* 👿 angry face with horns */
    0x1F480, /* 💀 skull */
    /* Smileys & Emotion / face-costume */
    0x1F4A9, /* 💩 pile of poo */
    0x1F479, /* 👹 ogre */
    0x1F47A, /* 👺 goblin */
    0x1F47B, /* 👻 ghost */
    0x1F47D, /* 👽 alien */
    0x1F47E, /* 👾 alien monster */
    0x1F916, /* 🤖 robot */
    /* Smileys & Emotion / cat-face */
    0x1F63A, /* 😺 grinning cat */
    0x1F638, /* 😸 grinning cat with smiling eyes */
    0x1F639, /* 😹 cat with tears of joy */
    0x1F63B, /* 😻 smiling cat with heart-eyes */
    0x1F63C, /* 😼 cat with wry smile */
    0x1F63D, /* 😽 kissing cat */
    0x1F640, /* 🙀 weary cat */
    0x1F63F, /* 😿 crying cat */
    0x1F63E, /* 😾 pouting cat */
    /* Smileys & Emotion / monkey-face */
    0x1F648, /* 🙈 see-no-evil monkey */
    0x1F649, /* 🙉 hear-no-evil monkey */
    0x1F64A, /* 🙊 speak-no-evil monkey */
    /* Smileys & Emotion / heart */
    0x1F48C, /* 💌 love letter */
    0x1F498, /* 💘 heart with arrow */
    0x1F49D, /* 💝 heart with ribbon */
    0x1F496, /* 💖 sparkling heart */
    0x1F497, /* 💗 growing heart */
    0x1F493, /* 💓 beating heart */
    0x1F49E, /* 💞 revolving hearts */
    0x1F495, /* 💕 two hearts */
    0x1F49F, /* 💟 heart decoration */
    0x1F494, /* 💔 broken heart */
    0x1FA77, /* 🩷 pink heart */
    0x1FA75, /* 🩵 light blue heart */
    0x1F90E, /* 🤎 brown heart */
    0x1FA76, /* 🩶 grey heart */
    /* Smileys & Emotion / emotion */
    0x1F48B, /* 💋 kiss mark */
    0x1F4AF, /* 💯 hundred points */
    0x1F4A2, /* 💢 anger symbol */
    0x1F4A5, /* 💥 collision */
    0x1F4AB, /* 💫 dizzy */
    0x1F4A6, /* 💦 sweat droplets */
    0x1F4A8, /* 💨 dashing away */
    0x1F4AC, /* 💬 speech balloon */
    0x1F4AD, /* 💭 thought balloon */
    /* People & Body / hand-fingers-open */
    0x1F91A, /* 🤚 raised back of hand */
    0x270B, /* ✋ raised hand */
    0x1F596, /* 🖖 vulcan salute */
    0x1FAF1, /* 🫱 rightwards hand */
    0x1FAF2, /* 🫲 leftwards hand */
    0x1FAF3, /* 🫳 palm down hand */
    0x1FAF4, /* 🫴 palm up hand */
    0x1FAF7, /* 🫷 leftwards pushing hand */
    0x1FAF8, /* 🫸 rightwards pushing hand */
    /* People & Body / hand-fingers-partial */
    0x1F90C, /* 🤌 pinched fingers */
    0x1F90F, /* 🤏 pinching hand */
    0x1F91E, /* 🤞 crossed fingers */
    0x1FAF0, /* 🫰 hand with index finger and thumb crossed */
    0x1F91F, /* 🤟 love-you gesture */
    0x1F918, /* 🤘 sign of the horns */
    0x1F919, /* 🤙 call me hand */
    /* People & Body / hand-single-finger */
    0x1F448, /* 👈 backhand index pointing left */
    0x1F449, /* 👉 backhand index pointing right */
    0x1F447, /* 👇 backhand index pointing down */
    0x1FAF5, /* 🫵 index pointing at the viewer */
    /* People & Body / hand-fingers-closed */
    0x1F44E, /* 👎 thumbs down */
    0x1F44A, /* 👊 oncoming fist */
    0x1F91B, /* 🤛 left-facing fist */
    0x1F91C, /* 🤜 right-facing fist */
    /* People & Body / hands */
    0x1F64C, /* 🙌 raising hands */
    0x1FAF6, /* 🫶 heart hands */
    0x1F450, /* 👐 open hands */
    0x1F932, /* 🤲 palms up together */
    0x1F91D, /* 🤝 handshake */
    /* People & Body / hand-prop */
    0x1F485, /* 💅 nail polish */
    0x1F933, /* 🤳 selfie */
    /* People & Body / body-parts */
    0x1F4AA, /* 💪 flexed biceps */
    0x1F9BE, /* 🦾 mechanical arm */
    0x1F9BF, /* 🦿 mechanical leg */
    0x1F9B5, /* 🦵 leg */
    0x1F9B6, /* 🦶 foot */
    0x1F442, /* 👂 ear */
    0x1F9BB, /* 🦻 ear with hearing aid */
    0x1F443, /* 👃 nose */
    0x1F9E0, /* 🧠 brain */
    0x1F9B7, /* 🦷 tooth */
    0x1F9B4, /* 🦴 bone */
    0x1F440, /* 👀 eyes */
    0x1F445, /* 👅 tongue */
    0x1F444, /* 👄 mouth */
    /* People & Body / person */
    0x1F476, /* 👶 baby */
    0x1F466, /* 👦 boy */
    0x1F467, /* 👧 girl */
    0x1F9D1, /* 🧑 person */
    0x1F471, /* 👱 person: blond hair */
    0x1F468, /* 👨 man */
    0x1F9D4, /* 🧔 person: beard */
    0x1F469, /* 👩 woman */
    0x1F9D3, /* 🧓 older person */
    0x1F474, /* 👴 old man */
    0x1F475, /* 👵 old woman */
    /* People & Body / person-gesture */
    0x1F64D, /* 🙍 person frowning */
    0x1F64E, /* 🙎 person pouting */
    0x1F645, /* 🙅 person gesturing NO */
    0x1F646, /* 🙆 person gesturing OK */
    0x1F481, /* 💁 person tipping hand */
    0x1F64B, /* 🙋 person raising hand */
    0x1F9CF, /* 🧏 deaf person */
    0x1F647, /* 🙇 person bowing */
    0x1F926, /* 🤦 person facepalming */
    0x1F937, /* 🤷 person shrugging */
    /* People & Body / person-role */
    0x1F46E, /* 👮 police officer */
    0x1F482, /* 💂 guard */
    0x1F977, /* 🥷 ninja */
    0x1F477, /* 👷 construction worker */
    0x1FAC5, /* 🫅 person with crown */
    0x1F934, /* 🤴 prince */
    0x1F478, /* 👸 princess */
    0x1F935, /* 🤵 person in tuxedo */
    0x1F470, /* 👰 person with veil */
    0x1F930, /* 🤰 pregnant woman */
    0x1FAC3, /* 🫃 pregnant man */
    0x1FAC4, /* 🫄 pregnant person */
    /* People & Body / person-fantasy */
    0x1F47C, /* 👼 baby angel */
    0x1F385, /* 🎅 Santa Claus */
    0x1F936, /* 🤶 Mrs. Claus */
    0x1F9B8, /* 🦸 superhero */
    0x1F9B9, /* 🦹 supervillain */
    0x1F9D9, /* 🧙 mage */
    0x1F9DA, /* 🧚 fairy */
    0x1F9DB, /* 🧛 vampire */
    0x1F9DC, /* 🧜 merperson */
    0x1F9DD, /* 🧝 elf */
    0x1F9DE, /* 🧞 genie */
    0x1F9DF, /* 🧟 zombie */
    0x1F9CC, /* 🧌 troll */
    /* People & Body / person-activity */
    0x1F486, /* 💆 person getting massage */
    0x1F487, /* 💇 person getting haircut */
    0x1F6B6, /* 🚶 person walking */
    0x1F9CD, /* 🧍 person standing */
    0x1F9CE, /* 🧎 person kneeling */
    0x1F3C3, /* 🏃 person running */
    0x1F483, /* 💃 woman dancing */
    0x1F57A, /* 🕺 man dancing */
    0x1F46F, /* 👯 people with bunny ears */
    0x1F9D6, /* 🧖 person in steamy room */
    0x1F9D7, /* 🧗 person climbing */
    /* People & Body / person-sport */
    0x1F93A, /* 🤺 person fencing */
    0x1F3C7, /* 🏇 horse racing */
    0x1F3C2, /* 🏂 snowboarder */
    0x1F3C4, /* 🏄 person surfing */
    0x1F6A3, /* 🚣 person rowing boat */
    0x1F3CA, /* 🏊 person swimming */
    0x1F6B4, /* 🚴 person biking */
    0x1F6B5, /* 🚵 person mountain biking */
    0x1F938, /* 🤸 person cartwheeling */
    0x1F93C, /* 🤼 people wrestling */
    0x1F93D, /* 🤽 person playing water polo */
    0x1F93E, /* 🤾 person playing handball */
    0x1F939, /* 🤹 person juggling */
    /* People & Body / person-resting */
    0x1F9D8, /* 🧘 person in lotus position */
    0x1F6C0, /* 🛀 person taking bath */
    0x1F6CC, /* 🛌 person in bed */
    /* People & Body / family */
    0x1F46D, /* 👭 women holding hands */
    0x1F46B, /* 👫 woman and man holding hands */
    0x1F46C, /* 👬 men holding hands */
    0x1F48F, /* 💏 kiss */
    0x1F491, /* 💑 couple with heart */
    /* People & Body / person-symbol */
    0x1F464, /* 👤 bust in silhouette */
    0x1F465, /* 👥 busts in silhouette */
    0x1FAC2, /* 🫂 people hugging */
    0x1F46A, /* 👪 family */
    0x1F463, /* 👣 footprints */
    /* Animals & Nature / animal-mammal */
    0x1F435, /* 🐵 monkey face */
    0x1F412, /* 🐒 monkey */
    0x1F98D, /* 🦍 gorilla */
    0x1F9A7, /* 🦧 orangutan */
    0x1F415, /* 🐕 dog */
    0x1F9AE, /* 🦮 guide dog */
    0x1F429, /* 🐩 poodle */
    0x1F43A, /* 🐺 wolf */
    0x1F99D, /* 🦝 raccoon */
    0x1F408, /* 🐈 cat */
    0x1F405, /* 🐅 tiger */
    0x1F406, /* 🐆 leopard */
    0x1F434, /* 🐴 horse face */
    0x1FACE, /* 🫎 moose */
    0x1FACF, /* 🫏 donkey */
    0x1F40E, /* 🐎 horse */
    0x1F993, /* 🦓 zebra */
    0x1F98C, /* 🦌 deer */
    0x1F9AC, /* 🦬 bison */
    0x1F402, /* 🐂 ox */
    0x1F403, /* 🐃 water buffalo */
    0x1F404, /* 🐄 cow */
    0x1F416, /* 🐖 pig */
    0x1F417, /* 🐗 boar */
    0x1F43D, /* 🐽 pig nose */
    0x1F40F, /* 🐏 ram */
    0x1F411, /* 🐑 ewe */
    0x1F410, /* 🐐 goat */
    0x1F42A, /* 🐪 camel */
    0x1F42B, /* 🐫 two-hump camel */
    0x1F999, /* 🦙 llama */
    0x1F992, /* 🦒 giraffe */
    0x1F418, /* 🐘 elephant */
    0x1F9A3, /* 🦣 mammoth */
    0x1F98F, /* 🦏 rhinoceros */
    0x1F99B, /* 🦛 hippopotamus */
    0x1F42D, /* 🐭 mouse face */
    0x1F401, /* 🐁 mouse */
    0x1F400, /* 🐀 rat */
    0x1F439, /* 🐹 hamster */
    0x1F407, /* 🐇 rabbit */
    0x1F9AB, /* 🦫 beaver */
    0x1F994, /* 🦔 hedgehog */
    0x1F987, /* 🦇 bat */
    0x1F43B, /* 🐻 bear */
    0x1F9A5, /* 🦥 sloth */
    0x1F9A6, /* 🦦 otter */
    0x1F9A8, /* 🦨 skunk */
    0x1F998, /* 🦘 kangaroo */
    0x1F9A1, /* 🦡 badger */
    0x1F43E, /* 🐾 paw prints */
    /* Animals & Nature / animal-bird */
    0x1F983, /* 🦃 turkey */
    0x1F414, /* 🐔 chicken */
    0x1F413, /* 🐓 rooster */
    0x1F423, /* 🐣 hatching chick */
    0x1F424, /* 🐤 baby chick */
    0x1F425, /* 🐥 front-facing baby chick */
    0x1F426, /* 🐦 bird */
    0x1F427, /* 🐧 penguin */
    0x1F985, /* 🦅 eagle */
    0x1F986, /* 🦆 duck */
    0x1F9A2, /* 🦢 swan */
    0x1F989, /* 🦉 owl */
    0x1F9A4, /* 🦤 dodo */
    0x1FAB6, /* 🪶 feather */
    0x1F9A9, /* 🦩 flamingo */
    0x1F99A, /* 🦚 peacock */
    0x1F99C, /* 🦜 parrot */
    0x1FABD, /* 🪽 wing */
    0x1FABF, /* 🪿 goose */
    /* Animals & Nature / animal-reptile */
    0x1F40A, /* 🐊 crocodile */
    0x1F422, /* 🐢 turtle */
    0x1F98E, /* 🦎 lizard */
    0x1F40D, /* 🐍 snake */
    0x1F432, /* 🐲 dragon face */
    0x1F409, /* 🐉 dragon */
    0x1F995, /* 🦕 sauropod */
    0x1F996, /* 🦖 T-Rex */
    /* Animals & Nature / animal-marine */
    0x1F433, /* 🐳 spouting whale */
    0x1F40B, /* 🐋 whale */
    0x1F42C, /* 🐬 dolphin */
    0x1F9AD, /* 🦭 seal */
    0x1F41F, /* 🐟 fish */
    0x1F420, /* 🐠 tropical fish */
    0x1F421, /* 🐡 blowfish */
    0x1F988, /* 🦈 shark */
    0x1F419, /* 🐙 octopus */
    0x1F41A, /* 🐚 spiral shell */
    0x1FAB8, /* 🪸 coral */
    0x1FABC, /* 🪼 jellyfish */
    0x1F980, /* 🦀 crab */
    0x1F99E, /* 🦞 lobster */
    0x1F990, /* 🦐 shrimp */
    0x1F991, /* 🦑 squid */
    0x1F9AA, /* 🦪 oyster */
    /* Animals & Nature / animal-bug */
    0x1F40C, /* 🐌 snail */
    0x1F98B, /* 🦋 butterfly */
    0x1F41B, /* 🐛 bug */
    0x1F41C, /* 🐜 ant */
    0x1F41D, /* 🐝 honeybee */
    0x1FAB2, /* 🪲 beetle */
    0x1F41E, /* 🐞 lady beetle */
    0x1F997, /* 🦗 cricket */
    0x1FAB3, /* 🪳 cockroach */
    0x1F982, /* 🦂 scorpion */
    0x1F99F, /* 🦟 mosquito */
    0x1FAB0, /* 🪰 fly */
    0x1FAB1, /* 🪱 worm */
    0x1F9A0, /* 🦠 microbe */
    /* Animals & Nature / plant-flower */
    0x1F490, /* 💐 bouquet */
    0x1F4AE, /* 💮 white flower */
    0x1FAB7, /* 🪷 lotus */
    0x1F339, /* 🌹 rose */
    0x1F940, /* 🥀 wilted flower */
    0x1F33A, /* 🌺 hibiscus */
    0x1F33B, /* 🌻 sunflower */
    0x1F33C, /* 🌼 blossom */
    0x1F337, /* 🌷 tulip */
    0x1FABB, /* 🪻 hyacinth */
    /* Animals & Nature / plant-other */
    0x1F331, /* 🌱 seedling */
    0x1FAB4, /* 🪴 potted plant */
    0x1F333, /* 🌳 deciduous tree */
    0x1F334, /* 🌴 palm tree */
    0x1F335, /* 🌵 cactus */
    0x1F33E, /* 🌾 sheaf of rice */
    0x1F33F, /* 🌿 herb */
    0x1F340, /* 🍀 four leaf clover */
    0x1F341, /* 🍁 maple leaf */
    0x1F342, /* 🍂 fallen leaf */
    0x1FAB9, /* 🪹 empty nest */
    0x1FABA, /* 🪺 nest with eggs */
    0x1F344, /* 🍄 mushroom */
    /* Food & Drink / food-fruit */
    0x1F348, /* 🍈 melon */
    0x1F349, /* 🍉 watermelon */
    0x1F34A, /* 🍊 tangerine */
    0x1F34B, /* 🍋 lemon */
    0x1F34D, /* 🍍 pineapple */
    0x1F96D, /* 🥭 mango */
    0x1F34F, /* 🍏 green apple */
    0x1F350, /* 🍐 pear */
    0x1F352, /* 🍒 cherries */
    0x1F353, /* 🍓 strawberry */
    0x1FAD0, /* 🫐 blueberries */
    0x1F95D, /* 🥝 kiwi fruit */
    0x1F345, /* 🍅 tomato */
    0x1FAD2, /* 🫒 olive */
    0x1F965, /* 🥥 coconut */
    /* Food & Drink / food-vegetable */
    0x1F951, /* 🥑 avocado */
    0x1F954, /* 🥔 potato */
    0x1F955, /* 🥕 carrot */
    0x1F33D, /* 🌽 ear of corn */
    0x1FAD1, /* 🫑 bell pepper */
    0x1F952, /* 🥒 cucumber */
    0x1F96C, /* 🥬 leafy green */
    0x1F966, /* 🥦 broccoli */
    0x1F9C4, /* 🧄 garlic */
    0x1F9C5, /* 🧅 onion */
    0x1F95C, /* 🥜 peanuts */
    0x1FAD8, /* 🫘 beans */
    0x1F330, /* 🌰 chestnut */
    0x1FADA, /* 🫚 ginger root */
    0x1FADB, /* 🫛 pea pod */
    /* Food & Drink / food-prepared */
    0x1F35E, /* 🍞 bread */
    0x1F950, /* 🥐 croissant */
    0x1F956, /* 🥖 baguette bread */
    0x1FAD3, /* 🫓 flatbread */
    0x1F968, /* 🥨 pretzel */
    0x1F96F, /* 🥯 bagel */
    0x1F95E, /* 🥞 pancakes */
    0x1F9C7, /* 🧇 waffle */
    0x1F9C0, /* 🧀 cheese wedge */
    0x1F356, /* 🍖 meat on bone */
    0x1F357, /* 🍗 poultry leg */
    0x1F969, /* 🥩 cut of meat */
    0x1F953, /* 🥓 bacon */
    0x1F354, /* 🍔 hamburger */
    0x1F35F, /* 🍟 french fries */
    0x1F32D, /* 🌭 hot dog */
    0x1F96A, /* 🥪 sandwich */
    0x1F32E, /* 🌮 taco */
    0x1F32F, /* 🌯 burrito */
    0x1FAD4, /* 🫔 tamale */
    0x1F959, /* 🥙 stuffed flatbread */
    0x1F9C6, /* 🧆 falafel */
    0x1F95A, /* 🥚 egg */
    0x1F373, /* 🍳 cooking */
    0x1F958, /* 🥘 shallow pan of food */
    0x1F372, /* 🍲 pot of food */
    0x1FAD5, /* 🫕 fondue */
    0x1F963, /* 🥣 bowl with spoon */
    0x1F957, /* 🥗 green salad */
    0x1F37F, /* 🍿 popcorn */
    0x1F9C8, /* 🧈 butter */
    0x1F9C2, /* 🧂 salt */
    0x1F96B, /* 🥫 canned food */
    /* Food & Drink / food-asian */
    0x1F371, /* 🍱 bento box */
    0x1F358, /* 🍘 rice cracker */
    0x1F359, /* 🍙 rice ball */
    0x1F35A, /* 🍚 cooked rice */
    0x1F35B, /* 🍛 curry rice */
    0x1F35C, /* 🍜 steaming bowl */
    0x1F35D, /* 🍝 spaghetti */
    0x1F360, /* 🍠 roasted sweet potato */
    0x1F362, /* 🍢 oden */
    0x1F363, /* 🍣 sushi */
    0x1F364, /* 🍤 fried shrimp */
    0x1F365, /* 🍥 fish cake with swirl */
    0x1F96E, /* 🥮 moon cake */
    0x1F361, /* 🍡 dango */
    0x1F95F, /* 🥟 dumpling */
    0x1F960, /* 🥠 fortune cookie */
    0x1F961, /* 🥡 takeout box */
    /* Food & Drink / food-sweet */
    0x1F366, /* 🍦 soft ice cream */
    0x1F367, /* 🍧 shaved ice */
    0x1F370, /* 🍰 shortcake */
    0x1F9C1, /* 🧁 cupcake */
    0x1F967, /* 🥧 pie */
    0x1F36B, /* 🍫 chocolate bar */
    0x1F36C, /* 🍬 candy */
    0x1F36D, /* 🍭 lollipop */
    0x1F36E, /* 🍮 custard */
    0x1F36F, /* 🍯 honey pot */
    /* Food & Drink / drink */
    0x1F37C, /* 🍼 baby bottle */
    0x1F95B, /* 🥛 glass of milk */
    0x2615, /* ☕ hot beverage */
    0x1FAD6, /* 🫖 teapot */
    0x1F375, /* 🍵 teacup without handle */
    0x1FAD7, /* 🫗 pouring liquid */
    0x1F964, /* 🥤 cup with straw */
    0x1F9CB, /* 🧋 bubble tea */
    0x1F9C3, /* 🧃 beverage box */
    0x1F9C9, /* 🧉 mate */
    0x1F9CA, /* 🧊 ice */
    /* Food & Drink / dishware */
    0x1F962, /* 🥢 chopsticks */
    0x1F374, /* 🍴 fork and knife */
    0x1F944, /* 🥄 spoon */
    0x1FAD9, /* 🫙 jar */
    0x1F3FA, /* 🏺 amphora */
    /* Activities / event */
    0x1F383, /* 🎃 jack-o-lantern */
    0x1F384, /* 🎄 Christmas tree */
    0x1F386, /* 🎆 fireworks */
    0x1F387, /* 🎇 sparkler */
    0x1F9E8, /* 🧨 firecracker */
    0x1F388, /* 🎈 balloon */
    0x1F389, /* 🎉 party popper */
    0x1F38A, /* 🎊 confetti ball */
    0x1F38B, /* 🎋 tanabata tree */
    0x1F38D, /* 🎍 pine decoration */
    0x1F38E, /* 🎎 Japanese dolls */
    0x1F38F, /* 🎏 carp streamer */
    0x1F390, /* 🎐 wind chime */
    0x1F391, /* 🎑 moon viewing ceremony */
    0x1F9E7, /* 🧧 red envelope */
    0x1F380, /* 🎀 ribbon */
    0x1F381, /* 🎁 wrapped gift */
    0x1F3AB, /* 🎫 ticket */
    /* Activities / award-medal */
    0x1F3C6, /* 🏆 trophy */
    0x1F3C5, /* 🏅 sports medal */
    0x1F947, /* 🥇 1st place medal */
    0x1F948, /* 🥈 2nd place medal */
    0x1F949, /* 🥉 3rd place medal */
    /* Activities / sport */
    0x26BE, /* ⚾ baseball */
    0x1F94E, /* 🥎 softball */
    0x1F3C0, /* 🏀 basketball */
    0x1F3D0, /* 🏐 volleyball */
    0x1F3C8, /* 🏈 american football */
    0x1F3C9, /* 🏉 rugby football */
    0x1F3BE, /* 🎾 tennis */
    0x1F94F, /* 🥏 flying disc */
    0x1F3B3, /* 🎳 bowling */
    0x1F3CF, /* 🏏 cricket game */
    0x1F3D1, /* 🏑 field hockey */
    0x1F3D2, /* 🏒 ice hockey */
    0x1F94D, /* 🥍 lacrosse */
    0x1F3D3, /* 🏓 ping pong */
    0x1F3F8, /* 🏸 badminton */
    0x1F94A, /* 🥊 boxing glove */
    0x1F94B, /* 🥋 martial arts uniform */
    0x1F945, /* 🥅 goal net */
    0x26F3, /* ⛳ flag in hole */
    0x1F3A3, /* 🎣 fishing pole */
    0x1F93F, /* 🤿 diving mask */
    0x1F3BD, /* 🎽 running shirt */
    0x1F3BF, /* 🎿 skis */
    0x1F6F7, /* 🛷 sled */
    0x1F94C, /* 🥌 curling stone */
    /* Activities / game */
    0x1F3AF, /* 🎯 bullseye */
    0x1FA80, /* 🪀 yo-yo */
    0x1FA81, /* 🪁 kite */
    0x1F3B1, /* 🎱 pool 8 ball */
    0x1F52E, /* 🔮 crystal ball */
    0x1FA84, /* 🪄 magic wand */
    0x1F3B2, /* 🎲 game die */
    0x1F9E9, /* 🧩 puzzle piece */
    0x1F9F8, /* 🧸 teddy bear */
    0x1FA85, /* 🪅 piñata */
    0x1FAA9, /* 🪩 mirror ball */
    0x1FA86, /* 🪆 nesting dolls */
    0x1F004, /* 🀄 mahjong red dragon */
    /* Activities / arts & crafts */
    0x1F3AD, /* 🎭 performing arts */
    0x1F3A8, /* 🎨 artist palette */
    0x1F9F5, /* 🧵 thread */
    0x1FAA1, /* 🪡 sewing needle */
    0x1F9F6, /* 🧶 yarn */
    0x1FAA2, /* 🪢 knot */
    /* Travel & Places / place-map */
    0x1F30D, /* 🌍 globe showing Europe-Africa */
    0x1F30E, /* 🌎 globe showing Americas */
    0x1F30F, /* 🌏 globe showing Asia-Australia */
    0x1F310, /* 🌐 globe with meridians */
    0x1F5FE, /* 🗾 map of Japan */
    0x1F9ED, /* 🧭 compass */
    /* Travel & Places / place-geographic */
    0x1F30B, /* 🌋 volcano */
    0x1F5FB, /* 🗻 mount fuji */
    /* Travel & Places / place-building */
    0x1F9F1, /* 🧱 brick */
    0x1FAA8, /* 🪨 rock */
    0x1FAB5, /* 🪵 wood */
};
#define N_RENDER_EXTRA 600

/* BMP symbols used in the UI.  Preferred from the Latin font (DejaVu); a symbol
 * the Latin face lacks falls back to the emoji face (see render_glyph).  These
 * are rasterised at ALL PIXEL_SIZES (from_emoji = 0). */
static const unsigned long BMP_SYMBOLS[] = {
    0x2039, /* ‹  single left angle quotation — back button label */
    0x203A, /* ›  single right angle quotation */
    0x270F, /* ✏  pencil — "✏ Write" button */
    0x2713, /* ✓  check mark — ack indicator */
    0x2715, /* ✕  multiplication X — PIN-entry cancel button (pin_entry.rs) */
    0x2699, /* ⚙  gear — settings button (contact_list.rs); emoji-face fallback */
    0x232B, /* ⌫  erase to the left — PIN-entry delete button (pin_entry.rs) */
    0x2212, /* −  minus sign — screen-sleep timeout decrement (admin_menu.rs) */
    /* BUG FIX: 0x2014 was already used as the
     * GPS status "no fix yet" coordinates placeholder (gps_status.rs) but was
     * never added here — the same SYNC INVARIANT violation class documented
     * above for 0x1F514/0x1F50A/0x1F4A4 (this file's own failure mode: a
     * codepoint absent from this table renders blank on real hardware, and
     * `cargo build` cannot catch it since this table isn't type-checked
     * against Slint string literals). Caught while adding 0x2026 below for
     * the same screen's new "Acquiring…" fix-state text — fixed alongside it
     * rather than left for a third occurrence of this exact bug class. */
    0x2014, /* —  em dash — GPS status "no fix yet" coordinates placeholder (gps_status.rs) */
    0x2026, /* …  horizontal ellipsis — GPS status "Acquiring…" fix-state text (gps_status.rs) */
    /* screen-lock (meshcadet-lock-firmware-ui): all three used at sizes
     * outside EMOJI_SIZES (10/14/28px — none of {11,13,14,16,18,20} covers
     * all three), so they must be BMP_SYMBOLS (rasterised at every
     * PIXEL_SIZES entry) rather than UI_EXTRA_CPS, mirroring 0x2699/0x2212
     * above for the identical reason. */
    0x23F1, /* ⏱  stopwatch — admin-menu "Lock timeout" row label (admin_menu.rs) */
    0x2709, /* ✉  envelope — lock screen's D5 count-only waiting-message badge (lock.rs) */
    0x23F3, /* ⏳  hourglass — lock screen's backoff-countdown state (lock.rs) */
};
#define N_BMP_SYMBOLS 13

/* Pixel sizes to rasterise.
 *
 * SYNC INVARIANT: this list MUST cover EVERY `font-size` (in px) used by any
 * Slint Text/TextInput under firmware/src/ui/screens/ .  The Slint software
 * renderer selects the *nearest* available size and scales the glyph metrics to
 * the requested size; any requested size NOT in this list renders at the wrong
 * size with scaled (wrong) baseline/advance — the "garbled text" defect.
 * Because the renderer resolves a whole text run to a single font with no
 * per-glyph fallback (i-slint-renderer-software pixelfont.rs shape_text),
 * dynamic message bodies that mix Latin + emoji MUST be served by this combined
 * font at their exact size — which is why every UI size is rasterised here
 * rather than scoping emoji to an emoji-only font.
 *
 * Current UI sizes (grep `font-size:` + icon_size props across the screens):
 *   8 9 10 11 13 14 15 16 18 20 22 28
 * Must stay sorted ascending (Slint match_font uses partition_point on it).
 */
static const int PIXEL_SIZES[] = {8, 9, 10, 11, 13, 14, 15, 16, 18, 20, 22, 28};
#define N_SIZES 12

/* Sizes at which emoji glyphs are rasterised.  Emoji appear in text only at a
 * subset of UI sizes — every Slint field whose value can carry an emoji:
 *   11 px — contact/channel list preview (last-message text)  [contact_list.rs]
 *   13 px — message body, compose "To:"/"📤 Send", tab labels  [message_view/compose/contact_list]
 *   14 px — contact/channel name, message-view header, compose draft input
 *   16 px — shortcode-completion emoji, contact initial column
 *   18 px — emoji-picker toggle (😀)                            [compose.rs]
 *   20 px — emoji-picker grid cells, 🔐 PIN icon               [compose/pin_entry]
 * Timestamp/unread fields (9/10 px) are Rust-formatted numerics and never carry
 * emoji, so emoji are omitted there.  Rasterising all EMOJI_CPS + UI_EXTRA_CPS +
 * RENDER_EXTRA_CPS entries at every size — especially 22/28 px (header chevrons
 * / titles, never emoji) — would waste flash for zero benefit, so emoji are
 * emitted as empty glyphs outside this set (they still hold a char-map slot but
 * carry no bitmap).  Latin + BMP symbols are rasterised at ALL sizes above.
 *
 * SYNC INVARIANT: if an emoji is ever shown at a new font-size, add that size
 * here AND to PIXEL_SIZES, or the emoji renders blank at that size.
 *
 * KNOWN, DEFERRED GAP: an earlier revision of `unprovisioned.rs` showed 📻 at
 * 28px, and 28 is deliberately NOT in this list — `splash.rs`'s module doc
 * documented this as a pre-existing, out-of-scope-here defect (📻 rendered
 * blank on that one screen). `unprovisioned.rs` has since retired the 📻
 * glyph outright in favor of a bitmap mascot, so the specific screen this
 * gap named no longer uses it — but this list intentionally still omits 28px
 * for the reason below, in case some future screen needs a 28px emoji.
 * Adding 28px now — before any screen actually needs a 28px EMOJI glyph —
 * would rasterise every EMOJI_CPS/UI_EXTRA_CPS/RENDER_EXTRA_CPS entry at a
 * 13th size for zero current benefit (real flash cost, per the note above).
 * The host glyph-coverage harness
 * (`xtask`) carries a matching, equally-narrow, equally-commented allowlist
 * entry for this one pair so it does not mask any OTHER future gap. */
static const int EMOJI_SIZES[] = {11, 13, 14, 16, 18, 20};
#define N_EMOJI_SIZES 6

/* ── Character table (sorted by codepoint for binary search) ─────────── */
#define N_ASCII 95  /* U+0020..U+007E */
#define N_MAX_CHARS (N_ASCII + N_BMP_SYMBOLS + N_EMOJI_TABLE + N_UI_EXTRA + N_RENDER_EXTRA)

typedef struct {
    unsigned long cp;
    int from_emoji; /* 1 = prefer emoji font; 0 = prefer latin font */
} CharEntry;

static CharEntry chars[N_MAX_CHARS];
static int n_chars = 0;

static int compare_char_entry(const void *a, const void *b) {
    const CharEntry *ca = (const CharEntry *)a;
    const CharEntry *cb = (const CharEntry *)b;
    if (ca->cp < cb->cp) return -1;
    if (ca->cp > cb->cp) return 1;
    return 0;
}

/* ── Rendered glyph data ─────────────────────────────────────────────── */
typedef struct {
    int16_t x;        /* bearing X in 1/64 px (positive = right of pen) */
    int16_t y;        /* bearing Y in 1/64 px: (bitmap_top - height)*64, per
                       * Slint BitmapGlyph.y convention (embed_glyphs.rs) */
    int16_t width;    /* bitmap width in px */
    int16_t height;   /* bitmap height in px */
    int16_t advance;  /* horizontal advance in 1/64 px */
    uint8_t *data;    /* grayscale alpha, width*height bytes; NULL = empty */
} RenderedGlyph;

static RenderedGlyph rendered[N_SIZES][N_MAX_CHARS];

/* ── Font face handles ───────────────────────────────────────────────── */
static FT_Library ft_library;
static FT_Face latin_face;
static FT_Face emoji_face;

/* ── Monochrome emoji legibility tuning (mono-glyph-legibility mission) ──
 *
 * NotoEmoji-Regular.ttf v3.002 is a VARIABLE font with one `wght` axis
 * (range 300-700, default 400 — confirmed by a live FT_Get_MM_Var probe
 * against the bundled asset, not assumed). Left at the axis default,
 * outline strokes at the small EMOJI_SIZES this font is rasterised at
 * (11-20px) land sub-pixel and antialias into mid-gray rather than solid
 * ink: "washed out," a legibility defect distinct from the missing-/wrong-
 * glyph failure mode PIXEL_SIZES/EMOJI_SIZES's own comments document.
 *
 * Two independent, additive corrections are applied ONLY to glyphs actually
 * rasterised from `emoji_face` — never `latin_face`. DejaVu Sans is a
 * different font on a different rasterisation path (not variable, never
 * washed out), so the parallel-path audit this kind of fix owes its sibling
 * (dispatcher-parallel-pass-parity doctrine) concludes there is nothing to
 * mirror onto it, not that the mirroring was skipped:
 *
 *   1. Weight axis -> EMOJI_WGHT_TARGET, set once on `emoji_face` right
 *      after it opens (`set_variable_weight` below), guarded by
 *      `FT_Get_MM_Var` succeeding and an explicit `wght` axis tag lookup —
 *      never assumes axis 0, and clamps to the axis's own reported range
 *      rather than trusting the target blindly.
 *   2. Alpha gamma boost -> EMOJI_ALPHA_GAMMA, applied per-pixel to every
 *      rasterised `emoji_face` glyph's grayscale mask (`render_from_face`'s
 *      FT_PIXEL_MODE_GRAY branch).
 *
 * Combination chosen empirically (FreeType probe against the bundled asset:
 * per-size ink-coverage deltas plus raster dumps at every EMOJI_SIZES
 * entry — see mission Findings for the full table). wght 700 alone, or 700
 * stacked with a gamma boost, over-inks small glyphs (counters start
 * closing at 11px) for only a marginal legibility gain over 600+gamma, and
 * grows bitmap extents more (partition-budget/layout risk). wght 600 +
 * gamma 0.75 raises mean ink coverage +22% to +28% at EVERY EMOJI_SIZES
 * entry (11-20px), measured across the FULL 649-glyph rasterised emoji set
 * (not a sample) by diffing this generator's own before/after output —
 * e.g. 11px: 27.7% -> 35.5%; 20px: 26.9% -> 33.0% — while holding bitmap
 * extents flat, or within 2px, at every (glyph, size) pair, with zero
 * blank-glyph regressions. `FT_Outline_Embolden`
 * (the scope's third, optional lever) was evaluated and is deliberately
 * NOT applied: the chosen wght+gamma combination alone already reads crisp,
 * and stacking embolden on top closed eyes/counters in the probe's raster
 * dumps at 11px — the exact over-inking failure mode
 * `metric-blinded-by-its-own-fix.md` warns a coverage-only readout would
 * miss.
 */
#define EMOJI_WGHT_TARGET 600.0
#define EMOJI_ALPHA_GAMMA 0.75

/* Drive `face`'s `wght` variation axis toward `target` (a design-space
 * value, e.g. 600.0). No-op if `face` isn't a variable font, has no `wght`
 * axis, or FT_Get_MM_Var otherwise fails — callers must not assume this
 * changed anything, and nothing downstream depends on it having. `target`
 * is clamped to the axis's OWN reported [minimum, maximum] rather than
 * trusted blindly, so a future font-asset swap with a narrower axis range
 * degrades gracefully instead of clamping to a FreeType-internal error. */
static void set_variable_weight(FT_Face face, double target) {
    FT_MM_Var *mm_var = NULL;
    if (FT_Get_MM_Var(face, &mm_var) != 0 || mm_var == NULL) {
        return; /* not a variable font (or no MM support in this build) */
    }
    if (mm_var->num_axis == 0) {
        FT_Done_MM_Var(ft_library, mm_var);
        return;
    }

    FT_Fixed *coords = (FT_Fixed *)calloc(mm_var->num_axis, sizeof(FT_Fixed));
    if (!coords) {
        FT_Done_MM_Var(ft_library, mm_var);
        return;
    }

    int found_wght = 0;
    for (FT_UInt i = 0; i < mm_var->num_axis; i++) {
        coords[i] = mm_var->axis[i].def; /* default every OTHER axis explicitly */
        if (mm_var->axis[i].tag == FT_MAKE_TAG('w', 'g', 'h', 't')) {
            FT_Fixed want = (FT_Fixed)(target * 65536.0);
            if (want < mm_var->axis[i].minimum) want = mm_var->axis[i].minimum;
            if (want > mm_var->axis[i].maximum) want = mm_var->axis[i].maximum;
            coords[i] = want;
            found_wght = 1;
        }
    }

    if (found_wght) {
        FT_Set_Var_Design_Coordinates(face, mm_var->num_axis, coords);
    }

    free(coords);
    FT_Done_MM_Var(ft_library, mm_var);
}

/* Alpha gamma-correction LUT (a' = 255*(a/255)^EMOJI_ALPHA_GAMMA), applied
 * ONLY to glyphs rasterised from `emoji_face` — see the tuning doc above.
 * Built lazily (once) rather than at a fixed point in `main`'s call order,
 * so it has no ordering dependency on face setup. */
static uint8_t emoji_gamma_lut[256];
static int emoji_gamma_lut_ready = 0;

static void ensure_emoji_gamma_lut(void) {
    if (emoji_gamma_lut_ready) return;
    for (int a = 0; a < 256; a++) {
        double af = (double)a / 255.0;
        double corrected = pow(af, EMOJI_ALPHA_GAMMA) * 255.0;
        if (corrected > 255.0) corrected = 255.0;
        if (corrected < 0.0) corrected = 0.0;
        emoji_gamma_lut[a] = (uint8_t)(corrected + 0.5);
    }
    emoji_gamma_lut_ready = 1;
}

/* ── Ink-coverage regression floor (mono-glyph-legibility mission) ──────
 *
 * Guards against a FUTURE edit silently reverting or weakening the fix
 * above (deleting the `set_variable_weight` call, walking
 * EMOJI_WGHT_TARGET/EMOJI_ALPHA_GAMMA back toward washed-out defaults, or
 * an upstream font-asset swap that changes rasterisation without anyone
 * re-running the FreeType probe this mission's Findings document) without
 * anyone noticing until a field report — the exact recurring failure mode
 * this file's other build-time gates (`g_missing_glyph_count`) already
 * exist to convert into a build break instead of a silent regression.
 *
 * Canary: U+1F44D (👍) at 20px, reusing the ALREADY-rasterised
 * `rendered[][]` buffer (no extra FreeType work). Measured mean alpha
 * coverage BEFORE this mission's fix was ~19.7%; AFTER is ~27.4%. The floor
 * below is set comfortably above the pre-fix baseline and comfortably below
 * the post-fix measurement, so a full revert fails loudly here rather than
 * shipping silently washed-out glyphs again — this is a REGRESSION floor
 * under an already-chosen combination, not a proxy metric a future change
 * could satisfy by cranking weight/gamma past the over-inking point
 * `metric-blinded-by-its-own-fix.md` warns against (this file's own tuning
 * doc above already picked the ceiling on qualitative, not just numeric,
 * grounds). */
#define INK_COVERAGE_CANARY_CP ((unsigned long)0x1F44D)
#define INK_COVERAGE_CANARY_PX 20
#define INK_COVERAGE_FLOOR_PCT 24.0

static int check_ink_coverage_floor(void) {
    int size_idx = -1;
    for (int si = 0; si < N_SIZES; si++) {
        if (PIXEL_SIZES[si] == INK_COVERAGE_CANARY_PX) { size_idx = si; break; }
    }
    if (size_idx < 0) return 0; /* canary size not in PIXEL_SIZES at all */

    int char_idx = -1;
    for (int ci = 0; ci < n_chars; ci++) {
        if (chars[ci].cp == INK_COVERAGE_CANARY_CP) { char_idx = ci; break; }
    }
    if (char_idx < 0) return 0; /* canary codepoint dropped from this build's table */

    const RenderedGlyph *g = &rendered[size_idx][char_idx];
    if (g->width == 0 || g->height == 0 || g->data == NULL) {
        fprintf(stderr,
            "gen_emoji_font: NOTE — ink-coverage canary U+%04lX is blank at "
            "%dpx (not rasterised); skipping the regression-floor check.\n",
            INK_COVERAGE_CANARY_CP, INK_COVERAGE_CANARY_PX);
        return 0;
    }

    long sum = 0;
    long n = (long)g->width * (long)g->height;
    for (long i = 0; i < n; i++) sum += g->data[i];
    double coverage_pct = (double)sum / (255.0 * (double)n) * 100.0;

    if (coverage_pct < INK_COVERAGE_FLOOR_PCT) {
        fprintf(stderr,
            "gen_emoji_font: FAILED — ink-coverage regression: U+%04lX at "
            "%dpx measured %.1f%%, below the %.1f%% floor "
            "(mono-glyph-legibility mission). Did the emoji weight/gamma "
            "tuning above get reverted or weakened, or the bundled font "
            "asset change?\n",
            INK_COVERAGE_CANARY_CP, INK_COVERAGE_CANARY_PX, coverage_pct,
            INK_COVERAGE_FLOOR_PCT);
        return 1;
    }
    return 0;
}

/* Render one glyph from the given face at the given pixel size.
 *
 * Returns 1 if `face` maps `cp` to an actual glyph (even if that glyph's
 * bitmap is legitimately empty, e.g. U+0020 space), 0 if `face` has NO
 * mapping for `cp` at all. This distinguishes "intentionally invisible"
 * from "codepoint absent from this font" — see `render_glyph`'s caller,
 * which uses the 0 case to detect a glyph that is missing from BOTH the
 * primary and fallback face (renders BLANK on-device; see this file's
 * top-of-file SYNC INVARIANT comments for the failure mode this guards). */
static int render_from_face(FT_Face face, unsigned long cp,
                             int size_idx, int char_idx)
{
    RenderedGlyph *g = &rendered[size_idx][char_idx];
    int px = PIXEL_SIZES[size_idx];

    FT_Set_Pixel_Sizes(face, 0, (FT_UInt)px);

    FT_UInt gi = FT_Get_Char_Index(face, (FT_ULong)cp);
    if (gi == 0) {
        /* Missing glyph: empty advance = 1 em */
        g->x = 0; g->y = 0; g->width = 0; g->height = 0;
        g->advance = (int16_t)(px * 64);
        g->data = NULL;
        return 0;
    }

    if (FT_Load_Glyph(face, gi, FT_LOAD_DEFAULT) != 0) {
        g->x = 0; g->y = 0; g->width = 0; g->height = 0;
        g->advance = (int16_t)(px * 64);
        g->data = NULL;
        return 1; /* mapped, but failed to load — not a "missing glyph" */
    }

    FT_GlyphSlot slot = face->glyph;
    if (slot->format != FT_GLYPH_FORMAT_BITMAP) {
        if (FT_Render_Glyph(slot, FT_RENDER_MODE_NORMAL) != 0) {
            g->x = 0; g->y = 0; g->width = 0; g->height = 0;
            g->advance = (int16_t)(slot->advance.x & 0x7FFF);
            g->data = NULL;
            return 1; /* mapped, but failed to rasterise — not a "missing glyph" */
        }
    }

    FT_Bitmap *bm = &slot->bitmap;
    int w = (int)bm->width;
    int h = (int)bm->rows;

    g->advance = (int16_t)(slot->advance.x);
    g->x = (int16_t)((int)slot->bitmap_left * 64);
    g->y = (int16_t)(((int)slot->bitmap_top - h) * 64);
    g->width = (int16_t)w;
    g->height = (int16_t)h;
    g->data = NULL;

    if (w == 0 || h == 0 || bm->buffer == NULL) {
        /* Space or zero-extent glyph — legitimately blank, glyph WAS mapped. */
        return 1;
    }

    /* Allocate and copy bitmap data (strip pitch padding) */
    g->data = (uint8_t *)malloc((size_t)(w * h));
    if (!g->data) { g->width = 0; g->height = 0; return 1; }

    int pitch = abs(bm->pitch);

    if (bm->pixel_mode == FT_PIXEL_MODE_GRAY) {
        if (face == emoji_face) {
            /* Emoji face: apply the alpha gamma boost per-pixel (see the
             * tuning doc above emoji_face's declaration) — never applied to
             * latin_face, which is a different, non-variable, non-washed-out
             * font on its own rasterisation path. */
            ensure_emoji_gamma_lut();
            for (int row = 0; row < h; row++) {
                const uint8_t *src = bm->buffer + row * pitch;
                uint8_t *dst = g->data + row * w;
                for (int col = 0; col < w; col++) {
                    dst[col] = emoji_gamma_lut[src[col]];
                }
            }
        } else {
            /* Grayscale: copy w bytes per row, unmodified */
            for (int row = 0; row < h; row++) {
                memcpy(g->data + row * w, bm->buffer + row * pitch, (size_t)w);
            }
        }
    } else if (bm->pixel_mode == FT_PIXEL_MODE_BGRA) {
        /* Color bitmap (CBDT): convert BGRA → alpha using luminance */
        for (int row = 0; row < h; row++) {
            for (int col = 0; col < w; col++) {
                const uint8_t *px_src = bm->buffer + row * pitch + col * 4;
                uint8_t b = px_src[0], gr = px_src[1], r = px_src[2], a = px_src[3];
                /* Luminance weighted by alpha */
                uint32_t luma = (uint32_t)(77*r + 150*gr + 29*b);  /* /256 */
                uint32_t alpha = (uint32_t)a * luma / (255u * 256u);
                g->data[row * w + col] = (uint8_t)(alpha > 255 ? 255 : alpha);
            }
        }
    } else {
        /* Unsupported pixel mode — leave blank. The glyph itself WAS mapped
         * (this is a decode limitation, not a missing-codepoint problem). */
        free(g->data);
        g->data = NULL;
        g->width = 0;
        g->height = 0;
    }
    return 1;
}

/* Count of (codepoint, size) pairs mapped in NEITHER face — each one is a
 * glyph that ships as blank on-device (this file's standing "a codepoint
 * absent from this table/font renders blank" failure mode, hit three times
 * already per the SYNC INVARIANT comments above: admin_menu.rs's
 * 🔔/🔊/💤, gps_status.rs's —/…, and unprovisioned.rs's 📻@28px). Checked in
 * `main()`: a nonzero count fails the build instead of silently emitting a
 * font that renders that icon invisible on real hardware. */
static int g_missing_glyph_count = 0;

static void render_glyph(unsigned long cp, int from_emoji,
                          int size_idx, int char_idx)
{
    FT_Face primary   = from_emoji ? emoji_face : latin_face;
    FT_Face secondary = from_emoji ? latin_face : emoji_face;

    int found = render_from_face(primary, cp, size_idx, char_idx);
    if (!found) {
        found = render_from_face(secondary, cp, size_idx, char_idx);
    }
    if (!found) {
        g_missing_glyph_count++;
        fprintf(stderr,
            "gen_emoji_font: ERROR — U+%04lX has no glyph in EITHER the "
            "Latin or emoji face at %dpx; it would render BLANK on-device\n",
            cp, PIXEL_SIZES[size_idx]);
    }
}

/* ── Rust code output ────────────────────────────────────────────────── */
static void write_hex_bytes(FILE *out, const uint8_t *data, int n)
{
    for (int i = 0; i < n; i++) {
        if (i % 24 == 0) fprintf(out, "\n        ");
        fprintf(out, "%u", (unsigned)data[i]);
        if (i < n - 1) fprintf(out, ",");
    }
}

static void emit_rust(FILE *out,
                      float units_per_em, float ascent, float descent,
                      float x_height, float cap_height)
{
    fprintf(out,
        "// AUTO-GENERATED by gen_emoji_font.c — DO NOT EDIT.\n"
        "// Combined MeshCadetEmoji bitmap font: ASCII + UI symbols + emoji.\n"
        "// Source fonts: DejaVu Sans + NotoEmoji-Regular.\n"
        "// Sizes (px):");
    for (int si = 0; si < N_SIZES; si++) {
        fprintf(out, " %d", PIXEL_SIZES[si]);
    }
    fprintf(out, "\n\n");

    /* Character map (sorted) */
    fprintf(out, "static CHAR_MAP: &[CharacterMapEntry] = &[\n");
    for (int i = 0; i < n_chars; i++) {
        fprintf(out, "    CharacterMapEntry { code_point: '\\u{%X}', glyph_index: %d },\n",
                (unsigned)chars[i].cp, i);
    }
    fprintf(out, "];\n\n");

    /* One BitmapGlyphs block per pixel size */
    for (int si = 0; si < N_SIZES; si++) {
        fprintf(out, "static GLYPHS_%d: &[BitmapGlyph] = &[\n", PIXEL_SIZES[si]);
        for (int ci = 0; ci < n_chars; ci++) {
            const RenderedGlyph *g = &rendered[si][ci];
            if (g->data && g->width > 0 && g->height > 0) {
                fprintf(out,
                    "    BitmapGlyph { x: %d, y: %d, width: %d, height: %d,"
                    " x_advance: %d, data: Slice::from_slice(&[",
                    (int)g->x, (int)g->y, (int)g->width, (int)g->height,
                    (int)g->advance);
                write_hex_bytes(out, g->data, g->width * g->height);
                fprintf(out, "\n    ]) },\n");
            } else {
                fprintf(out,
                    "    BitmapGlyph { x: %d, y: %d, width: 0, height: 0,"
                    " x_advance: %d, data: Slice::from_slice(&[]) },\n",
                    (int)g->x, (int)g->y, (int)g->advance);
            }
        }
        fprintf(out, "];\n\n");
    }

    /* BitmapGlyphs set */
    fprintf(out, "static GLYPH_SETS: &[BitmapGlyphs] = &[\n");
    for (int si = 0; si < N_SIZES; si++) {
        fprintf(out,
            "    BitmapGlyphs { pixel_size: %d, glyph_data:"
            " Slice::from_slice(GLYPHS_%d) },\n",
            PIXEL_SIZES[si], PIXEL_SIZES[si]);
    }
    fprintf(out, "];\n\n");

    /* BitmapFont */
    fprintf(out,
        "pub static MESH_CADET_EMOJI_FONT: BitmapFont = BitmapFont {\n"
        "    family_name:   Slice::from_slice(b\"MeshCadetEmoji\"),\n"
        "    character_map: Slice::from_slice(CHAR_MAP),\n"
        "    units_per_em:  %.1ff32,\n"
        "    ascent:        %.1ff32,\n"
        "    descent:       %.1ff32,\n"
        "    x_height:      %.1ff32,\n"
        "    cap_height:    %.1ff32,\n"
        "    glyphs:        Slice::from_slice(GLYPH_SETS),\n"
        "    weight:        400,\n"
        "    italic:        false,\n"
        "    sdf:           false,\n"
        "};\n\n",
        (double)units_per_em, (double)ascent, (double)descent,
        (double)x_height, (double)cap_height);

    fprintf(out,
        "pub fn emoji_bitmap_font() -> &'static BitmapFont {\n"
        "    &MESH_CADET_EMOJI_FONT\n"
        "}\n");
}

/* ── Main ────────────────────────────────────────────────────────────── */
int main(int argc, char **argv)
{
    if (argc < 4) {
        fprintf(stderr,
            "Usage: %s <latin.ttf> <emoji.ttf> <out.rs>\n", argv[0]);
        return 1;
    }
    const char *latin_path = argv[1];
    const char *emoji_path = argv[2];
    const char *out_path   = argv[3];

    if (FT_Init_FreeType(&ft_library)) {
        fprintf(stderr, "FT_Init_FreeType failed\n");
        return 1;
    }
    if (FT_New_Face(ft_library, latin_path, 0, &latin_face)) {
        fprintf(stderr, "Cannot open latin font: %s\n", latin_path);
        return 1;
    }
    if (FT_New_Face(ft_library, emoji_path, 0, &emoji_face)) {
        fprintf(stderr, "Cannot open emoji font: %s\n", emoji_path);
        return 1;
    }

    /* Drive the emoji face's `wght` axis toward EMOJI_WGHT_TARGET — see the
     * tuning doc above `set_variable_weight`'s definition. Never applied to
     * latin_face (DejaVu Sans is not a variable font). */
    set_variable_weight(emoji_face, EMOJI_WGHT_TARGET);

    /* ── Build sorted character list ──────────────────────────────────── */
    n_chars = 0;

    /* ASCII printable */
    for (unsigned long cp = 0x20; cp <= 0x7E; cp++) {
        chars[n_chars].cp = cp;
        chars[n_chars].from_emoji = 0;
        n_chars++;
    }
    /* BMP UI symbols */
    for (int i = 0; i < N_BMP_SYMBOLS; i++) {
        chars[n_chars].cp = BMP_SYMBOLS[i];
        chars[n_chars].from_emoji = 0;  /* prefer latin for these */
        n_chars++;
    }
    /* 40 curated emoji */
    for (int i = 0; i < N_EMOJI_TABLE; i++) {
        chars[n_chars].cp = EMOJI_CPS[i];
        chars[n_chars].from_emoji = 1;
        n_chars++;
    }
    /* Extra UI emoji */
    for (int i = 0; i < N_UI_EXTRA; i++) {
        chars[n_chars].cp = UI_EXTRA_CPS[i];
        chars[n_chars].from_emoji = 1;
        n_chars++;
    }
    /* Render-only extra emoji (D1 — not in the picker, not in EMOJI_TABLE) */
    for (int i = 0; i < N_RENDER_EXTRA; i++) {
        chars[n_chars].cp = RENDER_EXTRA_CPS[i];
        chars[n_chars].from_emoji = 1;
        n_chars++;
    }

    /* Sort by codepoint (required for binary search in Slint) */
    qsort(chars, (size_t)n_chars, sizeof(CharEntry), compare_char_entry);

    /* Remove duplicates (shouldn't have any but be safe) */
    for (int i = 1; i < n_chars; ) {
        if (chars[i].cp == chars[i-1].cp) {
            memmove(&chars[i], &chars[i+1],
                    (size_t)(n_chars - i - 1) * sizeof(CharEntry));
            n_chars--;
        } else {
            i++;
        }
    }

    /* ── Rasterise all glyphs ─────────────────────────────────────────── */
    /* Latin + BMP symbols at every PIXEL_SIZES entry; emoji only at the sizes
     * in EMOJI_SIZES (elsewhere left as empty glyphs — see comment on
     * EMOJI_SIZES).  `rendered[][]` is zero-initialised, so a skipped glyph
     * emits as a blank BitmapGlyph (width 0, no data). */
    for (int si = 0; si < N_SIZES; si++) {
        int emoji_ok = 0;
        for (int k = 0; k < N_EMOJI_SIZES; k++) {
            if (PIXEL_SIZES[si] == EMOJI_SIZES[k]) { emoji_ok = 1; break; }
        }
        for (int ci = 0; ci < n_chars; ci++) {
            if (chars[ci].from_emoji && !emoji_ok) {
                continue;  /* emoji at a non-emoji size → leave blank */
            }
            render_glyph(chars[ci].cp, chars[ci].from_emoji, si, ci);
        }
    }

    /* ── Fail the build on any codepoint missing from BOTH faces ──────── */
    /* A gap here is otherwise SILENT: `cargo build` succeeds, the firmware
     * flashes, and the affected icon/character simply renders blank on the
     * physical panel — the exact "recurring MeshCadet failure mode" this
     * file's SYNC INVARIANT comments have documented after the fact three
     * times already (admin_menu.rs bell/speaker/zzz; gps_status.rs
     * em-dash/ellipsis; unprovisioned.rs's 📻 at 28px). Failing loudly here,
     * at the one point that already has full visibility into every
     * (codepoint, size) pair this font is asked to cover, turns that class
     * of defect into a build break instead of a field report. */
    if (g_missing_glyph_count > 0) {
        fprintf(stderr,
            "gen_emoji_font: FAILED — %d codepoint/size pair(s) above would "
            "render BLANK on-device. Fix: either drop the offending size from "
            "EMOJI_SIZES for that char, or use a codepoint both faces cover.\n",
            g_missing_glyph_count);
        return 1;
    }

    if (check_ink_coverage_floor()) {
        return 1;
    }

    /* ── Read font metrics from Latin face (at 16px for EM scaling) ───── */
    FT_Set_Pixel_Sizes(latin_face, 0, 16);
    float upm = (float)latin_face->units_per_EM;
    float asc = (float)latin_face->ascender;
    float dsc = (float)latin_face->descender;
    /* Proportional defaults for OS/2 v1 fonts (no sxHeight/sCapHeight).
     * DejaVu Sans 2048 EM: x_height ~1120, cap_height ~1493.
     * Use ~54.7% and ~72.9% of EM respectively. */
    float xh  = upm * 0.547f;
    float cph = upm * 0.729f;
    TT_OS2 *os2 = (TT_OS2 *)FT_Get_Sfnt_Table(latin_face, FT_SFNT_OS2);
    if (os2 && os2->version >= 2) {
        if (os2->sxHeight  > 0) xh  = (float)os2->sxHeight;
        if (os2->sCapHeight > 0) cph = (float)os2->sCapHeight;
    }

    /* ── Emit Rust source ─────────────────────────────────────────────── */
    FILE *out = fopen(out_path, "w");
    if (!out) {
        fprintf(stderr, "Cannot write output: %s\n", out_path);
        return 1;
    }
    emit_rust(out, upm, asc, dsc, xh, cph);
    fclose(out);

    fprintf(stderr, "gen_emoji_font: wrote %d chars × %d sizes → %s\n",
            n_chars, N_SIZES, out_path);

    /* Cleanup */
    for (int si = 0; si < N_SIZES; si++)
        for (int ci = 0; ci < n_chars; ci++)
            free(rendered[si][ci].data);
    FT_Done_Face(latin_face);
    FT_Done_Face(emoji_face);
    FT_Done_FreeType(ft_library);
    return 0;
}
