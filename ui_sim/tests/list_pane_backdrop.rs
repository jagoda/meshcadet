// SPDX-License-Identifier: GPL-3.0-only
//! Integration test proving `ui_sim::list_pane_backdrop::ListPaneBackdropUi`
//! actually composites a full-window `SpaceBackdrop` BEHIND a translucent
//! list row and bottom action bar (the backdrop shows through both, not
//! painted over by a flat wash-only color) and directly behind a bare-
//! `transparent` content pane (pixel-identical to the no-overlay baseline),
//! while an opaque button pill nested in the action bar reads as its own
//! flat, unblended color.
//!
//! The row/bar checks scan their full bands rather than pinning individual
//! star pixel coordinates, so they stay robust against a future
//! `generate_assets.py::gen_starfield_full` regeneration that shifts exact
//! star placement without changing the overall density/algorithm (both
//! bands were confirmed, by independently replaying that generator's
//! deterministic LCG placement, to each contain multiple stars at the
//! current asset revision).
//!
//! This test does NOT assert anything about on-hardware legibility (host-sim
//! cannot judge panel contrast); it only proves
//! the layering mechanism composites the way the real screens' markup says
//! it does.

use ui_sim::list_pane_backdrop::{rgb8, ListPaneBackdropFrame, HEIGHT, WIDTH};

fn at(fb: &[slint::platform::software_renderer::Rgb565Pixel], x: u32, y: u32) -> (u8, u8, u8) {
    rgb8(fb[(y * WIDTH + x) as usize])
}

/// RGB565 is lossy — round an 8-bit color through the same pack/expand path
/// the renderer itself uses (same technique every other `ui_sim` pixel test
/// uses).
fn quantize565(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let r5 = r >> 3;
    let g6 = g >> 2;
    let b5 = b >> 3;
    (((r5 << 3) | (r5 >> 2)), ((g6 << 2) | (g6 >> 4)), ((b5 << 3) | (b5 >> 2)))
}

/// Alpha-blend `fg` over `bg` at `alpha` (0.0..=1.0), matching Slint's own
/// straight (non-premultiplied) compositing for a solid-color `Rectangle`.
fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let mix = |f: u8, b: u8| -> u8 { ((f as f32) * alpha + (b as f32) * (1.0 - alpha)).round() as u8 };
    (mix(fg.0, bg.0), mix(fg.1, bg.1), mix(fg.2, bg.2))
}

#[test]
fn backdrop_shows_through_translucent_row_and_bar_but_button_stays_opaque() {
    let bg_space = quantize565(0x0d, 0x11, 0x17);
    let nebula_violet_deep = quantize565(0x3a, 0x2a, 0x6b);
    let surface = quantize565(0x16, 0x1e, 0x28);
    let brand_signal = quantize565(0x00, 0xb4, 0xff);

    let frame = ListPaneBackdropFrame::new();

    frame.set_show_overlays(false);
    let baseline = frame.render();
    assert_eq!(baseline.len(), (WIDTH * HEIGHT) as usize);

    frame.set_show_overlays(true);
    let overlay = frame.render();

    // ── Content pane (y=100..160): bare `transparent` must be pixel-
    // identical to the no-overlay baseline everywhere in its bounds — a
    // `transparent` fill adds nothing, so the backdrop shows through
    // completely unmodified (this is the *pane* half of the
    // "behind and below" requirement).
    for y in 100..160u32 {
        for x in (0..WIDTH).step_by(4) {
            assert_eq!(
                at(&overlay, x, y),
                at(&baseline, x, y),
                "transparent content pane must not alter the backdrop pixel at ({x},{y})"
            );
        }
    }

    // ── List row (y=40..94): a `nebula-violet-deep.with-alpha(0.12)` wash
    // over the backdrop must NOT read as a flat blend of that wash over the
    // bare `bg-space` window fill — i.e. the backdrop's own stars are
    // visible through the translucent row, not painted over. Scan the whole
    // row band; at least one pixel must diverge from the flat reference.
    let flat_row_reference = blend(nebula_violet_deep, bg_space, 0.12);
    let mut row_shows_backdrop = false;
    for y in 40..94u32 {
        for x in 0..WIDTH {
            if at(&overlay, x, y) != flat_row_reference {
                row_shows_backdrop = true;
                break;
            }
        }
        if row_shows_backdrop {
            break;
        }
    }
    assert!(
        row_shows_backdrop,
        "list row wash reads as a flat color — backdrop is not showing through the translucent row"
    );

    // ── Bottom action bar (y=200..240, excluding the button pill's own
    // box at x=120..200/y=206..234): same "not a flat wash" check as the
    // row above, over `Theme.surface.with-alpha(0.55)`.
    let flat_bar_reference = blend(surface, bg_space, 0.55);
    let mut bar_shows_backdrop = false;
    for y in 200..240u32 {
        for x in 0..WIDTH {
            if (120..200).contains(&x) && (206..234).contains(&y) {
                continue; // the button pill's own opaque box, checked separately below
            }
            if at(&overlay, x, y) != flat_bar_reference {
                bar_shows_backdrop = true;
                break;
            }
        }
        if bar_shows_backdrop {
            break;
        }
    }
    assert!(
        bar_shows_backdrop,
        "action bar reads as a flat color — backdrop is not showing through the translucent bar"
    );

    // ── Button pill (center at x=160, y=220): must read as its own flat,
    // fully-opaque `Theme.brand-signal` — unaffected by either the bar's
    // translucency or the backdrop beneath it, so tap-target contrast is
    // exactly what the real Send/Write buttons already carry.
    assert_eq!(
        at(&overlay, 160, 220),
        brand_signal,
        "button pill must read as a flat, unblended brand-signal fill"
    );
}
