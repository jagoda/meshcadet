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

/// Format the time-sync row's PRIMARY line: the actual GPS-synced absolute
/// wall-clock date+time — `"2026-07-15 14:32:10 UTC"` — or `"Not synced"`
/// when the system clock has never been set from a GPS fix since boot (this
/// device has no battery-backed RTC; see `firmware::gps`'s module doc).
///
/// Split from the relative-age line (see [`format_time_sync_age`]) so the
/// screen can render them as two rows instead of one over-wide line — see
/// `firmware/src/ui/screens/gps_status.rs`'s `StatusRow.value2`. The full
/// date INCLUDING YEAR is load-bearing, not decorative: it is what lets the
/// Commander confirm the sync is actually CORRECT rather than merely that it
/// happened. GPS week-number rollover is a real failure mode that produces a
/// correct time-of-day paired with a wrong date (often years off) — trimming
/// the year would hide exactly the field that catches it. Never compact this
/// line by dropping the date or the year.
///
/// `clock_unix_secs` is the CURRENT synced wall-clock time (ticks forward
/// every call as long as sync holds — see
/// [`crate::gps::synced_wall_clock_secs`]), not the time sync last occurred.
pub fn format_time_sync_date(clock_unix_secs: Option<u32>) -> String {
    match clock_unix_secs {
        Some(unix_secs) => {
            let (year, month, day, hour, minute, second) =
                crate::gps::civil_from_unix(unix_secs as i64);
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                year, month, day, hour, minute, second
            )
        }
        None => "Not synced".to_string(),
    }
}

/// Format the time-sync row's SECONDARY line: how long ago the sync
/// happened — `"synced 300s ago"` — or `""` (empty, so the screen renders no
/// second line at all) when the clock has never been synced. Companion to
/// [`format_time_sync_date`]; see that function's doc for why the row is
/// split in two.
///
/// `clock_unix_secs` gates the same way `format_time_sync_date` does: `None`
/// always renders empty regardless of `clock_sync_age_secs` — the two must
/// agree (both `None`/`0` or both populated), which is exactly what
/// `GpsStatus`'s single `clock_synced` flag already guarantees for both
/// fields.
pub fn format_time_sync_age(clock_unix_secs: Option<u32>, clock_sync_age_secs: u32) -> String {
    match clock_unix_secs {
        Some(_) => format!("synced {}s ago", clock_sync_age_secs),
        None => String::new(),
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
    fn time_sync_date_not_synced() {
        assert_eq!(format_time_sync_date(None), "Not synced");
    }

    #[test]
    fn time_sync_age_not_synced_is_empty() {
        // Empty (not "Not synced" repeated) so the screen renders no second
        // line at all — see `format_time_sync_age`'s doc.
        assert_eq!(format_time_sync_age(None, 0), "");
    }

    #[test]
    fn time_sync_date_shows_full_wall_clock_incl_year() {
        // 2026-07-15T14:32:10Z (known-answer instant shared with
        // `crate::gps::civil_from_unix_time_of_day_decodes`).
        let unix_secs = crate::gps::unix_timestamp(2026, 7, 15, 14, 32, 10) as u32;
        assert_eq!(
            format_time_sync_date(Some(unix_secs)),
            "2026-07-15 14:32:10 UTC"
        );
    }

    #[test]
    fn time_sync_age_shows_relative_age() {
        let unix_secs = crate::gps::unix_timestamp(2026, 7, 15, 14, 32, 10) as u32;
        assert_eq!(
            format_time_sync_age(Some(unix_secs), 300),
            "synced 300s ago"
        );
    }

    #[test]
    fn time_sync_date_survives_gps_week_rollover_with_year_intact() {
        // GPS week-number rollover: a correct time-of-day paired with a
        // wrong (often years-off) date — see `format_time_sync_date`'s doc.
        // A pre-rollover-fixed year (e.g. 1999, one full 1024-week epoch
        // before 2019's rollover) must still render in full, not truncated
        // or dropped — the year is precisely the field that makes this bug
        // diagnosable from the screen.
        let unix_secs = crate::gps::unix_timestamp(1999, 8, 22, 0, 0, 0) as u32;
        assert_eq!(
            format_time_sync_date(Some(unix_secs)),
            "1999-08-22 00:00:00 UTC"
        );
    }
}
