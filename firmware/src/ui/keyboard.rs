// SPDX-License-Identifier: GPL-3.0-only
//! T-Deck Plus physical QWERTY keyboard driver.
//!
//! # Hardware
//!
//! The T-Deck Plus QWERTY keyboard is **not** wired to the ESP32-S3 GPIO matrix
//! directly.  It is driven by a dedicated **ESP32-C3 keyboard co-processor**
//! running LilyGo's keyboard firmware, exposed to the host as an I2C slave on
//! the **shared I2C1 bus** (SDA=GPIO18 / SCL=GPIO8 — the same bus the GT911
//! touch controller uses) at address **`0x55`**.
//!
//! The co-processor does all the matrix scanning, debouncing, and modifier
//! handling (Sym / Shift / Alt) itself.  The host simply reads **one byte** per
//! transaction: the ASCII code of the most-recently-pressed key, or `0x00` when
//! no new key is available.  Each physical key-press is reported exactly once
//! (the co-processor buffers key-down events FIFO), so there is no host-side
//! key-repeat or release tracking to do.
//!
//! # Bus sharing
//!
//! Touch and keyboard share one [`I2cBus`] (`Rc<RefCell<I2cDriver>>`).  The UI
//! is a single cooperative task, so each `poll_key` borrows the bus for the
//! duration of one 1-byte read and releases it — the borrow never overlaps the
//! GT911 transaction in the same `step()`.
//!
//! # Key decoding
//!
//! Decoded ASCII bytes map to Slint [`Key`] events via [`key_text`]:
//!
//! | Byte            | Slint key            |
//! |-----------------|----------------------|
//! | `0x08` / `0x7F` | Backspace            |
//! | `0x0D` / `0x0A` | Return (Enter)       |
//! | `0x09`          | Tab                  |
//! | `0x1B`          | Escape               |
//! | `0x20..=0x7E`   | the printable char   |
//! | anything else   | ignored (`None`)     |
//!
//! Printable bytes include `:` (`0x3A`), which drives the Compose screen's
//! `:shortcode:` emoji autocomplete (see `screens::compose`).
//!
//! # Backlight (host-driven)
//!
//! The co-processor also owns a keyboard backlight LED (its own GPIO9, driven
//! by its own LEDC PWM channel — nothing to do with the display's GPIO42
//! backlight). Unlike keypress reads, this is host-**writable**: a 2-byte I2C
//! write `[0x01, duty]` (`duty` 0–255, `0` = off) sets it. This command byte
//! (`LILYGO_KB_BRIGHTNESS_CMD`) and its wire format are confirmed against
//! LilyGo's own reference co-processor firmware source
//! (`Xinyuan-LilyGO/T-Deck`, `examples/Keyboard_ESP32C3/Keyboard_ESP32C3.ino`
//! and the host-side `examples/Keyboard_T_Deck_Master/Keyboard_T_Deck_Master.ino`)
//! — the same firmware this module's `poll_key`/probe docs already cite as
//! ground truth for the read side. This was the feasibility gate for wiring
//! the keyboard backlight to screen wake/sleep (see [`set_backlight`]): the
//! co-processor does expose a host-driven set/clear command, so no
//! independent timer or new hardware channel is needed. It also turned out to
//! be the feasibility gate for a second rule — blinking the backlight for an
//! incoming message while asleep — so
//! [`crate::ui::UiRuntime`] now routes every write through a single
//! `sync_keyboard_backlight` arbiter rather than writing from the
//! wake/sleep transitions directly, to keep the two rules from fighting over
//! the same wire.

use slint::platform::Key;
use slint::SharedString;

use crate::ui::touch::I2cBus;

/// Conventional I2C address of the T-Deck Plus keyboard co-processor.
pub const KEYBOARD_ADDR: u8 = 0x55;

/// I2C command byte for setting the keyboard backlight duty cycle.
///
/// Matches `LILYGO_KB_BRIGHTNESS_CMD` in LilyGo's reference co-processor
/// firmware: a 2-byte write `[CMD_SET_BACKLIGHT, duty]` sets the backlight LED
/// PWM duty (0–255; `0` = off). See the module docs' "Backlight" section.
const CMD_SET_BACKLIGHT: u8 = 0x01;

