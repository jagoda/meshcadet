// SPDX-License-Identifier: GPL-3.0-only
//! Pure decision logic for `meshcadet-power-optimization` Phase 5 (the
//! idle-screen enabler leg): the dim-before-sleep step, the render-skip/
//! forced-repaint invariants around the sleep/wake transition, and the
//! adaptive `ui_task` tick period.
//!
//! Everything here is plain data in, plain data out — no hardware, no
//! Slint — so it runs under `cargo test --workspace`, matching the pattern
//! `touch::touch_wake_transition` and `lock::idle_lock_due` already
//! established for the other screen-sleep-adjacent decisions. The two real
//! call sites are `firmware/src/ui/mod.rs`'s `UiRuntime::step()` (the
//! screen-sleep inactivity check, `sync_backlight_brightness`, and the
//! render section) and `firmware/src/ui_task.rs`'s steady-state loop (the
//! `evt_rx.recv_timeout` period).

// ── Dim-before-sleep ─────────────────────────────────────────────────────────

/// How long before the real `screen_sleep_timeout_s` deadline the dim-step
/// warning engages.
///
/// A fixed lead time rather than a fraction of `screen_sleep_timeout_s`:
/// the point of the dim step is "notice before it goes dark", which is a
/// roughly constant human reaction window regardless of whether the
/// configured timeout is 10s or 120s.
pub const DIM_LEAD_MS: u64 = 3_000;

/// Percentage of the user's configured `backlight_brightness` to drive
/// during the dim-lead window.
pub const DIM_FRACTION_PCT: u32 = 25;

/// What `UiRuntime::step()`'s screen-sleep inactivity check should do this
/// tick, given how long the screen has been idle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenIdleAction {
    /// Not (or no longer) inside the dim-lead window — full configured
    /// brightness.
    Awake,
    /// Inside the dim-lead window before the real timeout: one step down in
    /// brightness, still awake (`screen_asleep` stays `false`) — an early
    /// warning that sleep is imminent.
    Dim,
    /// The full timeout has elapsed: go to sleep
    /// (`UiRuntime::sleep_screen`).
    Sleep,
}

/// Decide this tick's [`ScreenIdleAction`] from `elapsed_ms` (time since the
/// last touch/key activity, i.e. `now_ms.saturating_sub(last_activity_ms)`)
/// against `timeout_s` (`RuntimeSettings::screen_sleep_timeout_s`) and
/// `dim_lead_ms` (how long before the real timeout the dim step engages).
///
/// `timeout_s == 0` is the existing "never sleep" sentinel
/// (`RuntimeSettings::screen_sleep_timeout_s`'s own doc) — dimming is part
/// of the lead-up to a sleep that will never happen, so it is suppressed
/// too, not just the sleep itself: always [`ScreenIdleAction::Awake`].
///
/// `dim_lead_ms` is clamped to `timeout_ms` internally (`.min(timeout_ms)`)
/// so a short `screen_sleep_timeout_s` (the admin-menu stepper's 5s
/// decrement floor, or any other non-zero value under `DIM_LEAD_MS`/1000
/// seconds) never underflows computing the dim-start instant — the dim
/// window simply covers the whole awake period in that case (dims
/// immediately, then sleeps at the real timeout) rather than this function
/// ever panicking or silently skipping the dim step.
pub fn screen_idle_action(elapsed_ms: u64, timeout_s: u8, dim_lead_ms: u64) -> ScreenIdleAction {
    if timeout_s == 0 {
        return ScreenIdleAction::Awake;
    }
    let timeout_ms = (timeout_s as u64) * 1000;
    if elapsed_ms >= timeout_ms {
        return ScreenIdleAction::Sleep;
    }
    let dim_at_ms = timeout_ms.saturating_sub(dim_lead_ms.min(timeout_ms));
    if elapsed_ms >= dim_at_ms {
        ScreenIdleAction::Dim
    } else {
        ScreenIdleAction::Awake
    }
}

/// Brightness percentage to drive during [`ScreenIdleAction::Dim`], derived
/// from `configured_pct` (the user's `RuntimeSettings::backlight_brightness`).
///
/// A proportion of the CONFIGURED level (`DIM_FRACTION_PCT`), not a fixed
/// absolute value — a user who already runs at low brightness still sees a
/// perceptible dim step, and a user at full brightness dims by the same
/// proportion. Floored at 1, never 0: the dim step is a visible "sleep is
/// imminent" warning, distinct from `sleep_screen`'s actual backlight-off —
/// a 0% dim would look identical to already-asleep and defeats the
/// warning's whole purpose.
pub fn dim_brightness_pct(configured_pct: u8) -> u8 {
    (((configured_pct as u32) * DIM_FRACTION_PCT) / 100).max(1) as u8
}

