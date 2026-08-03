// SPDX-License-Identifier: GPL-3.0-only
//! Regression guard, half 1 of 2: proves the ORIGINAL, buggy construction
//! ordering (`shared_backdrop_image()`'s cache-miss path running BETWEEN a
//! screen's own construction and its `.show()` call) really does leave the
//! shared window's component unresolvable — the defect
//! `meshcadet-boot-splash-renders-no-component-set` reported. See
//! `ui_sim::splash_backdrop_steal_probe`'s module doc for the full
//! mechanism.
//!
//! Lives in its own file/process — see that module's `install()` doc for
//! why (Slint's process-wide `Platform` singleton).

#[test]
fn buggy_construction_ordering_leaves_component_unresolvable() {
    let (attached_before_steal, attached_after_steal, painted_after_steal) =
        ui_sim::splash_backdrop_steal_probe::buggy_ordering();

    assert!(
        attached_before_steal,
        "the screen's own construction should attach its component immediately"
    );
    assert!(
        !attached_after_steal,
        "the asset-carrier's construct-then-drop should have knocked the screen's \
         component reference stale — this is the reported defect's exact mechanism"
    );
    assert!(
        !painted_after_steal,
        "render_by_line should paint nothing while the component is stale — the render \
         path's own guard condition, not a device-timestamp read"
    );
}
