// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig for the `BatteryIndicator` widget
//! (`meshcadet-battery-glanceable-indicator` campaign).
//!
//! # Why this is a separate, narrower render path from `HostSimUi` /
//! `motif_library` / `compose_send` / `gps_status_rows`
//!
//! `firmware/src/ui/screens/*.rs` cannot itself be compiled on the host —
//! the `firmware` crate cross-compiles for `xtensa-esp32s3-espidf` only (see
//! `lib.rs`'s module doc). Unlike `gps_status_rows.rs` (which copies a
//! screen-local component verbatim, since `StatusRow` is declared inline in
//! `gps_status.rs`'s own `slint::slint!{}` block), `BatteryIndicator` is its
//! own standalone `.slint` FILE (`firmware/src/ui/battery_indicator.slint`)
//! — so this rig imports it directly by relative path, the same
//! single-source-of-truth technique `ui_sim::signal_meter` (its own sibling
//! widget rig) already uses.
//!
//! Renders all five `battery-level` states (`0` = Unknown outline, `1` =
//! Charging, `2..=4` = Low/Partial/Full) side by side in one frame, proving:
//! - Unknown paints an outline-only shell (a visible non-background stroke,
//!   no fill);
//! - Charging paints a FULL body in `brand-signal` cyan, distinct from every
//!   bucket color;
//! - Low/Partial/Full each paint their own distinct fill color (`alert` red
//!   / `warn` yellow / `ok` green) with a strictly-ascending
//!   filled-pixel-area sequence, proving the fill fraction actually scales
//!   with bucket rather than every level painting the same fixed shape.
//!
//! Slint enforces a process-wide `Platform` singleton, so this module's
//! render entry point must never run in the same process as `lib.rs`'s,
//! `motif_library`'s, `compose_send`'s, `gps_status_rows`'s, or
//! `signal_meter`'s — `ui_sim/tests/battery_indicator.rs` is its own Cargo
//! integration-test binary (own process), same isolation technique those
//! modules' own docs explain.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

/// Width of each indicator's own column in the row (see
/// `BatteryIndicatorRowUi`) — deliberately wider than any single indicator
/// instance so all five render with visible gaps between them, easing
/// visual/pixel-scan inspection. Same value `ui_sim::signal_meter` uses for
/// its own six-state row.
pub const COL_WIDTH: u32 = 48;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import { BatteryIndicator } from "../../firmware/src/ui/battery_indicator.slint";

    export component BatteryIndicatorRowUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        HorizontalLayout {
            alignment: start;
            padding-left: 8px;
            padding-top: 20px;

            // One column per `battery-level` state, 0 (Unknown) through 4
            // (Full) — the exact `0..=4` range
            // `firmware_core::ui::battery_indicator::level_to_indicator_level`
            // ever emits.
            for level in 5 : Rectangle {
                width: 48px;
                height: 24px;
                BatteryIndicator {
                    battery-level: level;
                    width: 24px;
                    height: 16px;
                    x: 0px;
                    y: 0px;
                }
            }
        }
    }
}

struct BatteryIndicatorPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for BatteryIndicatorPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// One rendered frame of the six-level `BatteryIndicator` row.
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `signal_meter.rs::SignalMeterFrame::new`'s identical note. Callers must
/// ensure exactly one [`BatteryIndicatorFrame::new`] runs per process.
pub struct BatteryIndicatorFrame {
    window: Rc<MinimalSoftwareWindow>,
    #[allow(dead_code)] // keeps the Slint component (and its window) alive
    ui: BatteryIndicatorRowUi,
}

impl BatteryIndicatorFrame {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(BatteryIndicatorPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = BatteryIndicatorRowUi::new().expect("BatteryIndicatorRowUi::new");
        ui.show().expect("BatteryIndicatorRowUi::show");

        BatteryIndicatorFrame { window, ui }
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
            "battery-indicator frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for BatteryIndicatorFrame {
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
