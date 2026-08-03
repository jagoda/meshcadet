// SPDX-License-Identifier: GPL-3.0-only
//! Sole compile-time embed point for the shared full-window starfield
//! backdrop texture (`ui/motifs.slint`'s `SpaceBackdrop`).
//!
//! # The defect this file fixes
//!
//! `SLINT_EMBED_RESOURCES=embed-for-software-renderer` (`.cargo/config.toml`)
//! decodes every `@image-url(...)` literal it finds into raw pixel data and
//! bakes it into the generated component's init code — but it does this
//! **once per compiled `slint::slint!{}` invocation**, not once per distinct
//! asset path. `i-slint-compiler`'s embed pass
//! (`passes::embed_images::embed_images`) keys its resource-dedup table
//! (`path_to_id: HashMap<SmolStr, EmbeddedResourcesIdx>`) per `Document`, and
//! every `slint::slint!{}` macro call is its own separate `Document`.
//!
//! `SpaceBackdrop` used to carry a default
//! `source: @image-url("../../assets/space/starfield_full.png")` binding in
//! `motifs.slint`. Seven screens (`splash`, `contact_list`, `message_view`,
//! `compose`, `pin_entry`, `admin_menu`, `gps_status`) each `import` it from
//! their own, separately-compiled `slint::slint!{}` block — so the compiler
//! re-embedded a full, byte-identical copy of the 300,480-byte decoded
//! texture in EVERY one of the seven, 7 * 300,480 B = 2,103,360 B, when
//! exactly one copy was needed. Confirmed on the xtensa release ELF: seven
//! `SLINT_EMBEDDED_RESOURCE_*_DATA` symbols (one per screen module), each
//! 0x495c0 (300,480) bytes, sha256 `a1a06304f649f331...` — byte-identical.
//! 1,802,880 B (1.72 MiB) of that was pure waste: 28.6% of the 6 MB factory
//! partition.
//!
//! # The fix
//!
//! Isolate the literal `@image-url(...)` binding to exactly ONE
//! `slint::slint!{}` invocation — this one — so the compiler's per-Document
//! dedup only ever sees it once. `SpaceBackdrop` no longer carries a default
//! `source`; each of the 7 screens' `Window` root now exposes a plain
//! `in property <image> backdrop_image` and binds
//! `SpaceBackdrop { source: backdrop_image; }`. Every screen's Rust
//! constructor sets that property from [`shared_backdrop_image`] below,
//! right after building the component and before `.show()` — the same
//! "set initial state before show" convention every other per-screen
//! property already follows.
//!
//! `slint::Image` is a cheap, `Rc`-style handle around the decoded texture,
//! so cloning it into all 7 screens shares the SAME underlying pixel data —
//! it does not copy or re-decode anything. [`shared_backdrop_image`] caches
//! the one `BackdropAsset` construction in a `thread_local`, so even the
//! (already cheap) component construction only ever happens once per
//! process, not once per screen navigation.
//!
//! `BackdropAsset` `inherits Window` (rather than being a bare
//! non-visual component) solely to avoid `slint::slint!{}`'s "doesn't
//! inherit Window" deprecation warning — it is never `.show()`n, so its
//! CONTENT never becomes what actually paints to the single shared
//! `MinimalSoftwareWindow` every real screen's `Window` component draws
//! into (see `platform.rs`'s `TDeckPlatform::create_window_adapter`, which
//! hands out clones of that one adapter to every constructed `Window`).
//!
//! GOTCHA (see `meshcadet-boot-splash-renders-no-component-set` — the boot
//! splash rendered nothing, every frame silently dropped, because of
//! exactly this): "never `.show()`n" does NOT mean `BackdropAsset::new()`
//! is free of side effects on the shared window. Slint's generated
//! `X::new()`, for ANY `Window`-inheriting component, calls
//! `WindowInner::set_component()` UNCONDITIONALLY as part of construction
//! itself (`i-slint-compiler`'s generated `window_adapter_ref()` — "ensure
//! that the window exist as this point so further call to window() don't
//! panic") — `.show()` is a separate, later step that is NOT what
//! associates a component with the shared window. So every
//! `BackdropAsset::new()` call — even though the value is immediately
//! dropped after this function extracts its `image` property, and even
//! though it is never shown — REPOINTS the shared window's component
//! reference to itself, then immediately orphans that reference when it
//! drops (nothing else keeps `BackdropAsset` alive). If that happens
//! WHILE a real screen is being constructed — i.e. this function's
//! cache-miss (first-ever call) path runs from inside that screen's own
//! constructor, between the screen setting its OWN component and calling
//! its OWN `.show()` — it silently steals the shared window's component
//! away from that screen, and nothing re-attaches it afterward (a
//! component's `.show()` only calls `set_component()` on ITS OWN
//! first-ever call — see the `window_adapter_ref()` `OnceCell` above — a
//! later `.show()` call is a no-op with respect to `set_component()`).
//! `UiRuntime::new()` closes this by calling this function once, up
//! front, before any real screen is constructed — see that call site's
//! own doc for the full mechanism and fix.

slint::slint! {
    // Non-visual carrier: its only job is to hold the ONE compile-time
    // binding of `starfield_full.png` (see module doc). Never `.show()`n.
    export component BackdropAsset inherits Window {
        out property <image> image: @image-url("../../assets/space/starfield_full.png");
    }
}

/// Returns the shared full-window starfield backdrop image, decoded into
/// the binary exactly once (see module doc). Cheap to call from every
/// screen's constructor: after the first call this clones a cached
/// `Rc`-style `Image` handle, not the underlying ~300 KB of pixel data.
pub fn shared_backdrop_image() -> slint::Image {
    thread_local! {
        static CACHED: std::cell::RefCell<Option<slint::Image>> = std::cell::RefCell::new(None);
    }
    CACHED.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.is_none() {
            let asset = BackdropAsset::new()
                .expect("BackdropAsset has no window/platform dependency to fail — \
                         constructed only after TDeckPlatform::install()");
            *cell = Some(asset.get_image());
        }
        cell.clone().expect("just populated above")
    })
}
