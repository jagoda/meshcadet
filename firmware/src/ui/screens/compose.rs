// SPDX-License-Identifier: GPL-3.0-only
//! Message compose screen — draft + send with emoji picker.
//!
//! # Layout
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │ ‹  To: Alice                        │  header bar (36 px)
//! ├─────────────────────────────────────┤
//! │                                     │
//! │  [text input area — 4 lines]        │  compose area (grows, ≤ 100 px)
//! │                                     │
//! ├─────────────────────────────────────┤
//! │ [😊][❤][👍][…]  |  📤 Send  | 😀  │  action bar (40 px)
//! ├─────────────────────────────────────┤
//! │  EMOJI PICKER GRID (5×8, 40 cells)  │  emoji overlay (visible when open)
//! └─────────────────────────────────────┘
//! ```
//!
//! # Emoji entry
//!
//! Two pathways:
//! 1. **Shortcode** — the user types `:` on the physical keyboard; as they
//!    type letters, `shortcode_completions()` suggests matching emoji inline
//!    (a 5-entry autocomplete row appears above the text area).  Tapping a
//!    suggestion inserts the codepoint and closes the autocomplete.
//! 2. **Picker grid** — tapping the 😀 button in the action bar opens a
//!    full-screen overlay grid of the 40 curated emoji.  Tapping any cell
//!    inserts that emoji's codepoint at the cursor and closes the picker.
//!
//! The `expand_shortcodes()` function in `protocol::emoji` is called on the
//! final text just before encoding the outbound DM, not at compose time.
//! This keeps the raw `:shortcode:` in the text area so the user can still
//! edit it, and only expands on Send.
//!
//! # Theme tokens + animation language
//!
//! Every color/font-size literal in this screen's `slint::slint!{}` block
//! (below) now reads from the shared `Theme` global (`ui/theme.slint`,
//! imported below) instead of a bare hex/px literal — same tokens, same
//! values, so this is a pixel-identical swap (see `splash.rs`'s doc for the
//! pilot precedent). Two one-shot animations apply this UI's "never an
//! infinite loop, never cut off mid-cycle" animation language to this
//! screen's own affordances (not a screen-entry animation — this screen is
//! reached by interactive navigation, not boot, so there is no splash-style
//! deferred-start concern to work around):
//! - The emoji-picker overlay fades in once, every time it mounts (`init`
//!   writes a hidden→settled opacity, exactly like `SplashScreen`'s
//!   deferred-write mechanism, just self-contained in Slint here since
//!   `UiRuntime::step()` is already ticking every frame before this screen
//!   is ever navigated to).
//! - The shortcode-autocomplete bar's height, and the toggle/send buttons'
//!   background+text colors, get a short `animate` on their existing
//!   state-driven bindings (no new Rust wiring) instead of an instant snap.
//! This is presentation-only: no Rust wrapper method below was touched.
//!
//! # Outer-space theme (per-screen spec row 5: "star-gold send
//! affordance" / "rocket-on-send")
//!
//! Two additive changes on top of the section above, both still
//! presentation-only:
//! - The Send button's enabled-state accent switches from `Theme.brand-
//!   signal` (cyan, still used everywhere else in this file — the back
//!   chevron) to the widened palette's `Theme.star-gold`, per this
//!   screen's row in the per-screen spec table.
//! - `RocketOnSend` (`ui/motifs.slint`) floats above the Send button and
//!   fires its one-shot arc-up
//!   + fade whenever `rocket_trigger` flips to `true` — done inline in the
//!   Send button's own `clicked` handler, entirely within this file's
//!   `slint::slint!{}` block (no new Rust wrapper method, no change to any
//!   existing callback signature). A sibling `Timer` auto-resets
//!   `rocket_trigger` back to `false` ~100ms after the 400ms animation
//!   settles, re-arming the one-shot for the next send-tap — exactly the
//!   "resetting `play` back to `false` ... is the consuming screen's
//!   responsibility" contract `RocketOnSend`'s own doc comment documents.
//!
//! BUG FIX: on real
//! hardware this one-shot never visibly played. `send_pressed`'s Rust
//! handler (`ui/mod.rs`'s `navigate_to_compose`) used to queue the
//! Compose → MessageView navigation in the very same tick as the Send tap,
//! so this screen (and the `RocketOnSend` floating on it) was torn down
//! before the 400ms arc-up+fade ever rendered a frame. `ui/mod.rs`'s
//! `step()` now defers that navigation ~450ms (see its
//! `deferred_message_view_nav_at_ms` doc) — the send itself is still
//! synchronous and unaffected, only the screen swap trails the animation.
//! That widened the window this screen stays live and its Send button stays
//! tappable, so a `sent` latch (declared above, alongside
//! `rocket_trigger`) now gates the button after the first tap to prevent a
//! second tap from queuing a duplicate send.
//!
//! # BUG FIXes
//!
//! 1. **Back chevron centered instead of left-aligned.** The header's
//!    `HorizontalLayout` carried `alignment: center;`, which packs children at
//!    their natural size and centers that packed block as a whole — instead of
//!    stretching the "To: name" `Text` (its `horizontal-stretch: 1.0`) to fill
//!    the remaining width, the back-button `Rectangle` + title text got
//!    centered together as a unit, pulling the chevron away from the left
//!    edge. Every sibling header (`message_view.rs`, `gps_status.rs`) omits
//!    this property and relies on the stretch instead — removed here to match.
//! 2. **Cursor prepended instead of appended on keypress-seeded compose
//!    entry.** `navigate_to_compose` (`ui/mod.rs`) seeds a physical-keyboard
//!    keypress into `draft` via `set_draft` before the user has typed
//!    anything else. Assigning `draft`/`text` programmatically does not move
//!    the `TextInput`'s cursor — it stays at byte offset 0 (the start) — so
//!    continued typing inserted ahead of the seeded character instead of
//!    after it. `move_cursor_to_end()` (below) is invoked right after
//!    `set_draft` to fix this.
//!
//! # BUG FIXes
//!
//! 1. **Bottom picker rows clipped and untappable.** `EmojiPickerGrid`'s 5×8
//!    `GridLayout` (40 cells @ 58×36px) has a natural content height of
//!    ~310px, but the overlay component itself is only 164px tall (all the
//!    compose window's 240px are otherwise spoken for by the header + action
//!    bar). With no scroll container, the bottom ~3 rows (cells 25-39) laid
//!    out past the visible/physical panel bounds and were simply never
//!    painted or reachable by touch. Fixed by wrapping the `GridLayout` in a
//!    `Flickable` (same idiom as `message_view.rs`'s `flick` and
//!    `contact_list.rs`'s `main_flick`) — see the doc comment at the
//!    `Flickable` site for why it needs an *explicit* width/height rather
//!    than relying on default-fill.
//! 2. **Cursor left before the just-picked emoji.** The `emoji_selected`
//!    handler assigns `root.draft += cp` — same "assign `draft` /
//!    `draft_input.text` programmatically" pattern as BUG FIX 2 above — which
//!    does not move the `TextInput`'s cursor, so it was left wherever it sat
//!    before the pick instead of landing after the newly inserted codepoint.
//!    `move_cursor_to_end()` is now invoked right after the append, same
//!    fix, same reason.
//!
//! # Full-window starfield backdrop
//!
//! `SpaceBackdrop` is now the `Window`'s first (z-bottom) child, reversing
//! the earlier "scrolling-content screens excluded from backdrop" call.
//! Two fills changed so the backdrop shows through the
//! main pane and behind/below the Send button: the draft text area's
//! background dropped from opaque `Theme.bg-space` to bare `transparent`
//! (same rest-state fill the settings view's `ToggleRow`/`NavRow` use), and
//! the action bar's background switched from opaque `Theme.surface` to the
//! translucent `Theme.surface.with-alpha(0.55)` wash `contact_list.rs`'s
//! header uses. The header bar and the emoji-picker overlay's own opaque
//! backdrop Rectangle (a dense, small-cell touch grid) are unchanged —
//! out of scope for the four named screens/panes this change touches, and left opaque for
//! touch-target legibility. Both action-bar buttons (emoji-picker toggle,
//! Send) keep their existing opaque pill fills.

