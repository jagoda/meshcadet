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
