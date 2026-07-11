// SPDX-License-Identifier: GPL-3.0-only
//! Screen-entry-fade repaint-cost proof.
//!
//! FINDING THIS TEST PINS: `content_opacity`/`reveal_opacity` — the
//! screen-entry fade EVERY themed screen uses (`contact_list.rs`,
//! `message_view.rs`, `pin_entry.rs`, `gps_status.rs`, `admin_menu.rs`,
//! `unprovisioned.rs`, and (scoped to its emoji-picker overlay)
//! `compose.rs`) — wraps its content in an `opacity: content_opacity`
//! binding. Per Slint's own dirty-region rule (`i-slint-core`'s
//! `partial_renderer.rs`: "When the opacity ... change[s], this will impact
//! all the children, including the ones outside the element, regardless if
//! they are themselves dirty or not"), EVERY tick that opacity value is
//! still interpolating, the WHOLE wrapped region — here, and on every real
//! screen, effectively the whole window minus whatever static backdrop sits
//! outside it — is marked dirty and re-flushed, not just once at
//! navigation. `EntryFadeScene` (`ui_perf::EntryFadeScene`, `src/lib.rs`)
//! mirrors that exact idiom against the real `motifs.slint`/`theme.slint`
//! assets, so the counts below are a real measurement of production
//! behavior, not a guess.
//!
//! `UiRuntime::step()` (`firmware/src/ui/mod.rs`) runs once per
//! dispatcher-loop iteration, which idles at roughly `RX_POLL_YIELD_MS` (5
//! ms, ~200 Hz) cadence — so an UNTHROTTLED render call on every iteration
//! re-flushes this scene's full bounding region roughly every 5 ms for the
//! entire 200 ms fade: ~40 full-window flushes for one screen navigation,
//! each contending with the shared SPI2 bus's next CAD attempt / RX poll.
//! `step()`'s render-cadence throttle (`RENDER_MIN_INTERVAL_MS`, `Self::
//! render_settling`) caps this to one flush per `RENDER_MIN_INTERVAL_MS`
//! while `Window::has_active_animations()` reports an animation still in
//! flight. This test proves BOTH halves of that fix's contract on the host,
//! against the real renderer:
//!   1. capping a full-window fade this way strictly reduces total renders
//!      (the mechanism actually helps), and
//!   2. the FINAL settled framebuffer is bit-for-bit IDENTICAL whether the
//!      fade was rendered every tick or only every `RENDER_MIN_INTERVAL_MS`
//!      (the "identical visual result" hard constraint holds — Slint's
//!      `animate` interpolates from wall-clock time elapsed, not from how
//!      many times it happens to get rendered, so skipping intermediate
//!      samples cannot change the settled end state).
//!
//! Both scenarios run against ONE shared [`Harness`] (Slint enforces a
//! single platform per process, same constraint every other `ui_perf`/
//! `ui_sim` render rig documents) — each scenario instantiates its OWN fresh
//! `EntryFadeScene` and `.show()`s it, which is exactly the "fresh Slint
//! component per screen navigation" pattern `firmware/src/ui/mod.rs`'s own
//! `navigate_to_*` methods use, so replacing the shown component mid-process
//! is itself production-faithful, not a test-only shortcut.

use slint::ComponentHandle;
use ui_perf::{Harness, HEIGHT};

/// Mirrors `firmware/src/ui/mod.rs::UiRuntime::RENDER_MIN_INTERVAL_MS`. Kept
/// as an independent literal (not shared via `#[path]`/a common crate)
/// because `firmware/` cross-compiles for `xtensa-esp32s3-espidf` and cannot
/// be linked into a host test binary — same split every other ported
/// constant in this measurement suite documents (see `docs/perf/ui-perf-
/// baseline.md` §1). If the firmware constant ever changes, update this one
/// to match.
const RENDER_MIN_INTERVAL_MS: u64 = 16;

/// Dispatcher-loop tick cadence this rig assumes when idle (mirrors
/// `firmware/src/main.rs::RX_POLL_YIELD_MS`).
const DISPATCHER_TICK_MS: u64 = 5;

