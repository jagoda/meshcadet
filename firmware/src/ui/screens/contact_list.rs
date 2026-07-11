// SPDX-License-Identifier: GPL-3.0-only
//! Contact / channel list screen.
//!
//! The primary home screen after provisioning.  Shows two tabs:
//! - **Contacts** — direct message peers (admin-provisioned, sorted by
//!   last-message recency)
//! - **Channels** — group channels (admin-provisioned)
//!
//! Tapping a contact or channel navigates to [`MessageViewScreen`].
//!
//! Each contact row shows:
//! - Contact display name (provisioned)
//! - Unread badge (count of unread messages)
//! - Last message preview (truncated to ~30 chars)
//! - Relative timestamp ("2m ago", "1h ago")
//!
//! # Full-window starfield backdrop
//!
//! `SpaceBackdrop` (`ui/motifs.slint`) is now the `Window`'s first
//! (z-bottom) child, reversing the earlier "scrolling-content screens
//! excluded from backdrop" call — the same
//! full-window dim starfield `admin_menu.rs` already carries. No row/header
//! alpha tuning was needed here: this screen's tab-bar header
//! (`Theme.surface.with-alpha(0.55)`) and unselected `ContactRow` fill
//! (`Theme.nebula-violet-deep.with-alpha(0.12)`) already predate this
//! change and are already translucent, so the backdrop shows through both
//! without further changes — this screen was only missing the shared
//! full-window layer underneath.

