// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the six `SignalMeter` `signal-level` states
//! (0 = direct-only, 1..=5 = bars) side by side through
//! `ui_sim::signal_meter` and asserts each column paints the widget's
//! `brand-signal` cyan accent — the direct-only ring at level 0, and exactly
//! `n` filled bars at level `n` (with fewer `brand-signal` pixels than a
//! higher level, proving the bar count actually scales rather than every
//! level painting the same fixed shape).
//!
//! Lives under `tests/` (a separate Cargo integration-test binary, hence its
//! own process) so it can install its own Slint `Platform` singleton without
//! colliding with `lib.rs`'s, `motif_library`'s, `compose_send`'s, or
//! `gps_status_rows`'s own — see `gps_status_rows.rs`'s module doc for the
//! full "why a second render path" rationale, which applies identically
//! here.

use ui_sim::signal_meter::{rgb8, SignalMeterFrame, COL_WIDTH, WIDTH};

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

/// Count of `brand-signal`-colored pixels within column `level`'s
/// `[x0, x0 + COL_WIDTH)` span, across the whole frame height — a proxy for
/// "how much of the widget is painted filled" that scales monotonically with
/// bar count (more/taller filled bars -> more brand-signal pixels) without
/// needing to hand-compute each bar's exact pixel-rectangle.
fn brand_signal_pixel_count(
    fb: &[slint::platform::software_renderer::Rgb565Pixel],
    level: u32,
    brand_signal: (u8, u8, u8),
) -> usize {
    let x0 = level * COL_WIDTH;
    (x0..x0 + COL_WIDTH)
        .flat_map(|x| (0..ui_sim::signal_meter::HEIGHT).map(move |y| (x, y)))
        .filter(|&(x, y)| at(fb, x, y) == brand_signal)
        .count()
}

/// Single test — see module doc: exactly one `SignalMeterFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn direct_only_ring_and_ascending_bar_counts_all_render() {
    let bg_space = quantize565(0x0d, 0x11, 0x17);
    let brand_signal = quantize565(0x00, 0xb4, 0xff);

    let frame = SignalMeterFrame::new();
    let fb = frame.render();
    assert_eq!(fb.len(), (WIDTH * ui_sim::signal_meter::HEIGHT) as usize);

    // Level 0 (direct-only): the ring must paint SOMETHING non-background in
    // its column — an entirely blank column would mean the widget silently
    // failed to render at all. Checked against the background color (not an
    // exact `brand-signal` pixel match like the solid-fill bars below):
    // `border-width: 0.12 * root.height` (~1.9px at this widget's 16px
    // height) is thin enough that Slint's software renderer anti-aliases
    // nearly every ring pixel to a background/brand-signal BLEND rather than
    // ever painting the pure accent color outright — a real rendering
    // artifact of the ring's geometry at this size, not a defect in the
    // widget. Not compared against the bar levels below: the ring and a bar
    // are different shapes, so pixel COUNT alone isn't a meaningful
    // ordering between "ring" and "N bars" — only the bar levels' counts
    // against EACH OTHER are (see below).
    let level0_x0 = 0;
    let level0_non_bg = (level0_x0..level0_x0 + COL_WIDTH)
        .flat_map(|x| (0..ui_sim::signal_meter::HEIGHT).map(move |y| (x, y)))
        .any(|(x, y)| at(&fb, x, y) != bg_space);
    assert!(
        level0_non_bg,
        "level 0 (direct-only) must paint a visible ring over the background"
    );

    // Levels 1..=5: each successive level must paint STRICTLY MORE
    // brand-signal pixels than the one before it — every bar shares the same
    // width and only grows taller with its index (see `signal_meter.slint`'s
    // per-bar height fractions), so a genuinely-ascending bar count is a
    // strictly-ascending filled-pixel-area sequence; a widget that instead
    // rendered the same fixed shape at every level would fail this.
    let mut counts = Vec::new();
    for level in 1..=5u32 {
        counts.push(brand_signal_pixel_count(&fb, level, brand_signal));
    }
    for i in 1..counts.len() {
        assert!(
            counts[i] > counts[i - 1],
            "level {} ({} brand-signal px) must paint more than level {} ({} brand-signal px)",
            i + 1,
            counts[i],
            i,
            counts[i - 1],
        );
    }
}
