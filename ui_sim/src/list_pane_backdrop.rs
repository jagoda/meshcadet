// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig proving the shared mechanism the four fan-out screens
//! (`contact_list.rs`, `message_view.rs`, `compose.rs`) all newly rely on —
//! a full-window `SpaceBackdrop` sitting BEHIND a translucent list-row wash,
//! a bare-`transparent` content pane, and a translucent bottom action bar
//! that itself hosts an opaque button pill — actually composites the way
//! the Objective describes, not just "the markup parses".
//!
//! # Why this is a separate, narrower render path from the other `ui_sim` rigs
//!
//! None of `contact_list.rs`/`message_view.rs`/`compose.rs` can themselves be
//! compiled on the host (the `firmware` crate cross-compiles for
//! `xtensa-esp32s3-espidf` only — see `gps_status_rows.rs`'s module doc for
//! the full explanation this crate's every render rig repeats). `SpaceBackdrop`
//! itself is already proven by `motif_library.rs`'s `SpaceBackdropFrame`
//! and its
//! composition behind dense STATIC content by that same contract plus
//! `splash_lineart.rs`. What is NEW here — and previously unproven — is the
//! combination this rig introduces: a **translucent, alpha-blended**
//! foreground layer (not an opaque stand-in Rectangle) sitting on top of the
//! backdrop, in a *scrolling-list* row and in a *bottom action bar*, with an
//! *opaque* button pill nested inside that bar. This module copies the exact
//! background VALUES the three real screens currently use (verbatim, not
//! re-derived — see each Rectangle's own comment below for its source) so
//! this proof is checked against the screens' actual tokens, not an
//! approximation.
//!
//! Slint enforces a process-wide `Platform` singleton, so this module's
//! render entry point must never run in the same process as any of the
//! other render rigs' — `ui_sim/tests/list_pane_backdrop.rs` is its own
//! Cargo integration-test binary (own process), same isolation technique
//! every other `ui_sim` render module's doc explains. That test renders
//! `ListPaneBackdropUi` TWICE (overlays hidden, then shown) from the SAME
//! window/platform — same "multiple snapshots, one process" technique
//! `motif_library.rs::MotifLibraryFrame` already established — rather than
//! installing two platforms, which would panic.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import { SpaceBackdrop } from "../../firmware/src/ui/motifs.slint";

    export component ListPaneBackdropUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        // Toggles the three foreground overlays below — `false` captures
        // the bare backdrop alone (this scene's baseline), `true` layers
        // the list row / content pane / action bar on top of it, mirroring
        // the real screens' current markup.
        in property <bool> show_overlays: false;

        // Full-window dim starfield backdrop — same z-bottom placement
        // every consumer screen uses.
        SpaceBackdrop {}

        // List row — verbatim value of `contact_list.rs`'s unselected
        // `ContactRow` fill (`Theme.nebula-violet-deep.with-alpha(0.12)`,
        // predates this rig but is the exact mechanism this rig
        // now composites the backdrop underneath).
        if show_overlays : Rectangle {
            x: 0px;
            y: 40px;
            width: 320px;
            height: 54px;
            background: Theme.nebula-violet-deep.with-alpha(0.12);
        }

        // Content pane — verbatim value of `compose.rs`'s draft text area
        // current fill (bare `transparent`, replacing the prior opaque
        // `Theme.bg-space`).
        if show_overlays : Rectangle {
            x: 0px;
            y: 100px;
            width: 320px;
            height: 60px;
            background: transparent;
        }

        // Bottom action bar — verbatim value of `message_view.rs`'s
        // Compose-button bar / `compose.rs`'s action bar current fill
        // (`Theme.surface.with-alpha(0.55)`, replacing the prior opaque
        // `Theme.bg-space`/`Theme.surface`), with an opaque button pill
        // nested inside it (verbatim fill class of the Send/Write buttons,
        // `Theme.brand-signal`) — proving the bar's own translucency does
        // not bleed into the button's legibility.
        if show_overlays : Rectangle {
            x: 0px;
            y: 200px;
            width: 320px;
            height: 40px;
            background: Theme.surface.with-alpha(0.55);

            Rectangle {
                x: 120px;
                y: 6px;
                width: 80px;
                height: 28px;
                background: Theme.brand-signal;
            }
        }
    }
}

struct ListPaneBackdropPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for ListPaneBackdropPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Renders `ListPaneBackdropUi` at whatever `show_overlays` state the caller
/// sets via [`Self::set_show_overlays`] before calling [`Self::render`].
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `motif_library.rs::MotifLibraryFrame::new`'s identical note. Callers must
/// ensure exactly one [`ListPaneBackdropFrame::new`] runs per process.
pub struct ListPaneBackdropFrame {
    window: Rc<MinimalSoftwareWindow>,
    ui: ListPaneBackdropUi,
}

impl ListPaneBackdropFrame {
    pub fn new() -> Self {
        // `NewBuffer`: this rig calls `render()` twice per process (overlays
        // hidden, then shown — see this struct's doc), each a fresh,
        // self-contained repaint, same reasoning as
        // `motif_library::MotifLibraryFrame::new`'s identical choice.
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(ListPaneBackdropPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = ListPaneBackdropUi::new().expect("ListPaneBackdropUi::new");
        ui.show().expect("ListPaneBackdropUi::show");

        ListPaneBackdropFrame { window, ui }
    }

    pub fn set_show_overlays(&self, v: bool) {
        self.ui.set_show_overlays(v);
    }

    /// Advance Slint's animation clock and render one frame.
    pub fn render(&self) -> Vec<Rgb565Pixel> {
        slint::platform::update_timers_and_animations();
        self.window.request_redraw();

        let mut framebuffer = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
        let rendered = self.window.draw_if_needed(|renderer| {
            renderer.render(&mut framebuffer, WIDTH as usize);
        });
        assert!(rendered, "list-pane-backdrop frame was not dirty — nothing painted");
        framebuffer
    }
}

impl Default for ListPaneBackdropFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a rendered RGB565 framebuffer to an `image::RgbImage` (RGB8) for
/// PNG export — same conversion every other `ui_sim` render module uses.
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