slint::slint! {
    import { Theme } from "../theme.slint";
    import { Starfield, CometOnNotify, SpaceBackdrop } from "../motifs.slint";

    // A single row in the contact list.
    component ContactRow {
        in property <string>  name;
        in property <string>  initial;
        in property <string>  preview;
        in property <string>  time_str;
        in property <int>     unread;
        in property <string>  unread_str;
        in property <bool>    selected;
        callback clicked;

        height: 54px;

        // Rest-state fill is a subtle widened-palette wash —
        // `nebula-violet-deep` at low
        // alpha over the `bg-space` window backdrop, distinguishing rows
        // from the surrounding chrome without touching the
        // selected/hover states' existing, already-legible tokens.
        Rectangle {
            background: selected ? Theme.surface-raised : (touch_area.has-hover ? Theme.surface : Theme.nebula-violet-deep.with-alpha(0.12));
            border-radius: 0px;

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
                padding-top: 8px;
                padding-bottom: 8px;
                spacing: 8px;

                // Avatar circle (first letter of name)
                Rectangle {
                    width: 36px;
                    height: 36px;
                    border-radius: 18px;
                    background: Theme.select;

                    Text {
                        text: initial;
                        font-size: Theme.size-title;
                        font-weight: 700;
                        color: Theme.brand-signal-bright;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }
                }

                VerticalLayout {
                    spacing: 2px;
                    alignment: center;

                    HorizontalLayout {
                        Text {
                            text: name;
                            font-size: Theme.size-body-lg;
                            font-weight: 600;
                            color: Theme.text-primary;
                            horizontal-stretch: 1.0;
                        }
                        Text {
                            text: time_str;
                            font-size: Theme.size-meta;
                            color: Theme.text-secondary;
                        }
                    }
                    HorizontalLayout {
                        Text {
                            text: preview;
                            font-size: Theme.size-preview;
                            color: Theme.text-secondary;
                            horizontal-stretch: 1.0;
                            overflow: elide;
                        }
                        if unread > 0 : Rectangle {
                            width: 20px;
                            height: 20px;
                            border-radius: 10px;
                            background: Theme.brand-signal;
                            Text {
                                text: unread_str;
                                font-size: Theme.size-caption;
                                font-weight: 700;
                                color: Theme.text-primary;
                                horizontal-alignment: center;
                                vertical-alignment: center;
                            }
                        }
                    }
                }
            }

            touch_area := TouchArea {
                clicked => { root.clicked(); }
            }
        }
    }

    // A header icon button (gear / back).  Mirrors the proven ContactRow
    // touch pattern: a forwarded `clicked` callback on a TouchArea that
    // explicitly fills its parent, so the hit target is never zero-sized and
    // the click is not swallowed by header layout/z-order.  The earlier inline
    // raw TouchAreas in the header band did not complete their clicks.
    component HeaderIconButton {
        in property <string> icon;
        in property <color>  icon_color: Theme.text-secondary;
        in property <length> icon_size: Theme.size-title;
        callback clicked;

        Rectangle {
            background: touch.has-hover ? Theme.surface-raised : transparent;
            Text {
                text: icon;
                font-size: icon_size;
                color: icon_color;
                horizontal-alignment: center;
                vertical-alignment: center;
            }
            touch := TouchArea {
                width: parent.width;
                height: parent.height;
                clicked => { root.clicked(); }
            }
        }
    }

    // ── Models ───────────────────────────────────────────────────────────────

    struct ContactEntry {
        name:    string,
        initial: string,
        preview: string,
        time_str: string,
        unread:  int,
        unread_str: string,
        hash:    int,  // pub_hash (u8) stored as int
    }

    struct ChannelEntry {
        name:    string,
        initial: string,
        preview: string,
        time_str: string,
        unread:  int,
        unread_str: string,
        hash:    int,
    }

    // ── Root screen component ─────────────────────────────────────────────────

    export component ContactListScreenUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        // ── One-shot screen-entry animation ──
        // `ContactListScreen::new()` (firmware/src/ui/mod.rs::navigate_to_contact_list)
        // constructs a FRESH `ContactListScreenUi` every time the user (re-)enters
        // this screen, so a per-instance `init =>` fade-in fires exactly once on
        // every visit — no Rust wiring needed (same "animate on value-change, not
        // on declared default" mechanic as `splash.rs`'s one-shot choreography, see
        // that module's doc, but self-contained here since this screen's render
        // loop is already ticking continuously by the time it is shown, unlike the
        // boot splash's deferred-start problem). Content starts hidden and fades to
        // fully opaque; `PropertyAnimation::iteration-count` defaults to 1 (not
        // infinite), so it plays once and holds its end state — never a cut-off loop.
        in-out property <float> content_opacity: 0;
        animate content_opacity { duration: 200ms; easing: ease-out; }
        init => { content_opacity = 1; }

        // ── Comet-on-notify trigger ──
        // Retriggerable one-shot, same `play`-property contract as every
        // other `motifs.slint` motion helper (see `CometOnNotify`'s own
        // doc). Rust flips this `false -> true` from `ContactListScreen::
        // maybe_fire_notify` the first time a tab's aggregate unread count
        // genuinely increases while this screen instance is showing; it is
        // deliberately never reset back to `false` from Rust within one
        // instance — `ContactListScreen::new()` builds a FRESH
        // `ContactListScreenUi` every time the user (re-)enters this screen
        // (same fact `content_opacity` above already relies on), so the
        // property's declared `false` default IS the re-arm for the next
        // visit; no explicit reset plumbing needed.
        in-out property <bool> notify_trigger: false;

        in property <[ContactEntry]>  contacts;
        in property <[ChannelEntry]>  channels;
        in-out property <bool>        show_contacts: true;  // true=DMs, false=channels

        // Aggregate unread badges for the tab bar — sum of the per-row `unread`
        // values in `contacts`/`channels` respectively, computed Rust-side in
        // `set_contacts`/`set_channels` so the tab reflects unread state even
        // while the other tab is showing.
        in property <int>    contacts_unread_total;
        in property <string> contacts_unread_str;
        in property <int>    channels_unread_total;
        in property <string> channels_unread_str;

        // Trackball-driven row highlight, index into whichever list
        // (contacts/channels) `show_contacts` currently selects. `-1` = no
        // highlight (touch taps a row directly and never sets this — see
        // `UiRuntime::contact_list_selected`'s doc).
        in property <int> selected_index: -1;

        callback contact_selected(int);  // emits contact hash
        callback channel_selected(int);  // emits channel hash
        callback settings_pressed;       // settings / PIN-menu entry

        // Scroll `main_flick` so `selected_index`'s row is in view. Called
        // from Rust right after `set_selected_index` — same "Rust drives a
        // public function after a model/property update" pattern as
        // `MessageViewScreenUi.scroll_to_bottom()`. `54px` mirrors
        // `ContactRow.height` below (kept in sync by hand; the two can't share
        // a named constant across Slint components here).
        public function scroll_selected_into_view() {
            if selected_index < 0 {
                return;
            }
            main_flick.viewport-y = max(
                min(0px, main_flick.height - main_flick.viewport-height),
                -(selected_index * 54px),
            );
        }

        // Touch-coordinate debug overlay text.
        // Empty string in production; set via set_touch_debug() in --features diagnostics builds.
        // Shows raw GT911 coords and transformed logical coords for empirical calibration.
        in property <string> touch_debug;

        // Full-window dim starfield backdrop — same z-bottom placement
        // `admin_menu.rs`'s `SpaceBackdrop` doc establishes (declared first,
        // so Slint paints it before every other node below). The tab-bar
        // header already washes itself at `Theme.surface.with-alpha(0.55)`
        // and each unselected `ContactRow` already fills at
        // `Theme.nebula-violet-deep.with-alpha(0.12)` (both predate this
        // change), so no row/header alpha tuning was needed here — this
        // screen already had the translucent-row treatment the settings view
        // established; it was only missing the full-window backdrop layer
        // underneath. The Flickable itself carries no fill, so the backdrop
        // also shows through the blank area below the last row.
        SpaceBackdrop {}

        VerticalLayout {
            // Screen-entry one-shot fade-in — see `content_opacity`'s doc above.
            // Scoped to the branded content only (tab bar + list), NOT the
            // touch-debug overlay below, which is a diagnostics affordance and
            // should stay immediately legible regardless of the animation.
            opacity: content_opacity;

            // ── Tab bar ─────────────────────────────────────────────────────
            // Starfield header motif: baked celestial scenery from the shared motif library,
            // sized down from its native 320x40 to this header's 36px height
            // (components expose `width`/`height` as ordinary overridable
            // defaults — see `motifs.slint`'s own doc). Declared first so it
            // paints BEHIND the tab content (Slint z-orders by declaration
            // order); the outer Rectangle's own fill is dropped to a
            // translucent wash of the same `surface` token so the stars show
            // through while tab-label contrast is unchanged from before.
            Rectangle {
                height: 36px;
                background: Theme.surface.with-alpha(0.55);

                Starfield {
                    x: 0px;
                    y: 0px;
                    width: 320px;
                    height: 36px;
                }

                HorizontalLayout {
                    Rectangle {
                        horizontal-stretch: 1.0;
                        background: show_contacts ? Theme.bg-space : transparent;

                        Text {
                            text: "📬 Messages";
                            font-size: Theme.size-body;
                            color: show_contacts ? Theme.brand-signal : Theme.text-secondary;
                            horizontal-alignment: center;
                            vertical-alignment: center;
                        }
                        // Tab-level unread badge — mirrors the per-row ContactRow
                        // badge styling so a nonzero DM unread count is visible
                        // without switching tabs.
                        if contacts_unread_total > 0 : Rectangle {
                            x: parent.width - 18px;
                            y: 3px;
                            width: 14px;
                            height: 14px;
                            border-radius: 7px;
                            background: Theme.brand-signal;
                            Text {
                                text: contacts_unread_str;
                                font-size: Theme.size-badge;
                                font-weight: 700;
                                color: Theme.text-primary;
                                horizontal-alignment: center;
                                vertical-alignment: center;
                            }
                        }
                        // Active-tab underline (per-side borders aren't a Slint property)
                        Rectangle {
                            y: parent.height - 2px;
                            height: show_contacts ? 2px : 0px;
                            width: parent.width;
                            background: Theme.brand-signal;
                        }
                        TouchArea { clicked => { show_contacts = true; } }
                    }
                    Rectangle {
                        horizontal-stretch: 1.0;
                        background: !show_contacts ? Theme.bg-space : transparent;

                        Text {
                            text: "📡 Channels";
                            font-size: Theme.size-body;
                            color: !show_contacts ? Theme.brand-signal : Theme.text-secondary;
                            horizontal-alignment: center;
                            vertical-alignment: center;
                        }
                        // Tab-level unread badge — see the Messages tab above.
                        if channels_unread_total > 0 : Rectangle {
                            x: parent.width - 18px;
                            y: 3px;
                            width: 14px;
                            height: 14px;
                            border-radius: 7px;
                            background: Theme.brand-signal;
                            Text {
                                text: channels_unread_str;
                                font-size: Theme.size-badge;
                                font-weight: 700;
                                color: Theme.text-primary;
                                horizontal-alignment: center;
                                vertical-alignment: center;
                            }
                        }
                        // Active-tab underline (per-side borders aren't a Slint property)
                        Rectangle {
                            y: parent.height - 2px;
                            height: !show_contacts ? 2px : 0px;
                            width: parent.width;
                            background: Theme.brand-signal;
                        }
                        TouchArea { clicked => { show_contacts = false; } }
                    }
                    // ── Settings / PIN-menu entry ─────────────────────────────
                    HeaderIconButton {
                        width: 44px;
                        icon: "⚙";
                        clicked => { root.settings_pressed(); }
                    }
                }

                // Comet-on-notify sweep — declared last so it draws on top of the
                // tab content, sweeping across the header's bottom 14px
                // band. See `notify_trigger`'s doc above for the trigger
                // contract; `ContactListScreen::maybe_fire_notify` (Rust
                // side) flips it on a genuine new-message unread increase.
                CometOnNotify {
                    x: 0px;
                    y: parent.height - 14px;
                    play: root.notify_trigger;
                }
            }

            // ── Contact / channel list ──────────────────────────────────────────
            // One shared Flickable (named so `scroll_selected_into_view` can
            // drive it) with the two row sets as mutually-exclusive conditional
            // children — only one is ever instantiated at a time, same runtime
            // behavior as the previous two-separate-Flickables layout.
            main_flick := Flickable {
                vertical-stretch: 1.0;
                VerticalLayout {
                    if show_contacts : VerticalLayout {
                        for c[i] in contacts : ContactRow {
                            name:       c.name;
                            initial:    c.initial;
                            preview:    c.preview;
                            time_str:   c.time_str;
                            unread:     c.unread;
                            unread_str: c.unread_str;
                            selected:   i == root.selected_index;
                            clicked => { root.contact_selected(c.hash); }
                        }
                    }
                    if !show_contacts : VerticalLayout {
                        for ch[i] in channels : ContactRow {
                            name:       ch.name;
                            initial:    ch.initial;
                            preview:    ch.preview;
                            time_str:   ch.time_str;
                            unread:     ch.unread;
                            unread_str: ch.unread_str;
                            selected:   i == root.selected_index;
                            clicked => { root.channel_selected(ch.hash); }
                        }
                    }
                }
            }
        }

        // ── Touch-coordinate debug overlay ────────────────────────────────────
        // Visible only when touch_debug is non-empty (--features diagnostics builds).
        // Displays "raw(gx,gy) → logical(lx,ly)" in a green bar at the screen bottom.
        // Tap each display corner and verify the logical coords match the expectations
        // in platform.rs::dispatch_touch to confirm (or correct) the rotation transform.
        if touch_debug != "" : Rectangle {
            x: 0;
            y: root.height - 16px;
            width: root.width;
            height: 16px;
            // Diagnostics-only overlay; reuses the frozen `ok` semantic token
            // (success/positive) rather than a bespoke debug hex, per this
            // screen's "no stray hex literals" theming bar.
            background: Theme.ok.with-alpha(0.12);
            Text {
                x: 0;
                y: 0;
                width: root.width;
                height: 16px;
                text: touch_debug;
                font-size: Theme.size-caption;
                color: Theme.ok;
                horizontal-alignment: center;
                vertical-alignment: center;
            }
        }
    }
}

