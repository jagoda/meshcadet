// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig for the screen-lock overlay
//! (`firmware/src/ui/screens/lock.rs`, `meshcadet-lock-firmware-ui`).
//!
//! # Why this is a separate, narrower render path from `HostSimUi` /
//! `motif_library` / `compose_send` / `gps_status_rows`
//!
//! `firmware/src/ui/screens/lock.rs` cannot itself be compiled on the host
//! — the `firmware` crate cross-compiles for `xtensa-esp32s3-espidf` only
//! (see `lib.rs`'s module doc). This rig copies `LockScreenUi`'s real
//! `PinDot`/`NumPadButton`/`LockScreenUi` markup verbatim (importing the
//! REAL `theme.slint`/`motifs.slint` by relative path — single source of
//! truth, same technique every other `ui_sim` module uses), with one
//! deliberate omission: `backdrop_image`/`SpaceBackdrop` and the
//! `reveal_opacity` one-shot fade are dropped. Both are ALREADY proven
//! elsewhere (`list_pane_backdrop.rs`/`splash_lineart.rs` for the backdrop
//! compositing; every other themed screen's identical `init =>` pattern for
//! the fade) and are irrelevant to the four states this rig exists to
//! verify — see `compose_send.rs`'s module doc for the identical
//! "deliberately not a pixel-for-pixel mirror, isolate what's actually
//! unproven" scoping rule this follows. Dropping the fade also means every
//! render below is deterministic at `opacity: 1.0` with no animation
//! settling window to wait out.
//!
//! Renders the four states this mission's acceptance criteria name:
//! **locked** (empty digit buffer, numpad visible, no badge), **wrong-PIN**
//! (`reject_trigger` — the transient dot-row recolor that replaced the
//! removed border flash, see `lock.rs`'s module doc for that hard
//! constraint), **backing-off** (D4's escalating-lockout countdown, which
//! REPLACES the numpad entirely), and **unread-badge** (D5's count-only
//! waiting-message indicator).
//!
//! Slint enforces a process-wide `Platform` singleton, so this module's
//! render entry point must never run in the same process as `lib.rs`'s,
//! `motif_library`'s, `compose_send`'s, `gps_status_rows`'s, etc. —
//! `ui_sim/tests/lock_screen.rs` is its own Cargo integration-test binary
//! (own process), same isolation technique those modules' own docs explain.
//! Unlike those single-static-frame rigs, this one renders MULTIPLE frames
//! from the SAME constructed component (setting properties between calls)
//! — the four states are mutually exclusive settings of one screen, not
//! four independent components, so this is truer to how `UiRuntime`
//! actually drives the real `LockScreen` wrapper (`set_backing_off`/
//! `trigger_reject`/`set_unread_count` mutating one live component).

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

