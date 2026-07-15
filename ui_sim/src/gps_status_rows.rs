// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig for the GPS status screen's theme
//! (per-screen spec row 8: "Planet/orbit
//! motif for location, comet for signal").
//!
//! # Why this is a separate, narrower render path from `HostSimUi` /
//! `motif_library` / `compose_send`
//!
//! `firmware/src/ui/screens/gps_status.rs` cannot itself be compiled on the
//! host — the `firmware` crate cross-compiles for `xtensa-esp32s3-espidf`
//! only (see `lib.rs`'s module doc for the full explanation). This module
//! re-declares ONLY the previously-unproven mechanisms this theming pass and
//! the later Time-sync row-overflow fix touch: `StatusRow`'s `icon-kind`
//! selector, which picks between the shared `RingedPlanetCorner`
//! ("location") and `Comet` ("signal") motifs, and (added for the
//! row-overflow fix) `StatusRow`'s `value2`/`row-height` pair, which lets
//! the Time-sync row grow a second, secondary-styled value line without
//! outgrowing the 240px window — all copied verbatim from `gps_status.rs`'s
//! real markup, not re-derived. Everything else `gps_status.rs` themes (the
//! header, the plain `Theme`-token colors/sizes on the icon-less rows) is
//! either untouched by this pass or pure-Slint `Theme`-token idiom already
//! proven by every other themed screen in this codebase — same
//! "deliberately not a pixel-for-pixel mirror" scoping `compose_send.rs`'s
//! own module doc establishes for its own narrower proof.
//!
//! The Time-sync row (`row-height: 60px`, `value2` set) is the one row this
//! module renders that `gps_status.rs`'s real screen ALSO renders at that
//! same 60px height — see `ui_sim/tests/gps_status_rows.rs`'s
//! `time_sync_row_value2_fits_within_its_own_row_height` test for the actual
//! proof this fix's row-height arithmetic (`36 + 48*3 + 60 == 240`) holds
//! against the real fonts/theme, not just against a hand computation.
//!
//! Imports the REAL `theme.slint` / `motifs.slint` by relative path (not
//! forked token values or re-derived motif components) — single source of
//! truth, same technique every other `ui_sim` render module uses.
//!
//! Slint enforces a process-wide `Platform` singleton, so this module's
//! render entry point must never run in the same process as `lib.rs`'s,
//! `motif_library`'s, or `compose_send`'s — `ui_sim/tests/gps_status_rows.rs`
//! is its own Cargo integration-test binary (own process), same isolation
//! technique those modules' own docs explain.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

/// Row height, mirroring `gps_status.rs`'s own `StatusRow` default
/// `row-height`.
pub const ROW_HEIGHT: u32 = 48;

/// Time-sync row height override, mirroring `gps_status.rs`'s own
/// `row-height: 60px` on that one row (see this module's doc).
pub const TIME_SYNC_ROW_HEIGHT: u32 = 60;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import { RingedPlanetCorner, Comet } from "../../firmware/src/ui/motifs.slint";

    // Verbatim copy of `gps_status.rs`'s `StatusRow` component (icon-kind
    // selector + value2/row-height + layout) — see this file's module doc
    // for why a copy rather than an import.
    component StatusRow {
        in property <string> label;
        in property <string> value;
        in property <string> value2: "";
        in property <string> icon-kind: "none"; // "none" | "planet" | "comet"
        in property <length> row-height: 48px;

        height: row-height;

        Rectangle {
            background: transparent;

            Rectangle {
                y: parent.height - 1px;
                height: 1px;
                width: parent.width;
                background: Theme.surface-raised;
            }

            HorizontalLayout {
                padding-left: 12px;
                padding-right: 12px;
                padding-top: 4px;
                padding-bottom: 4px;
                spacing: 8px;

                if icon-kind == "planet" : Rectangle {
                    width: 22px;
                    RingedPlanetCorner {
                        width: 22px;
                        height: 22px;
                        y: (parent.height - self.height) / 2;
                    }
                }

                if icon-kind == "comet" : Rectangle {
                    width: 22px;
                    Comet {
                        width: 22px;
                        height: 11px;
                        y: (parent.height - self.height) / 2;
                    }
                }

                VerticalLayout {
                    horizontal-stretch: 1.0;
                    spacing: 1px;

                    Text {
                        text: label;
                        font-size: Theme.size-body;
                        color: Theme.text-secondary;
                    }

                    Text {
                        text: value;
                        font-size: Theme.size-subtitle;
                        color: Theme.text-primary;
                    }

                    if value2 != "" : Text {
                        text: value2;
                        font-size: Theme.size-caption;
                        color: Theme.text-secondary;
                    }
                }
            }
        }
    }

    export component GpsStatusRowsUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        VerticalLayout {
            spacing: 0px;

            // Row 0: "signal" motif (mirrors gps_status.rs's "Fix" row).
            StatusRow {
                label: "Fix";
                value: "Fix acquired";
                icon-kind: "comet";
            }
            // Row 1: no motif (mirrors "Satellites" — must stay icon-less).
            StatusRow {
                label: "Satellites";
                value: "8 satellites";
            }
            // Row 2: "location" motif (mirrors "Coordinates").
            StatusRow {
                label: "Coordinates";
                value: "48.117300, 11.516667 (age 42s)";
                icon-kind: "planet";
            }
            // Row 3: no motif, `value2` set + `row-height: 60px` (mirrors
            // "Time sync" — the row-overflow fix this module's doc points
            // to). Two value lines: absolute wall clock (full date incl.
            // year) on `value`, relative age on `value2`.
            StatusRow {
                label: "Time sync";
                value: "2026-07-15 14:32:10 UTC";
                value2: "synced 300s ago";
                row-height: 60px;
            }
        }
    }
}

struct GpsStatusRowsPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for GpsStatusRowsPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// One rendered frame of the four gps_status rows (comet / none / planet /
/// none-with-`value2`).
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `motif_library.rs::MotifLibraryFrame::new`'s identical note. Callers
/// must ensure exactly one [`GpsStatusRowsFrame::new`] runs per process.
pub struct GpsStatusRowsFrame {
    window: Rc<MinimalSoftwareWindow>,
    #[allow(dead_code)] // keeps the Slint component (and its window) alive
    ui: GpsStatusRowsUi,
}

impl GpsStatusRowsFrame {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(GpsStatusRowsPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = GpsStatusRowsUi::new().expect("GpsStatusRowsUi::new");
        ui.show().expect("GpsStatusRowsUi::show");

        GpsStatusRowsFrame { window, ui }
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
            "gps-status-rows frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for GpsStatusRowsFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a rendered RGB565 framebuffer to an `image::RgbImage` (RGB8) for
/// PNG export — same conversion every other `ui_sim` render module
/// duplicates locally (see `motif_library.rs`'s identical function doc for
/// why: no shared dependency on `lib.rs`'s `#[cfg(test)]`-adjacent
/// internals).
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