slint::slint! {
    import { Theme } from "../theme.slint";
    import { RocketOnSend, SpaceBackdrop } from "../motifs.slint";
    import { SignalMeter } from "../signal_meter.slint";

    // ── Emoji picker overlay ──────────────────────────────────────────────────

    struct EmojiCell {
        codepoint_str: string,  // UTF-8 char as string (Slint Text renders it)
        label:         string,
    }

    component EmojiPickerGrid {
        in property <[EmojiCell]> cells;
        callback emoji_selected(string);

        width:  320px;
        height: 164px;

        // One-shot reveal fade — the overlay's own "screen entry" per the
        // theme's animation language (module: `theme.slint`'s doc).
        // `reveal_opacity` is declared at its
        // hidden (0) default; `init` fires exactly once per mount (this
        // component is created/destroyed by the `if picker_open` conditional
        // in `ComposeScreenUi` below, so it re-fires — and re-fades-in — every
        // time the picker opens) and writes the settled (1.0) value, which is
        // the change that triggers `animate` below. Same deferred-write
        // mechanism as `splash.rs`'s `start_animation`, but self-contained in
        // Slint since there is no boot-bring-up gap here to defer past: the
        // dispatcher's `step()` loop is already ticking every frame by the
        // time a screen is interactively navigated to.
        in-out property <float> reveal_opacity: 0;
        animate reveal_opacity { duration: 200ms; easing: ease-out; }
        init => { self.reveal_opacity = 1.0; }
        opacity: reveal_opacity;

        // Opaque backdrop (background isn't a property of the implicit root)
        Rectangle { background: Theme.surface; }

        // 5 columns × 8 rows = 40 cells at 58×36px + 2px spacing + 4px padding
        // want 5*58=290 × 8*36 + 7*2 + 2*4 = 310px, but this component is only
        // 164px tall (the compose window has just 240px total, and the header
        // + action bar above already claim 76px of it) — the grid's natural
        // content height overflows the visible overlay by ~146px. Without a
        // scroll container that overflow renders past the physical panel's
        // bottom edge, so rows 5-7 (cells 25-39) are silently clipped by the
        // hardware boundary: present in the model, laid out by GridLayout, but
        // never painted and never reachable by touch (BUG: unreachable emoji).
        // Wrapping the grid in a `Flickable` fixes this the same way
        // `message_view.rs`'s `flick` does for the message list: giving the
        // Flickable an explicit width/height (rather than leaving it to
        // default-fill, which the compiler's `flickable` pass would instead
        // resolve by growing the Flickable itself to the content's preferred
        // size — see i-slint-compiler's `passes/flickable.rs::fixup_geometry`,
        // which only forwards a content-driven height when the Flickable has
        // no explicit height binding of its own) keeps the *visible* viewport
        // pinned at this component's actual 164px, while `viewport-height`
        // auto-binds to `max(164px, grid.min-height)` = the grid's full
        // 310px — so every row lays out at its natural size and the
        // now-taller-than-visible content becomes touch-scrollable instead of
        // clipped. All 40 cells stay reachable; the picker just scrolls.
        Flickable {
            width: parent.width;
            height: parent.height;

            GridLayout {
                padding: 4px;
                spacing: 2px;

                for cell[i] in cells : Rectangle {
                    col: mod(i, 5);
                    row: floor(i / 5);
                    width: 58px;
                    height: 36px;
                    background: emoji_touch.has-hover ? Theme.select : transparent;
                    animate background { duration: 100ms; easing: ease-out; }
                    border-radius: 6px;

                    Text {
                        text: cell.codepoint_str;
                        font-size: Theme.icon-lg; // 20px
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }

                    emoji_touch := TouchArea {
                        width: parent.width;
                        height: parent.height;
                        clicked => { root.emoji_selected(cell.codepoint_str); }
                    }
                }
            }
        }
    }

    // ── Shortcode autocomplete bar ─────────────────────────────────────────────

    struct AutocompleteEntry {
        shortcode: string,
        emoji_str: string,
    }

    component AutocompleteBar {
        in property <[AutocompleteEntry]> entries;
        in property <bool> visible_bar;
        callback selected(string, string);  // (shortcode, emoji_str)

        height: visible_bar ? 32px : 0px;
        // One-shot slide per the theme's animation language: this bar is
        // always mounted (only its `height` toggles), so — unlike
        // `EmojiPickerGrid`'s mount/unmount reveal above — a direct `animate`
        // on the live `visible_bar`-driven binding is enough; no deferred
        // `init` write needed.
        animate height { duration: 150ms; easing: ease-out; }

        Rectangle {
            clip: true;
            HorizontalLayout {
            spacing: 2px;
            padding: 2px;
            for e in entries : Rectangle {
                background: bar_touch.has-hover ? Theme.select : Theme.surface;
                animate background { duration: 100ms; easing: ease-out; }
                border-radius: 6px;
                min-width: 60px;
                HorizontalLayout {
                    spacing: 4px;
                    padding-left: 6px;
                    padding-right: 6px;
                    Text {
                        text: e.emoji_str;
                        font-size: Theme.size-title; // 16px
                        vertical-alignment: center;
                    }
                    Text {
                        text: ":" + e.shortcode + ":";
                        font-size: Theme.size-meta; // 10px
                        color: Theme.text-secondary;
                        vertical-alignment: center;
                    }
                }
                bar_touch := TouchArea {
                    clicked => { root.selected(e.shortcode, e.emoji_str); }
                }
            }
            }
        }
    }

    // ── Main compose screen ───────────────────────────────────────────────────

    export component ComposeScreenUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        // Physical keyboard is the only text-entry path (no on-screen keyboard).
        // Forward window focus to the draft input so key events injected via
        // `dispatch_key` land in the TextInput as soon as the screen is shown.
        forward-focus: draft_input;

        in property <string>            to_name;
        in property <[EmojiCell]>       emoji_cells;
        in property <[AutocompleteEntry]> completions;
        in-out property <string>        draft;         // two-way bound to TextInput
        in-out property <bool>          picker_open: false;
        in-out property <bool>          show_completions: false;
        // Repeater signal-meter reading (ADR-0010): 0 = direct-only,
        // 1..=5 = bars. Pushed by `ComposeScreen::set_signal_level`; see
        // `SignalMeter`'s embedding below.
        in property <int>               signal_level: 0;

        // Drives the Send button's `RocketOnSend` one-shot (see module doc's
        // "Outer-space theme" section) — flipped `true` in the Send button's
        // own `clicked` handler below, and auto-reset back to `false` by the
        // sibling `Timer` once the fired-state animation has settled.
        in-out property <bool>          rocket_trigger: false;

        // BUG FIX:
        // `ui/mod.rs` now defers the Compose → MessageView navigation ~450ms
        // past the Send tap so `RocketOnSend`'s one-shot has time to render
        // before this screen is torn down (see that file's
        // `deferred_message_view_nav_at_ms` doc). That means this screen —
        // and its Send button — stays live and (absent this flag) still
        // tappable for the whole deferred window; without a guard a second
        // tap in that window would re-fire `send_pressed` and queue a
        // duplicate send of the same draft. `sent` latches `true` on the
        // first tap and gates both the button's `enabled` state and the
        // `clicked` handler below; never reset back to `false` because this
        // screen is always freshly constructed on the next
        // `navigate_to_compose` (see the `ComposeScreen` NOTE below on why
        // there's no `clear_draft`), so a fresh instance already starts with
        // `sent: false`.
        in-out property <bool>          sent: false;

        callback back_pressed;
        callback send_pressed(string);      // sends the draft text
        callback emoji_chosen(string);      // inserts a codepoint into draft
        callback draft_changed(string);     // notifies Rust of text change (for autocomplete)

        // Move the text cursor to the end of whatever `draft` currently holds.
        // Called from Rust right after `set_draft` seeds a keypress-to-write
        // character (see `navigate_to_compose` in `ui/mod.rs`) — assigning
        // `draft`/`text` programmatically does not itself move the TextInput's
        // cursor, so without this the seeded character is left BEFORE the
        // cursor and continued typing prepends ahead of it instead of
        // appending after it. `set-selection-offsets` clamps any out-of-range
        // byte offset to the text's length (see Slint's `safe_byte_offset`),
        // so an oversized sentinel always lands exactly at the end regardless
        // of the seeded text's byte length. Same "Rust drives a public
        // function after a property update" pattern as
        // `ContactListScreenUi.scroll_selected_into_view()`.
        public function move_cursor_to_end() {
            draft_input.set-selection-offsets(2147483647, 2147483647);
        }

        // Auto-reset for `rocket_trigger` (see module doc + Send button
        // below). `running` is bound to the trigger itself: arming the
        // trigger starts the timer, and the instant it fires and clears the
        // trigger back to `false`, `running` follows it back to `false` too
        // — a self-disabling one-shot, no separate "armed" bookkeeping
        // needed. 500ms comfortably clears `RocketOnSend`'s 400ms
        // arc-up+fade duration before quietly resetting the rocket back to
        // its rest position, off the user's attention by the time the
        // reverse transition plays.
        Timer {
            interval: 500ms;
            running: root.rocket_trigger;
            triggered => { root.rocket_trigger = false; }
        }

        // Full-window dim starfield backdrop — same z-bottom placement
        // `admin_menu.rs`'s `SpaceBackdrop` doc establishes, declared first
        // so it paints behind the header, the draft text area, and the
        // action bar (Send button) below. The draft area's own background
        // is switched to `transparent` and the action bar's to a
        // translucent wash (see those Rectangles below) so the backdrop
        // shows behind AND below the Send button too, by deliberate design
        // — matching the settings view's reference
        // treatment of transparent content over this shared backdrop.
        SpaceBackdrop {}

        VerticalLayout {
            // ── Header bar ─────────────────────────────────────────────────
            Rectangle {
                height: 36px;
                background: Theme.surface;
                HorizontalLayout {
                    padding-left: 4px;
                    padding-right: 8px;
                    spacing: 4px;
                    Rectangle {
                        width: 44px; height: 36px;
                        Text { text: "‹"; font-size: Theme.size-display; color: Theme.brand-signal;
                               horizontal-alignment: center; vertical-alignment: center; }
                        back_touch := TouchArea {
                            width: parent.width;
                            height: parent.height;
                            clicked => { root.back_pressed(); }
                        }
                    }
                    Text {
                        text: "To: " + to_name;
                        font-size: Theme.size-body; color: Theme.text-primary;
                        horizontal-stretch: 1.0;
                        // BUG FIX:
                        // vertical-alignment was unset, so Slint's Text default
                        // (top) rendered this flush against the top edge of the
                        // 36px header bar instead of centered in it — the sibling
                        // back-chevron Text above sets vertical-alignment: center
                        // explicitly; this one never did. horizontal-alignment is
                        // set to `left` explicitly (matching the prior default)
                        // rather than `center` like message_view.rs/gps_status.rs's
                        // page-title Text: those headers center a static title
                        // across the full bar (with a balancing spacer Rectangle),
                        // but "To: <name>" is a label+value line, not a title, and
                        // this file's own doc comment above already
                        // establishes left-reading as this header's intended
                        // layout — see the ASCII diagram at the top of this file.
                        horizontal-alignment: left;
                        vertical-alignment: center;
                    }

                    // `SignalMeter` (ADR-0010) — this header has no existing
                    // trailing spacer to nest into (unlike `gps_status.rs`/
                    // `message_view.rs`, whose centered titles already carry
                    // one for balance; "To: <name>" here is left-reading, not
                    // centered, so there is nothing to balance). A small new
                    // flow child at the end reserves just enough width for
                    // the meter; the "To: <name>" Text above simply gets
                    // `horizontal-stretch: 1.0`'s remaining space minus this
                    // reservation — it stays left-aligned and un-clipped for
                    // every contact/channel name this header has ever shown.
                    Rectangle {
                        width: 26px; height: 36px;
                        SignalMeter {
                            signal-level: root.signal_level;
                            width: 16px;
                            height: 14px;
                            x: (parent.width - self.width) / 2;
                            y: (parent.height - self.height) / 2;
                        }
                    }
                }
            }

            // ── Autocomplete bar (appears when typing :shortcode) ───────────
            AutocompleteBar {
                entries: completions;
                visible_bar: show_completions;
                selected(sc, em) => {
                    root.draft += em;
                }
            }

            // ── Draft text area ─────────────────────────────────────────────
            // Fill switched from opaque `Theme.bg-space` to `transparent` —
            // same bare `transparent` fill the settings view's `ToggleRow`/
            // `NavRow` rest-state uses over their shared `SpaceBackdrop`, so
            // the starfield shows directly behind the typed draft text.
            Rectangle {
                background: transparent;
                vertical-stretch: 1.0;

                // `padding` has no effect on a plain `Rectangle` (it's not a
                // layout element) — the 8px inset needs an actual layout
                // wrapper around the single child to take effect.
                VerticalLayout {
                    padding: 8px;

                    // `single-line: false` + `wrap: word-wrap` keep long typed text
                    // auto-wrapping across multiple visual lines, but this TextInput
                    // never sees a Return/Enter key at all: `UiRuntime::step()`
                    // (firmware/src/ui/mod.rs) intercepts the Return byte from the
                    // physical keyboard BEFORE dispatching into Slint and triggers
                    // Send instead (this mesh
                    // device's short messages don't need literal multi-line entry,
                    // so Return-inserts-newline is superseded outright rather than
                    // left reachable through some other path). Nothing here needs
                    // to special-case Return; the interception happens upstream.
                    draft_input := TextInput {
                        text <=> draft;
                        font-size: Theme.size-body-lg;
                        color: Theme.text-primary;
                        wrap: word-wrap;
                        single-line: false;
                        init => { self.focus(); }
                        edited => { root.draft_changed(self.text); }
                    }
                }
            }

            // ── Action bar ──────────────────────────────────────────────────
            // Fill switched from opaque `Theme.surface` to the same
            // translucent wash `contact_list.rs`'s header uses — so the
            // backdrop shows behind AND below the Send button, by deliberate
            // design. Both child buttons (emoji
            // picker toggle, Send) keep their own opaque pill fills,
            // unchanged, so tap-target contrast is unaffected.
            Rectangle {
                height: 40px;
                background: Theme.surface.with-alpha(0.55);
                HorizontalLayout {
                    padding: 6px;
                    spacing: 8px;
                    alignment: center;

                    // Emoji picker toggle
                    Rectangle {
                        width: 36px; height: 28px;
                        background: picker_open ? Theme.brand-signal : Theme.surface-raised;
                        // Subtle one-shot feedback transition on toggle — the
                        // theme's animation language applied to interactive
                        // state, not a screen entry (see EmojiPickerGrid above
                        // for that case).
                        animate background { duration: 120ms; easing: ease-out; }
                        border-radius: 8px;
                        Text {
                            text: "😀";
                            font-size: Theme.icon-sm; // 18px
                            horizontal-alignment: center;
                            vertical-alignment: center;
                        }
                        TouchArea {
                            width: parent.width;
                            height: parent.height;
                            clicked => { picker_open = !picker_open; }
                        }
                    }

                    // Spacer
                    Rectangle { horizontal-stretch: 1.0; }

                    // Send button — `star-gold` affordance + rocket-on-send
                    // (see module doc's "Outer-space theme" section).
                    Rectangle {
                        width: 80px; height: 28px;
                        background: draft != "" ? Theme.star-gold : Theme.surface-raised;
                        animate background { duration: 120ms; easing: ease-out; }
                        border-radius: 14px;
                        Text {
                            text: "📤 Send";
                            font-size: Theme.size-body; font-weight: 600;
                            color: draft != "" ? Theme.bg-space : Theme.text-secondary;
                            animate color { duration: 120ms; easing: ease-out; }
                            horizontal-alignment: center;
                            vertical-alignment: center;
                        }
                        TouchArea {
                            width: parent.width;
                            height: parent.height;
                            // `!root.sent` — see that property's doc above:
                            // this screen now outlives the send tap by the
                            // deferred nav window, so the button must stop
                            // reacting after the first tap or a second tap
                            // would queue a duplicate send.
                            enabled: draft != "" && !root.sent;
                            clicked => {
                                root.sent = true;
                                root.send_pressed(draft);
                                root.rocket_trigger = true;
                            }
                        }

                        // Floats above the button on its own explicit x/y —
                        // does not perturb this Rectangle's fixed 80x28 size
                        // or the action bar's HorizontalLayout flow.
                        RocketOnSend {
                            x: parent.width / 2 - self.width / 2;
                            y: -20px;
                            play: root.rocket_trigger;
                        }
                    }
                }
            }

            // ── Emoji picker overlay (overlays rest of screen when open) ────
            if picker_open : EmojiPickerGrid {
                cells: emoji_cells;
                height: 164px;
                emoji_selected(cp) => {
                    // `root.draft += cp` assigns `draft`/the two-way-bound
                    // `draft_input.text` programmatically — same mechanism as
                    // `set_draft` in `navigate_to_compose` (see the module-doc
                    // BUG FIXes section) — which does NOT move the
                    // `TextInput`'s cursor. Left alone, the cursor stays at
                    // wherever it was before the insert (typically right
                    // before the newly appended codepoint), so continued
                    // typing would land ahead of the just-picked emoji instead
                    // of after it. The `+=` always appends at the very end of
                    // `draft` (there is no mid-draft insertion path here), so
                    // "after the inserted emoji" and "end of draft" coincide —
                    // `move_cursor_to_end()` is exactly the fix, mirroring the
                    // keypress-seeded-draft precedent.
                    root.draft += cp;
                    root.move_cursor_to_end();
                    root.picker_open = false;
                }
            }
        }
    }
}

