// SPDX-License-Identifier: GPL-3.0-only
//! Measures the per-frame repaint scope of a foreground motif animation
//! (CometOnNotify sweep) rendered OVER a static space backdrop + starfield,
//! through the real ReusedBuffer dirty-region renderer.
//!
//! FINDING THIS TEST PROVES: the space theme does NOT have a "full-window
//! animated backdrop" repaint problem. The backdrop layers (window fill +
//! Starfield + planet corner) are STATIC; only the foreground motif animates,
//! and Slint's ReusedBuffer scopes each animation frame's flush to that
//! motif's motion band — the static backdrop is painted ONCE (navigation) and
//! never re-flushed while the motif moves. So the provisional
//! "prime-lever = full-window backdrop" hypothesis is DEMOTED by measurement:
//! the repaint scope of the animations is already near-minimal.
//!
//! (Own integration-test binary ⇒ own process ⇒ own Slint platform singleton.)

use slint::ComponentHandle;
use ui_perf::{Harness, HEIGHT};

#[test]
fn comet_sweep_flushes_a_narrow_band_not_the_full_window() {
    let mut h = Harness::new();
    let ui = ui_perf::MotifScene::new().expect("MotifScene::new");
    ui.show().expect("show");

    // Frame 0: navigation-equivalent full paint (backdrop + starfield settle).
    h.request_redraw();
    let settle = h.frame().expect("settle frame must paint");
    assert_eq!(
        settle.lines_flushed, HEIGHT as usize,
        "the first (navigation) frame is a full-window paint by design"
    );
    println!(
        "[motif] settle (navigation full paint): {} lines, {} px, bbox {}x{}",
        settle.lines_flushed, settle.dirty_pixels, settle.bbox_w, settle.bbox_h
    );

    // Fire the comet sweep and sample its animation frames.
    ui.set_notify_trigger(true);

    let mut max_lines = 0usize;
    let mut max_bbox_h = 0u32;
    let mut animated_frames = 0usize;
    // 600 ms sweep; sample every 16 ms (~60 fps), a few frames past the end.
    for _ in 0..45 {
        h.advance(16);
        if let Some(s) = h.frame() {
            animated_frames += 1;
            max_lines = max_lines.max(s.lines_flushed);
            max_bbox_h = max_bbox_h.max(s.bbox_h);
        }
    }

    println!(
        "[motif] comet sweep: {} animated frames, worst-frame {} lines flushed, tallest bbox {}px",
        animated_frames, max_lines, max_bbox_h
    );

    assert!(
        animated_frames > 0,
        "the comet sweep must produce at least one dirty animation frame"
    );
    // The comet is a 14px-tall motif near the top. Even the worst animation
    // frame must flush FAR fewer lines than a full window — the static backdrop
    // underneath is never re-flushed by the motif's motion.
    assert!(
        max_lines < HEIGHT as usize / 3,
        "a foreground-motif animation frame flushed {} of {} lines — the static \
         backdrop is being needlessly re-flushed (repaint scope NOT minimal)",
        max_lines,
        HEIGHT
    );
    assert!(
        max_bbox_h < HEIGHT,
        "motif animation dirty bbox spanned the full window height ({}px)",
        max_bbox_h
    );
}
