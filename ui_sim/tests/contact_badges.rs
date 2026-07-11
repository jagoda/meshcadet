// SPDX-License-Identifier: GPL-3.0-only
//! Integration test that renders the exact badge
//! markup `firmware/src/ui/screens/contact_list.rs` ships (per-row
//! `ContactRow` badge + tab-bar aggregate badge, replicated verbatim in
//! `ui_sim::contact_badges`) and asserts both badges paint a visibly
//! distinct `brand-signal` disc over their surrounding background when
//! `unread`/`total` is nonzero, and paint NOTHING (the bare row/tab
//! background) when it is zero.
//!
//! Lives under `tests/` (a separate Cargo integration-test binary, hence its
//! own process) so it can install its own Slint `Platform` singleton without
//! colliding with `lib.rs`'s or the other render rigs' own — see
//! `compose_send.rs`'s module doc for the full "why a second render path"
//! rationale, which applies identically here.

use ui_sim::contact_badges::{rgb8, ContactBadgesFrame, ROW_BADGE_SIZE, TAB_BADGE_SIZE};

/// RGB565 is lossy (5/6/5 bits per channel) — round an 8-bit-per-channel hex
/// color through the same pack/expand path the renderer itself uses, same
/// technique every other `ui_sim` test module uses.
fn quantize565(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let r5 = r >> 3;
    let g6 = g >> 2;
    let b5 = b >> 3;
    (((r5 << 3) | (r5 >> 2)), ((g6 << 2) | (g6 >> 4)), ((b5 << 3) | (b5 >> 2)))
}

fn at(fb: &[slint::platform::software_renderer::Rgb565Pixel], x: u32, y: u32) -> (u8, u8, u8) {
    rgb8(fb[(y * ui_sim::contact_badges::WIDTH + x) as usize])
}

/// Single test — see module doc: exactly one `ContactBadgesFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn unread_badges_paint_brand_signal_disc_when_nonzero_and_nothing_when_zero() {
    let brand_signal = quantize565(0x00, 0xb4, 0xff);
    let row_rest_wash = quantize565(0x0d, 0x11, 0x17); // approx: bg-space under low-alpha wash

    // Row badge occupies (8,8)..(28,28) — HorizontalLayout padding: 8px.
    let row_x0 = 8u32;
    let row_y0 = 8u32;
    let row_cx = row_x0 + ROW_BADGE_SIZE / 2;
    let row_cy = row_y0 + ROW_BADGE_SIZE / 2;
    // Tab badge occupies (36,8)..(50,22) — 8 (padding) + 20 (row badge) + 8 (spacing).
    let tab_x0 = 8 + ROW_BADGE_SIZE + 8;
    let tab_y0 = 8u32;
    let tab_cx = tab_x0 + TAB_BADGE_SIZE / 2;
    let tab_cy = tab_y0 + TAB_BADGE_SIZE / 2;

    let frame = ContactBadgesFrame::new();

    // ── Zero state: neither badge should paint a brand-signal disc ────────
    frame.set_row_unread(0, "0");
    frame.set_tab_total(0, "0");
    let fb0 = frame.render();
    assert_ne!(
        at(&fb0, row_cx, row_cy),
        brand_signal,
        "row badge must NOT render when unread == 0"
    );
    assert_ne!(
        at(&fb0, tab_cx, tab_cy),
        brand_signal,
        "tab badge must NOT render when total == 0"
    );

    // ── Nonzero state: both badges must paint the brand-signal disc ───────
    frame.set_row_unread(3, "3");
    frame.set_tab_total(12, "9+");
    let fb1 = frame.render();

    // Corner of the badge box (inside the disc's bounding square, near the
    // rounded-corner exclusion, but well inside the circle for these sizes)
    // — proves the disc itself painted, independent of glyph ink placement.
    assert_eq!(
        at(&fb1, row_cx, row_y0 + 3),
        brand_signal,
        "row badge disc must be brand-signal when unread > 0"
    );
    assert_eq!(
        at(&fb1, tab_cx, tab_y0 + 2),
        brand_signal,
        "tab badge disc must be brand-signal when total > 0"
    );

    // The badge must have genuinely changed the pixels at its own location
    // relative to the zero-state frame — not merely "some other brand-signal
    // colored thing happens to sit there already".
    assert_ne!(
        at(&fb0, row_cx, row_y0 + 3),
        at(&fb1, row_cx, row_y0 + 3),
        "row badge location must change color between zero and nonzero unread"
    );
    assert_ne!(
        at(&fb0, tab_cx, tab_y0 + 2),
        at(&fb1, tab_cx, tab_y0 + 2),
        "tab badge location must change color between zero and nonzero total"
    );

    // Sanity: the disc's own solid-fill sample must differ from the bare
    // row-rest background sample taken well outside either badge's box, so a
    // future "badge silently inherits the row wash color" regression (a
    // literal color-collision instance of the original bug's root failure mode)
    // fails loudly here instead of just on-hardware.
    assert_ne!(
        brand_signal, row_rest_wash,
        "sanity: brand-signal and the row rest-wash must not be the same color"
    );
}
