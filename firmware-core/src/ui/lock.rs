// SPDX-License-Identifier: GPL-3.0-only
//! Screen lock — pure idle→lock decision, escalating brute-force backoff,
//! and unlock-attempt state machine.
//!
//! Modelled on [`crate::ui::touch::touch_wake_transition`]: every decision
//! here is a plain function over plain data, with no hardware/Slint/NVS
//! dependency, so it executes under `cargo test --workspace` (`firmware/`
//! is a detached, cross-compiled workspace — see `Cargo.toml`'s doc comment
//! — so a `#[cfg(test)]` block written there would type-check but never
//! run). `firmware/src/ui/mod.rs`'s `UiRuntime::step()` calls these
//! functions and owns only the Slint/hardware plumbing (the lock overlay
//! itself, the actual PIN comparison against stored bytes, and real-clock
//! wiring) — see the screen-lock plan's D-test and D3.
//!
//! # What lives here vs. what doesn't
//!
//! - [`idle_lock_due`] — plan D1's idle→lock decision, off the same
//!   `last_activity_ms` clock the screen-sleep check already uses.
//! - [`boots_locked`] — plan D4's "if the lock is enabled, the device boots
//!   locked" rule, trivial but pinned so a caller can't get it backwards.
//! - [`LockAttemptState`] / [`lockout_seconds_after_failure`] /
//!   [`unlock_attempt_allowed`] / [`attempt_unlock`] — plan D4's in-RAM-only
//!   escalating backoff and the unlock transition itself.
//!
//! **Deliberately NOT here:** the actual PIN comparison. `attempt_unlock`
//! takes `pin_correct: bool` — the caller's own comparison result (e.g.
//! `pin_menu::verify_pin` against the stored lock PIN bytes) — rather than
//! raw PIN bytes, exactly the same separation `touch_wake_transition` keeps
//! from the I2C driver: this module owns the backoff STATE MACHINE, not PIN
//! storage or comparison. That keeps it decoupled from `lock_store`'s
//! on-disk shape and from `pin_menu::MAX_PIN_LEN`/`protocol::provisioning::
//! LOCK_PIN_LEN` entirely.

/// Decide whether the idle-lock should trip right now (plan D1).
///
/// Reads the SAME `last_activity_ms` clock the screen-sleep check uses and
/// trips independently of screen-sleep state (D1: no shared field, no grace
/// period — a tap two seconds after the lock trips must re-enter the PIN).
///
/// Returns `false` immediately if the lock feature is disabled or the
/// device is already locked — mirrors `touch_wake_transition`'s "no-op if
/// already in the target state" arm, so callers don't need to special-case
/// "already locked" themselves before calling this.
pub fn idle_lock_due(
    lock_enabled: bool,
    already_locked: bool,
    now_ms: u64,
    last_activity_ms: u64,
    lock_timeout_s: u16,
) -> bool {
    if !lock_enabled || already_locked {
        return false;
    }
    let timeout_ms = (lock_timeout_s as u64).saturating_mul(1000);
    now_ms.saturating_sub(last_activity_ms) >= timeout_ms
}

/// Whether the device should present itself locked immediately at boot
/// (plan D4: "If the lock is enabled, the device boots locked" — no wipe,
/// no grace window; a reboot is the anti-brute-force property this control
/// actually has, so it must not have a silent bypass).
pub fn boots_locked(lock_enabled: bool) -> bool {
    lock_enabled
}

// ── D4: escalating brute-force backoff ──────────────────────────────────────

/// Attempts 1..=FREE_ATTEMPTS are free — no lockout.
const FREE_ATTEMPTS: u32 = 4;

/// Escalating lockout durations, in seconds, once backoff starts: the 5th
/// consecutive wrong PIN gets `BACKOFF_STEPS_S[0]`, the 6th gets `[1]`, etc.,
/// holding at the LAST (capped) value for every attempt beyond the table's
/// length — this is what "capped at 300 s" means in practice.
const BACKOFF_STEPS_S: [u32; 4] = [30, 60, 120, 300];

/// In-RAM-only wrong-PIN attempt counter (plan D4). Never persisted to NVS —
/// an NVS write on every wrong PIN would be an attacker-controlled flash-
/// write amplifier, and clearing an in-RAM counter already requires a
/// reboot, which already requires the physical access that trivially
/// defeats this control via `reset-lock-pin` over USB (see plan D4's
/// rationale in full).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockAttemptState {
    /// Consecutive wrong-PIN attempts since the last correct PIN (or boot).
    pub consecutive_failures: u32,
}

