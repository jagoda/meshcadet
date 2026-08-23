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
        // the min_value/max_value bounds. The visible text is `display_text`.
        in property <int>    value;
        in property <string> display_text;
        // Bounds for the +/- enable/disable + muted-color gating below.
        // Default 0..=120 matches this component's original sole caller
        // (the screen-sleep row) exactly, so that instantiation's behavior
        // is byte-for-byte unchanged; the screen-lock timeout row (plan D1:
        // `LOCK_TIMEOUT_MIN_S..=LOCK_TIMEOUT_MAX_S`, 15..=3600) sets both
        // explicitly. `apply_menu_action` re-clamps to the real bound
        // regardless — these two properties only drive this ROW's own
        // affordance styling, never the persisted value.
        in property <int>    min_value: 0;
        in property <int>    max_value: 120;
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
                        color: value <= min_value ? Theme.text-muted : Theme.brand-signal;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }
                    dec_touch := TouchArea {
                        enabled: value > min_value;
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
                        color: value >= max_value ? Theme.text-muted : Theme.brand-signal;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }
                    inc_touch := TouchArea {
                        enabled: value < max_value;
                        clicked => { root.incremented(); }
                    }
                }
            }
        }
    }

    // Read-only info row — a label + right-aligned value, no touch/toggle at
    // all. Used ONLY for "🔋 Battery" (a single instantiation below), which
    // is pure display (no control surface, same "status/display only"
    // contract as the GPS status screen).
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

                // `meshcadet-battery-three-state-pipeline` (2026-08-22) grew
                // this row's `value` from a short "~63% (3900mV)" summary to
                // the full HIL-capture state vector
                // (`firmware_core::ui::admin_menu::format_battery_display`'s
                // doc) — up to ~35 characters. The screen is 320px wide with
                // a fixed 40px row height and no vertical layout slack to
                // grow it (`AdminMenuScreenUi`'s row budget is already
                // 236/240px committed), so this instance is sized DOWN to
                // `Theme.size-meta` (not the row's default `size-body-lg`)
                // for width headroom, with `overflow: elide` as a defensive
                // fallback so a still-too-long capture (e.g. an unexpectedly
                // large millivolt reading) truncates visibly with an
                // ellipsis rather than silently clipping mid-character or
                // painting outside the row.
                Text {
                    text: value;
                    font-size: Theme.size-meta;
                    color: Theme.text-secondary;
                    vertical-alignment: center;
                    horizontal-alignment: right;
                    overflow: elide;
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
        // Precomputed Rust-side — see `firmware_core::ui::admin_menu::
        // format_battery_display`'s doc for the current row layout (the
        // full HIL-capture state vector as of
        // `meshcadet-battery-three-state-pipeline`, 2026-08-22) — same
        // "format on the Rust side, pass a plain string" convention as
        // `screen_sleep_display` above.
        in property <string> battery_display: "—";
        // Screen-lock enable toggle + idle-timeout stepper (screen-lock
        // plan D1/D6) — same apply/persist pattern as the toggles/stepper
        // above, wired through `MenuAction::SetLockEnabled`/
        // `SetLockTimeout` (see `ui::mod::UiRuntime::navigate_to_admin_menu`).
        in property <bool>   lock_enabled: false;
        in property <int>    lock_timeout_s: 300;
        in property <string> lock_timeout_display: "300s";
        // Trackball-driven row highlight: 0=visual toggle, 1=audible toggle,
        // 2=screen-sleep stepper, 3=lock-enable toggle, 4=lock-timeout
        // stepper, 5=GPS status row. `-1` = no highlight yet (touch taps a
        // row directly and never sets this).
        in property <int>    selected_index: -1;
        // Shared full-window starfield texture, set once by Rust right after
        // construction (`ui::backdrop_asset::shared_backdrop_image()`) — see
        // that module's doc for why this isn't a `SpaceBackdrop` default.
        in property <image>  backdrop_image;

        callback back_pressed;
        callback toggle_notif_visual;
        callback toggle_notif_audible;
        callback decrement_screen_sleep_timeout;
        callback increment_screen_sleep_timeout;
        callback toggle_lock_enabled;
        callback decrement_lock_timeout;
        callback increment_lock_timeout;
        callback open_gps_status;

        // Scroll `main_flick` so `selected_index`'s row is in view — same
        // "Rust drives a public function after a property update" pattern
        // `contact_list.rs`'s `scroll_selected_into_view` uses, here at this
        // screen's own uniform 40px row height (every `StepperRow`/
        // `ToggleRow`/`NavRow`/`InfoRow` instance is `height: 40px`) instead
        // of `ContactRow`'s 54px. `selected_index` is 0-based against the
        // SELECTABLE rows only (battery's `InfoRow` has no `selected`
        // property and is never highlighted), so row `i`'s y-offset within
        // the Flickable is `(i + 1) * 40px` — `+1` skips over the
        // non-selectable battery row that always sits above every
        // selectable one.
        public function scroll_selected_into_view() {
            if selected_index < 0 {
                return;
            }
            main_flick.viewport-y = max(
                min(0px, main_flick.height - main_flick.viewport-height),
                -((selected_index + 1) * 40px),
            );
        }

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
        // ceiling is baked into `SpaceBackdrop` itself (see `motifs.slint`).
        // `source` comes from `backdrop_image` above, not a component
        // default — see `backdrop_asset.rs`.
        SpaceBackdrop { source: backdrop_image; }

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
                    // Equalized 4px/8px -> 6px/6px (header-icon-edge-
                    // alignment mission) — see `message_view.rs`'s header
                    // for the shared-inset convention this mirrors.
                    padding-left: 6px;
                    padding-right: 6px;
                    spacing: 4px;

                    Rectangle {
                        width: 44px; height: 36px;
                        Text {
                            text: "‹";
                            font-size: Theme.size-display; // 22px
                            color: Theme.brand-signal;
                            // Flush left (was `center`) so the glyph sits at
                            // the slab's own left edge, the header's true
                            // outer edge once `padding-left` above is
                            // applied.
                            horizontal-alignment: left;
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

            // ── Rows (scrollable — see `scroll_selected_into_view`'s doc) ────
            // Seven rows at 40px each (280px) plus this screen's own 36px
            // header exceed the 320x240 panel (316px total content), so —
            // unlike the pre-screen-lock 5-row layout, which fit at
            // 236/240px with no scrolling needed — this list is wrapped in a
            // `Flickable`, the same mechanism `contact_list.rs`'s row list
            // already uses for the same reason (an unbounded/growing row
            // count on a fixed-height panel). `vertical-stretch: 1.0` lets
            // the Flickable's own viewport claim the remaining height below
            // the fixed 36px header.
            main_flick := Flickable {
                vertical-stretch: 1.0;
                VerticalLayout {
                    // ── Battery (read-only info row) ─────────────────────────
                    InfoRow {
                        label: "🔋  Battery";
                        value: battery_display;
                    }

                    // ── Toggle rows ───────────────────────────────────────────
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
                    // ── Screen lock (plan D1/D6) ─────────────────────────────
                    ToggleRow {
                        label: "🔒  Screen lock";
                        value: lock_enabled;
                        selected: selected_index == 3;
                        toggled => { root.toggle_lock_enabled(); }
                    }
                    StepperRow {
                        label: "⏱  Lock timeout";
                        value: lock_timeout_s;
                        display_text: lock_timeout_display;
                        min_value: 15; // LOCK_TIMEOUT_MIN_S
                        max_value: 3600; // LOCK_TIMEOUT_MAX_S
                        selected: selected_index == 4;
                        decremented => { root.decrement_lock_timeout(); }
                        incremented => { root.increment_lock_timeout(); }
                    }
                    NavRow {
                        label: "📍  GPS status";
                        selected: selected_index == 5;
                        tapped => { root.open_gps_status(); }
                    }
                }
            }
        }
    }
}

