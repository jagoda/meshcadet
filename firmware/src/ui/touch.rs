// SPDX-License-Identifier: GPL-3.0-only
//! GT911 capacitive touch controller driver for T-Deck Plus.
//!
//! # Hardware
//!
//! The GT911 is a 5-point capacitive touch IC connected via I2C1:
//! - SDA = GPIO18
//! - SCL = GPIO8
//! - INT = GPIO16 (open-drain output from GT911; polled via I2C status register,
//!   not ISR-driven)
//! - No RST pin is wired to the GT911 on the T-Deck Plus.
//!
//! # Address-select
//!
//! The GT911 latches its I2C address from the INT pin level during the power-on
//! reset pulse:
//! - INT LOW  at power-on → `0x5D`
//! - INT HIGH at power-on → `0x14`
//!
//! On the T-Deck Plus no firmware-controlled RST pin is connected, so no
//! software-driven address-select pulse is possible.  The board hardware holds INT
//! low at power-on, giving address `0x5D`.  `TouchDriver::new` probes `0x5D` first,
//! then `0x14`, and returns `Err` if neither ACKs — avoiding NAK-forever polling on
//! absent or misconfigured hardware.
//!
//! # Register map (relevant subset)
//!
//! | Address | R/W | Description |
//! |---------|-----|-------------|
//! | 0x8040  | W   | Configuration registers (69 bytes; only touched at init) |
//! | 0x814E  | R/W | Touch status: bit7 = buffer_ready, bits[3:0] = touch count |
//! | 0x814F  | R   | First touch-point track ID (1 byte; NOT included in the REG_POINT0 read window) |
//! | 0x8150  | R   | First touch-point data: x_lo(1), x_hi(1), y_lo(1), y_hi(1), size_lo(1), size_hi(1) — 6 bytes; read as `REG_POINT0` |
//!
//! Writing 0x00 to 0x814E clears the buffer-ready flag.
//!
//! # Usage
//!
//! ```rust,ignore
//! // `i2c` is the shared bus handle (Rc<RefCell<I2cDriver>>); the keyboard
//! // co-processor shares the same bus — clone it for each driver.
//! let mut touch = TouchDriver::new(i2c.clone())?;
//! loop {
//!     if let Some(pt) = touch.poll_point()? {
//!         log::info!("touch x={} y={}", pt.x, pt.y);
//!     }
//! }
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use esp_idf_hal::i2c::I2cDriver;

/// Shared handle to the single I2C1 bus driver.
///
/// The GT911 touch controller (`0x5D`) and the T-Deck Plus keyboard
/// co-processor (`0x55`) both live on I2C1 (SDA=GPIO18 / SCL=GPIO8).  Because
/// the UI runs as a single cooperative task, the bus is software-serialised:
/// each driver borrows the `RefCell` for the duration of one transaction and
/// releases it, so the borrows never overlap.  This `Rc<RefCell<…>>` lets both
/// [`TouchDriver`] and [`crate::ui::keyboard::KeyboardDriver`] hold the same
/// underlying `I2cDriver` without a second peripheral instance.
pub type I2cBus<'d> = Rc<RefCell<I2cDriver<'d>>>;

/// I2C address of the GT911 when INT is LOW at power-on (T-Deck Plus default).
pub const GT911_ADDR: u8 = 0x5D;
/// I2C address of the GT911 when INT is HIGH at power-on (alternate; not T-Deck Plus).
const GT911_ADDR_ALT: u8 = 0x14;

/// Status register address (2-byte big-endian register addressing on GT911).
const REG_STATUS: u16 = 0x814E;
/// First touch-point data register.
const REG_POINT0: u16 = 0x8150;
/// Bytes read per touch point from REG_POINT0 (x_lo/x_hi/y_lo/y_hi/size_lo/size_hi; track_id at 0x814F is excluded).
const BYTES_PER_POINT: usize = 6;

