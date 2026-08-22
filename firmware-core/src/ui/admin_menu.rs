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

/// Abbreviate a [`crate::battery::BatteryLevel`] for the admin-menu row's
/// `L:` field — see [`format_battery_display`]'s doc.
fn level_abbrev(level: crate::battery::BatteryLevel) -> &'static str {
    use crate::battery::BatteryLevel;
    match level {
        BatteryLevel::Unknown => "Unk",
        BatteryLevel::Charging => "Chg",
        BatteryLevel::Low => "Low",
        BatteryLevel::Partial => "Part",
        BatteryLevel::Full => "Full",
    }
}

/// Format the battery row from a shared [`crate::battery::BatteryStatus`]:
/// `"<raw_mv>mV b:<basis_mv> B:<boot_mv> c:<0|1> f:<0|1> L:<Low|Part|Full>"`
/// — the full HIL capture state vector on one line
/// (`meshcadet-battery-three-state-pipeline`, 2026-08-22; `c` = charging,
/// `f` = confirmed). USB carries both the console and charge power on this
/// board, so any serial/host-CLI read is by construction a charging read —
/// the device's own screen is the only instrument that can observe an
/// unplugged state, so this row is what a real capture session reads
/// directly, mapping 1:1 onto the `BATTCAP v1` report block's fields.
///
/// **`percent` is deliberately OMITTED** — dropping `percent` (it's
/// derivable from `basis_mv` via [`crate::battery::percent_from_millivolts`])
/// is the sanctioned width reduction, rather than dropping `basis_mv`/
/// `boot_mv` ("the millivolt fields are the evidence"). The screen is 320px
/// wide at a
/// 14px row font (`AdminMenuScreenUi`'s `InfoRow`) with a fixed 40px row
/// height and no vertical slack to grow it (the layout is already at
/// 236/240px before this row) — even the trimmed 6-field vector is dense
/// enough that the Slint-side `InfoRow` instance for this row is sized down
/// to `Theme.size-meta` for headroom (see that call site's own comment).
/// No unit suffix on `basis_mv`/`boot_mv` (unlike `raw_mv`'s `mV`) —
/// deliberately terser for the same width reason; both are still
/// millivolts.
///
/// Supersedes the narrower `meshcadet-battery-glanceable-indicator`
/// (2026-08-04) row (`"~<n>% (<mv>mV[, charging])"`) — `raw_mv` is still
/// here, reformatted alongside the five new HIL probe fields; `percent` is
/// not (see above).
pub fn format_battery_display(status: crate::battery::BatteryStatus) -> String {
    format!(
        "{}mV b:{} B:{} c:{} f:{} L:{}",
        status.raw_mv,
        status.held_raw_mv,
        status.boot_mv,
        status.charging as u8,
        status.confirmed as u8,
        level_abbrev(status.level),
    )
}

