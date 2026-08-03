// SPDX-License-Identifier: GPL-3.0-only
//! DIO1 wait-strategy abstraction.
//!
//! `firmware/src/radio.rs`'s `transmit` / `try_receive` /
//! `channel_activity_detection` all block until the SX1262's DIO1 line
//! asserts (TxDone / RxDone|CrcErr / CadDone respectively) or a deadline
//! elapses. `firmware/` is xtensa-only (`firmware/.cargo/config.toml` pins
//! `build.target`) and cannot be host-compiled at all — so the *shape* of
//! that wait lives here, in the host-buildable `firmware-core`, purely so a
//! host harness (`perf_loop_model`, `meshcadet-perf-radio-host-validation`)
//! can drive both the legacy spin-poll behaviour and the interrupt/
//! notification-driven replacement through one interface, without linking
//! `esp-idf-hal`.
//!
//! This is a *reporting* abstraction, not a simulation of GPIO electrical
//! behaviour: [`Dio1Wait::wait_high`] is still implemented against a real
//! pin in `firmware/src/radio.rs` (the only place that can read one); what
//! lives here is the trait boundary plus [`Dio1WaitKind`], which lets a host
//! model or test charge the right cost/timing model for whichever variant a
//! `Dio1Wait` impl says it is, and [`quantize_spin_poll_ms`], the exact
//! quantization arithmetic the legacy spin-poll variant is bound by — pulled
//! out as a pure function precisely so both the production spin-poll
//! reference (kept in `firmware/src/radio.rs` behind a `#[cfg(test)]`-only
//! path, see its doc) and a host reference model compute the identical
//! number from the identical formula, instead of two hand-copied constants
//! silently drifting apart.
//!
//! **[`level_triggered_wait`]** (added by `meshcadet-perf-radio-host-
//! validation`) is the same idea applied to the wait's *sequencing*, not
//! just its quantization arithmetic: the exact armed/fired/timed-out/
//! re-arm state machine `GpioDio1Wait::wait_high` runs against real
//! hardware, extracted into a hardware-agnostic reference function and
//! driven on host by a scriptable mock (`level_triggered_wait_tests`,
//! below) — every doc claim `GpioDio1Wait` makes in prose (the re-arm race
//! is self-correcting; a late notification is observed, not lost, and
//! consumed exactly once; the timeout path disarms; **a wake — genuine or
//! stale — never asserts unless the line reads high at the moment of the
//! check**, `GpioDio1Wait`'s "Postcondition" doc) is pinned there as a
//! runnable test, not left as an argument nobody can check.
//! `firmware/src/radio.rs`'s `GpioDio1Wait` is NOT refactored to call this
//! function — it is a proven-equivalent reference model, in the same spirit
//! as `quantize_spin_poll_ms` above, not a live dependency; wiring the two
//! together is out of scope for a host-validation mission that does not
//! touch already-landed, already-reviewed `firmware/` behaviour under the
//! no-HIL constraint (nothing here could be confirmed against real hardware
//! this session regardless).

/// How a [`Dio1Wait`] implementation waits, and its quantization parameters
/// if any. Used by a host model to charge the correct simulated cost for
/// whichever variant a given implementation reports — never consulted by
/// the wait itself, which must behave correctly regardless of what it
/// reports here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dio1WaitKind {
    /// The legacy behaviour this mission removes from production: poll
    /// `is_high()` in a loop, sleeping `tick_ms` between checks
    /// (`FreeRtos::delay_ms(1)` in the removed code). Quantizes every wait
    /// up to `tick_ms`, and burns one scheduler slot per tick for the whole
    /// wait — the two costs [`quantize_spin_poll_ms`] and the module doc
    /// above both refer to.
    SpinPoll { tick_ms: u32 },
    /// Interrupt/notification-driven: the calling task blocks on an
    /// ISR-signalled FreeRTOS task notification and wakes with no polling
    /// quantization once DIO1 asserts (or the deadline elapses, likewise
    /// with no quantization on the timeout side).
    Notify,
}

