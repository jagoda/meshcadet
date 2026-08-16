// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the six `BatteryIndicator` `battery-level`
//! states (0 = Unknown outline, 1 = Charging, 2..=5 =
//! Critical/Low/Medium/High) side by side through `ui_sim::battery_indicator`
//! and asserts each column paints the widget's own distinct signature:
//! Unknown paints an outline (non-background) with no bucket color at all;
//! Charging paints the `brand-signal` cyan accent, exclusive of every
//! percent-bucket color; Critical/Low/Medium/High each paint their own
//! `alert`/`warn`/`ok`/`ok` fill color, exclusive of every OTHER level's
//! color, and Medium->High strictly increases the `ok`-colored pixel count
//! (both share `ok` green — proving the fill fraction actually scales
//! rather than every bucket painting the same fixed shape).
//!
//! This is the acceptance-line probe for the new widget, mirroring
//! `ui_sim/tests/signal_meter.rs`'s own shape exactly — see this crate's
//! `battery_indicator` module doc for why the render path is separate.
//!
//! Lives under `tests/` (a separate Cargo integration-test binary, hence its
//! own process) so it can install its own Slint `Platform` singleton without
//! colliding with any other `ui_sim` render rig's own — see
//! `gps_status_rows.rs`'s module doc for the full "why a second render path"
//! rationale, which applies identically here.

use ui_sim::battery_indicator::{rgb8, BatteryIndicatorFrame, COL_WIDTH, WIDTH};

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

fn at(fb: &[slint::platform::software_renderer::Rgb565Pixel], x: u32, y: u32) -> (u8, u8, u8) {
    rgb8(fb[(y * WIDTH + x) as usize])
}

/// Count of `color`-matching pixels within column `level`'s
/// `[x0, x0 + COL_WIDTH)` span, across the whole frame height.
fn color_pixel_count(
    fb: &[slint::platform::software_renderer::Rgb565Pixel],
    level: u32,
    color: (u8, u8, u8),
) -> usize {
    let x0 = level * COL_WIDTH;
    (x0..x0 + COL_WIDTH)
        .flat_map(|x| (0..ui_sim::battery_indicator::HEIGHT).map(move |y| (x, y)))
        .filter(|&(x, y)| at(fb, x, y) == color)
        .count()
}

/// Single test — see module doc: exactly one `BatteryIndicatorFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn every_battery_level_renders_its_own_distinct_signature() {
    let bg_space = quantize565(0x0d, 0x11, 0x17);
    let brand_signal = quantize565(0x00, 0xb4, 0xff);
    let alert = quantize565(0xff, 0x00, 0x00);
    let warn = quantize565(0xff, 0xff, 0x00);
    let ok = quantize565(0x00, 0xff, 0x00);
    let all_bucket_colors = [brand_signal, alert, warn, ok];

    let frame = BatteryIndicatorFrame::new();
    let fb = frame.render();
    assert_eq!(
        fb.len(),
        (WIDTH * ui_sim::battery_indicator::HEIGHT) as usize
    );

    // Level 0 (Unknown): the outline shell must paint SOMETHING non-background
    // in its column (proves the widget actually rendered, not a blank/failed
    // widget) — but must NOT paint any of the four bucket-fill colors, since
    // "no reading yet" must never be visually confused with a real bucket.
    let level0_non_bg = (0..COL_WIDTH)
        .flat_map(|x| (0..ui_sim::battery_indicator::HEIGHT).map(move |y| (x, y)))
        .any(|(x, y)| at(&fb, x, y) != bg_space);
    assert!(
        level0_non_bg,
        "level 0 (Unknown) must paint a visible outline shell over the background"
    );
    for &color in &all_bucket_colors {
        assert_eq!(
            color_pixel_count(&fb, 0, color),
            0,
            "level 0 (Unknown) must not paint any bucket-fill color — no reading yet"
        );
    }

    // Level 1 (Charging): must paint brand-signal, and NONE of the three
    // percent-bucket colors — charging is a distinct MODE, not a percent
    // reading, so it must never be visually confused with Critical/Low/
    // Medium/High.
    assert!(
        color_pixel_count(&fb, 1, brand_signal) > 0,
        "level 1 (Charging) must paint the brand-signal cyan fill"
    );
    for &color in &[alert, warn, ok] {
        assert_eq!(
            color_pixel_count(&fb, 1, color),
            0,
            "level 1 (Charging) must not paint any percent-bucket color"
        );
    }

    // Levels 2..=5 (Critical/Low/Medium/High): each must paint its OWN
    // expected color, and NONE of the other three colors from this set (incl.
    // brand-signal) — resolving each bucket to a genuinely distinct signature,
    // not an adjacent bucket's color bleeding through.
    let expected = [(2u32, alert), (3u32, warn), (4u32, ok), (5u32, ok)];
    for &(level, color) in &expected {
        assert!(
            color_pixel_count(&fb, level, color) > 0,
            "level {level} must paint its expected bucket-fill color"
        );
    }
    // Critical (2) and Low (3) each get their own color exclusively — must
    // not paint brand-signal, or the OTHER of {alert, warn}.
    assert_eq!(color_pixel_count(&fb, 2, warn), 0);
    assert_eq!(color_pixel_count(&fb, 2, brand_signal), 0);
    assert_eq!(color_pixel_count(&fb, 3, alert), 0);
    assert_eq!(color_pixel_count(&fb, 3, brand_signal), 0);
    assert_eq!(color_pixel_count(&fb, 4, brand_signal), 0);
    assert_eq!(color_pixel_count(&fb, 4, alert), 0);
    assert_eq!(color_pixel_count(&fb, 4, warn), 0);
    assert_eq!(color_pixel_count(&fb, 5, brand_signal), 0);
    assert_eq!(color_pixel_count(&fb, 5, alert), 0);
    assert_eq!(color_pixel_count(&fb, 5, warn), 0);

    // Medium (4) and High (5) share the same `ok` green fill color, so their
    // pixel COUNTS are directly comparable — High (100% fill) must paint
    // STRICTLY MORE `ok` pixels than Medium (76% fill), proving the fill
    // fraction actually scales with bucket rather than both painting the
    // same fixed shape.
    let medium_ok = color_pixel_count(&fb, 4, ok);
    let high_ok = color_pixel_count(&fb, 5, ok);
    assert!(
        high_ok > medium_ok,
        "level 5 (High, {high_ok} ok px) must paint more fill than level 4 (Medium, {medium_ok} ok px)"
    );
}
