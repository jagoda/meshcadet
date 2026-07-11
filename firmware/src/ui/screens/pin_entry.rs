// SPDX-License-Identifier: GPL-3.0-only
//! PIN entry surface — shared widget for PIN-gated admin menu access.
//!
//! # Interface contract with the admin-menu + history logic
//!
//! This module owns the **widget** (the visible 4-digit pad and numpad).
//! The caller owns the **menu logic** that runs after a
//! successful PIN.
//!
//! Boundary:
//! - `PinEntryScreen` fires `on_pin_correct(pin: String)` when the entered PIN
//!   matches the stored hash.  The caller navigates to the admin menu.
//! - `PinEntryScreen` fires `on_pin_wrong()` on mismatch (for the notification
//!   and attempt-counter logic, owned by the caller).
//! - PIN storage / hashing is NOT in this module — the caller supplies the
//!   verification closure.
//!
//! # UI
//!
//! ```text
//! ┌──────────────────────────────────────┐
//! │  🔐  Admin Menu                     │  title (reason-dependent)
//! │  ● ● ○ ○                            │  4 dots (filled = entered)
//! ├──────────────────────────────────────┤
//! │   1   │   2   │   3   │             │
//! │   4   │   5   │   6   │             │
//! │   7   │   8   │   9   │             │
//! │  ⌫   │   0   │  ✓   │             │
//! └──────────────────────────────────────┘
//! ```
//!
//! # Theme tokens + one-shot animation language
//!
//! Every color/font-size literal in this screen's `slint::slint!{}` block
//! (below) now reads from the shared `Theme` global (`ui/theme.slint`,
//! imported below) at the SAME values — a pixel-identical swap, same pattern
//! as `splash.rs`'s Phase-1 pilot and the other themed screens
//! (`contact_list.rs`, `compose.rs`, `gps_status.rs`, `message_view.rs`). The
//! 🔐 lock icon stays at `Theme.icon-lg` (20px), which is both a `PIXEL_SIZES`
//! and an `EMOJI_SIZES` entry (`gen_emoji_font.c`), so it keeps rendering —
//! never a blank glyph.
//!
//! Two additions apply this UI's animation language:
//! - **Screen-entry reveal:** a one-shot 200ms ease-out opacity fade,
//!   identical mechanism to `gps_status.rs`/`compose.rs` — a fresh
//!   `PinEntryScreenUi` is built by [`PinEntryScreen::new`] on every PIN
//!   prompt (see that struct's own doc: "a fresh `PinEntryScreen` is created
//!   per PIN prompt"), so `init` fires exactly once per mount and the single
//!   settled-value write is what triggers the `animate` transition. Nothing
//!   else ever touches `reveal_opacity`, so live `digits_entered` updates
//!   (dot fills) never re-fire it.
//! - **NumPadButton hover:** the existing `has-hover` background swap now
//!   gets a short `animate` (100ms ease-out), matching the hover-feedback
//!   convention already applied to `contact_list.rs`/`message_view.rs` rows.
//!
//! # Outer-space theme (per-screen spec row 6: "Crescent moon +
//! lock motif; Cadet peeking")
//!
//! Two additive, presentation-only motif placements on top of the palette
//! wiring above — both reused as-is from the shared `ui/motifs.slint`
//! contract; no new asset is
//! authored here, and neither touches the 230/240px height budget documented
//! on `PinEntryScreenUi` below:
//! - `CrescentMoon` sits beside the existing 🔐 lock emoji in the title bar,
//!   inside its own fixed-width `Rectangle` (same "icon column" idiom
//!   `gps_status.rs`'s `StatusRow` uses for `RingedPlanetCorner`/`Comet`) —
//!   the pair reads as the plan's "crescent moon + lock" motif without
//!   growing the 36px title bar.
//! - `CadetPeeking` sits at the right edge of the dot row, inside that row's
//!   existing 28px-tall `Rectangle` (declared after the dot `HorizontalLayout`
//!   so it paints on top, same z-order idiom `contact_list.rs`'s `Starfield`
//!   uses), well clear of the centered dots (which span roughly x=100..220 of
//!   320) so it never overlaps them. `cadet_peeking.png`'s art already bakes
//!   in the "peeking over a ledge" framing (helmet low in frame, body
//!   omitted — see `generate_assets.py::gen_cadet_peeking`'s doc), so no
//!   additional clipping/offset trick is needed beyond bottom-aligning it in
//!   its row.
//!
//! No motion is applied to either motif — the design plan's motion-language
//! table does not list pin_entry among the animated screens (row 6 has no
//! motion column entry).
//!
//! Presentation-only: no callback signatures, no Rust handler logic changed.

