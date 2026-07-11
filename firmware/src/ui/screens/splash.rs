// SPDX-License-Identifier: GPL-3.0-only
//! Boot / splash screen.
//!
//! Shown first, on every boot path (provisioned and unprovisioned) — see
//! `UiRuntime::new` / `UiRuntime::dismiss_splash`.
//! Requirements: on-brand + a real (non-static)
//! animation; shows the firmware version; dismisses itself once the app is
//! ready, with no user interaction; the animation always plays to completion
//! even on a very fast boot (enforced by `UiRuntime::SPLASH_MIN_MS`, not by
//! this module); total on-screen time stays at roughly 1.6 s past the
//! animation's start in the common case (`SPLASH_MIN_MS`), comfortably inside
//! the ~2.5 s acceptance budget (`SPLASH_MAX_MS`'s defensive cap).
//!
//! # Static-complete, then ripple
//!
//! The splash's FIRST-EVER rendered frame already shows the complete,
//! fully-opaque radio logo + "MeshCadet" wordmark + firmware version — see
//! `logo_opacity` / `title_opacity` / `version_opacity` below, all declared at
//! `1` (not `0`): there is no assembling image and no white screen, because
//! there is nothing left to fade in. That static-complete frame HOLDS,
//! unanimated, for the entire bring-up (NVS/radio/GPS/admin-server), with no
//! rings visible either (`ripple_active` gates the two radar rings to fully
//! transparent — see below). Only once bring-up settles
//! (`UiRuntime::mark_app_ready()`, see "Deferred animation start" below) does
//! `start_animation()` fire, and even then the logo/wordmark/version are NOT
//! touched — only the two radar "ping" rings play, expanding outward from
//! around the already-visible logo and fading to nothing. That one ripple is
//! the screen's entire animation; the logo/wordmark/version are static from
//! frame one to the moment the splash dismisses.
//!
//! This replaces an earlier (wrong) design where `logo_opacity` /
//! `title_opacity` / `version_opacity` were declared at `0` and
//! `start_animation()` faded them in alongside the rings — which meant the
//! screen's true "static state" (i.e. every frame before
//! `mark_app_ready()`) was actually BLANK, not "logo + version text" as the
//! module doc used to (incorrectly) claim: bring-up rendered a white/blank
//! screen, then a choppy multi-element build-up once the animation started.
//! The fix here is the initial property values themselves, not the gating
//! mechanism (that part — deferring the animation start to
//! `mark_app_ready()` — was already correct; see below).
//!
//! # Theme token pilot
//!
//! This screen already established the "Mission Control" dark-space + cyan
//! brand look, so it is the pilot consumer of the shared `Theme` global
//! (`ui/theme.slint`, imported below): every color and font-size literal
//! that used to live here now reads from `Theme` instead, with the SAME hex
//! values / pixel sizes it already had — a pixel-identical render is the
//! whole point (it proves the token plumbing end-to-end without touching
//! the already-on-brand look). The one exception is the radar rings' color,
//! which used a Slint `rgba(0, 180, 255, opacity)` literal; that becomes
//! `Theme.brand-signal.with-alpha(opacity)`, which is the same RGB
//! (`#00b4ff` = `rgb(0, 180, 255)`) with the alpha channel set the same way.
//!
//! # Animation design — a looping ripple, until dismiss
//!
//! Slint's `animate` block fires a transition whenever the animated
//! property's VALUE CHANGES — it does not auto-play a declared default. The
//! two radar rings (`ring1_size_px`/`ring1_opacity`,
//! `ring2_size_px`/`ring2_opacity`) are declared at their small/near-opaque
//! "about to ping" values, but are held INVISIBLE the whole time
//! `ripple_active` is `0` (each ring `Rectangle`'s own `opacity` is bound to
//! `ripple_active`, an in-out `float` with NO `animate` block of its own —
//! flipping it is an instant, un-eased reveal, not a fade). `start_animation()`
//! flips `ripple_active` to `1` and, in the SAME batch of writes, moves
//! `ring{1,2}_size_px`/`ring{1,2}_opacity` to their expanded/faded end values
//! — the value change that fires the rings' `animate` transitions. Net
//! effect: the instant the ripple starts, each ring is instantly revealed at
//! its small, near-opaque starting look, and immediately begins smoothly
//! expanding + fading per the `animate` blocks below — the classic
//! "sonar ping" shape. `logo_opacity`/`title_opacity`/`version_opacity`,
//! by contrast, are never written by `start_animation()` at all (see above):
//! they hold their initial `1` from frame one straight through to dismissal,
//! so they have nothing to animate and no `animate` block is declared for
//! them.
//!
//! Each ring's `animate` block below now declares `iteration-count: -1`
//! (`PropertyAnimation::iteration-count` otherwise defaults to `1.0`, NOT
//! infinite — see `i-slint-compiler`'s `builtins.slint`): once
//! `start_animation()` fires the single value-change that starts the
//! transition, Slint's animation runtime (`i-slint-core`'s
//! `PropertyValueAnimationData::compute_interpolated_value`) keeps
//! re-wrapping that SAME `from → to` transition every `duration` (850 ms per
//! ring) for as long as anything keeps calling
//! `update_timers_and_animations()` against a live `SplashScreenUi` instance
//! — i.e. for as long as the splash stays the active screen. No further
//! Rust-side re-trigger is needed: this one property write, once, is enough
//! for the ripple to keep pinging indefinitely.
//!
//! This SUPERSEDES the previous one-shot design's reasoning (each ring used
//! to play exactly once, then hold its faded-out end state, specifically so
//! an in-progress pulse could never be visibly "cut off mid-cycle" the
//! instant `dismiss_splash()` swaps the screen out from under it). That
//! concern does not actually apply here: `dismiss_splash()` doesn't snip one
//! property out of a still-visible screen — it replaces `active_screen`
//! wholesale (see `UiRuntime::dismiss_splash`), dropping this entire
//! `SplashScreenUi` component (rings, logo, wordmark, all of it) in the same
//! instant the next screen's first frame appears. An infinite ripple ending
//! mid-expansion is therefore indistinguishable, on screen, from any OTHER
//! screen swap that happens to land between two frames of an ongoing
//! animation (e.g. `MascotBob`'s idle bob, elsewhere in this UI) — it is not
//! a new class of visual defect, just the ordinary cost of any screen
//! transition. The "animation always completes" acceptance requirement (see
//! module doc's top paragraph) is unaffected: it refers to the ripple being
//! visible long enough to register as an actual ping rather than a flash
//! (`UiRuntime::SPLASH_MIN_MS`, unchanged, still floors the on-screen hold at
//! comfortably more than one full ring cycle), not to a single iteration
//! running start-to-end uninterrupted.
//!
//! # Deferred animation start
//!
//! `start_animation()` is a SEPARATE method from `new()`. Historically it was
//! called by `UiRuntime::step()` itself, gated on `UiRuntime::mark_app_ready()`
//! having been called (see that method's doc and `step()`'s dismissal-gate
//! block) — see points 1/2 below for why. It is instead called by
//! `UiRuntime::run_splash_ripple()`, invoked directly by `main.rs` right after
//! `mark_app_ready()` and BEFORE `step()` ever runs — see the
//! "Dedicated render loop for the ripple" section further down for why the
//! "when" needed one more correction beyond
//! the two below. Until `run_splash_ripple()` fires it, every `step()` call
//! still renders this screen at its declared static start state — logo +
//! wordmark + version, all fully opaque, no rings — which is deliberate, not
//! a placeholder: a static frame has nothing to fall behind schedule, so it
//! cannot look choppy no matter how irregularly `step()` itself gets to run
//! during bring-up.
//!
//! This is a two-part fix (the "when" of the
//! animation start; see the section above for the "what",
//! i.e. static-complete vs. fade-in):
//!
//! 1. Slint's `animate` blocks
//!    measure progress from real wall-clock time elapsed since the property
//!    write, regardless of whether the render loop has ticked in between.
//!    Firing the properties at CONSTRUCTION time (`UiRuntime::new()`) meant
//!    all of `main.rs`'s bring-up between construction and the first `step()`
//!    call ran invisibly against the animation's budget: by the time the
//!    render loop first evaluated the properties, some or all could already
//!    be at their settled end value, so the choreography was never actually
//!    seen — the panel's first-ever frame was already the animation's LAST
//!    frame. Deferring the write to `step()` fixed that.
//! 2. Firing on `step()`'s
//!    first-ever call ONLY fixed the "never seen" defect above; it did
//!    nothing about the SAME bring-up (GPS baud probe, radio SPI config,
//!    flash hydrate) instead starving `step()` itself once it started
//!    running — so the animation's own frames, once they did start, still
//!    landed sparsely/irregularly (choppy, not smooth). Gating the start on
//!    `mark_app_ready()` instead means the animation's real start coincides
//!    with the main loop settling into its steady per-iteration cadence
//!    (radio/GPS/history/admin-server bring-up all done), on every boot
//!    path, regardless of how long either bring-up phase took.
//!
//! Timeline (all offsets from the moment `start_animation()` fires the
//! transitions — that's the `run_splash_ripple()` call right after `mark_app_ready()`, not
//! a `step()` call); logo/wordmark/version are already at full opacity before
//! this clock starts and are not touched by it:
//!   - Ring 1 (radar "ping" around the logo) instantly revealed, then
//!     expands + fades: done at  850 ms.
//!   - Ring 2 (staggered 300 ms behind ring 1): done at 1150 ms.
//! `UiRuntime::SPLASH_MIN_MS` (1600 ms) is kept comfortably above this 1150 ms
//! total so the minimum on-screen hold never dismisses mid-animation (and so
//! the splash lingers a bit past the animation's own end).
//! `SPLASH_MIN_MS` is measured from
//! `splash_animation_started_ms` — the animation's OWN start clock, seeded the
//! instant `start_animation()` fires (not from `splash_started_ms`, the
//! splash's first-tick clock, which the SEPARATE `SPLASH_MAX_MS` defensive cap
//! still uses) — so this timeline lines up with it exactly regardless of how
//! long boot took to reach `mark_app_ready()`. If this timeline changes, that
//! constant must move with it.
//!
//! # Dedicated render loop for the ripple
//!
//! The two prior fixes above addressed WHEN the ripple starts (after
//! `mark_app_ready()`, not before). They did not fix HOW it is driven, and
//! that turned out to still be broken: the ripple played INCONSISTENTLY on
//! real hardware — sometimes a smooth expanding radar ping, but often just a
//! quick flash of the rings with almost no visible motion, varying boot to
//! boot on IDENTICAL firmware.
//!
//! ROOT CAUSE: `UiRuntime::step()` used to fire
//! `SplashScreen::start_animation()` and then rely on the ORDINARY dispatcher
//! loop's own subsequent `step()` calls to render the ripple's
//! `~1150 ms` of `animate` transitions. But that same dispatcher loop
//! iteration also polls radio RX and GPS every single pass (`main.rs`'s
//! `loop { ... }`), and those polls do not run in bounded, evenly-spaced time
//! — a CAD/TX cycle, an SPI transfer, a GPS NMEA sentence read can each eat an
//! irregular slice of the loop iteration. Slint's `animate` blocks compute
//! their progress from REAL WALL-CLOCK TIME elapsed since the property write
//! — not from how many render frames have actually been painted. A `step()`
//! cadence that is fast and even for one boot but lumpy for the next means
//! the SAME ~1150 ms wall-clock window gets rendered as ~70 evenly-spaced
//! frames on a lucky boot (smooth ripple) or as a handful of frames on an
//! unlucky one (radio packet / GPS burst landing mid-window) — the animation
//! doesn't slow down when starved of frames, it just skips visibly painting
//! most of itself, which reads as a flash.
//!
//! FIX: `UiRuntime::run_splash_ripple()` (called once by `main.rs`,
//! immediately after `mark_app_ready()`, BEFORE the dispatcher loop / USB
//! provisioning-wait loop is ever entered) now owns firing the animation
//! AND rendering its full timeline, on its OWN dedicated loop: fire
//! `start_animation()`, then spin `update_timers_and_animations()` +
//! `render_if_needed()` + a ~16 ms sleep, in a tight loop, for the ripple's
//! total `~1150 ms` duration — nothing else runs on the main thread during
//! that window (no RX poll, no GPS poll, no touch/keyboard poll). Every tick
//! renders a frame; the frame rate no longer depends on what the radio or GPS
//! happen to be doing at that exact moment. `step()` itself no longer starts
//! the animation at all — see that method's doc.
//!
//! SAFETY NOTE — is it safe to defer radio RX polling for ~1.15 s, once, at
//! boot? Yes: `radio::Radio::try_receive`'s own doc states the SX1262 "stays
//! in continuous RX throughout" the poll — reception is driven by the radio
//! hardware's own clock (it latches `IRQ_RX_DONE` autonomously), not by how
//! often `try_receive` is called to check DIO1. A polling gap is therefore
//! NOT a reception gap; the only real risk is a SECOND distinct packet
//! landing before the FIRST is drained over SPI, which could overwrite the
//! single hardware RX buffer before it's read. At this network's LoRa
//! airtimes, two independent packets landing inside one ~1.15 s window is
//! already an infrequent, boot-time-only coincidence — and this codebase
//! already tolerates RX gaps of comparable-or-greater duration continuously,
//! post-boot, on every transmit (`radio.transmit()` blocks RX for its own
//! airtime) and every CAD pass, relying on mesh flooding (a relay repeats the
//! same logical message multiple times) to make any one dropped copy a
//! non-event. A single one-time, strictly-bounded boot-time gap is well
//! inside that already-accepted envelope.
//!
//! # Looping the ripple until dismiss
//!
//! The dedicated render loop above still spins for exactly
//! `UiRuntime::SPLASH_RIPPLE_TOTAL_MS` (~1150 ms, one full ring1+ring2 cycle)
//! and then returns — UNCHANGED. That fixed window is what guarantees the
//! ripple's FIRST cycle renders at a steady ~60 fps regardless of
//! radio/GPS/USB-provisioning timing (see above); stretching it to cover the
//! splash's entire on-screen lifetime would re-delay the exact boot handoff
//! (`main.rs` entering the dispatcher / USB-provisioning-wait loop) this
//! design went out of its way to keep un-delayed, for no additional visual
//! benefit — see next paragraph for why nothing further is needed there.
//!
//! Only the `animate` blocks below changed (`iteration-count: -1`, i.e. loop
//! forever — see the "Animation design" section above). Once
//! `run_splash_ripple()`'s dedicated loop returns, `UiRuntime::step()` — which
//! already calls `update_timers_and_animations()` + `render_if_needed()`
//! unconditionally on EVERY dispatcher-loop iteration, for whichever screen
//! is active, not just the splash — keeps advancing the still-looping ring
//! transitions for as long as the splash remains `active_screen`, using
//! exactly the render cadence the ordinary dispatcher loop already provides
//! (no new polling, no new sleep, no new thread). The ripple therefore visibly
//! repeats for the splash's ENTIRE display window: one steady cycle from the
//! dedicated loop, then further cycles rendered by ordinary `step()` calls —
//! and stops the instant `step()`'s dismissal gate calls `dismiss_splash()`,
//! which drops this `SplashScreenUi` (and every property/animation on it)
//! wholesale (see "Animation design" section above for why a mid-cycle cutoff
//! there is not a defect). No iteration budget, no added delay: the loop's
//! lifetime is tied entirely to the splash's own dismiss lifecycle.
//!
//! # Font-size choice
//!
//! `gen_emoji_font.c` only rasterises emoji glyphs at a curated subset of
//! sizes (`EMOJI_SIZES`, currently `{11,13,14,16,18,20}`) — any other size
//! renders the glyph BLANK (see that file's SYNC INVARIANT comment; this is
//! the same trap `unprovisioned.rs`'s "📻" at 28px already fell into, since
//! 28 is not in `EMOJI_SIZES` — pre-existing, out of scope here). This
//! screen deliberately sizes "📻" at 20px — the same already-rasterised size
//! `pin_entry.rs` uses for "🔐" — and the two plain-Latin labels at 22px /
//! 11px, both already in `PIXEL_SIZES`, so no new font size (and no
//! `gen_emoji_font.c` edit) is needed.
//!
//! # Outer-space theme (per-screen spec row 1: "starfield
//! backdrop" / "starfield behind logo" / "Cadet beside radio logo" / "keep
//! radar ripple + mascot-bob")
//!
//! One additive placement inside `logo_area`, reused as-is from the shared
//! `ui/motifs.slint` contract — no
//! new asset is authored here, and neither the radar-ripple properties nor
//! any Rust wrapper method is touched:
//!
//! - **Cadet beside the radio logo.** `logo_area` is split in half: the
//!   radar rings + "📻" glyph (previously centered across the full 320px
//!   width) now center within a new `icon_box` confined to the LEFT 160px
//!   half — no change to either's own internal centering math, which already
//!   expressed itself in terms of `parent.width`/`parent.height` and simply
//!   inherits `icon_box`'s narrower box. The shared `MascotBob` component
//!   (idle pose, its own default) fills the RIGHT 160px half, centered — the
//!   "one-shot mascot-bob" the spec row calls for is `MascotBob`'s own
//!   self-contained `init`-driven bob-in (see `motifs.slint`'s doc), which
//!   fires once when `SplashScreenUi` is constructed, exactly matching this
//!   screen's existing "fires once per boot" semantics (`SplashScreen::new()`
//!   builds a fresh component on every boot). `RING_END_SIZE_PX` (90px)
//!   still fits comfortably inside `icon_box`'s 160px width, so the ripple's
//!   expanded rings are unaffected by the narrower box.
//!
//! # Full-window backdrop + lower-half line art
//!
//! Splash is the ONLY screen with a durable, empty top+
//! bottom band around its centered content, so it is the sole consumer of
//! BOTH shared full-window
//! components from `ui/motifs.slint`:
//!
//! - **`SpaceBackdrop`** replaces the former `logo_area` header-strip
//!   `Starfield` (removed — keeping both would double the starfield: one
//!   dim field behind the whole window plus a second, denser one confined to
//!   `logo_area`). `SpaceBackdrop` is now the Window's FIRST child (z-bottom,
//!   behind every other node, per the component's own doc), so its baked
//!   ≤0.35-alpha starfield reads as a single, uniform dim field behind the
//!   ring animation, radio glyph, Cadet mascot, wordmark and version — same
//!   visual role the header-strip `Starfield` played for `logo_area` alone,
//!   just extended to the full 320x240 window.
//! - **`PlanetHorizon`** sits in the lower band at `x: 0px; y: 198px`,
//!   declared right after `SpaceBackdrop` and still before the content
//!   `VerticalLayout`, so it too paints behind every foreground element.
//!   `y: 198px` is NOT flush with the window's bottom edge (`168px`, which
//!   Phase 1's own `ui_sim` proof — `ui_sim/src/motif_library.rs`'s
//!   `SpaceBackdropDemoUi` — anticipated as this screen's placement): a
//!   host-sim render at `168px` (`ui_sim/src/splash_lineart.rs`, this
//!   screen's own verbatim-markup proof) measured the motif's crest
//!   painting pixels from y=174 — squarely inside the version string's own
//!   y≈185-193 span (`generate_assets.py::gen_planet_horizon`'s doc: the
//!   limb/orbit strokes crest near the TOP of the 72px band at center-x and
//!   taper down to the bottom corners) — i.e. this screen's own abort
//!   condition, measured, not assumed. Shifting the component down to
//!   `198px` (clipping the asset's own widest, corner-touching bottom rows
//!   against the window's bottom edge, which is visually inert — a smaller,
//!   more-receding arc, not a defect) moves the first painted pixel to
//!   y=200, a measured 7px clear of the version string's last painted row
//!   (193) — see `docs/renders/splash-lineart-host-sim.png` and
//!   `ui_sim/tests/splash_lineart.rs` for the visual + regression evidence.
//!
//! Neither addition touches the radar-ripple properties, `start_animation()`,
//! or the static-complete opacity contract above — both are pure z-bottom
//! `Image` layers with no interactivity of their own.

