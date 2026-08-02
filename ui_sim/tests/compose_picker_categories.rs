// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders `ui_sim::compose_picker_probe`'s host-sim copy
//! of `compose.rs`'s `EmojiPickerGrid` (real `protocol::emoji::EMOJI_TABLE`
//! data, 96 entries / 6 category tabs) and proves the picker's touch
//! reachability directly, by simulating real touch events
//! (`WindowEvent::PointerPressed`/`PointerReleased`, the exact events
//! `firmware/src/ui/platform.rs`'s touch-panel driver dispatches for a
//! physical tap) at each cell's actual on-screen coordinates and asserting
//! `emoji_selected` fires with that cell's REAL codepoint.
//!
//! This is `meshcadet-emoji-picker-expansion`'s acceptance predicate "all 96
//! cells are reachable by touch and none are clipped" — re-verified
//! explicitly at the new 96-entry/6-tab shape (per that mission's
//! instruction not to assume the pre-tabs 40-cell Flickable scroll fix
//! scales unchanged), not asserted from the layout math alone. Interaction
//! (does a tap at this coordinate fire the right callback), not rendered
//! pixel content, is the right proof here: the host build has no color-emoji
//! font (see `emoji_blank_cell_probe.rs`'s own doc for why glyph rendering
//! itself isn't host-provable without a hand-built font), but reachability
//! is purely a hit-testing/layout property, which IS faithfully reproduced
//! by the real Slint renderer + real event dispatch on host.
//!
//! # Geometry (see `compose_picker_probe.rs`'s copy of the real markup)
//!
//! Tab strip: 24px tall, 6 equal-width tabs (`horizontal-stretch: 1.0`) each
//! `320px/6 ≈ 53.3px` wide. Grid: `Flickable` at `y: 24px`, `height: 140px`
//! (164 - 24). 16 cells/category = 4 rows (5+5+5+1) at 58×36px + 2px
//! spacing + 4px padding; natural content height 158px > 140px visible, so
//! rows 0-2 are reachable at the DEFAULT scroll position but row 3 (index
//! 15, the sole cell in the last, partial row) requires scrolling — this
//! test scrolls to the bottom before tapping it, exercising the actual
//! Flickable scroll path rather than relying on row 3's center happening to
//! fall just inside the unscrolled visible window (a ~4px margin that would
//! make an unscrolled tap fragile, not a deliberate proof of the scroll
//! mechanism the mission asks to re-verify).
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process) —
//! see `compose_send.rs`'s module doc for the full "why a second render
//! path" rationale, which applies identically here.

use std::cell::RefCell;
use std::rc::Rc;

use protocol::emoji::{EMOJI_CATEGORIES, EMOJI_TABLE};
use ui_sim::compose_picker_probe::EmojiPickerProbeFrame;

/// Center-x of grid column `col` (0..=4) — `4px` padding + `col * (58 + 2)`
/// cell pitch + half the 58px cell width.
fn col_center_x(col: i32) -> f32 {
    4.0 + (col as f32) * 60.0 + 29.0
}

/// Center-y (window-local) of grid row `row` (0..=3) at the given Flickable
/// `viewport_y` shift (0 = scrolled to top, negative = scrolled down).
/// `24px` tab strip + `4px` grid padding + `row * (36 + 2)` row pitch + half
/// the 36px cell height, shifted by the current scroll offset.
fn row_center_y(row: i32, viewport_y_shift: f32) -> f32 {
    24.0 + 4.0 + (row as f32) * 38.0 + 18.0 + viewport_y_shift
}

/// Center-x of tab `i` (0..=5) in the 6-way equal-stretch tab strip —
/// `320px / 6` pitch, centered.
fn tab_center_x(i: i32) -> f32 {
    (320.0 / 6.0) * (i as f32 + 0.5)
}

const TAB_CENTER_Y: f32 = 12.0; // mid of the 24px tab strip

/// The Flickable's max-scroll `viewport-y` shift for a 16-cell category:
/// content height 158px - visible height 140px = 18px scrolled up (negative).
const SCROLLED_VIEWPORT_Y_SHIFT: f32 = -18.0;

