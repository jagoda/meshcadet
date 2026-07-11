// SPDX-License-Identifier: GPL-3.0-only
//! Host-native measurement harness for the UI performance pass.
//! Two independent proof rigs share this crate:
//!
//! 1. **REPAINT-SCOPE rig** (this module) — renders the REAL
//!    `firmware/src/ui/motifs.slint` animations — and a synthetic
//!    backdrop+list scene that mirrors how the message-view screen layers a
//!    static space backdrop under a live-updating list — through the
//!    IDENTICAL Slint `SoftwareRenderer` in `RepaintBufferType::ReusedBuffer`
//!    (dirty-region) mode `firmware/src/ui/platform.rs` drives on-target, and
//!    reports, per frame, the exact dirty region the renderer would flush
//!    over SPI. See "Why this is a faithful proxy" / "Determinism" below.
//! 2. **Phase-1 BASELINE rig** ([`counting_alloc`] + [`render_logic`]
//!    modules) — timed host
//!    benchmarks over the pure render-logic hot paths in
//!    `firmware_core::ui::message_view` (moved there from
//!    `firmware/src/ui/mod.rs` by the `firmware-core-extract-ui-runtime`
//!    increment — [`render_logic`] re-exports the real functions rather
//!    than porting them, see that module's doc), plus a reusable
//!    counting-`GlobalAlloc` wrapper every downstream optimization child
//!    measures against. See those modules' own docs, `src/bin/bench.rs`,
//!    and `docs/perf/ui-perf-baseline.md` for the full methodology + ledger.
//!
//! # Why this is a faithful proxy for the on-target flush cost (repaint-scope)
//!
//! The firmware's `TDeckWindowAdapter::render_if_needed` (ui/platform.rs) calls
//! `window.draw_if_needed(|r| r.render_by_line(..))`. `render_by_line` walks
//! the SAME per-frame dirty `PhysicalRegion` that `render(buffer, stride)`
//! returns here, and issues one `flush_line_range` SPI transaction per dirty
//! scanline covering exactly that line's dirty x-range. So:
//!   * **lines-flushed / frame** = number of distinct dirty scanlines in the
//!     region = number of `flush_line_range` SPI window-set+write cycles the
//!     firmware pays that frame.
//!   * **dirty pixels / frame** = total px the renderer recomposites + ships.
//!   * **bbox** = the repaint-scope bounding box.
//!
//! Reducing these is exactly "reduce repaint scope": fewer SPI holds/frame →
//! less bus contention with the SX1262 radio on the shared SPI2 bus, and less
//! recomposite work → higher frame rate.
//!
//! Frame-time (ms) and radio-op timing stay ON-HARDWARE (HIL capture):
//! they depend on the ESP32-S3 CPU + the real SPI2 bus, neither of which this
//! host rig has. This crate quantifies the *cause* (flush scope); the HIL /
//! synthesis gate confirms the *effect* (felt snappiness + radio no-regress).
//!
//! # Determinism
//!
//! Slint's `animate` blocks compute progress from `Platform::duration_since_
//! start()`. This rig's platform returns a MANUALLY advanced clock (see
//! [`Harness::advance`]) rather than wall-clock, so a given animation always
//! samples the identical set of frames run-to-run — the measured line counts
//! are reproducible, not timing-dependent.
//!
//! # Why the counting allocator is NOT a crate-wide `#[global_allocator]`
//!
//! `tests/flush_line_alloc.rs` and `tests/alloc_tick_dedup.rs` (each its own
//! integration-test binary) already declare their OWN local
//! `#[global_allocator]` static — and `#[global_allocator]` is a process-wide
//! lang item, so a SECOND declaration anywhere in the same linked binary
//! (e.g. in this library crate) is a hard duplicate-lang-item link error.
//! [`counting_alloc::CountingAllocator`] is therefore installed as the
//! `#[global_allocator]` only where it is actually needed: locally in
//! `src/bin/bench.rs`'s own binary crate, which links this library but not
//! the other test binaries (Cargo compiles each `[[bin]]` and each
//! `tests/*.rs` file as a separate crate, so a bin-local declaration there
//! cannot collide with a test-local one elsewhere).

pub mod counting_alloc;
pub mod render_logic;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use slint::platform::software_renderer::PhysicalRegion;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

