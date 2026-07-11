// SPDX-License-Identifier: GPL-3.0-only
/*
 * gen_emoji_font.c — Build-time bitmap font generator for MeshCadet emoji.
 *
 * Rasterises a combined "MeshCadetEmoji" font that includes:
 *   - Printable ASCII (U+0020..U+007E) from a Latin TTF (DejaVu Sans)
 *   - A small set of BMP symbols used by the UI (‹ › ✏ ✓ ✕ ⚙ ⌫ −) from the
 *     Latin font (with emoji-face fallback for any the Latin font lacks)
 *   - 40 curated picker emoji + 6 UI-chrome emoji (📤 😀 📬 📡 🔐 📍) from
 *     NotoEmoji-Regular.ttf
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
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <ft2build.h>
#include FT_FREETYPE_H
#include FT_SFNT_NAMES_H
#include FT_TRUETYPE_TABLES_H

/* ── Curated emoji codepoints (40 total) ─────────────────────────────── */
/*
 * SYNC INVARIANT: these codepoints MUST match EMOJI_TABLE in
 * protocol/src/emoji.rs.  If emoji are added or removed from that table,
 * update this array AND the N_EMOJI_TABLE define below.  A divergence
 * compiles silently but the added emoji will render blank on-device.
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
    0x1F4EC,  /* 📬 mailbox       — "📬 Messages" tab (contact_list.rs, 13px)  */
    0x1F4E1,  /* 📡 satellite     — "📡 Channels" tab (contact_list.rs, 13px)  */
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
};
#define N_UI_EXTRA 10

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
};
#define N_BMP_SYMBOLS 10

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
 * emoji, so emoji are omitted there.  Rasterising the 42 emoji at every size —
 * especially 22/28 px (header chevrons / titles, never emoji) — wastes ~78 KB of
 * flash, so emoji are emitted as empty glyphs outside this set (they still hold a
 * char-map slot but carry no bitmap).  Latin + BMP symbols are rasterised at ALL
 * sizes above.
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
 * would rasterise all ~49 curated emoji at a 13th size for zero current
 * benefit (~KB of flash per the note above). The host glyph-coverage harness
 * (`xtask`) carries a matching, equally-narrow, equally-commented allowlist
 * entry for this one pair so it does not mask any OTHER future gap. */
static const int EMOJI_SIZES[] = {11, 13, 14, 16, 18, 20};
#define N_EMOJI_SIZES 6

/* ── Character table (sorted by codepoint for binary search) ─────────── */
#define N_ASCII 95  /* U+0020..U+007E */
#define N_MAX_CHARS (N_ASCII + N_BMP_SYMBOLS + N_EMOJI_TABLE + N_UI_EXTRA)

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
        /* Grayscale: copy w bytes per row */
        for (int row = 0; row < h; row++) {
            memcpy(g->data + row * w, bm->buffer + row * pitch, (size_t)w);
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