/// Whether `prev` -> `new` changes anything the AdminMenu battery row
/// renders. [`format_battery_display`] reads `raw_mv`, `held_raw_mv`
/// (`basis_mv`), `boot_mv`, `charging`, `confirmed`, and `level` —
/// deliberately NOT `percent` (dropped from the row entirely, see that
/// function's own doc for the width rationale), so `percent` is
/// deliberately excluded from this gate too: it is a pure function of
/// `held_raw_mv` (see [`crate::battery::percent_from_millivolts`]), so a
/// `percent` change can never fire without `held_raw_mv` having changed
/// first, which the `held_raw_mv` gate below already catches — including it
/// here would only ever be a redundant, always-true-when-the-other-is-true
/// comparison.
///
/// `charging`/`held_raw_mv`/`level` gate on exact equality — they only move
/// on a genuine ADC-derived state transition, never per-poll jitter
/// (`held_raw_mv`/`level` added 2026-08-22,
/// `meshcadet-battery-three-state-pipeline`, now that the row renders
/// them). `raw_mv` gates on a [`RAW_MV_DISPLAY_DELTA_MV`]-magnitude move
/// instead of equality (see that constant's doc for why), compared against
/// `prev` — **the caller must pass the last-DISPLAYED status as `prev`, not
/// the last-POLLED one** (`UiRuntime::set_battery_status` tracks this
/// separately as `battery_status_displayed`): comparing against the last
/// poll instead would silently defeat the delta-gate against a slow,
/// continuous drift, since each individual poll-to-poll step can stay under
/// the threshold forever even as the cumulative drift since the row last
/// updated grows past it. `boot_mv`/`confirmed` are deliberately NOT gated
/// on directly: `boot_mv` is fixed for the life of the boot (never changes
/// after construction, so it can never itself trigger a redundant push),
/// and `confirmed` only ever flips `false`->`true` in lockstep with
/// `charging` flipping (the same poll that first proves the basis
/// trustworthy — see `advance_settled_confirmed`'s doc), which the
/// `charging` gate above already catches.
///
/// Used by `UiRuntime::set_battery_status` to skip the row's `format!`
/// allocation + Slint push on ticks that don't move the displayed text.
pub fn battery_display_fields_changed(
    prev: crate::battery::BatteryStatus,
    new: crate::battery::BatteryStatus,
) -> bool {
    prev.charging != new.charging
        || prev.held_raw_mv != new.held_raw_mv
        || prev.level != new.level
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

    /// Test-only convenience constructor — every field explicit so a reader
    /// can see exactly what a given `BatteryStatus` fixture represents,
    /// without every call site enumerating fields that don't matter to that
    /// test.
    fn status(
        percent: u8,
        charging: bool,
        raw_mv: u32,
        held_raw_mv: u32,
        boot_mv: u32,
        confirmed: bool,
        level: crate::battery::BatteryLevel,
    ) -> crate::battery::BatteryStatus {
        crate::battery::BatteryStatus {
            percent,
            charging,
            raw_mv,
            held_raw_mv,
            boot_mv,
            confirmed,
            level,
        }
    }

    #[test]
    fn format_battery_not_charging_renders_the_full_state_vector() {
        use crate::battery::BatteryLevel;
        let s = status(63, false, 3_900, 3_900, 3_850, true, BatteryLevel::Partial);
        assert_eq!(
            format_battery_display(s),
            "3900mV b:3900 B:3850 c:0 f:1 L:Part",
            "percent is deliberately omitted — see this function's own width-budget doc"
        );
    }

    #[test]
    fn format_battery_charging_unconfirmed_boot_shows_c1_f0() {
        use crate::battery::BatteryLevel;
        let s = status(9, true, 4_888, 3_624, 4_888, false, BatteryLevel::Charging);
        assert_eq!(
            format_battery_display(s),
            "4888mV b:3624 B:4888 c:1 f:0 L:Chg"
        );
    }

    // ── battery_display_fields_changed (alloc-and-tick dedup guard) ─────────
    // Regression guard: pins exactly which `BatteryStatus` fields (and, for
    // `raw_mv`, what MAGNITUDE of movement) gate the AdminMenu battery row's
    // `format!` + Slint push, independent of the hardware-backed
    // `UiRuntime`. `raw_mv` is delta-gated at `RAW_MV_DISPLAY_DELTA_MV`;
    // `charging`/`held_raw_mv`/`level` gate on exact equality; `boot_mv`/
    // `confirmed`/`percent` are deliberately excluded (see this function's
    // own doc for why).

    #[test]
    fn battery_display_fields_changed_false_when_nothing_material_moves() {
        use crate::battery::BatteryLevel;
        let a = status(50, false, 3_700, 3_700, 3_700, true, BatteryLevel::Partial);
        let b = status(50, false, 3_712, 3_700, 3_700, true, BatteryLevel::Partial); // raw_mv +12mV, below delta (20)
        assert!(!battery_display_fields_changed(a, b));
    }

    #[test]
    fn battery_display_fields_changed_true_when_raw_mv_moves_by_at_least_the_delta() {
        use crate::battery::BatteryLevel;
        let a = status(50, false, 3_700, 3_700, 3_700, true, BatteryLevel::Partial);
        let b = status(50, false, 3_720, 3_700, 3_700, true, BatteryLevel::Partial); // +20mV, exactly the delta
        assert!(
            battery_display_fields_changed(a, b),
            "a raw_mv move of exactly the delta threshold must gate the row (>=, not >)"
        );
    }

    #[test]
    fn battery_display_fields_changed_true_on_held_raw_mv_change() {
        use crate::battery::BatteryLevel;
        let a = status(50, false, 3_700, 3_700, 3_700, true, BatteryLevel::Partial);
        let b = status(50, false, 3_700, 4_100, 3_700, true, BatteryLevel::Partial);
        assert!(
            battery_display_fields_changed(a, b),
            "held_raw_mv (basis_mv) is now rendered by the row, so it must gate"
        );
    }

    #[test]
    fn battery_display_fields_changed_true_on_level_change() {
        use crate::battery::BatteryLevel;
        let a = status(50, false, 3_700, 3_700, 3_700, true, BatteryLevel::Low);
        let b = status(50, false, 3_700, 3_700, 3_700, true, BatteryLevel::Partial);
        assert!(battery_display_fields_changed(a, b));
    }

    #[test]
    fn battery_display_fields_changed_ignores_boot_mv_and_confirmed_alone() {
        use crate::battery::BatteryLevel;
        let a = status(50, false, 3_700, 3_700, 3_700, false, BatteryLevel::Low);
        let b = status(50, false, 3_700, 3_700, 4_888, true, BatteryLevel::Low);
        assert!(
            !battery_display_fields_changed(a, b),
            "boot_mv/confirmed moving alone, with nothing else changing, must not gate the row"
        );
    }

    #[test]
    fn battery_display_fields_changed_ignores_percent_alone_since_the_row_never_renders_it() {
        use crate::battery::BatteryLevel;
        // percent moves with nothing else changing — a synthetic case (in
        // the real pipeline percent is a pure function of held_raw_mv, so
        // this can't happen on its own), but it pins the documented
        // exclusion regardless.
        let a = status(50, false, 0, 0, 0, true, BatteryLevel::Unknown);
        let b = status(49, false, 0, 0, 0, true, BatteryLevel::Unknown);
        assert!(!battery_display_fields_changed(a, b));
    }

    #[test]
    fn battery_display_fields_changed_true_on_charging_flip() {
        use crate::battery::BatteryLevel;
        let a = status(50, false, 0, 0, 0, true, BatteryLevel::Unknown);
        let b = status(50, true, 0, 0, 0, true, BatteryLevel::Charging);
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