/// A single touch contact point (first finger only; we are touch-first but
/// single-touch for this UI).
#[derive(Clone, Copy, Debug)]
pub struct TouchPoint {
    /// X coordinate in display pixels (0 = left, 319 = right for 320-wide display).
    pub x: u16,
    /// Y coordinate in display pixels (0 = top, 239 = bottom for 240-tall display).
    pub y: u16,
    /// Contact area / pressure proxy.
    ///
    /// Read off the wire alongside x/y at zero extra I2C cost (same 6-byte
    /// `REG_POINT0` burst) but not consumed by any gesture logic yet — no
    /// long-press / force-touch disambiguation exists today. Kept rather than
    /// dropped: once discarded here it cannot be recovered from a later poll.
    #[allow(dead_code)]
    pub size: u16,
}

/// Touch event abstraction fed to the UI event dispatcher.
#[derive(Clone, Copy, Debug)]
pub struct TouchEvent {
    pub point: TouchPoint,
    pub kind: TouchKind,
}

/// Touch gesture kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchKind {
    /// Finger pressed down.
    Pressed,
    /// Finger moved while pressed.
    Moved,
    /// Finger lifted.
    Released,
}

/// GT911 driver.
///
/// Owns the I2C bus.  Wrap in a `Mutex` if sharing between tasks (not required
/// for the cooperative single-thread model used here).
pub struct TouchDriver<'d> {
    i2c: I2cBus<'d>,
    /// Confirmed I2C address of the GT911 (0x5D or 0x14); resolved at init.
    addr: u8,
    /// Last known touch state: `Some(point)` = currently pressed.
    last: Option<TouchPoint>,
    /// `uptime_ms` of the most recent Pressed/Moved read (i.e. the last time
    /// `last` was set from a fresh, buffer-ready frame). Used by
    /// [`silence_implies_release`] to debounce the release-by-silence
    /// inference below — see that function's doc for why this exists.
    last_update_ms: u64,
}

impl<'d> TouchDriver<'d> {
    /// Create a new `TouchDriver` wrapping the given `I2cDriver`.
    ///
    /// Probes the GT911 at `0x5D` (T-Deck Plus default) and then `0x14`.
    /// Returns `Err` if neither address ACKs, preventing NAK-forever polling.
    ///
    /// # Address-select
    ///
    /// The GT911 has no software-accessible RST pin on the T-Deck Plus; the I2C
    /// address is fixed by the board hardware at power-on.  The probe write (2-byte
    /// register address to `REG_STATUS`) is the minimum transaction needed to
    /// confirm the device is present and ACKing before entering the poll loop.
    pub fn new(i2c: I2cBus<'d>) -> anyhow::Result<Self> {
        // Probe: write the 2-byte register address for REG_STATUS.  An I2C ACK
        // from the device confirms presence at that address.  This is an
        // address-presence check, not a full product-ID verification (reading
        // the 4-byte product ID at 0x8140 would confirm "9110" but adds round
        // trips for marginal gain — GT911 is the only device on I2C1 on this
        // board).  Try 0x5D first (T-Deck Plus holds INT LOW at power-on).
        let probe_bytes = [(REG_STATUS >> 8) as u8, REG_STATUS as u8];
        let addr = if i2c.borrow_mut().write(GT911_ADDR, &probe_bytes, 50).is_ok() {
            GT911_ADDR
        } else if i2c.borrow_mut().write(GT911_ADDR_ALT, &probe_bytes, 50).is_ok() {
            log::warn!(
                "GT911 found at 0x{:02X} (expected 0x{:02X}) — INT line may not have \
                 been LOW at power-on",
                GT911_ADDR_ALT,
                GT911_ADDR,
            );
            GT911_ADDR_ALT
        } else {
            anyhow::bail!(
                "GT911 not found at 0x{:02X} or 0x{:02X} — I2C bus error or device absent",
                GT911_ADDR,
                GT911_ADDR_ALT,
            );
        };
        log::info!("GT911 touch controller found at I2C 0x{:02X}", addr);
        Ok(TouchDriver { i2c, addr, last: None, last_update_ms: 0 })
    }

