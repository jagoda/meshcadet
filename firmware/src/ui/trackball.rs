// SPDX-License-Identifier: GPL-3.0-only
//! T-Deck Plus trackball driver — 4-direction roll + center click.
//!
//! # Hardware
//!
//! The T-Deck Plus trackball is wired directly to five ESP32-S3 GPIOs (no I2C
//! co-processor, unlike touch/keyboard). Each direction and the center click
//! is a discrete momentary switch to GND with no external pull-up populated
//! on the board, so every pin must be driven with the ESP32's **internal**
//! pull-up and read **active-low** (idle HIGH, pulled LOW on contact).
//!
//! Pin assignments are confirmed against two independent, already-shipped
//! firmwares for this exact board (not guessed): LilyGo's own
//! `Xinyuan-LilyGO/T-Deck` reference `utilities.h` (`BOARD_TBOX_G01..G04`) and
//! Meshtastic's `variants/esp32s3/t-deck/variant.h` (`TB_UP`/`TB_DOWN`/
//! `TB_LEFT`/`TB_RIGHT`/`TB_PRESS`), which additionally names the click line
//! and confirms `FALLING`-edge / active-low behavior:
//!
//! | Signal | GPIO | Note |
//! |--------|------|------|
//! | Up     | 3    | `BOARD_TBOX_G01` / `TB_UP` |
//! | Down   | 15   | `BOARD_TBOX_G03` / `TB_DOWN` |
//! | Left   | 1    | `BOARD_TBOX_G04` / `TB_LEFT` |
//! | Right  | 2    | `BOARD_TBOX_G02` / `TB_RIGHT` |
//! | Click  | 0    | `TB_PRESS` — shared with `BOARD_BOOT_PIN` (see below). |
//!
//! **GPIO0/BOOT dual-use (hardware-safety note, not a software choice):** the
//! trackball's center-click switch is physically wired to GPIO0 on the T-Deck
//! Plus PCB — the same strapping pin the ESP32-S3 samples at the start of
//! *every* reset to choose SPI-flash boot (HIGH, the normal case) vs. UART/USB
//! ROM-download mode (LOW). If the click switch is held down at the exact
//! moment the device resets (power-cycle, brown-out, a firmware update, or a
//! watchdog reboot), the device will come up in the ROM downloader instead of
//! the app — it will look "stuck" (blank screen, no boot log) until the
//! button is released and the device is power-cycled again, or until
//! `espflash` is used to flash it, whichever comes first. This is standard
//! ROM behavior, **fully recoverable, and cannot brick the device** — it is
//! not a damage- or harm-class finding. It is also not something this
//! firmware's code can avoid: the wire from the switch to GPIO0 is a fixed
//! PCB trace, not a software pin assignment, and both LilyGo's own reference
//! firmware and Meshtastic's shipped T-Deck support accept this exact
//! trade-off for this exact pin (see the citations above) rather than leaving
//! the trackball's click line unconnected. Recorded here so it's diagnosable
//! ("why did it not boot?") rather than a surprise.
//!
//! None of GPIO 0/1/2/3/15 are claimed anywhere else in this firmware's pin
//! budget (see `main.rs`'s module doc and its `peripherals.pins.gpioNN` call
//! sites) — verified by grep across `firmware/src` before wiring this up, as
//! a hardware-feasibility gating requirement.
//!
//! # Read method: poll, not interrupt
//!
//! This firmware's entire UI/radio stack is cooperative-poll, single-task
//! (see `ui/mod.rs`'s module doc): the SX1262 `DIO1`/`BUSY` lines, the GT911
//! touch controller, and the keyboard co-processor are all read via plain
//! `is_high()`/register polls inside one `step()`/dispatcher-loop iteration —
//! there is no ISR-registered GPIO anywhere in this codebase today. Adding one
//! here would be the first, and would need its own ISR-safe hand-off queue
//! (the trackball's owning struct is not `Send`/`Sync`-safe for interrupt
//! context without one) — a materially bigger, riskier change than this
//! feature's scope justifies. Polling is also *sufficient*: `step()` runs at
//! roughly the same ~20 Hz cadence as the radio RX poll's 50 ms timeout
//! (`main.rs`), which both naturally debounces mechanical switch bounce
//! (settles in a few ms, well under one 50 ms poll period) and is fast enough
//! for a deliberate, one-row-per-detent roll gesture — the product target
//! (simple, low-precision-friendly nav).
//! The known trade-off: a very fast continuous roll can under-count detents
//! (multiple pulses inside one 50 ms window collapse to one step). Given the
//! target audience and interaction model (roll to move a highlight, not a
//! high-throughput scroll wheel), this is an acceptable v1 limitation, not a
//! defect — a follow-on can add acceleration/multi-step-per-poll handling if
//! field use shows it's needed.
//!
//! # Debounce
//!
//! Edge-triggered: an event fires only on the HIGH→LOW transition, and the
//! pin must be read HIGH again before the same direction can fire again (armed
//! once released). On top of that, [`DEBOUNCE_MS`] enforces a minimum gap
//! between two fires of the *same* signal, independent of the poll cadence —
//! cheap insurance if this driver is ever polled faster than today's ~50 ms
//! loop period.

