// SPDX-License-Identifier: GPL-3.0-only
//! Integration test:
//! renders `firmware/src/ui/motifs.slint`'s new `SpaceBackdrop` /
//! `PlanetHorizon` components through `ui_sim::motif_library` and asserts
//! both actually compile and paint non-blank pixels.
//!
//! Lives under `tests/` (its own Cargo integration-test binary / process)
//! for the same reason `tests/motif_library.rs` does: exactly one Slint
//! `Platform` may be installed per process, and this crate's `lib.rs`
//! `#[cfg(test)]` module + `tests/motif_library.rs` each already claim one.
//!
//! This test does NOT assert anything about on-hardware legibility (host-sim
//! cannot judge panel contrast);
//! it only proves the shared-asset contract renders at all, same scope
//! `tests/motif_library.rs` established for the v2 motif library.

use ui_sim::motif_library::{rgb8, SpaceBackdropFrame, HEIGHT, WIDTH};

fn at(fb: &[slint::platform::software_renderer::Rgb565Pixel], x: u32, y: u32) -> (u8, u8, u8) {
    rgb8(fb[(y * WIDTH + x) as usize])
}

/// RGB565 is lossy — round an 8-bit color through the same pack/expand path
/// the renderer itself uses (same technique `tests/motif_library.rs` uses).
fn quantize565(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let r5 = r >> 3;
    let g6 = g >> 2;
    let b5 = b >> 3;
    (
        ((r5 << 3) | (r5 >> 2)),
        ((g6 << 2) | (g6 >> 4)),
        ((b5 << 3) | (b5 >> 2)),
    )
}

#[test]
fn space_backdrop_and_planet_horizon_render() {
    let space_deep = quantize565(0x07, 0x0a, 0x12);
    let bg_space = quantize565(0x0d, 0x11, 0x17);

    let frame = SpaceBackdropFrame::new();
    let fb = frame.render();
    assert_eq!(fb.len(), (WIDTH * HEIGHT) as usize);

    // `SpaceBackdrop` is a full-window layer: SOME pixel across the window
    // (excluding the foreground rectangle and the lower planet-horizon
    // band, both asserted separately below) must differ from the bare
    // `space-deep` window fill — i.e. the starfield actually painted.
    let mut backdrop_painted = false;
    for y in 0..168u32 {
        for x in 0..WIDTH {
            if (40..280).contains(&x) && (100..140).contains(&y) {
                continue; // the stand-in foreground rectangle's own box
            }
            if at(&fb, x, y) != space_deep {
                backdrop_painted = true;
            }
        }
    }
    assert!(
        backdrop_painted,
        "SpaceBackdrop did not paint any pixel over the space-deep fill"
    );

    // The stand-in foreground "content" rectangle (bg-space fill) must
    // still read as its own flat color at its center — i.e. the backdrop
    // sits BEHIND it, not composited on top.
    assert_eq!(
        at(&fb, 160, 120),
        bg_space,
        "foreground content rectangle should read as a flat bg-space fill, not blended with the backdrop"
    );

    // `PlanetHorizon` occupies the lower band (y=168..240) — some pixel in
    // that band must differ from the bare space-deep fill.
    let mut horizon_painted = false;
    for y in 168..HEIGHT {
        for x in 0..WIDTH {
            if at(&fb, x, y) != space_deep {
                horizon_painted = true;
                break;
            }
        }
        if horizon_painted {
            break;
        }
    }
    assert!(
        horizon_painted,
        "PlanetHorizon did not paint any pixel in the lower band"
    );
}
