// SPDX-License-Identifier: GPL-3.0-only
//! Host-native regression guard for `meshcadet-boot-splash-renders-no-
//! component-set`: the boot splash logged "ui: render_if_needed called
//! with no Slint component set on the window — skipping this frame" on
//! EVERY tick of its dedicated ripple render loop, dropping the whole
//! animation, despite the splash's own screen object staying fully alive
//! throughout.
//!
//! # Root cause this reproduces
//!
//! `firmware/src/ui/backdrop_asset.rs::shared_backdrop_image()` lazily
//! constructs `BackdropAsset` — itself a `Window`-inheriting Slint
//! component — on its first-ever call, then caches only the `Image`
//! property it extracts and drops the `BackdropAsset` value. Slint's
//! generated `X::new()`, for ANY `Window`-inheriting component, calls
//! `WindowInner::set_component()` UNCONDITIONALLY as part of construction
//! itself — NOT only when `.show()` is later called — so constructing (and
//! immediately dropping) `BackdropAsset` repoints the shared window's
//! component reference away from whatever screen was mid-construction, and
//! orphans it. `SplashScreen::new()` calls `shared_backdrop_image()`
//! BETWEEN building its own component and calling its own `.show()` — on
//! the first boot, that call's cache-miss path is exactly this steal, and
//! nothing re-attaches the splash afterward (a component's `.show()` only
//! calls `set_component()` on its OWN first-ever call — a later `.show()`
//! is a no-op with respect to `set_component()`).
//!
//! This module isolates that mechanism with minimal stand-ins — a
//! `Window`-inheriting "screen" and a `Window`-inheriting "asset carrier"
//! — rather than the full splash markup (already covered by
//! `splash_promo.rs`'s test), and proves:
//!
//! 1. Building the screen, THEN triggering the asset-carrier steal (the
//!    ORIGINAL, buggy ordering) leaves the shared window's component
//!    reference unresolvable — `render_by_line` hits the same guard
//!    `platform.rs::TDeckWindowAdapter::render_if_needed` does, painting
//!    nothing, exactly matching the reported symptom.
//! 2. Triggering the asset-carrier steal FIRST, THEN building the screen
//!    (`UiRuntime::new()`'s fix — warm the cache before any real screen is
//!    constructed) leaves the component correctly attached and rendering
//!    proceeds normally.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{
    LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel,
};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 64;
pub const HEIGHT: u32 = 48;

slint::slint! {
    // Stand-in for a real screen (`SplashScreenUi` etc.) — a plain
    // `Window`-inheriting component that gets `.show()`n and is meant to
    // stay the active on-screen content.
    export component ProbeScreenUi inherits Window {
        width: 64px;
        height: 48px;
        Rectangle { background: #ff00ff; }
    }

    // Stand-in for `BackdropAsset` (`backdrop_asset.rs`) — a
    // `Window`-inheriting component constructed only to read one property
    // off it, never `.show()`n, and dropped immediately after.
    export component ProbeAssetCarrierUi inherits Window {
        width: 64px;
        height: 48px;
        out property <int> dummy_value: 42;
    }
}

struct ProbePlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for ProbePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

struct DiscardLine;
impl LineBufferProvider for DiscardLine {
    type TargetPixel = Rgb565Pixel;
    fn process_line(
        &mut self,
        _line: usize,
        range: core::ops::Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        let mut buf = vec![Rgb565Pixel(0); range.len()];
        render_fn(&mut buf);
    }
}

/// Render rig for the backdrop-asset-steal regression guard.
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `splash_promo.rs::SplashPromoFrame::new`'s identical note. Callers must
/// ensure exactly one [`install`] runs per process — so this module exposes
/// TWO free functions (one per ordering under test) rather than a
/// constructible struct, and each must run in its own process (its own
/// `#[test]`, its own file under `tests/`).
fn install() -> Rc<MinimalSoftwareWindow> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    slint::platform::set_platform(Box::new(ProbePlatform {
        window: window.clone(),
        start: Instant::now(),
    }))
    .expect("Slint platform already set in this process");
    window
}

/// `true` if the shared window currently has a live component attached —
/// the exact check `platform::TDeckWindowAdapter::render_if_needed` (via
/// its own `try_component()` guard) makes before rendering.
fn component_attached(window: &MinimalSoftwareWindow) -> bool {
    i_slint_core::window::WindowInner::from_pub(window.window())
        .try_component()
        .is_some()
}

/// One `update_timers_and_animations` + `render_by_line` tick — mirrors
/// `TDeckWindowAdapter::render_if_needed`'s body exactly, guard included
/// (`render_by_line` panics on a genuinely-unset component with no
/// fallback — see that method's own doc), so a stale component here
/// degrades to "nothing painted" rather than panicking the test process.
///
/// Returns whether anything was actually rendered.
fn render_tick(window: &MinimalSoftwareWindow) -> bool {
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    if !component_attached(window) {
        return false;
    }
    window.draw_if_needed(|renderer| {
        renderer.render_by_line(DiscardLine);
    })
}

/// The ORIGINAL, buggy ordering: build the screen, set its `backdrop`-style
/// property from the asset-carrier cache-miss path AFTER — i.e. between
/// the screen's own `set_component()` (which happens inside `X::new()`,
/// per this module's doc) and its `.show()` call. Returns
/// `(attached_before_steal, attached_after_steal, painted_after_steal)`.
pub fn buggy_ordering() -> (bool, bool, bool) {
    let window = install();

    let screen = ProbeScreenUi::new().expect("ProbeScreenUi::new");
    let attached_before_steal = component_attached(&window);

    // Mirrors `shared_backdrop_image()`'s cache-miss path: construct,
    // extract a property, drop.
    let carrier = ProbeAssetCarrierUi::new().expect("ProbeAssetCarrierUi::new");
    let _ = carrier.get_dummy_value();
    drop(carrier);

    screen.show().expect("ProbeScreenUi::show");
    let attached_after_steal = component_attached(&window);
    let painted_after_steal = render_tick(&window);

    (attached_before_steal, attached_after_steal, painted_after_steal)
}

/// The FIX's ordering: pre-warm the asset-carrier cache-miss path BEFORE
/// building any real screen — mirrors `UiRuntime::new()`'s fix (call
/// `backdrop_asset::shared_backdrop_image()` once, up front, before
/// `SplashScreen::new()`). Returns whether the component is attached and
/// rendering succeeds after the screen's own construction + `.show()`.
pub fn fixed_ordering() -> (bool, bool) {
    let window = install();

    let carrier = ProbeAssetCarrierUi::new().expect("ProbeAssetCarrierUi::new");
    let _ = carrier.get_dummy_value();
    drop(carrier);

    let screen = ProbeScreenUi::new().expect("ProbeScreenUi::new");
    screen.show().expect("ProbeScreenUi::show");

    let attached = component_attached(&window);
    let painted = render_tick(&window);
    (attached, painted)
}
