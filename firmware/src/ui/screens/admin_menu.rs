// SPDX-License-Identifier: GPL-3.0-only
//! Admin menu screen — shown after a correct PIN is entered.
//!
//! Replaces the `TODO(admin-menu-screen)` that previously sent every
//! successful PIN unlock straight back to the contact list.  This screen
//! exposes a small set of on-device toggles (no laptop required) that map
//! directly onto `pin_menu::RuntimeSettings` fields via
//! `pin_menu::apply_menu_action`.
//!
//! # Boundary
//!
//! Mirrors the `PinEntryScreen` widget/logic split documented there:
//! - This module owns the **widget** (header, back button, toggle rows) and
//!   the purely visual "flip the switch" behaviour.
//! - The caller (`ui::mod::UiRuntime::navigate_to_admin_menu`) owns the
//!   **menu logic**: applying the toggle to `RuntimeSettings` via
//!   `pin_menu::apply_menu_action` and persisting the result to NVS.
//!
//! Each toggle callback reports the NEW boolean value it just set visually
//! (`on_toggle_notif_visual(|new_val| ...)`), so the caller does not need to
//! re-read component state to know what changed.
//!
//! # Theme tokens + one-shot animation language
//!
//! Every color/font-size literal in this screen's `slint::slint!{}` block
//! (below) now reads from the shared `Theme` global (`ui/theme.slint`,
//! imported below) at the SAME values — a pixel-identical swap, same pattern
//! as `splash.rs`'s Phase-1 pilot and `gps_status.rs`'s Phase-8 application.
//! A single one-shot screen-entry fade applies this UI's "never an
//! infinite loop, never cut off mid-cycle" animation language:
//! `AdminMenuScreen::new()` builds a fresh component on every navigation here
//! (mirrors `GpsStatusScreen`/`ComposeScreen` — reached by interactive
//! navigation, not boot), so the `init` handler below fires exactly once per
//! mount and its single write to `reveal_opacity`'s settled value is what
//! fires the `animate` transition — same self-contained deferred-write
//! mechanism as `gps_status.rs`. The toggle pill's existing `animate x`
//! (state feedback, not screen entry) is left untouched.
//!
//! # Outer-space theme (per-screen spec row 7: "console tint" /
//! "ringed planet in header")
//!
//! Two additive, presentation-only changes on top of the palette wiring
//! above — no new asset is authored here, both are reused as-is from the
//! shared `ui/motifs.slint` contract:
//!
//! - **Ringed planet in header** — `RingedPlanetCorner` sits in the header
//!   bar's top-right corner, scaled down from its 40x40 default to 28x28 and
//!   declared BEFORE the header's `HorizontalLayout` so it paints BEHIND the
//!   back button / title / balance spacer (Slint z-orders by declaration
//!   order — same convention `contact_list.rs`'s header `Starfield` and
//!   `unprovisioned.rs`'s corner-planet placement already established). It
//!   sits entirely under the balance spacer's 44px column, which carries no
//!   fill, so the motif shows through cleanly without touching the back
//!   button or the centered title's layout.
//! - **Console tint** — originally a full-bleed `Theme.nebula-violet-deep`
//!   wash at low alpha; **superseded** by the shared full-window
//!   `SpaceBackdrop` dim-starfield component instead of stacking both washes
//!   behind the content — per the "do not double-wash; pick
//!   one" design rule. `SpaceBackdrop` sits behind the whole screen, between
//!   the `bg-space` window fill and the foreground content, in the same
//!   declared-first z-bottom slot the old tint Rectangle occupied.
//!   This is a screen-wide treatment — distinct from `unprovisioned.rs`'s
//!   direct `space-deep` background swap (row 2) and `contact_list.rs`'s
//!   per-row wash (row 3) — giving the admin's settings "console" a dim
//!   starfield backdrop without touching row/header contrast (rows keep
//!   their existing `surface-raised`/`transparent` fills unchanged).

