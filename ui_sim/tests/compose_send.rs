// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the
//! Send-button markup through `ui_sim::compose_send` and asserts the
//! `star-gold` affordance + `RocketOnSend` one-shot + auto-reset `Timer`
//! all behave per the design spec.
//!
//! Lives under `tests/` (a separate Cargo integration-test binary, hence
//! its own process) so it can install its own Slint `Platform` singleton
//! without colliding with `lib.rs`'s or `motif_library.rs`'s own — see
//! `motif_library.rs`'s module doc for the full "why a second render path"
//! rationale, which applies identically here.

use std::time::Duration;

use ui_sim::compose_send::{rgb8, ComposeSendFrame, HEIGHT, WIDTH};

/// RGB565 is lossy (5/6/5 bits per channel) — round an 8-bit-per-channel hex
/// color through the same pack/expand path the renderer itself uses, same
/// technique every other `ui_sim` test module uses.
fn quantize565(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let r5 = r >> 3;
    let g6 = g >> 2;
    let b5 = b >> 3;
    (((r5 << 3) | (r5 >> 2)), ((g6 << 2) | (g6 >> 4)), ((b5 << 3) | (b5 >> 2)))
}

fn at(fb: &[slint::platform::software_renderer::Rgb565Pixel], x: u32, y: u32) -> (u8, u8, u8) {
    rgb8(fb[(y * WIDTH + x) as usize])
}

/// Single test — see module doc: exactly one `ComposeSendFrame` (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn send_button_shows_star_gold_and_fires_rocket_on_send() {
    let star_gold = quantize565(0xff, 0xd6, 0x6b);
    let surface_raised = quantize565(0x1e, 0x2a, 0x38);
    let brand_signal = quantize565(0x00, 0xb4, 0xff);

    // Button geometry, mirroring compose.rs's own (see compose_send.rs).
    let btn_x = WIDTH - 80 - 8;
    let btn_y = HEIGHT - 28 - 8;

    let frame = ComposeSendFrame::new();

    // ── Idle state: no draft text, no trigger ──────────────────────────────
    let fb0 = frame.render();
    assert_eq!(fb0.len(), (WIDTH * HEIGHT) as usize);
    assert_eq!(
        at(&fb0, btn_x + 40, btn_y + 14),
        surface_raised,
        "idle Send button should be surface-raised, not star-gold"
    );

    // RocketOnSend REST state: rocket body visible at its home position.
    // `RocketOnSend` is nested inside the 80x28 button at
    // `x: parent.width/2 - self.width/2` (= 40 - 10 = 30), `y: -20px`
    // (see compose_send.rs) — i.e. its own origin sits at
    // (btn_x + 30, btn_y - 20). The rocket body's brand-signal fill sits
    // at a (+6, +11) offset within its 20x24 box (same offset
    // `motif_library.rs`'s own test uses for the identical asset).
    let rocket_home_x = btn_x + 30 + 6;
    let rocket_home_y = btn_y - 20 + 11;
    assert_eq!(
        at(&fb0, rocket_home_x, rocket_home_y),
        brand_signal,
        "RocketOnSend should show the rocket body at rest (play: false)"
    );

    // ── Armed state: draft text present → star-gold affordance ────────────
    frame.set_has_draft(true);
    let fb1 = frame.render();
    assert_eq!(
        at(&fb1, btn_x + 40, btn_y + 14),
        star_gold,
        "armed (draft != \"\") Send button should be star-gold"
    );
    // Rocket still at rest — arming the button alone must not fire it.
    assert_eq!(
        at(&fb1, rocket_home_x, rocket_home_y),
        brand_signal,
        "RocketOnSend should stay at rest until send-tap fires the trigger"
    );

    // ── Send-tap: fire the one-shot ─────────────────────────────────────────
    // Slint's `Timer` only registers a fresh "running" edge (and starts
    // counting its `interval`) the next time `update_timers_and_animations()`
    // actually runs — so render once right after arming to pin the Timer's
    // due-time at (this render's timestamp + 500ms), not some later instant.
    frame.set_rocket_trigger(true);
    let _ = frame.render();
    std::thread::sleep(Duration::from_millis(400)); // well short of the 500ms Timer, clears the 400ms rocket animation
    let fb2 = frame.render();
    assert_ne!(
        at(&fb2, rocket_home_x, rocket_home_y),
        brand_signal,
        "RocketOnSend should have arced up + faded off its home position after settling (play: true)"
    );

    // ── Auto-reset: the sibling Timer clears rocket_trigger ~500ms after
    // arming, which is itself a VALUE CHANGE that plays the 400ms
    // rocket_y/rocket_opacity `animate` transitions in REVERSE back to the
    // rest state — so the pixel doesn't actually settle back to
    // brand_signal until ~500ms (Timer) + 400ms (reverse animation) =
    // ~900ms since arm. Poll rather than sleep-a-fixed-amount-once: more
    // robust to scheduler jitter than betting the whole assertion on one
    // exact sleep duration, same reasoning `frame.render()`'s own
    // `draw_if_needed` "was it dirty" checks already apply elsewhere in
    // this crate.
    let mut settled = false;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        let fbx = frame.render();
        if at(&fbx, rocket_home_x, rocket_home_y) == brand_signal {
            settled = true;
            break;
        }
    }
    assert!(
        settled,
        "rocket_trigger should have auto-reset to false, settling the rocket back to its rest position within 2s"
    );
}
