// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: regression guard for the signal-meter reposition. The
//! header's `SignalMeter` (ADR-0010) must render in its own slot to the
//! RIGHT of the gear button (`HeaderIconButton { width: 44px; icon: "⚙"; …
//! }`), not to its left — the two used to be declared meter-then-gear
//! (meter read as part of the "Groups" tab beside it, the defect this
//! guards against); `contact_list_promo.rs`'s copied markup now declares
//! gear-then-meter, matching `contact_list.rs`'s real-screen header.
//!
//! Lives in its own `tests/*.rs` file (its own Cargo integration-test
//! binary / process) rather than as a second `#[test]` fn alongside
//! `contact_list_promo.rs`'s existing badge test — Slint enforces a
//! process-wide `Platform` singleton (see `ui_sim::contact_list_promo`'s
//! module doc), and `cargo test` runs every `#[test]` fn in a single
//! integration-test binary as sibling THREADS of the same process, not
//! separate processes; a second `ContactListPromoFrame::new()` in that same
//! binary would panic on the second `set_platform` call.

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

#[test]
fn signal_meter_renders_right_of_the_gear_button_not_left() {
    let brand_signal = quantize565(0x00, 0xb4, 0xff);

    let frame = ContactListPromoFrame::new();
    frame.set_contacts(&[PromoContact {
        name: "Nova",
        initial: "N",
        preview: "All quiet",
        time_str: "2m ago",
        unread: 0,
    }]);
    // All 5 bars filled — the strongest, most visually unmistakable meter
    // reading, so a stray brand-signal pixel anywhere in the header band
    // can only be the meter, never anti-aliasing noise from a dimmer state.
    frame.set_signal_level(5);
    // Same wall-clock fade-in note as `contact_list_promo.rs`'s existing
    // test / `contact_list_promo_render.rs` — the screen's `content_opacity`
    // one-shot fade animates over 200ms of REAL TIME from construction;
    // sleep past it before capturing.
    std::thread::sleep(std::time::Duration::from_millis(250));
    let fb = frame.render();
    let img = framebuffer_to_rgb_image(
        &fb,
        ui_sim::contact_list_promo::WIDTH,
        ui_sim::contact_list_promo::HEIGHT,
    );

    // Header HorizontalLayout is [Contacts tab: stretch][Groups tab:
    // stretch][gear: 44px fixed][SignalMeter+BatteryIndicator slot: 36px
    // fixed] across 320px (slot widened 26px -> 36px by
    // `meshcadet-battery-glanceable-indicator` to fit the new
    // `BatteryIndicator` beside the meter — see `contact_list_promo.rs`'s
    // own comment), so the gear occupies x: 240..284 and the
    // meter+battery slot occupies x: 284..320 (tab rects each narrow to
    // (320-44-36)/2 = 120px — same total fixed width on both sides,
    // declaration ORDER is what keeps the meter right of the gear).
    //
    // Scanned by REGION rather than a single hardcoded pixel (the meter's
    // exact bar position shifts with slot width, and a scan is robust to
    // that the way a single coordinate isn't — see this test's own history:
    // the slot widened once already, for the battery indicator, and a
    // single-pixel assertion would have needed hand-recomputing again).
    const GEAR_END_X: u32 = 284;

    let mut found_brand_signal_right_of_gear = false;
    for y in 0..36u32 {
        for x in GEAR_END_X..ui_sim::contact_list_promo::WIDTH {
            if rgb8_at(&img, x, y) == brand_signal {
                found_brand_signal_right_of_gear = true;
            }
        }
    }
    assert!(
        found_brand_signal_right_of_gear,
        "expected a filled signal-meter bar somewhere to the RIGHT of the \
         gear button (x: {GEAR_END_X}..320) at signal_level=5 — the meter \
         did not render in its slot"
    );

    // The gear button's own span (x: 240..284) must be entirely free of
    // brand-signal pixels — the gear glyph paints in `Theme.text-secondary`,
    // never brand-signal, so any brand-signal pixel found there means the
    // meter (or something else) bled into the gear's box.
    for y in 0..36u32 {
        for x in 240..GEAR_END_X {
            let px = rgb8_at(&img, x, y);
            assert_ne!(
                px, brand_signal,
                "found a brand-signal pixel at ({x}, {y}), inside the gear \
                 button's own span (x: 240..{GEAR_END_X}) — the meter \
                 regressed back to the left of the gear button"
            );
        }
    }
}