    /// Write a 2-byte register address (big-endian) then read `n` bytes.
    fn read_reg(&mut self, reg: u16, buf: &mut [u8]) -> anyhow::Result<()> {
        let addr_bytes = [(reg >> 8) as u8, reg as u8];
        let mut bus = self.i2c.borrow_mut();
        bus.write(self.addr, &addr_bytes, 50)?;
        bus.read(self.addr, buf, 50)?;
        Ok(())
    }

    /// Write a 2-byte register address followed by `data` bytes.
    fn write_reg(&mut self, reg: u16, data: &[u8]) -> anyhow::Result<()> {
        // Pack address + data into a single write (GT911 requires it).
        let mut buf = [0u8; 4];
        buf[0] = (reg >> 8) as u8;
        buf[1] = reg as u8;
        buf[2..2 + data.len()].copy_from_slice(data);
        self.i2c.borrow_mut().write(self.addr, &buf[..2 + data.len()], 50)?;
        Ok(())
    }

    /// Debounce window for the release-by-silence inference in `poll_event`
    /// (see that function's `status & 0x80 == 0` arm and
    /// [`silence_implies_release`]).
    ///
    /// The GT911 refreshes its buffer-ready bit roughly every ~10 ms while a
    /// finger is down. 40 ms comfortably exceeds that native refresh
    /// interval (plus I2C/bus-sharing jitter with the keyboard co-processor)
    /// while staying well under human perception of release lag.
    const SILENCE_RELEASE_DEBOUNCE_MS: u64 = 40;

    /// Poll for a single touch event.
    ///
    /// Returns `Ok(Some(event))` if a state change occurred (press, move,
    /// release), `Ok(None)` if no new data is available.
    ///
    /// Call this once per dispatcher loop iteration (≥ every 20 ms is
    /// sufficient for interactive response). `now_ms` is the caller's
    /// monotonic uptime clock (same clock as `UiRuntime::step`'s `now_ms`) —
    /// used only to debounce the release-by-silence inference below; it does
    /// not gate how often the caller may call this function.
    pub fn poll_event(&mut self, now_ms: u64) -> anyhow::Result<Option<TouchEvent>> {
        let mut status_buf = [0u8; 1];
        self.read_reg(REG_STATUS, &mut status_buf)?;
        let status = status_buf[0];

        // Buffer-ready bit: bit 7. Cleared means "no NEW frame since the last
        // read" — it does NOT mean "finger lifted" (that is reported
        // explicitly below via `touch_count == 0` on a buffer-ready frame).
        //
        // DEFECT FIX: this branch used
        // to treat "not ready" as an immediate release whenever a finger was
        // down (`self.last.is_some()`). That was harmless when `poll_event`
        // was called at most once per `UiRuntime::step()`, because ~20-50ms
        // elapsed between calls — comfortably longer than the GT911's own
        // ~10ms refresh interval, so "not ready" reliably meant "actually
        // released" by the time we asked again. A later drain-loop
        // rework calls `poll_event` back-to-back, microseconds apart, within
        // a single `step()` — far faster than the GT911 refreshes — so the
        // SAME still-held touch now reads "not ready" on the second call and
        // this branch synthesized a spurious Released while the finger was
        // still down. The very next real frame (still the same contact) then
        // read `self.last == None` and was reported as a fresh Pressed,
        // giving one physical tap two full press/release pairs — two Slint
        // `clicked` events, i.e. the doubled PIN digit. (That drain-loop
        // rework itself fixed a separate input-latency defect where dropped
        // events piled up faster than the loop drained them.)
        //
        // Fix: "not ready" alone must never assert a release. Only treat
        // prolonged silence (no fresh frame for `SILENCE_RELEASE_DEBOUNCE_MS`)
        // as an inferred release — a safety net for the rare case a lifted
        // finger never gets an explicit zero-count frame — via
        // `silence_implies_release`, a pure function so this decision is
        // host-testable independent of the I2C/hardware stack (same rationale
        // as `touch_wake_transition` in `ui/mod.rs`).
        if status & 0x80 == 0 {
            if let Some(last_pt) = self.last {
                if silence_implies_release(now_ms, self.last_update_ms, Self::SILENCE_RELEASE_DEBOUNCE_MS) {
                    self.last = None;
                    return Ok(Some(TouchEvent {
                        point: last_pt,
                        kind: TouchKind::Released,
                    }));
                }
            }
            return Ok(None);
        }

        let touch_count = status & 0x0F;

        // Clear buffer-ready flag immediately so the GT911 can write the next frame.
        self.write_reg(REG_STATUS, &[0x00])?;

        if touch_count == 0 {
            // Explicitly reported as no contacts (differs from "no new data").
            if let Some(last_pt) = self.last.take() {
                return Ok(Some(TouchEvent {
                    point: last_pt,
                    kind: TouchKind::Released,
                }));
            }
            return Ok(None);
        }

        // Read first touch point data (6 bytes): x_lo,x_hi,y_lo,y_hi,size_lo,size_hi.
        let mut pt_buf = [0u8; BYTES_PER_POINT];
        self.read_reg(REG_POINT0, &mut pt_buf)?;

        // REG_POINT0 (0x8150) = x_lo; layout: [x_lo, x_hi, y_lo, y_hi, size_lo, size_hi]
        // track_id lives at 0x814F, one byte before this window — it is NOT buf[0].
        let x = u16::from_le_bytes([pt_buf[0], pt_buf[1]]);
        let y = u16::from_le_bytes([pt_buf[2], pt_buf[3]]);
        let size = u16::from_le_bytes([pt_buf[4], pt_buf[5]]);

        let point = TouchPoint { x, y, size };
        let kind = if self.last.is_none() {
            TouchKind::Pressed
        } else {
            TouchKind::Moved
        };
        self.last = Some(point);
        self.last_update_ms = now_ms;

        Ok(Some(TouchEvent { point, kind }))
    }
}