/// Rust-side wrapper.
pub struct ComposeScreen {
    component: self::ComposeScreenUi,
}

impl ComposeScreen {
    pub fn new() -> anyhow::Result<Self> {
        use protocol::emoji::EMOJI_TABLE;

        let component = self::ComposeScreenUi::new()
            .map_err(|e| anyhow::anyhow!("slint component init: {:?}", e))?;

        // Populate the emoji picker grid from the canonical table.
        let cells: slint::VecModel<EmojiCell> = slint::VecModel::default();
        for entry in EMOJI_TABLE {
            cells.push(EmojiCell {
                codepoint_str: entry.codepoint.to_string().into(),
                label: entry.label.into(),
            });
        }
        component.set_emoji_cells(slint::ModelRc::new(cells));
        component.show()
            .map_err(|e| anyhow::anyhow!("slint window show: {:?}", e))?;
        Ok(ComposeScreen { component })
    }

    pub fn set_to_name(&self, name: &str) {
        self.component.set_to_name(name.into());
    }

    /// Push a fresh repeater signal-meter reading (ADR-0010) into the
    /// header's `SignalMeter` — see `GpsStatusScreen::set_signal_level`'s
    /// identical doc for the `bars` contract.
    pub fn set_signal_level(&self, bars: i32) {
        self.component.set_signal_level(bars);
    }

