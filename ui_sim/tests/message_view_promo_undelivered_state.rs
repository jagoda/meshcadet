// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the promotional message-view screenshot rig
//! (`ui_sim::message_view_promo`) with an own (`is_ours: true`) message at
//! `delivery_state: 2` (Undelivered) and asserts its ack checkmark paints
//! `Theme.alert` red — not `Theme.brand-signal` blue (Acked) or
//! `Theme.text-secondary` grey (Pending).
//!
//! Regression guard for `meshcadet-site-screenshot-refresh-20260822-174709190`
//! (the site-screenshot rig-drift audit): this rig's `MessageEntry` was
//! previously `acked: bool`, which could only ever express Pending/Acked —
//! the real screen's third state (`message_view.rs`'s
//! `MessageEntry.delivery_state == 2`, landed by
//! `meshcadet-dm-room-delivery-state-model`) was structurally unreachable
//! here, so no test — this one included, since it didn't exist — could have
//! caught the rig silently falling behind the real screen's contract.
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process) —
//! see `compose_send.rs`'s module doc for the full "why a second render
//! path" rationale, which applies identically here.

use ui_sim::message_view_promo::{framebuffer_to_rgb_image, MessageViewPromoFrame, PromoMessage};

fn rgb8_at(img: &image::RgbImage, x: u32, y: u32) -> (u8, u8, u8) {
    let px = img.get_pixel(x, y);
    (px[0], px[1], px[2])
}

/// The ✓ glyph is tiny (`Theme.size-caption`, 11px) and anti-aliased, so no
/// pixel ever reaches a fully-saturated (255, 0, 0)/(0, 180, 255) — even the
/// glyph's own darkest-covered pixel blends partway with the near-black
/// background. A loose channel-dominance threshold (not exact/quantized
/// equality) is what every sibling ack-color check site in this crate that
/// samples anti-aliased text uses for the same reason.
fn looks_red(px: (u8, u8, u8)) -> bool {
    px.0 > 100 && px.1 < 80 && px.2 < 80
}

fn looks_brand_signal_blue(px: (u8, u8, u8)) -> bool {
    px.2 > 100 && px.0 < 80 && px.1 > 60
}

/// Single test — see module doc: exactly one `MessageViewPromoFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn undelivered_own_message_ack_checkmark_renders_alert_red() {
    let frame = MessageViewPromoFrame::new();
    frame.set_thread(
        "Nova",
        &[PromoMessage {
            text: "Snapped a few - sending later tonight",
            from_name: "",
            time_str: "2:16p",
            is_ours: true,
            delivery_state: 2, // Undelivered
        }],
    );
    // Same wall-clock fade-in note as `message_view_promo.rs` — the screen's
    // `content_opacity` one-shot fade animates over 200ms of REAL TIME from
    // construction; sleep past it before capturing.
    std::thread::sleep(std::time::Duration::from_millis(250));
    let fb = frame.render();
    let img = framebuffer_to_rgb_image(
        &fb,
        ui_sim::message_view_promo::WIDTH,
        ui_sim::message_view_promo::HEIGHT,
    );

    // Scoped to the single seeded message's own bubble+checkmark row, NOT
    // the full message-list band: `Theme.brand-signal` blue also paints the
    // header's back chevron/signal-meter (y < 36) and the bottom "Write"
    // button pill (this short, one-message thread's `Flickable` sizes to
    // content, so the pill sits far higher than a full thread would — see
    // `message_view_promo.rs`'s own test's identical note) — scanning the
    // whole list would make a brand-signal-absence assertion meaningless
    // (it would always find the Write button). y=36..80 covers the bubble
    // and its ack checkmark for this one-message seed and nothing else.
    let mut alert_pixels = 0u32;
    let mut brand_pixels = 0u32;
    for y in 36..80u32 {
        for x in 0..ui_sim::message_view_promo::WIDTH {
            let px = rgb8_at(&img, x, y);
            if looks_red(px) {
                alert_pixels += 1;
            } else if looks_brand_signal_blue(px) {
                brand_pixels += 1;
            }
        }
    }
    assert!(
        alert_pixels > 0,
        "Undelivered (delivery_state: 2) ack checkmark did not paint Theme.alert red anywhere \
         near the message bubble — did the content_opacity fade-in settle before capture, or \
         has this rig regressed back to a two-state acked/not-acked checkmark?"
    );
    assert_eq!(
        brand_pixels, 0,
        "Undelivered message's ack checkmark painted brand-signal blue (the Acked color) \
         instead of alert red"
    );
}
