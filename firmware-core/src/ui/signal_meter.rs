// SPDX-License-Identifier: GPL-3.0-only
//! Pure Rust-side glue between the frozen [`crate::signal_tracker::SignalLevel`]
//! contract (ADR-0010) and the Slint `SignalMeter` widget
//! (`firmware/src/ui/signal_meter.slint`), which — like every other Slint
//! component in this codebase — can only carry a plain `int` property, never
//! a Rust enum. This module does NOT touch `signal_tracker.rs` (that
//! contract is frozen — see this crate's own doc table) and adds no new
//! tracking/decay logic of its own; it is a one-way, stateless format
//! conversion, kept here (rather than inline in `firmware/src/ui/mod.rs`) so
//! it is host-testable, matching every other pure UI helper in this module
//! (`contact_list::format_unread_badge`, `gps_status::format_fix_state`, …).

use crate::signal_tracker::SignalLevel;

/// Convert a [`SignalLevel`] to the `0..=5` int the Slint `SignalMeter`
/// widget's `signal-level` property expects: `DirectOnly` -> `0` (renders
/// the direct-only ring), `Bars(n)` -> `n` (renders `n` filled bars).
pub fn level_to_bars(level: SignalLevel) -> i32 {
    match level {
        SignalLevel::DirectOnly => 0,
        SignalLevel::Bars(n) => n as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_only_maps_to_zero() {
        assert_eq!(level_to_bars(SignalLevel::DirectOnly), 0);
    }

    #[test]
    fn bars_map_to_their_own_count() {
        for n in 1..=5u8 {
            assert_eq!(level_to_bars(SignalLevel::Bars(n)), n as i32);
        }
    }
}
