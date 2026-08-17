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

/// Minimum `raw_mv` movement (since the value the AdminMenu row LAST
/// ACTUALLY DISPLAYED — see [`battery_display_fields_changed`]'s doc for why
/// that's a different basis than "last polled") before that movement alone
/// is treated as a display change. `meshcadet-battery-glanceable-indicator`:
/// the row now renders `raw_mv` directly (see [`format_battery_display`]),
/// so it can no longer be excluded from the change gate outright the way
/// `meshcadet-perf-ui-*`'s original fix excluded it — `raw_mv` updates on
/// (almost) every ADC poll (`battery.rs`'s module doc), so an exact-equality
/// gate would re-`format!` + re-push the row nearly every poll, reintroducing
/// the allocation churn the original fix eliminated. Delta-gating instead
/// keeps the displayed mV honestly fresh once it moves by a perceptible
/// amount while absorbing the live ADC-noise floor. 20mV is comfortably
/// above single-poll ADC jitter (a few mV, per `battery.rs`'s averaging) and
/// well under a single [`crate::battery::RESTING_SOC_CURVE`] breakpoint
/// spacing (50mV+) — the same order of magnitude
/// `crate::battery::PERSIST_MIN_DELTA_MV` uses for "meaningful movement"
/// elsewhere in this same module's sibling `battery.rs`.
pub const RAW_MV_DISPLAY_DELTA_MV: u32 = 20;

/// Format the battery row from a shared [`crate::battery::BatteryStatus`]:
/// `"~<n>% (<mv>mV)"`, or `"~<n>% (<mv>mV, charging)"` while charging.
/// `meshcadet-battery-glanceable-indicator`: the row now also surfaces the
/// raw mV reading (`raw_mv` — the live, instantaneous ADC reading; NOT
/// `held_raw_mv`, which stays diagnostic-only, read only by the host CLI
/// `status` command) alongside the percent, so the on-device admin screen
/// carries the same raw-voltage diagnostic previously visible only over the
/// host CLI. The leading `~` signals "approximate" — `percent` is itself a
/// slew-limited, curve-interpolated estimate (see `battery.rs`'s module
/// doc), not a precise fuel-gauge reading.
pub fn format_battery_display(status: crate::battery::BatteryStatus) -> String {
    if status.charging {
        format!("~{}% ({}mV, charging)", status.percent, status.raw_mv)
    } else {
        format!("~{}% ({}mV)", status.percent, status.raw_mv)
    }
}