/// Title bar height — `y` in `[0, TITLE_HEIGHT)` is where the D5 unread
/// badge paints (or doesn't).
pub const TITLE_HEIGHT: u32 = 36;
/// Dot-row height, right below the title bar — where the wrong-PIN reject
/// cue paints (or doesn't).
pub const DOT_ROW_HEIGHT: u32 = 28;
/// Numpad-or-backoff-panel height, right below the dot row — where the
/// backing-off countdown's `Theme.warn`-colored "⏳" paints (or doesn't;
/// the ordinary numpad state never uses `Theme.warn` at all).
pub const LOWER_PANEL_HEIGHT: u32 = 166;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import { CrescentMoon, CadetPeeking } from "../../firmware/src/ui/motifs.slint";

    // Verbatim copy of `lock.rs`'s `PinDot` — see this file's module doc.
    component PinDot {
        in property <bool> filled;
        in property <bool> reject;
        width: 18px;
        height: 18px;
        Rectangle {
            border-radius: 9px;
            background: reject ? Theme.alert : (filled ? Theme.brand-signal : Theme.surface-raised);
            width: 18px;
            height: 18px;
        }
    }

    // Verbatim copy of `lock.rs`'s `NumPadButton`.
    component NumPadButton {
        in property <string> label;
        in property <bool>   enabled: true;
        callback clicked;

        height: 36px;
        Rectangle {
            background: Theme.surface;
            border-radius: 8px;
            Text {
                text: label;
                font-size: Theme.icon-lg; // 20px
                font-weight: 600;
                color: enabled ? Theme.text-primary : Theme.text-muted;
                horizontal-alignment: center;
                vertical-alignment: center;
            }
            TouchArea { enabled: enabled; clicked => { root.clicked(); } }
        }
    }

    // Verbatim copy of `lock.rs`'s `LockScreenUi`, minus `backdrop_image`/
    // `SpaceBackdrop`/`reveal_opacity` — see this file's module doc for why.
    export component LockScreenTestUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        in property <int>    unread_count: 0;
        in-out property <int> digits_entered: 0;
        in property <bool>   backing_off: false;
        in property <string> lockout_text: "";
        in-out property <bool> reject_trigger: false;

        callback digit_pressed(int);
        callback backspace_pressed;
        callback confirm_pressed;

        Timer {
            interval: 350ms;
            running: root.reject_trigger;
            triggered => { root.reject_trigger = false; }
        }

        VerticalLayout {
            spacing: 0px;

            // ── Title ───────────────────────────────────────────────────────
            Rectangle {
                height: 36px;
                background: Theme.surface;
                HorizontalLayout {
                    padding-left: 8px;
                    padding-right: 8px;
                    spacing: 6px;
                    alignment: center;
                    Rectangle {
                        width: 18px;
                        CrescentMoon {
                            width: 18px;
                            height: 18px;
                            y: (parent.height - self.height) / 2;
                        }
                    }
                    Text { text: "🔒"; font-size: Theme.icon-lg; color: Theme.warn; vertical-alignment: center; }
                    Text {
                        text: "Locked";
                        font-size: Theme.size-subtitle;
                        font-weight: 600;
                        color: Theme.text-primary;
                        vertical-alignment: center;
                    }
                    Rectangle { horizontal-stretch: 1.0; }
                    if unread_count > 0 : Rectangle {
                        width: 40px;
                        height: 22px;
                        border-radius: 11px;
                        background: Theme.surface-raised;
                        HorizontalLayout {
                            alignment: center;
                            spacing: 3px;
                            Text { text: "✉"; font-size: Theme.size-meta; color: Theme.text-secondary; vertical-alignment: center; }
                            Text { text: unread_count; font-size: Theme.size-meta; color: Theme.text-primary; vertical-alignment: center; }
                        }
                    }
                }
            }

            // ── Dot row ─────────────────────────────────────────────────────
            Rectangle {
                height: 28px;
                background: Theme.bg-space;
                HorizontalLayout {
                    alignment: center;
                    spacing: 16px;
                    padding-top: 5px;
                    PinDot { filled: digits_entered >= 1; reject: reject_trigger; }
                    PinDot { filled: digits_entered >= 2; reject: reject_trigger; }
                    PinDot { filled: digits_entered >= 3; reject: reject_trigger; }
                    PinDot { filled: digits_entered >= 4; reject: reject_trigger; }
                }
                CadetPeeking {
                    width: 24px;
                    height: 24px;
                    x: parent.width - self.width - 6px;
                    y: parent.height - self.height;
                }
            }

            // ── Numpad (hidden while backing off) ──────────────────────────
            if !backing_off : GridLayout {
                spacing: 4px;
                padding: 5px;

                NumPadButton { col: 0; row: 0; label: "1"; clicked => { root.digit_pressed(1); } }
                NumPadButton { col: 1; row: 0; label: "2"; clicked => { root.digit_pressed(2); } }
                NumPadButton { col: 2; row: 0; label: "3"; clicked => { root.digit_pressed(3); } }

                NumPadButton { col: 0; row: 1; label: "4"; clicked => { root.digit_pressed(4); } }
                NumPadButton { col: 1; row: 1; label: "5"; clicked => { root.digit_pressed(5); } }
                NumPadButton { col: 2; row: 1; label: "6"; clicked => { root.digit_pressed(6); } }

                NumPadButton { col: 0; row: 2; label: "7"; clicked => { root.digit_pressed(7); } }
                NumPadButton { col: 1; row: 2; label: "8"; clicked => { root.digit_pressed(8); } }
                NumPadButton { col: 2; row: 2; label: "9"; clicked => { root.digit_pressed(9); } }

                NumPadButton { col: 0; row: 3; label: "⌫"; clicked => { root.backspace_pressed(); } }
                NumPadButton { col: 1; row: 3; label: "0"; clicked => { root.digit_pressed(0); } }
                NumPadButton {
                    col: 2; row: 3; label: "✓";
                    enabled: digits_entered == 4;
                    clicked => { root.confirm_pressed(); }
                }
            }

            // ── Backoff countdown (replaces the numpad entirely — D4) ──────
            if backing_off : Rectangle {
                height: 166px;
                VerticalLayout {
                    alignment: center;
                    spacing: 6px;
                    Text {
                        text: "⏳";
                        font-size: Theme.size-hero;
                        color: Theme.warn;
                        horizontal-alignment: center;
                    }
                    Text {
                        text: lockout_text;
                        font-size: Theme.size-body-lg;
                        color: Theme.text-secondary;
                        horizontal-alignment: center;
                    }
                }
            }
        }
    }
}