// ── Render skip / forced repaint ─────────────────────────────────────────────

/// Whether `UiRuntime::step()`'s render section should even attempt
/// `render_if_needed` this tick.
///
/// The ST7789 is fully asleep (`SLPIN` + `DISPOFF`, `TDeckDisplay::sleep`)
/// for the entire `screen_asleep` window — its output stage is off and the
/// panel is not observable — so flushing dirty regions over SPI to it is
/// pure waste, not merely redundant. This is the render-skip half of the
/// Phase 5 objective; the xtask guard `verify-render-asleep-gate` pins that
/// `step()`'s render section actually calls this before `render_if_needed`.
pub fn render_gate(screen_asleep: bool) -> bool {
    !screen_asleep
}

/// Whether waking the screen (a `screen_asleep: true -> false` transition)
/// must force a full repaint on the very next render, rather than trusting
/// whatever partial dirty-region state Slint's `MinimalSoftwareWindow`
/// happens to hold.
///
/// Always `true` given a real wake (`was_asleep: true`): [`render_gate`]
/// above means every model update (battery/GPS/clock/incoming message) that
/// arrived while asleep had its render skipped outright, so the ST7789's
/// GRAM reflects whatever was on screen at the MOMENT sleep began, not the
/// current state. `was_asleep: false` never forces one on its own — kept as
/// an explicit input (rather than hardcoding `true` at the one call site)
/// so `UiRuntime::wake_screen` states its own precondition instead of
/// silently assuming it.
pub fn wake_forces_full_repaint(was_asleep: bool) -> bool {
    was_asleep
}

// ── Adaptive ui_task tick ─────────────────────────────────────────────────────

/// `ui_task`'s asleep-and-idle (no blink burst live) `recv_timeout` period,
/// in milliseconds.
///
/// Constraint P2 (responsiveness) bounds this from the wake-latency side,
/// but a SECOND, harder constraint bounds it from above and is now the
/// binding one: `TouchDriver::poll_event`'s cadence contract
/// (`firmware/src/ui/touch.rs`). The GT911 does not queue events — a tap
/// whose entire press-then-release cycle completes inside one poll gap is
/// lost outright, not delayed (M2-gate finding,
/// `meshcadet-power-m2-gate-20260823-223120079`; at the previous 120ms
/// value this dropped ~17% of 100ms taps). This constant MUST NOT exceed
/// [`crate::ui::touch::GT911_MIN_RELIABLE_TAP_MS`] — see that constant's
/// doc for the tap-loss mechanism — and
/// `asleep_idle_tick_bounds_gt911_tap_loss` below pins the two against each
/// other so a future edit to either one that breaks the bound fails loudly
/// instead of silently reintroducing wake-tap loss.
///
/// The HONEST worst-case wake-to-first-paint number is dominated by a FIXED
/// cost this same phase introduces, not by this constant:
/// `TDeckDisplay::wake`'s mandatory `SLPOUT` settling delay (120ms — the
/// ST7789's own datasheet requirement, not tunable) plus one forced
/// full-repaint flush (~30.7ms, `docs/perf/ui-perf-baseline.md` §4.1,
/// currency 2026-08-03) already totals ~150.7ms BEFORE this period's own
/// contribution is added. **Stated bound (this phase's own re-derivation,
/// not the plan-of-record's rough "~150ms" aspiration): worst-case
/// wake-to-first-paint ≈ `ASLEEP_IDLE_TICK_MS` + 120ms + 30.7ms ≈ 200.7ms.**
/// 50ms — equal to [`ASLEEP_BLINK_TICK_MS`] — is the largest value that
/// still satisfies the GT911 tap-loss bound above; it still cuts the
/// touch/keyboard I²C poll rate from ~62.5 Hz (awake) down to 20 Hz, most of
/// this leg's win even though it is no longer the number the wake-latency
/// aspiration alone would have picked. The M2 gate re-derives this number
/// against the merged tree rather than trusting this comment.
pub const ASLEEP_IDLE_TICK_MS: u64 = 50;

/// `ui_task`'s asleep-but-blink-burst-live `recv_timeout` period, in
/// milliseconds.
///
/// Bounded by the Nyquist argument against `BLINK_PHASE_MS` (150ms,
/// `firmware_core::notification`): sampling (i.e. rendering, via
/// `sync_keyboard_backlight`'s `notif.poll_blink` call) an on/off phase of
/// period 150ms at a rate slower than every 75ms risks missing an entire
/// phase. 50ms leaves comfortable margin under that 75ms ceiling.
pub const ASLEEP_BLINK_TICK_MS: u64 = 50;

