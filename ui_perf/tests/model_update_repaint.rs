// SPDX-License-Identifier: GPL-3.0-only
//! Measures the repaint scope of a LIVE message-list update to an
//! already-visible screen — the message-view "new message arrived in the open
//! conversation" path — and proves the repaint-scope optimization this fix
//! lands.
//!
//! THE OPTIMIZATION: `firmware/src/ui/screens/message_view.rs::set_messages`
//! used to build a fresh `VecModel` and REPLACE the component's model wholesale
//! (`set_messages(ModelRc::new(fresh))`). Under the firmware's
//! `RepaintBufferType::ReusedBuffer` (dirty-region) renderer, a wholesale model
//! replacement makes the renderer conservatively invalidate the ENTIRE 320x240
//! window (it cannot diff the dropped repeater instances against the new ones)
//! — so the static space backdrop + starfield header are re-flushed over the
//! shared SPI2 bus on EVERY incoming message. The fix reconciles the SAME model
//! in place (push the new row, `set_row_data` a changed row, leave unchanged
//! rows untouched), which dirties only the changed rows.
//!
//! This test proves, against the identical Slint renderer the firmware uses:
//!   1. THE WIN — an in-place append / single-row edit flushes far fewer
//!      scanlines than a wholesale replace of the same final state.
//!   2. PARITY — after the in-place updates, the framebuffer is pixel-for-pixel
//!      identical to a from-scratch wholesale-replace render of the same state
//!      (the scoped flushes painted correctly, no stale/black gap left behind).
//!   3. BEHAVIOR-SAFE — the in-place updates self-dirty and paint without any
//!      help from `request_redraw()`.
//!
//! (Own integration-test binary ⇒ own process ⇒ own Slint platform singleton;
//! Slint allows only ONE platform per process, so this is a single `#[test]`.)

use slint::{ComponentHandle, Model};
use ui_perf::{Harness, Row, HEIGHT};

fn entries(n: usize) -> Vec<Row> {
    (0..n)
        .map(|i| Row { text: format!("message row {i}").into(), ours: i % 2 == 0 })
        .collect()
}

// The exact logical state the in-place sequence below arrives at: 6 rows, with
// row 2 edited to its acked form. Used to build the wholesale-replace control.
fn final_state() -> Vec<Row> {
    let mut v = entries(6);
    v[2] = Row { text: "message row 2 (acked)".into(), ours: true };
    v
}

#[test]
fn in_place_updates_beat_wholesale_replace_and_are_pixel_identical() {
    let mut h = Harness::new();
    let ui = ui_perf::ListScene::new().expect("ListScene::new");

    // Persistent model, reconciled in place — mirrors the fixed
    // `MessageViewScreen::messages_model`.
    let model = std::rc::Rc::new(slint::VecModel::from(entries(5)));
    ui.set_rows(model.clone().into());
    ui.show().expect("show");

    // Settle a 5-row list with a navigation-equivalent full paint.
    h.request_redraw();
    let settle = h.frame().expect("settle frame must paint");
    assert_eq!(settle.lines_flushed, HEIGHT as usize, "settle is a full paint");
    println!(
        "[model] settle (navigation full paint): {} lines, {} px",
        settle.lines_flushed, settle.dirty_pixels
    );

    // ── OPTIMIZED path 1: a new message arrives; push it in place, NO redraw ─
    model.push(Row { text: "message row 5".into(), ours: false });
    let appended = h.frame().expect(
        "BEHAVIOR-SAFE: an in-place model append must self-dirty & paint with \
         no request_redraw() — else the live update would silently stall",
    );
    println!(
        "[model] live append, IN-PLACE (fixed): {} lines, {} px, bbox {}x{}",
        appended.lines_flushed, appended.dirty_pixels, appended.bbox_w, appended.bbox_h
    );

    // ── OPTIMIZED path 2: an ack-state flip on an existing sent message is an
    // in-place single-row edit (what happens when an ACK lands).
    model.set_row_data(2, Row { text: "message row 2 (acked)".into(), ours: true });
    let edited = h.frame().expect("in-place edit must self-dirty & paint");
    println!(
        "[model] live ack-flip, IN-PLACE single-row edit: {} lines, bbox {}x{}",
        edited.lines_flushed, edited.bbox_w, edited.bbox_h
    );
    let hash_in_place = h.framebuffer_hash();

    // ── CONTROL: the OLD firmware behavior — a wholesale model REPLACE with the
    // identical final state. Under ReusedBuffer this conservatively re-flushes
    // the whole window; its pixels are the parity reference.
    ui.set_rows(std::rc::Rc::new(slint::VecModel::from(final_state())).into());
    let replaced = h.frame().expect("wholesale replace must paint");
    let hash_replaced = h.framebuffer_hash();
    println!(
        "[model] same state, WHOLESALE REPLACE (old firmware): {} lines, {} px",
        replaced.lines_flushed, replaced.dirty_pixels
    );

    // PARITY: the in-place-updated framebuffer == the wholesale-replace one.
    assert_eq!(
        hash_in_place, hash_replaced,
        "PARITY FAIL: in-place updates produced different pixels than a wholesale \
         replace of the same final state"
    );

    // THE WIN: each in-place update flushed far fewer scanlines than the replace.
    assert_eq!(
        replaced.lines_flushed,
        HEIGHT as usize,
        "the old wholesale-replace path is a full-window flush"
    );
    for (label, s) in [("append", appended), ("edit", edited)] {
        assert!(
            s.lines_flushed < HEIGHT as usize / 3,
            "in-place {label} flushed {} of {} lines — larger than expected for one row",
            s.lines_flushed,
            HEIGHT
        );
    }

    let saved = replaced.lines_flushed - appended.lines_flushed;
    println!(
        "[model] REPAINT-SCOPE WIN: in-place live update flushes {} lines vs {} \
         (wholesale replace) — {} fewer SPI line-flush cycles/message ({}% \
         reduction); static backdrop + header NOT re-flushed",
        appended.lines_flushed,
        replaced.lines_flushed,
        saved,
        (saved * 100) / replaced.lines_flushed
    );
}
