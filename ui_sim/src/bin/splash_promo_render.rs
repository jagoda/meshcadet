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
    // Mirrors firmware's release-build version string (`firmware/build.rs`'s
    // `MESHCADET_RELEASE_VERSION` seam, `firmware/release-container/build.sh`'s
    // `VERSION="${1}"` == the `vX.Y.Z` release tag): a "v" prefix + the bare
    // semver, sourced from the workspace version at compile time
    // (`ui_sim/Cargo.toml`'s `version.workspace = true`) so this promo
    // screenshot tracks the current release instead of re-staling at the next
    // one. Deliberately NOT the firmware dev-build path (`git rev-parse
    // --short HEAD`, no "v" prefix) — the promo screenshot represents a
    // release, not a dev build.
    frame.set_version(concat!("v", env!("CARGO_PKG_VERSION")));
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
