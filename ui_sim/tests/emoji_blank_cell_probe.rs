// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders a VS16-suffixed heart through
//! `ui_sim::emoji_blank_cell_probe`'s hand-built minimal `BitmapFont` (see
//! that module's doc for the full mechanism) BOTH raw and normalized via
//! `protocol::emoji::normalize_inbound`, and asserts the normalized render
//! has NO blank cell between the heart and the following marker glyph —
//! this mission's `ui_sim` acceptance predicate.
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process) —
//! see `signal_meter.rs`'s module doc for the full "why a second render
//! path" rationale, which applies identically here. Both renders (raw and
//! normalized) happen on ONE `EmojiBlankCellProbeFrame` within this single
//! `#[test]` fn, since Slint enforces one `Platform` per process and this
//! file's process hosts exactly one test function.

use ui_sim::emoji_blank_cell_probe::{
    classify_column, framebuffer_to_rgb_image, ColumnKind, EmojiBlankCellProbeFrame, HEIGHT, WIDTH,
};

/// Android's ❤️ — `U+2764 U+FE0F` — followed by the probe's `X` marker
/// glyph, exactly as it would arrive concatenated with more message text.
const RAW_WIRE_TEXT: &str = "\u{2764}\u{FE0F}X";

#[test]
fn normalized_heart_has_no_blank_cell_before_the_next_glyph() {
    let frame = EmojiBlankCellProbeFrame::new();

    // ── Control: the RAW (unnormalized) wire text reproduces the live
    // defect — a visible blank cell between the heart and the marker,
    // because this probe font (like the real bundled one) has no entry for
    // VS16. This is the failure mode `normalize_inbound` exists to fix; it
    // is asserted here so the fixed case below is contrasted against a
    // demonstrated-real defect, not an assumption.
    frame.set_line(RAW_WIRE_TEXT);
    let raw_fb = frame.render();
    let raw_img = framebuffer_to_rgb_image(&raw_fb, WIDTH, HEIGHT);

    assert_eq!(
        classify_column(&raw_img, 0, HEIGHT),
        ColumnKind::Heart,
        "raw render: heart glyph must paint starting at column 0"
    );
    assert_eq!(
        classify_column(&raw_img, 16, HEIGHT),
        ColumnKind::Blank,
        "raw render: column 16 must be a BLANK cell (VS16 has no glyph in \
         this probe font, same gap as the real bundled font) — this is the \
         live defect normalize_inbound fixes"
    );
    let raw_marker_col = (0..WIDTH)
        .find(|&x| classify_column(&raw_img, x, HEIGHT) == ColumnKind::Marker)
        .expect("raw render: marker glyph 'X' must paint somewhere");
    assert_eq!(
        raw_marker_col, 32,
        "raw render: marker must start at column 32 (16px heart + 16px \
         blank VS16 cell)"
    );

    // ── Fix: the NORMALIZED text (VS16 stripped by
    // `protocol::emoji::normalize_inbound`, exactly as
    // `firmware_core::ui::message_view::build_message_items` applies it on
    // the render path) must paint the marker IMMEDIATELY after the heart —
    // no blank cell.
    let mut normalized_buf = [0u8; 16];
    let n = protocol::emoji::normalize_inbound(RAW_WIRE_TEXT.as_bytes(), &mut normalized_buf)
        .expect("normalize_inbound must not overflow a 16-byte buffer for this input");
    let normalized = core::str::from_utf8(&normalized_buf[..n]).unwrap();
    assert_eq!(
        normalized, "\u{2764}X",
        "sanity: normalize_inbound dropped VS16"
    );

    frame.set_line(normalized);
    let norm_fb = frame.render();
    let norm_img = framebuffer_to_rgb_image(&norm_fb, WIDTH, HEIGHT);

    assert_eq!(
        classify_column(&norm_img, 0, HEIGHT),
        ColumnKind::Heart,
        "normalized render: heart glyph must still paint starting at column 0"
    );
    let norm_marker_col = (0..WIDTH)
        .find(|&x| classify_column(&norm_img, x, HEIGHT) == ColumnKind::Marker)
        .expect("normalized render: marker glyph 'X' must paint somewhere");
    assert_eq!(
        norm_marker_col, 16,
        "normalized render: marker must start immediately at column 16 — \
         NO blank cell between the heart and the marker"
    );
    // Restated directly in the vocabulary of the acceptance predicate: every
    // column between the two glyphs is painted (heart), none of them are
    // blank.
    for x in 0..16 {
        assert_eq!(
            classify_column(&norm_img, x, HEIGHT),
            ColumnKind::Heart,
            "normalized render: column {x} must not be a blank cell"
        );
    }
}
