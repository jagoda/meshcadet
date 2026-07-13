// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim --bin message_view_promo_render` — regenerates the
//! message-view screen's promotional landing-page screenshot.
//!
//! Writes `site/assets/screenshot-messages.png`: the full message-view
//! screen (real markup — see `ui_sim::message_view_promo`'s module doc)
//! seeded with an engaging, OSS-appropriate sample conversation — no PII,
//! no internal vernacular. Regenerate after any change to
//! `firmware/src/ui/screens/message_view.rs`'s markup or
//! `firmware/src/ui/theme.slint` by re-copying the updated markup into
//! `ui_sim::message_view_promo` and re-running this binary.

use std::path::PathBuf;
use std::time::Duration;

use ui_sim::message_view_promo::{MessageViewPromoFrame, PromoMessage};

fn main() {
    let frame = MessageViewPromoFrame::new();
    frame.set_thread(
        "Nova",
        &[
            PromoMessage {
                text: "Just spotted a bright pass overhead - ISS maybe?",
                from_name: "",
                time_str: "2:14p",
                is_ours: false,
                acked: false,
            },
            PromoMessage {
                text: "Nice catch! Any photos?",
                from_name: "",
                time_str: "2:15p",
                is_ours: true,
                acked: true,
            },
            PromoMessage {
                text: "Snapped a few - sending later tonight",
                from_name: "",
                time_str: "2:16p",
                is_ours: false,
                acked: false,
            },
            PromoMessage {
                text: "Can't wait!",
                from_name: "",
                time_str: "2:17p",
                is_ours: true,
                acked: false,
            },
            PromoMessage {
                text: "Also - clear skies forecast for the meteor shower this weekend!",
                from_name: "",
                time_str: "2:19p",
                is_ours: false,
                acked: false,
            },
        ],
    );
    // Good repeater signal (ADR-0010) — a compelling, on-brand default for
    // the promo shot rather than the direct-only ring.
    frame.set_signal_level(4);
    // Same wall-clock fade-in note as `contact_list_promo_render.rs` — sleep
    // past `content_opacity`'s 200ms `animate` before capturing.
    std::thread::sleep(Duration::from_millis(250));
    let framebuffer = frame.render();

    let img = ui_sim::message_view_promo::framebuffer_to_rgb_image(
        &framebuffer,
        ui_sim::message_view_promo::WIDTH,
        ui_sim::message_view_promo::HEIGHT,
    );

    let out_path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "site",
        "assets",
        "screenshot-messages.png",
    ]
    .iter()
    .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create site/assets");
    img.save(&out_path).expect("write promo screenshot PNG");
    println!("wrote promo screenshot: {}", out_path.display());
}
