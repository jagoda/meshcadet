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

/// Format the time-sync row: the actual GPS-synced wall-clock date+time plus
/// how long ago the sync happened — `"2026-07-15 14:32:10 UTC (synced 5s
/// ago)"` — or `"Not synced"` when the system clock has never been set from
/// a GPS fix since boot (this device has no battery-backed RTC; see
/// `firmware::gps`'s module doc).
///
/// `clock_unix_secs` is the CURRENT synced wall-clock time (ticks forward
/// every call as long as sync holds — see
/// [`crate::gps::synced_wall_clock_secs`]), not the time sync last occurred;
/// `clock_sync_age_secs` is how many seconds ago that sync happened. Passing
/// `None` for `clock_unix_secs` always renders "Not synced", regardless of
/// `clock_sync_age_secs` — the two must agree (both `None`/`0` or both
/// populated), which is exactly what `GpsStatus`'s single `clock_synced`
/// flag already guarantees for both fields.
pub fn format_time_sync(clock_unix_secs: Option<u32>, clock_sync_age_secs: u32) -> String {
    match clock_unix_secs {
        Some(unix_secs) => {
            let (year, month, day, hour, minute, second) =
                crate::gps::civil_from_unix(unix_secs as i64);
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC (synced {}s ago)",
                year, month, day, hour, minute, second, clock_sync_age_secs
            )
        }
        None => "Not synced".to_string(),
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
        assert_eq!(format_time_sync(None, 0), "Not synced");
    }

    #[test]
    fn time_sync_synced_shows_wall_clock_and_age() {
        // 2026-07-15T14:32:10Z (known-answer instant shared with
        // `crate::gps::civil_from_unix_time_of_day_decodes`).
        let unix_secs = crate::gps::unix_timestamp(2026, 7, 15, 14, 32, 10) as u32;
        assert_eq!(
            format_time_sync(Some(unix_secs), 300),
            "2026-07-15 14:32:10 UTC (synced 300s ago)"
        );
    }
}
