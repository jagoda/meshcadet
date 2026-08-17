// SPDX-License-Identifier: GPL-3.0-only
//! Pure Rust-side glue between [`crate::battery::BatteryLevel`] (the coarse
//! bucket landed by `meshcadet-battery-soc-filtering`, data-only until this
//! mission) and the Slint `BatteryIndicator` widget
//! (`firmware/src/ui/battery_indicator.slint`), which — like every other
//! Slint component in this codebase — can only carry a plain `int`
//! property, never a Rust enum. Same one-way, stateless format-conversion
//! shape as `signal_meter::level_to_bars`, kept here so it's host-testable
//! independent of any hardware/Slint dependency.

use crate::battery::BatteryLevel;

/// Convert a [`BatteryLevel`] to the `0..=5` int the Slint `BatteryIndicator`
/// widget's `battery-level` property expects. Matches `BatteryLevel`'s own
/// declaration order 1:1 — `Unknown` -> `0` (outline-only, no reading yet),
/// `Charging` -> `1` (full body, distinct accent color), `Critical`/`Low`/
/// `Medium`/`High` -> `2..=5` (ascending fill).
pub fn level_to_indicator_level(level: BatteryLevel) -> i32 {
    match level {
        BatteryLevel::Unknown => 0,
        BatteryLevel::Charging => 1,
        BatteryLevel::Critical => 2,
        BatteryLevel::Low => 3,
        BatteryLevel::Medium => 4,
        BatteryLevel::High => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bucket_maps_to_its_own_distinct_int() {
        let levels = [
            BatteryLevel::Unknown,
            BatteryLevel::Charging,
            BatteryLevel::Critical,
            BatteryLevel::Low,
            BatteryLevel::Medium,
            BatteryLevel::High,
        ];
        let ints: Vec<i32> = levels
            .iter()
            .map(|&l| level_to_indicator_level(l))
            .collect();
        assert_eq!(ints, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn unknown_maps_to_zero() {
        assert_eq!(level_to_indicator_level(BatteryLevel::Unknown), 0);
    }

    #[test]
    fn high_maps_to_five() {
        assert_eq!(level_to_indicator_level(BatteryLevel::High), 5);
    }
}
