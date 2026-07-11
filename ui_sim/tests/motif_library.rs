// SPDX-License-Identifier: GPL-3.0-only
//! Integration test (M2): renders
//! `firmware/src/ui/motifs.slint`'s full component set through
//! `ui_sim::motif_library` and asserts non-blank / correctly-triggered
//! pixels for every static asset and every one-shot motion helper.
//!
//! This lives under `tests/` (a SEPARATE Cargo integration-test binary,
//! hence its own process) specifically so it can install its own Slint
//! `Platform` singleton without colliding with `lib.rs`'s `#[cfg(test)]`
//! module, which does the same for M1's `HostSimUi` — see both modules'
//! doc comments for the full "why a second render path" rationale.

use std::time::Duration;

use ui_sim::motif_library::{rgb8, MotifLibraryFrame, HEIGHT, WIDTH};

/// RGB565 is lossy (5/6/5 bits per channel) — round an 8-bit-per-channel hex
/// color through the SAME pack/expand path the renderer itself uses, same
/// technique `lib.rs`'s own test module already established.
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

fn at(fb: &[slint::platform::software_renderer::Rgb565Pixel], x: u32, y: u32) -> (u8, u8, u8) {
    rgb8(fb[(y * WIDTH + x) as usize])
}

/// Any pixel in `[x0, x1) x [y0, y1)` that differs from `bg`.
fn region_has_non_bg(
    fb: &[slint::platform::software_renderer::Rgb565Pixel],
    x0: u32,
    x1: u32,
    y0: u32,
    y1: u32,
    bg: (u8, u8, u8),
) -> bool {
    (x0..x1).any(|x| (y0..y1).any(|y| at(fb, x, y) != bg))
}

/// Single test — see module doc: exactly one `MotifLibraryFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process,
/// and `cargo test` runs `#[test]` functions as threads within ONE process
/// per test BINARY, so this must be the only test in this file.
#[test]
fn motif_library_renders_every_asset_and_motion_helper() {
    let space_deep = quantize565(0x07, 0x0a, 0x12);
    let brand_signal = quantize565(0x00, 0xb4, 0xff);
    let nebula_violet_deep = quantize565(0x3a, 0x2a, 0x6b);
    let star_gold = quantize565(0xff, 0xd6, 0x6b);
    let star_white = quantize565(0xf4, 0xf7, 0xff);

    let frame = MotifLibraryFrame::new();

    // ── t≈0: statics + the two motion helpers' REST states ────────────────
    let fb0 = frame.render();
    assert_eq!(fb0.len(), (WIDTH * HEIGHT) as usize);

    // Background is visible somewhere nothing else covers.
    assert_eq!(
        at(&fb0, WIDTH - 4, 4),
        space_deep,
        "space-deep backdrop missing"
    );

    // Starfield header strip (0,0)-(320,40).
    assert!(
        region_has_non_bg(&fb0, 0, 320, 0, 40, space_deep),
        "Starfield region is blank"
    );

    // RingedPlanetCorner at (8,44), 40x40 — same offset technique M1's own
    // test uses (near-but-not-exact center, avoiding the terminator overlay).
    assert_ne!(
        at(&fb0, 8 + 20, 44 + 20),
        space_deep,
        "RingedPlanetCorner did not paint"
    );

    // CrescentMoon at (60,50), 28x28 — a point inside the un-erased sliver.
    assert_ne!(
        at(&fb0, 60 + 4, 50 + 14),
        space_deep,
        "CrescentMoon did not paint"
    );

    // Static Comet at (104,58): opaque star-gold head, unmodified by the
    // small glint circle at this exact point.
    assert_eq!(
        at(&fb0, 104 + 21, 58 + 7),
        star_gold,
        "static Comet head is not star-gold"
    );

    // Static Rocket at (148,44): opaque star-white window, drawn last.
    assert_eq!(
        at(&fb0, 148 + 10, 44 + 11),
        star_white,
        "static Rocket window is not star-white"
    );

    // Mascot poses: visor color at each pose's own helmet-center offset.
    assert_eq!(
        at(&fb0, 4 + 32, 92 + 25),
        brand_signal,
        "CadetIdle visor is not brand-signal"
    );
    assert_eq!(
        at(&fb0, 72 + 32, 92 + 25),
        brand_signal,
        "CadetWave visor is not brand-signal"
    );
    assert_eq!(
        at(&fb0, 140 + 32, 92 + 25),
        brand_signal,
        "CadetThumbsUp visor is not brand-signal"
    );
    // CadetSleeping: visor is DIMMED (nebula-violet-deep, not brand-signal) —
    // the pose-specific differentiator.
    assert_eq!(
        at(&fb0, 208 + 32, 92 + 25),
        nebula_violet_deep,
        "CadetSleeping visor should be dimmed nebula-violet-deep, not lit brand-signal"
    );
    assert_eq!(
        at(&fb0, 4 + 32, 160 + 41),
        brand_signal,
        "CadetPeeking visor is not brand-signal"
    );

    // RocketOnSend REST state: rocket visible at its home position (288,160).
    assert_eq!(
        at(&fb0, 288 + 6, 160 + 11),
        brand_signal,
        "RocketOnSend should show the rocket body at rest (play: false)"
    );

    // CometOnNotify REST state: parked fully off-canvas — no comet-teal or
    // star-gold pixel anywhere in its header band.
    let comet_band_clear_at_rest = !region_has_non_bg(&fb0, 0, 320, 226, 240, space_deep);
    assert!(
        comet_band_clear_at_rest,
        "CometOnNotify should be off-canvas (play: false)"
    );

    // ── t≈1000ms: mount-time one-shots (MascotBob 450ms, Twinkle 900ms) settle ──
    std::thread::sleep(Duration::from_millis(1000));
    let fb1 = frame.render();

    // MascotBob at (100,160), default cadet_idle pose, settled (bob_y: 0px).
    assert_eq!(
        at(&fb1, 100 + 32, 160 + 25),
        brand_signal,
        "MascotBob (settled) visor is not brand-signal"
    );

    // Twinkle at (280,170), 3x3, settled to full star-gold opacity.
    assert_ne!(
        at(&fb1, 280 + 1, 170 + 1),
        space_deep,
        "Twinkle (settled) did not paint"
    );

    // ── Trigger the two retriggerable one-shots ────────────────────────────
    frame.set_send_trigger(true);
    frame.set_notify_trigger(true);

    // t≈300ms after trigger: comet is mid-sweep (50% of its 600ms duration) —
    // must now be visible SOMEWHERE in its band, having been absent at rest.
    std::thread::sleep(Duration::from_millis(300));
    let fb2 = frame.render();
    let comet_visible_mid_flight = region_has_non_bg(&fb2, 0, 320, 226, 240, space_deep);
    assert!(
        comet_visible_mid_flight,
        "CometOnNotify did not become visible after play: true"
    );

    // t≈700ms after trigger: both RocketOnSend (400ms) and CometOnNotify
    // (600ms) have fully settled to their "fired" end states.
    std::thread::sleep(Duration::from_millis(400));
    let fb3 = frame.render();
    assert_ne!(
        at(&fb3, 288 + 6, 160 + 11),
        brand_signal,
        "RocketOnSend should have moved off its home position after settling (play: true)"
    );
}
