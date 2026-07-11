// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p ui_sim --bin compose_send_render` — regenerates the
//! compose screen's theme host-sim render deliverable.
//!
//! Writes `docs/renders/compose-space-host-sim.png`: two Send-button states
//! side by side in one frame aren't needed — instead this renders the
//! ARMED state (`has_draft: true`, star-gold background) with the rocket
//! mid-flight (200ms into its 400ms arc-up+fade), the same "capture
//! mid-flight so the render shows motion, not just a settled/empty end
//! state" choice `motif_library_render.rs` makes. See
//! `ui_sim/src/compose_send.rs`'s module doc for the full design; the same
//! render pipeline is asserted on (non-visually) by
//! `cargo test -p ui_sim --test compose_send`.

use std::path::PathBuf;
use std::time::Duration;

fn main() {
    let frame = ui_sim::compose_send::ComposeSendFrame::new();
    frame.set_has_draft(true);
    // Render once at the REST state first — Slint only animates a VALUE
    // CHANGE relative to an already-rendered previous value (same "deferred
    // write" mechanic `EmojiPickerGrid`/`splash.rs` rely on elsewhere in
    // this codebase); triggering before any frame has been rendered would
    // jump straight to the settled end state instead of animating through
    // it, same as a real send-tap always follows an already-visible idle
    // button.
    let _ = frame.render();
    frame.set_rocket_trigger(true);
    std::thread::sleep(Duration::from_millis(200));
    let framebuffer = frame.render();

    let img = ui_sim::compose_send::framebuffer_to_rgb_image(
        &framebuffer,
        ui_sim::compose_send::WIDTH,
        ui_sim::compose_send::HEIGHT,
    );

    let out_path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "docs",
        "renders",
        "compose-space-host-sim.png",
    ]
    .iter()
    .collect();
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create docs/renders");
    img.save(&out_path).expect("write host-sim render PNG");
    println!("wrote host-sim render: {}", out_path.display());
}
