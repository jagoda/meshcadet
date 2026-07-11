// SPDX-License-Identifier: GPL-3.0-only
//! GT911 touch controller — pure release-by-silence debounce decision, the
//! plain-data gesture-kind enum, and the screen-sleep wake/swallow state
//! machine.
//!
//! The GT911 I2C driver (`TouchDriver`, plus the `TouchPoint`/`TouchEvent`
//! types it hands back) stays in `firmware/src/ui/touch.rs` — it owns real
//! hardware I/O; `TouchKind` moves here (plain data, no Slint/I2C
//! dependency) alongside the pure predicates that operate on it, so their
//! tests execute under `cargo test --workspace` (this crate is a detached,
//! cross-compiled workspace — see `Cargo.toml`'s doc comment — so a
//! `#[cfg(test)]` block written there would type-check but never run).
//! `firmware/src/ui/touch.rs` re-exports `TouchKind` via
//! `pub use firmware_core::ui::touch::TouchKind;` so every existing call
//! site (`TouchEvent { kind: TouchKind::Pressed, .. }`, etc.) resolves
//! unchanged. See `docs/adr/0005-firmware-core-extraction.md`.

/// Touch gesture kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchKind {
    /// Finger pressed down.
    Pressed,
    /// Finger moved while pressed.
    Moved,
    /// Finger lifted.
    Released,
}

/// Pure decision function for the release-by-silence debounce in
/// `TouchDriver::poll_event`'s "buffer not ready" arm — extracted so this
/// regression-causing logic is covered by a host-runnable unit test
/// independent of the I2C/hardware stack, same rationale as
/// [`touch_wake_transition`] below.
///
/// Returns `true` once `now_ms` is at least `debounce_ms` past
/// `last_update_ms` (saturating, so a clock that hasn't advanced — or has
/// wrapped — never spuriously asserts a release).
pub fn silence_implies_release(now_ms: u64, last_update_ms: u64, debounce_ms: u64) -> bool {
    now_ms.saturating_sub(last_update_ms) >= debounce_ms
}

/// Result of [`touch_wake_transition`]: what `UiRuntime::step()` should do
/// with one polled touch event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TouchWakeOutcome {
    /// `true` if this event is the one that woke the screen (backlight
    /// should turn on) — `step()` calls `wake_screen` when this is set.
    pub woke: bool,
    /// `true` if this event should be forwarded to `window.dispatch_touch`.
    /// Mutually exclusive with `woke` and with mid-gesture swallow.
    pub dispatch: bool,
    /// New value for `UiRuntime::touch_wake_swallow_active`.
    pub swallow_active: bool,
}

