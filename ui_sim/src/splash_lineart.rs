// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig for the splash screen's full-window backdrop +
//! lower-half line art (Phase 3).
//!
//! `firmware/src/ui/screens/splash.rs` cannot itself be compiled on the host
//! (see `gps_status_rows.rs`'s module doc for the same explanation) — this
//! module copies its markup VERBATIM (same technique that file's own doc
//! establishes) so this check's abort condition ("if the horizon motif
//! overlaps or obscures the wordmark or version string, hold — do not fold")
//! can be checked against the screen's ACTUAL centered-content geometry
//! rather than estimated by hand. `tests/splash_lineart.rs` asserts (non-
//! visually) that the version-string text row is untouched by the
//! `PlanetHorizon` line art; `src/bin/splash_lineart_render.rs` writes the
//! human-visible PNG this doc references.
//!
//! Deliberately omits the Rust-side `SplashScreen` wrapper (animation
//! start/version-setter methods) — those are pure Rust, already proven by
//! `firmware`'s own `cargo test`, and out of scope for this host-sim
//! geometry check.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import { MascotBob, SpaceBackdrop, PlanetHorizon } from "../../firmware/src/ui/motifs.slint";

    export component SplashLineartUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        SpaceBackdrop {}
        PlanetHorizon { x: 0px; y: 198px; }

        in property <string> version_str: "v1.2.3";

        in-out property <float> logo_opacity: 1;
        in-out property <float> title_opacity: 1;
        in-out property <float> version_opacity: 1;

        in-out property <float> ripple_active: 0;
        in-out property <float> ring1_size_px: 20;
        in-out property <float> ring1_opacity: 0.85;
        in-out property <float> ring2_size_px: 20;
        in-out property <float> ring2_opacity: 0.85;

        VerticalLayout {
            alignment: center;
            spacing: 6px;

            logo_area := Rectangle {
                height: 96px;
                background: transparent;

                icon_box := Rectangle {
                    x: 0px;
                    width: parent.width / 2;
                    height: parent.height;
                    background: transparent;

                    Rectangle {
                        opacity: ripple_active;
                        width: ring1_size_px * 1px;
                        height: ring1_size_px * 1px;
                        x: (parent.width - self.width) / 2;
                        y: (parent.height - self.height) / 2;
                        border-radius: self.width / 2;
                        border-width: 2px;
                        border-color: Theme.brand-signal.with-alpha(ring1_opacity);
                        background: transparent;
                    }
                    Rectangle {
                        opacity: ripple_active;
                        width: ring2_size_px * 1px;
                        height: ring2_size_px * 1px;
                        x: (parent.width - self.width) / 2;
                        y: (parent.height - self.height) / 2;
                        border-radius: self.width / 2;
                        border-width: 2px;
                        border-color: Theme.brand-signal.with-alpha(ring2_opacity);
                        background: transparent;
                    }

                    Text {
                        text: "📻";
                        font-size: Theme.icon-lg;
                        color: Theme.text-primary;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                        width: parent.width;
                        height: parent.height;
                        opacity: logo_opacity;
                    }
                }

                MascotBob {
                    x: parent.width / 2 + (parent.width / 2 - self.width) / 2;
                    y: (parent.height - self.height) / 2;
                }
            }

            title_row := Text {
                text: "MeshCadet";
                opacity: title_opacity;
                font-size: Theme.size-display;
                font-weight: 700;
                color: Theme.brand-signal;
                horizontal-alignment: center;
            }

            version_row := Text {
                text: version_str;
                opacity: version_opacity;
                font-size: Theme.size-preview;
                color: Theme.text-muted;
                horizontal-alignment: center;
            }
        }
    }
}

struct SplashLineartPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for SplashLineartPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// One rendered frame of the splash screen's static-complete state (the
/// state held throughout ALL of bring-up, per `splash.rs`'s own module doc
/// — logo/wordmark/version fully opaque, rings not yet rippling), which is
/// the frame the lower-band line art must not collide with.
///
/// # Panics
/// Same process-wide-`Platform`-singleton constraint as
/// `motif_library::MotifLibraryFrame::new` — callers must ensure exactly
/// one Slint `Platform` is installed per process (this crate's `tests/
/// splash_lineart.rs` integration-test binary gets its own process).
pub struct SplashLineartFrame {
    window: Rc<MinimalSoftwareWindow>,
    #[allow(dead_code)] // kept alive for the duration of the frame's window
    ui: SplashLineartUi,
}

impl SplashLineartFrame {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(SplashLineartPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = SplashLineartUi::new().expect("SplashLineartUi::new");
        ui.show().expect("SplashLineartUi::show");

        SplashLineartFrame { window, ui }
    }

    pub fn render(&self) -> Vec<Rgb565Pixel> {
        slint::platform::update_timers_and_animations();
        self.window.request_redraw();

        let mut framebuffer = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
        let rendered = self.window.draw_if_needed(|renderer| {
            renderer.render(&mut framebuffer, WIDTH as usize);
        });
        assert!(
            rendered,
            "splash-lineart frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for SplashLineartFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a rendered RGB565 framebuffer to an `image::RgbImage` (RGB8) for
/// PNG export — same conversion every other `ui_sim` render module uses.
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