// `format_unread_badge`/`unread_total_increased` are pure Rust with no Slint
// dependency — they now live in `firmware_core::ui::contact_list` so their
// tests execute under `cargo test --workspace` (this crate is a detached,
// cross-compiled workspace — see `Cargo.toml`'s doc comment — so a
// `#[cfg(test)]` block written here would type-check but never run).
// `ChannelItem`/`ContactItem` (plain data, no Slint dependency) moved
// alongside them, together with `build_contact_items`/`build_channel_items`
// (the `UiRuntime` list-builders that construct them — see
// `firmware/src/ui/mod.rs`'s `register_contact`/`set_channels`/
// `handle_event`/`handle_trackball_contact_list` call sites); only this
// Slint-backed view wrapper stays. See
// `docs/adr/0005-firmware-core-extraction.md`.
use firmware_core::ui::contact_list::{format_unread_badge, unread_total_increased};
pub use firmware_core::ui::contact_list::{
    build_channel_items, build_contact_items, ChannelItem, ContactItem,
};

/// Rust-side wrapper.
pub struct ContactListScreen {
    component: self::ContactListScreenUi,
    // Baselines for the comet-on-notify trigger (see `unread_total_increased`
    // and `maybe_fire_notify` below) — `None` until the first
    // `set_contacts`/`set_channels` call on THIS instance records one.
    // `Cell`, not a plain field, because every method here takes `&self`
    // (the Slint component handle itself is shared this way throughout this
    // file) — interior mutability is required to update it.
    prev_contacts_unread: std::cell::Cell<Option<i32>>,
    prev_channels_unread: std::cell::Cell<Option<i32>>,
}

