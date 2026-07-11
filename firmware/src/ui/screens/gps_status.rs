// SPDX-License-Identifier: GPL-3.0-only
//! GPS status screen — read-only view reachable from the admin settings menu.
//!
//! Shows four facts about the GPS subsystem (charter scope: status/display
//! only, **no controls**):
//!
//! 1. Fix state — three-way (`gps::FixState`): `NoSignal` (nothing heard from
//!    the module — hardware/wiring suspect), `Acquiring` (receiver alive,
//!    searching, no fix yet), or `Fix` (a fix has been captured since boot).
//!    Replaces a plain has-fix boolean specifically so a genuinely dead GPS
//!    module doesn't look identical to "still acquiring, give it a minute".
//! 2. Satellite count — satellites used in the most recent GGA sentence,
//!    shown regardless of fix state so the admin can see acquisition
//!    progress ("4 satellites") even before a fix lands.
//! 3. Coordinates + age — the cached fix's lat/lon and how many seconds old
//!    it is (the driver never discards a stale fix; staleness is surfaced,
//!    not hidden — mirrors `gps::GpsDriver::get_fix_and_age`'s doc contract).
//! 4. Time-sync state — whether the system clock has been set from a valid
//!    GPS date+time sentence since boot (the T-Deck Plus has no
//!    battery-backed RTC, so this resets to "not synced" every power-off).
//!
//! All display strings are formatted Rust-side (`firmware_core::ui::
//! gps_status::format_*`, imported below) and passed to Slint as plain text —
//! the same convention used throughout this UI (e.g.
//! `admin_menu::format_screen_sleep`).
//!
//! # Theme tokens + one-shot animation language
//!
//! Every color/font-size literal in this screen's `slint::slint!{}` block
//! (below) now reads from the shared `Theme` global (`ui/theme.slint`,
//! imported below) at the SAME values — a pixel-identical swap, same pattern
//! as `splash.rs`'s Phase-1 pilot and `compose.rs`'s Phase-5 application.
//! This is also where the two BUG FIXes below (12px label / 15px header-icon)
//! now live permanently: `Theme.size-body` (13px) and `Theme.size-body-lg`
//! (14px) are the only names either literal can be expressed as, so the
//! contract itself now prevents either regressing back to an unregistered or
//! blank-glyph size (see `theme.slint`'s own doc on why every `size-*`/
//! `icon-*` token is, by construction, a member of `PIXEL_SIZES`).
//!
//! A single one-shot screen-entry fade applies this UI's "never an
//! infinite loop, never cut off mid-cycle" animation language: `GpsStatusScreen
//! ::new()` builds a fresh component on every navigation here (mirrors
//! `ComposeScreen`/`EmojiPickerGrid` — reached by interactive navigation, not
//! boot, so there is no splash-style deferred-start gap to work around), so
//! the `init` handler below fires exactly once per mount and its single write
//! to `reveal_opacity`'s settled value is what fires the `animate` transition
//! — same self-contained deferred-write mechanism as `compose.rs`'s
//! `EmojiPickerGrid` reveal. Live status updates (`set_status`, called every
//! dispatcher-loop tick while this screen is open — see that method's doc)
//! only ever touch the four `*_text` string properties, never
//! `reveal_opacity`, so the tick-driven age refreshes never re-fire this
//! transition.
//!
//! # Outer-space theme (per-screen spec row 8: "Planet/orbit motif
//! for location, comet for signal")
//!
//! Two additive, presentation-only motif placements on top of the palette
//! wiring above — both reused as-is from the shared `ui/motifs.slint`
//! contract; no new asset is
//! authored here:
//! - `RingedPlanetCorner` (scaled down from its 40x40 default) sits in the
//!   icon column of the **Coordinates** row — the one row on this screen
//!   about *where* the device is, matching the plan's "location" assignment.
//! - `Comet` (scaled down from its 28x14 default) sits in the icon column of
//!   the **Fix** row — the row that most directly reads as GPS *signal*
//!   state (`No signal` / `Acquiring...` / `Fix acquired`), matching the
//!   plan's "signal" assignment. This is the STATIC `Comet` wrapper, not the
//!   retriggerable `CometOnNotify` motion helper: gps_status has no discrete
//!   "new signal" event to trigger off (fix state free-runs off
//!   `set_status`'s tick-driven pushes, not a one-shot arrival), and the
//!   design's motion-language list does not name gps_status among the
//!   animated screens — so this motif is a static badge, not a new
//!   interaction affordance.
//!
//! `StatusRow` grew an optional `icon-kind` string selector (`"none"` by
//! default, so the `Satellites`/`Time sync` rows render byte-identical to
//! before this change) rather than forking a second row component; the two
//! themed rows above set it to `"planet"`/`"comet"` to pick which shared
//! motif fills their icon column. It is a plain `string` property consumed
//! entirely inside the `slint!{}` block below — the Rust-side
//! `GpsStatusScreen` wrapper and `set_status` are untouched.