/// Select `ui_task`'s next `evt_rx.recv_timeout` period.
///
/// Three inputs fully determine it:
/// - `screen_asleep`: `false` always wins outright at `awake_tick_ms` — a
///   slowed tick is a sleep-only trade, never one that touches perceived
///   responsiveness while the screen is actually in use (constraint P2).
/// - `blink_active`: the ONE thing that can override a slow asleep tick
///   back up, even while `screen_asleep` — see [`ASLEEP_BLINK_TICK_MS`]'s
///   doc for the Nyquist argument this exists to satisfy. Constraint P3
///   (notifications) is a correctness bound, not a nicety: a blanket
///   asleep-tick slowdown would silently break the incoming-message blink.
/// - Otherwise (`screen_asleep` and no blink burst live): `asleep_idle_tick_ms`
///   — see [`ASLEEP_IDLE_TICK_MS`]'s doc for its own bound (constraint P2
///   again, the general wake-latency case).
///
/// Deliberately only three inputs, not four: a dispatcher event already
/// queued when `recv_timeout` is called returns immediately regardless of
/// which period this function picked (`std::sync::mpsc::Receiver::
/// recv_timeout` only actually waits the full period when the queue is
/// empty), so a "pending events" input would add a parameter this function
/// could never observe a wrong answer from — the channel's own semantics
/// already give an immediate wake for that case, for free.
pub fn next_tick_period_ms(
    screen_asleep: bool,
    blink_active: bool,
    awake_tick_ms: u64,
    asleep_idle_tick_ms: u64,
    asleep_blink_tick_ms: u64,
) -> u64 {
    if !screen_asleep {
        return awake_tick_ms;
    }
    if blink_active {
        asleep_blink_tick_ms
    } else {
        asleep_idle_tick_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::touch::GT911_MIN_RELIABLE_TAP_MS;

    // ── ASLEEP_IDLE_TICK_MS vs. the GT911 tap-loss bound ─────────────────
    //
    // The binding invariant this pins: `TouchDriver::poll_event`
    // (`firmware/src/ui/touch.rs`) can only guarantee catching a physical
    // tap if it is called at least once during that tap's press-to-release
    // window. `ASLEEP_IDLE_TICK_MS` widening past
    // `GT911_MIN_RELIABLE_TAP_MS` reopens the M2-gate wake-tap-loss finding
    // (`meshcadet-power-m2-gate-20260823-223120079`) for taps at or above
    // that floor — a regression this host test catches without any I2C/HIL
    // rig, same rationale as `touch::touch_wake_transition`'s tests.
    #[test]
    fn asleep_idle_tick_bounds_gt911_tap_loss() {
        // Both operands are `const`, so this is decidable at compile time —
        // clippy's `assertions_on_constants` correctly flags a runtime
        // `assert!` here as dead weight and points at the inline-const-block
        // form instead. Wrapping it in `const { .. }` keeps the invariant
        // exactly as binding (a violation now fails the *build*, not just
        // this test) while satisfying `-D warnings`.
        const {
            assert!(
                ASLEEP_IDLE_TICK_MS <= GT911_MIN_RELIABLE_TAP_MS,
                "ASLEEP_IDLE_TICK_MS exceeds GT911_MIN_RELIABLE_TAP_MS — a tap of the \
                 reference floor duration can now complete entirely inside one asleep poll \
                 gap and be lost outright, not merely delayed (see both constants' docs)",
            );
        }
    }

    // ── screen_idle_action ───────────────────────────────────────────────

    #[test]
    fn never_sleep_sentinel_is_always_awake() {
        assert_eq!(
            screen_idle_action(0, 0, DIM_LEAD_MS),
            ScreenIdleAction::Awake
        );
        assert_eq!(
            screen_idle_action(1_000_000, 0, DIM_LEAD_MS),
            ScreenIdleAction::Awake,
            "timeout_s == 0 must never dim or sleep, regardless of elapsed time"
        );
    }

    #[test]
    fn well_before_the_dim_window_is_awake() {
        // 30s timeout, 3s dim lead -> dim starts at 27s. 10s elapsed is
        // comfortably before that.
        assert_eq!(
            screen_idle_action(10_000, 30, 3_000),
            ScreenIdleAction::Awake
        );
    }

    #[test]
    fn inside_the_dim_lead_window_is_dim() {
        // 30s timeout, 3s dim lead -> dim window is [27_000, 30_000).
        assert_eq!(screen_idle_action(27_000, 30, 3_000), ScreenIdleAction::Dim);
        assert_eq!(screen_idle_action(29_999, 30, 3_000), ScreenIdleAction::Dim);
    }

    #[test]
    fn at_or_past_the_timeout_is_sleep() {
        assert_eq!(
            screen_idle_action(30_000, 30, 3_000),
            ScreenIdleAction::Sleep
        );
        assert_eq!(
            screen_idle_action(60_000, 30, 3_000),
            ScreenIdleAction::Sleep,
            "well past the timeout must still resolve to Sleep, not overflow/panic"
        );
    }

    #[test]
    fn short_timeout_shorter_than_dim_lead_dims_immediately_then_sleeps() {
        // 2s timeout, 3s dim lead: dim_at_ms clamps to 0 (dim_lead_ms.min(timeout_ms)),
        // so the whole awake window is the dim window.
        assert_eq!(screen_idle_action(0, 2, 3_000), ScreenIdleAction::Dim);
        assert_eq!(screen_idle_action(1_999, 2, 3_000), ScreenIdleAction::Dim);
        assert_eq!(screen_idle_action(2_000, 2, 3_000), ScreenIdleAction::Sleep);
    }

    // ── dim_brightness_pct ───────────────────────────────────────────────

    #[test]
    fn dims_to_a_quarter_of_full_brightness() {
        assert_eq!(dim_brightness_pct(100), 25);
    }

    #[test]
    fn dims_proportionally_to_a_lower_configured_setting() {
        assert_eq!(dim_brightness_pct(40), 10);
    }

    #[test]
    fn never_dims_all_the_way_to_zero() {
        // BACKLIGHT_BRIGHTNESS_MIN_PCT is 10 (pin_menu.rs) -> 10*25/100 = 2.
        assert_eq!(dim_brightness_pct(10), 2);
        // A pathological/legacy-blob value of 1 would compute to 0 before
        // the floor — must still show SOMETHING, distinct from asleep.
        assert_eq!(dim_brightness_pct(1), 1);
        assert_eq!(dim_brightness_pct(0), 1);
    }

    // ── render_gate / wake_forces_full_repaint ───────────────────────────

    #[test]
    fn render_gate_skips_only_while_asleep() {
        assert!(render_gate(false), "awake must render");
        assert!(
            !render_gate(true),
            "asleep must skip render_if_needed entirely"
        );
    }

    #[test]
    fn wake_forces_full_repaint_only_on_a_real_wake() {
        assert!(wake_forces_full_repaint(true));
        assert!(!wake_forces_full_repaint(false));
    }

    // ── next_tick_period_ms ──────────────────────────────────────────────

    const AWAKE_MS: u64 = 16;

    // "The whole point of Phase 5 is a slower asleep tick than the awake
    // one" — both sides are constants, so this is a COMPILE-time check
    // (clippy correctly flags a runtime `assert!` over two consts as dead
    // weight; a `const` block is the right home for it, not a `#[test]`).
    const _: () = assert!(
        ASLEEP_IDLE_TICK_MS > AWAKE_MS,
        "asleep-idle tick must be slower than the awake tick"
    );

    #[test]
    fn awake_period_is_exactly_the_awake_tick_unchanged() {
        assert_eq!(
            next_tick_period_ms(
                false,
                false,
                AWAKE_MS,
                ASLEEP_IDLE_TICK_MS,
                ASLEEP_BLINK_TICK_MS
            ),
            AWAKE_MS
        );
        assert_eq!(
            next_tick_period_ms(
                false,
                true,
                AWAKE_MS,
                ASLEEP_IDLE_TICK_MS,
                ASLEEP_BLINK_TICK_MS
            ),
            AWAKE_MS,
            "awake always wins outright, even if blink_active is somehow true"
        );
    }

    #[test]
    fn asleep_with_no_blink_burst_is_the_slow_idle_period() {
        assert_eq!(
            next_tick_period_ms(
                true,
                false,
                AWAKE_MS,
                ASLEEP_IDLE_TICK_MS,
                ASLEEP_BLINK_TICK_MS
            ),
            ASLEEP_IDLE_TICK_MS
        );
    }

    #[test]
    fn asleep_with_a_live_blink_burst_never_exceeds_the_nyquist_bound() {
        let period = next_tick_period_ms(
            true,
            true,
            AWAKE_MS,
            ASLEEP_IDLE_TICK_MS,
            ASLEEP_BLINK_TICK_MS,
        );
        assert_eq!(period, ASLEEP_BLINK_TICK_MS);
        // BLINK_PHASE_MS = 150 (firmware_core::notification) -> Nyquist
        // ceiling is 75ms. Cross-checked against the real constant, not a
        // hardcoded literal, so a future change to BLINK_PHASE_MS re-derives
        // this bound instead of silently going stale.
        assert!(
            period <= crate::notification::BLINK_PHASE_MS / 2,
            "asleep-blink tick period {period}ms exceeds the Nyquist bound against a {}ms blink \
             phase — the incoming-message blink would stop rendering correctly",
            crate::notification::BLINK_PHASE_MS,
        );
    }
}
