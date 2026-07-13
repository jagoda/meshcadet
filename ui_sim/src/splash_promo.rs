// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig producing the promotional landing-page screenshot
//! of the boot splash screen (`site/index.html`'s screenshots gallery).
//!
//! Same rationale as `contact_list_promo.rs`'s module doc: `firmware/src/ui/
//! screens/splash.rs` cannot itself be compiled on the host, so this module
//! copies `SplashScreenUi`'s markup VERBATIM in full — radar rings, radio
//! glyph, Cadet mascot, wordmark, version string, full-window backdrop +
//! lower-half line art — because the deliverable is a promotional
//! screenshot of the REAL screen. Renders the screen's
//! "static-complete" first frame (see `splash.rs`'s own module doc,
//! "Static-complete, then ripple" section): logo, wordmark and version are
//! already fully opaque, exactly the brand shot a landing page wants — the
//! radar-ripple animation itself has no single frame that reads better as a
//! static screenshot than the settled starting frame does. Imports the REAL
//! `theme.slint` / `motifs.slint` by relative path (not forked token values
//! or re-derived components).
//!
//! Slint enforces a process-wide `Platform` singleton, so this module's
//! render entry point must never run in the same process as any other
//! `ui_sim` render rig.

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

    // Verbatim copy of `splash.rs`'s markup — see this file's module doc
    // for why a copy (not an import) is used here.

    export component SplashPromoUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        SpaceBackdrop {}

        PlanetHorizon { x: 0px; y: 198px; }

        in property <string> version_str: "";

        in-out property <float> logo_opacity: 1;
        in-out property <float> title_opacity: 1;
        in-out property <float> version_opacity: 1;

        in-out property <float> ripple_active: 0;
        in-out property <float> ring1_size_px: 20;
        in-out property <float> ring1_opacity: 0.85;
        in-out property <float> ring2_size_px: 20;
        in-out property <float> ring2_opacity: 0.85;

        animate ring1_size_px { duration: 850ms; easing: ease-out-quad; iteration-count: -1; }
        animate ring1_opacity { duration: 850ms; easing: ease-out-quad; iteration-count: -1; }
        animate ring2_size_px { duration: 850ms; delay: 300ms; easing: ease-out-quad; iteration-count: -1; }
        animate ring2_opacity { duration: 850ms; delay: 300ms; easing: ease-out-quad; iteration-count: -1; }

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

            Text {
                text: "MeshCadet";
                opacity: title_opacity;
                font-size: Theme.size-display;
                font-weight: 700;
                color: Theme.brand-signal;
                horizontal-alignment: center;
            }

            Text {
                text: version_str;
                opacity: version_opacity;
                font-size: Theme.size-preview;
                color: Theme.text-muted;
                horizontal-alignment: center;
            }
        }
    }
}

struct SplashPromoPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for SplashPromoPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Render rig for the splash promo screenshot.
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `compose_send.rs::ComposeSendFrame::new`'s identical note. Callers must
/// ensure exactly one [`SplashPromoFrame::new`] runs per process.
pub struct SplashPromoFrame {
    window: Rc<MinimalSoftwareWindow>,
    ui: SplashPromoUi,
}

impl SplashPromoFrame {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(SplashPromoPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = SplashPromoUi::new().expect("SplashPromoUi::new");
        ui.show().expect("SplashPromoUi::show");

        SplashPromoFrame { window, ui }
    }

    pub fn set_version(&self, version: &str) {
        self.ui.set_version_str(version.into());
    }

    /// Advance Slint's animation clock and render one frame.
    pub fn render(&self) -> Vec<Rgb565Pixel> {
        slint::platform::update_timers_and_animations();
        self.window.request_redraw();

        let mut framebuffer = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
        let rendered = self.window.draw_if_needed(|renderer| {
            renderer.render(&mut framebuffer, WIDTH as usize);
        });
        assert!(
            rendered,
            "splash promo frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for SplashPromoFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a rendered RGB565 framebuffer to an `image::RgbImage` (RGB8) for
/// PNG export — same conversion every other `ui_sim` render module
/// duplicates locally.
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
