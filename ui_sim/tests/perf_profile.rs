// SPDX-License-Identifier: GPL-3.0-only
//! Redraw-scope + render-path allocation-count baseline capture for the
//! UI perf-pass baseline.
//!
//! Runs as its own process (Slint `Platform` singleton — see
//! `ui_sim::perf_profile`'s module doc), prints the captured numbers with
//! `eprintln!` (visible via `cargo test -p ui_sim --test perf_profile --
//! --nocapture`) and asserts the invariants the hotspot ledger in
//! `docs/perf/ui-perf-baseline.md` relies on, so a future regression in
//! Slint's dirty-tracking assumptions (or an accidental full-window
//! `request_redraw()` added to a one-shot motif) fails `cargo test` instead
//! of only showing up as a silent number change in the doc.

use ui_sim::alloc_count;
use ui_sim::perf_profile::{DirtyStats, PerfProfileScene};

fn print_stats(label: &str, s: DirtyStats) {
    eprintln!(
        "  {label:<32} lines={:>3}/240  px={:>6}  widest_line_px={:>3}",
        s.lines_touched, s.total_dirty_pixels, s.widest_range
    );
}

#[test]
fn redraw_scope_and_alloc_baseline() {
    eprintln!("\n=== ui-perf-baseline: redraw-scope (dirty-region) capture ===");
    let scene = PerfProfileScene::new();

    // ── Frame 0: first paint is always full (nothing to diff against) ─────
    let (frame0, frame0_allocs) = alloc_count::measure(|| scene.tick());
    print_stats("frame0 (initial full paint)", frame0);
    assert_eq!(
        frame0.lines_touched, 240,
        "initial paint must touch every line (full-window content on frame 0)"
    );
    assert!(
        frame0_allocs.allocs > 0,
        "rendering a real frame should allocate at least once (line-buffer conversion, string building, etc.) — a zero count here means the harness measured nothing"
    );

    // ── Settle the one-shot `init =>` transitions before sampling idle ─────
    // `MascotBob.bob_y` and `Twinkle.twinkle_opacity` are each driven by an
    // `animate` block on a property only the `init` handler ever assigns
    // (motifs.slint:190-192, :217-219). An `animate` on an init-assigned
    // property is a TIMED transition, not something that completes
    // synchronously at construction — `PerfProfilePlatform::duration_since_
    // start` (perf_profile.rs) measures real wall time from just before
    // `PerfProfileUi::new()`, so whether an unslept frame1 lands before or
    // after those transitions finish is an accident of how long scene
    // construction + the frame0 full paint happened to take on this
    // machine, not an invariant. Advance past the longest one (Twinkle's)
    // in ticked increments so the settle is explicit rather than accidental.
    // The +50ms margin on top of Twinkle's duration absorbs scheduler
    // jitter right at the boundary between the settle loop's last sleep and
    // the animation's actual completion instant.
    const MASCOT_BOB_ANIMATION_MS: u64 = 450; // motifs.slint:191
    const TWINKLE_ANIMATION_MS: u64 = 900; // motifs.slint:218
    const INIT_ANIMATION_SETTLE_MS: u64 = TWINKLE_ANIMATION_MS + 50;
    // Compile-time guard on the bound derivation above: if a motif with a
    // longer `animate` duration is ever added to this scene, its
    // motifs.slint duration must become the new basis, not silently keep
    // Twinkle's.
    const _: () = assert!(TWINKLE_ANIMATION_MS > MASCOT_BOB_ANIMATION_MS);

    let mut settle_saw_dirty_tick = false;
    let settle_deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(INIT_ANIMATION_SETTLE_MS);
    while std::time::Instant::now() < settle_deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let s = scene.tick();
        settle_saw_dirty_tick |= s.lines_touched > 0;
    }
    // Flush the transition's final settled-value frame (it may land exactly
    // on the deadline, in which case the loop above already exited without
    // painting it) before the frame the assertion below grades.
    scene.tick();
    eprintln!(
        "  settle: observed a dirty tick while advancing past init animations: {settle_saw_dirty_tick}"
    );

    // ── Frame 1: idle steady state, nothing changed — should be a no-op ────
    let idle = scene.tick();
    print_stats("frame1 (idle, no property change)", idle);
    assert_eq!(
        idle,
        DirtyStats::default(),
        "an idle tick with no property change must dirty NOTHING — platform.rs's own doc \
         claims render_if_needed is a no-op at idle; this is the host-side proof of that claim"
    );

    // ── RocketOnSend: fire the one-shot, sample every tick until settled ───
    scene.set_send_trigger(true);
    let mut rocket_peak = DirtyStats::default();
    let mut rocket_ticks = 0;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let s = scene.tick();
        if s.lines_touched > 0 {
            rocket_ticks += 1;
            if s.total_dirty_pixels > rocket_peak.total_dirty_pixels {
                rocket_peak = s;
            }
        } else if rocket_ticks > 0 {
            break; // settled back to zero-dirty steady state
        }
    }
    print_stats("RocketOnSend peak dirty frame", rocket_peak);
    eprintln!(
        "  RocketOnSend: {} dirty ticks over its one-shot transition",
        rocket_ticks
    );
    assert!(
        rocket_ticks > 0,
        "RocketOnSend's play=true must dirty at least one frame"
    );
    assert!(
        rocket_peak.lines_touched < 240,
        "RocketOnSend is a small nested motif — it must never force a full-window repaint \
         (lines_touched={} of 240)",
        rocket_peak.lines_touched
    );

    // ── CometOnNotify: same shape of proof ──────────────────────────────────
    scene.set_notify_trigger(true);
    let mut comet_peak = DirtyStats::default();
    let mut comet_ticks = 0;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let s = scene.tick();
        if s.lines_touched > 0 {
            comet_ticks += 1;
            if s.total_dirty_pixels > comet_peak.total_dirty_pixels {
                comet_peak = s;
            }
        } else if comet_ticks > 0 {
            break;
        }
    }
    print_stats("CometOnNotify peak dirty frame", comet_peak);
    eprintln!(
        "  CometOnNotify: {} dirty ticks over its one-shot transition",
        comet_ticks
    );
    assert!(
        comet_ticks > 0,
        "CometOnNotify's play=true must dirty at least one frame"
    );
    assert!(
        comet_peak.lines_touched < 240,
        "CometOnNotify is a header-strip-height motif — it must never force a full-window \
         repaint (lines_touched={} of 240)",
        comet_peak.lines_touched
    );

    eprintln!("=== end redraw-scope capture ===\n");
}