/// Whether `prev` -> `new` changes anything the AdminMenu battery row
/// renders. [`format_battery_display`] reads `percent`, `charging`, and
/// `raw_mv` (see that function's doc) — `held_raw_mv` remains a live
/// diagnostic-only field the on-device row never shows, so it stays
/// deliberately excluded here, same rationale the original fix gave for both
/// fields before `raw_mv` became a rendered field.
///
/// `percent`/`charging` still gate on exact equality (unchanged — they only
/// move on a genuine ADC-derived state transition, never per-poll jitter).
/// `raw_mv` gates on a [`RAW_MV_DISPLAY_DELTA_MV`]-magnitude move instead of
/// equality (see that constant's doc for why), compared against `prev` —
/// **the caller must pass the last-DISPLAYED status as `prev`, not the last-
/// POLLED one** (`UiRuntime::set_battery_status` tracks this separately as
/// `battery_status_displayed`): comparing against the last poll instead would
/// silently defeat the delta-gate against a slow, continuous drift, since
/// each individual poll-to-poll step can stay under the threshold forever
/// even as the cumulative drift since the row last updated grows past it.
///
/// Used by `UiRuntime::set_battery_status` to skip the row's `format!`
/// allocation + Slint push on ticks that don't move the displayed text.
pub fn battery_display_fields_changed(
    prev: crate::battery::BatteryStatus,
    new: crate::battery::BatteryStatus,
) -> bool {
    prev.percent != new.percent
        || prev.charging != new.charging
        || prev.raw_mv.abs_diff(new.raw_mv) >= RAW_MV_DISPLAY_DELTA_MV
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
    fn format_battery_not_charging_shows_percent_and_raw_mv() {
        let s = crate::battery::BatteryStatus {
            percent: 63,
            charging: false,
            raw_mv: 3900,
            held_raw_mv: 3900,
            level: crate::battery::BatteryLevel::Unknown,
        };
        assert_eq!(format_battery_display(s), "~63% (3900mV)");
    }

    #[test]
    fn format_battery_charging_appends_suffix_after_mv() {
        let s = crate::battery::BatteryStatus {
            percent: 9,
            charging: true,
            raw_mv: 4888,
            held_raw_mv: 3624,
            level: crate::battery::BatteryLevel::Charging,
        };
        assert_eq!(format_battery_display(s), "~9% (4888mV, charging)");
    }

    // ── battery_display_fields_changed (alloc-and-tick dedup guard) ─────────
    // Regression guard: pins exactly which `BatteryStatus` fields (and, for
    // `raw_mv`, what MAGNITUDE of movement) gate the AdminMenu battery row's
    // `format!` + Slint push, independent of the hardware-backed
    // `UiRuntime`. `held_raw_mv` stays fully excluded (never rendered);
    // `raw_mv` is delta-gated at `RAW_MV_DISPLAY_DELTA_MV`, not excluded
    // outright, now that `format_battery_display` renders it.

    #[test]
    fn battery_display_fields_changed_false_when_percent_charging_same_and_raw_mv_below_delta() {
        let a = crate::battery::BatteryStatus {
            percent: 50,
            charging: false,
            raw_mv: 3700,
            held_raw_mv: 3700,
            level: crate::battery::BatteryLevel::Unknown,
        };
        let b = crate::battery::BatteryStatus {
            percent: 50,
            charging: false,
            raw_mv: 3712, // +12mV — below RAW_MV_DISPLAY_DELTA_MV (20)
            held_raw_mv: 3705,
            level: crate::battery::BatteryLevel::Unknown,
        };
        // Sub-threshold raw_mv jitter (e.g. one ADC sample apart) must NOT
        // count as a display change — held_raw_mv is never rendered at all.
        assert!(!battery_display_fields_changed(a, b));
    }

    #[test]
    fn battery_display_fields_changed_true_when_raw_mv_moves_by_at_least_the_delta() {
        let a = crate::battery::BatteryStatus {
            percent: 50,
            charging: false,
            raw_mv: 3700,
            held_raw_mv: 3700,
            level: crate::battery::BatteryLevel::Unknown,
        };
        let b = crate::battery::BatteryStatus {
            percent: 50,
            charging: false,
            raw_mv: 3720, // +20mV — exactly RAW_MV_DISPLAY_DELTA_MV
            held_raw_mv: 3700,
            level: crate::battery::BatteryLevel::Unknown,
        };
        assert!(
            battery_display_fields_changed(a, b),
            "a raw_mv move of exactly the delta threshold must gate the row \
             (>=, not >)"
        );
    }

    #[test]
    fn battery_display_fields_changed_ignores_held_raw_mv_even_on_a_large_move() {
        let a = crate::battery::BatteryStatus {
            percent: 50,
            charging: false,
            raw_mv: 3700,
            held_raw_mv: 3700,
            level: crate::battery::BatteryLevel::Unknown,
        };
        let b = crate::battery::BatteryStatus {
            percent: 50,
            charging: false,
            raw_mv: 3700,
            held_raw_mv: 4100, // a large move — must still not gate the row
            level: crate::battery::BatteryLevel::Unknown,
        };
        assert!(
            !battery_display_fields_changed(a, b),
            "held_raw_mv is diagnostic-only — the row never renders it"
        );
    }

    #[test]
    fn battery_display_fields_changed_true_on_percent_change() {
        let a = crate::battery::BatteryStatus {
            percent: 50,
            charging: false,
            raw_mv: 0,
            held_raw_mv: 0,
            level: crate::battery::BatteryLevel::Unknown,
        };
        let b = crate::battery::BatteryStatus {
            percent: 49,
            charging: false,
            raw_mv: 0,
            held_raw_mv: 0,
            level: crate::battery::BatteryLevel::Unknown,
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
            level: crate::battery::BatteryLevel::Unknown,
        };
        let b = crate::battery::BatteryStatus {
            percent: 50,
            charging: true,
            raw_mv: 0,
            held_raw_mv: 0,
            level: crate::battery::BatteryLevel::Unknown,
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