/// Seconds a wrong PIN must lock out the NEXT attempt for, given the
/// resulting `consecutive_failures` count (i.e. the count AFTER this
/// attempt was scored). `0` means the attempt was free — no escalation has
/// started yet.
pub fn lockout_seconds_after_failure(consecutive_failures: u32) -> u32 {
    if consecutive_failures <= FREE_ATTEMPTS {
        return 0;
    }
    let step = (consecutive_failures - FREE_ATTEMPTS - 1) as usize;
    BACKOFF_STEPS_S[step.min(BACKOFF_STEPS_S.len() - 1)]
}

/// Whether an unlock attempt made at `now_ms` is allowed, given a prior wrong
/// attempt's lockout window (`lockout_started_ms`, `lockout_s` — the value
/// [`attempt_unlock`] returned for that prior attempt). Mirrors
/// `crate::ui::touch::silence_implies_release`'s saturating-elapsed
/// arithmetic (a clock that hasn't advanced, or has wrapped, must never
/// spuriously clear a lockout early). `lockout_s == 0` (no active lockout)
/// is always allowed.
pub fn unlock_attempt_allowed(now_ms: u64, lockout_started_ms: u64, lockout_s: u32) -> bool {
    if lockout_s == 0 {
        return true;
    }
    now_ms.saturating_sub(lockout_started_ms) >= (lockout_s as u64).saturating_mul(1000)
}

/// Result of one [`attempt_unlock`] call: what the lock screen should do
/// with a just-entered PIN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnlockOutcome {
    /// `true` if the PIN was correct — the caller hides the lock overlay
    /// and re-shows the retained underlying screen (plan D3).
    pub unlocked: bool,
    /// Seconds the NEXT attempt must wait before [`unlock_attempt_allowed`]
    /// permits it. `0` if this attempt was free (no escalation yet) or
    /// correct.
    pub lockout_s: u32,
}

