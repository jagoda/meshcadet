// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: regression guard for the header status-icon vertical
//! alignment fix (`meshcadet-messaging-status-icon-vertical-alignment`).
//! `message_view.rs`'s header `SignalMeter`/`BatteryIndicator` pair used to
//! sit at hardcoded `y: 3px`/`y: 5px` inside the 36px header spacer, while
//! `gps_status.rs`/`contact_list.rs`/`compose.rs` all vertically CENTER the
//! same pair (`y: (parent.height - self.height) / 2`) — the mismatch is
//! what made the pair visibly jump on every navigation between the
//! messaging view and any other screen. This test pins the centered
//! position on `message_view_promo.rs` (a verbatim copy of the real
//! screen's markup — see that module's own doc) so a future hand-edit
//! reverting to a hardcoded top-anchored offset fails loudly instead of
//! silently reintroducing the jump.
//!
//! Uses `SignalMeter`'s tallest bar (bar 5 of 5, at `signal_level: 5`) as
//! the probe: `signal_meter.slint` bottom-anchors every bar within its own
//! 14px-tall box, and the tallest bar's height equals the full box height,
//! so its top edge lands exactly at the widget's own `y` offset within the
//! embedding header — the topmost `Theme.brand-signal`-colored row in the
//! header band directly reveals the widget's `y`. Centered
//! (`y: (36 - 14) / 2 = 11`) vs. the old hardcoded `y: 3px` are far enough
//! apart (8px) that anti-aliasing/rounding noise can't confuse the two.
//!
//! Lives in its own `tests/*.rs` file (its own Cargo integration-test
//! binary / process) — see `contact_list_promo_meter_position.rs`'s module
//! doc for why: Slint enforces a process-wide `Platform` singleton, and a
//! second `MessageViewPromoFrame::new()` in the same binary as
//! `message_view_promo.rs`'s existing test would panic on the second
//! `set_platform` call.

use ui_sim::message_view_promo::{framebuffer_to_rgb_image, MessageViewPromoFrame, PromoMessage};

/// RGB565 is lossy (5/6/5 bits per channel) — round an 8-bit-per-channel
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
fn status_icon_pair_is_vertically_centered_in_the_header_not_top_anchored() {
    let brand_signal = quantize565(0x00, 0xb4, 0xff);

    let frame = MessageViewPromoFrame::new();
    frame.set_thread(
        "Nova",
        &[PromoMessage {
            text: "Just spotted a bright pass overhead",
            from_name: "",
            time_str: "2:14p",
            is_ours: false,
            acked: false,
        }],
    );
    // All 5 bars filled — the strongest, most visually unmistakable signal
    // reading, so the tallest bar's top edge is unambiguous.
    frame.set_signal_level(5);
    // Same wall-clock fade-in note as this module's sibling test
    // (`message_view_promo.rs`) — the screen's `content_opacity` one-shot
    // fade animates over 200ms of REAL TIME from construction; sleep past
    // it before capturing.
    std::thread::sleep(std::time::Duration::from_millis(250));
    let fb = frame.render();
    let img = framebuffer_to_rgb_image(
        &fb,
        ui_sim::message_view_promo::WIDTH,
        ui_sim::message_view_promo::HEIGHT,
    );

    // The status-icon slot occupies the header's rightmost 64px
    // (`Rectangle { width: 64px; height: 36px; }`, right-pinned by the
    // header-icon-edge-alignment mission) — scan that whole column band
    // across the 36px header row rather than hand-computing the bars'
    // exact x span, the same "scan by region" robustness
    // `contact_list_promo_meter_position.rs` documents.
    const SLOT_START_X: u32 = ui_sim::message_view_promo::WIDTH - 64;

    let mut topmost_brand_signal_y: Option<u32> = None;
    for y in 0..36u32 {
        for x in SLOT_START_X..ui_sim::message_view_promo::WIDTH {
            if rgb8_at(&img, x, y) == brand_signal {
                topmost_brand_signal_y = Some(y);
                break;
            }
        }
        if topmost_brand_signal_y.is_some() {
            break;
        }
    }

    let topmost_y = topmost_brand_signal_y.expect(
        "expected a filled signal-meter bar somewhere in the header's status-icon \
         slot at signal_level=5 — the meter did not render",
    );

    // Centered: y = (36 - 14) / 2 = 11. Old hardcoded top-anchored: y = 3.
    // Allow a few px of slack for rasterization, but the two positions are
    // 8px apart — nowhere near enough overlap to false-positive either way.
    assert!(
        (9..=13).contains(&topmost_y),
        "expected the signal meter's tallest bar to start at the header's \
         vertically CENTERED offset (y ≈ 11, i.e. `(parent.height - \
         self.height) / 2`) — found its topmost brand-signal pixel at y = \
         {topmost_y} instead. If this is 3 (or nearby), the header's status \
         icons have regressed back to the old hardcoded top-anchored \
         `y: 3px`/`y: 5px` offsets that caused the icons to visibly jump \
         when navigating between the messaging view and other screens."
    );
}
