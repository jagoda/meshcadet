// SPDX-License-Identifier: GPL-3.0-only
//! Slint `Platform` implementation for the T-Deck Plus.
//!
//! # Overview
//!
//! Slint's embedded "software renderer" path requires the host application to
//! supply a `Platform` implementation that:
//!
//! 1. Reports the system clock (`duration_since_start`).
//! 2. Creates the `WindowAdapter` that owns the `SoftwareRenderer`.
//! 3. Optionally implements `run_event_loop` (unused here — we drive the loop
//!    cooperatively from the radio dispatcher).
//!
//! We use Slint's canonical [`MinimalSoftwareWindow`] as the window adapter; it
//! owns the [`SoftwareRenderer`] and tracks the dirty/redraw state.  A clone of
//! the same `Rc` is registered with Slint (via `create_window_adapter`) and
//! returned to the caller (via [`TDeckPlatform::install`]) so the cooperative
//! loop can drive rendering without a fragile downcast.
//!
//! `TDeckPlatform::install()` calls `slint::platform::set_platform()` once at
//! startup (panics if called more than once — Slint enforces a singleton).
//!
//! # Cooperative loop integration
//!
//! Instead of calling `slint::run_event_loop()`, the radio dispatcher calls:
//!
//! ```rust,ignore
//! // Once per dispatcher loop iteration:
//! slint::platform::update_timers_and_animations();
//! window_handle.render_if_needed(&mut display)?;
//! if let Some(ev) = touch.poll_event(now_ms)? { window_handle.dispatch_touch(ev); }
//! ```
//!
//! This keeps Slint ticking without blocking the radio loop.  At idle (no
//! animations, no redraws), `update_timers_and_animations` returns in < 10 µs
//! and `render_if_needed` is a no-op (nothing dirty).

use std::rc::Rc;
use std::time::Duration;

use slint::platform::software_renderer::{
    LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel,
};
use slint::platform::{Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{PhysicalPosition, PhysicalSize};

use crate::ui::display::{TDeckDisplay, DISPLAY_HEIGHT, DISPLAY_WIDTH};

// Build-time generated combined bitmap font: ASCII + UI symbols + 40 curated emoji.
// `gen_emoji_font.c` (compiled in build.rs) rasterises DejaVu Sans (Latin) and
// NotoEmoji-Regular (emoji) via FreeType into static BitmapGlyph arrays at EVERY
// font-size the UI uses (8/9/10/11/13/14/15/16/18/20/22/28 px; emoji only at the
// subset where they actually appear — see EMOJI_SIZES in gen_emoji_font.c).
//
// This font is registered FIRST in `install()` so it is the global fallback for
// ALL Slint Text elements.  Registering it globally is REQUIRED, not incidental:
// the Slint software renderer resolves an entire text run to ONE font and does
// NO per-glyph fallback (i-slint-renderer-software pixelfont.rs `shape_text` —
// a char absent from the selected font renders blank).  Dynamic message bodies
// mix Latin + emoji in a single run, so the serving font must cover both; an
// "emoji-only, scoped via font-family" approach cannot render those runs.
//
// The renderer also snaps each request to the NEAREST available pixel size and
// scales the glyph metrics — so the font MUST be rasterised at every UI size or
// text at an unlisted size renders too small/large with a wrong baseline (the
// "garbled text" defect this font's size list fixes).
mod emoji_font {
    use i_slint_core::graphics::{BitmapFont, BitmapGlyph, BitmapGlyphs, CharacterMapEntry};
    use i_slint_core::slice::Slice;
    include!(concat!(env!("OUT_DIR"), "/emoji_font.rs"));
}
use crate::ui::touch::{TouchEvent, TouchKind};

/// Slint platform backed by the T-Deck display + esp_timer.
///
/// Holds a clone of the single [`MinimalSoftwareWindow`] adapter so that
/// `create_window_adapter` can hand it to Slint while the application keeps its
/// own handle for cooperative rendering.
struct TDeckPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl TDeckPlatform {
    /// Register this as Slint's global platform and return the rendering handle.
    ///
    /// Call exactly once, before any Slint window/component is created.
    ///
    /// # Panics
    ///
    /// Panics if Slint already has a platform registered (would be a
    /// programming error — call this before any slint window creation).
    pub fn install() -> Rc<TDeckWindowAdapter> {
        // `ReusedBuffer`: line-buffer rendering reuses one strip; partial
        // (dirty-region) rendering is retained across frames.
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(DISPLAY_WIDTH, DISPLAY_HEIGHT));

        slint::platform::set_platform(Box::new(TDeckPlatform { window: window.clone() }))
            .expect("Slint platform already set");

        // Register the combined emoji + Latin bitmap font BEFORE any component inits.
        // Component init (ComposeScreenUi::new etc.) also registers per-screen
        // SLINT_EMBED_TEXTURES fonts, but those only cover each screen's literals
        // at that screen's sizes and would shadow each other across screens.  Ours
        // is registered first and covers full ASCII + UI symbols + emoji at every
        // UI size, so the renderer's weight-nearest / first-registered fallback
        // resolves EVERY text run (static labels, dynamic message bodies, and
        // dynamic emoji like `cell.codepoint_str`) to it — at the exact requested
        // size, eliminating size-snapping. See the `emoji_font` module comment.
        window.renderer().register_bitmap_font(emoji_font::emoji_bitmap_font());

        Rc::new(TDeckWindowAdapter { window })
    }
}