/// Pure decision function for one PIN-entry attempt against the lock screen
/// (plan D4). `pin_correct` is the caller's own comparison result (see the
/// module doc for why the comparison itself is deliberately not here).
///
/// A correct PIN resets `state.consecutive_failures` to zero and returns
/// `lockout_s: 0`. A wrong PIN increments the counter and returns the
/// escalating lockout the NEXT attempt must wait out (`0` for attempts 1-4,
/// then 30/60/120/300s-capped from the 5th consecutive wrong PIN).
///
/// Does NOT itself enforce a lockout window — callers gate whether an
/// attempt is even offered to this function using [`unlock_attempt_allowed`]
/// and their own clock, mirroring `touch_wake_transition`'s division of
/// labor between the pure decision and the caller's timing.
pub fn attempt_unlock(pin_correct: bool, state: &mut LockAttemptState) -> UnlockOutcome {
    if pin_correct {
        state.consecutive_failures = 0;
        UnlockOutcome {
            unlocked: true,
            lockout_s: 0,
        }
    } else {
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        UnlockOutcome {
            unlocked: false,
            lockout_s: lockout_seconds_after_failure(state.consecutive_failures),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── idle_lock_due ────────────────────────────────────────────────────

    #[test]
    fn idle_lock_not_due_before_timeout_elapses() {
        assert!(!idle_lock_due(true, false, 1_000, 1_000, 300));
        assert!(!idle_lock_due(true, false, 300_999, 1_000, 300));
    }

    #[test]
    fn idle_lock_due_once_timeout_elapses() {
        assert!(idle_lock_due(true, false, 301_000, 1_000, 300));
        assert!(idle_lock_due(true, false, 999_999, 1_000, 300));
    }

    #[test]
    fn idle_lock_never_due_when_disabled() {
        // Even with the clock arbitrarily far past the timeout, a disabled
        // lock must never trip — D1's "no zero sentinel" lives on
        // lock_flags bit 0, not on this function silently no-op'ing.
        assert!(!idle_lock_due(false, false, 1_000_000, 0, 15));
    }

    #[test]
    fn idle_lock_never_due_when_already_locked() {
        // Mirrors touch_wake_transition's "already awake" no-op arm — a
        // caller doesn't need to special-case "already locked" itself.
        assert!(!idle_lock_due(true, true, 1_000_000, 0, 15));
    }

    #[test]
    fn idle_lock_backwards_clock_never_spuriously_fires() {
        // saturating_sub must not wrap into a huge elapsed value if now_ms
        // is somehow behind last_activity_ms.
        assert!(!idle_lock_due(true, false, 500, 1_000, 300));
    }

    #[test]
    fn idle_lock_trips_at_the_minimum_timeout_bound() {
        // LOCK_TIMEOUT_MIN_S = 15 in the wire contract — exercise it here
        // without importing the constant, to keep this module decoupled.
        assert!(!idle_lock_due(true, false, 14_999, 0, 15));
        assert!(idle_lock_due(true, false, 15_000, 0, 15));
    }

    // ── boots_locked ─────────────────────────────────────────────────────

    #[test]
    fn boots_locked_mirrors_the_enable_flag() {
        assert!(boots_locked(true), "enabled lock must boot locked");
        assert!(!boots_locked(false), "disabled lock must not boot locked");
    }

    // ── lockout_seconds_after_failure — escalation ladder ───────────────

    #[test]
    fn attempts_one_through_four_are_free() {
        for n in 1..=4 {
            assert_eq!(
                lockout_seconds_after_failure(n),
                0,
                "attempt {n} must be free, no lockout yet"
            );
        }
    }

    #[test]
    fn fifth_consecutive_failure_starts_the_30s_lockout() {
        assert_eq!(lockout_seconds_after_failure(5), 30);
    }

    #[test]
    fn escalation_ladder_is_30_60_120_300() {
        assert_eq!(lockout_seconds_after_failure(5), 30);
        assert_eq!(lockout_seconds_after_failure(6), 60);
        assert_eq!(lockout_seconds_after_failure(7), 120);
        assert_eq!(lockout_seconds_after_failure(8), 300);
    }

    #[test]
    fn escalation_caps_at_300s_beyond_the_ladder() {
        assert_eq!(lockout_seconds_after_failure(9), 300);
        assert_eq!(lockout_seconds_after_failure(100), 300);
        assert_eq!(lockout_seconds_after_failure(u32::MAX), 300);
    }

    // ── unlock_attempt_allowed ───────────────────────────────────────────

    #[test]
    fn no_active_lockout_always_allows() {
        assert!(unlock_attempt_allowed(0, 0, 0));
        assert!(unlock_attempt_allowed(999_999, 0, 0));
    }

    #[test]
    fn attempt_blocked_within_the_lockout_window() {
        assert!(!unlock_attempt_allowed(1_000, 1_000, 30));
        assert!(!unlock_attempt_allowed(30_999, 1_000, 30));
    }

    #[test]
    fn attempt_allowed_once_the_lockout_window_elapses() {
        assert!(unlock_attempt_allowed(31_000, 1_000, 30));
        assert!(unlock_attempt_allowed(500_000, 1_000, 30));
    }

    #[test]
    fn unlock_attempt_allowed_backwards_clock_never_spuriously_clears() {
        assert!(!unlock_attempt_allowed(500, 1_000, 30));
    }

    // ── attempt_unlock — the full state machine end to end ──────────────

    #[test]
    fn correct_pin_unlocks_and_resets_counter() {
        let mut state = LockAttemptState {
            consecutive_failures: 3,
        };
        let outcome = attempt_unlock(true, &mut state);
        assert!(outcome.unlocked);
        assert_eq!(outcome.lockout_s, 0);
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn wrong_pin_does_not_unlock_and_increments_counter() {
        let mut state = LockAttemptState::default();
        let outcome = attempt_unlock(false, &mut state);
        assert!(!outcome.unlocked);
        assert_eq!(outcome.lockout_s, 0, "1st wrong attempt is still free");
        assert_eq!(state.consecutive_failures, 1);
    }

    #[test]
    fn four_free_wrong_attempts_then_fifth_escalates() {
        let mut state = LockAttemptState::default();
        for n in 1..=4 {
            let outcome = attempt_unlock(false, &mut state);
            assert_eq!(outcome.lockout_s, 0, "attempt {n} must be free");
        }
        let outcome = attempt_unlock(false, &mut state);
        assert_eq!(outcome.lockout_s, 30, "5th consecutive wrong PIN escalates");
        assert_eq!(state.consecutive_failures, 5);
    }

    #[test]
    fn full_escalation_ladder_via_attempt_unlock() {
        let mut state = LockAttemptState::default();
        let expected = [0, 0, 0, 0, 30, 60, 120, 300, 300];
        for &want in &expected {
            let outcome = attempt_unlock(false, &mut state);
            assert_eq!(outcome.lockout_s, want);
        }
    }

    #[test]
    fn correct_pin_after_escalation_fully_resets_the_ladder() {
        let mut state = LockAttemptState::default();
        for _ in 0..7 {
            attempt_unlock(false, &mut state);
        }
        assert!(lockout_seconds_after_failure(state.consecutive_failures) > 0);

        let outcome = attempt_unlock(true, &mut state);
        assert!(outcome.unlocked);
        assert_eq!(state.consecutive_failures, 0);

        // The very next wrong attempt must be free again — no residual
        // escalation carried across the successful unlock.
        let next = attempt_unlock(false, &mut state);
        assert_eq!(
            next.lockout_s, 0,
            "escalation must fully reset on a correct PIN, not merely pause"
        );
    }
}
