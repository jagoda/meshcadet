// SPDX-License-Identifier: GPL-3.0-only
//! Host redraw-scope (dirty-region) rig — the "AUTOMATABLE: redraw-scope
//! analysis (dirty-region size per screen/animation)" leg of the
//! UI perf-pass baseline's measurement contract.
//!
//! # Why this measurement is representative, not a guess
//!
//! `firmware/src/ui/platform.rs::TDeckWindowAdapter::render_if_needed` calls
//! `renderer.render_by_line(TDeckLineRenderer { display })` against a
//! `MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer)` — Slint's
//! software renderer decides, per line, whether ANY pixel in that line's
//! dirty x-range changed since the previous frame, and only invokes
//! `process_line` for lines with a non-empty range (see
//! `TDeckLineRenderer::process_line`'s own `if range_len == 0 { return; }`
//! guard). This module drives the IDENTICAL renderer API
//! (`MinimalSoftwareWindow` + `ReusedBuffer` + `render_by_line`) against the
//! REAL `firmware/src/ui/motifs.slint` components (imported by relative
//! path, not forked — same technique `motif_library.rs` already
//! establishes), with a `LineBufferProvider` that COUNTS touched
//! lines/pixels instead of writing them to an SPI display. Because the
//! dirty-region decision is made entirely inside Slint's renderer (this
//! module doesn't reimplement or approximate it), the numbers this produces
//! are the actual repaint scope the real firmware would push over SPI for
//! the same component/animation state — not a host-side approximation of it.
//! This is what upgrades `docs/perf/ui-perf-baseline.md` §5.3's STATIC audit
//! ("does not match a naive full-window backdrop failure mode", magnitude
//! flagged unconfirmed) to a host-MEASURED confirmation — see
//! `tests/perf_profile.rs`.
//!
//! # What this does NOT measure
//!
//! SPI transfer time, per-transaction bus-hold overhead, and radio
//! contention are hardware-specific and out of reach for a host process —
//! those stay on the on-hardware capture protocol (see
//! `docs/perf/ui-perf-baseline.md` §8). This module measures the ONE thing
//! that IS host-reproducible: how many lines / pixels a given animation
//! frame actually dirties, which is the direct input to SPI-hold time
//! (more dirty lines → more `flush_line_range` calls → longer bus hold).

use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{
    LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel,
};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import {
        Starfield, RingedPlanetCorner, Comet, MascotBob, Twinkle,
        RocketOnSend, CometOnNotify,
    } from "../../firmware/src/ui/motifs.slint";

    export component PerfProfileUi inherits Window {
        width: 320px;
        height: 240px;
        // Full-window static backdrop — same `Theme.bg-space`/`Theme.space-
        // deep` full-window `Rectangle`/`Window.background` every themed
        // screen sets ONCE at construction (see `contact_list.rs`,
        // `message_view.rs`, etc.). Never reassigned after construction, so
        // it dirties the window exactly once (the first paint) and never
        // again — the scene below re-measures that claim directly.
        background: Theme.space-deep;

        in-out property <bool> send_trigger: false;
        in-out property <bool> notify_trigger: false;

        // Header strip — static (matches every fan-out screen's own
        // `Starfield { x: 0; y: 0; }` header usage).
        Starfield { x: 0px; y: 0px; }
        RingedPlanetCorner { x: 320px - 40px - 8px; y: 8px; }
        Comet { x: 140px; y: 70px; }

        // One-shot motion helpers under test, positioned clear of each
        // other (mirrors real per-screen placement, e.g.
        // `message_view.rs`'s header Comet + `compose.rs`'s corner
        // RocketOnSend + `contact_list.rs`'s CometOnNotify).
        MascotBob {
            x: 40px;
            y: 160px;
            source: @image-url("../../firmware/assets/space/cadet_idle.png");
        }
        Twinkle { x: 300px; y: 20px; twinkle-color: Theme.star-gold; }
        RocketOnSend { x: 220px; y: 180px; play: root.send_trigger; }
        CometOnNotify { x: 0px; y: 226px; play: root.notify_trigger; }
    }
}

struct PerfProfilePlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for PerfProfilePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// One frame's redraw-scope measurement: how many lines Slint's renderer
/// deemed dirty, the total dirty pixel count across them, and the widest
/// single-line dirty range (all derived from `LineBufferProvider::
/// process_line` calls with a non-empty range — see module doc).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirtyStats {
    pub lines_touched: usize,
    pub total_dirty_pixels: usize,
    pub widest_range: usize,
}

struct CountingLineRenderer<'a> {
    stats: &'a mut DirtyStats,
}

impl<'a> LineBufferProvider for CountingLineRenderer<'a> {
    type TargetPixel = Rgb565Pixel;

    fn process_line(
        &mut self,
        _line: usize,
        range: Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        // Slint still requires a real buffer to render into even though we
        // only care about the range size — mirrors platform.rs's own
        // early-return-on-empty guard so a zero-width call (a line Slint
        // visited but found nothing dirty on) is not counted as "touched".
        let range_len = range.len();
        let mut scratch = vec![Rgb565Pixel(0); range_len];
        render_fn(&mut scratch);
        if range_len == 0 {
            return;
        }
        self.stats.lines_touched += 1;
        self.stats.total_dirty_pixels += range_len;
        self.stats.widest_range = self.stats.widest_range.max(range_len);
    }
}

/// Redraw-scope rig: a real `PerfProfileUi` scene over a `ReusedBuffer`
/// window (production repaint-buffer mode), rendered via `render_by_line` +
/// [`CountingLineRenderer`] so each call reports the REAL per-frame
/// dirty-region size instead of writing pixels anywhere.
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — same
/// process-wide-singleton constraint every other `ui_sim` render rig
/// documents. Run this from its own `tests/*.rs` integration-test binary.
pub struct PerfProfileScene {
    window: Rc<MinimalSoftwareWindow>,
    ui: PerfProfileUi,
}

impl PerfProfileScene {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(PerfProfilePlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = PerfProfileUi::new().expect("PerfProfileUi::new");
        ui.show().expect("PerfProfileUi::show");

        PerfProfileScene { window, ui }
    }

    pub fn set_send_trigger(&self, v: bool) {
        self.ui.set_send_trigger(v);
    }

    pub fn set_notify_trigger(&self, v: bool) {
        self.ui.set_notify_trigger(v);
    }

    /// Tick Slint's animation/timer clock and render one frame, reporting
    /// the dirty-region size the REAL renderer computed (zeroed if nothing
    /// was dirty — `draw_if_needed` simply doesn't invoke the callback).
    pub fn tick(&self) -> DirtyStats {
        slint::platform::update_timers_and_animations();
        let mut stats = DirtyStats::default();
        self.window.draw_if_needed(|renderer| {
            renderer.render_by_line(CountingLineRenderer { stats: &mut stats });
        });
        stats
    }
}

impl Default for PerfProfileScene {
    fn default() -> Self {
        Self::new()
    }
}
