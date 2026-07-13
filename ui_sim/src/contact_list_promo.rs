// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig producing the promotional landing-page screenshot
//! of the contact-list screen (`site/index.html`'s screenshots gallery).
//!
//! # Why this is a separate, narrower render path from `HostSimUi` / the
//! other `ui_sim` proof rigs
//!
//! `firmware/src/ui/screens/contact_list.rs` cannot itself be compiled on
//! the host — the `firmware` crate cross-compiles for
//! `xtensa-esp32s3-espidf` only (see `lib.rs`'s module doc for the full
//! explanation). Unlike this crate's other narrow, single-mechanism proof
//! rigs (`compose_send.rs`, `gps_status_rows.rs`, …), this module copies
//! `ContactListScreenUi`'s markup VERBATIM in full — every row, the tab
//! bar, the header icon button, the header `SignalMeter` (ADR-0010), the
//! full-window backdrop — because the
//! deliverable here is a promotional screenshot of the REAL screen, not a
//! narrow proof of one previously-unproven mechanism. Imports the REAL
//! `theme.slint` / `motifs.slint` by relative path (not forked token values
//! or re-derived components) — single source of truth, same technique
//! every other `ui_sim` render module uses.
//!
//! Seeded with tasteful, OSS-appropriate sample contacts (space-mission
//! callsigns, no PII, no internal vernacular) — see
//! `ui_sim/src/bin/contact_list_promo_render.rs` for the actual seed data
//! and `site/README.md` for the gallery's regeneration instructions.
//!
//! Slint enforces a process-wide `Platform` singleton, so this module's
//! render entry point must never run in the same process as any other
//! `ui_sim` render rig — each lives in its own `cargo run --bin` process.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::{ModelRc, PhysicalSize, VecModel};

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";
    import { Starfield, CometOnNotify, SpaceBackdrop } from "../../firmware/src/ui/motifs.slint";
    import { SignalMeter } from "../../firmware/src/ui/signal_meter.slint";

    // Verbatim copy of `contact_list.rs`'s markup — see this file's module
    // doc for why a copy (not an import) is used here.

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

        Rectangle {
            background: selected ? Theme.surface-raised : (touch_area.has-hover ? Theme.surface : Theme.nebula-violet-deep.with-alpha(0.12));
            border-radius: 0px;

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

    struct ContactEntry {
        name:    string,
        initial: string,
        preview: string,
        time_str: string,
        unread:  int,
        unread_str: string,
        hash:    int,
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

    export component ContactListPromoUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        in-out property <float> content_opacity: 0;
        animate content_opacity { duration: 200ms; easing: ease-out; }
        init => { content_opacity = 1; }

        in-out property <bool> notify_trigger: false;

        in property <[ContactEntry]>  contacts;
        in property <[ChannelEntry]>  channels;
        in-out property <bool>        show_contacts: true;

        in property <int>    contacts_unread_total;
        in property <string> contacts_unread_str;
        in property <int>    channels_unread_total;
        in property <string> channels_unread_str;

        // Repeater signal-meter reading (ADR-0010): 0 = direct-only,
        // 1..=5 = bars. See `SignalMeter`'s embedding below.
        in property <int> signal_level: 0;

        in property <int> selected_index: -1;

        callback contact_selected(int);
        callback channel_selected(int);
        callback settings_pressed;

        in property <string> touch_debug;

        SpaceBackdrop {}

        VerticalLayout {
            opacity: content_opacity;

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
                        Rectangle {
                            y: parent.height - 2px;
                            height: !show_contacts ? 2px : 0px;
                            width: parent.width;
                            background: Theme.brand-signal;
                        }
                        TouchArea { clicked => { show_contacts = false; } }
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
                    HeaderIconButton {
                        width: 44px;
                        icon: "⚙";
                        clicked => { root.settings_pressed(); }
                    }
                }

                CometOnNotify {
                    x: 0px;
                    y: parent.height - 14px;
                    play: root.notify_trigger;
                }
            }

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

        if touch_debug != "" : Rectangle {
            x: 0;
            y: root.height - 16px;
            width: root.width;
            height: 16px;
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

/// One seeded contact row for the promo screenshot.
pub struct PromoContact {
    pub name: &'static str,
    pub initial: &'static str,
    pub preview: &'static str,
    pub time_str: &'static str,
    pub unread: i32,
}

struct ContactListPromoPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for ContactListPromoPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Render rig for the contact-list promo screenshot.
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `compose_send.rs::ComposeSendFrame::new`'s identical note. Callers must
/// ensure exactly one [`ContactListPromoFrame::new`] runs per process.
pub struct ContactListPromoFrame {
    window: Rc<MinimalSoftwareWindow>,
    ui: ContactListPromoUi,
}

impl ContactListPromoFrame {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(ContactListPromoPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = ContactListPromoUi::new().expect("ContactListPromoUi::new");
        ui.show().expect("ContactListPromoUi::show");

        ContactListPromoFrame { window, ui }
    }

    /// Set the header's repeater signal-meter reading (ADR-0010): 0 =
    /// direct-only ring, 1..=5 = filled-bar count.
    pub fn set_signal_level(&self, bars: i32) {
        self.ui.set_signal_level(bars);
    }

    /// Seed the Contacts tab with `contacts` and compute the aggregate
    /// unread badge the same way `ContactListScreen::set_contacts` does.
    pub fn set_contacts(&self, contacts: &[PromoContact]) {
        let model: VecModel<ContactEntry> = VecModel::default();
        let mut total = 0;
        for c in contacts {
            total += c.unread;
            model.push(ContactEntry {
                name: c.name.into(),
                initial: c.initial.into(),
                preview: c.preview.into(),
                time_str: c.time_str.into(),
                unread: c.unread,
                unread_str: if c.unread > 0 {
                    c.unread.to_string().into()
                } else {
                    "".into()
                },
                hash: 0,
            });
        }
        self.ui.set_contacts(ModelRc::new(model));
        self.ui.set_contacts_unread_total(total);
        self.ui.set_contacts_unread_str(if total > 0 {
            total.to_string().into()
        } else {
            "".into()
        });
        // Empty channels model — the Contacts tab is the one shown.
        self.ui
            .set_channels(ModelRc::new(VecModel::<ChannelEntry>::default()));
        self.ui.set_show_contacts(true);
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
            "contact-list promo frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for ContactListPromoFrame {
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
