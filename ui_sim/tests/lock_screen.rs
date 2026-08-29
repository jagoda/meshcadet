// SPDX-License-Identifier: GPL-3.0-only
//! Integration test: renders the screen-lock overlay's four named states
//! (locked / wrong-PIN / backing-off / unread-badge —
//! `meshcadet-lock-firmware-ui`'s acceptance criteria) through
//! `ui_sim::lock_screen` and asserts each state's own distinguishing pixel
//! content actually paints, and does NOT bleed into the other states.
//!
//! Lives under `tests/` (a separate Cargo integration-test binary, hence
//! its own process) so it can install its own Slint `Platform` singleton
//! without colliding with `lib.rs`'s, `gps_status_rows.rs`'s, etc. — see
//! `gps_status_rows.rs`'s own module doc for the full "why a second render
//! path" rationale, which applies identically here. All four states are
//! rendered from the SAME [`LockScreenFrame`] (one `#[test]`, per the
//! "exactly one platform per process" constraint), matching how the real
//! `LockScreen` wrapper is actually driven — see `lock_screen.rs`'s module
//! doc.

//! # Emoji glyphs are out of scope for this test, even though the rig now
//! # registers the real device font
//!
//! `ui_sim::lock_screen::LockScreenFrame::new` now calls
//! `ui_sim::register_device_font` (mechanized 2026-08-29 — see
//! `build.rs::lint_font_provisioning`),
//! registering the SAME real on-device `MeshCadetEmoji` bitmap font
//! `firmware/src/ui/platform.rs::TDeckPlatform::install` does, so the
//! "🔒"/"✉"/"⏳" emoji codepoints this screen uses now actually render here
//! rather than silently blanking. This test still deliberately never
//! asserts on an emoji glyph's own pixels, though: per-codepoint glyph
//! coverage against the curated `gen_emoji_font.c` tables is proven
//! separately by `xtask`'s glyph-coverage harness, not by rendering this
//! rig. Every assertion below therefore targets a `Rectangle` fill color
//! (the reject dots, the badge pill) or a plain-ASCII `Text` color
//! (`lockout_text`, "Try again in Ns").

use ui_sim::lock_screen::{
    rgb8, LockScreenFrame, DOT_ROW_HEIGHT, LOWER_PANEL_HEIGHT, TITLE_HEIGHT, WIDTH,
};

/// RGB565 is lossy (5/6/5 bits per channel) — round an 8-bit-per-channel hex
/// color through the same pack/expand path the renderer itself uses, same
/// technique every other `ui_sim` test module uses.
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

/// Whether `color` appears anywhere within `[y0, y1)` across the full width.
/// Reliable for a SOLID `Rectangle` fill (badge pill, reject dots) — those
/// paint one flat color across their whole area, with no blending.
fn range_contains(
    fb: &[slint::platform::software_renderer::Rgb565Pixel],
    y0: u32,
    y1: u32,
    color: (u8, u8, u8),
) -> bool {
    (0..WIDTH).any(|x| (y0..y1).any(|y| at(fb, x, y) == color))
}

/// Count of pixels matching `color` within `[y0, y1)`. Text glyph
/// antialiasing blends a continuous gradient between foreground and
/// background at a stroke's edges, so a handful of blended pixels can land
/// on the EXACT quantized foreground color by chance even where the glyph
/// itself never actually rendered (confirmed empirically: up to a few
/// stray matches from unrelated numpad-glyph antialiasing). Text color
/// checks below therefore use a MINIMUM COUNT threshold, not mere
/// presence, to distinguish "the text actually rendered" from that noise
/// floor.
fn count_matches(
    fb: &[slint::platform::software_renderer::Rgb565Pixel],
    y0: u32,
    y1: u32,
    color: (u8, u8, u8),
) -> usize {
    (0..WIDTH)
        .flat_map(|x| (y0..y1).map(move |y| (x, y)))
        .filter(|&(x, y)| at(fb, x, y) == color)
        .count()
}

/// Empirically well clear of the antialiasing noise floor (observed at most
/// 3 stray matches from unrelated glyph edges) and well under a real
/// rendered text line's own solid-color pixel count (observed 20+ for
/// "Try again in 23s" at `Theme.size-body-lg`).
const TEXT_MATCH_THRESHOLD: usize = 10;