slint::slint! {
    import { Theme } from "../theme.slint";
    import { RingedPlanetCorner, SpaceBackdrop } from "../motifs.slint";

    // Numeric "-"/"+" row for the screen-sleep inactivity timeout (0-120s,
    // 0 = never). The displayed label is precomputed Rust-side (`"Never"` vs.
    // `"<n>s"`) — see `ContactRow.unread_str` in `contact_list.rs` for the same
    // "format on the Rust side, pass a plain string" convention used
    // throughout this UI (Slint has no int->string formatting helper here).
    component StepperRow {
        in property <string> label;
        // Raw seconds value, used only to enable/disable the +/- buttons at
        // the 0/120 bounds. The visible text is `display_text`.
        in property <int>    value;
        in property <string> display_text;
        // Trackball highlight — see `AdminMenuScreenUi.selected_index`.
        in property <bool>   selected;
        callback decremented;
        callback incremented;

        height: 40px;

        Rectangle {
            background: selected ? Theme.surface-raised : transparent;

            // Bottom separator
            Rectangle {
                y: parent.height - 1px;
                height: 1px;
                width: parent.width;
                background: Theme.surface-raised;
            }

            HorizontalLayout {
                padding-left: 12px;
                padding-right: 12px;
                spacing: 8px;

                Text {
                    text: label;
                    font-size: Theme.size-body-lg;
                    color: Theme.text-primary;
                    vertical-alignment: center;
                    horizontal-stretch: 1.0;
                }

                Rectangle {
                    width: 28px;
                    height: 28px;
                    border-radius: 14px;
                    background: dec_touch.has-hover ? Theme.surface-alt : Theme.surface-raised;
                    Text {
                        text: "−";
                        font-size: Theme.icon-sm; // 18px
                        font-weight: 600;
                        color: value <= 0 ? Theme.text-muted : Theme.brand-signal;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }
                    dec_touch := TouchArea {
                        enabled: value > 0;
                        clicked => { root.decremented(); }
                    }
                }

                Text {
                    text: display_text;
                    font-size: Theme.size-body-lg;
                    color: Theme.text-primary;
                    horizontal-alignment: center;
                    vertical-alignment: center;
                    width: 54px;
                }

                Rectangle {
                    width: 28px;
                    height: 28px;
                    border-radius: 14px;
                    background: inc_touch.has-hover ? Theme.surface-alt : Theme.surface-raised;
                    Text {
                        text: "+";
                        font-size: Theme.icon-sm; // 18px
                        font-weight: 600;
                        color: value >= 120 ? Theme.text-muted : Theme.brand-signal;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }
                    inc_touch := TouchArea {
                        enabled: value < 120;
                        clicked => { root.incremented(); }
                    }
                }
            }
        }
    }

    // Read-only info row — a label + right-aligned value, no touch/toggle at
    // all. Used for "🔋 Battery", which is pure display (no control surface,
    // same "status/display only" contract as the GPS status screen).
    component InfoRow {
        in property <string> label;
        in property <string> value;

        height: 40px;

        Rectangle {
            // Bottom separator
            Rectangle {
                y: parent.height - 1px;
                height: 1px;
                width: parent.width;
                background: Theme.surface-raised;
            }

            HorizontalLayout {
                padding-left: 12px;
                padding-right: 12px;
                spacing: 8px;

                Text {
                    text: label;
                    font-size: Theme.size-body-lg;
                    color: Theme.text-primary;
                    vertical-alignment: center;
                    horizontal-stretch: 1.0;
                }

                Text {
                    text: value;
                    font-size: Theme.size-body-lg;
                    color: Theme.text-secondary;
                    vertical-alignment: center;
                    horizontal-alignment: right;
                }
            }
        }
    }

    // Plain navigation row — a label + chevron, no toggle/stepper. Used for
    // "📍 GPS status", which opens a read-only sub-screen (no on/off state to
    // show here).
    component NavRow {
        in property <string> label;
        // Trackball highlight — see `AdminMenuScreenUi.selected_index`.
        in property <bool>   selected;
        callback tapped;

        height: 40px;

        Rectangle {
            background: (selected || row_touch.has-hover) ? Theme.surface-raised : transparent;

            // Bottom separator
            Rectangle {
                y: parent.height - 1px;
                height: 1px;
                width: parent.width;
                background: Theme.surface-raised;
            }

            HorizontalLayout {
                padding-left: 12px;
                padding-right: 12px;
                spacing: 8px;

                Text {
                    text: label;
                    font-size: Theme.size-body-lg;
                    color: Theme.text-primary;
                    vertical-alignment: center;
                    horizontal-stretch: 1.0;
                }

                Text {
                    text: "›";
                    font-size: Theme.icon-sm; // 18px
                    color: Theme.text-secondary;
                    vertical-alignment: center;
                }
            }

            row_touch := TouchArea {
                width: parent.width;
                height: parent.height;
                clicked => { root.tapped(); }
            }
        }
    }

    component ToggleRow {
        in property <string> label;
        in property <bool>   value;
        // Trackball highlight — see `AdminMenuScreenUi.selected_index`.
        in property <bool>   selected;
        callback toggled;

        height: 40px;

        Rectangle {
            background: (selected || row_touch.has-hover) ? Theme.surface-raised : transparent;

            // Bottom separator
            Rectangle {
                y: parent.height - 1px;
                height: 1px;
                width: parent.width;
                background: Theme.surface-raised;
            }

            HorizontalLayout {
                padding-left: 12px;
                padding-right: 12px;
                spacing: 8px;

                Text {
                    text: label;
                    font-size: Theme.size-body-lg;
                    color: Theme.text-primary;
                    vertical-alignment: center;
                    horizontal-stretch: 1.0;
                }

                // Pill-shaped switch.
                Rectangle {
                    width: 44px;
                    height: 24px;
                    border-radius: 12px;
                    background: value ? Theme.brand-signal : Theme.surface-alt;
                    y: (parent.height - self.height) / 2;

                    Rectangle {
                        width: 18px;
                        height: 18px;
                        border-radius: 9px;
                        background: Theme.text-primary;
                        y: 3px;
                        x: value ? parent.width - self.width - 3px : 3px;
                        animate x { duration: 120ms; }
                    }
                }
            }

            row_touch := TouchArea {
                width: parent.width;
                height: parent.height;
                clicked => { root.toggled(); }
            }
        }
    }

    export component AdminMenuScreenUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        in property <bool>   notif_visual: true;
        in property <bool>   notif_audible: true;
        in property <int>    screen_sleep_timeout_s: 30;
        in property <string> screen_sleep_display: "30s";
        // Precomputed Rust-side (`"<n>%"` / `"<n>% (charging)"`) — same
        // "format on the Rust side, pass a plain string" convention as
        // `screen_sleep_display` above.
        in property <string> battery_display: "—";
        // Trackball-driven row highlight: 0=visual toggle, 1=audible toggle,
        // 2=screen-sleep stepper, 3=GPS status row. `-1` = no highlight yet
        // (touch taps a row directly and never sets this).
        in property <int>    selected_index: -1;

        callback back_pressed;
        callback toggle_notif_visual;
        callback toggle_notif_audible;
        callback decrement_screen_sleep_timeout;
        callback increment_screen_sleep_timeout;
        callback open_gps_status;

        // ── One-shot screen-entry reveal — see module doc ───────────────────
        in-out property <float> reveal_opacity: 0;
        animate reveal_opacity { duration: 200ms; easing: ease-out; }
        init => { self.reveal_opacity = 1.0; }

        // Full-window dim starfield backdrop — replaces the flat
        // `nebula-violet-deep.with-alpha(0.08)` console-tint wash this screen
        // used to paint here (see the module doc's now-superseded "Console
        // tint" note) rather than stacking both, per the "do not
        // double-wash; pick one" design rule. Declared first, so
        // Slint paints it before the header/rows below; the ≤0.35 alpha
        // ceiling is baked into `SpaceBackdrop` itself (see `motifs.slint`),
        // not overridden here.
        SpaceBackdrop {}

        VerticalLayout {
            spacing: 0px;
            opacity: reveal_opacity;

            // ── Header bar ──────────────────────────────────────────────────
            Rectangle {
                height: 36px;
                background: Theme.surface;

                // Ringed-planet corner accent — see module doc. Declared
                // before the HorizontalLayout below so it paints behind the
                // back button / title / balance spacer; it sits entirely
                // under the (unfilled) balance spacer's 44px column at the
                // far right, so it shows through without touching either the
                // back button or the centered title.
                RingedPlanetCorner {
                    x: parent.width - 28px - 4px;
                    y: (parent.height - self.height) / 2;
                    width: 28px;
                    height: 28px;
                }

                HorizontalLayout {
                    padding-left: 4px;
                    padding-right: 8px;
                    spacing: 4px;

                    Rectangle {
                        width: 44px; height: 36px;
                        Text {
                            text: "‹";
                            font-size: Theme.size-display; // 22px
                            color: Theme.brand-signal;
                            horizontal-alignment: center;
                            vertical-alignment: center;
                        }
                        TouchArea { clicked => { root.back_pressed(); } }
                    }

                    Text {
                        text: "⚙ Admin Menu";
                        font-size: Theme.size-subtitle; // 15px
                        font-weight: 600;
                        color: Theme.text-primary;
                        horizontal-stretch: 1.0;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }

                    // Balance the back button's width so the title stays centered.
                    Rectangle { width: 44px; height: 36px; }
                }
            }

            // ── Battery (read-only info row) ─────────────────────────────────
            InfoRow {
                label: "🔋  Battery";
                value: battery_display;
            }

            // ── Toggle rows ─────────────────────────────────────────────────
            ToggleRow {
                label: "🔔  Visual notifications";
                value: notif_visual;
                selected: selected_index == 0;
                toggled => { root.toggle_notif_visual(); }
            }
            ToggleRow {
                label: "🔊  Audible notifications";
                value: notif_audible;
                selected: selected_index == 1;
                toggled => { root.toggle_notif_audible(); }
            }
            StepperRow {
                label: "💤  Screen sleep";
                value: screen_sleep_timeout_s;
                display_text: screen_sleep_display;
                selected: selected_index == 2;
                decremented => { root.decrement_screen_sleep_timeout(); }
                incremented => { root.increment_screen_sleep_timeout(); }
            }
            NavRow {
                label: "📍  GPS status";
                selected: selected_index == 3;
                tapped => { root.open_gps_status(); }
            }

            Rectangle { vertical-stretch: 1.0; }
        }
    }
}