impl ContactListScreen {
    pub fn new() -> anyhow::Result<Self> {
        let component = self::ContactListScreenUi::new()
            .map_err(|e| anyhow::anyhow!("slint component init: {:?}", e))?;
        component.show()
            .map_err(|e| anyhow::anyhow!("slint window show: {:?}", e))?;
        Ok(ContactListScreen {
            component,
            prev_contacts_unread: std::cell::Cell::new(None),
            prev_channels_unread: std::cell::Cell::new(None),
        })
    }

    /// Fire the shared header `CometOnNotify` one-shot (see
    /// `notify_trigger`'s doc in the markup above) iff `total` is a genuine
    /// increase over `baseline`'s last-observed value, then update the
    /// baseline unconditionally. `motifs.slint`'s `CometOnNotify` only
    /// animates on a `false -> true` VALUE CHANGE — since `notify_trigger`
    /// starts `false` and this method only ever writes `true`, calling it
    /// again while already `true` (a second increase before the screen is
    /// next re-mounted) is a harmless no-op write, not a re-fire; the next
    /// visible sweep waits for a fresh screen instance (see
    /// `ContactListScreen::new`'s doc note on `content_opacity`).
    fn maybe_fire_notify(&self, baseline: &std::cell::Cell<Option<i32>>, total: i32) {
        if unread_total_increased(baseline.get(), total) {
            self.component.set_notify_trigger(true);
        }
        baseline.set(Some(total));
    }