/// Outcome of a single [`Dio1Wait::wait_high`] call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dio1WaitOutcome {
    /// DIO1 asserted before the deadline.
    Asserted,
    /// The deadline elapsed with DIO1 never asserting.
    TimedOut,
}

/// A DIO1 wait strategy: block until DIO1 reads high, or `timeout_ms`
/// elapses.
///
/// Implemented against real hardware exactly once, in
/// `firmware/src/radio.rs`'s `GpioDio1Wait` (GPIO ISR subscribe + a
/// FreeRTOS task notification). This trait exists so that implementation is
/// swappable, at the call sites in `transmit`/`try_receive`/
/// `channel_activity_detection`, for a host-side mock implementing the same
/// interface without linking `esp-idf-hal` — see this module's doc.
pub trait Dio1Wait {
    /// Which strategy this is, and its quantization parameters if any.
    fn kind(&self) -> Dio1WaitKind;

    /// Block (or, in a host mock, simulate blocking) until DIO1 reads high,
    /// or `timeout_ms` elapses.
    fn wait_high(&mut self, timeout_ms: u32) -> Dio1WaitOutcome;
}

/// Given that DIO1 would actually assert `edge_at_ms` milliseconds after a
/// spin-poll of `tick_ms`-spaced `is_high()` checks starts, return the
/// wall-clock delay (in ms) until the FIRST post-edge check observes it —
/// i.e. the quantization the legacy `FreeRtos::delay_ms(tick_ms)` spin-poll
/// adds on top of the real edge time. A poll happens at every multiple of
/// `tick_ms` (0, `tick_ms`, `2*tick_ms`, ...); the observed time is the
/// smallest such multiple that is `>= edge_at_ms`.
///
/// `tick_ms == 0` is treated as "poll continuously" (no quantization),
/// returning `edge_at_ms` unchanged, so a caller cannot divide by zero by
/// constructing a degenerate `SpinPoll { tick_ms: 0 }`.
pub fn quantize_spin_poll_ms(edge_at_ms: u32, tick_ms: u32) -> u32 {
    if tick_ms == 0 {
        return edge_at_ms;
    }
    edge_at_ms.div_ceil(tick_ms).saturating_mul(tick_ms)
}

// ── Level-triggered wait state machine (host-testable reference) ──────────

/// The two hardware-adjacent primitives [`level_triggered_wait`] composes,
/// abstracted so the exact sequencing `firmware/src/radio.rs`'s
/// `GpioDio1Wait::wait_high` follows (fast-path level check, arm, block on
/// notify-or-timeout, disarm-on-timeout) can be driven — and every ordering
/// of edges/timeouts/arm-failures/stale-notifications exercised — by a
/// scriptable mock on host, without linking `esp-idf-hal` at all. Mirrors
/// `PinDriver`'s `is_high`/`set_interrupt_type`+`enable_interrupt`/
/// `disable_interrupt` exactly (`esp-idf-hal` 0.46.2 `src/gpio.rs`).
/// Marker error for [`LevelTriggeredLine::arm`] failing — a unit type
/// rather than a bare `Result<(), ()>` per `clippy::result_unit_err`; there
/// is exactly one way this trait's `arm` can fail (the underlying
/// arm-the-interrupt call errored) so no payload is needed, only the
/// `Result` shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArmFailed;

