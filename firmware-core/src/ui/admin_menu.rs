// SPDX-License-Identifier: GPL-3.0-only
//! Admin menu screen — pure display-string formatting, the battery-row
//! change-detection gate, and the notification master-toggle mapping.
//!
//! The `slint::slint!{}` view and the `AdminMenuScreen` Rust wrapper stay in
//! `firmware/src/ui/screens/admin_menu.rs` (they depend on Slint); only the
//! plain-data helpers below move here so their tests execute under `cargo
//! test --workspace` (this crate is a detached, cross-compiled workspace —
//! see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block written there
//! would type-check but never run). See
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

/// Whether `prev` -> `new` changes anything the AdminMenu battery row
/// renders. [`format_battery_display`] reads exactly `percent` and
/// `charging` (see that function's doc) — `raw_mv`/`held_raw_mv` are live
/// diagnostic-only fields the on-device row never shows, so they are
/// deliberately excluded here. Used by `UiRuntime::set_battery_status` to
/// skip the row's `format!` allocation + Slint push on ADC-jitter ticks that
/// don't move the displayed percentage or charging state.
pub fn battery_display_fields_changed(
    prev: crate::battery::BatteryStatus,
    new: crate::battery::BatteryStatus,
) -> bool {
    prev.percent != new.percent || prev.charging != new.charging
}

/// Map the admin-menu's two master toggles (`RuntimeSettings.notif_visual` /
/// `notif_audible`) to the [`crate::notification::NotifPrefs`] table
/// `UiRuntime::sync_notif_prefs` installs into its `NotifDispatcher` every
/// `step()`.
///
/// Extracted as a pure function (no `UiRuntime`/hardware dependency) so the
/// actual value of this fix — "the toggle wired to what `fire()` gates on" —
/// has a host-checkable unit test.
pub fn notif_prefs_from_toggles(visual: bool, audible: bool) -> crate::notification::NotifPrefs {
    crate::notification::NotifPrefs::from_provisioning_defaults(visual, audible)
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

    // ── battery_display_fields_changed (alloc-and-tick dedup guard) ─────────
    // Regression guard: pins exactly which `BatteryStatus` fields gate the
    // AdminMenu battery row's `format!` + Slint push, independent of the
    // hardware-backed `UiRuntime`.

    #[test]
    fn battery_display_fields_changed_false_when_percent_and_charging_same() {
        let a = crate::battery::BatteryStatus {
            percent: 50,
            charging: false,
            raw_mv: 3700,
            held_raw_mv: 3700,
        };
        let b = crate::battery::BatteryStatus {
            percent: 50,
            charging: false,
            raw_mv: 3712,
            held_raw_mv: 3705,
        };
        // raw_mv/held_raw_mv jitter (e.g. one ADC sample apart) must NOT count
        // as a display change — the row never renders either field.
        assert!(!battery_display_fields_changed(a, b));
    }

    #[test]
    fn battery_display_fields_changed_true_on_percent_change() {
        let a = crate::battery::BatteryStatus {
            percent: 50,
            charging: false,
            raw_mv: 0,
            held_raw_mv: 0,
        };
        let b = crate::battery::BatteryStatus {
            percent: 49,
            charging: false,
            raw_mv: 0,
            held_raw_mv: 0,
        };
        assert!(battery_display_fields_changed(a, b));
    }

    #[test]
    fn battery_display_fields_changed_true_on_charging_flip() {
        let a = crate::battery::BatteryStatus {
            percent: 50,
            charging: false,
            raw_mv: 0,
            held_raw_mv: 0,
        };
        let b = crate::battery::BatteryStatus {
            percent: 50,
            charging: true,
            raw_mv: 0,
            held_raw_mv: 0,
        };
        assert!(battery_display_fields_changed(a, b));
    }

    // ── notif_prefs_from_toggles (admin-menu master toggles) ───────────────
    // Regression guard for "audio/visual notifications ignore the admin
    // settings toggles": pins the pure mapping `UiRuntime::sync_notif_prefs`
    // installs into its `NotifDispatcher` every `step()`, independent of the
    // hardware-backed `UiRuntime`.

    #[test]
    fn notif_prefs_from_toggles_both_off_disables_every_event() {
        use crate::notification::NotifEvent;
        let prefs = notif_prefs_from_toggles(false, false);
        for event in [
            NotifEvent::IncomingDm,
            NotifEvent::IncomingGroupMsg,
            NotifEvent::DmAcked,
            NotifEvent::ChannelAcked,
            NotifEvent::Provisioned,
            NotifEvent::TelemetryResponse,
            NotifEvent::PinError,
            NotifEvent::PinSuccess,
        ] {
            let pref = prefs.pref_for(event);
            assert!(!pref.visual, "{:?} visual should be off", event);
            assert!(!pref.audible, "{:?} audible should be off", event);
        }
    }

    #[test]
    fn notif_prefs_from_toggles_both_on_enables_incoming_dm() {
        let prefs = notif_prefs_from_toggles(true, true);
        assert!(prefs.incoming_dm.visual);
        assert!(prefs.incoming_dm.audible);
    }

    #[test]
    fn notif_prefs_from_toggles_gates_dispatcher_fire() {
        // End-to-end through the real gating path: build the prefs the
        // "both off" master toggle produces, install them via `set_prefs`
        // (same call `sync_notif_prefs` makes), then confirm `fire()`
        // actually produces no tone (PinSuccess has no visual mechanism to
        // gate at all now that the border flash is gone).
        use crate::notification::{NotifDispatcher, NotifEvent, NotifPrefs};
        let mut d = NotifDispatcher::new(NotifPrefs::default());
        d.set_prefs(notif_prefs_from_toggles(false, false));
        d.fire(NotifEvent::PinSuccess, 0, false);
        assert!(d.take_tones().is_none());
    }

    #[test]
    fn notif_prefs_from_toggles_visual_off_audible_on_is_independent() {
        // The two toggles are independent switches, not a single master
        // mute — audible-only must still fire tones. (`pin_success.visual`
        // is inert now that the border flash is gone, but the toggle
        // mapping still threads the raw bool through uniformly; see
        // `NotifPref`'s doc.)
        let prefs = notif_prefs_from_toggles(false, true);
        assert!(!prefs.pin_success.visual);
        assert!(prefs.pin_success.audible);
    }
}
