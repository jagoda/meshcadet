// SPDX-License-Identifier: GPL-3.0-only
//! T-Deck Plus display driver wrapper.
//!
//! Wraps `mipidsi::Display` (ST7789 profile) on the SPI2 bus with the T-Deck
//! Plus display pin assignments.
//!
//! # Pin assignments
//!
//! | Signal | GPIO | Notes |
//! |--------|------|-------|
//! | SCK    | 40   | Shared SPI2 (also radio) |
//! | MOSI   | 41   | Shared SPI2 |
//! | MISO   | 38   | Shared SPI2 |
//! | CS     | 12   | LCD chip-select (radio CS = GPIO9) |
//! | DC     | 11   | Data/Command |
//! | RST    | 16   | Display reset (radio RST = GPIO17) |
//! | BL     | 42   | Backlight PWM (LEDC ch1/tim1, 2 kHz, full duty = on) |
//!
//! # Display geometry
//!
//! The T-Deck Plus 2.8" ST7789V2 is configured in landscape mode:
//! - Width  = 320 px  (post-rotation logical frame, used for line buffers + Slint size)
//! - Height = 240 px
//!
//! The ST7789 native (un-rotated) framebuffer is portrait 240 × 320.
//! `mipidsi::Builder::display_size()` takes the **native** dimensions; the Deg90
//! rotation flag in MADCTL swaps rows/columns so the logical drawing surface
//! becomes 320 × 240 landscape.
//!
//! # Slint integration
//!
//! `TDeckDisplay` implements the `LineFlush` trait used by the Slint
//! `SoftwareRenderer`.  The renderer calls `flush_line_range()` for each dirty
//! horizontal strip; this function sets the display window to that strip and
//! writes the RGB565 pixel data via SPI. Rendering line-by-line rather than
//! into a full frame buffer saves ~149 KB of RAM at the cost of multiple SPI
//! writes per refresh cycle — acceptable because the SPI bus runs at 40 MHz
//! and a 320-pixel line takes ≤ 13 µs.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use mipidsi::Builder;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, ColorOrder, Orientation, Rotation};

use esp_idf_hal::{
    delay::FreeRtos,
    gpio::{Output, OutputPin, PinDriver},
    ledc::{LedcChannel, LedcDriver, LedcTimerDriver},
    spi::{SpiDeviceDriver, SpiDriver},
};

/// Display width in pixels (landscape, post-rotation logical frame).
pub const DISPLAY_WIDTH:  u32 = 320;
/// Display height in pixels (landscape, post-rotation logical frame).
pub const DISPLAY_HEIGHT: u32 = 240;

/// Alias for the concrete mipidsi display type on the T-Deck.
///
/// `SpiDeviceDriver<'d, &'d SpiDriver<'d>>` wraps the shared SPI2 bus with the LCD CS pin so
/// the radio and display can coexist on the same SPI bus.
pub type TDeckDisplayInner<'d> = mipidsi::Display<
    display_interface_spi::SPIInterface<SpiDeviceDriver<'d, &'d SpiDriver<'d>>, PinDriver<'d, Output>>,
    ST7789,
    PinDriver<'d, Output>,
>;

/// Display driver wrapper with backlight control.
pub struct TDeckDisplay<'d> {
    inner: TDeckDisplayInner<'d>,
    backlight: LedcDriver<'d>,
}