    /// Replace the full contact list model.
    pub fn set_contacts(&self, contacts: &[ContactItem]) {
        let model: slint::VecModel<ContactEntry> = slint::VecModel::default();
        let mut total: i32 = 0;
        for c in contacts {
            let initial = c.name.chars().next()
                .map(|ch| ch.to_uppercase().to_string())
                .unwrap_or_default();
            let unread_str = format_unread_badge(c.unread);
            total += c.unread;
            model.push(ContactEntry {
                name:       c.name.clone().into(),
                initial:    initial.into(),
                preview:    c.preview.clone().into(),
                time_str:   c.time_str.clone().into(),
                unread:     c.unread,
                unread_str: unread_str.into(),
                hash:       c.hash as i32,
            });
        }
        self.component.set_contacts(slint::ModelRc::new(model));
        // Aggregate badge for the "📬 Messages" tab.
        self.component.set_contacts_unread_total(total);
        self.component.set_contacts_unread_str(format_unread_badge(total).into());
        // Comet-on-notify — see `maybe_fire_notify`'s doc.
        self.maybe_fire_notify(&self.prev_contacts_unread, total);
    }

    /// Replace the full channel list model.
    pub fn set_channels(&self, channels: &[ChannelItem]) {
        let model: slint::VecModel<ChannelEntry> = slint::VecModel::default();
        let mut total: i32 = 0;
        for ch in channels {
            let initial = ch.name.chars().next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default();
            let unread_str = format_unread_badge(ch.unread);
            total += ch.unread;
            model.push(ChannelEntry {
                name:       ch.name.clone().into(),
                initial:    initial.into(),
                preview:    ch.preview.clone().into(),
                time_str:   ch.time_str.clone().into(),
                unread:     ch.unread,
                unread_str: unread_str.into(),
                hash:       ch.hash as i32,
            });
        }
        self.component.set_channels(slint::ModelRc::new(model));
        // Aggregate badge for the "📡 Channels" tab.
        self.component.set_channels_unread_total(total);
        self.component.set_channels_unread_str(format_unread_badge(total).into());
        // Comet-on-notify — see `maybe_fire_notify`'s doc.
        self.maybe_fire_notify(&self.prev_channels_unread, total);
    }

