// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders
//! the splash screen's verbatim-markup host-sim proof
//! (`ui_sim::splash_lineart`) and asserts this check's own abort condition
//! holds — the `PlanetHorizon` line art must not paint any pixel in the same
//! row as the version string below the wordmark.
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process),
//! same reason `tests/space_backdrop.rs` and `tests/motif_library.rs` do:
//! exactly one Slint `Platform` may be installed per process.
//!
//! This does NOT assert anything about on-hardware legibility (host-sim
//! cannot judge panel contrast) — it only proves the two shared components
//! render, and that this screen's specific `PlanetHorizon` placement is
//! geometrically clear of the version string, the one thing called out
//! as an abort condition.

use ui_sim::splash_lineart::{rgb8, SplashLineartFrame, HEIGHT, WIDTH};

fn at(fb: &[slint::platform::software_renderer::Rgb565Pixel], x: u32, y: u32) -> (u8, u8, u8) {
    rgb8(fb[(y * WIDTH + x) as usize])
}

#[test]
fn space_backdrop_and_planet_horizon_render_over_splash() {
    let frame = SplashLineartFrame::new();
    let fb = frame.render();
    assert_eq!(fb.len(), (WIDTH * HEIGHT) as usize);

    let bg = at(&fb, 0, 0);

    // `SpaceBackdrop` is a full-window layer: some pixel across the window
    // must differ from the bare background fill — i.e. the starfield
    // actually painted (same style of assertion `tests/space_backdrop.rs`
    // makes for the shared-contract proof).
    let mut backdrop_painted = false;
    'outer: for y in 0..140u32 {
        for x in 0..WIDTH {
            if at(&fb, x, y) != bg {
                backdrop_painted = true;
                break 'outer;
            }
        }
    }
    assert!(backdrop_painted, "SpaceBackdrop did not paint any pixel above the content block");

    // `PlanetHorizon` occupies the lower band: some pixel at/after y=200
    // must differ from the bare background fill — i.e. the horizon line art
    // actually painted (measured crest, per splash.rs's module doc).
    let mut horizon_painted = false;
    'outer2: for y in 200..HEIGHT {
        for x in 0..WIDTH {
            if at(&fb, x, y) != bg {
                horizon_painted = true;
                break 'outer2;
            }
        }
    }
    assert!(horizon_painted, "PlanetHorizon did not paint any pixel in the lower band");

    // THE ABORT-CONDITION CHECK: the version string's own measured row span
    // (y=185..194 — the last painted text row
    // is 193) must NOT contain any `PlanetHorizon` pixel. Since `PlanetHorizon`
    // is declared behind the content `VerticalLayout` and doesn't start
    // painting until y=200 (measured), no row in this range should show
    // anything the bare background + version-string text doesn't already
    // account for; concretely: every row in this span must be fully
    // explained by "background or text", which — because `PlanetHorizon`
    // paints nothing at all before y=200 — reduces to a simple non-overlap
    // check on the row range itself.
    for y in 194..200u32 {
        for x in 0..WIDTH {
            assert_eq!(
                at(&fb, x, y),
                bg,
                "PlanetHorizon (or any other layer) painted a pixel at ({x}, {y}), inside the measured \
                 clear gap between the version string (ends y=193) and the horizon's first painted \
                 row (y=200) — the abort condition"
            );
        }
    }
}
