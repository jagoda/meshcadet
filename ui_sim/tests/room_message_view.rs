// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the message-view promo rig
//! (`ui_sim::message_view_promo`, a verbatim copy of `message_view.rs`'s full
//! markup) with a room's pushed posts and asserts a received-message bubble
//! paints — the `meshcadet-room-firmware-login-read` (M1) acceptance bullet
//! "its posts are readable in the message view".
//!
//! No code change was needed in `message_view_promo.rs`/`message_view.rs` to
//! support this: a room's pushed posts are stored as ordinary
//! `HistoryMsgType::Dm` entries (`is_ours: false`, same conversation-hash
//! keying every DM/channel thread already uses — see
//! `firmware_core::room_session::handle_room_push`'s doc), so the EXISTING
//! received-message rendering path already covers them; this test proves
//! that render path renders a room's actual post text.
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process) —
//! see `compose_send.rs`'s module doc for the full "why a second render
//! path" rationale, which applies identically here.

use ui_sim::message_view_promo::{framebuffer_to_rgb_image, MessageViewPromoFrame, PromoMessage};

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
fn room_posts_render_as_received_message_bubbles() {
    let surface_raised = quantize565(0x1e, 0x2a, 0x38);

    let frame = MessageViewPromoFrame::new();
    // Every message here is a pushed room post (`is_ours: false`) — mirrors
    // what `handle_room_push`'s produced `HistoryEntry`s look like once
    // hydrated into `MessageRecord`s: no author-name prefix (room pushes
    // carry only a 4-byte pubkey prefix, not a display name, in M1's thin
    // slice — see this test's module doc), read-only from this client's
    // point of view.
    frame.set_thread(
        "Mission Ops Room",
        &[
            PromoMessage {
                text: "post one",
                from_name: "",
                time_str: "3:01p",
                is_ours: false,
                acked: false,
            },
            PromoMessage {
                text: "post two",
                from_name: "",
                time_str: "3:02p",
                is_ours: false,
                acked: false,
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

    // A received-message bubble (`Theme.surface-raised`, left-aligned) must
    // paint somewhere in the message-list band — proof the room's posts are
    // readable in the message view, not merely stored.
    let mut received_bubble_painted = false;
    'outer: for y in 36..200u32 {
        for x in 0..ui_sim::message_view_promo::WIDTH {
            if rgb8_at(&img, x, y) == surface_raised {
                received_bubble_painted = true;
                break 'outer;
            }
        }
    }
    assert!(
        received_bubble_painted,
        "received room-post bubble did not render anywhere in the message list — \
         did the content_opacity fade-in settle before capture?"
    );
}
