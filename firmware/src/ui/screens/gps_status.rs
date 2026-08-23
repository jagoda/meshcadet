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
//! 4. Time-sync state — when synced, the actual GPS-synced wall-clock
//!    date+time on its own row (`"2026-07-15 14:32:10 UTC"`) plus how long
//!    ago the sync happened on a second row underneath (`"synced 5s ago"`);
//!    `"Not synced"` (no second row) if the system clock has never been set
//!    from a valid GPS date+time sentence since boot. The ESP32-S3 itself
//!    has no battery-backed RTC, so this still resets to "not synced" every
//!    power-off from the SoC's own perspective — but the GPS shield's GNSS
//!    module DOES carry one, so a sync from its pre-fix, RTC-derived
//!    sentence typically lands within seconds of boot rather than only once
//!    a real fix is acquired — see `gps::GpsDriver`'s module doc's "Clock
//!    sync" section. Two rows,
//!    not one: a single line wide enough to hold both the absolute date+time
//!    and the relative age overflowed off the T-Deck's 320px-wide display
//!    (see git history for the one-line version this replaced). The full
//!    date INCLUDING YEAR on row one is load-bearing — see
//!    `firmware_core::ui::gps_status::format_time_sync_date`'s doc for why
//!    it must never be trimmed.
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
//! only ever touch the five `*_text` string properties, never
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
//! entirely inside the `slint!{}` block below.
//!
//! `StatusRow` also grew two more optional properties for the Time-sync
//! row's overflow fix (see the "Time-sync state" item above):
//! `value2` (default `""`, hidden when empty) renders a second, smaller,
//! secondary-styled line under `value` — every OTHER row leaves it unset
//! and renders byte-identical to before; and `row-height` (default `48px`,
//! matching the previous hardcoded literal) lets the one row that needs a
//! third line claim more vertical space without changing anyone else's.
//! Both are plain properties consumed entirely inside the `slint!{}` block
//! below — the Rust-side `GpsStatusScreen` wrapper and `set_status` gained
//! one new field push (`time_sync_age_text`) but are otherwise untouched.

