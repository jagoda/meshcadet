// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the promotional splash screenshot rig
//! (`ui_sim::splash_promo`, a verbatim copy of `splash.rs`'s full markup)
//! and asserts the screen's "static-complete" first frame — see
//! `splash.rs`'s own module doc — actually paints content in each of its
//! three static bands (radio glyph + mascot, "MeshCadet" wordmark, version
//! string), all already fully opaque with no animation needed. A
//! regression guard against the copied markup silently drifting from the
//! real screen.
//!
//! Bands are scanned for "differs from the bare background", not an exact
//! glyph color match — small/anti-aliased text blends toward the backdrop
//! at the pixel level, so an exact RGB565-quantized color match is too
//! brittle (same reasoning `splash_lineart.rs`'s own test already applies
//! to this exact screen's line art).
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process) —
//! see `compose_send.rs`'s module doc for the full "why a second render
//! path" rationale, which applies identically here.

use ui_sim::splash_promo::{framebuffer_to_rgb_image, SplashPromoFrame};

fn rgb8_at(img: &image::RgbImage, x: u32, y: u32) -> (u8, u8, u8) {
    let px = img.get_pixel(x, y);
    (px[0], px[1], px[2])
}

/// Assert at least one pixel in `y_range` differs from `bg` — i.e. some
/// layer painted something in that row band.
fn assert_band_painted(
    img: &image::RgbImage,
    y_range: std::ops::Range<u32>,
    bg: (u8, u8, u8),
    what: &str,
) {
    let mut painted = false;
    'outer: for y in y_range.clone() {
        for x in 0..ui_sim::splash_promo::WIDTH {
            if rgb8_at(img, x, y) != bg {
                painted = true;
                break 'outer;
            }
        }
    }
    assert!(
        painted,
        "{what} did not paint anything in y={}..{}",
        y_range.start, y_range.end
    );
}

/// Single test — see module doc: exactly one `SplashPromoFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn static_complete_frame_paints_logo_wordmark_and_version() {
    let frame = SplashPromoFrame::new();
    frame.set_version("v0.2.0");
    let fb = frame.render();
    let img = framebuffer_to_rgb_image(
        &fb,
        ui_sim::splash_promo::WIDTH,
        ui_sim::splash_promo::HEIGHT,
    );

    let bg = rgb8_at(&img, 0, 0); // top-left corner: bare bg-space + backdrop's low-density edge

    // Radio glyph + Cadet mascot, in `logo_area` (0..96, vertically
    // centered within the window per the outer `VerticalLayout`'s
    // `alignment: center` — measured empirically against this rig at
    // y=73..111).
    assert_band_painted(&img, 73..111, bg, "logo_area (radio glyph + mascot)");

    // "MeshCadet" wordmark (measured at y=153..170).
    assert_band_painted(&img, 153..170, bg, "\"MeshCadet\" wordmark");

    // Version string (measured at y=185..193).
    assert_band_painted(&img, 185..193, bg, "version string");
}
