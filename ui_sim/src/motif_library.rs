// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig for the shared motif library (M2).
//!
//! # Why this is a SEPARATE component/render path from `lib.rs`'s
//! `HostSimUi` / `render_host_sim_frame`
//!
//! That pair is M1's exact walking-skeleton proof, already independently
//! re-verified pixel-for-pixel against a specific
//! landed commit (`e6e9a3e`). Leaving it
//! untouched keeps that evidence trail mechanical rather than "trust the
//! diff". This module instead proves M2's OWN deliverable: every static
//! celestial/mascot component and every one-shot motion helper exported by
//! `firmware/src/ui/motifs.slint` actually compiles and paints correct
//! pixels. It imports the REAL `motifs.slint` (and, transitively, the real
//! `theme.slint`) by relative path — not a fork.
//!
//! Slint enforces a process-wide `Platform` singleton (see `lib.rs`'s own
//! `render_host_sim_frame` panic-note), so this module's render entry point
//! must never run in the same process as `lib.rs`'s — `ui_sim/tests/
//! motif_library.rs` is a separate Cargo integration-test BINARY (each file
//! under `tests/` gets its own process), which is exactly what keeps this
//! collision-free from the existing `#[cfg(test)]` module in `lib.rs`.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import {
        Starfield, RingedPlanetCorner, CrescentMoon, Comet, Rocket,
        CadetIdle, CadetWave, CadetThumbsUp, CadetSleeping, CadetPeeking,
        MascotBob, Twinkle, RocketOnSend, CometOnNotify,
    } from "../../firmware/src/ui/motifs.slint";

    export component MotifLibraryUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.space-deep;

        // Forwarded to the two retriggerable one-shot motion helpers below
        // — the same "Rust sets a top-level property, Slint's `animate`
        // fires on the resulting value change" contract every other
        // themed screen in this codebase uses (see `splash.rs`'s
        // `start_animation()`).
        in-out property <bool> send_trigger: false;
        in-out property <bool> notify_trigger: false;

        // ── Static celestial scenery ─────────────────────────────────────
        Starfield { x: 0px; y: 0px; }
        RingedPlanetCorner { x: 8px; y: 44px; }
        CrescentMoon { x: 60px; y: 50px; }
        Comet { x: 104px; y: 58px; }
        Rocket { x: 148px; y: 44px; }

        // ── Mascot poses ──────────────────────────────────────────────────
        CadetIdle { x: 4px; y: 92px; }
        CadetWave { x: 72px; y: 92px; }
        CadetThumbsUp { x: 140px; y: 92px; }
        CadetSleeping { x: 208px; y: 92px; }
        CadetPeeking { x: 4px; y: 160px; }

        // ── Motion helpers ──────────────────────────────────────────────
        MascotBob {
            x: 100px;
            y: 160px;
            source: @image-url("../../firmware/assets/space/cadet_idle.png");
        }
        Twinkle { x: 280px; y: 170px; twinkle-color: Theme.star-gold; }
        RocketOnSend { x: 288px; y: 160px; play: root.send_trigger; }
        CometOnNotify { x: 0px; y: 226px; play: root.notify_trigger; }
    }
}

struct MotifLibraryPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for MotifLibraryPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// One rendered frame of the motif library, at whatever `send_trigger`/
/// `notify_trigger` state the caller sets via the returned handle before
/// calling [`MotifLibraryFrame::render`].
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `lib.rs::render_host_sim_frame`'s identical note. Callers must ensure
/// exactly one [`MotifLibraryFrame::new`] runs per process (this crate's
/// `tests/motif_library.rs` integration-test binary gets its own process,
/// distinct from `lib.rs`'s `#[cfg(test)]` module).
pub struct MotifLibraryFrame {
    window: Rc<MinimalSoftwareWindow>,
    ui: MotifLibraryUi,
}