slint::slint! {
    import { Theme } from "../theme.slint";
    import { RingedPlanetCorner, Comet, SpaceBackdrop } from "../motifs.slint";
    import { SignalMeter } from "../signal_meter.slint";
    import { BatteryIndicator } from "../battery_indicator.slint";

    component StatusRow {
        in property <string> label;
        in property <string> value;
        // Optional second value line — see module doc's `StatusRow`
        // paragraph. "" (the default) renders NO second line at all, so
        // rows that don't opt in keep their prior layout exactly.
        in property <string> value2: "";
        // Optional per-row motif badge — see module doc's "Outer-space
        // theme" section. Selects which shared `ui/motifs.slint` component
        // (if any) fills the icon column; "none" (the default) reserves NO
        // icon column, so rows that don't opt in keep their prior
        // layout exactly.
        in property <string> icon-kind: "none"; // "none" | "planet" | "comet"
        // Row height override — see module doc's `StatusRow` paragraph.
        // 48px (the default) is the previous hardcoded literal, unchanged
        // for every row that doesn't opt into a taller `value2` line.
        in property <length> row-height: 48px;

        height: row-height;

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
                    // 1px (was 2px): frees just enough vertical room for the
                    // Time-sync row's third (`value2`) line to fit inside
                    // this screen's fixed 240px height without pushing
                    // anything below it off-screen — see `row-height`'s doc.
                    // Harmless for every other row: measured against the
                    // real theme/fonts via `ui_sim::gps_status_rows`, this
                    // only ever *adds* headroom to their existing (already
                    // comfortable) margins.
                    spacing: 1px;

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

                    // Second value line — see `value2`'s doc above. Styled
                    // like a secondary caption (smaller, dimmer) rather than
                    // matching `value`'s primary styling: it's supplementary
                    // ("how long ago"), the row above it is the fact that
                    // matters ("what the wall clock actually reads"). Same
                    // size/color pairing `message_view.rs` uses for its own
                    // timestamp caption.
                    if value2 != "" : Text {
                        text: value2;
                        font-size: Theme.size-caption; // 9px
                        color: Theme.text-secondary;
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
        // Row 1 of the Time-sync row: the absolute wall clock (full
        // date+time incl. year, e.g. "2026-07-15 14:32:10 UTC") or
        // "Not synced". See module doc's "Time-sync state" item for why
        // this is now two rows instead of one over-wide line.
        in property <string> time_sync_text: "Not synced";
        // Row 2 of the Time-sync row: the relative age ("synced 5s ago"), or
        // "" (renders no second row at all) when never synced — mirrors
        // `time_sync_text`'s "Not synced"/populated pairing 1:1.
        in property <string> time_sync_age_text: "";
        // Time-sync row LABEL — `meshcadet-room-clock-ux`: was a static
        // "Time sync" literal on the row instantiation below; now driven by
        // `GpsStatusScreen::set_clock_source` so the row reads "GPS time" /
        // "GPS RTC" / "Room time" / "Time sync" depending on which clock
        // source is currently in effect (`firmware_core::ui::gps_status::
        // format_clock_source_label`'s doc explains why this answers "why
        // does this say no fix but the time is right?" — "GPS RTC"
        // additionally answers "why does this say GPS time with no fix?",
        // `meshcadet-clock-source-provenance-and-sync-age`'s Objective).
        in property <string> time_sync_label_text: "Time sync";
        // Repeater signal-meter reading (ADR-0010): 0 = direct-only,
        // 1..=5 = bars. Pushed by `GpsStatusScreen::set_signal_level`
        // (`UiRuntime::set_signal_level` in `ui/mod.rs`); see
        // `SignalMeter`'s embedding below.
        in property <int> signal_level: 0;
        // Coarse battery-level bucket (`meshcadet-battery-soc-filtering` /
        // `meshcadet-battery-glanceable-indicator`): 0 = Unknown, 1 =
        // Charging, 2..=4 = Low/Partial/Full. Pushed by
        // `GpsStatusScreen::set_battery_level` (`UiRuntime::set_battery_level`
        // in `ui/mod.rs`); see `BatteryIndicator`'s embedding below.
        in property <int> battery_level: 0;
        // Shared full-window starfield texture, set once by Rust right after
        // construction (`ui::backdrop_asset::shared_backdrop_image()`) — see
        // that module's doc for why this isn't a `SpaceBackdrop` default.
        in property <image> backdrop_image;

        callback back_pressed;

        // ── One-shot screen-entry reveal — see module doc ───────────────────
        in-out property <float> reveal_opacity: 0;
        animate reveal_opacity { duration: 200ms; easing: ease-out; }
        init => { self.reveal_opacity = 1.0; }

        // Full-window dim starfield backdrop — declared first so it paints
        // behind every other node; the ≤0.35 alpha ceiling is baked into
        // `SpaceBackdrop` itself (see `motifs.slint`). `source` comes from
        // `backdrop_image` above, not a component default — see
        // `backdrop_asset.rs`.
        SpaceBackdrop { source: backdrop_image; }

        VerticalLayout {
            spacing: 0px;
            opacity: reveal_opacity;

            // ── Header bar ──────────────────────────────────────────────────
            Rectangle {
                height: 36px;
                background: Theme.surface;
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
                            // the slab's own left edge — the header's true
                            // outer edge once `padding-left` above is
                            // applied — instead of ~22px in from it.
                            horizontal-alignment: left;
                            vertical-alignment: center;
                        }
                        TouchArea { clicked => { root.back_pressed(); } }
                    }

                    // Split into two Text elements (mono-glyph-legibility
                    // mission) so the 📍 glyph can carry its own color
                    // (`Theme.ok` — green, this mission's "location" choice)
                    // WITHOUT recoloring "GPS Status", which stays
                    // `Theme.text-primary` to match every other screen's
                    // header-title convention. A single merged Text/color
                    // pair (the previous shape) would have made the whole
                    // title green instead of just the icon — Slint has no
                    // per-run color, so this is the only way to color one
                    // glyph inside what used to be one string. `alignment:
                    // center` + `horizontal-stretch: 1.0` on the wrapping
                    // layout reproduce the prior Text's own centering.
                    HorizontalLayout {
                        horizontal-stretch: 1.0;
                        alignment: center;
                        spacing: 4px;

                        Text {
                            text: "📍";
                            // BUG FIX:
                            // was 15px. 15 is a valid PIXEL_SIZES entry but is
                            // NOT in `EMOJI_SIZES` (`gen_emoji_font.c`), so the
                            // 📍 glyph rasterised as an empty (blank) bitmap at
                            // this size — the exact "silent blank icon" failure
                            // mode this file's own SYNC INVARIANT comments
                            // document, caught by the host glyph-coverage
                            // harness (`xtask`). `Theme.size-body-lg` (14px) IS
                            // in EMOJI_SIZES and matches the header-title
                            // convention used elsewhere (e.g. message_view.rs's
                            // contact-name header, also 14px).
                            font-size: Theme.size-body-lg;
                            font-weight: 600;
                            color: Theme.ok;
                            vertical-alignment: center;
                        }

                        Text {
                            text: "GPS Status";
                            font-size: Theme.size-body-lg;
                            font-weight: 600;
                            color: Theme.text-primary;
                            vertical-alignment: center;
                        }
                    }

                    // Balance the back button's width so the title stays
                    // centered — same 44px spacer as before. The
                    // `SignalMeter` (ADR-0010) nests INSIDE it, right-aligned
                    // with a small margin, rather than adding a new flow
                    // sibling: nesting keeps the spacer's own reserved width
                    // (and so the title's centering) byte-identical, and this
                    // is the one operational-screen header with an otherwise
                    // completely empty top-right corner — no existing motif
                    // or touch target to avoid here (unlike message_view.rs's
                    // header, which already carries a static `Comet` in this
                    // same zone). `BatteryIndicator`
                    // (`meshcadet-battery-glanceable-indicator`) nests
                    // immediately to the SignalMeter's LEFT, inside the same
                    // 44px spacer — this screen's top-right corner has 44px
                    // of reserved width and the meter+nub only needs ~24px of
                    // it, so both widgets fit with no spacer resize needed
                    // (unlike `message_view.rs`, whose spacer this same
                    // change DOES widen — see that screen's own comment for
                    // why).
                    Rectangle {
                        width: 44px; height: 36px;
                        // Flush right (was a 4px baked-in margin) — the
                        // uniform 6px edge inset now comes solely from the
                        // header's own `padding-right` above, matching every
                        // other in-scope screen's convention (header-icon-
                        // edge-alignment mission).
                        SignalMeter {
                            signal-level: root.signal_level;
                            width: 16px;
                            height: 14px;
                            x: parent.width - self.width;
                            y: (parent.height - self.height) / 2;
                        }
                        BatteryIndicator {
                            battery-level: root.battery_level;
                            width: 14px;
                            height: 9px;
                            // Left of the SignalMeter (16px) with a 3px gap.
                            x: parent.width - 16px - 3px - 14px;
                            y: (parent.height - self.height) / 2;
                        }
                    }
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
                label: time_sync_label_text;
                value: time_sync_text;
                value2: time_sync_age_text;
                // 60px (was the 48px default): the only row wide enough in
                // content to need it — three lines (label / absolute
                // date+time / relative age) instead of two. Exactly the
                // remaining budget on this 240px-tall, 36px-header screen
                // (36 + 48*3 + 60 == 240) — measured against the real
                // theme/fonts via `ui_sim::gps_status_rows` before landing;
                // see that module's doc for the render this claim is proven
                // against.
                row-height: 60px;
            }

            Rectangle { vertical-stretch: 1.0; }
        }
    }
}

