// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig for the `SignalMeter` widget (ADR-0010,
//! `meshcadet-signal-meter` campaign).
//!
//! # Why this is a separate, narrower render path from `HostSimUi` /
//! `motif_library` / `compose_send` / `gps_status_rows`
//!
//! `firmware/src/ui/screens/*.rs` cannot itself be compiled on the host —
//! the `firmware` crate cross-compiles for `xtensa-esp32s3-espidf` only (see
//! `lib.rs`'s module doc). Unlike `gps_status_rows.rs` (which copies a
//! screen-local component verbatim, since `StatusRow` is declared inline in
//! `gps_status.rs`'s own `slint::slint!{}` block), `SignalMeter` is its own
//! standalone `.slint` FILE (`firmware/src/ui/signal_meter.slint`) — so this
//! rig imports it directly by relative path, the same single-source-of-truth
//! technique every `ui_sim` module already uses for `theme.slint`/
//! `motifs.slint`. Nothing here is a fork or a re-derivation.
//!
//! Renders all six `signal-level` states (`0` = direct-only ring, `1..=5` =
//! bars) side by side in one frame, proving:
//! - the direct-only ring paints (an unfilled `brand-signal` circle, not a
//!   blank widget or a solid dot);
//! - each bar count `1..=5` paints exactly that many `brand-signal`-filled
//!   bars, with the rest dim/unfilled (the "out of 5" scale stays legible at
//!   every level, not just full/empty).
//!
//! Slint enforces a process-wide `Platform` singleton, so this module's
//! render entry point must never run in the same process as `lib.rs`'s,
//! `motif_library`'s, `compose_send`'s, or `gps_status_rows`'s —
//! `ui_sim/tests/signal_meter.rs` is its own Cargo integration-test binary
//! (own process), same isolation technique those modules' own docs explain.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

/// Width of each meter's own column in the row (see `SignalMeterRowUi`) —
/// deliberately wider than any single meter instance so six render with
/// visible gaps between them, easing visual/pixel-scan inspection.
pub const COL_WIDTH: u32 = 48;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import { SignalMeter } from "../../firmware/src/ui/signal_meter.slint";

    export component SignalMeterRowUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        HorizontalLayout {
            alignment: start;
            padding-left: 8px;
            padding-top: 20px;

            // One column per `signal-level` state, 0 (direct-only) through 5
            // (full bars) — the exact `0..=5` range
            // `firmware_core::ui::signal_meter::level_to_bars` ever emits.
            for level in 6 : Rectangle {
                width: 48px;
                height: 24px;
                SignalMeter {
                    signal-level: level;
                    width: 24px;
                    height: 16px;
                    x: 0px;
                    y: 0px;
                }
            }
        }
    }
}

struct SignalMeterPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for SignalMeterPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// One rendered frame of the six-level `SignalMeter` row.
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `gps_status_rows.rs::GpsStatusRowsFrame::new`'s identical note. Callers
/// must ensure exactly one [`SignalMeterFrame::new`] runs per process.
pub struct SignalMeterFrame {
    window: Rc<MinimalSoftwareWindow>,
    #[allow(dead_code)] // keeps the Slint component (and its window) alive
    ui: SignalMeterRowUi,
}

impl SignalMeterFrame {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(SignalMeterPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = SignalMeterRowUi::new().expect("SignalMeterRowUi::new");
        ui.show().expect("SignalMeterRowUi::show");

        SignalMeterFrame { window, ui }
    }

    /// Render the (static — no animation, no trigger) single frame.
    pub fn render(&self) -> Vec<Rgb565Pixel> {
        slint::platform::update_timers_and_animations();
        self.window.request_redraw();

        let mut framebuffer = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
        let rendered = self.window.draw_if_needed(|renderer| {
            renderer.render(&mut framebuffer, WIDTH as usize);
        });
        assert!(
            rendered,
            "signal-meter frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for SignalMeterFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a rendered RGB565 framebuffer to an `image::RgbImage` (RGB8) for
/// PNG export — same conversion every other `ui_sim` render module
/// duplicates locally (see `gps_status_rows.rs`'s identical function doc for
/// why: no shared dependency on `lib.rs`'s `#[cfg(test)]`-adjacent internals).
pub fn framebuffer_to_rgb_image(
    framebuffer: &[Rgb565Pixel],
    width: u32,
    height: u32,
) -> image::RgbImage {
    let mut img = image::RgbImage::new(width, height);
    for (i, px) in framebuffer.iter().enumerate() {
        let r5 = (px.0 >> 11) & 0x1F;
        let g6 = (px.0 >> 5) & 0x3F;
        let b5 = px.0 & 0x1F;
        let r8 = ((r5 << 3) | (r5 >> 2)) as u8;
        let g8 = ((g6 << 2) | (g6 >> 4)) as u8;
        let b8 = ((b5 << 3) | (b5 >> 2)) as u8;
        let x = (i as u32) % width;
        let y = (i as u32) / width;
        img.put_pixel(x, y, image::Rgb([r8, g8, b8]));
    }
    img
}

/// Expand a rendered RGB565 pixel back to 8-bit-per-channel for assertions.
pub fn rgb8(px: Rgb565Pixel) -> (u8, u8, u8) {
    let r5 = (px.0 >> 11) & 0x1F;
    let g6 = (px.0 >> 5) & 0x3F;
    let b5 = px.0 & 0x1F;
    (
        ((r5 << 3) | (r5 >> 2)) as u8,
        ((g6 << 2) | (g6 >> 4)) as u8,
        ((b5 << 3) | (b5 >> 2)) as u8,
    )
}