/// Step size (seconds) applied per +/- tap on the screen-sleep stepper.
/// Not part of the persisted `RuntimeSettings` contract — purely a UI
/// increment; `pin_menu::apply_menu_action` clamps the result to 0..=120
/// regardless of what step size the widget uses.
const SCREEN_SLEEP_STEP_S: i32 = 5;

/// Format the screen-sleep seconds value for display: `0` → "Never", else `"<n>s"`.
fn format_screen_sleep(seconds: i32) -> String {
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
///
/// `pub(crate)`: also called from `ui::mod::UiRuntime::set_battery_status` to
/// push a live-refreshed value into an already-open AdminMenu screen.
pub(crate) fn format_battery_display(status: crate::battery::BatteryStatus) -> String {
    if status.charging {
        format!("{}% (charging)", status.percent)
    } else {
        format!("{}%", status.percent)
    }
}

/// Rust-side wrapper.
pub struct AdminMenuScreen {
    component: self::AdminMenuScreenUi,
}

impl AdminMenuScreen {
    pub fn new() -> anyhow::Result<Self> {
        let component = self::AdminMenuScreenUi::new()
            .map_err(|e| anyhow::anyhow!("slint component init: {:?}", e))?;
        component.show()
            .map_err(|e| anyhow::anyhow!("slint window show: {:?}", e))?;
        Ok(AdminMenuScreen { component })
    }

    /// Set the initial displayed state of the "visual notifications" toggle.
    pub fn set_notif_visual(&self, v: bool) {
        self.component.set_notif_visual(v);
    }

    /// Set the initial displayed state of the "audible notifications" toggle.
    pub fn set_notif_audible(&self, v: bool) {
        self.component.set_notif_audible(v);
    }

    /// Set the initial displayed screen-sleep timeout (seconds, 0..=120; 0 =
    /// "Never"). Updates both the raw value (bounds-check for +/-) and the
    /// precomputed display string.
    pub fn set_screen_sleep_timeout(&self, seconds: i32) {
        self.component.set_screen_sleep_timeout_s(seconds);
        self.component.set_screen_sleep_display(format_screen_sleep(seconds).into());
    }

    /// Set the displayed battery row (`"<n>%"` / `"<n>% (charging)"`),
    /// precomputed Rust-side by [`format_battery_display`] from the shared
    /// `battery::BatteryStatus` snapshot — the same source the host `status`
    /// command and the radio telemetry RESPONSE read (single shared source;
    /// see the firmware `battery` module docs).
    pub fn set_battery_display(&self, text: &str) {
        self.component.set_battery_display(text.into());
    }

    pub fn on_back_pressed(&self, cb: impl Fn() + 'static) {
        self.component.on_back_pressed(cb);
    }

    /// Move the trackball highlight to row `idx` (0..=3; see
    /// `AdminMenuScreenUi.selected_index`'s doc for the row mapping; `-1`
    /// clears it). The caller (`UiRuntime::handle_trackball_admin_menu`) owns
    /// clamping `idx` to the row count.
    pub fn set_selected_index(&self, idx: i32) {
        self.component.set_selected_index(idx);
    }

    /// Fire `back_pressed` exactly as the header's back button would — used
    /// by the trackball's roll-Left handler.
    pub fn invoke_back_pressed(&self) {
        self.component.invoke_back_pressed();
    }

    /// Fire `toggle_notif_visual` exactly as tapping that row would — used by
    /// the trackball's Click handler when row 0 is highlighted.
    pub fn invoke_toggle_notif_visual(&self) {
        self.component.invoke_toggle_notif_visual();
    }

    /// Fire `toggle_notif_audible` exactly as tapping that row would — used by
    /// the trackball's Click handler when row 1 is highlighted.
    pub fn invoke_toggle_notif_audible(&self) {
        self.component.invoke_toggle_notif_audible();
    }

    /// Fire `increment_screen_sleep_timeout` exactly as tapping the stepper's
    /// "+" would — used by the trackball's Click handler when row 2 (the
    /// screen-sleep stepper) is highlighted. See
    /// `UiRuntime::handle_trackball_admin_menu`'s doc for why Click maps to
    /// increment specifically (a bidirectional stepper has no single obvious
    /// "activate").
    pub fn invoke_increment_screen_sleep_timeout(&self) {
        self.component.invoke_increment_screen_sleep_timeout();
    }

    /// Fire `open_gps_status` exactly as tapping that row would — used by the
    /// trackball's Click handler when row 3 is highlighted.
    pub fn invoke_open_gps_status(&self) {
        self.component.invoke_open_gps_status();
    }

    /// Fires `cb(new_value)` when the user taps the visual-notifications row.
    /// The switch's displayed position is flipped here (the widget's own
    /// concern) before `cb` is invoked, so the caller only needs to apply the
    /// new value to `RuntimeSettings` and persist it.
    pub fn on_toggle_notif_visual(&self, cb: impl Fn(bool) + 'static) {
        let comp = self.component.clone_strong();
        self.component.on_toggle_notif_visual(move || {
            let new_val = !comp.get_notif_visual();
            comp.set_notif_visual(new_val);
            cb(new_val);
        });
    }

    /// Fires `cb(new_value)` when the user taps the audible-notifications row.
    /// See [`Self::on_toggle_notif_visual`] for the displayed-state contract.
    pub fn on_toggle_notif_audible(&self, cb: impl Fn(bool) + 'static) {
        let comp = self.component.clone_strong();
        self.component.on_toggle_notif_audible(move || {
            let new_val = !comp.get_notif_audible();
            comp.set_notif_audible(new_val);
            cb(new_val);
        });
    }

    /// Fires `cb(new_seconds)` when the user taps "−" on the screen-sleep row.
    /// Clamped to a floor of 0 here for the widget's own displayed-state
    /// consistency; the caller (`ui::mod::navigate_to_admin_menu`) applies
    /// `new_seconds` via `pin_menu::apply_menu_action`, which re-clamps to
    /// 0..=120 as the single source of truth for the persisted invariant.
    pub fn on_decrement_screen_sleep_timeout(&self, cb: impl Fn(i32) + 'static) {
        let comp = self.component.clone_strong();
        self.component.on_decrement_screen_sleep_timeout(move || {
            let new_val = (comp.get_screen_sleep_timeout_s() - SCREEN_SLEEP_STEP_S).max(0);
            comp.set_screen_sleep_timeout_s(new_val);
            comp.set_screen_sleep_display(format_screen_sleep(new_val).into());
            cb(new_val);
        });
    }

    /// Fires `cb(new_seconds)` when the user taps "+" on the screen-sleep row.
    /// See [`Self::on_decrement_screen_sleep_timeout`] for the clamp contract.
    pub fn on_increment_screen_sleep_timeout(&self, cb: impl Fn(i32) + 'static) {
        let comp = self.component.clone_strong();
        self.component.on_increment_screen_sleep_timeout(move || {
            let new_val = (comp.get_screen_sleep_timeout_s() + SCREEN_SLEEP_STEP_S).min(120);
            comp.set_screen_sleep_timeout_s(new_val);
            comp.set_screen_sleep_display(format_screen_sleep(new_val).into());
            cb(new_val);
        });
    }

    /// Fires `cb()` when the user taps the "📍 GPS status" row. The caller
    /// navigates to the read-only [`super::gps_status::GpsStatusScreen`]
    /// sub-screen — no state to flip here (this row is pure navigation).
    pub fn on_open_gps_status(&self, cb: impl Fn() + 'static) {
        self.component.on_open_gps_status(cb);
    }

    pub fn hide(&self) { self.component.hide().ok(); }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
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
        let s = crate::battery::BatteryStatus { percent: 63, charging: false, raw_mv: 0, held_raw_mv: 0 };
        assert_eq!(format_battery_display(s), "63%");
    }

    #[test]
    fn format_battery_charging_appends_suffix() {
        let s = crate::battery::BatteryStatus { percent: 9, charging: true, raw_mv: 0, held_raw_mv: 0 };
        assert_eq!(format_battery_display(s), "9% (charging)");
    }
}