/// Single test function — see module doc: exactly one
/// [`EmojiPickerProbeFrame`] (and therefore exactly one Slint `Platform`)
/// may be installed per PROCESS, and `cargo test` runs every `#[test]` fn
/// in one file within the same process (on separate threads, not separate
/// processes) — a second `EmojiPickerProbeFrame::new()` in a sibling
/// `#[test]` fn in this same file would panic with `AlreadySet` regardless
/// of execution order, so both reachability proofs (cell taps, tab taps)
/// live in this one function rather than split across two `#[test]`s (same
/// one-test-per-file idiom `compose_promo.rs`'s own module doc explains).
#[test]
fn every_cell_and_every_tab_is_reachable_by_touch() {
    assert_eq!(
        EMOJI_CATEGORIES.len(),
        6,
        "recalibrate this test if categories change"
    );
    assert_eq!(
        EMOJI_TABLE.len(),
        96,
        "recalibrate this test if EMOJI_TABLE's size changes"
    );

    let frame = EmojiPickerProbeFrame::new();
    let last_selected: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let last_selected_cb = last_selected.clone();
    frame.on_emoji_selected(move |cp| {
        *last_selected_cb.borrow_mut() = Some(cp);
    });

    // ── Part 1: every one of the 96 cells, across all 6 categories ───────
    for (cat_idx, &category) in EMOJI_CATEGORIES.iter().enumerate() {
        let entries: Vec<_> = EMOJI_TABLE
            .iter()
            .filter(|e| e.category == category)
            .collect();
        assert_eq!(
            entries.len(),
            16,
            "category {category:?} has {} entries, want 16",
            entries.len()
        );

        frame.set_active_category(cat_idx as i32);

        // ── Rows 0-2 (indices 0..15) — reachable at the default (top)
        // scroll position. ──────────────────────────────────────────────
        frame.scroll_to_top();
        frame.render();
        for (i, entry) in entries.iter().enumerate().take(15) {
            let row = (i / 5) as i32;
            let col = (i % 5) as i32;
            let (x, y) = (col_center_x(col), row_center_y(row, 0.0));
            *last_selected.borrow_mut() = None;
            frame.tap(x, y);
            frame.render();
            let expected = entry.codepoint.to_string();
            assert_eq!(
                last_selected.borrow().as_deref(),
                Some(expected.as_str()),
                "category {category:?} cell {i} (row {row}, col {col}) at ({x}, {y}) did not \
                 select the expected codepoint {expected:?} — tap missed or hit the wrong cell"
            );
        }

        // ── Row 3 (index 15, the sole cell in the last partial row) —
        // only reachable after scrolling; this is the exact case the
        // mission's acceptance calls out to re-verify explicitly. ────────
        frame.scroll_to_bottom();
        frame.render();
        let (x, y) = (col_center_x(0), row_center_y(3, SCROLLED_VIEWPORT_Y_SHIFT));
        *last_selected.borrow_mut() = None;
        frame.tap(x, y);
        frame.render();
        let expected = entries[15].codepoint.to_string();
        assert_eq!(
            last_selected.borrow().as_deref(),
            Some(expected.as_str()),
            "category {category:?} cell 15 (row 3, scrolled to bottom) at ({x}, {y}) did not \
             select the expected codepoint {expected:?} — the bottom row is unreachable, the \
             exact defect class this screen has a documented history of (see compose.rs's BUG \
             FIXes doc)"
        );
    }

    // ── Part 2: the tab row itself (not just the underlying per-category
    // data swap) — a real tap on tab `i`'s on-screen position switches
    // `active_category` to `i`, for every one of the 6 tabs. ─────────────
    for i in 0..6i32 {
        // Start from a different category so a no-op tap can't pass.
        frame.set_active_category((i + 1) % 6);
        frame.render();
        assert_eq!(frame.get_active_category(), (i + 1) % 6);

        let (x, y) = (tab_center_x(i), TAB_CENTER_Y);
        frame.tap(x, y);
        frame.render();
        assert_eq!(
            frame.get_active_category(),
            i,
            "tapping tab {i} at ({x}, {y}) did not switch active_category to {i}"
        );
    }
}
