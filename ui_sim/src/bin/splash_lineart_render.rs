// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim --bin splash_lineart_render` — regenerates the
//! splash screen's full-window backdrop + lower-half line art host-sim
//! render deliverable.
//!
//! Writes `docs/renders/splash-lineart-host-sim.png`: the splash screen's
//! static-complete frame (logo/wordmark/version fully opaque, rings not yet
//! rippling) with the full-window `SpaceBackdrop` + bottom-anchored
//! `PlanetHorizon` line art. Host-sim renders cannot judge final on-hardware
//! legibility; this is the visual evidence this
//! check's abort condition ("horizon overlaps/obscures the wordmark or
//! version string") was checked against. Asserted on (non-visually) by
//! `cargo test -p ui_sim --test splash_lineart`.

use std::path::PathBuf;

fn main() {
    let frame = ui_sim::splash_lineart::SplashLineartFrame::new();
    let framebuffer = frame.render();

    let img = ui_sim::splash_lineart::framebuffer_to_rgb_image(
        &framebuffer,
        ui_sim::splash_lineart::WIDTH,
        ui_sim::splash_lineart::HEIGHT,
    );

    let out_path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "docs", "renders", "splash-lineart-host-sim.png"]
        .iter()
        .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create docs/renders");
    img.save(&out_path).expect("write host-sim render PNG");
    println!("wrote host-sim render: {}", out_path.display());
}
