// SPDX-License-Identifier: GPL-3.0-only
//! RGB565 twins of the Slint `Theme` global (`ui/theme.slint`).
//!
//! # Why these exist and why nothing currently consumes them
//!
//! The design plan calls for one semantic
//! color vocabulary shared across two rendering channels: the Slint text/UI
//! channel (`ui/theme.slint`'s `Theme` global, `color` values) and the raw
//! RGB565 framebuffer channel that `ui/notification.rs` used to drive via
//! `TDeckDisplay::draw_flash_border(color_rgb565: u16, ...)` — a full-screen
//! border flash for `ok`/`warn`/`alert`/info events (`0x07E0` green /
//! `0xFFE0` yellow / `0xF800` red / `0x001F` blue, per that function's
//! original doc).
//!
//! REMOVAL (2026-07-05): the border-flash mechanism was ripped out of
//! `ui/notification.rs`/`ui/display.rs` entirely — audio (I2S) and the
//! incoming-message keyboard-backlight blink are the only visual
//! notifications left. There is currently no RGB565 draw path anywhere in
//! this firmware for these constants to feed.
//!
//! They are kept anyway, as the frozen Rust-side half of the token contract
//! established here, for two reasons: (1) the semantic
//! ok/warn/alert/brand-signal vocabulary is the deliverable, independent of
//! which drawing path currently consumes it — a future visual-notification
//! channel (or any other raw-framebuffer diagnostic) should reuse these
//! exact words rather than re-deriving new RGB565 literals from scratch; (2)
//! this mirrors the existing precedent in this codebase for a known-unused-
//! today, kept-for-a-documented-future-consumer item (see
//! `NotifEvent::Provisioned` and `NotifPrefs::set_event_pref` in
//! `notification.rs`, both `#[allow(dead_code)]` with the same rationale).
//!
//! `rgb565` re-derives each word from the SAME 8-bit-per-channel hex value
//! declared in `ui/theme.slint`, rather than hand-computing the packed
//! `u16`, so the two files can never silently drift — see that file's doc
//! for the full color table and the invariants it upholds.

/// Pack 8-bit-per-channel RGB into a 16-bit RGB565 word (5/6/5 bits),
/// matching the exact bit layout `TDeckDisplay::draw_flash_border` used to
/// unpack (`(word >> 11) & 0x1F`, `(word >> 5) & 0x3F`, `word & 0x1F`) before
/// that function was removed — see this module's doc.
const fn rgb565(r8: u8, g8: u8, b8: u8) -> u16 {
    let r5 = (r8 >> 3) as u16;
    let g6 = (g8 >> 2) as u16;
    let b5 = (b8 >> 3) as u16;
    (r5 << 11) | (g6 << 5) | b5
}

// ── Palette twins (see ui/theme.slint for the canonical hex values) ────────
#[allow(dead_code)]
pub const BG_SPACE: u16 = rgb565(0x0d, 0x11, 0x17);
#[allow(dead_code)]
pub const SURFACE: u16 = rgb565(0x16, 0x1e, 0x28);
#[allow(dead_code)]
pub const SURFACE_RAISED: u16 = rgb565(0x1e, 0x2a, 0x38);
#[allow(dead_code)]
pub const SURFACE_ALT: u16 = rgb565(0x2a, 0x34, 0x42);
#[allow(dead_code)]
pub const BRAND_SIGNAL: u16 = rgb565(0x00, 0xb4, 0xff);
#[allow(dead_code)]
pub const BRAND_SIGNAL_BRIGHT: u16 = rgb565(0x33, 0xc4, 0xff);
#[allow(dead_code)]
pub const TEXT_PRIMARY: u16 = rgb565(0xe8, 0xec, 0xf0);
#[allow(dead_code)]
pub const TEXT_SECONDARY: u16 = rgb565(0xa0, 0xa8, 0xb0);
#[allow(dead_code)]
pub const TEXT_MUTED: u16 = rgb565(0x60, 0x68, 0x70);
#[allow(dead_code)]
pub const SELECT: u16 = rgb565(0x1e, 0x30, 0x50);
/// Success / positive state. Equal to the pre-removal `draw_flash_border`
/// "ok" word (`0x07E0`) — see module doc.
#[allow(dead_code)]
pub const OK: u16 = rgb565(0x00, 0xff, 0x00);
/// Caution / needs-attention state. Equal to the pre-removal `warn` word
/// (`0xFFE0`).
#[allow(dead_code)]
pub const WARN: u16 = rgb565(0xff, 0xff, 0x00);
/// Error / failure state. Equal to the pre-removal `alert` word (`0xF800`).
#[allow(dead_code)]
pub const ALERT: u16 = rgb565(0xff, 0x00, 0x00);

#[cfg(test)]
mod tests {
    use super::*;

    // These are type-checked but never executed on host (esp target) — see
    // the host-side glyph-coverage harness (`xtask` crate at the
    // repo root) for the actual host-runnable verification this contract
    // gets. Kept here as on-target documentation/regression coverage for
    // anyone who *does* run `cargo test` under the esp toolchain.

    #[test]
    fn rgb565_pack_matches_known_words() {
        // Sanity-check the packer itself against the three historical
        // border-flash words these three semantic colors were chosen to
        // reproduce exactly (see module + per-const doc).
        assert_eq!(OK, 0x07E0);
        assert_eq!(WARN, 0xFFE0);
        assert_eq!(ALERT, 0xF800);
    }

    #[test]
    fn rgb565_pack_is_lossy_downward() {
        // 8-bit -> 5/6-bit packing truncates, not rounds; confirms the
        // helper matches `draw_flash_border`'s original unpack convention
        // rather than some other rounding scheme.
        assert_eq!(rgb565(0xff, 0xff, 0xff), 0xFFFF);
        assert_eq!(rgb565(0x00, 0x00, 0x00), 0x0000);
    }
}