// ── Reusable render components ──────────────────────────────────────────────
//
// Both import the REAL theme + motifs by relative path (not a fork), exactly
// like ui_sim's motif_library rig, so this measures the shipped animation
// primitives — not look-alikes.
slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import {
        Starfield, RingedPlanetCorner,
        MascotBob, Twinkle, RocketOnSend, CometOnNotify,
    } from "../../firmware/src/ui/motifs.slint";

    // A screen-shaped scene: a full-window static space backdrop + static
    // starfield header + corner planet (the "backdrop layers" that
    // must NOT re-flush when only a foreground motif animates), with the
    // two retriggerable one-shot motion helpers a real screen animates on
    // send / notify. The `*_trigger` props are Rust-set, exactly the contract
    // every themed screen uses (see splash.rs::start_animation).
    export component MotifScene inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        in-out property <bool> send_trigger: false;
        in-out property <bool> notify_trigger: false;

        // Static backdrop layers.
        Starfield { x: 0px; y: 0px; }
        RingedPlanetCorner { x: 280px; y: 200px; }

        // Foreground motion helpers a screen animates over that backdrop.
        RocketOnSend  { x: 150px; y: 120px; play: root.send_trigger; }
        CometOnNotify { x: 0px;   y: 12px;  play: root.notify_trigger; }
    }

    // A message-view-shaped scene: the same static backdrop, plus a live list
    // whose row model is Rust-set. Used to prove that a MODEL update to an
    // already-visible screen self-dirties to only the changed rows — so the
    // firmware's redundant full-window `request_redraw()` on those live-update
    // paths flushes the whole 320x240 (incl. the static backdrop) for nothing.
    export struct Row { text: string, ours: bool }

    // A screen-entry-fade scene — mirrors the REAL shape every themed screen uses (`contact_list.rs`,
    // `message_view.rs`, `pin_entry.rs`, `gps_status.rs`, `admin_menu.rs`,
    // `unprovisioned.rs`, `compose.rs`): a full-window static backdrop
    // (`Starfield`, matching `SpaceBackdrop`'s "declared first, outside the
    // fading subtree" placement those screens use), plus a header + a
    // handful of static content rows, all wrapped in ONE `VerticalLayout`
    // whose `opacity` animates 0 → 1 once on `init` — the exact
    // `reveal_opacity`/`content_opacity` idiom this pass targets.
    // Nothing here is animated PER-ROW; the only moving property
    // in the whole scene is the wrapping `VerticalLayout`'s own `opacity`.
    export component EntryFadeScene inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        in-out property <float> content_opacity: 0;
        animate content_opacity { duration: 200ms; easing: ease-out; }
        init => { content_opacity = 1; }

        // Static backdrop layer — OUTSIDE the fading VerticalLayout below,
        // exactly like every real screen's `SpaceBackdrop {}` placement.
        Starfield { x: 0px; y: 0px; }

        VerticalLayout {
            opacity: content_opacity;
            Rectangle { height: 36px; background: Theme.surface; }
            for i in 5: Rectangle { height: 40px; background: Theme.surface; }
        }
    }

    export component ListScene inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        in property <[Row]> rows;

        // Static backdrop layers (must survive a list update un-reflushed).
        Starfield { x: 0px; y: 0px; }

        VerticalLayout {
            x: 8px;
            y: 44px;
            width: 304px;
            spacing: 4px;
            for row in root.rows: Rectangle {
                height: 22px;
                background: row.ours ? Theme.nebula-violet-deep : Theme.surface;
                Text {
                    x: 6px;
                    text: row.text;
                    color: Theme.text-primary;
                    vertical-alignment: center;
                }
            }
        }
    }
}

// ── Platform with a manually advanced clock ─────────────────────────────────

struct HarnessPlatform {
    window: Rc<MinimalSoftwareWindow>,
    clock: Rc<Cell<Duration>>,
}

impl Platform for HarnessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> Duration {
        self.clock.get()
    }
}

// ── Per-frame dirty-region statistics ───────────────────────────────────────

/// The flush cost of one rendered frame, derived from the software renderer's
/// returned [`PhysicalRegion`] (the SAME region `render_by_line` would flush).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Distinct dirty scanlines = one `flush_line_range` SPI cycle each.
    pub lines_flushed: usize,
    /// Total dirty pixels recomposited + shipped this frame.
    pub dirty_pixels: usize,
    /// Repaint-scope bounding-box width (px).
    pub bbox_w: u32,
    /// Repaint-scope bounding-box height (px).
    pub bbox_h: u32,
}