pub trait LevelTriggeredLine {
    /// Current level of the underlying line — the fast path. Must be
    /// re-read fresh on every call, never cached: this is what makes
    /// "arming a level-triggered interrupt after the level already went
    /// high still observes it" (`GpioDio1Wait`'s "Why level-triggered, not
    /// edge-triggered" doc) hold for the SOFTWARE side of the wait too, not
    /// just the GPIO hardware's own re-evaluation.
    fn is_high(&mut self) -> bool;
    /// Arm the interrupt. `Err` models `set_interrupt_type`/
    /// `enable_interrupt` failing (real ISR-service exhaustion) — the
    /// production fallback path this abstraction deliberately does NOT
    /// re-implement (that is `quantize_spin_poll_ms`'s job, exercised
    /// separately above); a caller of [`level_triggered_wait`] that gets
    /// [`LevelTriggeredOutcome::ArmFailed`] decides what to do next, the
    /// same way `GpioDio1Wait::wait_high` falls back to
    /// `spin_poll_fallback`.
    fn arm(&mut self) -> Result<(), ArmFailed>;
    /// Disarm the interrupt (`gpio_isr_handler_remove`-equivalent). Called
    /// exactly once, on the timeout path — see [`level_triggered_wait`]'s
    /// doc for why arming stays intentionally un-disarmed on the Asserted
    /// path (matches `GpioDio1Wait::wait_high`: no `disable_interrupt` call
    /// on that branch).
    fn disarm(&mut self);
}

/// The one-shot wake primitive [`level_triggered_wait`] blocks on once
/// armed — abstracts `esp_idf_hal::task::notification::Notification::wait`,
/// whose real semantics (FreeRTOS `xTaskGenericNotifyWait`, `esp-idf-hal`
/// 0.46.2 `src/task.rs:127-138`) are exactly what a conforming mock must
/// reproduce for the tests below to mean anything: **a notification posted
/// at ANY time before this call is made is observed immediately** (never
/// lost, never requiring the poster and the waiter to overlap in time —
/// `xTaskGenericNotifyFromISR`'s `eSetBits` sets the "received" state
/// unconditionally, `GpioDio1Wait`'s "Lost-wakeup semantics" doc), and
/// **exactly one call consumes it** (the state resets to "not received" on
/// every successful return, so a second, immediately-following call without
/// an intervening new notification genuinely blocks/times out rather than
/// spuriously re-firing).
pub trait OneShotNotify {
    /// Block (or, in a host mock, simulate blocking) up to `timeout_ms`;
    /// `true` if a notification was already pending OR arrived before the
    /// deadline, `false` on timeout.
    fn wait(&mut self, timeout_ms: u32) -> bool;
}

/// Outcome of [`level_triggered_wait`] — a superset of [`Dio1WaitOutcome`]
/// that also surfaces the arm-failed case a real caller must fall back on
/// (see [`LevelTriggeredLine::arm`]'s doc), rather than folding it into
/// [`Dio1WaitOutcome::TimedOut`] and hiding the distinction from a test or a
/// caller that needs to react differently to the two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelTriggeredOutcome {
    Asserted,
    TimedOut,
    ArmFailed,
}

