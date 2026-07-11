// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim` — regenerates the host-sim render deliverable.
//!
//! Writes `docs/renders/unprovisioned-space-host-sim.png`: one rendered
//! frame proving both the primary (`@image-url` +
//! `SLINT_EMBED_RESOURCES=embed-for-software-renderer`) and fallback
//! (build.rs RGB565 byte array + `SharedPixelBuffer`) asset-embed paths — see
//! `ui_sim/src/lib.rs`'s module doc for the full design. The same render
//! pipeline is asserted on (non-visually) by `cargo test -p ui_sim`.

use std::path::PathBuf;

fn main() {
    let framebuffer = ui_sim::render_host_sim_frame();
    let img = ui_sim::framebuffer_to_rgb_image(&framebuffer, ui_sim::WIDTH, ui_sim::HEIGHT);

    let out_path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "docs", "renders", "unprovisioned-space-host-sim.png"]
        .iter()
        .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create docs/renders");
    img.save(&out_path).expect("write host-sim render PNG");
    println!("wrote host-sim render: {}", out_path.display());
}