// `backlight_duty` is pure Rust with no I2C/hardware dependency — it now
// lives in `firmware_core::ui::keyboard` so its test executes under `cargo
// test --workspace` (this crate is a detached, cross-compiled workspace —
// see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block written here
// would type-check but never run). `key_text` below stays: it constructs
// `slint::platform::Key`/`SharedString` values directly, and firmware-core's
// boundary is deliberately `slint`-free — see that module's doc for the
// full reclassification note. See `docs/adr/0005-firmware-core-extraction.md`.
use firmware_core::ui::keyboard::backlight_duty;

/// Driver for the T-Deck Plus QWERTY keyboard co-processor.
///
/// Shares the I2C1 bus with [`crate::ui::touch::TouchDriver`]; see module docs.
pub struct KeyboardDriver<'d> {
    i2c: I2cBus<'d>,
    addr: u8,
}

impl<'d> KeyboardDriver<'d> {
    /// Probe the keyboard co-processor at `0x55` on the shared bus.
    ///
    /// Returns `Err` if the address does not ACK, so the caller can degrade to
    /// touch-only operation instead of NAK-polling an absent device every loop.
    pub fn new(i2c: I2cBus<'d>) -> anyhow::Result<Self> {
        // A zero-length write is the lightest presence check, but some I2C
        // stacks reject empty buffers; read one byte instead.  An ACK (Ok)
        // confirms the co-processor is present.  Any pending key byte returned
        // here is intentionally discarded (pre-boot key-presses are noise).
        //
        // Probe with a bounded retry, NOT one-shot.  The co-processor is an
        // ESP32-C3 running its own keyboard firmware: it does not answer on I2C
        // until that firmware has booted and brought up its slave, and the
        // ~100 ms board-power settle (main.rs) is not guaranteed to cover it.
        // LilyGo's own reference `checkKb()` retries (3×) for the same reason.
        // Without the retry a slow C3 boot is permanently misdiagnosed as an
        // absent co-processor (the UI degrades to touch-only and never recovers).
        //
        // Each attempt is logged so the boot log is the diagnostic instrument:
        // an eventual ACK means it was a boot/timing (or bus-speed) issue, now
        // resolved; never ACKing across all attempts means the C3 firmware is
        // absent/crashed — a hardware remediation (re-flash the C3 keyboard
        // firmware), not a host-firmware fix.
        const PROBE_ATTEMPTS: u32 = 5;
        const PROBE_DELAY_MS: u32 = 50;
        let mut probe = [0u8; 1];
        let mut last_err = None;
        for attempt in 1..=PROBE_ATTEMPTS {
            // Bind the read result in its own statement so the `RefMut` borrow
            // is released before the match arm runs — the `Ok` arm needs to move
            // `i2c` into the returned driver.
            let result = i2c.borrow_mut().read(KEYBOARD_ADDR, &mut probe, 50);
            match result {
                Ok(()) => {
                    log::info!(
                        "T-Deck keyboard co-processor found at I2C 0x{:02X} (attempt {}/{})",
                        KEYBOARD_ADDR,
                        attempt,
                        PROBE_ATTEMPTS,
                    );
                    return Ok(KeyboardDriver { i2c, addr: KEYBOARD_ADDR });
                }
                Err(e) => {
                    log::warn!(
                        "keyboard probe {}/{} at 0x{:02X} did not ACK: {:?}",
                        attempt,
                        PROBE_ATTEMPTS,
                        KEYBOARD_ADDR,
                        e,
                    );
                    last_err = Some(e);
                    if attempt < PROBE_ATTEMPTS {
                        esp_idf_hal::delay::FreeRtos::delay_ms(PROBE_DELAY_MS);
                    }
                }
            }
        }
        Err(anyhow::anyhow!(
            "keyboard co-processor not found at I2C 0x{:02X} after {} attempts: {:?}",
            KEYBOARD_ADDR,
            PROBE_ATTEMPTS,
            last_err,
        ))
    }