/// Pure decision function for the release-by-silence debounce in
/// [`TouchDriver::poll_event`]'s `status & 0x80 == 0` arm — extracted so this
/// regression-causing logic is covered by a host-runnable unit test
/// independent of the I2C/hardware stack, same rationale as
/// `touch_wake_transition` in `ui/mod.rs`.
///
/// Returns `true` once `now_ms` is at least `debounce_ms` past
/// `last_update_ms` (saturating, so a clock that hasn't advanced — or has
/// wrapped — never spuriously asserts a release).
fn silence_implies_release(now_ms: u64, last_update_ms: u64, debounce_ms: u64) -> bool {
    now_ms.saturating_sub(last_update_ms) >= debounce_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── silence_implies_release ─────────────────────────────────────────────
    // Pure debounce arithmetic — see the function's doc for why isolating it
    // from the I2C/hardware stack matters here. NOTE: this crate's `[[bin]]` target sets
    // `harness = false` (see `ui/mod.rs`'s test-module doc for the full
    // explanation) so `cargo test` on host only type-checks this module; the
    // hardware-runner path in `.cargo/config.toml` is what actually executes
    // it.

    #[test]
    fn silence_implies_release_false_within_debounce_window() {
        // This is the exact regression scenario: back-to-back polls within
        // the same `step()`'s drain loop are microseconds apart, i.e.
        // `now_ms == last_update_ms` — must NOT infer a release.
        assert!(!silence_implies_release(1_000, 1_000, 40));
        assert!(!silence_implies_release(1_020, 1_000, 40));
    }

    #[test]
    fn silence_implies_release_true_once_debounce_elapsed() {
        assert!(silence_implies_release(1_040, 1_000, 40));
        assert!(silence_implies_release(5_000, 1_000, 40));
    }

    #[test]
    fn silence_implies_release_never_fires_on_backwards_clock() {
        // `saturating_sub` must not wrap into a huge elapsed value if
        // `now_ms` is somehow behind `last_update_ms`.
        assert!(!silence_implies_release(500, 1_000, 40));
    }
}