    /// Move the trackball highlight to `idx` (row index within whichever tab
    /// — contacts or channels — is currently visible; `-1` clears it) and
    /// scroll it into view. The caller (`UiRuntime::handle_trackball_contact_list`)
    /// owns clamping `idx` to the current list length.
    pub fn set_selected_index(&self, idx: i32) {
        self.component.set_selected_index(idx);
        self.component.invoke_scroll_selected_into_view();
    }

    /// Which tab is currently visible: `true` = Contacts (DMs), `false` =
    /// Channels. Read by the trackball handler to know which list
    /// `selected_index` indexes into.
    pub fn show_contacts(&self) -> bool {
        self.component.get_show_contacts()
    }

    /// Fire the `contact_selected` callback exactly as a tap on row `hash`
    /// would — used by the trackball Click handler so both input paths funnel
    /// through the same navigation logic.
    pub fn invoke_contact_selected(&self, hash: u8) {
        self.component.invoke_contact_selected(hash as i32);
    }

    /// Fire the `channel_selected` callback exactly as a tap on row `hash`
    /// would — see [`Self::invoke_contact_selected`].
    pub fn invoke_channel_selected(&self, hash: u8) {
        self.component.invoke_channel_selected(hash as i32);
    }

    /// Set callback for when a contact row is tapped.
    pub fn on_contact_selected(&self, cb: impl Fn(u8) + 'static) {
        self.component.on_contact_selected(move |hash| cb(hash as u8));
    }

    /// Set callback for when a channel row is tapped.
    pub fn on_channel_selected(&self, cb: impl Fn(u8) + 'static) {
        self.component.on_channel_selected(move |hash| cb(hash as u8));
    }

    /// Set callback for when the settings gear icon is tapped.
    ///
    /// The UI runtime uses this to navigate to the PIN-entry screen
    /// (`pin_menu::verify_pin` → admin settings menu).
    pub fn on_settings_pressed(&self, cb: impl Fn() + 'static) {
        self.component.on_settings_pressed(cb);
    }

    /// Update the touch-coordinate debug overlay.
    ///
    /// Formats both raw GT911 coords and transformed Slint logical coords into a
    /// one-line string shown in the green bar at the screen bottom.
    ///
    /// Only available with `--features diagnostics`; compiled out of production.
    #[cfg(feature = "diagnostics")]
    pub fn set_touch_debug(&self, raw: (u16, u16), logical: (i32, i32)) {
        let s = format!(
            "raw({},{}) \u{2192} logical({},{})",
            raw.0, raw.1, logical.0, logical.1,
        );
        self.component.set_touch_debug(s.into());
    }

    pub fn hide(&self) { self.component.hide().ok(); }
}

// `format_unread_badge`/`unread_total_increased`'s tests moved to
// `firmware-core/src/ui/contact_list.rs` alongside the functions — see this
// file's module-level move note above.