    /// Poll for one key byte.
    ///
    /// Returns `Ok(Some(byte))` for a fresh key-press, `Ok(None)` when no key
    /// is pending (`0x00`).  Call once per cooperative `step()` iteration.
    pub fn poll_key(&mut self) -> anyhow::Result<Option<u8>> {
        let mut buf = [0u8; 1];
        self.i2c.borrow_mut().read(self.addr, &mut buf, 50)?;
        if buf[0] == 0 {
            Ok(None)
        } else {
            Ok(Some(buf[0]))
        }
    }

    /// Set the keyboard co-processor's backlight LED on or off.
    ///
    /// Writes `[CMD_SET_BACKLIGHT, duty]` (2 bytes) to the co-processor —
    /// see the module docs' "Backlight" section for where this command comes
    /// from. One bus borrow for the duration of the write, released before
    /// returning, matching `poll_key`'s discipline so a backlight write can
    /// never overlap a touch transaction sharing this I2C1 bus.
    ///
    /// Called from [`crate::ui::UiRuntime`]'s `sync_keyboard_backlight`
    /// arbiter (and once at boot) — the single place that decides the
    /// keyboard backlight's on/off state from both the screen-follows rule
    /// and the incoming-message blink loop, so the two never issue
    /// conflicting writes.
    pub fn set_backlight(&mut self, on: bool) -> anyhow::Result<()> {
        let duty = backlight_duty(on);
        self.i2c
            .borrow_mut()
            .write(self.addr, &[CMD_SET_BACKLIGHT, duty], 50)
            .map_err(|e| anyhow::anyhow!("keyboard backlight set({}) failed: {:?}", on, e))
    }
}

/// Translate a raw keyboard ASCII byte into the Slint key `text` payload.
///
/// Returns `None` for the no-key sentinel (`0x00`) and for control bytes that
/// have no Slint key mapping, so callers can simply skip them.
pub fn key_text(byte: u8) -> Option<SharedString> {
    match byte {
        0x00 => None,
        // Backspace and DEL both delete backward in this UI.
        0x08 | 0x7F => Some(Key::Backspace.into()),
        // Carriage-return and line-feed both mean Enter.
        0x0D | 0x0A => Some(Key::Return.into()),
        0x09 => Some(Key::Tab.into()),
        0x1B => Some(Key::Escape.into()),
        // Printable ASCII (space through '~'), including ':' for shortcodes.
        0x20..=0x7E => Some(SharedString::from((byte as char).to_string())),
        // Other control bytes have no text mapping.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_ascii_maps_to_itself() {
        assert_eq!(key_text(b'a').unwrap().as_str(), "a");
        assert_eq!(key_text(b'Z').unwrap().as_str(), "Z");
        assert_eq!(key_text(b' ').unwrap().as_str(), " ");
        // ':' must pass through so the compose autocomplete can trigger.
        assert_eq!(key_text(b':').unwrap().as_str(), ":");
    }

    #[test]
    fn special_keys_map_to_slint_codes() {
        assert_eq!(key_text(0x08).unwrap().as_str(), "\u{8}"); // Backspace
        assert_eq!(key_text(0x7F).unwrap().as_str(), "\u{8}"); // DEL -> Backspace
        assert_eq!(key_text(0x0D).unwrap().as_str(), "\n");    // CR -> Return
        assert_eq!(key_text(0x0A).unwrap().as_str(), "\n");    // LF -> Return
        assert_eq!(key_text(0x09).unwrap().as_str(), "\t");    // Tab
    }

    #[test]
    fn no_key_and_unmapped_controls_return_none() {
        assert!(key_text(0x00).is_none());
        assert!(key_text(0x01).is_none());
        assert!(key_text(0x1F).is_none());
    }

    // `backlight_duty`'s test moved to `firmware-core/src/ui/keyboard.rs`
    // alongside the function — see this file's module-level move note above.
}
