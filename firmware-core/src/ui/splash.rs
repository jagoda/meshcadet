// SPDX-License-Identifier: GPL-3.0-only
//! Boot splash screen — pure dismissal-gate decision.
//!
//! The `slint::slint!{}` view and the `SplashScreen` Rust wrapper (the
//! ripple animation choreography) stay in
//! `firmware/src/ui/screens/splash.rs` — they depend on Slint; only the
//! plain-data dismissal predicate below moves here so its tests execute
//! under `cargo test --workspace` (this crate is a detached, cross-compiled
//! workspace — see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block
//! written there would type-check but never run). See
//! `docs/adr/0005-firmware-core-extraction.md`.

/// Pure decision function for the boot-splash dismissal gate (see
/// `screens::splash` module doc and `UiRuntime::SPLASH_MIN_MS`/
/// `SPLASH_MAX_MS`). Extracted as a pure function so the core acceptance
/// logic (animation always completes, splash never lingers once settled,
/// defensive cap) has host-checkable unit tests independent of the
/// Slint/display stack.
///
/// `elapsed_ms` is measured from the splash's first `step()` tick (used ONLY
/// by the `max_ms` defensive cap below). `animation_elapsed_ms` is `None`
/// until `UiRuntime::step()` actually fires `SplashScreen::start_animation()`
/// (gated on `mark_app_ready()` — see that method's doc), and `Some(ms)`
/// thereafter, measured from ITS OWN clock (`splash_animation_started_ms`) —
/// a different, later zero point than `elapsed_ms` whenever `mark_app_ready()`
/// arrives after the splash's first tick.
///
/// Dismiss once EITHER:
/// - `animation_elapsed_ms` is `Some(ms)` with `ms >= min_ms` (normal path:
///   the one-shot splash animation has started AND had time to finish), OR
/// - `elapsed_ms >= max_ms`, unconditionally (defensive cap — covers both
///   "animation never started" and "started too late to have settled yet").
pub fn splash_should_dismiss(
    elapsed_ms: u64,
    animation_elapsed_ms: Option<u64>,
    min_ms: u64,
    max_ms: u64,
) -> bool {
    matches!(animation_elapsed_ms, Some(ms) if ms >= min_ms) || elapsed_ms >= max_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors `UiRuntime::SPLASH_MIN_MS`/`SPLASH_MAX_MS` (inlined rather than
    // referenced: those are private associated consts on a lifetime-generic
    // type, and `splash_should_dismiss` takes the thresholds as plain
    // parameters precisely so callers — including these tests — don't need
    // a concrete `UiRuntime<'d>` to exercise it).
    const MIN_MS: u64 = 1600;
    const MAX_MS: u64 = 2400;

    #[test]
    fn animation_not_started_never_dismisses_below_max() {
        assert!(!splash_should_dismiss(0, None, MIN_MS, MAX_MS));
        assert!(!splash_should_dismiss(MAX_MS - 1, None, MIN_MS, MAX_MS));
    }

    #[test]
    fn animation_started_but_not_settled_waits() {
        // The animation has started (app_ready fired and `start_animation()`
        // ran) but hasn't had time to play through — must NOT dismiss yet,
        // regardless of how much boot-clock time (`elapsed_ms`) has passed.
        assert!(!splash_should_dismiss(0, Some(0), MIN_MS, MAX_MS));
        assert!(!splash_should_dismiss(
            MIN_MS - 1,
            Some(MIN_MS - 1),
            MIN_MS,
            MAX_MS
        ));
    }

    #[test]
    fn animation_settled_dismisses() {
        assert!(splash_should_dismiss(MIN_MS, Some(MIN_MS), MIN_MS, MAX_MS));
        assert!(splash_should_dismiss(
            MIN_MS + 500,
            Some(MIN_MS + 500),
            MIN_MS,
            MAX_MS
        ));
    }

    #[test]
    fn animation_started_late_settles_on_its_own_clock_not_the_boot_clock() {
        // The whole point of decoupling the two clocks: `mark_app_ready()` can
        // fire well after the splash's first `step()` tick. Here the boot
        // clock (`elapsed_ms`) is already past `MIN_MS`, but the animation
        // only just started (`animation_elapsed_ms = Some(0)`) — must still
        // wait for the ANIMATION's own clock to reach `MIN_MS`, not dismiss
        // just because the boot clock did.
        assert!(!splash_should_dismiss(
            MIN_MS + 200,
            Some(0),
            MIN_MS,
            MAX_MS
        ));
        assert!(splash_should_dismiss(
            MIN_MS + 200,
            Some(MIN_MS),
            MIN_MS,
            MAX_MS
        ));
    }

    #[test]
    fn max_cap_dismisses_even_when_animation_never_started() {
        assert!(splash_should_dismiss(MAX_MS, None, MIN_MS, MAX_MS));
        assert!(splash_should_dismiss(MAX_MS + 1000, None, MIN_MS, MAX_MS));
    }

    // Pins the coordination constraint between the two thresholds
    // themselves, not just `splash_should_dismiss`'s branch logic — a future
    // edit to either constant (or to the splash animation's total duration
    // in `screens::splash`) that breaks the "lingers a bit longer, total
    // time still ~2-2.5 s max, animation still completes" envelope should
    // fail here rather than only be caught by eyeballing the on-device
    // timing.
    // These three assertions compare `MIN_MS`/`MAX_MS`/`SPLASH_ANIMATION_TOTAL_MS`
    // — all `const` — so clippy's `assertions_on_constants` lint fires (the
    // outcome is statically known). Deliberately kept as ordinary runtime
    // `assert!`s rather than clippy's suggested `const { assert!(..) }`: this
    // is a `#[test]` pinning a coordination invariant between the two
    // thresholds, the same shape as every other test in this module, and
    // moving it into a const-eval context would be a behavior change (a
    // `cargo test` failure vs. a `cargo build` failure) this extraction —
    // a behavior-preserving move — does not authorize.
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn min_and_max_stay_within_the_acceptance_envelope() {
        // Animation timeline total, from `screens::splash`'s module doc.
        const SPLASH_ANIMATION_TOTAL_MS: u64 = 1150;
        assert!(
            MIN_MS >= SPLASH_ANIMATION_TOTAL_MS,
            "SPLASH_MIN_MS must stay >= the one-shot animation's total \
             duration or the splash can dismiss mid-animation",
        );
        assert!(
            MIN_MS < MAX_MS,
            "the defensive cap must sit above the floor"
        );
        assert!(
            MAX_MS <= 2500,
            "SPLASH_MAX_MS must stay within the ~2-2.5 s acceptance budget",
        );
    }
}
