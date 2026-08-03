// SPDX-License-Identifier: GPL-3.0-only
//! Regression guard, half 2 of 2: proves the FIX (`UiRuntime::new()` now
//! calls `backdrop_asset::shared_backdrop_image()` once, up front, before
//! any real screen is constructed) leaves the shared window's component
//! correctly attached and rendering functional. See
//! `ui_sim::splash_backdrop_steal_probe`'s module doc for the full
//! mechanism, and `splash_backdrop_steal_probe_buggy_ordering.rs` for the
//! matching negative case this is the positive counterpart of.
//!
//! Lives in its own file/process — see that module's `install()` doc for
//! why (Slint's process-wide `Platform` singleton).

#[test]
fn prewarming_the_asset_cache_before_the_screen_keeps_component_attached() {
    let (attached, painted) = ui_sim::splash_backdrop_steal_probe::fixed_ordering();

    assert!(
        attached,
        "pre-warming the asset-carrier cache before constructing the real screen should \
         leave the shared window's component correctly attached — this is the fix \
         `UiRuntime::new()` applies"
    );
    assert!(
        painted,
        "render_by_line should paint normally once the component is correctly attached — \
         the splash-window-frames-no-longer-hit-the-no-component-set-path acceptance check"
    );
}
