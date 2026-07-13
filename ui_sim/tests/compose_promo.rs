// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the promotional compose screenshot rig
//! (`ui_sim::compose_promo`, a verbatim copy of `compose.rs`'s full markup)
//! and asserts the Send button's `star-gold` armed/idle affordance — the
//! same behavior `ui_sim::compose_send`'s own dedicated test proves on its
//! narrower, single-mechanism markup — also holds on this full-screen
//! mirror. A regression guard against the copied markup silently drifting
//! from the real screen.
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process) —
//! see `compose_send.rs`'s module doc for the full "why a second render
//! path" rationale, which applies identically here.

use ui_sim::compose_promo::{framebuffer_to_rgb_image, ComposePromoFrame};

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

/// Single test — see module doc: exactly one `ComposePromoFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn send_button_is_idle_gray_with_no_draft_and_star_gold_once_seeded() {
    let star_gold = quantize565(0xff, 0xd6, 0x6b);
    let surface_raised = quantize565(0x1e, 0x2a, 0x38);

    // Send button geometry on the FULL compose screen (not
    // `compose_send.rs`'s narrower standalone rig): the action bar's
    // `HorizontalLayout` sets `alignment: center`, which — per `compose.rs`'s
    // own documented header BUG FIX note — packs children at NATURAL size
    // (ignoring the spacer `Rectangle`'s `horizontal-stretch: 1.0`, which
    // becomes a no-op zero-width child under `alignment: center`) and
    // centers that packed group as a whole, rather than pinning the emoji
    // toggle left / Send button right. Measured empirically against this
    // rig (not re-derived from the layout math, which this exact gotcha
    // makes easy to get wrong): the button's solid fill spans roughly
    // x=147..224, y=206..233; (155, 220) sits inside that box clear of the
    // "\u{1F4E4} Send" glyph ink (checked in both idle and armed states).
    let btn_cx = 155u32;
    let btn_cy = 220u32;

    let frame = ComposePromoFrame::new();

    // ── Idle: no draft ──────────────────────────────────────────────────
    frame.set_to_name("Nova");
    let fb0 = frame.render();
    let img0 = framebuffer_to_rgb_image(
        &fb0,
        ui_sim::compose_promo::WIDTH,
        ui_sim::compose_promo::HEIGHT,
    );
    assert_eq!(
        rgb8_at(&img0, btn_cx, btn_cy),
        surface_raised,
        "idle Send button (no draft) should be surface-raised, not star-gold"
    );

    // ── Armed: draft text present ───────────────────────────────────────
    frame.set_draft("Meet at the north ridge tonight");
    // The button's `animate background { duration: 120ms; }` (see
    // `compose_promo.rs`'s copied markup) plays a transition on this VALUE
    // CHANGE — sleep past it before capturing, same wall-clock-animation
    // gotcha `contact_list_promo_render.rs` hit for `content_opacity`.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let fb1 = frame.render();
    let img1 = framebuffer_to_rgb_image(
        &fb1,
        ui_sim::compose_promo::WIDTH,
        ui_sim::compose_promo::HEIGHT,
    );
    assert_eq!(
        rgb8_at(&img1, btn_cx, btn_cy),
        star_gold,
        "armed (draft != \"\") Send button should be star-gold"
    );
}