impl MotifLibraryFrame {
    pub fn new() -> Self {
        // `NewBuffer`, not `ReusedBuffer`: this rig calls `render()` multiple
        // times per process (rest state, settled state, mid-flight state —
        // see this struct's doc), handing a FRESH zeroed `Vec` to each call.
        // `ReusedBuffer` tells the renderer it may assume the incoming buffer
        // already holds the PREVIOUS frame's pixels and skip repainting
        // regions that aren't dirty since last frame — with a fresh buffer
        // each time, that would leave every non-dirty region at its zeroed
        // (black) initial value instead of the correct settled content.
        // `NewBuffer` makes every call a full, self-contained repaint, which
        // is what this multi-snapshot usage actually needs. `lib.rs`'s
        // `HostSimUi` render path is unaffected (it calls `render()` exactly
        // once per process, so the distinction is a no-op there).
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(MotifLibraryPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = MotifLibraryUi::new().expect("MotifLibraryUi::new");
        ui.show().expect("MotifLibraryUi::show");

        MotifLibraryFrame { window, ui }
    }

    pub fn set_send_trigger(&self, v: bool) {
        self.ui.set_send_trigger(v);
    }

    pub fn set_notify_trigger(&self, v: bool) {
        self.ui.set_notify_trigger(v);
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
            "motif-library frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for MotifLibraryFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a rendered RGB565 framebuffer to an `image::RgbImage` (RGB8) for
/// PNG export — same conversion `lib.rs::framebuffer_to_rgb_image` does,
/// duplicated locally so this module has no dependency on `lib.rs`'s
/// `#[cfg(test)]`-adjacent internals (both are trivial, stable
/// 5/6/5-to-8-bit expansions).
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

// ── Full-window backdrop + lower-half line art proof ─────────────────────
// A SEPARATE Slint window/component + render-frame struct from
// `MotifLibraryUi`/`MotifLibraryFrame` above, deliberately not folded into
// that scene: the existing `motif_library_renders_every_asset_and_motion_
// helper` test asserts specific pixel colors at fixed offsets across that
// entire 320x240 canvas, and `SpaceBackdrop` is itself a full-window
// (320x240) layer — compositing it into the SAME scene risks a stray
// (if dim) star pixel landing on one of those already-verified assertion
// coordinates, turning a passing, mechanically-proven test fragile for
// reasons unrelated to this proof's own deliverable. Keeping the new
// components' proof in their own window (same file, same overall pattern —
// inline `slint::slint!{}` macro + `MinimalSoftwareWindow` + a dedicated
// `Platform` impl) is additive and collision-free instead.

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import { SpaceBackdrop, PlanetHorizon } from "../../firmware/src/ui/motifs.slint";

    export component SpaceBackdropDemoUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.space-deep;

        // `SpaceBackdrop` first (z-bottom, the same "first child of Window"
        // contract every real consumer screen follows) with a plain
        // `Rectangle` standing in for a screen's live text/content on top,
        // proving the backdrop composites BEHIND foreground content rather
        // than asserting anything about final on-hardware legibility (that
        // sign-off is deferred to the HIL gate).
        SpaceBackdrop {}
        Rectangle {
            x: 40px;
            y: 100px;
            width: 240px;
            height: 40px;
            background: Theme.bg-space;
        }

        // `PlanetHorizon` in the lower band, same placement Phase 3's
        // `splash.rs` is expected to use (bottom-anchored, full width).
        PlanetHorizon { x: 0px; y: 168px; }
    }
}

struct SpaceBackdropPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for SpaceBackdropPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// One rendered frame of the `SpaceBackdrop`/`PlanetHorizon` demo scene.
///
/// # Panics
/// Same process-wide-`Platform`-singleton constraint as
/// [`MotifLibraryFrame::new`] — callers must ensure exactly one Slint
/// `Platform` is installed per process (this crate's `tests/
/// space_backdrop.rs` integration-test binary gets its own process).
pub struct SpaceBackdropFrame {
    window: Rc<MinimalSoftwareWindow>,
    #[allow(dead_code)] // kept alive for the duration of the frame's window
    ui: SpaceBackdropDemoUi,
}

impl SpaceBackdropFrame {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(SpaceBackdropPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = SpaceBackdropDemoUi::new().expect("SpaceBackdropDemoUi::new");
        ui.show().expect("SpaceBackdropDemoUi::show");

        SpaceBackdropFrame { window, ui }
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
            "space-backdrop frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for SpaceBackdropFrame {
    fn default() -> Self {
        Self::new()
    }
}