/// The exact sequencing `firmware/src/radio.rs`'s `GpioDio1Wait::wait_high`
/// follows, extracted into a hardware-agnostic reference so it can be
/// exercised on host against a scriptable [`LevelTriggeredLine`] +
/// [`OneShotNotify`] mock — this is what lets
/// `meshcadet-perf-radio-host-validation` host-test the armed / fired /
/// timed-out / spurious-wake / re-arm-ordering state machine `cargo test
/// --workspace` runs, without linking `esp-idf-hal` (`firmware/` is
/// xtensa-only and cannot be host-compiled at all — same constraint this
/// module's own top doc states).
///
/// 1. **Fast path**: if the line already reads high, return `Asserted`
///    immediately — no arm, no wait. This is the self-correcting half of
///    the re-arm-race argument: an edge that already landed before this
///    call began is still observed.
/// 2. **Arm**: if arming fails, return `ArmFailed` (caller's fallback
///    responsibility — see [`LevelTriggeredLine::arm`]'s doc).
/// 3. **Wait, then re-check**: block on the notification. A notification is
///    a HINT that something happened, not a snapshot of the line right now
///    — [`OneShotNotify`]'s sticky, exactly-once-consumed state can wake
///    this call on a notification left over from an earlier,
///    already-serviced assertion whose line has since gone low again (see
///    this module's doc and `GpioDio1Wait`'s "Lost-wakeup semantics"). So
///    every wake — genuine or stale — is followed by re-reading the line
///    before trusting it:
///    - Line reads high: `Asserted`. The interrupt is deliberately left
///      armed (matches `GpioDio1Wait::wait_high`: no `disarm()` call on
///      this branch — an already-fired level-triggered interrupt has
///      nothing further to protect against until the caller explicitly
///      re-arms via its NEXT `wait_high`-equivalent call).
///    - Line still reads low (the notification was stale): loop back and
///      wait again. The notification has already been consumed
///      (exactly-once), so this genuinely blocks for a fresh edge rather
///      than spinning.
///    - `notify.wait` itself times out: `disarm()`, then `TimedOut` — the
///      timeout-path doc's "so a later, unrelated fire doesn't land on a
///      wait no one issued".
///
/// **Postcondition**: `Asserted` is returned only when the line reads high
/// at the moment of the check — never merely because a notification fired.
/// This mirrors `GpioDio1Wait::wait_high`'s own "Postcondition" doc; unlike
/// the production wait, this reference model has no clock primitive to
/// shrink `timeout_ms` across re-waits (each `notify.wait` call below is
/// still passed the caller's original `timeout_ms`) — deadline-honouring is
/// exercised on the real implementation, not here; what this model pins is
/// the *sequencing*, i.e. that a stale wake does not end the wait early.
pub fn level_triggered_wait(
    line: &mut impl LevelTriggeredLine,
    notify: &mut impl OneShotNotify,
    timeout_ms: u32,
) -> LevelTriggeredOutcome {
    if line.is_high() {
        return LevelTriggeredOutcome::Asserted;
    }
    if line.arm().is_err() {
        return LevelTriggeredOutcome::ArmFailed;
    }
    loop {
        if !notify.wait(timeout_ms) {
            line.disarm();
            return LevelTriggeredOutcome::TimedOut;
        }
        // A notification is a hint, not a snapshot of the line — re-check
        // before trusting it (see this function's doc).
        if line.is_high() {
            return LevelTriggeredOutcome::Asserted;
        }
        // Stale: already consumed, loop back and genuinely wait again.
    }
}

#[cfg(test)]
mod level_triggered_wait_tests {
    use super::*;
    use std::collections::VecDeque;

    /// A scripted [`LevelTriggeredLine`] — call counts recorded so a test
    /// can pin exactly which operations ran, not just the final outcome.
    struct ScriptedLine {
        /// One entry consumed per `is_high()` call; the last entry repeats
        /// once exhausted (so a test can leave a steady-state level without
        /// scripting every call).
        levels: VecDeque<bool>,
        last_level: bool,
        arm_result: Result<(), ArmFailed>,
        arm_calls: u32,
        disarm_calls: u32,
    }

    impl ScriptedLine {
        fn always_low() -> Self {
            Self {
                levels: VecDeque::new(),
                last_level: false,
                arm_result: Ok(()),
                arm_calls: 0,
                disarm_calls: 0,
            }
        }

        fn with_levels(levels: impl IntoIterator<Item = bool>) -> Self {
            Self {
                levels: levels.into_iter().collect(),
                ..Self::always_low()
            }
        }

        fn arm_always_fails(mut self) -> Self {
            self.arm_result = Err(ArmFailed);
            self
        }
    }

    impl LevelTriggeredLine for ScriptedLine {
        fn is_high(&mut self) -> bool {
            self.last_level = self.levels.pop_front().unwrap_or(self.last_level);
            self.last_level
        }

        fn arm(&mut self) -> Result<(), ArmFailed> {
            self.arm_calls += 1;
            self.arm_result
        }

