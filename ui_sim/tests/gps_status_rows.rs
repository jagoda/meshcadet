// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the
//! four-row stack through `ui_sim::gps_status_rows` and asserts the
//! `StatusRow.icon-kind` selector paints the right motif (or none) in each
//! row, and that the Time-sync row's second (`value2`) line — the GPS
//! status time-row-overflow fix — actually fits inside its own
//! `row-height: 60px` without bleeding past it.
//!
//! Lives under `tests/` (a separate Cargo integration-test binary, hence
//! its own process) so it can install its own Slint `Platform` singleton
//! without colliding with `lib.rs`'s, `motif_library.rs`'s, or
//! `compose_send.rs`'s own — see `motif_library.rs`'s module doc for the
//! full "why a second render path" rationale, which applies identically
//! here.

use ui_sim::gps_status_rows::{rgb8, GpsStatusRowsFrame, ROW_HEIGHT, TIME_SYNC_ROW_HEIGHT, WIDTH};

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

/// Whether `color` appears anywhere within the row spanning
/// `[y0, y0 + ROW_HEIGHT)`.
fn row_contains(
    fb: &[slint::platform::software_renderer::Rgb565Pixel],
    y0: u32,
    color: (u8, u8, u8),
) -> bool {
    (0..WIDTH).any(|x| (y0..y0 + ROW_HEIGHT).any(|y| at(fb, x, y) == color))
}

/// Whether ANY non-`bg` pixel appears within `[y0, y1)`.
fn range_has_non_bg(
    fb: &[slint::platform::software_renderer::Rgb565Pixel],
    y0: u32,
    y1: u32,
    bg: (u8, u8, u8),
) -> bool {
    (0..WIDTH).any(|x| (y0..y1).any(|y| at(fb, x, y) != bg))
}

/// Single test — see module doc: exactly one `GpsStatusRowsFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn icon_kind_selects_the_right_motif_per_row_and_leaves_iconless_rows_blank() {
    let bg_space = quantize565(0x0d, 0x11, 0x17);
    let planet_warm = quantize565(0xe0, 0x8a, 0x4c);
    let star_gold = quantize565(0xff, 0xd6, 0x6b);

    let frame = GpsStatusRowsFrame::new();
    let fb = frame.render();
    assert_eq!(fb.len(), (WIDTH * ui_sim::gps_status_rows::HEIGHT) as usize);

    // Rows stack top-to-bottom at y = 0, 48, 96 (see `GpsStatusRowsUi`).
    let row0_y0 = 0; // "Fix" — icon-kind: comet
    let row1_y0 = ROW_HEIGHT; // "Satellites" — icon-kind: none
    let row2_y0 = 2 * ROW_HEIGHT; // "Coordinates" — icon-kind: planet

    // Row 0 ("Fix"): comet head (star-gold) must appear somewhere in the row.
    assert!(
        row_contains(&fb, row0_y0, star_gold),
        "Fix row (icon-kind: comet) should show the comet's star-gold head"
    );
    // The planet motif must NOT bleed into the comet row.
    assert!(
        !row_contains(&fb, row0_y0, planet_warm),
        "Fix row (icon-kind: comet) must not show the planet motif"
    );

    // Row 1 ("Satellites", icon-kind: none): neither motif color may appear
    // anywhere in this row — no icon column is reserved at all.
    assert!(
        !row_contains(&fb, row1_y0, star_gold),
        "Satellites row (icon-kind: none) must not show the comet motif"
    );
    assert!(
        !row_contains(&fb, row1_y0, planet_warm),
        "Satellites row (icon-kind: none) must not show the planet motif"
    );
    // Sanity: the row isn't simply blank/unrendered — its label/value text
    // must have painted something other than the plain background.
    assert!(
        (0..WIDTH).any(|x| (row1_y0..row1_y0 + ROW_HEIGHT).any(|y| at(&fb, x, y) != bg_space)),
        "Satellites row should still render its label/value text"
    );

    // Row 2 ("Coordinates"): ringed-planet body (planet-warm) must appear
    // somewhere in the row.
    assert!(
        row_contains(&fb, row2_y0, planet_warm),
        "Coordinates row (icon-kind: planet) should show the ringed planet's body"
    );
    // The comet motif must NOT bleed into the planet row.
    assert!(
        !row_contains(&fb, row2_y0, star_gold),
        "Coordinates row (icon-kind: planet) must not show the comet motif"
    );

    // Row 3 ("Time sync", row-height: 60px): the GPS status time-row-
    // overflow fix. Three lines (label / absolute date+time / relative
    // age) must all fit inside this row's own 60px allocation — nothing
    // may bleed past it into the window's remaining, otherwise-untouched
    // background below. (This render has no header, unlike the real
    // screen — `firmware/src/ui/screens/gps_status.rs`'s own comment on the
    // `row-height: 60px` binding is where the "36 (header) + 48*3 + 60 ==
    // 240" whole-screen arithmetic is asserted; this test proves the
    // narrower, portable claim: the row's OWN content fits its OWN height.)
    let row3_y0 = 3 * ROW_HEIGHT;
    let row3_y1 = row3_y0 + TIME_SYNC_ROW_HEIGHT;
    assert!(
        row3_y1 <= ui_sim::gps_status_rows::HEIGHT,
        "Time sync row must fit inside this render's window"
    );
    // Sanity: the row renders at all (label + both value lines painted
    // something other than plain background).
    assert!(
        range_has_non_bg(&fb, row3_y0, row3_y1, bg_space),
        "Time sync row should render its label/value/value2 text"
    );
    // The `value2` line (relative age) sits near the BOTTOM of the row,
    // below the label + primary value lines — assert content reaches that
    // far down, proving the second line actually painted, not just the
    // first two.
    assert!(
        range_has_non_bg(&fb, row3_y1 - 12, row3_y1, bg_space),
        "Time sync row's value2 (relative-age) line should paint near the row's bottom"
    );
    // The regression guard proper: nothing from this row (or anything else
    // in this render) may bleed past its own 60px allocation into the
    // window's remaining background — this is the literal "row overflow"
    // failure mode the fix addresses, now caught mechanically instead of
    // by eyeballing a screenshot.
    assert!(
        !range_has_non_bg(&fb, row3_y1, ui_sim::gps_status_rows::HEIGHT, bg_space),
        "Time sync row's content must not bleed past its own row-height into the window below it"
    );
}