impl FrameStats {
    fn from_region(region: &PhysicalRegion) -> Self {
        // `iter()` yields non-overlapping rectangles. Distinct scanlines =
        // union of their y-ranges; dirty pixels = sum of w*h.
        let mut lines = std::collections::BTreeSet::<i32>::new();
        let mut dirty_pixels = 0usize;
        for (pos, size) in region.iter() {
            for y in pos.y..pos.y + size.height as i32 {
                lines.insert(y);
            }
            dirty_pixels += size.width as usize * size.height as usize;
        }
        let bb = region.bounding_box_size();
        FrameStats {
            lines_flushed: lines.len(),
            dirty_pixels,
            bbox_w: bb.width,
            bbox_h: bb.height,
        }
    }

    /// A full-window flush (the worst case: every scanline, every pixel).
    pub fn full_window() -> Self {
        FrameStats {
            lines_flushed: HEIGHT as usize,
            dirty_pixels: (WIDTH * HEIGHT) as usize,
            bbox_w: WIDTH,
            bbox_h: HEIGHT,
        }
    }
}

/// One installed Slint platform + ReusedBuffer window + persistent framebuffer.
///
/// ReusedBuffer requires the SAME buffer be handed to every `render()` (it
/// holds the previous frame's pixels so the renderer can skip un-dirty
/// regions) — this struct owns that buffer, exactly as the firmware's single
/// line buffer persists across `render_if_needed` calls in intent.
pub struct Harness {
    window: Rc<MinimalSoftwareWindow>,
    clock: Rc<Cell<Duration>>,
    framebuffer: Vec<Rgb565Pixel>,
}

impl Harness {
    /// Install the platform (once per process — Slint enforces a singleton).
    ///
    /// # Panics
    /// Panics if a Slint platform is already installed in this process. Each
    /// `tests/*.rs` integration binary gets its own process, so one `Harness`
    /// per test file is collision-free.
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(HarnessPlatform {
            window: window.clone(),
            clock: clock.clone(),
        }))
        .expect("Slint platform already set in this process");
        Harness {
            window,
            clock,
            framebuffer: vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize],
        }
    }

    /// Advance the animation clock by `ms` (deterministic, replaces wall-clock).
    pub fn advance(&self, ms: u64) {
        self.clock.set(self.clock.get() + Duration::from_millis(ms));
    }

    /// Force the next frame to be a full-window repaint (mirrors the firmware's
    /// `request_redraw()` on navigation / on the live-update paths this rig
    /// scopes down).
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Tick animations only (no render) — the `update_timers_and_animations()`
    /// half of [`Harness::frame`], split out so `tests/entry_fade_repaint.rs` can mirror
    /// `UiRuntime::step()`'s render-cadence throttle exactly: that throttle
    /// ticks Slint every dispatcher iteration but only calls
    /// `render_if_needed` (this struct's [`Harness::render_now`]) on SOME of
    /// them.
    pub fn tick(&self) {
        slint::platform::update_timers_and_animations();
    }

    /// Render one frame into the persistent buffer (no tick), returning its
    /// flush cost. Returns `None` if nothing was dirty (the renderer painted
    /// nothing — a true no-op frame, zero SPI cost). See [`Harness::tick`].
    pub fn render_now(&mut self) -> Option<FrameStats> {
        let fb = &mut self.framebuffer;
        let mut stats = None;
        let rendered = self.window.draw_if_needed(|renderer| {
            let region = renderer.render(fb, WIDTH as usize);
            stats = Some(FrameStats::from_region(&region));
        });
        if rendered {
            stats
        } else {
            None
        }
    }

    /// Tick animations, then render one frame into the persistent buffer,
    /// returning its flush cost. Returns `None` if nothing was dirty (the
    /// renderer painted nothing — a true no-op frame, zero SPI cost).
    pub fn frame(&mut self) -> Option<FrameStats> {
        self.tick();
        self.render_now()
    }

    /// A stable content hash of the current framebuffer — for parity proofs
    /// (identical pixels ⇒ identical hash). FNV-1a over the RGB565 words.
    pub fn framebuffer_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for px in &self.framebuffer {
            for b in px.0.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        }
        h
    }

    /// Access the underlying window (to `show()` a component onto it).
    pub fn window(&self) -> &Rc<MinimalSoftwareWindow> {
        &self.window
    }

    /// `true` if the last [`Harness::frame`] call touched a still-in-flight
    /// `animate` — the exact
    /// signal `firmware/src/ui/mod.rs::UiRuntime::step()`'s render-cadence
    /// throttle keys off of (`TDeckWindowAdapter::has_active_animations`,
    /// same underlying `Window::has_active_animations()` call). Exposed here
    /// so `tests/entry_fade_repaint.rs` can exercise the IDENTICAL
    /// render-or-skip decision against the real renderer, not a re-derived
    /// approximation of it.
    pub fn has_active_animations(&self) -> bool {
        self.window.has_active_animations()
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}
