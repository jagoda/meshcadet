// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the contact-list promo rig
//! (`ui_sim::contact_list_promo`, a verbatim copy of `contact_list.rs`'s full
//! markup) with a MIXED Groups list — one true channel and one room-server
//! entry unioned together — and asserts:
//!
//! 1. The Groups tab (formerly "Channels") actually shows both kinds, proven
//!    the same indirect way `contact_list_promo.rs`'s own Contacts-tab test
//!    proves its tab: the tab-bar aggregate badge painting when a seeded
//!    entry has unread > 0.
//! 2. A room row and a channel row are **visually distinguishable** — the
//!    `meshcadet-groups-contacts-rename` mission's core acceptance bullet —
//!    by asserting their avatar-circle fills render as two DIFFERENT solid
//!    colors (`Theme.select` for a channel, `Theme.nebula-violet` for a
//!    room; see `contact_list.rs`'s `ContactRow.is_room` styling).
//!
//! Per `meshcadet-room-firmware-login-read`'s (M1) original acceptance, a
//! room already rendered read-only in this tab with no visual distinction;
//! this test supersedes that M1-era assertion now that this mission adds
//! the distinction M1 explicitly deferred.
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
fn room_and_channel_entries_render_distinguishably_in_the_groups_tab() {
    let brand_signal = quantize565(0x00, 0xb4, 0xff);
    let select = quantize565(0x1e, 0x30, 0x50);
    let nebula_violet = quantize565(0x7c, 0x5c, 0xff);

    let frame = ContactListPromoFrame::new();
    frame.set_channels(&[
        PromoChannel {
            name: "Ops Net",
            initial: "O",
            preview: "channel chatter",
            time_str: "2m ago",
            unread: 0,
            is_room: false,
        },
        PromoChannel {
            name: "Mission Ops Room",
            initial: "M",
            preview: "welcome to the room",
            time_str: "1m ago",
            unread: 1,
            is_room: true,
        },
    ]);
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

    // Groups tab must be the active one — proven indirectly via its tab-bar
    // aggregate badge, same technique `contact_list_promo.rs`'s own
    // Contacts-tab test uses for its tab. Tab rect geometry: see
    // `tab_badge_paints_...`'s comment in `ui_sim/tests/contact_list_promo.rs`
    // — the Groups tab occupies the second stretch rect, so its badge
    // center is offset by one tab-width (120px, was 125px before
    // `meshcadet-battery-glanceable-indicator` widened the ADR-0010 slot
    // 26px -> 36px) from the Contacts tab's (110, 9).
    //
    // `badge_cy` is deliberately NOT the disc's vertical center (9-10): see
    // this same note preserved verbatim from the prior version of this test
    // — a text-glyph-edge blend (not a settle-timing or handler-wiring bug)
    // made a center-row assertion flaky across font/version differences.
    // `badge_cy = 5` sits solidly inside the 14px disc, clear of the
    // glyph's vertical band, so it still proves the badge painted
    // (re-confirmed empirically against this rig's current slot width via a
    // scratch probe, same technique `contact_list_promo.rs`'s own badge
    // point uses).
    let badge_cx = 110u32 + 120u32;
    let badge_cy = 5u32;
    assert_eq!(
        rgb8_at(&img, badge_cx, badge_cy),
        brand_signal,
        "Groups tab badge must render when a seeded room entry has unread > 0"
    );

    // Row 0 (the true channel, is_room: false): avatar circle is at
    // (12..48, 44..80) — header (36px) + row padding-top (8px) — see
    // `contact_list_promo.rs`'s copied markup; (30, 45) sits inside the
    // circle's fill clear of the centered initial glyph.
    let channel_avatar = rgb8_at(&img, 30, 45);
    assert_eq!(
        channel_avatar, select,
        "a true channel's avatar circle must render Theme.select — \
         did the content_opacity fade-in settle before capture?"
    );

    // Row 1 (the room entry, is_room: true): one more row height (54px)
    // down — avatar circle center at (30, 45 + 54) = (30, 99).
    let room_avatar = rgb8_at(&img, 30, 99);
    assert_eq!(
        room_avatar, nebula_violet,
        "a room entry's avatar circle must render Theme.nebula-violet, \
         not the plain-channel Theme.select fill"
    );

    // The core acceptance bullet: the two kinds must be visually
    // DISTINGUISHABLE, not just individually correct.
    assert_ne!(
        channel_avatar, room_avatar,
        "a room row and a channel row must not render identically in the \
         unified Groups list"
    );
}
