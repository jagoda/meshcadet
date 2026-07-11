// SPDX-License-Identifier: GPL-3.0-only
//! T-Deck Plus keyboard co-processor — pure backlight duty mapping.
//!
//! The I2C driver (`KeyboardDriver`) stays in `firmware/src/ui/keyboard.rs`
//! — it owns real hardware I/O; `backlight_duty` below is its one pure,
//! host-testable helper and moves here so its test executes under `cargo
//! test --workspace` (this crate is a detached, cross-compiled workspace —
//! see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block written there
//! would type-check but never run).
//!
//! `key_text` — the module's other candidate pure helper — does NOT move
//! here: it constructs `slint::platform::Key`/`slint::SharedString` values
//! directly, and this crate's boundary is deliberately `slint`-free (see
//! `firmware-core/Cargo.toml`'s description and `lib.rs`'s doc). Per this
//! increment's abort clause ("if a 'pure' helper turns out to need a
//! Slint-generated type, reclassify it rather than forcing it into
//! firmware-core"), `key_text` and its three tests stay behind in
//! `firmware/src/ui/keyboard.rs`, still compile-only pending a future
//! `ui_sim`/`ui_perf` home. See `docs/adr/0005-firmware-core-extraction.md`.

/// Map "backlight on/off" to the co-processor's duty byte (0-255; `0` =
/// off, full brightness otherwise).
///
/// Pure (host-testable) so the on/off→duty mapping has a regression guard
/// independent of the I2C transaction.
pub fn backlight_duty(on: bool) -> u8 {
    if on {
        BACKLIGHT_ON_DUTY
    } else {
        0
    }
}

/// Duty byte sent for "backlight on" (full brightness; the co-processor's
/// duty range is 0–255).
const BACKLIGHT_ON_DUTY: u8 = 255;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backlight_duty_maps_on_to_full_and_off_to_zero() {
        assert_eq!(backlight_duty(true), BACKLIGHT_ON_DUTY);
        assert_eq!(backlight_duty(false), 0);
    }
}