use esp_idf_hal::gpio::{Input, InputPin, PinDriver, Pull};

/// Minimum milliseconds between two consecutive fires of the *same* trackball
/// signal (see module doc's "Debounce" section).
const DEBOUNCE_MS: u64 = 60;

/// One decoded trackball input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackballEvent {
    Up,
    Down,
    Left,
    Right,
    Click,
}

/// Number of physical trackball signals (up/down/left/right/click).
const NUM_SIGNALS: usize = 5;

/// The event each index of `TrackballDriver::pins` decodes to. Also fixes the
/// priority `poll()` checks in when more than one signal transitions in the
/// same call (Click first, then Up/Down/Left/Right) — physically only one
/// direction is ever actuated at a time by a real trackball roll, so this
/// only matters for the pathological case of two switches reading LOW in the
/// same poll (e.g. a bounce overlap), where picking a fixed, documented order
/// beats an arbitrary one.
const SIGNAL_EVENTS: [TrackballEvent; NUM_SIGNALS] = [
    TrackballEvent::Click,
    TrackballEvent::Up,
    TrackballEvent::Down,
    TrackballEvent::Left,
    TrackballEvent::Right,
];

/// Poll-based driver for the T-Deck Plus trackball (up/down/left/right/click).
///
/// See the module doc for pin assignments, the poll-vs-interrupt rationale,
/// and the debounce scheme. `poll()` is non-blocking and safe to call once
/// per cooperative `step()` iteration, same as `TouchDriver::poll_event` and
/// `KeyboardDriver::poll_key`.
pub struct TrackballDriver<'d> {
    // Indexed by IDX_CLICK/IDX_UP/IDX_DOWN/IDX_LEFT/IDX_RIGHT.
    pins: [PinDriver<'d, Input>; NUM_SIGNALS],
    /// `true` = last-read level was HIGH (released/idle).
    last_high: [bool; NUM_SIGNALS],
    /// `uptime_ms` of the last fire for each signal, for [`DEBOUNCE_MS`].
    last_fire_ms: [u64; NUM_SIGNALS],
}

