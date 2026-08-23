// SPDX-License-Identifier: GPL-3.0-only
//! Screen-lock overlay — the 4-dot pad + numpad shown while `UiRuntime.locked`
//! is `true` (screen-lock plan D3).
//!
//! # Boundary
//!
//! Mirrors `PinEntryScreen`'s widget/logic split (see that module's doc):
//! this module owns the **widget** only. The caller
//! (`ui::mod::UiRuntime`) owns the PIN comparison (against the SEPARATE
//! `stored_lock_pin`, never the admin PIN — see `firmware_core::lock_store::
//! verify`'s doc), the backoff state machine (`firmware_core::ui::lock`),
//! and the overlay/retained-component mechanics.
//!
//! # Why this is a distinct screen, not `PinEntryScreen` reused directly
//!
//! `PinEntryScreen` is reachable from `ContactList`'s settings button and
//! its confirm path leads to the admin menu — reusing it here would blur
//! the "the lock PIN is verified against the boot-seeded lock PIN, not the
//! admin PIN" hard constraint into one widget serving two different secrets
//! and two different post-confirm destinations. It DOES reuse
//! `pin_entry.rs`'s layout and `Theme` tokens verbatim (4-dot pad, 3x4
//! numpad, same 320x240 height budget) — only the following differ:
//!
//! - **No cancel (✕) button.** D7: nothing is reachable while locked —
//!   there is no legitimate destination to cancel TO.
//! - **D5 count-only waiting-message badge** in the title bar — no sender,
//!   no preview.
//! - **A backoff countdown** (`backing_off`/`lockout_text`) that REPLACES
//!   the numpad entirely while an escalating lockout
//!   (`firmware_core::ui::lock`) is active, at the numpad's own 166px
//!   height budget so the total stays within the 230px pin_entry.rs already
//!   proved fits.
//! - **A transient reject cue that is NOT the border flash** removed
//!   2026-07-05 (this mission's explicit hard constraint — wrong-PIN
//!   feedback has been audio-only since then). `reject_trigger` recolors
//!   the 4 dots to `Theme.alert` for one Timer-driven pulse (same
//!   trigger-property + auto-reset-`Timer` idiom `compose.rs`'s
//!   `rocket_trigger` already establishes) instead of reintroducing any
//!   full-window flash.