// Display-string formatting (`format_fix_state`/`format_sat_count`/
// `format_coords`/`format_time_sync_date`/`format_time_sync_age`) is pure
// Rust with no Slint dependency — it now lives in `firmware_core::ui::
// gps_status` so its tests execute under `cargo test --workspace` (this
// crate is a detached, cross-compiled workspace — see `Cargo.toml`'s doc
// comment — so a `#[cfg(test)]` block written here would type-check but
// never run). Only this Slint-backed view wrapper stays. See
// `docs/adr/0005-firmware-core-extraction.md`.
use firmware_core::ui::gps_status::{
    format_clock_source_label, format_coords, format_fix_state, format_sat_count,
    format_time_sync_age, format_time_sync_date,
};

/// Rust-side wrapper.
pub struct GpsStatusScreen {
    component: self::GpsStatusScreenUi,
}

impl GpsStatusScreen {
    pub fn new() -> anyhow::Result<Self> {
        let component = self::GpsStatusScreenUi::new()
            .map_err(|e| anyhow::anyhow!("slint component init: {:?}", e))?;
        component.set_backdrop_image(crate::ui::backdrop_asset::shared_backdrop_image());
        component.show()
            .map_err(|e| anyhow::anyhow!("slint window show: {:?}", e))?;
        Ok(GpsStatusScreen { component })
    }

