// SPDX-License-Identifier: GPL-3.0-only
//! Admin menu screen — pure display-string formatting.
//!
//! The `slint::slint!{}` view and the `AdminMenuScreen` Rust wrapper stay in
//! `firmware/src/ui/screens/admin_menu.rs` (they depend on Slint); only the
//! plain-data `format_*` helpers below move here so their tests execute
//! under `cargo test --workspace` (this crate is a detached, cross-compiled
//! workspace — see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block
//! written there would type-check but never run). See
//! `docs/adr/0005-firmware-core-extraction.md`.

/// Format the screen-sleep seconds value for display: `0` → "Never", else `"<n>s"`.
pub fn format_screen_sleep(seconds: i32) -> String {
    if seconds <= 0 {
        "Never".to_string()
    } else {
        format!("{seconds}s")
    }
}

/// Format the battery row from a shared [`crate::battery::BatteryStatus`]:
/// `"<n>% (charging)"` when charging, else `"<n>%"`. Same formatting
/// convention as the host `status` command's `format_battery` — both read the
/// identical two fields (percent, charging) so the numbers always agree.
pub fn format_battery_display(status: crate::battery::BatteryStatus) -> String {
    if status.charging {
        format!("{}% (charging)", status.percent)
    } else {
        format!("{}%", status.percent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_zero_is_never() {
        assert_eq!(format_screen_sleep(0), "Never");
    }

    #[test]
    fn format_negative_is_never() {
        // Defensive: the widget clamps at 0 before display, but the formatter
        // itself must not panic or show a negative number if ever called directly.
        assert_eq!(format_screen_sleep(-5), "Never");
    }

    #[test]
    fn format_positive_appends_s() {
        assert_eq!(format_screen_sleep(30), "30s");
        assert_eq!(format_screen_sleep(120), "120s");
    }

    #[test]
    fn format_battery_not_charging_is_bare_percent() {
        let s = crate::battery::BatteryStatus {
            percent: 63,
            charging: false,
            raw_mv: 0,
            held_raw_mv: 0,
        };
        assert_eq!(format_battery_display(s), "63%");
    }

    #[test]
    fn format_battery_charging_appends_suffix() {
        let s = crate::battery::BatteryStatus {
            percent: 9,
            charging: true,
            raw_mv: 0,
            held_raw_mv: 0,
        };
        assert_eq!(format_battery_display(s), "9% (charging)");
    }
}