struct LockScreenPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for LockScreenPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Render harness driving ONE live `LockScreenTestUi` instance through
/// multiple states — see this file's module doc for why that's truer to
/// the real `LockScreen` wrapper than four independent components.
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `gps_status_rows.rs::GpsStatusRowsFrame::new`'s identical note. Callers
/// must ensure exactly one [`LockScreenFrame::new`] runs per process.
pub struct LockScreenFrame {
    window: Rc<MinimalSoftwareWindow>,
    ui: LockScreenTestUi,
}

impl LockScreenFrame {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(LockScreenPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");
        crate::register_device_font(&window);

        let ui = LockScreenTestUi::new().expect("LockScreenTestUi::new");
        ui.show().expect("LockScreenTestUi::show");

        LockScreenFrame { window, ui }
    }

    /// **Locked** state: empty digit buffer, numpad visible, no reject cue,
    /// no waiting-message badge — the screen's resting state right after a
    /// lock trip.
    pub fn set_state_locked(&self) {
        self.ui.set_digits_entered(0);
        self.ui.set_backing_off(false);
        self.ui.set_reject_trigger(false);
        self.ui.set_unread_count(0);
        self.ui.set_lockout_text("".into());
    }

    /// **Wrong-PIN** state: the transient dot-row reject cue is active
    /// (`reject_trigger`) — NOT the removed border flash (this mission's
    /// hard constraint).
    pub fn set_state_wrong_pin(&self) {
        self.set_state_locked();
        self.ui.set_reject_trigger(true);
    }

    /// **Backing-off** state (D4): the numpad is replaced entirely by the
    /// countdown panel.
    pub fn set_state_backing_off(&self, lockout_text: &str) {
        self.set_state_locked();
        self.ui.set_backing_off(true);
        self.ui.set_lockout_text(lockout_text.into());
    }

    /// **Unread-badge** state (D5): count-only waiting-message indicator.
    pub fn set_state_unread_badge(&self, count: i32) {
        self.set_state_locked();
        self.ui.set_unread_count(count);
    }

    /// Render whatever state was last set via the `set_state_*` methods
    /// above.
    pub fn render(&self) -> Vec<Rgb565Pixel> {
        slint::platform::update_timers_and_animations();
        self.window.request_redraw();

        let mut framebuffer = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
        let rendered = self.window.draw_if_needed(|renderer| {
            renderer.render(&mut framebuffer, WIDTH as usize);
        });
        assert!(
            rendered,
            "lock-screen frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for LockScreenFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// Expand a rendered RGB565 pixel back to 8-bit-per-channel for assertions
/// — same conversion every other `ui_sim` render module duplicates locally
/// (see `gps_status_rows.rs`'s identical function doc for why).
pub fn rgb8(px: Rgb565Pixel) -> (u8, u8, u8) {
    let r5 = (px.0 >> 11) & 0x1F;
    let g6 = (px.0 >> 5) & 0x3F;
    let b5 = px.0 & 0x1F;
    (
        ((r5 << 3) | (r5 >> 2)) as u8,
        ((g6 << 2) | (g6 >> 4)) as u8,
        ((b5 << 3) | (b5 >> 2)) as u8,
    )
}
