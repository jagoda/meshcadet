// SPDX-License-Identifier: GPL-3.0-only
//! T-Deck Plus keyboard co-processor — pure backlight duty mapping and the
//! `UiRuntime::step()` keyboard-drain decision helpers.
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

/// Decide whether a keyboard byte, polled while `MessageView` is the active
/// screen and the device is already awake, should seed the Compose draft and
/// flip the UI into write mode.
///
/// Returns `Some(text)` — the character to load as the draft's first
/// character — for printable ASCII, using exactly the same byte range
/// `key_text` documents as "the printable char" (`0x20..=0x7E`, space
/// through `~`). Returns `None` for anything else: Backspace, Return, Tab,
/// Escape (which `key_text` maps to non-text Slint keys) and any byte with
/// no mapping at all — those must retain MessageView's current behavior
/// (today, a no-op, since MessageView has no focusable input) rather than
/// jumping to Compose.
///
/// Pure (no hardware/Slint dependency) so the printable/non-printable
/// boundary — the crux of the "non-text keys must be excluded" acceptance
/// criterion — is host-testable independent of the keyboard co-processor.
/// Callers are additionally responsible for the sleep-wake exclusion (only
/// calling this once a wake-triggering keypress has already been swallowed
/// elsewhere) — that is `UiRuntime::step()`'s job, not this function's.
pub fn message_view_compose_seed(byte: u8) -> Option<String> {
    match byte {
        0x20..=0x7E => Some((byte as char).to_string()),
        _ => None,
    }
}

/// Decide whether `UiRuntime::step()`'s keyboard byte-drain loop should stop
/// after processing the byte that was just handled.
///
/// `pending_nav` is `self.pending_nav.get()` — nonzero means the byte just
/// handled set a screen-navigation flag (MessageView-seed or Compose
/// Return-to-send). The loop must stop in that case even if the drain bound
/// hasn't been reached: `active_screen` is about to change on the *next*
/// `step()`, so evaluating a same-burst byte against the still-current (soon
/// stale) screen would misattribute it — e.g. a second buffered character
/// overwriting the just-set Compose seed instead of landing in the Compose
/// draft it seeded. `drained >= max` is the independent defensive bound so a
/// stuck/flooding bus cannot starve RX/render.
///
/// Pure (no hardware/Slint dependency) so this burst/nav interaction — the
/// one behavioral edge case the multi-byte drain fix had to get right to
/// avoid a regression — is host-testable independent of the keyboard
/// co-processor.
pub fn keyboard_drain_should_stop(pending_nav: u8, drained: u8, max: u8) -> bool {
    pending_nav != 0 || drained >= max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backlight_duty_maps_on_to_full_and_off_to_zero() {
        assert_eq!(backlight_duty(true), BACKLIGHT_ON_DUTY);
        assert_eq!(backlight_duty(false), 0);
    }

    // ── message_view_compose_seed ────────────────────────────────────────
    //
    // Acceptance-critical: printable keys must seed the Compose draft;
    // non-text/navigation keys must not.

    #[test]
    fn printable_letters_digits_and_symbols_seed_the_draft() {
        assert_eq!(message_view_compose_seed(b'a').as_deref(), Some("a"));
        assert_eq!(message_view_compose_seed(b'Z').as_deref(), Some("Z"));
        assert_eq!(message_view_compose_seed(b'5').as_deref(), Some("5"));
        assert_eq!(message_view_compose_seed(b'!').as_deref(), Some("!"));
        // ':' seeds too, so the compose shortcode autocomplete can trigger
        // immediately off a keypress that opened compose.
        assert_eq!(message_view_compose_seed(b':').as_deref(), Some(":"));
        assert_eq!(message_view_compose_seed(b' ').as_deref(), Some(" "));
        assert_eq!(message_view_compose_seed(b'~').as_deref(), Some("~"));
    }

    #[test]
    fn control_and_navigation_bytes_do_not_seed() {
        // Backspace, Return/Enter, Tab, Escape — all have Slint key mappings
        // in `key_text` but must NOT flip MessageView to Compose.
        for byte in [0x08u8, 0x7F, 0x0D, 0x0A, 0x09, 0x1B] {
            assert_eq!(
                message_view_compose_seed(byte),
                None,
                "byte 0x{:02X} must not seed a Compose draft",
                byte,
            );
        }
    }

    #[test]
    fn unmapped_control_bytes_do_not_seed() {
        assert_eq!(message_view_compose_seed(0x00), None);
        assert_eq!(message_view_compose_seed(0x01), None);
        assert_eq!(message_view_compose_seed(0x1F), None);
    }

    // ── keyboard_drain_should_stop ────────────────────────────────────────
    //
    // Regression guard: the multi-byte keyboard drain must not evaluate a
    // same-burst byte against a screen that a nav-triggering byte just
    // scheduled to change out from under it.

    #[test]
    fn continues_below_bound_with_no_pending_nav() {
        assert!(!keyboard_drain_should_stop(0, 0, 8));
        assert!(!keyboard_drain_should_stop(0, 7, 8));
    }

    #[test]
    fn stops_at_the_defensive_bound_even_with_no_nav() {
        assert!(keyboard_drain_should_stop(0, 8, 8));
        assert!(keyboard_drain_should_stop(0, 9, 8));
    }

    #[test]
    fn stops_on_any_nonzero_pending_nav_regardless_of_count() {
        // A nav-triggering byte (MessageView-seed = 5, Compose Return-to-send
        // = 6) must stop the drain immediately, even on the very first byte
        // of a burst — the whole point is to defer the REST of the burst to
        // the next step() rather than misattribute it to the about-to-change
        // screen.
        assert!(keyboard_drain_should_stop(5, 0, 8));
        assert!(keyboard_drain_should_stop(6, 1, 8));
    }
}
