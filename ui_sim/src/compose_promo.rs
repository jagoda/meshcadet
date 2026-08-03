// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig producing the promotional landing-page screenshot
//! of the compose screen (`site/index.html`'s screenshots gallery).
//!
//! Same rationale as `contact_list_promo.rs`'s module doc: `firmware/src/ui/
//! screens/compose.rs` cannot itself be compiled on the host, so this
//! module copies `ComposeScreenUi`'s markup VERBATIM in full — header,
//! draft text area, action bar, Send button + `RocketOnSend` — because the
//! deliverable is a promotional screenshot of the REAL screen. The emoji
//! picker overlay and autocomplete bar markup are copied too (referenced by
//! the root component) but are never opened for this screenshot — a clean,
//! populated draft-in-progress is the compelling shot, not the picker
//! overlay. Imports the REAL `theme.slint` / `motifs.slint` by relative
//! path (not forked token values or re-derived components).
//!
//! Captures the Send button ARMED (star-gold, draft populated) with the
//! rocket mid-flight — same "capture mid-flight so the render shows
//! motion, not just a settled/empty end state" choice
//! `compose_send_render.rs` makes; see that module's doc for the full
//! two-render technique this mirrors.
//!
//! Slint enforces a process-wide `Platform` singleton, so this module's
//! render entry point must never run in the same process as any other
//! `ui_sim` render rig.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::{ModelRc, PhysicalSize, VecModel};

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import { RocketOnSend, SpaceBackdrop } from "../../firmware/src/ui/motifs.slint";
    import { SignalMeter } from "../../firmware/src/ui/signal_meter.slint";

    // Verbatim copy of `compose.rs`'s markup — see this file's module doc
    // for why a copy (not an import) is used here.

    struct EmojiCell {
        codepoint_str: string,
        label:         string,
    }

    // ── Category tabs (meshcadet-emoji-picker-expansion) ───────────────────
    //
    // `protocol::emoji::EMOJI_TABLE` grew from 40 to 96 entries (6
    // categories x 16 — campaign D1). Rather than filter a single flat
    // array in Slint (which has no array-filter primitive that would keep
    // the GridLayout's `col`/`row` indices contiguous for a subset), Rust
    // splits `EMOJI_TABLE` into 6 fixed per-category models ONCE at
    // construction (`ComposeScreen::new`/`ComposePromoFrame::new`) and
    // hands each its own property below. Which one is shown is then PURE
    // Slint-local state (`active_category`, exactly like this screen's
    // existing `picker_open` toggle) — no Rust round-trip needed on tab
    // tap.
    component EmojiPickerGrid {
        in property <[EmojiCell]> faces_cells;
        in property <[EmojiCell]> gestures_cells;
        in property <[EmojiCell]> hearts_cells;
        in property <[EmojiCell]> nature_cells;
        in property <[EmojiCell]> fun_cells;
        in property <[EmojiCell]> objects_cells;
        // Tab labels, in the same order as the 6 properties above —
        // populated once from `protocol::emoji::EMOJI_CATEGORIES` so the
        // tab row's text never hand-duplicates that list.
        in property <[string]> category_names;
        in-out property <int> active_category: 0;
        callback emoji_selected(string);

        width:  320px;
        height: 164px;

        in-out property <float> reveal_opacity: 0;
        animate reveal_opacity { duration: 200ms; easing: ease-out; }
        init => { self.reveal_opacity = 1.0; }
        opacity: reveal_opacity;

        Rectangle { background: Theme.surface; }

        // The active category's cells — a ternary chain over the 6 fixed
        // properties above, keyed by `active_category` (0..=5, matching
        // `category_names`'/`EMOJI_CATEGORIES`' order).
        property <[EmojiCell]> active_cells:
            active_category == 0 ? faces_cells :
            active_category == 1 ? gestures_cells :
            active_category == 2 ? hearts_cells :
            active_category == 3 ? nature_cells :
            active_category == 4 ? fun_cells :
            objects_cells;

        // Category tab row — fixed 24px strip along the top (matching this
        // screen's other fixed-height chrome strips: autocomplete bar
        // 32px, action bar 40px). Purely local state: tapping a tab only
        // ever changes `active_category`, which `active_cells` above
        // reacts to — same "Slint owns UI-only state" pattern as
        // `picker_open`.
        Rectangle {
            x: 0px; y: 0px;
            width: parent.width;
            height: 24px;
            HorizontalLayout {
                padding: 2px;
                spacing: 2px;
                for name[i] in category_names : Rectangle {
                    horizontal-stretch: 1.0;
                    background: (i == root.active_category) ? Theme.brand-signal : Theme.surface-raised;
                    animate background { duration: 100ms; easing: ease-out; }
                    border-radius: 4px;
                    Text {
                        text: name;
                        font-size: Theme.size-meta; // 10px
                        color: (i == root.active_category) ? Theme.bg-space : Theme.text-secondary;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }
                    tab_touch := TouchArea {
                        width: parent.width;
                        height: parent.height;
                        clicked => { root.active_category = i; }
                    }
                }
            }
        }

        // Cell grid for the active category — SAME Flickable-wrapped
        // GridLayout technique as the pre-tabs 40-cell picker (see this
        // component's own BUG-FIX history in `compose.rs`'s module doc):
        // an EXPLICIT width/height on the Flickable (164px minus the 24px
        // tab strip above) so `viewport-height` auto-binds to the active
        // category's own natural content height, keeping every one of its
        // (up to 16) cells reachable by touch-scroll — re-verified for the
        // 96-entry/6-tab shape by
        // `ui_sim/tests/compose_picker_categories.rs`, not just assumed to
        // scale from the 40-cell case.
        Flickable {
            x: 0px; y: 24px;
            width: parent.width;
            height: parent.height - 24px;

            GridLayout {
                padding: 4px;
                spacing: 2px;

                for cell[i] in active_cells : Rectangle {
                    col: mod(i, 5);
                    row: floor(i / 5);
                    width: 58px;
                    height: 36px;
                    background: emoji_touch.has-hover ? Theme.select : transparent;
                    animate background { duration: 100ms; easing: ease-out; }
                    border-radius: 6px;

                    Text {
                        text: cell.codepoint_str;
                        font-size: Theme.icon-lg;
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

    struct AutocompleteEntry {
        shortcode: string,
        emoji_str: string,
    }

    component AutocompleteBar {
        in property <[AutocompleteEntry]> entries;
        in property <bool> visible_bar;
        callback selected(string, string);

        height: visible_bar ? 32px : 0px;
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
                        font-size: Theme.size-title;
                        vertical-alignment: center;
                    }
                    Text {
                        text: ":" + e.shortcode + ":";
                        font-size: Theme.size-meta;
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

    export component ComposePromoUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        forward-focus: draft_input;

        in property <string>            to_name;
        in property <[EmojiCell]>       faces_cells;
        in property <[EmojiCell]>       gestures_cells;
        in property <[EmojiCell]>       hearts_cells;
        in property <[EmojiCell]>       nature_cells;
        in property <[EmojiCell]>       fun_cells;
        in property <[EmojiCell]>       objects_cells;
        in property <[string]>          category_names;
        in property <[AutocompleteEntry]> completions;
        in-out property <string>        draft;
        in-out property <bool>          picker_open: false;
        in-out property <bool>          show_completions: false;
        // Repeater signal-meter reading (ADR-0010): 0 = direct-only,
        // 1..=5 = bars. See `SignalMeter`'s embedding below.
        in property <int>               signal_level: 0;
        in-out property <bool>          rocket_trigger: false;
        in-out property <bool>          sent: false;

        callback back_pressed;
        callback send_pressed(string);
        callback emoji_chosen(string);
        callback draft_changed(string);

        public function move_cursor_to_end() {
            draft_input.set-selection-offsets(2147483647, 2147483647);
        }

        Timer {
            interval: 500ms;
            running: root.rocket_trigger;
            triggered => { root.rocket_trigger = false; }
        }

        // `SpaceBackdrop` no longer carries a default `source` (production
        // fan-out feeds it a Rust-shared `Image` instead — see
        // `firmware/src/ui/backdrop_asset.rs`'s module doc); this
        // single-consumer host-sim demo binds the literal directly, no
        // duplication concern applies here.
        SpaceBackdrop { source: @image-url("../../firmware/assets/space/starfield_full.png"); }

        VerticalLayout {
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
                        horizontal-alignment: left;
                        vertical-alignment: center;
                    }

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

            AutocompleteBar {
                entries: completions;
                visible_bar: show_completions;
                selected(sc, em) => {
                    root.draft += em;
                }
            }

            Rectangle {
                background: transparent;
                vertical-stretch: 1.0;

                VerticalLayout {
                    padding: 8px;

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

            Rectangle {
                height: 40px;
                background: Theme.surface.with-alpha(0.55);
                HorizontalLayout {
                    padding: 6px;
                    spacing: 8px;
                    alignment: center;

                    Rectangle {
                        width: 36px; height: 28px;
                        background: picker_open ? Theme.brand-signal : Theme.surface-raised;
                        animate background { duration: 120ms; easing: ease-out; }
                        border-radius: 8px;
                        Text {
                            text: "😀";
                            font-size: Theme.icon-sm;
                            // Kept in sync with compose.rs's real `color:
                            // Theme.star-gold;` (mono-glyph-legibility
                            // mission) — this is a verbatim copy of that
                            // screen's markup (see module doc), so a
                            // fixed-emoji color change on the real screen
                            // must land here too or this promo rig silently
                            // drifts from what it's supposed to mirror.
                            color: Theme.star-gold;
                            horizontal-alignment: center;
                            vertical-alignment: center;
                        }
                        TouchArea {
                            width: parent.width;
                            height: parent.height;
                            clicked => { picker_open = !picker_open; }
                        }
                    }

                    Rectangle { horizontal-stretch: 1.0; }

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
                            enabled: draft != "" && !root.sent;
                            clicked => {
                                root.sent = true;
                                root.send_pressed(draft);
                                root.rocket_trigger = true;
                            }
                        }

                        RocketOnSend {
                            x: parent.width / 2 - self.width / 2;
                            y: -20px;
                            play: root.rocket_trigger;
                        }
                    }
                }
            }

            if picker_open : EmojiPickerGrid {
                faces_cells: faces_cells;
                gestures_cells: gestures_cells;
                hearts_cells: hearts_cells;
                nature_cells: nature_cells;
                fun_cells: fun_cells;
                objects_cells: objects_cells;
                category_names: category_names;
                height: 164px;
                emoji_selected(cp) => {
                    root.draft += cp;
                    root.move_cursor_to_end();
                    root.picker_open = false;
                }
            }
        }
    }
}

struct ComposePromoPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for ComposePromoPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Render rig for the compose promo screenshot.
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `compose_send.rs::ComposeSendFrame::new`'s identical note. Callers must
/// ensure exactly one [`ComposePromoFrame::new`] runs per process.
pub struct ComposePromoFrame {
    window: Rc<MinimalSoftwareWindow>,
    ui: ComposePromoUi,
}

impl ComposePromoFrame {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(ComposePromoPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = ComposePromoUi::new().expect("ComposePromoUi::new");
        ui.show().expect("ComposePromoUi::show");
        // Empty models — the picker/autocomplete overlays are never opened
        // for this screenshot (see module doc), but the properties are
        // still `in property` on the root and must be given a model.
        ui.set_faces_cells(ModelRc::new(VecModel::<EmojiCell>::default()));
        ui.set_gestures_cells(ModelRc::new(VecModel::<EmojiCell>::default()));
        ui.set_hearts_cells(ModelRc::new(VecModel::<EmojiCell>::default()));
        ui.set_nature_cells(ModelRc::new(VecModel::<EmojiCell>::default()));
        ui.set_fun_cells(ModelRc::new(VecModel::<EmojiCell>::default()));
        ui.set_objects_cells(ModelRc::new(VecModel::<EmojiCell>::default()));
        ui.set_category_names(ModelRc::new(VecModel::<slint::SharedString>::default()));
        ui.set_completions(ModelRc::new(VecModel::<AutocompleteEntry>::default()));

        ComposePromoFrame { window, ui }
    }

    pub fn set_to_name(&self, name: &str) {
        self.ui.set_to_name(name.into());
    }

    /// Set the header's repeater signal-meter reading (ADR-0010): 0 =
    /// direct-only ring, 1..=5 = filled-bar count.
    pub fn set_signal_level(&self, bars: i32) {
        self.ui.set_signal_level(bars);
    }

    pub fn set_draft(&self, text: &str) {
        self.ui.set_draft(text.into());
    }

    pub fn set_rocket_trigger(&self, v: bool) {
        self.ui.set_rocket_trigger(v);
    }

    /// Advance Slint's animation clock and render one frame.
    pub fn render(&self) -> Vec<Rgb565Pixel> {
        slint::platform::update_timers_and_animations();
        self.window.request_redraw();

        let mut framebuffer = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
        let rendered = self.window.draw_if_needed(|renderer| {
            renderer.render(&mut framebuffer, WIDTH as usize);
        });
        assert!(
            rendered,
            "compose promo frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for ComposePromoFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a rendered RGB565 framebuffer to an `image::RgbImage` (RGB8) for
/// PNG export — same conversion every other `ui_sim` render module
/// duplicates locally.
pub fn framebuffer_to_rgb_image(
    framebuffer: &[Rgb565Pixel],
    width: u32,
    height: u32,
) -> image::RgbImage {
    let mut img = image::RgbImage::new(width, height);
    for (i, px) in framebuffer.iter().enumerate() {
        let r5 = (px.0 >> 11) & 0x1F;
        let g6 = (px.0 >> 5) & 0x3F;
        let b5 = px.0 & 0x1F;
        let r8 = ((r5 << 3) | (r5 >> 2)) as u8;
        let g8 = ((g6 << 2) | (g6 >> 4)) as u8;
        let b8 = ((b5 << 3) | (b5 >> 2)) as u8;
        let x = (i as u32) % width;
        let y = (i as u32) / width;
        img.put_pixel(x, y, image::Rgb([r8, g8, b8]));
    }
    img
}
