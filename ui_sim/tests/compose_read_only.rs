// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the read-only compose indicator markup through
//! `ui_sim::compose_read_only_promo` and asserts the `meshcadet-room-
//! firmware-post-and-notify` Phase B acceptance bullet — "A `GUEST`-
//! permission session renders compose disabled with a read-only indicator —
//! asserted by a `ui_sim` render test, not by inspection."
//!
//! Lives under `tests/` (a separate Cargo integration-test binary, hence its
//! own process) so it can install its own Slint `Platform` singleton without
//! colliding with any other render rig's — see `compose_send.rs`'s module
//! doc for the full "why a second render path" rationale, which applies
//! identically here.

use ui_sim::compose_read_only_promo::{
    rgb8, ComposeReadOnlyFrame, BANNER_H, BANNER_X, BANNER_Y, HEIGHT, WIDTH,
};

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

/// Single test — see module doc: exactly one `ComposeReadOnlyFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn read_only_room_renders_disabled_send_and_a_visible_banner() {
    let star_gold = quantize565(0xff, 0xd6, 0x6b);
    let surface_raised = quantize565(0x1e, 0x2a, 0x38);
    let warn = quantize565(0xff, 0xff, 0x00);

    let btn_x = WIDTH - 80 - 8;
    let btn_y = HEIGHT - 28 - 8;
    // Sample a corner of the banner rectangle, not its center — the banner
    // also carries a left-aligned, vertically-centered Text label whose
    // glyph coverage can reach the center at this font size, which would
    // sample an anti-aliased blend instead of the solid `Theme.warn` fill.
    // A 2px inset corner is comfortably outside the glyph ascent/descent
    // band while still being inside the Rectangle's own bounds.
    let banner_corner_x = BANNER_X + 2;
    let banner_corner_y = BANNER_Y + BANNER_H - 2;

    let frame = ComposeReadOnlyFrame::new();

    // ── Read-write room (baseline): a draft arms the Send button, no banner ──
    frame.set_read_only(false);
    frame.set_has_draft(true);
    let fb0 = frame.render();
    assert_eq!(fb0.len(), (WIDTH * HEIGHT) as usize);
    assert_eq!(
        at(&fb0, btn_x + 40, btn_y + 14),
        star_gold,
        "read-write room with a draft: Send button must be armed (star-gold)"
    );
    assert_ne!(
        at(&fb0, banner_corner_x, banner_corner_y),
        warn,
        "read-write room: no read-only banner should paint"
    );

    // ── Read-only room, WITH a draft present: still disabled + banner ───────
    frame.set_read_only(true);
    frame.set_has_draft(true);
    let fb1 = frame.render();
    assert_eq!(
        at(&fb1, btn_x + 40, btn_y + 14),
        surface_raised,
        "read-only room: Send button must stay disabled even with draft text present"
    );
    assert_eq!(
        at(&fb1, banner_corner_x, banner_corner_y),
        warn,
        "read-only room: the read-only banner must be visibly painted"
    );

    // ── Read-only room, no draft: same disabled look + banner (idempotent) ──
    frame.set_has_draft(false);
    let fb2 = frame.render();
    assert_eq!(at(&fb2, btn_x + 40, btn_y + 14), surface_raised);
    assert_eq!(at(&fb2, banner_corner_x, banner_corner_y), warn);
}