/// Step size (seconds) applied per +/- tap on the screen-sleep stepper.
/// Not part of the persisted `RuntimeSettings` contract — purely a UI
/// increment; `pin_menu::apply_menu_action` clamps the result to 0..=120
/// regardless of what step size the widget uses.
const SCREEN_SLEEP_STEP_S: i32 = 5;

/// Step size (seconds) applied per +/- tap on the lock-timeout stepper.
/// `LOCK_TIMEOUT_MIN_S..=LOCK_TIMEOUT_MAX_S` is 15..=3600 — 15s matches the
/// floor exactly (so tapping "-" from the default 300s lands on clean
/// multiples all the way down to the minimum) without being so fine-grained
/// that reaching 3600s takes an unreasonable number of taps.
/// `pin_menu::apply_menu_action` re-clamps the result regardless of this
/// widget-only step size, same discipline as `SCREEN_SLEEP_STEP_S`.
const LOCK_TIMEOUT_STEP_S: i32 = 15;

// `format_screen_sleep`/`format_lock_timeout`/`format_battery_display` are
// pure Rust with no Slint dependency — they now live in
// `firmware_core::ui::admin_menu` so their tests execute under `cargo test
// --workspace` (this crate is a detached, cross-compiled workspace — see
// `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block written here would
// type-check but never run). Only this Slint-backed view wrapper stays.
// `pub(crate) use` preserves `format_battery_display`'s original
// crate-visible re-export (also called from
// `ui::mod::UiRuntime::set_battery_status`). See
// `docs/adr/0005-firmware-core-extraction.md`.
use firmware_core::ui::admin_menu::{format_lock_timeout, format_screen_sleep};
pub(crate) use firmware_core::ui::admin_menu::format_battery_display;

