// SPDX-License-Identifier: GPL-3.0-only
//! Build-time color-emoji raster asset generator for the emoji picker's
//! grid cells (`ui/screens/compose.rs`'s `EmojiPickerGrid`) — see
//! `meshcadet-emoji-picker-color-cells` mission.
//!
//! # Why the picker specifically, and why not `gen_emoji_font.c`
//!
//! Slint's software renderer has no per-glyph color path
//! (`i_slint_core::graphics::BitmapGlyph.data` is a single 8-bit alpha map
//! blended with the current text *color* property), so a bitmap font —
//! `gen_emoji_font.c`'s whole mechanism — structurally cannot carry color.
//! `Image` elements can. This is a SEPARATE pipeline from
//! `gen_emoji_font.c`'s alpha-mask font (untouched by this mission): same
//! "build-time asset generation feeding an `include!`-d `$OUT_DIR/*.rs`"
//! shape, different source font, different output format, and written in
//! Rust rather than C because there's no existing C dependency (FreeType's
//! CBDT decode) worth reaching for when `ttf-parser` already exposes the
//! embedded PNG-per-glyph strike directly.
//!
//! # Pipeline
//!
//! 1. Parse `assets/NotoColorEmoji.ttf` (OFL 1.1 — see
//!    `assets/NotoColorEmoji-LICENSE.txt`) with `ttf-parser`.
//! 2. For every `protocol::emoji::EMOJI_TABLE` entry (all 96 — this reads
//!    the REAL table directly, not a hand-duplicated codepoint list), look
//!    up its glyph and pull the embedded CBDT/CBLC raster
//!    (`Face::glyph_raster_image` — NotoColorEmoji.ttf embeds exactly one
//!    strike, a 136x128 PNG blob per glyph, regardless of the requested
//!    `pixels_per_em`).
//! 3. Decode the PNG (`image` crate) and downsample to [`CELL_PX`]x[`CELL_PX`]
//!    with a Lanczos3 filter.
//! 4. Quantize per-glyph to [`PALETTE_COLORS`] colors with
//!    `color_quant`'s NeuQuant, **sample factor 1 (exhaustive)** — a small
//!    20x20/400px image starves NeuQuant's histogram at any coarser sample
//!    factor (confirmed empirically: factor 10 collapsed a 6-band rainbow
//!    emoji to 2 visible colors; factor 1 recovered all 6). Exhaustive
//!    sampling is cheap at this size (96 glyphs x 400px, sub-second total).
//! 5. Pack each pixel into a 4-bit nibble: values `0..PALETTE_COLORS` index
//!    the glyph's own palette (RGB565, 2 B/entry); [`TRANSPARENT_NIBBLE`]
//!    (0xF) is a binary-alpha cutout sentinel — no partial-alpha blending is
//!    stored, matching the source art's mostly-hard edges at this size.
//!
//! Emits `pub struct EmojiColorGlyph` + `pub const EMOJI_COLOR_GLYPHS: &[..]`
//! to `out_rs`, `include!`-d by `src/ui/emoji_color.rs`, which does the
//! runtime nibble -> `slint::Image` decode (kept in `src/` rather than here
//! because it depends on `slint`, unavailable to a build script's own
//! compilation without a much heavier build-dependency chain than three
//! small host-tool crates).
//!
//! # Sizing
//! 96 glyphs x (15 colors x 2 B palette + 400px / 2 nibbles-per-byte) =
//! 96 x (30 + 200) = 22,080 B — well under the palette-indexed estimate this
//! pipeline was scoped against (roughly 4x cheaper than plain RGB565+alpha).

use std::path::Path;

use image::imageops::FilterType;

/// Target cell size — matches `EmojiPickerGrid`'s previous mono-glyph cells'
/// `font-size: Theme.icon-lg` (20px), so the swap to `Image` is a
/// same-footprint replacement, not a re-layout.
pub const CELL_PX: u32 = 20;

/// Real (non-transparent) colors in each glyph's own adaptive palette.
/// Deliberately one below the 16 a 4-bit nibble can address — see
/// [`TRANSPARENT_NIBBLE`].
pub const PALETTE_COLORS: usize = 15;

/// Nibble value reserved as "fully transparent" (binary alpha cutout).
/// Never a valid index into a glyph's `palette` array.
pub const TRANSPARENT_NIBBLE: u8 = 0xF;

/// Source pixels below this alpha are treated as fully transparent; at or
/// above, fully opaque and quantized. No partial-alpha value is ever stored.
const ALPHA_CUTOFF: u8 = 128;

fn rgb888_to_rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3)
}

