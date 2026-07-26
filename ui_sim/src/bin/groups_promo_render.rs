// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim --bin groups_promo_render` — regenerates the
//! contact-list screen's **Groups tab** promotional landing-page screenshot.
//!
//! Writes `site/assets/screenshot-groups.png`: the same real contact-list
//! markup `contact_list_promo_render.rs` captures (see
//! `ui_sim::contact_list_promo`'s module doc), but switched to the Groups
//! tab and seeded with a MIXED list — a true channel and a room-server
//! contact unioned together, `is_room`-tinted per `ui_sim/tests/
//! room_list_entry.rs`'s acceptance proof — so the campaign's headline UI
//! change (rooms appearing read-only in an existing, unified Groups list,
//! visually distinct from a true channel) is documented on the landing page,
//! not just proven by test. Regenerate after any change to
//! `firmware/src/ui/screens/contact_list.rs`'s markup, `Theme.select`/
//! `Theme.nebula-violet`, or this seed data by re-copying the updated markup
//! into `ui_sim::contact_list_promo` and re-running this binary.

use std::path::PathBuf;
use std::time::Duration;

use ui_sim::contact_list_promo::{ContactListPromoFrame, PromoChannel};

fn main() {
    let frame = ContactListPromoFrame::new();
    frame.set_channels(&[
        PromoChannel {
            name: "Ops Net",
            initial: "O",
            preview: "Copy that, switching to channel 3",
            time_str: "12m ago",
            unread: 0,
            is_room: false,
        },
        PromoChannel {
            name: "Basecamp Room",
            initial: "B",
            preview: "Backlog synced - 12 new posts",
            time_str: "4m ago",
            unread: 3,
            is_room: true,
        },
        PromoChannel {
            name: "Weather Net",
            initial: "W",
            preview: "Clear skies through the weekend",
            time_str: "40m ago",
            unread: 0,
            is_room: false,
        },
    ]);
    // Good repeater signal (ADR-0010) — a compelling, on-brand default for
    // the promo shot rather than the direct-only ring.
    frame.set_signal_level(4);
    // Same wall-clock fade-in note as `contact_list_promo_render.rs` — the
    // screen's one-shot `content_opacity` fade animates over 200ms of REAL
    // WALL-CLOCK TIME from component construction; sleep past it before
    // capturing.
    std::thread::sleep(Duration::from_millis(250));
    let framebuffer = frame.render();

    let img = ui_sim::contact_list_promo::framebuffer_to_rgb_image(
        &framebuffer,
        ui_sim::contact_list_promo::WIDTH,
        ui_sim::contact_list_promo::HEIGHT,
    );

    let out_path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "site",
        "assets",
        "screenshot-groups.png",
    ]
    .iter()
    .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create site/assets");
    img.save(&out_path).expect("write promo screenshot PNG");
    println!("wrote promo screenshot: {}", out_path.display());
}
