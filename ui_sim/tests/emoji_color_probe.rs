// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders three fixed-UI-emoji stand-in glyphs through
//! `ui_sim::emoji_color_probe`'s hand-built minimal `BitmapFont` (see that
//! module's doc for the full mechanism) and asserts each one paints in its
//! OWN intended `Theme` color — this mission's (`meshcadet-emoji-mono-
//! glyph-legibility`) `ui_sim` color-composition acceptance predicate.
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process) —
//! see `emoji_blank_cell_probe.rs`'s module doc for the full "why a second
//! render path" rationale, which applies identically here. Both cases below
//! (full-alpha, half-alpha) run on ONE `EmojiColorProbeFrame` within a
//! SINGLE `#[test]` fn, since Slint enforces one `Platform` per process and
//! this file's process hosts exactly one test function — same convention
//! `emoji_blank_cell_probe.rs`'s own test module doc explains.

use ui_sim::emoji_color_probe::{framebuffer_to_rgb_image, EmojiColorProbeFrame, HEIGHT};

/// RGB565 is lossy (5/6/5 bits per channel) — round an 8-bit-per-channel hex
/// color through the same pack/expand path the renderer itself uses, same
/// technique every other `ui_sim` test module uses.
fn quantize565(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let r5 = r >> 3;
    let g6 = g >> 2;
    let b5 = b >> 3;
    (
        ((r5 << 3) | (r5 >> 2)),
        ((g6 << 2) | (g6 >> 4)),
        ((b5 << 3) | (b5 >> 2)),
    )
}

fn rgb8_at(img: &image::RgbImage, x: u32, y: u32) -> (u8, u8, u8) {
    let px = img.get_pixel(x, y);
    (px[0], px[1], px[2])
}

#[test]
fn each_fixed_emoji_site_paints_its_own_intended_color() {
    // Theme.warn (pin_entry.rs's 🔐), Theme.ok (gps_status.rs's 📍),
    // Theme.star-gold (compose.rs's 😀 picker toggle) — see theme.slint.
    let warn = quantize565(0xff, 0xff, 0x00);
    let ok = quantize565(0x00, 0xff, 0x00);
    let star_gold = quantize565(0xff, 0xd6, 0x6b);

    let frame = EmojiColorProbeFrame::new();

    // ── Full alpha (0xFF) at all three positions ────────────────────────
    frame.set_glyphs("F", "F", "F");
    let fb = frame.render();
    let img = framebuffer_to_rgb_image(&fb, ui_sim::emoji_color_probe::WIDTH, HEIGHT);

    // One representative pixel per glyph's 16x16 cell (glyphs are uniform
    // fill, so any interior pixel is representative).
    assert_eq!(
        rgb8_at(&img, 4, 8),
        warn,
        "lock-site glyph (x=[0,16)) must paint Theme.warn, not some other/shared color"
    );
    assert_eq!(
        rgb8_at(&img, 36, 8),
        ok,
        "gps-site glyph (x=[32,48)) must paint Theme.ok"
    );
    assert_eq!(
        rgb8_at(&img, 68, 8),
        star_gold,
        "picker-site glyph (x=[64,80)) must paint Theme.star-gold"
    );

    // The whole point of per-`Text`-element coloring: three DIFFERENT
    // colors from ONE globally-registered font, not one shared tint.
    assert_ne!(warn, ok);
    assert_ne!(ok, star_gold);
    assert_ne!(warn, star_gold);

    // ── Half alpha (0x80) at the lock position ──────────────────────────
    // Stand-in for an antialiased edge, or a pre-gamma-boost washed-out
    // pixel — over the probe's pure-black background this reduces to
    // color * (alpha/255), same premultiplied-over-black arithmetic the
    // real SoftwareRenderer performs. Proves the per-`Text`-element
    // coloring mechanism holds at partial alpha too, not only at the
    // fully-opaque case above.
    let half_warn = quantize565(
        (0xffu32 * 0x80 / 0xff) as u8,
        (0xffu32 * 0x80 / 0xff) as u8,
        0,
    );
    frame.set_glyphs("H", "F", "F");
    let fb2 = frame.render();
    let img2 = framebuffer_to_rgb_image(&fb2, ui_sim::emoji_color_probe::WIDTH, HEIGHT);
    assert_eq!(
        rgb8_at(&img2, 4, 8),
        half_warn,
        "half-alpha lock-site glyph must still composite against Theme.warn, \
         dimmed proportionally to its own alpha — not some other color"
    );
}