/// Quantizes and packs one glyph's already-resized `CELL_PX`x`CELL_PX` RGBA
/// buffer into a (palette, packed-nibbles) pair.
fn quantize_glyph(resized: &image::RgbaImage) -> ([u16; PALETTE_COLORS], Vec<u8>) {
    let mut opaque_rgba: Vec<u8> = Vec::new();
    for px in resized.pixels() {
        if px.0[3] >= ALPHA_CUTOFF {
            opaque_rgba.extend_from_slice(&px.0);
        }
    }
    assert!(
        !opaque_rgba.is_empty(),
        "glyph rasterised fully transparent at {CELL_PX}px — no opaque pixels to quantize \
         (a curated picker entry should never hit this; investigate the source glyph)"
    );

    // Sample factor 1 (exhaustive) — see module doc for why a coarser factor
    // silently starves the histogram at this pixel count.
    let nq = color_quant::NeuQuant::new(1, PALETTE_COLORS, &opaque_rgba);
    let palette_rgba = nq.color_map_rgba();

    let mut palette = [0u16; PALETTE_COLORS];
    for (i, slot) in palette.iter_mut().enumerate() {
        let base = i * 4;
        *slot = rgb888_to_rgb565(
            palette_rgba[base],
            palette_rgba[base + 1],
            palette_rgba[base + 2],
        );
    }

    let mut nibbles = vec![0u8; (CELL_PX * CELL_PX) as usize];
    for (i, px) in resized.pixels().enumerate() {
        nibbles[i] = if px.0[3] < ALPHA_CUTOFF {
            TRANSPARENT_NIBBLE
        } else {
            nq.index_of(&px.0) as u8
        };
    }
    let mut packed = vec![0u8; nibbles.len() / 2];
    for (i, chunk) in nibbles.chunks_exact(2).enumerate() {
        packed[i] = chunk[0] | (chunk[1] << 4);
    }
    (palette, packed)
}

/// Runs the full pipeline and writes the generated Rust source to `out_rs`.
/// Panics loudly (build failure, never a silent blank cell) if
/// `font_path` fails to parse or any `protocol::emoji::EMOJI_TABLE`
/// codepoint has no glyph in it — the equivalent, for this pipeline, of
/// `gen_emoji_font.c`'s own `g_missing_glyph_count` hard-error.
pub fn generate(out_rs: &Path, font_path: &Path) {
    let font_data = std::fs::read(font_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", font_path.display()));
    let face = ttf_parser::Face::parse(&font_data, 0)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e:?}", font_path.display()));

    let mut out = String::new();
    out.push_str("// Generated by build_emoji_color.rs — do not edit by hand.\n\n");
    out.push_str("pub struct EmojiColorGlyph {\n");
    out.push_str("    pub codepoint: char,\n");
    out.push_str(&format!("    pub palette: [u16; {PALETTE_COLORS}],\n"));
    out.push_str(&format!(
        "    pub indices: [u8; {}],\n",
        (CELL_PX * CELL_PX / 2) as usize
    ));
    out.push_str("}\n\n");
    out.push_str(&format!("pub const EMOJI_CELL_PX: u32 = {CELL_PX};\n"));
    out.push_str(&format!(
        "pub const TRANSPARENT_NIBBLE: u8 = {TRANSPARENT_NIBBLE};\n\n"
    ));
    out.push_str("pub const EMOJI_COLOR_GLYPHS: &[EmojiColorGlyph] = &[\n");

    let mut missing: Vec<char> = Vec::new();

    for entry in protocol::emoji::EMOJI_TABLE {
        let cp = entry.codepoint;
        let raster = face
            .glyph_index(cp)
            .and_then(|gid| face.glyph_raster_image(gid, CELL_PX as u16));
        let Some(raster) = raster else {
            missing.push(cp);
            continue;
        };
        assert_eq!(
            raster.format,
            ttf_parser::RasterImageFormat::PNG,
            "unexpected raster format for {cp:?} (expected the PNG-per-glyph CBDT strike \
             NotoColorEmoji.ttf embeds)"
        );
        let decoded = image::load_from_memory(raster.data)
            .unwrap_or_else(|e| panic!("failed to decode embedded PNG for {cp:?}: {e}"))
            .to_rgba8();
        let resized = image::imageops::resize(&decoded, CELL_PX, CELL_PX, FilterType::Lanczos3);
        let (palette, packed) = quantize_glyph(&resized);

        out.push_str(&format!(
            "    EmojiColorGlyph {{ codepoint: {cp:?}, palette: [{}], indices: [{}] }},\n",
            palette
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            packed
                .iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }

    assert!(
        missing.is_empty(),
        "EMOJI_TABLE codepoints missing a NotoColorEmoji.ttf glyph: {missing:?} — every picker \
         entry must resolve to a color raster or it will render as an EMPTY cell on-device"
    );

    out.push_str("];\n");
    std::fs::write(out_rs, out)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", out_rs.display()));
}