slint::slint! {
    import { Theme } from "../theme.slint";
    import { MascotBob, SpaceBackdrop, PlanetHorizon } from "../motifs.slint";

    export component SplashScreenUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        // Full-window dim starfield backdrop — FIRST child of the Window, so it
        // paints behind everything else below (z-bottom, per its own doc
        // in `motifs.slint`). Replaces the former `logo_area` header-strip
        // `Starfield` — see module doc's "Full-window backdrop" section.
        SpaceBackdrop {}

        // Dim lower-band planet-horizon line art — declared right after
        // the backdrop (still behind the content layout below). y: 198px,
        // NOT flush with the bottom edge (168px) — measured via host-sim
        // render to keep the motif's crest clear of the version string
        // below; see module doc's "Full-window backdrop + lower-half line
        // art" section for the measurement.
        PlanetHorizon { x: 0px; y: 198px; }

        in property <string> version_str: "";

        // ── Static-complete content — visible from frame one ────────────────
        // Declared at full opacity (NOT 0): the logo, wordmark and version
        // are already fully shown on the very first rendered frame, hold
        // static through all of bring-up, and are never written by
        // `start_animation()` — see the module doc's "Static-complete, then
        // ripple" section. No `animate` block is declared for these three:
        // they never change value, so there is nothing to animate.
        in-out property <float> logo_opacity: 1;
        in-out property <float> title_opacity: 1;
        in-out property <float> version_opacity: 1;

        // ── One-shot ripple state — see module doc's "Animation design" ─────
        // `ripple_active` gates the two rings to fully transparent
        // regardless of their own opacity value below, until
        // `start_animation()` (called on the first `UiRuntime::step()` call
        // AFTER `mark_app_ready()` — see module doc's "Deferred animation
        // start") flips it to 1. It deliberately has NO `animate` block:
        // the reveal itself is instant, un-eased — the rings then visibly
        // animate via their OWN properties below, which `start_animation()`
        // changes in the same batch of writes.
        in-out property <float> ripple_active: 0;
        // Declared at their small, near-opaque "about to ping" values;
        // `start_animation()` moves each to its expanded/faded end value,
        // which is the value change that fires the `animate` transitions
        // below.
        in-out property <float> ring1_size_px: 20;
        in-out property <float> ring1_opacity: 0.85;
        in-out property <float> ring2_size_px: 20;
        in-out property <float> ring2_opacity: 0.85;

        // `iteration-count: -1` (loop forever). See module doc's
        // "Looping the ripple until dismiss" section for why this alone is
        // sufficient (no Rust-side re-trigger, no render-loop change needed)
        // and why a mid-cycle cutoff at dismiss time is not a defect.
        animate ring1_size_px { duration: 850ms; easing: ease-out-quad; iteration-count: -1; }
        animate ring1_opacity { duration: 850ms; easing: ease-out-quad; iteration-count: -1; }
        animate ring2_size_px { duration: 850ms; delay: 300ms; easing: ease-out-quad; iteration-count: -1; }
        animate ring2_opacity { duration: 850ms; delay: 300ms; easing: ease-out-quad; iteration-count: -1; }

        VerticalLayout {
            alignment: center;
            spacing: 6px;

            logo_area := Rectangle {
                height: 96px;
                background: transparent;

                // Radio logo + radar rings, confined to the LEFT half of
                // `logo_area` — see module doc's "Outer-space theme" section
                // for why (`MascotBob` fills the right half, below). Neither
                // the rings' nor the glyph's own centering math changed; they
                // now simply resolve against this narrower box's
                // `parent.width`/`parent.height` instead of `logo_area`'s.
                icon_box := Rectangle {
                    x: 0px;
                    width: parent.width / 2;
                    height: parent.height;
                    background: transparent;

                    // Two staggered radar "ping" rings, expanding + fading
                    // around the logo — the screen's one animation. Gated
                    // fully transparent by `ripple_active` until the ripple
                    // starts (see property doc above).
                    Rectangle {
                        opacity: ripple_active;
                        width: ring1_size_px * 1px;
                        height: ring1_size_px * 1px;
                        x: (parent.width - self.width) / 2;
                        y: (parent.height - self.height) / 2;
                        border-radius: self.width / 2;
                        border-width: 2px;
                        // `.with-alpha()` sets the alpha channel directly (same
                        // semantics as the previous `rgba(0, 180, 255, opacity)`
                        // literal) — Theme.brand-signal is `#00b4ff`, i.e.
                        // rgb(0, 180, 255), so this is a pixel-identical swap.
                        border-color: Theme.brand-signal.with-alpha(ring1_opacity);
                        background: transparent;
                    }
                    Rectangle {
                        opacity: ripple_active;
                        width: ring2_size_px * 1px;
                        height: ring2_size_px * 1px;
                        x: (parent.width - self.width) / 2;
                        y: (parent.height - self.height) / 2;
                        border-radius: self.width / 2;
                        border-width: 2px;
                        border-color: Theme.brand-signal.with-alpha(ring2_opacity);
                        background: transparent;
                    }

                    Text {
                        text: "📻";
                        font-size: Theme.icon-lg; // 20px, EMOJI_SIZES-safe — see module doc
                        // BUG FIX: this Text left
                        // `color` unset, so it fell back to the built-in `Text`
                        // element's default (`Palette.foreground`, per
                        // i-slint-compiler's `apply_default_properties_from_style`
                        // pass) — which, absent any OS dark-mode signal on this
                        // embedded/no-std target, resolves to the *light*-scheme
                        // foreground (`#000000E6`, near-black). Against this
                        // screen's near-black `#0d1117` background that was
                        // effectively invisible (reported as "white-on-white" —
                        // functionally the same class of bug: fill color and
                        // background color at matching luminance). Every other
                        // Text below sets `color` explicitly; this one now does
                        // too, at the same light neutral already used for
                        // readable body text elsewhere in the UI (e.g.
                        // `pin_entry.rs`'s numpad digits).
                        color: Theme.text-primary;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                        width: parent.width;
                        height: parent.height;
                        opacity: logo_opacity;
                    }
                }

                // Cadet mascot, beside the radio logo — see module doc's
                // "Outer-space theme" section. Centered within the RIGHT
                // half of `logo_area`; `MascotBob` defaults to the idle pose
                // and fires its own one-shot bob-in on mount (no property
                // here needs to drive it).
                MascotBob {
                    x: parent.width / 2 + (parent.width / 2 - self.width) / 2;
                    y: (parent.height - self.height) / 2;
                }
            }

            Text {
                text: "MeshCadet";
                opacity: title_opacity;
                font-size: Theme.size-display; // 22px
                font-weight: 700;
                color: Theme.brand-signal;
                horizontal-alignment: center;
            }

            Text {
                text: version_str;
                opacity: version_opacity;
                font-size: Theme.size-preview; // 11px
                color: Theme.text-muted;
                horizontal-alignment: center;
            }
        }
    }
}

