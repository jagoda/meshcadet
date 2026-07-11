// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim --bin gps_status_rows_render` — regenerates the
//! GPS status screen's theme host-sim render deliverable.
//!
//! Writes `docs/renders/gps-status-space-host-sim.png`: the three-row
//! stack (`Fix` w/ comet, `Satellites` w/ no icon, `Coordinates` w/ ringed
//! planet) in its one static (no animation/trigger) frame. See
//! `ui_sim/src/gps_status_rows.rs`'s module doc for the full design; the
//! same render pipeline is asserted on (non-visually) by
//! `cargo test -p ui_sim --test gps_status_rows`.

use std::path::PathBuf;

fn main() {
    let frame = ui_sim::gps_status_rows::GpsStatusRowsFrame::new();
    let framebuffer = frame.render();

    let img = ui_sim::gps_status_rows::framebuffer_to_rgb_image(
        &framebuffer,
        ui_sim::gps_status_rows::WIDTH,
        ui_sim::gps_status_rows::HEIGHT,
    );

    let out_path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "docs",
        "renders",
        "gps-status-space-host-sim.png",
    ]
    .iter()
    .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create docs/renders");
    img.save(&out_path).expect("write host-sim render PNG");
    println!("wrote host-sim render: {}", out_path.display());
}