    /// Push a fresh GPS status snapshot into the Fix/Satellites/Coordinates
    /// rows. Safe to call repeatedly while the screen is open (e.g. every
    /// `step()`) so the fix age ticks upward live rather than freezing at
    /// nav-open time.
    ///
    /// Does NOT touch the Time-sync row — see [`Self::set_clock_source`].
    /// Before `meshcadet-room-clock-ux` this method also drove that row
    /// directly off `status.clock_unix_secs`/`clock_sync_age_secs`, which
    /// is GPS-only and so stayed "Not synced" forever on a GPS-denied
    /// device even once a room server's clock had been adopted — exactly
    /// the "why does this say no fix but the time is right?" gap that
    /// split call now closes.
    pub fn set_status(&self, status: &crate::gps::GpsStatus) {
        self.component.set_fix_state_text(format_fix_state(status.fix_state).into());
        self.component.set_sat_count_text(format_sat_count(status.sat_count).into());
        self.component.set_coords_text(
            format_coords(status.has_fix, status.lat_e7, status.lon_e7, status.fix_age_secs).into(),
        );
    }

    /// Push a fresh room-clock-provenance snapshot into the Time-sync row —
    /// `meshcadet-room-clock-ux`'s Objective item 3. `source` picks the
    /// row's label (`format_clock_source_label`: "GPS time" / "GPS RTC" /
    /// "Room time" / "Time sync" — the `GpsUnverified` variant added by
    /// `meshcadet-clock-source-provenance-and-sync-age`); `unix_secs`/
    /// `age_secs` are the SAME combined "whichever clock is trusted right
    /// now" reading `room_session::trusted_wall_clock_secs` produces
    /// (verified GPS first, then an adopted room-server clock, then an
    /// unverified GPS sync, else `None` — see that function's doc for the
    /// full priority order) — not raw `GpsStatus` fields, so the row stays
    /// populated on a GPS-denied device once a room server's clock has been
    /// adopted.
    ///
    /// Safe to call repeatedly while the screen is open, same "cheap no-op
    /// on an unchanged reading" caller discipline `UiRuntime::set_room_
    /// clock_source` applies before ever reaching here.
    pub fn set_clock_source(
        &self,
        source: crate::room_session::ClockSource,
        unix_secs: Option<u32>,
        age_secs: u32,
    ) {
        self.component
            .set_time_sync_label_text(format_clock_source_label(source).into());
        self.component
            .set_time_sync_text(format_time_sync_date(unix_secs).into());
        self.component
            .set_time_sync_age_text(format_time_sync_age(unix_secs, age_secs).into());
    }

    /// Push a fresh repeater signal-meter reading (ADR-0010) into the
    /// header's `SignalMeter`. `bars` is `0` (direct-only) or `1..=5`
    /// (`firmware_core::ui::signal_meter::level_to_bars`'s output) — the
    /// caller (`UiRuntime::set_signal_level`) owns the `SignalLevel` ->
    /// `int` conversion so every screen's wrapper takes the same plain type.
    pub fn set_signal_level(&self, bars: i32) {
        self.component.set_signal_level(bars);
    }

    /// Push a fresh coarse battery-level bucket into the header's
    /// `BatteryIndicator` (`meshcadet-battery-soc-filtering` /
    /// `meshcadet-battery-glanceable-indicator`). `level` is `0` (Unknown),
    /// `1` (Charging), or `2..=4` (Low/Partial/Full —
    /// `firmware_core::ui::battery_indicator::level_to_indicator_level`'s
    /// output) — the caller (`UiRuntime::set_battery_level`) owns the
    /// `BatteryLevel` -> `int` conversion so every screen's wrapper takes the
    /// same plain type, mirroring `set_signal_level`'s identical contract
    /// above.
    pub fn set_battery_level(&self, level: i32) {
        self.component.set_battery_level(level);
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

    /// Re-attach this (already-constructed) component as the window's
    /// current one — see `message_view.rs`'s identical `show()` doc for the
    /// screen-lock D3 retained-overlay rationale.
    pub fn show(&self) { self.component.show().ok(); }
    pub fn hide(&self) { self.component.hide().ok(); }
}
