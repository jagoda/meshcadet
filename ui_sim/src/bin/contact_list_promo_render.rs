// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim --bin contact_list_promo_render` — regenerates the
//! contact-list screen's promotional landing-page screenshot.
//!
//! Writes `site/assets/screenshot-contacts.png`: the full contact-list
//! screen (real markup — see `ui_sim::contact_list_promo`'s module doc)
//! seeded with a tasteful, OSS-appropriate sample of space-mission
//! callsigns — no PII, no internal vernacular. Regenerate after any change
//! to `firmware/src/ui/screens/contact_list.rs`'s markup or
//! `firmware/src/ui/theme.slint` by re-copying the updated markup into
//! `ui_sim::contact_list_promo` and re-running this binary.

use std::path::PathBuf;
use std::time::Duration;

use ui_sim::contact_list_promo::{ContactListPromoFrame, PromoContact};

fn main() {
    let frame = ContactListPromoFrame::new();
    frame.set_contacts(&[
        PromoContact {
            name: "Nova",
            initial: "N",
            preview: "Just spotted a bright pass overhead",
            time_str: "2m ago",
            unread: 2,
        },
        PromoContact {
            name: "Orion Relay",
            initial: "O",
            preview: "Solar panels reading 98%",
            time_str: "18m ago",
            unread: 0,
        },
        PromoContact {
            name: "Vega Station",
            initial: "V",
            preview: "Copy that, switching to channel 3",
            time_str: "1h ago",
            unread: 1,
        },
        PromoContact {
            name: "Comet Watch",
            initial: "C",
            preview: "Meteor shower peaks Friday night",
            time_str: "3h ago",
            unread: 0,
        },
        PromoContact {
            name: "Ground Control",
            initial: "G",
            preview: "Telemetry nominal, all systems go",
            time_str: "5h ago",
            unread: 0,
        },
        PromoContact {
            name: "Stargazer",
            initial: "S",
            preview: "Clear skies forecast this weekend",
            time_str: "1d ago",
            unread: 0,
        },
    ]);
    // The screen's one-shot `content_opacity` fade-in (see
    // `ui_sim::contact_list_promo`'s copied markup) animates over 200ms of
    // REAL WALL-CLOCK TIME from component construction — sleep past it
    // before capturing, or the screenshot would show the screen mid-fade
    // (near-invisible) instead of its settled, fully-opaque state. Same
    // "Slint's `animate` measures wall-clock time, not render calls" fact
    // `splash.rs`'s module doc explains at length.
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
        "screenshot-contacts.png",
    ]
    .iter()
    .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create site/assets");
    img.save(&out_path).expect("write promo screenshot PNG");
    println!("wrote promo screenshot: {}", out_path.display());
}
