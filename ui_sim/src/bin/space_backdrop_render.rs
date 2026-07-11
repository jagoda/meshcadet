// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim --bin space_backdrop_render` — regenerates the
//! `SpaceBackdrop` / `PlanetHorizon` host-sim render deliverable.
//!
//! Writes `docs/renders/space-backdrop-host-sim.png`: one frame of the demo
//! scene `ui_sim::motif_library::SpaceBackdropFrame` renders — full-window
//! dim starfield, a stand-in flat-fill "content" rectangle sitting on top of
//! it (proving the backdrop composites BEHIND foreground content, not over
//! it), and the lower-band `PlanetHorizon` outline. Host-sim renders cannot
//! judge final on-hardware legibility; this is
//! evidence the two new components compile and paint plausible pixels, the
//! same scope `motif_library_render.rs` established for the v2 motif
//! library. Asserted on (non-visually) by
//! `cargo test -p ui_sim --test space_backdrop`.

use std::path::PathBuf;

fn main() {
    let frame = ui_sim::motif_library::SpaceBackdropFrame::new();
    let framebuffer = frame.render();

    let img = ui_sim::motif_library::framebuffer_to_rgb_image(
        &framebuffer,
        ui_sim::motif_library::WIDTH,
        ui_sim::motif_library::HEIGHT,
    );

    let out_path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "docs",
        "renders",
        "space-backdrop-host-sim.png",
    ]
    .iter()
    .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create docs/renders");
    img.save(&out_path).expect("write host-sim render PNG");
    println!("wrote host-sim render: {}", out_path.display());
}
