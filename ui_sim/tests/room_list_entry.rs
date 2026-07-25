// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the contact-list promo rig
//! (`ui_sim::contact_list_promo`, a verbatim copy of `contact_list.rs`'s full
//! markup) with a room-server contact seeded into the Channels tab, and
//! asserts the row actually paints — the `meshcadet-room-firmware-login-read`
//! (M1) acceptance bullet "a `ui_sim` render shows a room entry in the list".
//!
//! Per that mission's Objective, a room renders read-only in the EXISTING
//! Channels tab with no new tab and no visual-distinction work — so this
//! seeds a room the same way `contact_list_promo.rs`'s new `set_channels`
//! seeds any other channel row, and reuses the exact avatar-circle assertion
//! technique `contact_list_promo.rs`'s own Contacts-tab test already
//! establishes (`tab_badge_paints_when_seeded_unread_is_nonzero_and_rows_are_visible`).
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process) —
//! see `compose_send.rs`'s module doc for the full "why a second render
//! path" rationale, which applies identically here.

use ui_sim::contact_list_promo::{framebuffer_to_rgb_image, ContactListPromoFrame, PromoChannel};

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
fn room_entry_renders_in_the_channels_tab() {
    let brand_signal = quantize565(0x00, 0xb4, 0xff);
    let select = quantize565(0x1e, 0x30, 0x50);

    let frame = ContactListPromoFrame::new();
    frame.set_channels(&[PromoChannel {
        name: "Mission Ops Room",
        initial: "M",
        preview: "welcome to the room",
        time_str: "1m ago",
        unread: 1,
    }]);
    // Same wall-clock fade-in note as `contact_list_promo_render.rs` — the
    // screen's `content_opacity` one-shot fade animates over 200ms of REAL
    // TIME from construction; sleep past it before capturing.
    std::thread::sleep(std::time::Duration::from_millis(250));
    let fb = frame.render();
    let img = framebuffer_to_rgb_image(
        &fb,
        ui_sim::contact_list_promo::WIDTH,
        ui_sim::contact_list_promo::HEIGHT,
    );

    // Channels tab must be the active one (underline + active label color) —
    // proven indirectly via its tab-bar aggregate badge, same technique
    // `contact_list_promo.rs`'s own Contacts-tab test uses for its tab.
    // Tab rect geometry: see `tab_badge_paints_...`'s comment in
    // `ui_sim/tests/contact_list_promo.rs` — the Channels tab occupies the
    // second stretch rect, so its badge center is offset by one tab-width
    // (125px) from the Messages tab's (115, 9).
    let badge_cx = 115u32 + 125u32;
    let badge_cy = 9u32;
    assert_eq!(
        rgb8_at(&img, badge_cx, badge_cy),
        brand_signal,
        "Channels tab badge must render when the seeded room entry has unread > 0"
    );

    // The room row's avatar circle (row 0, same geometry as the Contacts-tab
    // test) must paint as a solid `Theme.select` fill — proof the row itself
    // rendered, not just that the tab switched.
    assert_eq!(
        rgb8_at(&img, 30, 45),
        select,
        "room entry's avatar circle did not render in the Channels tab list — \
         did the content_opacity fade-in settle before capture?"
    );
}