    pub fn get_draft(&self) -> String {
        self.component.get_draft().to_string()
    }

    pub fn set_draft(&self, text: &str) {
        self.component.set_draft(text.into());
    }

    /// Move the text cursor to the end of the current draft — call after
    /// [`Self::set_draft`] when the text was seeded programmatically (e.g.
    /// the keypress-to-write-mode path in `navigate_to_compose`) so continued
    /// typing appends instead of prepending ahead of the seeded character.
    pub fn move_cursor_to_end(&self) {
        self.component.invoke_move_cursor_to_end();
    }

    // NOTE: no `clear_draft` here. `navigate_to_compose` (ui/mod.rs) always
    // builds a fresh `ComposeScreen::new()` on entry rather than reusing one,
    // so the draft starts empty by construction — an explicit clear is never
    // reached.

    /// Update the autocomplete completions (call from Rust when `draft_changed` fires).
    pub fn set_completions(&self, completions: &[(&'static str, char)]) {
        let model = slint::VecModel::<AutocompleteEntry>::default();
        for &(shortcode, codepoint) in completions {
            model.push(AutocompleteEntry {
                shortcode: shortcode.into(),
                emoji_str: codepoint.to_string().into(),
            });
        }
        self.component.set_completions(slint::ModelRc::new(model));
        self.component.set_show_completions(!completions.is_empty());
    }

    pub fn on_back_pressed(&self, cb: impl Fn() + 'static) {
        self.component.on_back_pressed(cb);
    }

    pub fn on_send_pressed(&self, cb: impl Fn(String) + 'static) {
        self.component.on_send_pressed(move |s| cb(s.to_string()));
    }

    /// Fire `RocketOnSend`'s one-shot exactly as the Send button's own
    /// `clicked` handler does (see the "Outer-space theme" section of this
    /// module's doc comment, `root.rocket_trigger = true`). The physical
    /// Return/Enter key sends via a completely separate path — `ui/mod.rs`'s
    /// `step()` intercepts the Return byte upstream of Slint (see
    /// `draft_input`'s comment above) and never runs the Send button's
    /// `clicked` handler at all, so before this method existed Return-to-send
    /// never set `rocket_trigger` and the rocket never appeared. Poking the property
    /// directly from Rust here is the mirror of the button's own flip; the
    /// component's own `Timer` (see its doc above) auto-resets it back to
    /// `false` the same way regardless of which path armed it, so re-arming
    /// for a later send is unaffected by which path fired it.
    pub fn trigger_rocket(&self) {
        self.component.set_rocket_trigger(true);
    }

    // NOTE: no Rust-side `on_emoji_chosen` / `on_draft_changed` subscriptions.
    // Emoji insertion (`root.draft += cp`, inline in the picker's
    // `emoji_selected` handler above) never calls the declared `emoji_chosen`
    // callback at all; `draft_changed` does fire on every `edited`, but
    // autocomplete refresh is driven by `refresh_completions` (called from
    // `UiRuntime::step` after each keystroke) instead of subscribing to it.
    // Either way, nothing on the Rust side listens, so a wrapper here would
    // itself be dead code.

    /// Recompute the `:shortcode:` autocomplete bar from the current draft.
    ///
    /// Called after each physical-keyboard keystroke (see `UiRuntime::step`).
    /// Finds the shortcode token currently being typed — the text after the
    /// last `:` that has not been closed by whitespace or another `:` — and
    /// shows up to five matching emoji.  Typing a bare `:` yields an empty
    /// prefix that matches all shortcodes, so the bar opens immediately
    /// (HIL squawk #1 acceptance: "typing `:` opens shortcode autocomplete").
    /// Hides the bar when the cursor is not inside a shortcode.
    pub fn refresh_completions(&self) {
        use protocol::emoji::{lookup_shortcode, shortcode_completions};
        let draft = self.get_draft();
        match current_shortcode_prefix(&draft) {
            Some(prefix) => {
                let mut found = [""; 5];
                let n = shortcode_completions(prefix, &mut found);
                let pairs: Vec<(&'static str, char)> = found[..n]
                    .iter()
                    .filter_map(|&sc| lookup_shortcode(sc).map(|e| (sc, e.codepoint)))
                    .collect();
                self.set_completions(&pairs);
            }
            None => {
                self.component.set_show_completions(false);
            }
        }
    }

    pub fn hide(&self) { self.component.hide().ok(); }
}

// `current_shortcode_prefix` is pure Rust with no Slint dependency — it now
// lives in `firmware_core::ui::compose` so its tests execute under `cargo
// test --workspace` (this crate is a detached, cross-compiled workspace —
// see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block written here
// would type-check but never run). Only this Slint-backed view wrapper
// stays. See `docs/adr/0005-firmware-core-extraction.md`.
use firmware_core::ui::compose::current_shortcode_prefix;