slint::slint! {
    import { Theme } from "../theme.slint";
    import { CrescentMoon, CadetPeeking, SpaceBackdrop } from "../motifs.slint";

    component PinDot {
        in property <bool> filled;
        // Transient wrong-PIN reject cue (see module doc) — recolors ALL
        // four dots together for one pulse, distinct from `filled` (which
        // dots are filled is irrelevant to a reject cue; the whole row
        // pulses as one signal).
        in property <bool> reject;
        width: 18px;
        height: 18px;
        Rectangle {
            border-radius: 9px;
            background: reject ? Theme.alert : (filled ? Theme.brand-signal : Theme.surface-raised);
            animate background { duration: 120ms; easing: ease-out; }
            width: 18px;
            height: 18px;
        }
    }

    component NumPadButton {
        in property <string> label;
        in property <bool>   enabled: true;
        callback clicked;

        // Same 36px key height as pin_entry.rs's NumPadButton — see that
        // component's own doc for the height-budget rationale.
        height: 36px;
        Rectangle {
            background: btn_touch.has-hover ? Theme.select : Theme.surface;
            animate background { duration: 100ms; easing: ease-out; }
            border-radius: 8px;
            Text {
                text: label;
                font-size: Theme.icon-lg; // 20px
                font-weight: 600;
                color: enabled ? Theme.text-primary : Theme.text-muted;
                horizontal-alignment: center;
                vertical-alignment: center;
            }
            btn_touch := TouchArea {
                enabled: enabled;
                clicked => { root.clicked(); }
            }
        }
    }

    export component LockScreenUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        // ── Height budget (mirrors pin_entry.rs's own — see that file) ──
        //   title       36
        //   dot row     28
        //   numpad OR backing-off panel   166
        //   ─────────────────────────────────────────
        //   total      230  ≤ 240  (10px headroom)

        // Waiting-message count only (D5) — no sender, no preview. `<= 0`
        // hides the badge entirely (nothing waiting).
        in property <int>    unread_count: 0;
        // Number of digits entered so far (0-4).
        in-out property <int> digits_entered: 0;
        // Escalating-backoff state (firmware_core::ui::lock D4): while
        // `backing_off`, the numpad is replaced by `lockout_text`
        // ("Try again in Ns") entirely — no attempt can be made at all,
        // so there is nothing to disable/re-enable per-key.
        in property <bool>   backing_off: false;
        in property <string> lockout_text: "";
        // Transient wrong-PIN reject cue — see PinDot's own doc. Setting
        // this `true` starts the pulse; the sibling Timer below resets it.
        in-out property <bool> reject_trigger: false;
        // Shared full-window starfield texture — see pin_entry.rs's
        // identical property for why this isn't a SpaceBackdrop default.
        in property <image>   backdrop_image;

        callback digit_pressed(int);   // 0-9
        callback backspace_pressed;
        callback confirm_pressed;

        // Auto-reset for reject_trigger — same trigger-property + Timer
        // idiom compose.rs's rocket_trigger uses (see that file's module
        // doc). 350ms: long enough to read as a deliberate pulse, short
        // enough that a fast re-attempt right after a wrong PIN never
        // stacks a second pulse mid-animation (the dot-fill state itself,
        // driven by digits_entered, is independent and unaffected).
        Timer {
            interval: 350ms;
            running: root.reject_trigger;
            triggered => { root.reject_trigger = false; }
        }

        // ── One-shot screen-entry reveal — see pin_entry.rs's identical block ──
        in-out property <float> reveal_opacity: 0;
        animate reveal_opacity { duration: 200ms; easing: ease-out; }
        init => { self.reveal_opacity = 1.0; }

        SpaceBackdrop { source: backdrop_image; }

        VerticalLayout {
            spacing: 0px;
            opacity: reveal_opacity;

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
                        font-size: Theme.size-subtitle; // 15px
                        font-weight: 600;
                        color: Theme.text-primary;
                        vertical-alignment: center;
                    }
                    Rectangle { horizontal-stretch: 1.0; }
                    // ── D5: count-only waiting-message badge ──────────────────
                    // No sender, no preview — see module doc. Hidden entirely
                    // at zero rather than showing "0".
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
            // Same 166px allocation the numpad above occupies, so the total
            // screen height budget is identical in both states.
            if backing_off : Rectangle {
                height: 166px;
                VerticalLayout {
                    alignment: center;
                    spacing: 6px;
                    Text {
                        text: "⏳";
                        font-size: Theme.size-hero; // 28px
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

/// Rust-side wrapper + PIN-digit state machine.
///
/// Mirrors `PinEntryScreen`'s shape (see that module's doc): a fresh
/// `LockScreen` is constructed each time the lock trips (both idle-timeout
/// and boot-locked — see `UiRuntime::trip_lock`), never reused/reset across
/// visits, so `init` fires exactly once per mount just like every other
/// per-visit screen in this UI.
pub struct LockScreen {
    component: self::LockScreenUi,
}

impl LockScreen {
    pub fn new() -> anyhow::Result<Self> {
        let component = self::LockScreenUi::new()
            .map_err(|e| anyhow::anyhow!("slint component init: {:?}", e))?;
        component.set_backdrop_image(crate::ui::backdrop_asset::shared_backdrop_image());
        component.show()
            .map_err(|e| anyhow::anyhow!("slint window show: {:?}", e))?;
        Ok(LockScreen { component })
    }

    /// Set the D5 count-only waiting-message badge. `<= 0` hides it
    /// entirely (see `LockScreenUi.unread_count`'s doc).
    pub fn set_unread_count(&self, n: i32) {
        self.component.set_unread_count(n);
    }

    /// Enter the backoff-countdown state, replacing the numpad with
    /// `lockout_text` (e.g. "Try again in 23s") — the caller
    /// (`UiRuntime::step`) recomputes and re-calls this every tick while a
    /// lockout is active so the countdown visibly ticks down.
    pub fn set_backing_off(&self, lockout_text: &str) {
        self.component.set_backing_off(true);
        self.component.set_lockout_text(lockout_text.into());
    }

    /// Leave the backoff-countdown state, restoring the numpad.
    pub fn clear_backing_off(&self) {
        self.component.set_backing_off(false);
    }

    /// Fire the transient wrong-PIN reject cue (see `PinDot`'s doc) — NOT
    /// the removed border flash (this mission's hard constraint).
    pub fn trigger_reject(&self) {
        self.component.set_reject_trigger(true);
    }

    /// Wire digit/backspace/confirm callbacks. `digit_buf` is a shared
    /// buffer the caller owns (mirrors `PinEntryScreen::wire_pin_callbacks`);
    /// `on_confirmed(digits)` fires on ✓ press with the full entered digit
    /// sequence, AFTER the Slint dot counter and buffer are already reset —
    /// see that method's identical contract for why. There is no
    /// `on_cancelled` — this screen has no cancel affordance (D7: nothing is
    /// reachable while locked, so there is no legitimate destination to
    /// cancel to).
    pub fn wire_pin_callbacks(
        &self,
        digit_buf: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
        on_confirmed: impl Fn(Vec<u8>) + 'static,
    ) {
        let comp_d = self.component.clone_strong();
        let buf_d = digit_buf.clone();
        self.component.on_digit_pressed(move |d| {
            let mut buf = buf_d.borrow_mut();
            if buf.len() < 4 {
                // ASCII digit bytes ('0'=0x30 .. '9'=0x39) — matches
                // firmware_core::lock_store's stored PIN byte convention.
                buf.push(b'0'.wrapping_add(d as u8));
                comp_d.set_digits_entered(buf.len() as i32);
            }
        });

        let comp_b = self.component.clone_strong();
        let buf_b = digit_buf.clone();
        self.component.on_backspace_pressed(move || {
            let mut buf = buf_b.borrow_mut();
            if !buf.is_empty() {
                buf.pop();
                comp_b.set_digits_entered(buf.len() as i32);
            }
        });

        let comp_c = self.component.clone_strong();
        let buf_c = digit_buf.clone();
        self.component.on_confirm_pressed(move || {
            let digits = buf_c.borrow().clone();
            comp_c.set_digits_entered(0);
            buf_c.borrow_mut().clear();
            on_confirmed(digits);
        });
    }

    pub fn show(&self) {
        self.component.show().ok();
    }

    pub fn hide(&self) {
        self.component.hide().ok();
    }
}
