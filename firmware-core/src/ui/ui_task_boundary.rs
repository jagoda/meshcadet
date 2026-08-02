// SPDX-License-Identifier: GPL-3.0-only
//! Pure logic for the dispatcher ↔ `ui_task` queue boundary (ADR-0012,
//! `meshcadet-perf-rearchitecture` M1) — the two invariants the ADR's
//! "Regression-check strategy" leg 5 asks to be host-tested directly:
//!
//! - **C4's change-detector.** The dispatcher holds the last-sent value of
//!   each per-iteration state snapshot (GPS status, room-clock provenance,
//!   battery status, signal level) and sends a `UiEvent` only when it
//!   changes. [`changed_on_send`] is that comparison, generic over any
//!   `PartialEq` payload — `firmware/src/main.rs`'s dispatcher loop calls it
//!   once per snapshot type, with the four real `Copy + PartialEq + Eq`
//!   payload types this module doc cites (`gps::GpsStatus`,
//!   `battery::BatteryStatus`, `signal_tracker::SignalLevel`,
//!   `room_session::ClockSource` + its two companion fields), all of which
//!   already live in this crate.
//! - **C2's drop-and-count overflow policy.** Both directions of the
//!   boundary use `SyncSender::try_send`, which never blocks; a full or
//!   disconnected queue must degrade (drop + count), never panic or silently
//!   vanish without record. [`send_or_count`] is that policy, generic over
//!   `std::sync::mpsc::SyncSender<T>` — real channels, not a fake — so the
//!   full/disconnected paths are exercised against the actual `std` types
//!   `firmware/src/main.rs::send_ui_event` wraps.
//!
//! Both helpers are deliberately generic and hardware-free: `std::sync::mpsc`
//! is available on host same as on `xtensa-esp32s3-espidf` (it's `libstd`,
//! not an `esp-idf-*` peripheral), which is exactly what makes this
//! boundary host-testable at all despite living inside the otherwise
//! detached, cross-compiled `firmware` crate. `firmware/src/main.rs::
//! send_ui_event` and `firmware/src/ui/mod.rs::UiRuntime::try_send_command`
//! are thin, hardware-side callers of these two functions.

use std::sync::mpsc::SyncSender;

/// C4: report whether `new` differs from the last value sent (`*last`), and
/// if so, adopt it as the new last-sent value.
///
/// Returns `true` exactly when the caller should actually send `new` — i.e.
/// on the very first call (`*last` is `None`) or whenever `new != last`.
/// Returns `false` (and leaves `*last` untouched) when `new` is bit-identical
/// to what was last sent, which is the common case: all four real payload
/// types this is called with change roughly once a second while this is
/// evaluated many times a second (every dispatcher-loop iteration).
pub fn changed_on_send<T: PartialEq + Copy>(last: &mut Option<T>, new: T) -> bool {
    if *last == Some(new) {
        return false;
    }
    *last = Some(new);
    true
}