        fn disarm(&mut self) {
            self.disarm_calls += 1;
        }
    }

    /// A scripted [`OneShotNotify`] — one queued outcome per call, ALSO
    /// modelling the real primitive's "a notification posted before this
    /// call is still observed" property when `pending` is pre-seeded true
    /// (see [`OneShotNotify`]'s own doc).
    #[derive(Default)]
    struct ScriptedNotify {
        /// If `true`, the very next `wait()` call returns `true`
        /// immediately (models a notification already posted — e.g. a late
        /// ISR fire from a prior, already-abandoned wait cycle) and clears
        /// itself (exactly-once consumption, per the real primitive).
        pending: bool,
        /// Consulted only when `pending` is false: `true` = a fresh
        /// notification arrives before the deadline, `false` = genuine
        /// timeout.
        arrives_in_time: VecDeque<bool>,
        wait_calls: u32,
    }

    impl OneShotNotify for ScriptedNotify {
        fn wait(&mut self, _timeout_ms: u32) -> bool {
            self.wait_calls += 1;
            if self.pending {
                self.pending = false;
                return true;
            }
            self.arrives_in_time.pop_front().unwrap_or(false)
        }
    }

    // ── armed / fired ────────────────────────────────────────────────────

    #[test]
    fn fast_path_asserts_without_arming_when_already_high() {
        let mut line = ScriptedLine::with_levels([true]);
        let mut notify = ScriptedNotify::default();
        let outcome = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(outcome, LevelTriggeredOutcome::Asserted);
        assert_eq!(line.arm_calls, 0, "fast path must not arm");
        assert_eq!(notify.wait_calls, 0, "fast path must not wait");
    }

    #[test]
    fn armed_then_fired_within_deadline_asserts() {
        // The line goes high at the same moment the genuine edge fires the
        // ISR (that is what a level-triggered interrupt IS): scripted as a
        // second `is_high()` entry so the post-notify re-check observes it.
        let mut line = ScriptedLine::with_levels([false, true]);
        let mut notify = ScriptedNotify {
            arrives_in_time: VecDeque::from([true]),
            ..Default::default()
        };
        let outcome = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(outcome, LevelTriggeredOutcome::Asserted);
        assert_eq!(line.arm_calls, 1);
        assert_eq!(
            line.disarm_calls, 0,
            "the Asserted path must not disarm — matches GpioDio1Wait::wait_high"
        );
    }

    // ── timed-out ────────────────────────────────────────────────────────

    #[test]
    fn armed_then_no_edge_before_deadline_times_out_and_disarms() {
        let mut line = ScriptedLine::always_low();
        let mut notify = ScriptedNotify {
            arrives_in_time: VecDeque::from([false]),
            ..Default::default()
        };
        let outcome = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(outcome, LevelTriggeredOutcome::TimedOut);
        assert_eq!(line.arm_calls, 1);
        assert_eq!(
            line.disarm_calls, 1,
            "the timeout path must disarm exactly once — \"Timeout path\" doc"
        );
    }

    #[test]
    fn arm_failure_is_reported_distinctly_from_a_timeout() {
        // A real caller (`GpioDio1Wait::wait_high`) falls back to a spin
        // poll on THIS outcome specifically, not on a genuine timeout —
        // collapsing the two would silently break that branch.
        let mut line = ScriptedLine::always_low().arm_always_fails();
        let mut notify = ScriptedNotify::default();
        let outcome = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(outcome, LevelTriggeredOutcome::ArmFailed);
        assert_eq!(notify.wait_calls, 0, "must not wait after a failed arm");
    }

    // ── re-arm ordering ──────────────────────────────────────────────────