slint::slint! {
    import { Theme } from "../theme.slint";
    import { RingedPlanetCorner, Comet, SpaceBackdrop } from "../motifs.slint";

    component StatusRow {
        in property <string> label;
        in property <string> value;
        // Optional per-row motif badge — see module doc's "Outer-space
        // theme" section. Selects which shared `ui/motifs.slint` component
        // (if any) fills the icon column; "none" (the default) reserves NO
        // icon column, so rows that don't opt in keep their prior
        // layout exactly.
        in property <string> icon-kind: "none"; // "none" | "planet" | "comet"

        height: 48px;

        Rectangle {
            background: transparent;

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
                padding-top: 4px;
                padding-bottom: 4px;
                spacing: 8px;

                if icon-kind == "planet" : Rectangle {
                    width: 22px;
                    RingedPlanetCorner {
                        width: 22px;
                        height: 22px;
                        y: (parent.height - self.height) / 2;
                    }
                }

                if icon-kind == "comet" : Rectangle {
                    width: 22px;
                    Comet {
                        width: 22px;
                        height: 11px;
                        y: (parent.height - self.height) / 2;
                    }
                }

                VerticalLayout {
                    horizontal-stretch: 1.0;
                    spacing: 2px;

                    Text {
                        text: label;
                        // BUG FIX: was
                        // 12px, not a member of `PIXEL_SIZES` in
                        // `gen_emoji_font.c` — the Slint software renderer snaps
                        // an unregistered size to the nearest registered one
                        // (11 or 13) and rescales the glyph metrics, producing
                        // garbled text. `Theme.size-body` (13px) IS registered.
                        font-size: Theme.size-body;
                        color: Theme.text-secondary;
                    }

                    Text {
                        text: value;
                        font-size: Theme.size-subtitle; // 15px
                        color: Theme.text-primary;
                    }
                }
            }
        }
    }

    export component GpsStatusScreenUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        in property <string> fix_state_text: "No signal";
        in property <string> sat_count_text: "0 satellites";
        in property <string> coords_text: "\u{2014}";
        in property <string> time_sync_text: "Not synced";

        callback back_pressed;

        // ── One-shot screen-entry reveal — see module doc ───────────────────
        in-out property <float> reveal_opacity: 0;
        animate reveal_opacity { duration: 200ms; easing: ease-out; }
        init => { self.reveal_opacity = 1.0; }

        // Full-window dim starfield backdrop — declared first so it paints
        // behind every other node; the ≤0.35 alpha ceiling is baked into
        // `SpaceBackdrop` itself (see `motifs.slint`), not overridden here.
        SpaceBackdrop {}

        VerticalLayout {
            spacing: 0px;
            opacity: reveal_opacity;

            // ── Header bar ──────────────────────────────────────────────────
            Rectangle {
                height: 36px;
                background: Theme.surface;
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
                        text: "📍 GPS Status";
                        // BUG FIX:
                        // was 15px. 15 is a valid PIXEL_SIZES entry but is NOT
                        // in `EMOJI_SIZES` (`gen_emoji_font.c`), so the 📍
                        // glyph rasterised as an empty (blank) bitmap at this
                        // size — the exact "silent blank icon" failure mode
                        // this file's own SYNC INVARIANT comments document,
                        // caught by the host glyph-coverage harness (`xtask`).
                        // `Theme.size-body-lg` (14px) IS in EMOJI_SIZES and
                        // matches the header-title convention used elsewhere
                        // (e.g. message_view.rs's contact-name header, also
                        // 14px).
                        font-size: Theme.size-body-lg;
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

            // ── Status rows (read-only — no controls) ────────────────────────
            StatusRow {
                label: "Fix";
                value: fix_state_text;
                // Comet = signal motif (see module doc) — this is the row
                // that most directly reads as GPS signal state.
                icon-kind: "comet";
            }
            StatusRow {
                label: "Satellites";
                value: sat_count_text;
            }
            StatusRow {
                label: "Coordinates";
                value: coords_text;
                // RingedPlanetCorner = location motif (see module doc) —
                // this is the row that reads as *where* the device is.
                icon-kind: "planet";
            }
            StatusRow {
                label: "Time sync";
                value: time_sync_text;
            }

            Rectangle { vertical-stretch: 1.0; }
        }
    }
}

// Display-string formatting (`format_fix_state`/`format_sat_count`/
// `format_coords`/`format_time_sync`) is pure Rust with no Slint dependency —
// it now lives in `firmware_core::ui::gps_status` so its tests execute under
// `cargo test --workspace` (this crate is a detached, cross-compiled
// workspace — see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block
// written here would type-check but never run). Only this Slint-backed view
// wrapper stays. See `docs/adr/0005-firmware-core-extraction.md`.
use firmware_core::ui::gps_status::{format_coords, format_fix_state, format_sat_count, format_time_sync};

/// Rust-side wrapper.
pub struct GpsStatusScreen {
    component: self::GpsStatusScreenUi,
}

impl GpsStatusScreen {
    pub fn new() -> anyhow::Result<Self> {
        let component = self::GpsStatusScreenUi::new()
            .map_err(|e| anyhow::anyhow!("slint component init: {:?}", e))?;
        component.show()
            .map_err(|e| anyhow::anyhow!("slint window show: {:?}", e))?;
        Ok(GpsStatusScreen { component })
    }

    /// Push a fresh GPS status snapshot into the three display rows. Safe to
    /// call repeatedly while the screen is open (e.g. every `step()`) so the
    /// fix/sync ages tick upward live rather than freezing at nav-open time.
    pub fn set_status(&self, status: &crate::gps::GpsStatus) {
        self.component.set_fix_state_text(format_fix_state(status.fix_state).into());
        self.component.set_sat_count_text(format_sat_count(status.sat_count).into());
        self.component.set_coords_text(
            format_coords(status.has_fix, status.lat_e7, status.lon_e7, status.fix_age_secs).into(),
        );
        self.component.set_time_sync_text(
            format_time_sync(status.clock_synced, status.clock_sync_age_secs).into(),
        );
    }

    pub fn on_back_pressed(&self, cb: impl Fn() + 'static) {
        self.component.on_back_pressed(cb);
    }

    /// Fire `back_pressed` exactly as the header's back button would — used
    /// by the trackball's roll-Left handler (this read-only screen has no
    /// other trackball job — see `UiRuntime::handle_trackball_event`).
    pub fn invoke_back_pressed(&self) {
        self.component.invoke_back_pressed();
    }

    pub fn hide(&self) { self.component.hide().ok(); }
}