impl Platform for TDeckPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        let us = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
        Duration::from_micros(us as u64)
    }

    // `run_event_loop` is intentionally NOT overridden.  The default
    // implementation returns an error if called, which is what we want —
    // cooperative callers must NOT call `slint::run_event_loop()`.
}

/// Cooperative rendering handle for the T-Deck display.
///
/// Wraps the shared [`MinimalSoftwareWindow`] and exposes `render_if_needed`
/// (called once per dispatcher loop) and `dispatch_touch` (called on each
/// GT911 touch event).
pub struct TDeckWindowAdapter {
    window: Rc<MinimalSoftwareWindow>,
}

impl TDeckWindowAdapter {
    /// Render the Slint scene to the display, if any region is dirty.
    ///
    /// Uses the `SoftwareRenderer` in line-buffer mode: one
    /// `DISPLAY_WIDTH`-pixel-wide RGB565 line at a time, avoiding the ~150 KB
    /// full-frame buffer.  A no-op when nothing is dirty.
    pub fn render_if_needed(&self, display: &mut TDeckDisplay<'_>) -> anyhow::Result<()> {
        self.window.draw_if_needed(|renderer| {
            renderer.render_by_line(TDeckLineRenderer { display });
        });
        Ok(())
    }

    /// Request a full repaint of the window on the next `render_if_needed`.
    ///
    /// Swapping the active Slint component clears the partial-render cache, but
    /// the cooperative loop only repaints when `needs_redraw` is set.  After a
    /// screen swap (navigation) the runtime calls this so the incoming screen
    /// actually surfaces on the panel instead of leaving the previous screen's
    /// pixels on the display.
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// `true` if Slint still has at least one animation in flight as of the
    /// last `render_if_needed` call that actually drew a frame.
    ///
    /// This is Slint's own documented signal for exactly this purpose — see
    /// `i-slint-core`'s `platform::update_timers_and_animations` doc: "Only
    /// go to sleep if `Window::has_active_animations()` returns false". The
    /// flag is refreshed lazily: `update_timers_and_animations()` resets it
    /// to `false` each tick, and it only flips back to `true` when a render
    /// pass actually reads a still-interpolating animated property (e.g. an
    /// `opacity`/position binding under an in-flight `animate`). That makes
    /// it accurate ONLY immediately after a render call that touched the
    /// scene — see `UiRuntime::step()`'s render-throttle call site, the one
    /// caller of this method, for the exact sequencing that keeps it valid.
    pub fn has_active_animations(&self) -> bool {
        self.window.has_active_animations()
    }

    /// Dispatch a touch event from the GT911 driver into the Slint window.
    ///
    /// Applies the Deg90-CCW rotation transform so GT911 native portrait
    /// coordinates map to Slint logical landscape coordinates:
    ///
    /// ```text
    /// GT911 native (portrait 240×320)  →  Slint logical (landscape 320×240)
    ///   lx = raw_y                                   [portrait Y → landscape X]
    ///   ly = (GT911_NATIVE_WIDTH − 1) − raw_x        [portrait X → landscape Y, reversed]
    /// ```
    ///
    /// Returns the logical `(x, y)` that was dispatched (useful for diagnostics).
    pub fn dispatch_touch(&self, ev: TouchEvent) -> (i32, i32) {
        // ── Coordinate transform: GT911 portrait → Slint landscape ───────────
        //
        // The ST7789V2 is configured `Rotation::Deg90` (mipidsi), yielding a
        // 320 × 240 landscape logical surface from the native 240 × 320 portrait
        // panel.  The GT911 reports touches in the native portrait space:
        //   raw_x ∈ [0..239]  (portrait left→right)
        //   raw_y ∈ [0..319]  (portrait top→bottom)
        //
        // For a 90° CCW rotation the transform is:
        //   lx = raw_y                                [portrait-Y → landscape-X]
        //   ly = (GT911_NATIVE_WIDTH − 1) − raw_x    [invert portrait-X → landscape-Y]
        //
        // Corner expectations (tap and verify with --features diagnostics):
        //   Landscape top-left    (lx=  0, ly=  0): GT911 raw ≈ (239,   0)
        //   Landscape top-right   (lx=319, ly=  0): GT911 raw ≈ (239, 319)
        //   Landscape bottom-left (lx=  0, ly=239): GT911 raw ≈ (  0,   0)
        //   Landscape bottom-right(lx=319, ly=239): GT911 raw ≈ (  0, 319)
        //
        // GT911 X-axis range in native portrait mode (panel width in portrait).
        const GT911_NATIVE_WIDTH: u16 = 240;

        let lx = ev.point.y as i32;
        let ly = (GT911_NATIVE_WIDTH - 1).saturating_sub(ev.point.x) as i32;

        let pos = PhysicalPosition::new(lx, ly);
        let slint_ev = match ev.kind {
            TouchKind::Pressed => WindowEvent::PointerPressed {
                position: pos.to_logical(1.0),
                button: PointerEventButton::Left,
            },
            TouchKind::Moved => WindowEvent::PointerMoved {
                position: pos.to_logical(1.0),
            },
            TouchKind::Released => WindowEvent::PointerReleased {
                position: pos.to_logical(1.0),
                button: PointerEventButton::Left,
            },
        };
        self.window.dispatch_event(slint_ev);
        (lx, ly)
    }