    #[test]
    fn re_arm_after_a_timeout_arms_again_on_the_next_call() {
        // Third scripted level is the genuine edge's own `is_high()`
        // observation, backing the second call's post-notify re-check.
        let mut line = ScriptedLine::with_levels([false, false, true]);
        let mut notify = ScriptedNotify {
            arrives_in_time: VecDeque::from([false, true]),
            ..Default::default()
        };
        let first = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(first, LevelTriggeredOutcome::TimedOut);
        let second = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(second, LevelTriggeredOutcome::Asserted);
        assert_eq!(
            line.arm_calls, 2,
            "each call must arm fresh — a timed-out wait must not leave a stale \
             armed state the next call silently relies on"
        );
    }

    #[test]
    fn a_level_that_asserts_between_calls_is_caught_by_the_next_calls_fast_path() {
        // The re-arm-race question `GpioDio1Wait`'s doc answers in prose,
        // pinned here as a state-machine test: an edge that lands in the
        // GAP between one call returning TimedOut and the next call
        // beginning is still observed — via the FAST PATH specifically
        // (`arm_calls` stays at 1, from the first call only), not because
        // the second call got lucky racing a fresh arm.
        let mut line = ScriptedLine::with_levels([false, true]);
        let mut notify = ScriptedNotify {
            arrives_in_time: VecDeque::from([false]),
            ..Default::default()
        };
        let first = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(first, LevelTriggeredOutcome::TimedOut);
        let second = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(second, LevelTriggeredOutcome::Asserted);
        assert_eq!(
            line.arm_calls, 1,
            "the second call's fast path must short-circuit before arming again"
        );
    }

    // ── spurious/stale wake (a wake is a hint, not a snapshot of the line —
    //    but a wake that IS still live is observed, not lost) ─────────────

    #[test]
    fn a_stale_pending_notification_with_the_line_low_keeps_waiting_and_can_time_out() {
        // The defect this pins the fix for: a notification pending from
        // BEFORE this call began (`ScriptedNotify::pending`) models a
        // FreeRTOS task notification left over from an earlier,
        // already-serviced-and-cleared DIO1 assertion — see
        // `GpioDio1Wait`'s "Lost-wakeup semantics" doc for how that
        // notification survives. If the line reads low when this call
        // checks it, that notification is stale and must NOT be reported
        // as `Asserted` — the wait must keep waiting for a genuine edge
        // instead (`GpioDio1Wait`'s "Postcondition" doc: `Asserted` ⇒ DIO1
        // is asserted right now).
        let mut line = ScriptedLine::always_low();
        let mut notify = ScriptedNotify {
            pending: true,
            arrives_in_time: VecDeque::from([false]),
            ..Default::default()
        };
        let outcome = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(outcome, LevelTriggeredOutcome::TimedOut);
        assert_eq!(
            notify.wait_calls, 2,
            "the stale pending notification must be consumed, then a fresh wait issued"
        );
        assert_eq!(
            line.disarm_calls, 1,
            "the eventual genuine timeout must still disarm"
        );
    }

    #[test]
    fn a_stale_pending_notification_does_not_prevent_a_later_genuine_edge_asserting() {
        // Same stale start as above, but a genuine edge follows within the
        // same call: the first, stale wake must not have ended the wait
        // permanently.
        let mut line = ScriptedLine::with_levels([false, false, true]);
        let mut notify = ScriptedNotify {
            pending: true,
            arrives_in_time: VecDeque::from([true]),
            ..Default::default()
        };
        let outcome = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(outcome, LevelTriggeredOutcome::Asserted);
        assert_eq!(notify.wait_calls, 2);
    }

    #[test]
    fn a_pending_notification_whose_level_is_still_high_asserts_immediately() {
        // The genuine lost-wakeup property, preserved: an ISR that fired
        // between the previous call ending and this one beginning, whose
        // assertion is STILL live (line reads high right now), must be
        // reported `Asserted` on this call's first wake — not require a
        // second, redundant edge. This is the case `GpioDio1Wait`'s
        // "Lost-wakeup semantics" doc names directly.
        let mut line = ScriptedLine::with_levels([false, true]);
        let mut notify = ScriptedNotify {
            pending: true,
            ..Default::default()
        };
        let outcome = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(outcome, LevelTriggeredOutcome::Asserted);
        assert_eq!(
            notify.wait_calls, 1,
            "a still-high pending notification must assert on the first wait, not require a second"
        );
        assert_eq!(
            line.arm_calls, 1,
            "must still arm before consulting the notifier"
        );
    }

