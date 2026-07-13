// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the promotional message-view screenshot rig
//! (`ui_sim::message_view_promo`, a verbatim copy of `message_view.rs`'s
//! full markup) and asserts an own (`is_ours: true`) message paints its
//! distinct `nebula-violet` bubble tint, and the bottom "Write" button
//! reads its own flat, opaque `brand-signal` fill — a regression guard
//! against the copied markup silently drifting from the real screen.
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process) —
//! see `compose_send.rs`'s module doc for the full "why a second render
//! path" rationale, which applies identically here.

use ui_sim::message_view_promo::{framebuffer_to_rgb_image, MessageViewPromoFrame, PromoMessage};

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

/// Single test — see module doc: exactly one `MessageViewPromoFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn own_message_bubble_and_write_button_render() {
    let nebula_violet = quantize565(0x7c, 0x5c, 0xff);
    let brand_signal = quantize565(0x00, 0xb4, 0xff);

    let frame = MessageViewPromoFrame::new();
    frame.set_thread(
        "Nova",
        &[
            PromoMessage {
                text: "Just spotted a bright pass overhead",
                from_name: "",
                time_str: "2:14p",
                is_ours: false,
                acked: false,
            },
            PromoMessage {
                text: "Nice catch!",
                from_name: "",
                time_str: "2:15p",
                is_ours: true,
                acked: true,
            },
        ],
    );
    // Same wall-clock fade-in note as `message_view_promo_render.rs` — the
    // screen's `content_opacity` one-shot fade animates over 200ms of REAL
    // TIME from construction; sleep past it before capturing.
    std::thread::sleep(std::time::Duration::from_millis(250));
    let fb = frame.render();
    let img = framebuffer_to_rgb_image(
        &fb,
        ui_sim::message_view_promo::WIDTH,
        ui_sim::message_view_promo::HEIGHT,
    );

    // The own-message bubble (nebula-violet, right-aligned per `is_ours`)
    // must paint somewhere in the message-list band (below the 36px header,
    // above the 40px bottom bar).
    let mut own_bubble_painted = false;
    'outer: for y in 36..200u32 {
        for x in 0..ui_sim::message_view_promo::WIDTH {
            if rgb8_at(&img, x, y) == nebula_violet {
                own_bubble_painted = true;
                break 'outer;
            }
        }
    }
    assert!(
        own_bubble_painted,
        "own-message nebula-violet bubble did not render anywhere in the message list — \
         did the content_opacity fade-in settle before capture?"
    );

    // "Write" button: 120x28, `brand-signal` fill. NOT pinned to the
    // bottom-bar's nominal (y=200..240) box — the containing `Flickable`
    // (`flick`, `vertical-stretch: 1.0`, no explicit height) sizes itself to
    // its CONTENT's natural height rather than the stretch-allocated
    // remainder when the thread is short (same "a `Flickable` with no
    // explicit height binding grows to its content's preferred size"
    // mechanism `compose.rs`'s `EmojiPickerGrid` module doc documents for
    // the identical component type) — this is the REAL screen's own
    // verbatim-copied behavior, not a defect in this rig, so a short
    // 2-message seed genuinely renders the button higher up than a long
    // thread does. Scan broadly rather than pinning a coordinate.
    let mut write_button_painted = false;
    'outer2: for y in 40..ui_sim::message_view_promo::HEIGHT {
        for x in 0..ui_sim::message_view_promo::WIDTH {
            if rgb8_at(&img, x, y) == brand_signal {
                write_button_painted = true;
                break 'outer2;
            }
        }
    }
    assert!(
        write_button_painted,
        "Write button did not render its flat brand-signal fill anywhere below the header"
    );
}
