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

/// Convert a [`BatteryLevel`] to the `0..=4` int the Slint `BatteryIndicator`
/// widget's `battery-level` property expects. Matches `BatteryLevel`'s own
/// declaration order 1:1 — `Unknown` -> `0` (outline-only, no reading yet),
/// `Charging` -> `1` (full body, distinct accent color), `Low`/`Partial`/
/// `Full` -> `2..=4` (ascending fill). As of 2026-08-22
/// (`meshcadet-battery-three-state-pipeline`) this is a 5-state range, not
/// the prior 6-state (`0..=5`) one — `BatteryLevel` dropped from 4
/// percent-domain buckets to 3 voltage-domain ones.
pub fn level_to_indicator_level(level: BatteryLevel) -> i32 {
    match level {
        BatteryLevel::Unknown => 0,
        BatteryLevel::Charging => 1,
        BatteryLevel::Low => 2,
        BatteryLevel::Partial => 3,
        BatteryLevel::Full => 4,
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
            BatteryLevel::Low,
            BatteryLevel::Partial,
            BatteryLevel::Full,
        ];
        let ints: Vec<i32> = levels
            .iter()
            .map(|&l| level_to_indicator_level(l))
            .collect();
        assert_eq!(ints, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn unknown_maps_to_zero() {
        assert_eq!(level_to_indicator_level(BatteryLevel::Unknown), 0);
    }

    #[test]
    fn full_maps_to_four() {
        assert_eq!(level_to_indicator_level(BatteryLevel::Full), 4);
    }
}
