// SPDX-License-Identifier: GPL-3.0-only
//! Runtime decode of the build-time-generated color-emoji picker-cell
//! rasters (`build_emoji_color.rs`) into `slint::Image`s.
//!
//! Only `ui/screens/compose.rs`'s `EmojiPickerGrid` cells use this path —
//! every other emoji call site (message bodies, the picker's own toggle
//! button, the draft field) keeps rendering through `platform.rs`'s bitmap
//! font, unchanged — color is only reachable through `Image` elements, and
//! the picker grid is the one place that swap is cheap (see
//! `build_emoji_color.rs`'s doc for the full rationale).

include!(concat!(env!("OUT_DIR"), "/emoji_color.rs"));

use std::cell::RefCell;
use std::collections::HashMap;

/// Returns the color `slint::Image` for `codepoint`.
///
/// # Panics
/// If `codepoint` isn't one of the 96 curated `protocol::emoji::EMOJI_TABLE`
/// entries `build_emoji_color.rs` covers. Every call site here
/// (`cells_for_category` in `ui/screens/compose.rs`) only ever passes an
/// `EmojiEntry::codepoint` drawn straight from that same table, and the
/// build-time generator hard-fails if any such codepoint lacks a raster —
/// so reaching this panic at runtime would mean the two have drifted apart
/// after compilation, which should be impossible; panicking loudly here
/// matches `ComposeScreen::new`'s existing "index straight into
/// `EMOJI_CATEGORIES`, panic rather than silently mis-wire" convention just
/// above it.
pub fn image_for_codepoint(codepoint: char) -> slint::Image {
    thread_local! {
        static CACHE: RefCell<HashMap<char, slint::Image>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        if let Some(img) = cache.borrow().get(&codepoint) {
            return img.clone();
        }
        let glyph = EMOJI_COLOR_GLYPHS
            .iter()
            .find(|g| g.codepoint == codepoint)
            .unwrap_or_else(|| {
                panic!(
                    "no color raster for {codepoint:?} — build_emoji_color.rs should have \
                     covered every EMOJI_TABLE codepoint or failed the build"
                )
            });
        let image = decode(glyph);
        cache.borrow_mut().insert(codepoint, image.clone());
        image
    })
}

/// Unpacks one glyph's palette-indexed nibble buffer into an RGBA8
/// `slint::Image`. `TRANSPARENT_NIBBLE` decodes to alpha 0 (binary-alpha
/// cutout — no partial-alpha values are ever stored, see
/// `build_emoji_color.rs`'s doc); every other nibble looks up the glyph's
/// own 15-entry RGB565 palette and expands it to RGB888 (the standard
/// 5/6/5 -> 8/8/8 bit-replication expansion, so a maxed channel round-trips
/// to 255, not 248/252).
fn decode(glyph: &EmojiColorGlyph) -> slint::Image {
    let mut buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(EMOJI_CELL_PX, EMOJI_CELL_PX);
    let pixels = buffer.make_mut_slice();
    for (i, &byte) in glyph.indices.iter().enumerate() {
        for (half, nibble) in [byte & 0x0F, byte >> 4].into_iter().enumerate() {
            let pixel_i = i * 2 + half;
            pixels[pixel_i] = if nibble == TRANSPARENT_NIBBLE {
                slint::Rgba8Pixel {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 0,
                }
            } else {
                let rgb565 = glyph.palette[nibble as usize];
                let r5 = (rgb565 >> 11) & 0x1F;
                let g6 = (rgb565 >> 5) & 0x3F;
                let b5 = rgb565 & 0x1F;
                slint::Rgba8Pixel {
                    r: ((r5 << 3) | (r5 >> 2)) as u8,
                    g: ((g6 << 2) | (g6 >> 4)) as u8,
                    b: ((b5 << 3) | (b5 >> 2)) as u8,
                    a: 255,
                }
            };
        }
    }
    slint::Image::from_rgba8(buffer)
}
