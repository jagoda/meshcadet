// SPDX-License-Identifier: GPL-3.0-only
//! GPS status screen — pure display-string formatting.
//!
//! The `slint::slint!{}` view and the `GpsStatusScreen` Rust wrapper stay in
//! `firmware/src/ui/screens/gps_status.rs` (they depend on Slint); only the
//! plain-data `format_*` helpers below — the part most likely to be gotten
//! subtly wrong — move here so their tests execute under `cargo test
//! --workspace` (this crate is a detached, cross-compiled workspace — see
//! `Cargo.toml`'s doc comment — so `#[cfg(test)]` blocks written there would
//! type-check but never run). See `docs/adr/0005-firmware-core-extraction.md`.

/// Format the fix-state row: three-way status distinguishing a genuinely
/// unresponsive GPS module (`NoSignal`) from one that is alive and searching
/// (`Acquiring`) — see `crate::gps`'s module doc for why this beats a has-fix
/// boolean.
pub fn format_fix_state(state: crate::gps::FixState) -> &'static str {
    use crate::gps::FixState;
    match state {
        FixState::NoSignal => "No signal",
        FixState::Acquiring => "Acquiring\u{2026}",
        FixState::Fix => "Fix acquired",
    }
}

/// Format the satellite-count row: `"<n> satellite(s)"`. Shown regardless of
/// fix state — meaningful acquisition-progress feedback even before a fix.
pub fn format_sat_count(sat_count: u8) -> String {
    if sat_count == 1 {
        "1 satellite".to_string()
    } else {
        format!("{} satellites", sat_count)
    }
}

/// Format the coordinates row: `"<lat>, <lon> (age <n>s)"`, or an em-dash
/// placeholder when no fix has ever been obtained.
///
/// Coordinates are converted from the wire's 1e-7-degree fixed-point
/// representation to decimal degrees for display only (the underlying
/// `lat_e7`/`lon_e7` integers remain the source of truth everywhere else).
pub fn format_coords(has_fix: bool, lat_e7: i32, lon_e7: i32, fix_age_secs: u32) -> String {
    if !has_fix {
        return "\u{2014}".to_string(); // em dash: no fix yet
    }
    let lat_deg = lat_e7 as f64 / 10_000_000.0;
    let lon_deg = lon_e7 as f64 / 10_000_000.0;
    format!("{:.6}, {:.6} (age {}s)", lat_deg, lon_deg, fix_age_secs)
}

/// Format the time-sync row: `"Synced (age <n>s)"` or `"Not synced"`.
pub fn format_time_sync(clock_synced: bool, clock_sync_age_secs: u32) -> String {
    if clock_synced {
        format!("Synced (age {}s)", clock_sync_age_secs)
    } else {
        "Not synced".to_string()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_state_no_signal() {
        assert_eq!(
            format_fix_state(crate::gps::FixState::NoSignal),
            "No signal"
        );
    }

    #[test]
    fn fix_state_acquiring() {
        assert_eq!(
            format_fix_state(crate::gps::FixState::Acquiring),
            "Acquiring\u{2026}"
        );
    }

    #[test]
    fn fix_state_has_fix() {
        assert_eq!(format_fix_state(crate::gps::FixState::Fix), "Fix acquired");
    }

    #[test]
    fn sat_count_zero() {
        assert_eq!(format_sat_count(0), "0 satellites");
    }

    #[test]
    fn sat_count_singular() {
        assert_eq!(format_sat_count(1), "1 satellite");
    }

    #[test]
    fn sat_count_plural() {
        assert_eq!(format_sat_count(8), "8 satellites");
    }

    #[test]
    fn coords_no_fix_is_em_dash() {
        assert_eq!(format_coords(false, 0, 0, 0), "\u{2014}");
    }

    #[test]
    fn coords_with_fix_formats_decimal_degrees_and_age() {
        // 48.1173000°N, 11.5166667°E, age 42s (values from gps.rs's own
        // known-answer GGA test, so the two modules agree on units/scale).
        let s = format_coords(true, 481_173_000, 115_166_667, 42);
        assert_eq!(s, "48.117300, 11.516667 (age 42s)");
    }

    #[test]
    fn coords_negative_hemisphere_formats_with_sign() {
        let s = format_coords(true, -335_100_000, -1_511_200_000, 5);
        assert_eq!(s, "-33.510000, -151.120000 (age 5s)");
    }

    #[test]
    fn time_sync_not_synced() {
        assert_eq!(format_time_sync(false, 0), "Not synced");
    }

    #[test]
    fn time_sync_synced_shows_age() {
        assert_eq!(format_time_sync(true, 300), "Synced (age 300s)");
    }
}