impl<'d> TDeckDisplay<'d> {
    /// Initialise the ST7789 display.
    ///
    /// `spi_device`: SPI device driver for the LCD CS pin on GPIO12.
    /// `dc`: Data/Command pin (GPIO11).
    /// `rst`: Display reset pin (GPIO16).
    /// `backlight_channel`: LEDC channel peripheral (GPIO42 backlight PWM).
    /// `backlight_timer`: Pre-configured LEDC timer driver (2 kHz, 10-bit).
    /// `backlight_pin`: GPIO42 (consumed by the LEDC driver).
    ///
    /// # Backlight drive method
    ///
    /// The T-Deck Plus backlight boost converter requires a PWM switching signal on
    /// GPIO42 — a static GPIO `set_high()` does NOT activate it.  This function
    /// drives GPIO42 via the LEDC peripheral at 100% duty cycle (full brightness).
    /// The `LedcDriver` is stored for program lifetime so the PWM signal is never
    /// interrupted.
    ///
    /// # Ordering: controller init + clear BEFORE backlight-on
    ///
    /// BUG FIX: the backlight used to come on
    /// (full duty) BEFORE the ST7789 controller's own init sequence ran.  The
    /// ST7789's frame memory (GRAM) is not guaranteed to be cleared by the
    /// RESX hardware-reset pulse `Builder::init` issues below — on a warm
    /// reboot (no full power-loss) it can still hold whatever the PREVIOUS
    /// firmware session last drew (e.g. the contact list). With the backlight
    /// already lit at that point, that leftover frame was briefly visible on
    /// the panel before the first real frame (the boot splash) was ever
    /// flushed over SPI — the "app screen flashes before the splash" defect.
    /// Fixing this in `screens::splash`/`UiRuntime` alone can't help: the
    /// leak happens here, before either of those even runs. The fix: bring
    /// the backlight up at ZERO duty, run the controller's init sequence,
    /// explicitly clear the whole panel to black, and ONLY THEN raise the
    /// backlight to full duty — so the first pixels the backlight ever
    /// illuminates are black, never a stale previous-session frame.
    ///
    /// # Errors
    ///
    /// Returns an error if the LEDC or mipidsi init sequence fails.
    pub fn new<C: LedcChannel + 'd>(
        spi_device: SpiDeviceDriver<'d, &'d SpiDriver<'d>>,
        dc: PinDriver<'d, Output>,
        rst: PinDriver<'d, Output>,
        backlight_channel: C,
        backlight_timer: LedcTimerDriver<'d, C::SpeedMode>,
        backlight_pin: impl OutputPin + 'd,
    ) -> anyhow::Result<Self> {
        // Bring up the LEDC channel but hold it at ZERO duty (backlight OFF)
        // until the panel's GRAM has been cleared below — see "Ordering"
        // above. A plain GPIO set_high() would be insufficient regardless:
        // the T-Deck Plus panel uses a boost converter on GPIO42 that needs a
        // PWM switching input, not DC.
        let mut backlight = LedcDriver::new(backlight_channel, backlight_timer, backlight_pin)
            .map_err(|e| anyhow::anyhow!("backlight LEDC init failed: {:?}", e))?;
        backlight
            .set_duty(0)
            .map_err(|e| anyhow::anyhow!("backlight set_duty failed: {:?}", e))?;

        let di = display_interface_spi::SPIInterface::new(spi_device, dc);
        let mut display = Builder::new(ST7789, di)
            // BUG FIX: mipidsi 0.8 display_size() takes the NATIVE (un-rotated) framebuffer
            // dimensions.  ST7789::FRAMEBUFFER_SIZE = (240, 320) (portrait).
            // Passing (320, 240) triggered: assert!(320 + 0 <= 240) → panic.
            // With (240, 320) the assertion passes and the Deg90 MADCTL flag below
            // produces the 320×240 landscape logical surface (options::display_size()
            // swaps width/height for vertical rotations).
            .display_size(240, 320)
            .orientation(Orientation::new().rotate(Rotation::Deg90))
            // BUG FIX: the T-Deck
            // Plus ST7789V2 panel is wired RGB, not BGR — `ColorOrder::Rgb`
            // is the correct setting. An earlier fix set this to `ColorOrder::Bgr` to cure
            // an accent-blue-renders-orange symptom, but that panel was, at
            // the time, ALSO missing `.invert_colors(...)` (see below): with
            // inversion absent, a correctly-ordered #00b4ff cyan already
            // rendered as its photometric complement (~#ff4b00, orange) —
            // the true bug was the missing inversion, not channel order.
            // Swapping to BGR on top of that masked the inversion bug for
            // that one hue by compounding two errors (swap the channels,
            // THEN complement them): (0,180,255) → swap → (255,180,0) →
            // complement → (0,75,255), which reads as a plausible blue.
            // Once `ColorInversion::Inverted` was added in a later fix,
            // that compounding
            // stopped cancelling out and the raw channel swap became visible
            // on its own: cyan (0,180,255) swapped to (255,180,0) = amber/
            // yellow, and the cool grey #a0a8b0 (160,168,176) swapped to
            // (176,168,160), a warm/brown grey — exactly the symptom this
            // fix addresses. With `ColorOrder::Rgb` restored and inversion
            // already correct, both render as intended (cyan stays cyan,
            // the grey stays cool). `ColorOrder` only controls the MADCTL
            // subpixel-order bit sent to the controller here — it does not
            // touch how embedded_graphics decodes an RGB565 word into R/G/B
            // components — so this is the single point of truth; the
            // RGB565 unpack in `ui/platform.rs::process_line`
            // (r=bits15-11, g=bits10-5, b=bits4-0 → `Rgb565::new(r,g,b)`) is
            // a standard, unswapped decode with no compensating swap.
            .color_order(ColorOrder::Rgb)
            // BUG FIX: the T-Deck
            // Plus 2.8" panel is an IPS ST7789V2 that requires the controller's
            // INVON (invert-mode-on) command to render true colors — without
            // it the panel shows the photometric inverse of every pixel this
            // driver pushes: a #0d1117 near-black background renders near-white,
            // and saturated accents render as their RGB-complement (e.g. cyan
            // as orange). mipidsi's `ColorInversion` default is `Normal`, which
            // sends INVOFF during `Builder::init` (see
            // `models::st7789::init` → `SetInvertMode`) — leaving this unset
            // silently picks the wrong polarity for this panel. This also
            // retroactively explains the earlier "white-on-white splash logo"
            // defect: a black-on-black framebuffer was displayed light by the
            // inverted panel.
            .invert_colors(ColorInversion::Inverted)
            .reset_pin(rst)
            .init(&mut FreeRtos)
            .map_err(|e| anyhow::anyhow!("display init failed: {:?}", e))?;

        // Clear the whole panel to black — overwriting any leftover GRAM
        // contents from a previous session — BEFORE the backlight comes on
        // (see "Ordering" above).
        display
            .clear(Rgb565::BLACK)
            .map_err(|e| anyhow::anyhow!("display clear failed: {:?}", e))?;

        // Only now raise the backlight to full duty: every pixel it
        // illuminates from this point on is either black (this clear) or a
        // real Slint-rendered frame, never stale GRAM data.
        backlight
            .set_duty(backlight.get_max_duty())
            .map_err(|e| anyhow::anyhow!("backlight set_duty failed: {:?}", e))?;

        Ok(TDeckDisplay { inner: display, backlight })
    }

    /// Turn the backlight on (full duty) or off (zero duty) via the LEDC PWM
    /// channel.
    ///
    /// # Sleep depth
    ///
    /// This is a backlight-only sleep: the ST7789 display controller and its
    /// framebuffer contents are left running untouched, and the Slint
    /// cooperative render loop keeps ticking (`render_if_needed` still flushes
    /// dirty regions over SPI even with the backlight off) — so the panel is
    /// showing the correct pixels the instant the backlight comes back on,
    /// with no re-render latency. Only the GPIO42 PWM duty cycle changes.
    ///
    /// # Errors
    ///
    /// Returns an error if the LEDC duty-cycle write fails.
    pub fn set_backlight(&mut self, on: bool) -> anyhow::Result<()> {
        let duty = if on { self.backlight.get_max_duty() } else { 0 };
        self.backlight
            .set_duty(duty)
            .map_err(|e| anyhow::anyhow!("backlight set_duty failed: {:?}", e))
    }

    /// Flush a partial horizontal strip to the display.
    ///
    /// `line_y`: Y coordinate of the line (0 = top).
    /// `x_start`: leftmost pixel column of `pixels` (0-based).
    /// `pixels`: RGB565 iterator, left to right; its own reported `len()` is
    ///           the dirty-strip width (`range.end - range.start` from
    ///           Slint's `process_line`) — see "Zero-alloc flush path" below
    ///           for why this is derived rather than passed separately.
    ///
    /// Called by the Slint `LineBufferProvider` implementation so that only the
    /// renderer's dirty x-range is sent over SPI, avoiding black-pixel corruption
    /// of the surrounding undirty content on partial redraws.
    ///
    /// # Zero-alloc flush path
    ///
    /// PRIOR DEFECT: the call site (`ui/platform.rs::process_line`) used to
    /// `.collect()` the converted RGB565 pixels into a heap `Vec` before
    /// calling this function with a `&[Rgb565]` slice — a fresh heap
    /// allocation (and matching deallocation) on EVERY dirty scanline,
    /// confirmed at up to 240 times per full-window repaint (baseline ledger
    /// §5 item 1, `docs/perf/ui-perf-baseline.md`). This runs nested inside
    /// `ui::step()` → `render_if_needed()`, once per main-task dispatcher-loop
    /// iteration — the SAME iteration whose wall-clock length determines how
    /// promptly the NEXT iteration's CAD attempt / RX poll runs (`main.rs`'s
    /// dispatcher loop: CAD+TX → RX poll → `ui.step()`, in that order). Every
    /// allocator round-trip cut from this hot path shortens that iteration.
    ///
    /// FIX: accept a plain pixel iterator instead of a slice. `mipidsi`'s
    /// `fill_contiguous` (called below) already takes `impl IntoIterator<Item
    /// = Rgb565>` and streams pixels directly into `set_pixels` — it never
    /// needed a materialized buffer. Taking an iterator here lets
    /// `process_line` pass its `range.clone().map(..)` conversion straight
    /// through with no intermediate collection: same SPI window-set + pixel
    /// stream as before (identical wire bytes, identical pixels), zero heap
    /// traffic per line.
    ///
    /// `ExactSizeIterator` (not a separately-passed `len: usize`) is the
    /// bound deliberately: an earlier draft of this fix took `len` as its own
    /// parameter, decoupled from the iterator — a caller could pass a `len`
    /// that disagreed with the iterator's actual element count with nothing
    /// to catch it (the old `&[Rgb565]` slice made length and content
    /// inseparable by construction; a raw `Iterator` parameter would have
    /// silently reintroduced that gap). Deriving the width from `pixels.len()`
    /// keeps that same one-value invariant with the allocation removed.
    pub fn flush_line_range(
        &mut self,
        line_y: u16,
        x_start: u16,
        pixels: impl ExactSizeIterator<Item = Rgb565>,
    ) -> anyhow::Result<()> {
        use embedded_graphics::primitives::Rectangle;

        let len = pixels.len();
        if len == 0 {
            return Ok(());
        }
        let area = Rectangle::new(
            Point::new(x_start as i32, line_y as i32),
            Size::new(len as u32, 1),
        );
        self.inner
            .fill_contiguous(&area, pixels)
            .map_err(|e| anyhow::anyhow!("flush_line_range failed: {:?}", e))
    }
}
