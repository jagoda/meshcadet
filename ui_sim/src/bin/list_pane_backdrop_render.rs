// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim --bin list_pane_backdrop_render` — regenerates the
//! list/message-pane starfield backdrop host-sim render
//! deliverable.
//!
//! Writes `docs/renders/list-pane-backdrop-host-sim.png`: one frame of
//! `ui_sim::list_pane_backdrop::ListPaneBackdropFrame` with its overlays
//! shown — full-window dim starfield, a translucent list row, a transparent
//! content pane, and a translucent bottom action bar hosting an opaque
//! button pill. Host-sim renders cannot judge final on-hardware legibility;
//! this is evidence the new layering composites
//! plausibly, same scope every other `ui_sim` render module's own doc
//! establishes. Asserted on (non-visually) by
//! `cargo test -p ui_sim --test list_pane_backdrop`.

use std::path::PathBuf;

fn main() {
    let frame = ui_sim::list_pane_backdrop::ListPaneBackdropFrame::new();
    frame.set_show_overlays(true);
    let framebuffer = frame.render();

    let img = ui_sim::list_pane_backdrop::framebuffer_to_rgb_image(
        &framebuffer,
        ui_sim::list_pane_backdrop::WIDTH,
        ui_sim::list_pane_backdrop::HEIGHT,
    );

    let out_path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "docs", "renders", "list-pane-backdrop-host-sim.png"]
        .iter()
        .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create docs/renders");
    img.save(&out_path).expect("write host-sim render PNG");
    println!("wrote host-sim render: {}", out_path.display());
}
