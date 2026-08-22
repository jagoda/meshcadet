// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the five `BatteryIndicator` `battery-level`
//! states (0 = Unknown outline, 1 = Charging, 2..=4 = Low/Partial/Full) side
//! by side through `ui_sim::battery_indicator` and asserts each column
//! paints the widget's own distinct signature: Unknown paints an outline
//! (non-background) with no bucket color at all; Charging paints the
//! `brand-signal` cyan accent, exclusive of every bucket color; Low/Partial/
//! Full each paint their own `alert`/`warn`/`ok` fill color, exclusive of
//! every OTHER level's color, and Partial->Full strictly increases the
//! filled-pixel count (proving the fill fraction actually scales with
//! bucket rather than every level painting the same fixed shape).
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
    // widget) — but must NOT paint any of the bucket-fill colors, since
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

    // Level 1 (Charging): must paint brand-signal, and NONE of the
    // bucket colors — charging is a distinct MODE, not a voltage reading,
    // so it must never be visually confused with Low/Partial/Full.
    assert!(
        color_pixel_count(&fb, 1, brand_signal) > 0,
        "level 1 (Charging) must paint the brand-signal cyan fill"
    );
    for &color in &[alert, warn, ok] {
        assert_eq!(
            color_pixel_count(&fb, 1, color),
            0,
            "level 1 (Charging) must not paint any bucket color"
        );
    }

    // Levels 2..=4 (Low/Partial/Full): each must paint its OWN expected
    // color, and NONE of the other colors from this set (incl.
    // brand-signal) — resolving each bucket to a genuinely distinct
    // signature, not an adjacent bucket's color bleeding through.
    let expected = [(2u32, alert), (3u32, warn), (4u32, ok)];
    for &(level, color) in &expected {
        assert!(
            color_pixel_count(&fb, level, color) > 0,
            "level {level} must paint its expected bucket-fill color"
        );
    }
    // Low (2), Partial (3), Full (4) each get their own color exclusively.
    assert_eq!(color_pixel_count(&fb, 2, warn), 0);
    assert_eq!(color_pixel_count(&fb, 2, ok), 0);
    assert_eq!(color_pixel_count(&fb, 2, brand_signal), 0);
    assert_eq!(color_pixel_count(&fb, 3, alert), 0);
    assert_eq!(color_pixel_count(&fb, 3, ok), 0);
    assert_eq!(color_pixel_count(&fb, 3, brand_signal), 0);
    assert_eq!(color_pixel_count(&fb, 4, brand_signal), 0);
    assert_eq!(color_pixel_count(&fb, 4, alert), 0);
    assert_eq!(color_pixel_count(&fb, 4, warn), 0);

    // Low/Partial/Full each use a DIFFERENT fill color, but the fill
    // rectangle's geometry is identical across columns (same body/height),
    // so a solid color's pixel COUNT is directly proportional to its fill
    // fraction regardless of which color it is. Full (100% fill) must paint
    // strictly more `ok` pixels than Partial (70% fill) painted `warn`
    // pixels, which must in turn paint strictly more than Low (35% fill)
    // painted `alert` pixels — proving the fill fraction actually scales
    // with bucket rather than every level painting the same fixed shape.
    let low_fill = color_pixel_count(&fb, 2, alert);
    let partial_fill = color_pixel_count(&fb, 3, warn);
    let full_fill = color_pixel_count(&fb, 4, ok);
    assert!(
        partial_fill > low_fill,
        "level 3 (Partial, {partial_fill} px) must paint more fill than level 2 (Low, {low_fill} px)"
    );
    assert!(
        full_fill > partial_fill,
        "level 4 (Full, {full_fill} px) must paint more fill than level 3 (Partial, {partial_fill} px)"
    );
}