/// C2: `try_send` `value` onto `tx`, without blocking. On success, returns
/// `true`. On failure (the queue is full, or the receiver has been dropped —
/// either because the far side never spawned at all or because it exited),
/// increments `*dropped` and returns `false`. Never panics, never blocks —
/// the two properties C2 exists to guarantee at this boundary.
pub fn send_or_count<T>(tx: &SyncSender<T>, value: T, dropped: &mut u32) -> bool {
    if tx.try_send(value).is_ok() {
        true
    } else {
        *dropped = dropped.saturating_add(1);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    // ── changed_on_send (C4) ─────────────────────────────────────────────

    #[test]
    fn first_call_always_sends_regardless_of_value() {
        let mut last: Option<u32> = None;
        assert!(changed_on_send(&mut last, 0));
        assert_eq!(last, Some(0));
    }

    #[test]
    fn identical_repeat_suppresses() {
        let mut last = Some(42u32);
        assert!(!changed_on_send(&mut last, 42));
        // Untouched — still the same value, not re-adopted.
        assert_eq!(last, Some(42));
    }

    #[test]
    fn a_real_change_sends_and_adopts_the_new_value() {
        let mut last = Some(1u32);
        assert!(changed_on_send(&mut last, 2));
        assert_eq!(last, Some(2));
    }

    #[test]
    fn a_changed_then_unchanged_sequence_sends_exactly_once() {
        let mut last: Option<u32> = None;
        let mut sends = 0;
        for v in [5, 5, 5, 6, 6, 7] {
            if changed_on_send(&mut last, v) {
                sends += 1;
            }
        }
        // 5 (first call), 6 (changed), 7 (changed) — three real transitions,
        // not six calls.
        assert_eq!(sends, 3);
    }

    #[test]
    fn works_over_the_real_gps_status_payload_type() {
        // The exact type ADR-0012 C4 names: Copy + PartialEq + Eq, no
        // esp-idf dependency — see `crate::gps::GpsStatus`.
        let mut last: Option<crate::gps::GpsStatus> = None;
        let never = crate::gps::GpsStatus::never();
        assert!(changed_on_send(&mut last, never));
        assert!(
            !changed_on_send(&mut last, never),
            "identical GpsStatus must suppress"
        );
    }

    #[test]
    fn works_over_the_real_battery_status_payload_type() {
        let mut last: Option<crate::battery::BatteryStatus> = None;
        let unknown = crate::battery::BatteryStatus::unknown();
        assert!(changed_on_send(&mut last, unknown));
        assert!(
            !changed_on_send(&mut last, unknown),
            "identical BatteryStatus must suppress"
        );
    }

    #[test]
    fn works_over_the_real_signal_level_payload_type() {
        let mut last: Option<crate::signal_tracker::SignalLevel> = None;
        let level = crate::signal_tracker::SignalLevel::DirectOnly;
        assert!(changed_on_send(&mut last, level));
        assert!(
            !changed_on_send(&mut last, level),
            "identical SignalLevel must suppress"
        );
    }

    #[test]
    fn works_over_the_real_room_clock_source_payload_type() {
        let mut last: Option<crate::room_session::ClockSource> = None;
        let source = crate::room_session::ClockSource::None;
        assert!(changed_on_send(&mut last, source));
        assert!(
            !changed_on_send(&mut last, source),
            "identical ClockSource must suppress"
        );
    }

    // ── send_or_count (C2) ───────────────────────────────────────────────

    #[test]
    fn sends_and_does_not_count_while_the_queue_has_room() {
        let (tx, rx) = sync_channel::<u32>(2);
        let mut dropped = 0;
        assert!(send_or_count(&tx, 1, &mut dropped));
        assert!(send_or_count(&tx, 2, &mut dropped));
        assert_eq!(dropped, 0);
        assert_eq!(rx.try_recv(), Ok(1));
        assert_eq!(rx.try_recv(), Ok(2));
    }

    #[test]
    fn a_full_queue_drops_and_counts_rather_than_blocking() {
        let (tx, _rx) = sync_channel::<u32>(1);
        let mut dropped = 0;
        assert!(
            send_or_count(&tx, 1, &mut dropped),
            "first send fills the one slot"
        );
        assert!(
            !send_or_count(&tx, 2, &mut dropped),
            "second send must not block — the queue is full"
        );
        assert_eq!(dropped, 1);
        // The first value is still there, untouched by the dropped second one.
        assert_eq!(_rx.try_recv(), Ok(1));
    }

    #[test]
    fn a_disconnected_receiver_drops_and_counts_rather_than_panicking() {
        let (tx, rx) = sync_channel::<u32>(4);
        drop(rx);
        let mut dropped = 0;
        assert!(!send_or_count(&tx, 1, &mut dropped));
        assert_eq!(dropped, 1);
    }

    #[test]
    fn dropped_counter_accumulates_across_multiple_failures() {
        let (tx, _rx) = sync_channel::<u32>(0);
        let mut dropped = 0;
        for v in 0..5u32 {
            send_or_count(&tx, v, &mut dropped);
        }
        // Capacity 0: every send finds the queue "full" (no rendezvous
        // receiver waiting), so all five are dropped and counted.
        assert_eq!(dropped, 5);
    }
}
