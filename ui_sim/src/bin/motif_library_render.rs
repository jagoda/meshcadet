// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim --bin motif_library_render` — regenerates the
//! motif-library host-sim render deliverable.
//!
//! Writes `docs/renders/motif-library-host-sim.png`: one frame with every
//! static celestial/mascot component from `firmware/src/ui/motifs.slint`
//! laid out, PLUS `RocketOnSend`/`CometOnNotify` triggered and given time to
//! settle so both motion helpers show their fired (not just rest) state.
//! See `ui_sim/src/motif_library.rs`'s module doc for the full design; the
//! same render pipeline is asserted on (non-visually) by
//! `cargo test -p ui_sim --test motif_library`.

use std::path::PathBuf;
use std::time::Duration;

fn main() {
    let frame = ui_sim::motif_library::MotifLibraryFrame::new();
    // Settle the mount-time one-shots (MascotBob 450ms, Twinkle 900ms).
    std::thread::sleep(Duration::from_millis(1000));
    let _ = frame.render();

    // Fire the two event-triggered motion helpers and capture them
    // mid-flight (well short of their 400ms/600ms durations) so the render
    // shows the rocket arced up and the comet part-way across, not just
    // the (visually empty) settled end states.
    frame.set_send_trigger(true);
    frame.set_notify_trigger(true);
    std::thread::sleep(Duration::from_millis(200));
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
        "motif-library-host-sim.png",
    ]
    .iter()
    .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create docs/renders");
    img.save(&out_path).expect("write host-sim render PNG");
    println!("wrote host-sim render: {}", out_path.display());
}