    /// Dispatch a decoded keyboard key into the focused Slint item.
    ///
    /// The T-Deck keyboard co-processor reports one key-press at a time with no
    /// hardware release event, so we synthesise a `KeyPressed`/`KeyReleased`
    /// pair back-to-back — Slint's `TextInput` inserts text on `KeyPressed` and
    /// expects the matching release.  `text` is the Slint key payload produced
    /// by [`crate::ui::keyboard::key_text`] (a printable char, or a `Key::*`
    /// control code such as Backspace / Return).
    pub fn dispatch_key(&self, text: slint::SharedString) {
        self.window
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.window
            .dispatch_event(WindowEvent::KeyReleased { text });
    }
}

/// Adapter that feeds Slint's `render_by_line` output into the T-Deck display.
struct TDeckLineRenderer<'a, 'd> {
    display: &'a mut TDeckDisplay<'d>,
}

impl<'a, 'd> LineBufferProvider for TDeckLineRenderer<'a, 'd> {
    type TargetPixel = Rgb565Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: core::ops::Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        // Stack-allocated line buffer (one DISPLAY_WIDTH-wide RGB565 strip).
        let mut line_buf = [Rgb565Pixel(0); DISPLAY_WIDTH as usize];
        render_fn(&mut line_buf[range.clone()]);

        // BUG FIX: with RepaintBufferType::ReusedBuffer, Slint can deliver
        // partial x-ranges (range.end < DISPLAY_WIDTH) on incremental redraws.
        // The old code converted the FULL 320-pixel buffer and sent it all to
        // the display — pixels outside `range` were zero (black), overwriting
        // previously rendered content outside the dirty region.
        //
        // Fix: convert only the `range` pixels and call flush_line_range so the
        // SPI write window covers exactly the dirty strip.
        let range_len = range.len();
        if range_len == 0 {
            return;
        }

        // Convert only the dirty range from Slint Rgb565Pixel → embedded-graphics
        // Rgb565, LAZILY — no intermediate buffer.
        //
        // PERF: this used
        // to `.collect()` the conversion into a heap `Vec<Rgb565>` (a
        // "STACK-SAVING FIX" from an earlier fix that traded a 640 B stack
        // array for a per-call heap allocation instead). That heap round-trip
        // fired on EVERY dirty scanline — up to 240 times per full-window
        // repaint (confirmed baseline hotspot, `docs/perf/ui-perf-baseline.md`
        // §5 item 1) — inside `ui::step()`'s render path, the same iteration
        // whose wall-clock length gates how promptly the NEXT dispatcher-loop
        // iteration's CAD attempt / RX poll runs.
        //
        // FIX: `TDeckDisplay::flush_line_range` now takes a plain iterator
        // (see its doc) instead of a slice — `mipidsi::fill_contiguous`
        // streams pixels directly into `set_pixels` and never needed a
        // materialized buffer in the first place. Passing `.map(..)` straight
        // through drops the heap Vec entirely (no replacement stack buffer
        // either — nothing needs to hold the converted pixels at all, they
        // stream straight from this lazy iterator into the SPI write), with
        // the identical per-pixel conversion math (same wire bytes, same
        // pixels) — a strict subset of the old work, not a behavior change.
        // (`line_buf` above is a separate, pre-existing stack buffer — the
        // Slint render TARGET this closure fills — untouched by this fix.)
        let eg_pixels = range.clone().map(|i| {
            let px = line_buf[i];
            let r = ((px.0 >> 11) & 0x1F) as u8;
            let g = ((px.0 >> 5)  & 0x3F) as u8;
            let b = (px.0 & 0x1F) as u8;
            embedded_graphics::pixelcolor::Rgb565::new(r, g, b)
        });

        if let Err(e) = self.display.flush_line_range(
            line as u16,
            range.start as u16,
            eg_pixels,
        ) {
            log::warn!("display flush_line_range {} [{}..{}]: {:?}", line, range.start, range.start + range_len, e);
        }
    }
}

/// Install the Slint platform and return the cooperative rendering handle.
///
/// Thin free-function wrapper over [`TDeckPlatform::install`] for call sites
/// that prefer not to name the platform type.
pub fn install() -> Rc<TDeckWindowAdapter> {
    TDeckPlatform::install()
}
