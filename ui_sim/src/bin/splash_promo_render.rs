// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim --bin splash_promo_render` — regenerates the boot
//! splash screen's promotional landing-page screenshot.
//!
//! Writes `site/assets/screenshot-splash.png`: the full splash screen (real
//! markup — see `ui_sim::splash_promo`'s module doc) at its
//! "static-complete" first frame — logo, "MeshCadet" wordmark, and version
//! string already fully opaque, full-window starfield backdrop + lower-half
//! planet-horizon line art. Regenerate after any change to
//! `firmware/src/ui/screens/splash.rs`'s markup or
//! `firmware/src/ui/theme.slint` by re-copying the updated markup into
//! `ui_sim::splash_promo` and re-running this binary.

use std::path::PathBuf;

use ui_sim::splash_promo::SplashPromoFrame;

fn main() {
    let frame = SplashPromoFrame::new();
    frame.set_version("v0.2.0");
    let framebuffer = frame.render();

    let img = ui_sim::splash_promo::framebuffer_to_rgb_image(
        &framebuffer,
        ui_sim::splash_promo::WIDTH,
        ui_sim::splash_promo::HEIGHT,
    );

    let out_path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "site",
        "assets",
        "screenshot-splash.png",
    ]
    .iter()
    .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create site/assets");
    img.save(&out_path).expect("write promo screenshot PNG");
    println!("wrote promo screenshot: {}", out_path.display());
}