/// Single test — see module doc: exactly one [`LockScreenFrame`] (and
/// therefore exactly one Slint `Platform`) may be installed per process.
#[test]
fn lock_screen_renders_its_four_named_states() {
    let alert = quantize565(0xff, 0x00, 0x00); // Theme.alert — wrong-PIN reject cue
    let text_secondary = quantize565(0xa0, 0xa8, 0xb0); // Theme.text-secondary — lockout_text ("Try again in Ns")
    let surface_raised = quantize565(0x1e, 0x2a, 0x38); // Theme.surface-raised — badge pill / unfilled dot
    let surface = quantize565(0x16, 0x1e, 0x28); // Theme.surface — numpad button fill
    let bg_space = quantize565(0x0d, 0x11, 0x17); // Theme.bg-space — dot-row/backoff-panel background

    let title_y1 = TITLE_HEIGHT;
    let dot_row_y0 = TITLE_HEIGHT;
    let dot_row_y1 = TITLE_HEIGHT + DOT_ROW_HEIGHT;
    let lower_panel_y0 = TITLE_HEIGHT + DOT_ROW_HEIGHT;
    let lower_panel_y1 = TITLE_HEIGHT + DOT_ROW_HEIGHT + LOWER_PANEL_HEIGHT;

    let frame = LockScreenFrame::new();

    // ── State 1: locked (resting state) ─────────────────────────────────
    frame.set_state_locked();
    let fb = frame.render();
    assert_eq!(fb.len(), (WIDTH * ui_sim::lock_screen::HEIGHT) as usize);

    assert!(
        !range_contains(&fb, dot_row_y0, dot_row_y1, alert),
        "resting locked state must show no reject-cue red in the dot row"
    );
    assert!(
        !range_contains(&fb, 0, title_y1, surface_raised),
        "resting locked state (unread_count=0) must show no badge pill in the title bar"
    );
    assert!(
        count_matches(&fb, lower_panel_y0, lower_panel_y1, text_secondary) < TEXT_MATCH_THRESHOLD,
        "resting locked state must show the numpad, not the backoff countdown's lockout_text"
    );
    // Sanity: the numpad itself actually painted something (buttons are
    // Theme.surface, distinct from the Theme.bg-space dot-row/backoff-panel
    // background used at zero digits/no backoff).
    assert!(
        range_contains(&fb, lower_panel_y0, lower_panel_y1, surface),
        "resting locked state's numpad buttons (Theme.surface) should be visible"
    );

    // ── State 2: wrong-PIN (transient reject cue — NOT the border flash) ─
    frame.set_state_wrong_pin();
    let fb = frame.render();
    assert!(
        range_contains(&fb, dot_row_y0, dot_row_y1, alert),
        "wrong-PIN state must recolor the dot row to Theme.alert (the transient reject cue)"
    );
    // The reject cue is scoped to the dot row — it must never bleed into a
    // full-window flash (this mission's explicit hard constraint: the
    // border flash removed 2026-07-05 must not be reintroduced).
    assert!(
        !range_contains(&fb, 0, title_y1, alert),
        "the reject cue must not paint the title bar — it is not a full-window flash"
    );
    assert!(
        !range_contains(&fb, lower_panel_y0, lower_panel_y1, alert),
        "the reject cue must not paint the numpad/lower panel — it is not a full-window flash"
    );

    // ── State 3: backing-off (D4 — numpad replaced by the countdown) ────
    frame.set_state_backing_off("Try again in 23s");
    let fb = frame.render();
    assert!(
        count_matches(&fb, lower_panel_y0, lower_panel_y1, text_secondary) >= TEXT_MATCH_THRESHOLD,
        "backing-off state must render the lockout_text (\"Try again in Ns\") countdown line"
    );
    // The ordinary numpad state never uses Theme.text-secondary anywhere in
    // the lower panel (button glyphs are text-primary/text-muted) — assert
    // the numpad's own background (Theme.surface, the button fill) is gone,
    // confirming the numpad slot was actually replaced, not just overlaid.
    assert!(
        !range_contains(&fb, lower_panel_y0, lower_panel_y1, surface),
        "backing-off state must not still show numpad button fills underneath the countdown"
    );
    // The countdown panel's own background is Theme.bg-space (same as the
    // dot row) — sanity that the panel itself painted, not just its text.
    assert!(
        range_contains(&fb, lower_panel_y0, lower_panel_y1, bg_space),
        "backing-off panel's own background should still show through around the text"
    );

    // ── State 4: unread badge (D5 — count only) ──────────────────────────
    frame.set_state_unread_badge(3);
    let fb = frame.render();
    assert!(
        range_contains(&fb, 0, title_y1, surface_raised),
        "unread_count > 0 must show the badge pill (Theme.surface-raised) in the title bar"
    );

    // Flipping back to zero hides it again — the badge must not latch.
    frame.set_state_unread_badge(0);
    let fb = frame.render();
    assert!(
        !range_contains(&fb, 0, title_y1, surface_raised),
        "unread_count == 0 must hide the badge entirely, not show \"0\""
    );
}