    #[test]
    fn a_consumed_notification_does_not_bleed_into_a_second_unrelated_wait() {
        // The other half of "observed, not lost" — EXACTLY once. If the
        // mock's exactly-once consumption (`ScriptedNotify::wait`'s
        // `pending = false` reset, modelling the real primitive's state
        // reset on every successful receipt — see `OneShotNotify`'s doc)
        // ever regressed to "sticky forever", this second, unrelated call
        // (no new edge scripted, line back to low) would incorrectly also
        // report Asserted.
        let mut line = ScriptedLine::with_levels([false, true, false]);
        let mut notify = ScriptedNotify {
            pending: true,
            arrives_in_time: VecDeque::from([false]),
            ..Default::default()
        };
        let first = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(first, LevelTriggeredOutcome::Asserted);
        let second = level_triggered_wait(&mut line, &mut notify, 20);
        assert_eq!(
            second,
            LevelTriggeredOutcome::TimedOut,
            "a notification already consumed by the first call must not also \
             satisfy a second, later call with no new edge"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_rounds_up_to_next_tick() {
        // Edge lands mid-tick: observed at the next tick boundary, not the
        // edge time itself — this is exactly the up-to-1-tick quantization
        // the campaign plan measured against the shipped spin-poll.
        assert_eq!(quantize_spin_poll_ms(1, 1), 1);
        assert_eq!(quantize_spin_poll_ms(3, 5), 5);
        assert_eq!(quantize_spin_poll_ms(5, 5), 5); // exact multiple: no extra wait
        assert_eq!(quantize_spin_poll_ms(6, 5), 10);
    }

    #[test]
    fn quantize_zero_edge_is_observed_at_zero() {
        assert_eq!(quantize_spin_poll_ms(0, 5), 0);
    }

    #[test]
    fn quantize_zero_tick_is_a_continuous_poll_noop() {
        // Degenerate input must not panic (division by zero) and must not
        // silently invent quantization for a strategy that has none.
        assert_eq!(quantize_spin_poll_ms(7, 0), 7);
        assert_eq!(quantize_spin_poll_ms(0, 0), 0);
    }

    /// A trivial host mock proving the trait boundary this module exists
    /// for: something with zero hardware dependency can stand in for
    /// `GpioDio1Wait` and be driven identically at both call sites.
    struct ScriptedWait {
        kind: Dio1WaitKind,
        edge_at_ms: Option<u32>,
    }

    impl Dio1Wait for ScriptedWait {
        fn kind(&self) -> Dio1WaitKind {
            self.kind
        }

        fn wait_high(&mut self, timeout_ms: u32) -> Dio1WaitOutcome {
            match self.edge_at_ms {
                Some(edge) if edge <= timeout_ms => Dio1WaitOutcome::Asserted,
                _ => Dio1WaitOutcome::TimedOut,
            }
        }
    }

    #[test]
    fn scripted_wait_reports_asserted_within_deadline() {
        let mut w = ScriptedWait {
            kind: Dio1WaitKind::Notify,
            edge_at_ms: Some(10),
        };
        assert_eq!(w.wait_high(20), Dio1WaitOutcome::Asserted);
    }

    #[test]
    fn scripted_wait_reports_timeout_past_deadline() {
        let mut w = ScriptedWait {
            kind: Dio1WaitKind::SpinPoll { tick_ms: 1 },
            edge_at_ms: Some(30),
        };
        assert_eq!(w.wait_high(20), Dio1WaitOutcome::TimedOut);
    }
}