/// Runs `EntryFadeScene`'s 200ms screen-entry fade to settle at
/// `DISPATCHER_TICK_MS` cadence on the given (already-installed) `Harness`,
/// either rendering every tick (`throttled = false`) or applying the SAME
/// cadence cap `UiRuntime::step()` uses (`throttled = true`). Returns
/// `(full_window_frames, total_frames_rendered, final_framebuffer_hash)`.
fn run_fade(h: &mut Harness, throttled: bool) -> (usize, usize, u64) {
    let ui = ui_perf::EntryFadeScene::new().expect("EntryFadeScene::new");
    ui.show().expect("show");

    // Frame 0: the fade's `init =>` write already happened at construction
    // (see `motifs.slint`'s "deferred-write" precedent this scene's
    // `content_opacity` follows) — this first tick is the unavoidable
    // navigation full paint every screen pays regardless of the fade.
    let settle = h.frame().expect("first frame must paint");
    assert_eq!(
        settle.lines_flushed,
        HEIGHT as usize,
        "navigation is always a full-window first paint, fade or not"
    );

    let mut full_window_frames = 0usize;
    let mut total_frames_rendered = 0usize;
    let mut last_render_ms: u64 = 0;
    let mut render_settling = false;
    let mut elapsed_ms: u64 = 0;

    // Run well past the 200ms fade duration so it's fully settled either way.
    for _ in 0..80 {
        elapsed_ms += DISPATCHER_TICK_MS;
        h.advance(DISPATCHER_TICK_MS);
        h.tick();

        let render_due = !throttled
            || !render_settling
            || elapsed_ms.saturating_sub(last_render_ms) >= RENDER_MIN_INTERVAL_MS;
        if !render_due {
            continue;
        }
        if let Some(stats) = h.render_now() {
            total_frames_rendered += 1;
            last_render_ms = elapsed_ms;
            if stats.lines_flushed == HEIGHT as usize {
                full_window_frames += 1;
            }
        }
        render_settling = h.has_active_animations();
    }

    (full_window_frames, total_frames_rendered, h.framebuffer_hash())
}

#[test]
fn entry_fade_render_cadence_throttle_reduces_renders_and_settles_identically() {
    let mut h = Harness::new();

    let (unthrottled_full, unthrottled_total, unthrottled_hash) = run_fade(&mut h, false);
    eprintln!(
        "[entry-fade] unthrottled: {unthrottled_total} frames rendered, \
         {unthrottled_full} of them full-window ({}x{})",
        ui_perf::WIDTH,
        HEIGHT
    );
    // The measured baseline (docs/perf/ui-perf-baseline.md):
    // at ~5ms dispatcher cadence a 200ms fade paints on the order
    // of a few dozen full-window frames, not one. Assert a generous lower
    // bound (>= 8, i.e. at minimum 40ms worth of full-window churn) so this
    // stays a meaningful regression tripwire without being flaky against
    // easing-curve rounding at the exact tick boundaries.
    assert!(
        unthrottled_full >= 8,
        "expected the UNTHROTTLED screen-entry fade to repaint the full window many times \
         over its 200ms duration (found {unthrottled_full}) — if this drops, either the fade \
         got cheaper (good — update this test's framing) or the harness stopped measuring what \
         it claims to"
    );

    let (throttled_full, throttled_total, throttled_hash) = run_fade(&mut h, true);
    eprintln!(
        "[entry-fade] throttled:   {throttled_total} frames rendered, \
         {throttled_full} of them full-window ({}x{})",
        ui_perf::WIDTH,
        HEIGHT
    );

    // The fix's whole point: strictly fewer renders, and strictly fewer
    // full-window ones, under the SAME 200ms fade.
    assert!(
        throttled_total < unthrottled_total,
        "the render-cadence throttle must reduce the total number of renders \
         (throttled={throttled_total}, unthrottled={unthrottled_total})"
    );
    assert!(
        throttled_full < unthrottled_full,
        "the render-cadence throttle must reduce the number of full-window flushes \
         (throttled={throttled_full}, unthrottled={unthrottled_full})"
    );

    // The hard constraint: identical visual result. Both runs settle to the
    // exact same final pixel content — the throttle changes WHEN frames are
    // sampled, never the animation's curve or its settled end state.
    assert_eq!(
        throttled_hash, unthrottled_hash,
        "throttled and unthrottled fades must settle to a BIT-IDENTICAL final framebuffer \
         (same curve, same duration, same end state — only the sampling cadence differs)"
    );

    // ── Responsiveness guard: a fresh one-off redraw, fired AFTER the fade has
    // fully settled, must never wait on the throttle ─────────────────────────
    //
    // The throttle only ever defers a render while `render_settling` is
    // `true` (an animation was observed still in flight on the last actual
    // render — see `UiRuntime::step()`'s field docs). By the end of the
    // throttled run above the fade is fully settled, so
    // `has_active_animations()` must already read `false` — confirming the
    // mirrored `render_settling` this test drives is `false` too, exactly
    // like `UiRuntime`'s own field would be. A fresh redraw observed under
    // that condition renders on the very next tick, completely uncapped —
    // this is the guarantee that keeps tap-to-first-frame latency
    // (`docs/perf/ui-perf-baseline.md` §8.A) unaffected by this fix.
    assert!(
        !h.has_active_animations(),
        "the fade must be fully settled (no active animations) by the end of the throttled run \
         above for this responsiveness check to be meaningful"
    );
    h.advance(1); // one dispatcher iteration later
    h.request_redraw(); // mirrors a fresh nav/model-update/incoming-message redraw
    h.tick();
    let post_settle_frame = h.render_now();
    assert!(
        post_settle_frame.is_some(),
        "a fresh redraw fired after the fade has settled must render on the very next tick \
         — the render-cadence throttle must never delay a one-off redraw once nothing is \
         actively animating"
    );
}