impl<'d> TrackballDriver<'d> {
    /// Initialise all five trackball GPIOs as internal-pull-up inputs.
    ///
    /// `up`/`down`/`left`/`right`/`press` are the raw peripheral pins
    /// (`peripherals.pins.gpio3` etc. — see module doc for the mapping).
    /// Infallible in practice (GPIO input configuration does not probe for
    /// hardware presence the way an I2C ACK does), but returns `Result` to
    /// match this UI's graceful-degradation pattern for every other optional
    /// input device (`KeyboardDriver::new`, `BuzzerDriver::new`): a caller can
    /// degrade to trackball-less operation on any unexpected failure instead
    /// of aborting boot.
    pub fn new(
        up: impl InputPin + 'd,
        down: impl InputPin + 'd,
        left: impl InputPin + 'd,
        right: impl InputPin + 'd,
        press: impl InputPin + 'd,
    ) -> anyhow::Result<Self> {
        let click_pin = PinDriver::input(press, Pull::Up)
            .map_err(|e| anyhow::anyhow!("trackball click (GPIO0) pin init failed: {:?}", e))?;
        let up_pin = PinDriver::input(up, Pull::Up)
            .map_err(|e| anyhow::anyhow!("trackball up (GPIO3) pin init failed: {:?}", e))?;
        let down_pin = PinDriver::input(down, Pull::Up)
            .map_err(|e| anyhow::anyhow!("trackball down (GPIO15) pin init failed: {:?}", e))?;
        let left_pin = PinDriver::input(left, Pull::Up)
            .map_err(|e| anyhow::anyhow!("trackball left (GPIO1) pin init failed: {:?}", e))?;
        let right_pin = PinDriver::input(right, Pull::Up)
            .map_err(|e| anyhow::anyhow!("trackball right (GPIO2) pin init failed: {:?}", e))?;

        Ok(TrackballDriver {
            pins: [click_pin, up_pin, down_pin, left_pin, right_pin],
            // All switches are idle (released/HIGH) at boot under normal
            // conditions; if one somehow reads LOW immediately (e.g. a stuck
            // switch), it simply won't be able to fire until it's seen HIGH
            // once — fails safe (missed input, not a spurious one).
            last_high: [true; NUM_SIGNALS],
            last_fire_ms: [0; NUM_SIGNALS],
        })
    }

    /// Poll all five signals once and return at most one [`TrackballEvent`].
    ///
    /// `now_ms`: `uptime_ms()`-style monotonic milliseconds, for debounce.
    /// Call once per cooperative `step()` iteration. Returns `None` when no
    /// signal has a fresh, debounced falling edge this call.
    // `idx` indexes FOUR parallel arrays in lockstep (`self.pins`,
    // `self.last_high`, `self.last_fire_ms`, `SIGNAL_EVENTS`), not just one —
    // clippy's `enumerate()` rewrite only helps the single-array case.
    #[allow(clippy::needless_range_loop)]
    pub fn poll(&mut self, now_ms: u64) -> Option<TrackballEvent> {
        for idx in 0..NUM_SIGNALS {
            // Active-low: `is_low()` == pressed/actuated.
            let now_low = self.pins[idx].is_low();
            let was_high = self.last_high[idx];
            self.last_high[idx] = !now_low;

            if now_low && was_high {
                // Falling edge: candidate fire, subject to the debounce floor.
                if now_ms.saturating_sub(self.last_fire_ms[idx]) >= DEBOUNCE_MS {
                    self.last_fire_ms[idx] = now_ms;
                    return Some(SIGNAL_EVENTS[idx]);
                }
                // Within the debounce window: treat as bounce, no event. The
                // pin has still been recorded as "currently low" above, so it
                // must go HIGH before it can fire again — no double-fire risk
                // even though this particular edge was suppressed.
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    // NOTE: `TrackballDriver` itself needs real GPIO hardware (`PinDriver`),
    // so it has no host-testable constructor — same limitation as
    // `KeyboardDriver`/`TouchDriver`. The debounce arithmetic is simple enough
    // (two comparisons) that it doesn't warrant extracting a parallel pure
    // function purely to get a host test, unlike `touch_wake_transition`
    // (a real multi-state decision) — see `ui/mod.rs`'s module doc for that
    // extraction rationale, and `NOTE` at the bottom of `ui/mod.rs` for why
    // this crate's `#[cfg(test)]` blocks type-check but never execute anyway.
    #[allow(unused_imports)]
    use super::*;
}
