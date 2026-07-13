// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the promotional contact-list screenshot rig
//! (`ui_sim::contact_list_promo`, a verbatim copy of `contact_list.rs`'s
//! full markup) and asserts the tab-bar aggregate unread badge actually
//! paints when seeded contacts carry unread messages, and does NOT paint
//! when they don't — a regression guard against the copied markup silently
//! drifting from the real screen (or the seed data silently losing its
//! `unread` counts) between `cargo run --bin contact_list_promo_render`
//! regenerations.
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process) —
//! see `compose_send.rs`'s module doc for the full "why a second render
//! path" rationale, which applies identically here.

use ui_sim::contact_list_promo::{framebuffer_to_rgb_image, ContactListPromoFrame, PromoContact};

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

/// Single test — see module doc: exactly one `ContactListPromoFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn tab_badge_paints_when_seeded_unread_is_nonzero_and_rows_are_visible() {
    let brand_signal = quantize565(0x00, 0xb4, 0xff);

    let frame = ContactListPromoFrame::new();

    // ── Zero-unread baseline ────────────────────────────────────────────
    frame.set_contacts(&[PromoContact {
        name: "Nova",
        initial: "N",
        preview: "All quiet",
        time_str: "2m ago",
        unread: 0,
    }]);
    // Same wall-clock fade-in note as `contact_list_promo_render.rs` — the
    // screen's `content_opacity` one-shot fade animates over 200ms of REAL
    // TIME from construction; sleep past it before capturing.
    std::thread::sleep(std::time::Duration::from_millis(250));
    let fb0 = frame.render();
    let img0 = framebuffer_to_rgb_image(
        &fb0,
        ui_sim::contact_list_promo::WIDTH,
        ui_sim::contact_list_promo::HEIGHT,
    );

    // Tab badge box: header HorizontalLayout is [Messages tab: stretch]
    // [Channels tab: stretch][gear button: 44px fixed] across 320px, so each
    // tab rect is (320-44)/2 = 138px wide; the badge sits at
    // `x: parent.width - 18px, y: 3px, width: 14px, height: 14px` inside the
    // Messages tab rect (see contact_list_promo.rs's copied markup) —
    // absolute (120..134, 3..17).
    let badge_cx = 127u32;
    let badge_cy = 10u32;
    assert_ne!(
        rgb8_at(&img0, badge_cx, badge_cy),
        brand_signal,
        "tab badge must NOT render when every contact's unread is 0"
    );

    // ── Nonzero-unread state ────────────────────────────────────────────
    frame.set_contacts(&[PromoContact {
        name: "Nova",
        initial: "N",
        preview: "Just spotted a bright pass overhead",
        time_str: "2m ago",
        unread: 2,
    }]);
    let fb1 = frame.render();
    let img1 = framebuffer_to_rgb_image(
        &fb1,
        ui_sim::contact_list_promo::WIDTH,
        ui_sim::contact_list_promo::HEIGHT,
    );
    assert_eq!(
        rgb8_at(&img1, badge_cx, badge_cy),
        brand_signal,
        "tab badge must render brand-signal when a seeded contact has unread > 0"
    );

    // Avatar circle of the first row must read as a SOLID, fully-settled
    // `Theme.select` fill (#1e3050) — not a blend toward `bg-space` — which
    // is only true once the screen's `content_opacity` one-shot fade-in has
    // fully settled to 1.0. This is a direct regression guard for the exact
    // bug this rig hit during authoring: capturing before the 200ms
    // wall-clock fade settles renders an all-but-invisible frame. Row 0's
    // 36x36 avatar circle is at (12..48, 44..80) — header (36px) + row
    // padding-top (8px) — see `contact_list_promo.rs`'s copied markup;
    // (30, 45) sits inside the circle's fill clear of the centered "N"
    // initial glyph (checked empirically against this rig).
    let select = quantize565(0x1e, 0x30, 0x50);
    assert_eq!(
        rgb8_at(&img1, 30, 45),
        select,
        "first row's avatar circle is not a solid Theme.select fill — \
         did the content_opacity fade-in settle before capture?"
    );
}
