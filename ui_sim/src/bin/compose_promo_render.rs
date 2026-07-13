// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim --bin compose_promo_render` — regenerates the
//! compose screen's promotional landing-page screenshot.
//!
//! Writes `site/assets/screenshot-compose.png`: the full compose screen
//! (real markup — see `ui_sim::compose_promo`'s module doc) seeded with a
//! sample draft-in-progress, Send button ARMED (star-gold), rocket
//! mid-flight — the same "capture mid-flight so the render shows motion,
//! not just a settled/empty end state" choice `compose_send_render.rs`
//! makes; see that binary's doc for the full two-render technique this
//! mirrors. Regenerate after any change to
//! `firmware/src/ui/screens/compose.rs`'s markup or
//! `firmware/src/ui/theme.slint` by re-copying the updated markup into
//! `ui_sim::compose_promo` and re-running this binary.

use std::path::PathBuf;
use std::time::Duration;

use ui_sim::compose_promo::ComposePromoFrame;

fn main() {
    let frame = ComposePromoFrame::new();
    frame.set_to_name("Nova");
    frame.set_draft("Meet at the north ridge for the meteor shower tonight - bring a blanket!");
    // Good repeater signal (ADR-0010) — a compelling, on-brand default for
    // the promo shot rather than the direct-only ring.
    frame.set_signal_level(4);
    // Render once at the REST state first — Slint only animates a VALUE
    // CHANGE relative to an already-rendered previous value (see
    // `compose_send_render.rs`'s identical note); triggering before any
    // frame has been rendered would jump straight to the settled end state
    // instead of animating through it.
    let _ = frame.render();
    frame.set_rocket_trigger(true);
    std::thread::sleep(Duration::from_millis(200));
    let framebuffer = frame.render();

    let img = ui_sim::compose_promo::framebuffer_to_rgb_image(
        &framebuffer,
        ui_sim::compose_promo::WIDTH,
        ui_sim::compose_promo::HEIGHT,
    );

    let out_path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "site",
        "assets",
        "screenshot-compose.png",
    ]
    .iter()
    .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create site/assets");
    img.save(&out_path).expect("write promo screenshot PNG");
    println!("wrote promo screenshot: {}", out_path.display());
}
