// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig for the compose screen's theme
//! (per-screen spec row 5: "star-gold send
//! affordance" / "rocket-on-send").
//!
//! # Why this is a separate, narrower render path from `HostSimUi` /
//! `motif_library`
//!
//! `firmware/src/ui/screens/compose.rs` cannot itself be compiled on the
//! host — the `firmware` crate cross-compiles for `xtensa-esp32s3-espidf`
//! only (see `lib.rs`'s module doc for the full explanation). This module
//! re-declares ONLY the one previously-unproven mechanism this
//! theming pass touches: the Send button's `Theme.star-gold` enabled-state
//! accent, plus its nested `RocketOnSend` one-shot + the `Timer` that
//! auto-resets its trigger. Everything else `compose.rs` themes (the
//! header, draft text area, emoji picker, autocomplete bar) is either
//! untouched by this pass or pure-Slint `Theme`-token/`animate` idiom
//! already proven by every other themed screen in this codebase — same
//! "deliberately not a pixel-for-pixel mirror" scoping `lib.rs`'s
//! `HostSimUi` module doc establishes for its own narrower proof.
//!
//! Imports the REAL `theme.slint` / `motifs.slint` by relative path (not
//! forked token values or a re-derived `RocketOnSend`) — single source of
//! truth, same technique every other `ui_sim` render module uses.
//!
//! Slint enforces a process-wide `Platform` singleton, so this module's
//! render entry point must never run in the same process as `lib.rs`'s or
//! `motif_library`'s — `ui_sim/tests/compose_send.rs` is its own Cargo
//! integration-test binary (own process), same isolation technique
//! `motif_library.rs`'s own doc explains.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import { RocketOnSend } from "../../firmware/src/ui/motifs.slint";

    export component ComposeSendUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        // Mirrors `ComposeScreenUi.draft != ""` — whether the Send button
        // reads as "armed" (star-gold) or idle (surface-raised).
        in property <bool> has_draft: false;

        // Mirrors `ComposeScreenUi.rocket_trigger` + its sibling `Timer` —
        // identical contract, identical 500ms auto-reset.
        in-out property <bool> rocket_trigger: false;
        Timer {
            interval: 500ms;
            running: root.rocket_trigger;
            triggered => { root.rocket_trigger = false; }
        }

        // Send button, positioned/sized exactly like `compose.rs`'s own —
        // same 80x28 box, same nested-`RocketOnSend`-floats-above technique.
        send_button := Rectangle {
            x: 320px - 80px - 8px;
            y: 240px - 28px - 8px;
            width: 80px; height: 28px;
            background: has_draft ? Theme.star-gold : Theme.surface-raised;
            border-radius: 14px;

            RocketOnSend {
                x: parent.width / 2 - self.width / 2;
                y: -20px;
                play: root.rocket_trigger;
            }
        }
    }
}

struct ComposeSendPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for ComposeSendPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// One rendered frame of the compose Send button, at whatever `has_draft`/
/// `rocket_trigger` state the caller sets before calling
/// [`ComposeSendFrame::render`].
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `motif_library.rs::MotifLibraryFrame::new`'s identical note. Callers
/// must ensure exactly one [`ComposeSendFrame::new`] runs per process.
pub struct ComposeSendFrame {
    window: Rc<MinimalSoftwareWindow>,
    ui: ComposeSendUi,
}

impl ComposeSendFrame {
    pub fn new() -> Self {
        // `NewBuffer`: this rig calls `render()` multiple times per process
        // (idle, armed, mid-flight, settled, reset states) — same
        // full-repaint-every-call reasoning as `motif_library.rs`'s
        // identical choice.
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(ComposeSendPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = ComposeSendUi::new().expect("ComposeSendUi::new");
        ui.show().expect("ComposeSendUi::show");

        ComposeSendFrame { window, ui }
    }

    pub fn set_has_draft(&self, v: bool) {
        self.ui.set_has_draft(v);
    }

    pub fn get_rocket_trigger(&self) -> bool {
        self.ui.get_rocket_trigger()
    }

    pub fn set_rocket_trigger(&self, v: bool) {
        self.ui.set_rocket_trigger(v);
    }

    /// Advance Slint's animation/timer clock and render one frame.
    pub fn render(&self) -> Vec<Rgb565Pixel> {
        slint::platform::update_timers_and_animations();
        self.window.request_redraw();

        let mut framebuffer = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
        let rendered = self.window.draw_if_needed(|renderer| {
            renderer.render(&mut framebuffer, WIDTH as usize);
        });
        assert!(rendered, "compose-send frame was not dirty — nothing painted");
        framebuffer
    }
}

impl Default for ComposeSendFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a rendered RGB565 framebuffer to an `image::RgbImage` (RGB8) for
/// PNG export — same conversion every other `ui_sim` render module
/// duplicates locally (see `motif_library.rs`'s identical function doc for
/// why: no shared dependency on `lib.rs`'s `#[cfg(test)]`-adjacent
/// internals).
pub fn framebuffer_to_rgb_image(framebuffer: &[Rgb565Pixel], width: u32, height: u32) -> image::RgbImage {
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
    (((r5 << 3) | (r5 >> 2)) as u8, ((g6 << 2) | (g6 >> 4)) as u8, ((b5 << 3) | (b5 >> 2)) as u8)
}