slint::slint! {
    import { Theme } from "../theme.slint";
    import { CrescentMoon, CadetPeeking, SpaceBackdrop } from "../motifs.slint";

    component PinDot {
        in property <bool> filled;
        width: 18px;
        height: 18px;
        Rectangle {
            border-radius: 9px;
            background: filled ? Theme.brand-signal : Theme.surface-raised;
            width: 18px;
            height: 18px;
        }
    }

    component NumPadButton {
        in property <string> label;
        in property <bool>   enabled: true;
        callback clicked;

        // 36px keys keep the 4-row pad within the 320x240 panel. With a 44px
        // key the pad's laid-out height (title 36 + dots 28 + numpad 200 = 264)
        // exceeded the 240px window and the bottom row (⌫ 0 ✓) was clipped
        // off-screen.  See the height budget on PinEntryScreenUi below.
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

    export component PinEntryScreenUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        // ── Height budget (must sum ≤ 240px so the bottom keypad row is on-screen) ──
        //   title       36
        //   dot row     28
        //   numpad      5 (pad) + 4*36 (keys) + 3*4 (spacing) + 5 (pad) = 166
        //   ─────────────────────────────────────────────────────────────
        //   total      230  ≤ 240  (10px headroom)
        // All 12 keys (0-9, ⌫, ✓) plus the title-bar ✕ are fully laid out
        // inside the panel and tappable.

        in property <string>  menu_title: "Admin Menu";
        in property <string>  subtitle: "Enter your PIN";
        // Number of digits entered so far (0–4).
        in-out property <int> digits_entered: 0;

        // Callbacks
        callback digit_pressed(int);   // 0–9
        callback backspace_pressed;
        callback confirm_pressed;
        callback cancel_pressed;

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

            // ── Title ───────────────────────────────────────────────────────
            Rectangle {
                height: 36px;
                background: Theme.surface;
                HorizontalLayout {
                    padding-left: 8px;
                    spacing: 6px;
                    alignment: center;
                    // Crescent moon + lock motif (see module doc's
                    // "Outer-space theme" section) — icon-column idiom, fits
                    // inside the 36px title bar without growing it.
                    Rectangle {
                        width: 18px;
                        CrescentMoon {
                            width: 18px;
                            height: 18px;
                            y: (parent.height - self.height) / 2;
                        }
                    }
                    Text { text: "🔐"; font-size: Theme.icon-lg; vertical-alignment: center; }
                    Text {
                        text: menu_title;
                        font-size: Theme.size-subtitle; // 15px
                        font-weight: 600;
                        color: Theme.text-primary;
                        vertical-alignment: center;
                    }
                    Rectangle { horizontal-stretch: 1.0; }
                    // Cancel (X)
                    Rectangle {
                        width: 32px; height: 30px;
                        Text { text: "✕"; font-size: Theme.size-title; color: Theme.text-secondary;
                               horizontal-alignment: center; vertical-alignment: center; }
                        TouchArea { clicked => { root.cancel_pressed(); } }
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
                    PinDot { filled: digits_entered >= 1; }
                    PinDot { filled: digits_entered >= 2; }
                    PinDot { filled: digits_entered >= 3; }
                    PinDot { filled: digits_entered >= 4; }
                }
                // Cadet peeking motif (see module doc's "Outer-space theme"
                // section) — declared after the dot layout so it paints on
                // top; parked at the row's right edge, clear of the centered
                // dots (roughly x=100..220 of 320).
                CadetPeeking {
                    width: 24px;
                    height: 24px;
                    x: parent.width - self.width - 6px;
                    y: parent.height - self.height;
                }
            }

            // ── Numpad ──────────────────────────────────────────────────────
            GridLayout {
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
        }
    }
}

/// Rust-side PIN state machine.
///
/// The caller supplies a verification closure that returns `true` if the PIN
/// matches. [`wire_pin_callbacks`][Self::wire_pin_callbacks] wires the digit
/// callbacks against a caller-owned digit buffer; on `Confirm`, the closure is
/// invoked.
pub struct PinEntryScreen {
    component: self::PinEntryScreenUi,
}

impl PinEntryScreen {
    pub fn new(title: &str) -> anyhow::Result<Self> {
        let component = self::PinEntryScreenUi::new()
            .map_err(|e| anyhow::anyhow!("slint component init: {:?}", e))?;
        component.set_menu_title(title.into());
        component.show()
            .map_err(|e| anyhow::anyhow!("slint window show: {:?}", e))?;
        Ok(PinEntryScreen { component })
    }

    /// Wire digit, backspace, confirm, and cancel callbacks — **the correct
    /// implementation** (use this instead of `wire_callbacks`).
    ///
    /// `digit_buf` is a shared buffer maintained by the caller; the digit and
    /// backspace handlers update it and keep the Slint dot-counter in sync.
    ///
    /// `on_confirmed(digits)` is called on ✓ press with the full entered digit
    /// sequence.  The Slint counter is reset to 0 and the buffer cleared before
    /// `on_confirmed` fires, so the handler may immediately set a pending
    /// navigation flag without worrying about stale display state.
    ///
    /// `on_cancelled()` is called on ✕ press.
    pub fn wire_pin_callbacks(
        &self,
        digit_buf: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
        on_confirmed: impl Fn(Vec<u8>) + 'static,
        on_cancelled: impl Fn() + 'static,
    ) {
        // ── digit pressed ─────────────────────────────────────────────────────
        let comp_d = self.component.clone_strong();
        let buf_d  = digit_buf.clone();
        self.component.on_digit_pressed(move |d| {
            let mut buf = buf_d.borrow_mut();
            if buf.len() < 4 {
                // The stored PIN bytes are ASCII ('0'=0x30 … '9'=0x39).
                // The Slint numpad fires digit_pressed(0) … digit_pressed(9) as
                // raw integers; add b'0' to produce the matching ASCII byte.
                buf.push(b'0'.wrapping_add(d as u8));
                comp_d.set_digits_entered(buf.len() as i32);
            }
        });

        // ── backspace ─────────────────────────────────────────────────────────
        let comp_b = self.component.clone_strong();
        let buf_b  = digit_buf.clone();
        self.component.on_backspace_pressed(move || {
            let mut buf = buf_b.borrow_mut();
            if !buf.is_empty() {
                buf.pop();
                comp_b.set_digits_entered(buf.len() as i32);
            }
        });

        // ── confirm ───────────────────────────────────────────────────────────
        let comp_c = self.component.clone_strong();
        let buf_c  = digit_buf.clone();
        self.component.on_confirm_pressed(move || {
            let digits = buf_c.borrow().clone();
            // Reset visual state immediately so the display is clean whether
            // the PIN is correct or wrong.
            comp_c.set_digits_entered(0);
            buf_c.borrow_mut().clear();
            on_confirmed(digits);
        });

        // ── cancel ────────────────────────────────────────────────────────────
        self.component.on_cancel_pressed(on_cancelled);
    }

    // NOTE: no standalone `on_cancel_pressed` / `on_digit_pressed` /
    // `on_backspace_pressed` / `on_confirm_pressed` / `reset` here.
    // `wire_pin_callbacks` above is the sole caller of the component's raw
    // callbacks (each screen is single-use: a fresh `PinEntryScreen` is
    // created per PIN prompt — see `UiRuntime::navigate_to_pin_entry` — so
    // there is no reset-and-reuse path either).

    pub fn hide(&self) { self.component.hide().ok(); }
}