/// Rust-side wrapper for the boot splash screen component.
pub struct SplashScreen {
    component: self::SplashScreenUi,
}

impl SplashScreen {
    /// End-state ring diameter — large enough to visibly expand past the
    /// logo, small enough to stay inside `logo_area`'s 96px height without
    /// clipping.
    const RING_END_SIZE_PX: f32 = 90.0;

    /// Create the splash component and show it, WITHOUT starting the
    /// one-shot ripple — see `start_animation` and the module doc's
    /// "Deferred animation start" section for why the two are split. The
    /// logo/wordmark/version are already fully opaque from this call
    /// onward (declared that way in the `.slint` markup above) — nothing
    /// further needs to happen for them to be visible.
    pub fn new() -> anyhow::Result<Self> {
        let component = self::SplashScreenUi::new()
            .map_err(|e| anyhow::anyhow!("slint component init: {:?}", e))?;
        component.show()
            .map_err(|e| anyhow::anyhow!("slint window show: {:?}", e))?;

        Ok(SplashScreen { component })
    }

    /// Fire the one-shot ripple animation (see module doc).
    ///
    /// BUG FIX: this used to run
    /// inside `new()`, i.e. at splash CONSTRUCTION time. Slint's `animate`
    /// blocks compute progress from real wall-clock time elapsed since the
    /// property write that triggered them — NOT from how many times the
    /// cooperative render loop has actually ticked. `UiRuntime::step()` (the
    /// ONLY call-site of `slint::platform::update_timers_and_animations()` /
    /// `render_if_needed()`) is not invoked at all between splash
    /// construction and each boot path's first `step()` call; on the
    /// provisioned boot path that gap spans ALL of NVS/radio/GPS/
    /// admin-server bring-up in `main.rs` (`UiRuntime::step`'s own
    /// `activity_clock_started` doc already documents that this bring-up
    /// "can take longer than a short configured timeout"). If that gap is
    /// >= an animated property's duration, that property is ALREADY at its
    /// settled end value the very first time the render loop ever evaluates
    /// it, so the corresponding transition is never actually seen: the
    /// panel's first-ever frame is already the animation's last frame. This
    /// was always latent (present since the splash's original
    /// implementation), and was originally masked by an unrelated missing-
    /// `color` contrast defect on the logo glyph (fixed separately) that
    /// made the whole thing hard to
    /// notice either way.
    ///
    /// Fix: fire the transition on the FIRST `UiRuntime::step()` call instead
    /// of at construction — exactly mirroring how `splash_started_ms` /
    /// `activity_clock_started` already seed their clocks from the first
    /// `step()` call rather than from construction, for the identical reason.
    /// This guarantees the animation's real start coincides with the moment
    /// the render loop actually begins ticking, on every boot path,
    /// regardless of how long bring-up took to get there.
    ///
    /// FOLLOW-UP BUG FIX: "the
    /// render loop actually begins ticking" (i.e. `step()`'s first-ever call)
    /// turned out not to be late enough. On a boot where GPS baud probe /
    /// radio SPI config / flash hydrate run BEFORE the first `step()` call
    /// but keep subsequent `step()` calls from running at a steady cadence
    /// (e.g. the admin-server thread and radio RX poll are still ramping up
    /// their own contention for the shared SPI bus in the boot's first
    /// stretch), starting the animation on that very first tick could still
    /// land its own frames sparsely — smooth-looking in a bench test, choppy
    /// on real boot timing. The caller (at the time, `UiRuntime::step()`) was
    /// changed to call this method on the first `step()` tick AFTER
    /// `UiRuntime::mark_app_ready()` instead — see that method's doc — so the
    /// animation's start coincides with the main loop actually being steady,
    /// not merely running.
    ///
    /// FOLLOW-UP FIX:
    /// gating the start on `mark_app_ready()` fixed WHEN the animation starts
    /// but not HOW it's driven afterward — `step()` still shared the
    /// dispatcher loop with radio RX / GPS polling every iteration, so the
    /// ripple's own frames still landed irregularly once started (boot-to-
    /// boot smooth-vs-flash variance). The caller is now
    /// `UiRuntime::run_splash_ripple()`, called directly by `main.rs`
    /// immediately after `mark_app_ready()` — on ITS OWN dedicated render
    /// loop, not via `step()` at all — so every one of this method's
    /// `animate` transitions gets rendered at a steady ~16ms cadence
    /// regardless of radio/GPS timing. See `run_splash_ripple`'s doc and the
    /// module doc's "Dedicated render loop for the ripple" section.
    ///
    /// FOLLOW-UP FIX: the
    /// animation itself shrank to JUST the two radar rings. Previously this
    /// method also faded `logo_opacity`/`title_opacity`/`version_opacity` in
    /// from 0, which — combined with those three being declared at 0 as the
    /// screen's "static" state — meant bring-up rendered a blank screen, not
    /// the logo + version text the module doc claimed. Now the
    /// logo/wordmark/version are declared at full opacity from construction
    /// and this method never touches them; it only reveals (`ripple_active`)
    /// and animates (`ring{1,2}_{size_px,opacity}`) the rings. One fewer
    /// moving part at ripple-start time is also a smoothness win in its own
    /// right: the render loop has strictly less to interpolate per frame
    /// during the one window (right after `mark_app_ready()`) where it's
    /// still finishing settling.
    ///
    /// CALLER CONTRACT: the caller MUST call
    /// `slint::platform::update_timers_and_animations()` immediately BEFORE
    /// calling this method (see `UiRuntime::run_splash_ripple`'s call site).
    /// Slint's `animate` blocks anchor their start to `i_slint_core::
    /// animations::current_tick()`, a CACHED value refreshed only by that
    /// call — reading it without ticking first would still return whatever
    /// instant `slint::platform::install()` cached back at construction time,
    /// silently reintroducing this exact defect one call frame later.
    pub fn start_animation(&self) {
        // Reveal the rings (instant — `ripple_active` has no `animate`
        // block) and, in the same batch of writes, move each ring's own
        // size/opacity to its expanded/faded end value — the value change
        // that actually fires the `animate` transitions declared on those
        // two properties. See the module doc's "Animation design" section.
        // logo_opacity/title_opacity/version_opacity are deliberately NOT
        // written here: they are already at their final value (1) from
        // construction and stay there.
        self.component.set_ripple_active(1.0);
        self.component.set_ring1_size_px(Self::RING_END_SIZE_PX);
        self.component.set_ring1_opacity(0.0);
        self.component.set_ring2_size_px(Self::RING_END_SIZE_PX);
        self.component.set_ring2_opacity(0.0);
    }

    /// Set the firmware version string shown at the bottom of the splash.
    pub fn set_version(&self, version: &str) {
        self.component.set_version_str(version.into());
    }

    pub fn hide(&self) {
        self.component.hide().ok();
    }
}
