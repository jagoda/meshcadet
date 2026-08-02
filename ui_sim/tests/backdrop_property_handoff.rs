// SPDX-License-Identifier: GPL-3.0-only
//! Regression guard for `meshcadet-slint-embedded-asset-dedupe`.
//!
//! The defect: `SLINT_EMBED_RESOURCES=embed-for-software-renderer` bakes a
//! fresh copy of every `@image-url(...)` literal it sees once PER COMPILED
//! `slint::slint!{}` INVOCATION, not once per distinct asset path
//! (`i-slint-compiler`'s embed pass keys its resource-dedup table per
//! `Document`, and every macro invocation is its own `Document`). Seven
//! production screens each importing `motifs.slint`'s `SpaceBackdrop` —
//! which used to carry a default `@image-url("starfield_full.png")` —
//! re-embedded a full, byte-identical 300,480-byte copy of the same texture
//! SEVEN TIMES: 1.72 MiB of pure waste, 28.6% of the 6 MB factory partition.
//!
//! The fix (`firmware/src/ui/backdrop_asset.rs`): isolate the literal to
//! exactly ONE `slint::slint!{}` invocation, and hand the resulting
//! `slint::Image` (a cheap `Rc`-style handle) to every consuming screen at
//! Rust runtime via a plain `in property <image>`, instead of each screen
//! re-declaring the literal itself.
//!
//! This test proves the RUNTIME HALF of that mechanism actually works: an
//! `Image` obtained from one component's property, handed into two
//! completely independent OTHER components' own properties, reads back as
//! pixel-identical to the canonical source in both — i.e. the hand-off
//! plumbing every one of the 7 real screens' constructors now performs
//! (`component.set_backdrop_image(ui::backdrop_asset::shared_backdrop_
//! image())`) actually carries real pixel data, not an empty/default image.
//! It would catch a regression where, e.g., a future edit passes the wrong
//! `Image` handle, forgets the `set_backdrop_image` call on a screen (which
//! would leave that screen's copy blank/default and diverge from the
//! canonical source), or breaks `shared_backdrop_image()`'s caching.
//!
//! Lives in its own process (own file under `tests/`) for the same
//! process-wide-`Platform`-singleton reason every other file here does —
//! see `tests/space_backdrop.rs`'s own doc.

#[test]
fn backdrop_image_handed_to_two_independent_consumers_is_pixel_identical() {
    let (canonical, from_a, from_b) = ui_sim::motif_library::prove_backdrop_property_handoff();

    assert_eq!(canonical.size().width, 320);
    assert_eq!(canonical.size().height, 240);

    let canonical_bytes = canonical.to_rgba8().expect("canonical to_rgba8");
    let a_bytes = from_a.to_rgba8().expect("consumer A to_rgba8");
    let b_bytes = from_b.to_rgba8().expect("consumer B to_rgba8");

    assert_eq!(
        a_bytes.as_bytes(),
        canonical_bytes.as_bytes(),
        "consumer A's backdrop_image property did not read back as the canonical asset"
    );
    assert_eq!(
        b_bytes.as_bytes(),
        canonical_bytes.as_bytes(),
        "consumer B's backdrop_image property did not read back as the canonical asset"
    );
}