/// Pure decision function for the touch wake/swallow state machine.
///
/// This is the technically sharp part of the screen-sleep feature: the
/// wake-triggering touch must be consumed to wake ONLY, never routed to the
/// focused widget, and a still-held finger must not leak the rest of its
/// Pressed→Moved→Released gesture into the app after the initiating Pressed
/// was swallowed. Extracted as a pure function (no hardware/Slint
/// dependency) so this invariant has host-checkable unit tests instead of
/// relying solely on the manual HIL procedure.
///
/// - `screen_asleep`: state BEFORE this event (i.e. before any wake this call causes).
/// - `swallow_active`: whether a previous call is still draining a wake gesture's tail.
/// - `kind`: the polled event's `TouchKind`.
pub fn touch_wake_transition(
    screen_asleep: bool,
    swallow_active: bool,
    kind: TouchKind,
) -> TouchWakeOutcome {
    if screen_asleep {
        // This event wakes the screen. Swallow it; if it's not already the
        // gesture's Released, keep swallowing until one arrives.
        TouchWakeOutcome {
            woke: true,
            dispatch: false,
            swallow_active: kind != TouchKind::Released,
        }
    } else if swallow_active {
        // Draining the wake gesture's Moved/Released tail — still swallowed.
        TouchWakeOutcome {
            woke: false,
            dispatch: false,
            swallow_active: kind != TouchKind::Released,
        }
    } else {
        // Normal operation: screen already awake, no gesture to drain.
        TouchWakeOutcome {
            woke: false,
            dispatch: true,
            swallow_active: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── silence_implies_release ─────────────────────────────────────────────
    // Pure debounce arithmetic — see the function's doc for why isolating it
    // from the I2C/hardware stack matters here.

    #[test]
    fn silence_implies_release_false_within_debounce_window() {
        // This is the exact regression scenario: back-to-back polls within
        // the same `step()`'s drain loop are microseconds apart, i.e.
        // `now_ms == last_update_ms` — must NOT infer a release.
        assert!(!silence_implies_release(1_000, 1_000, 40));
        assert!(!silence_implies_release(1_020, 1_000, 40));
    }

    #[test]
    fn silence_implies_release_true_once_debounce_elapsed() {
        assert!(silence_implies_release(1_040, 1_000, 40));
        assert!(silence_implies_release(5_000, 1_000, 40));
    }

    #[test]
    fn silence_implies_release_never_fires_on_backwards_clock() {
        // `saturating_sub` must not wrap into a huge elapsed value if
        // `now_ms` is somehow behind `last_update_ms`.
        assert!(!silence_implies_release(500, 1_000, 40));
    }

    // ── touch_wake_transition ─────────────────────────────────────────────
    //
    // Acceptance-critical: the central invariant is that the
    // wake-triggering input is swallowed globally and never reaches the
    // focused widget. These pin the state machine driving that invariant.

    #[test]
    fn asleep_pressed_wakes_and_swallows_without_dispatch() {
        let o = touch_wake_transition(true, false, TouchKind::Pressed);
        assert!(o.woke, "a Pressed while asleep must wake the screen");
        assert!(
            !o.dispatch,
            "the wake-triggering Pressed must NOT reach the focused widget"
        );
        assert!(
            o.swallow_active,
            "must keep swallowing until the matching Released"
        );
    }

    #[test]
    fn asleep_released_wakes_and_does_not_leave_swallow_active() {
        // Defensive case: shouldn't happen in practice (sleep can't engage
        // mid-gesture — see step()'s inactivity check — but a Released alone
        // must still wake+swallow, not dispatch, and not get stuck swallowing
        // forever waiting for a Released that already happened).
        let o = touch_wake_transition(true, false, TouchKind::Released);
        assert!(o.woke);
        assert!(!o.dispatch);
        assert!(!o.swallow_active);
    }

    #[test]
    fn wake_gesture_moved_tail_still_swallowed() {
        // After the initiating Pressed woke the screen, a held finger's
        // Moved samples must keep being swallowed, not dispatched.
        let o = touch_wake_transition(false, true, TouchKind::Moved);
        assert!(!o.woke, "already awake — this is not a second wake");
        assert!(!o.dispatch, "still draining the wake gesture's tail");
        assert!(o.swallow_active, "Moved does not end the gesture");
    }

    #[test]
    fn wake_gesture_released_ends_swallow() {
        let o = touch_wake_transition(false, true, TouchKind::Released);
        assert!(!o.woke);
        assert!(
            !o.dispatch,
            "the wake gesture's own Released must not dispatch either"
        );
        assert!(!o.swallow_active, "Released ends the swallowed gesture");
    }

    #[test]
    fn normal_operation_dispatches_every_kind() {
        // Screen already awake, no wake gesture in flight: every event kind
        // dispatches normally — this is the ordinary, un-swallowed path that
        // must not regress for existing touch interactions.
        for kind in [TouchKind::Pressed, TouchKind::Moved, TouchKind::Released] {
            let o = touch_wake_transition(false, false, kind);
            assert!(!o.woke);
            assert!(
                o.dispatch,
                "{:?} must dispatch during normal operation",
                kind
            );
            assert!(!o.swallow_active);
        }
    }

    #[test]
    fn a_full_wake_gesture_never_dispatches_any_event() {
        // End-to-end simulation of one physical tap that wakes the screen:
        // Pressed (wakes) -> Moved -> Released, driven through the state
        // machine exactly as step() would sequence it. NOT ONE event in this
        // gesture may reach `dispatch` — that is the whole point of the
        // swallow invariant.
        let mut asleep = true;
        let mut swallow = false;
        let mut any_dispatched = false;
        let mut woke_count = 0;
        for kind in [TouchKind::Pressed, TouchKind::Moved, TouchKind::Released] {
            let o = touch_wake_transition(asleep, swallow, kind);
            if o.woke {
                woke_count += 1;
            }
            if o.dispatch {
                any_dispatched = true;
            }
            swallow = o.swallow_active;
            asleep = false; // step() always clears asleep after processing any event
        }
        assert_eq!(woke_count, 1, "exactly one wake for the whole gesture");
        assert!(
            !any_dispatched,
            "no event in the waking gesture may reach the focused widget"
        );
        assert!(
            !swallow,
            "swallow must have cleared by the gesture's Released"
        );
    }
}
