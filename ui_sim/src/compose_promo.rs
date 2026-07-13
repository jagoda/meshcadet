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

    component EmojiPickerGrid {
        in property <[EmojiCell]> cells;
        callback emoji_selected(string);

        width:  320px;
        height: 164px;

        in-out property <float> reveal_opacity: 0;
        animate reveal_opacity { duration: 200ms; easing: ease-out; }
        init => { self.reveal_opacity = 1.0; }
        opacity: reveal_opacity;

        Rectangle { background: Theme.surface; }

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
        in property <[EmojiCell]>       emoji_cells;
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

        SpaceBackdrop {}

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
                cells: emoji_cells;
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
        ui.set_emoji_cells(ModelRc::new(VecModel::<EmojiCell>::default()));
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
