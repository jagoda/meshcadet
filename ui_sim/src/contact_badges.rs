// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig proving, pixel-for-pixel on
//! the host, that the unread-count badge markup `firmware/src/ui/screens/
//! contact_list.rs` actually ships (per-row `ContactRow` badge AND the
//! tab-bar aggregate badge) paints a visibly-distinct badge — not just "the
//! Rust side computed a nonzero count", which the existing
//! `format_unread_badge`/`unread_total_increased` unit tests in that module
//! already cover.
//!
//! # Why this is a separate, narrower render path from `HostSimUi` /
//! `motif_library` / `compose_send` / `gps_status_rows`
//!
//! `firmware/src/ui/screens/contact_list.rs` cannot itself be compiled on
//! the host — the `firmware` crate cross-compiles for
//! `xtensa-esp32s3-espidf` only (see `lib.rs`'s module doc for the full
//! explanation). This module re-declares ONLY the two badge mechanisms this
//! diagnosis needs to exercise: `ContactRow`'s per-row badge
//! Rectangle (`if unread > 0`) and the tab-bar's aggregate badge Rectangle
//! (`if *_unread_total > 0`) — copied verbatim from `contact_list.rs`'s real
//! markup, not re-derived. Everything else that screen themes (the
//! starfield header, comet-on-notify sweep, row rest-state wash) is
//! untouched by this diagnosis and already proven by
//! `motif_library.rs` (Starfield/CometOnNotify) — same "deliberately not a
//! pixel-for-pixel mirror" scoping every other `ui_sim` render module's own
//! doc establishes.
//!
//! Imports the REAL `theme.slint` by relative path (not forked token
//! values) — single source of truth, same technique every other `ui_sim`
//! render module uses.
//!
//! Slint enforces a process-wide `Platform` singleton, so this module's
//! render entry point must never run in the same process as any of the
//! other render rigs' — `ui_sim/tests/contact_badges.rs` is its own Cargo
//! integration-test binary (own process), same isolation technique those
//! modules' own docs explain.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

/// Per-row badge geometry, mirroring `contact_list.rs`'s `ContactRow` badge
/// Rectangle (`width: 20px; height: 20px;`).
pub const ROW_BADGE_SIZE: u32 = 20;

/// Tab-bar aggregate badge geometry, mirroring `contact_list.rs`'s tab badge
/// Rectangle (`width: 14px; height: 14px;`).
pub const TAB_BADGE_SIZE: u32 = 14;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";

    // Verbatim copy of `contact_list.rs`'s `ContactRow` per-row badge
    // Rectangle — see this file's module doc for why a copy rather than an
    // import.
    component RowBadge {
        in property <int>    unread;
        in property <string> unread_str;

        width: 20px;
        height: 20px;
        // Surrounding row background, so a badge that fails to paint over
        // it is indistinguishable from "no badge" rather than a false pass
        // against a bare-transparent Window backdrop.
        Rectangle {
            background: Theme.nebula-violet-deep.with-alpha(0.12);
            if unread > 0 : Rectangle {
                width: 20px;
                height: 20px;
                border-radius: 10px;
                background: Theme.brand-signal;
                Text {
                    text: unread_str;
                    font-size: Theme.size-caption;
                    font-weight: 700;
                    color: Theme.text-primary;
                    horizontal-alignment: center;
                    vertical-alignment: center;
                }
            }
        }
    }

    // Verbatim copy of `contact_list.rs`'s tab-bar aggregate badge
    // Rectangle.
    component TabBadge {
        in property <int>    total;
        in property <string> total_str;

        width: 14px;
        height: 14px;
        Rectangle {
            background: Theme.surface.with-alpha(0.55);
            if total > 0 : Rectangle {
                width: 14px;
                height: 14px;
                border-radius: 7px;
                background: Theme.brand-signal;
                Text {
                    text: total_str;
                    font-size: Theme.size-badge;
                    font-weight: 700;
                    color: Theme.text-primary;
                    horizontal-alignment: center;
                    vertical-alignment: center;
                }
            }
        }
    }

    export component ContactBadgesUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        in property <int>    row_unread;
        in property <string> row_unread_str;
        in property <int>    tab_total;
        in property <string> tab_total_str;

        HorizontalLayout {
            spacing: 8px;
            padding: 8px;
            row_badge := RowBadge {
                unread: root.row_unread;
                unread_str: root.row_unread_str;
            }
            tab_badge := TabBadge {
                total: root.tab_total;
                total_str: root.tab_total_str;
            }
        }
    }
}

struct ContactBadgesPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for ContactBadgesPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// One rendered frame of the two badge rigs (row badge at `(8, 8)`..`(28,
/// 28)`, tab badge at `(36, 8)`..`(50, 22)` — the `HorizontalLayout`'s
/// `padding: 8px` + `spacing: 8px` place them deterministically).
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `motif_library.rs::MotifLibraryFrame::new`'s identical note. Callers must
/// ensure exactly one [`ContactBadgesFrame::new`] runs per process.
pub struct ContactBadgesFrame {
    window: Rc<MinimalSoftwareWindow>,
    ui: ContactBadgesUi,
}

impl ContactBadgesFrame {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(ContactBadgesPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = ContactBadgesUi::new().expect("ContactBadgesUi::new");
        ui.show().expect("ContactBadgesUi::show");

        ContactBadgesFrame { window, ui }
    }

    pub fn set_row_unread(&self, unread: i32, unread_str: &str) {
        self.ui.set_row_unread(unread);
        self.ui.set_row_unread_str(unread_str.into());
    }

    pub fn set_tab_total(&self, total: i32, total_str: &str) {
        self.ui.set_tab_total(total);
        self.ui.set_tab_total_str(total_str.into());
    }

    /// Render one frame at the currently-set property values.
    pub fn render(&self) -> Vec<Rgb565Pixel> {
        slint::platform::update_timers_and_animations();
        self.window.request_redraw();

        let mut framebuffer = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
        let rendered = self.window.draw_if_needed(|renderer| {
            renderer.render(&mut framebuffer, WIDTH as usize);
        });
        assert!(
            rendered,
            "contact-badges frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for ContactBadgesFrame {
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
