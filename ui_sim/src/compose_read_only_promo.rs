// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig proving, pixel-for-pixel on the host, Phase B's
//! ("permission-aware UI") compose-screen contract from
//! `meshcadet-room-firmware-post-and-notify`: a `GUEST`/`READ_ONLY` room
//! session renders compose **disabled with an explicit read-only
//! indicator** — never a compose box for a message the server will
//! silently swallow (`RoomPermission::can_post()`'s doc,
//! `MyMesh.cpp:466`).
//!
//! # Why this is a separate, narrower render path from `HostSimUi` /
//! `compose_send` / `compose_promo`
//!
//! `firmware/src/ui/screens/compose.rs` cannot itself be compiled on the
//! host — the `firmware` crate cross-compiles for `xtensa-esp32s3-espidf`
//! only (see `lib.rs`'s module doc for the full explanation). This module
//! re-declares ONLY the two mechanisms Phase B added to that screen's real
//! markup: the `read_only`-gated Send-button color/text (verbatim copy of
//! `ComposeScreenUi`'s `background: (draft != "" && !read_only) ? …`
//! ternary) and the `if read_only` banner Text. Everything else that screen
//! themes (header, draft `TextInput`, emoji picker, autocomplete bar,
//! rocket-on-send) is untouched by Phase B and already proven by
//! `compose_send.rs`/`compose_promo.rs` — same "deliberately not a
//! pixel-for-pixel mirror" scoping every other `ui_sim` render module's own
//! doc establishes.
//!
//! Imports the REAL `theme.slint` by relative path (not forked token
//! values) — single source of truth, same technique every other `ui_sim`
//! render module uses.
//!
//! Slint enforces a process-wide `Platform` singleton, so this module's
//! render entry point must never run in the same process as any other render
//! rig's — `ui_sim/tests/compose_read_only.rs` is its own Cargo
//! integration-test binary (own process), same isolation technique those
//! modules' own docs explain.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

/// Read-only banner geometry — a fixed, solid-fill Rectangle (not just the
/// glyph text) so its presence is reliably sampled at one pixel rather than
/// depending on anti-aliased text-glyph coverage.
pub const BANNER_X: u32 = 8;
pub const BANNER_Y: u32 = 8;
pub const BANNER_W: u32 = 200;
pub const BANNER_H: u32 = 16;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";

    export component ComposeReadOnlyUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        // Mirrors `ComposeScreenUi.draft != ""` — whether a draft is
        // present.
        in property <bool> has_draft: false;
        // Mirrors `ComposeScreenUi.read_only` (Phase B).
        in property <bool> read_only: false;

        // Read-only indicator — verbatim-shape copy of `compose.rs`'s `if
        // read_only : Text { … }` banner, but backed by a solid Rectangle
        // fill (`Theme.warn`) so this rig's pixel assertions don't depend on
        // glyph anti-aliasing.
        if read_only : Rectangle {
            x: 8px; y: 8px;
            width: 200px; height: 16px;
            background: Theme.warn;
            Text {
                text: "🔒 Read-only — you can't post in this room";
                font-size: Theme.size-meta;
                color: Theme.bg-space;
                vertical-alignment: center;
            }
        }

        // Send button, positioned/sized exactly like `compose.rs`'s own —
        // same 80x28 box. `(has_draft && !read_only)` mirrors
        // `ComposeScreenUi`'s exact ternary (Phase B).
        send_button := Rectangle {
            x: 320px - 80px - 8px;
            y: 240px - 28px - 8px;
            width: 80px; height: 28px;
            background: (has_draft && !read_only) ? Theme.star-gold : Theme.surface-raised;
            border-radius: 14px;
        }
    }
}

struct ComposeReadOnlyPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for ComposeReadOnlyPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// One rendered frame of the read-only compose indicator, at whatever
/// `has_draft`/`read_only` state the caller sets before calling
/// [`ComposeReadOnlyFrame::render`].
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `motif_library.rs::MotifLibraryFrame::new`'s identical note. Callers must
/// ensure exactly one [`ComposeReadOnlyFrame::new`] runs per process.
pub struct ComposeReadOnlyFrame {
    window: Rc<MinimalSoftwareWindow>,
    ui: ComposeReadOnlyUi,
}

impl ComposeReadOnlyFrame {
    pub fn new() -> Self {
        // `NewBuffer`: this rig calls `render()` multiple times per process
        // (read-write idle/armed, read-only idle/armed) — same
        // full-repaint-every-call reasoning as `compose_send.rs`'s identical
        // choice.
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(ComposeReadOnlyPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = ComposeReadOnlyUi::new().expect("ComposeReadOnlyUi::new");
        ui.show().expect("ComposeReadOnlyUi::show");

        ComposeReadOnlyFrame { window, ui }
    }

    pub fn set_has_draft(&self, v: bool) {
        self.ui.set_has_draft(v);
    }

    pub fn set_read_only(&self, v: bool) {
        self.ui.set_read_only(v);
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
            "compose-read-only frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for ComposeReadOnlyFrame {
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