/// Rust-side wrapper.
pub struct AdminMenuScreen {
    component: self::AdminMenuScreenUi,
}

impl AdminMenuScreen {
    pub fn new() -> anyhow::Result<Self> {
        let component = self::AdminMenuScreenUi::new()
            .map_err(|e| anyhow::anyhow!("slint component init: {:?}", e))?;
        component.set_backdrop_image(crate::ui::backdrop_asset::shared_backdrop_image());
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

    /// Set the initial displayed state of the screen-lock enable toggle
    /// (plan D6 `LOCK_SCREEN_ENABLE`, `lock_flags` bit 0).
    pub fn set_lock_enabled(&self, v: bool) {
        self.component.set_lock_enabled(v);
    }

    /// Set the initial displayed lock idle-timeout
    /// (`LOCK_TIMEOUT_MIN_S..=LOCK_TIMEOUT_MAX_S`, 15..=3600s; plan D1).
    /// Same "raw value + precomputed display string" pair as
    /// [`Self::set_screen_sleep_timeout`].
    pub fn set_lock_timeout(&self, seconds: i32) {
        self.component.set_lock_timeout_s(seconds);
        self.component.set_lock_timeout_display(format_lock_timeout(seconds).into());
    }

    /// Set the displayed battery row — the full HIL-capture state vector,
    /// precomputed Rust-side by [`format_battery_display`] (see that
    /// function's own doc for the exact layout) from the shared
    /// `battery::BatteryStatus` snapshot — the same source the host
    /// `status` command and the radio telemetry RESPONSE read (single
    /// shared source; see the firmware `battery` module docs).
    pub fn set_battery_display(&self, text: &str) {
        self.component.set_battery_display(text.into());
    }

    pub fn on_back_pressed(&self, cb: impl Fn() + 'static) {
        self.component.on_back_pressed(cb);
    }

    /// Move the trackball highlight to row `idx` (0..=5; see
    /// `AdminMenuScreenUi.selected_index`'s doc for the row mapping; `-1`
    /// clears it) and scroll it into view — same
    /// set-property-then-invoke-the-scroll-function pattern
    /// `ContactListScreen::set_selected_index` uses. The caller
    /// (`UiRuntime::handle_trackball_admin_menu`) owns clamping `idx` to the
    /// row count.
    pub fn set_selected_index(&self, idx: i32) {
        self.component.set_selected_index(idx);
        self.component.invoke_scroll_selected_into_view();
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

    /// Fire `toggle_lock_enabled` exactly as tapping that row would — used
    /// by the trackball's Click handler when row 3 (lock-enable toggle) is
    /// highlighted.
    pub fn invoke_toggle_lock_enabled(&self) {
        self.component.invoke_toggle_lock_enabled();
    }

    /// Fire `increment_lock_timeout` exactly as tapping the lock-timeout
    /// stepper's "+" would — used by the trackball's Click handler when row
    /// 4 is highlighted. Same "Click maps to increment" convention as
    /// [`Self::invoke_increment_screen_sleep_timeout`].
    pub fn invoke_increment_lock_timeout(&self) {
        self.component.invoke_increment_lock_timeout();
    }

    /// Fire `open_gps_status` exactly as tapping that row would — used by the
    /// trackball's Click handler when row 5 is highlighted.
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

    /// Fires `cb(new_value)` when the user taps the screen-lock enable row.
    /// See [`Self::on_toggle_notif_visual`] for the displayed-state contract.
    pub fn on_toggle_lock_enabled(&self, cb: impl Fn(bool) + 'static) {
        let comp = self.component.clone_strong();
        self.component.on_toggle_lock_enabled(move || {
            let new_val = !comp.get_lock_enabled();
            comp.set_lock_enabled(new_val);
            cb(new_val);
        });
    }

    /// Fires `cb(new_seconds)` when the user taps "−" on the lock-timeout
    /// row. Clamped to a floor of `LOCK_TIMEOUT_MIN_S` here for the widget's
    /// own displayed-state consistency; the caller
    /// (`ui::mod::navigate_to_admin_menu`) applies `new_seconds` via
    /// `pin_menu::apply_menu_action`, which re-clamps to
    /// `LOCK_TIMEOUT_MIN_S..=LOCK_TIMEOUT_MAX_S` as the single source of
    /// truth for the persisted invariant — same division of labor as
    /// [`Self::on_decrement_screen_sleep_timeout`].
    pub fn on_decrement_lock_timeout(&self, cb: impl Fn(i32) + 'static) {
        let comp = self.component.clone_strong();
        self.component.on_decrement_lock_timeout(move || {
            let new_val =
                (comp.get_lock_timeout_s() - LOCK_TIMEOUT_STEP_S).max(protocol::provisioning::LOCK_TIMEOUT_MIN_S as i32);
            comp.set_lock_timeout_s(new_val);
            comp.set_lock_timeout_display(format_lock_timeout(new_val).into());
            cb(new_val);
        });
    }

    /// Fires `cb(new_seconds)` when the user taps "+" on the lock-timeout
    /// row. See [`Self::on_decrement_lock_timeout`] for the clamp contract.
    pub fn on_increment_lock_timeout(&self, cb: impl Fn(i32) + 'static) {
        let comp = self.component.clone_strong();
        self.component.on_increment_lock_timeout(move || {
            let new_val =
                (comp.get_lock_timeout_s() + LOCK_TIMEOUT_STEP_S).min(protocol::provisioning::LOCK_TIMEOUT_MAX_S as i32);
            comp.set_lock_timeout_s(new_val);
            comp.set_lock_timeout_display(format_lock_timeout(new_val).into());
            cb(new_val);
        });
    }

    /// Fires `cb()` when the user taps the "📍 GPS status" row. The caller
    /// navigates to the read-only [`super::gps_status::GpsStatusScreen`]
    /// sub-screen — no state to flip here (this row is pure navigation).
    pub fn on_open_gps_status(&self, cb: impl Fn() + 'static) {
        self.component.on_open_gps_status(cb);
    }

    /// Re-attach this (already-constructed) component as the window's
    /// current one — see `message_view.rs`'s identical `show()` doc for the
    /// screen-lock D3 retained-overlay rationale.
    pub fn show(&self) { self.component.show().ok(); }
    pub fn hide(&self) { self.component.hide().ok(); }
}

// `format_screen_sleep`/`format_battery_display`'s tests moved to
// `firmware-core/src/ui/admin_menu.rs` alongside the functions — see this
// file's module-level move note above.
