// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig producing the promotional landing-page screenshot
//! of the message-view screen (`site/index.html`'s screenshots gallery).
//!
//! Same rationale as `contact_list_promo.rs`'s module doc: `firmware/src/ui/
//! screens/message_view.rs` cannot itself be compiled on the host (the
//! `firmware` crate cross-compiles for `xtensa-esp32s3-espidf` only), so
//! this module copies `MessageViewScreenUi`'s markup VERBATIM in full —
//! every bubble, the header, the Write button — because the deliverable is
//! a promotional screenshot of the REAL screen, not a narrow proof of one
//! mechanism. Imports the REAL `theme.slint` / `motifs.slint` by relative
//! path (not forked token values or re-derived components).
//!
//! Seeded with a tasteful, OSS-appropriate sample conversation (space
//! theme, no PII, no internal vernacular) — see
//! `ui_sim/src/bin/message_view_promo_render.rs` for the actual seed data.
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
    import { CometOnNotify, RocketOnSend, SpaceBackdrop } from "../../firmware/src/ui/motifs.slint";
    import { SignalMeter } from "../../firmware/src/ui/signal_meter.slint";
    import { BatteryIndicator } from "../../firmware/src/ui/battery_indicator.slint";

    // Verbatim copy of `message_view.rs`'s markup — see this file's module
    // doc for why a copy (not an import) is used here.

    struct MessageEntry {
        text:         string,
        from_name:    string,
        time_str:     string,
        is_ours:      bool,
        // Tri-state delivery status: 0 = Pending (grey), 1 = Acked (blue),
        // 2 = Undelivered (red) — mirrors `message_view.rs`'s
        // `MessageEntry.delivery_state` (see that file's doc); this rig was
        // previously a `bool acked` field, which structurally couldn't
        // represent the Undelivered state at all (meshcadet-dm-room-
        // delivery-state-model landed the tri-state on the real screen
        // without this rig being updated to match).
        delivery_state: int,
        mention_tier: int,
    }

    component MessageBubble {
        in property <string>  text;
        in property <string>  from_name;
        in property <string>  time_str;
        in property <bool>    is_ours;
        in property <int>     delivery_state;
        in property <int>     mention_tier;

        height: content.preferred-height;

        content := HorizontalLayout {
            alignment: is_ours ? end : start;
            padding-left:  is_ours ? 48px : 8px;
            padding-right: is_ours ? 8px  : 48px;

            VerticalLayout {
                spacing: 2px;

                HorizontalLayout {
                    alignment: center;
                    spacing: 6px;

                    Rectangle {
                        background: mention_tier == 2
                            ? #0d3a52
                            : (is_ours ? Theme.nebula-violet : Theme.surface-raised);
                        border-radius: 10px;
                        border-width: mention_tier == 0 ? 0px : (mention_tier == 2 ? 2px : 1px);
                        border-color: Theme.brand-signal;

                        HorizontalLayout {
                            padding-left: 20px;
                            padding-right: 20px;
                            padding-top: 6px;
                            padding-bottom: 6px;

                            if from_name == "" : Text {
                                text: text;
                                font-size: Theme.size-body;
                                color: Theme.text-primary;
                                wrap: word-wrap;
                            }
                            if from_name != "" : HorizontalLayout {
                                spacing: 4px;

                                Text {
                                    text: from_name + ":";
                                    font-size: Theme.size-body;
                                    font-weight: 700;
                                    color: Theme.text-primary;
                                }

                                Text {
                                    text: text;
                                    font-size: Theme.size-body;
                                    color: Theme.text-primary;
                                    wrap: word-wrap;
                                    horizontal-stretch: 1;
                                }
                            }
                        }
                    }

                    if is_ours : Text {
                        text: "✓";
                        font-size: Theme.size-caption;
                        color: delivery_state == 1 ? Theme.brand-signal : (
                            delivery_state == 2 ? Theme.alert : Theme.text-secondary
                        );
                        animate color { duration: 150ms; easing: ease-out; }
                    }
                }

                Text {
                    text: time_str;
                    font-size: Theme.size-caption;
                    color: Theme.text-secondary;
                    horizontal-alignment: right;
                }
            }
        }
    }

    // See `message_view.rs`'s identical component for the edge-alignment
    // mission's rationale (highlight hugs the glyph, touch target unchanged).
    component HeaderIconButton {
        in property <string> icon;
        in property <color>  icon_color: Theme.text-secondary;
        in property <length> icon_size: Theme.size-title;
        callback clicked;

        Rectangle {
            x: 0px;
            width: min(parent.width, icon_size + 12px);
            height: parent.height;
            background: touch.has-hover ? Theme.surface-raised : transparent;
            animate background { duration: 120ms; easing: ease-out; }
            Text {
                text: icon;
                font-size: icon_size;
                color: icon_color;
                horizontal-alignment: left;
                vertical-alignment: center;
            }
        }
        touch := TouchArea {
            width: parent.width;
            height: parent.height;
            clicked => { root.clicked(); }
        }
    }

    export component MessageViewPromoUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        in-out property <float> content_opacity: 0;
        animate content_opacity { duration: 200ms; easing: ease-out; }
        init => { content_opacity = 1; }

        in-out property <bool> notify_trigger: false;
        in-out property <bool> rocket_trigger: false;

        in property <string>          contact_name;
        in property <[MessageEntry]>  messages;
        // Repeater signal-meter reading (ADR-0010): 0 = direct-only,
        // 1..=5 = bars. See `SignalMeter`'s embedding below.
        in property <int>             signal_level: 0;
        // Coarse battery-level bucket (`meshcadet-battery-glanceable-
        // indicator`): 0 = Unknown, 1 = Charging, 2..=5 =
        // Critical/Low/Medium/High. See `BatteryIndicator`'s embedding below.
        in property <int>             battery_level: 0;

        callback back_pressed;
        callback compose_pressed;

        Timer {
            interval: 500ms;
            running: root.rocket_trigger;
            triggered => { root.rocket_trigger = false; }
        }

        public function scroll_to_bottom() {
            flick.viewport-y = min(0px, flick.height - flick.viewport-height);
        }

        // `SpaceBackdrop` no longer carries a default `source` (production
        // fan-out feeds it a Rust-shared `Image` instead — see
        // `firmware/src/ui/backdrop_asset.rs`'s module doc); this
        // single-consumer host-sim demo binds the literal directly, no
        // duplication concern applies here.
        SpaceBackdrop { source: @image-url("../../firmware/assets/space/starfield_full.png"); }

        VerticalLayout {
            opacity: content_opacity;

            Rectangle {
                height: 36px;
                background: Theme.surface;

                HorizontalLayout {
                    // See `message_view.rs`'s identical header for the
                    // edge-alignment mission's 6px/6px convention.
                    padding-left: 6px;
                    padding-right: 6px;
                    spacing: 4px;

                    HeaderIconButton {
                        // 64px (was 44px) — widened in lockstep with the
                        // balancing spacer below; see `message_view.rs`'s
                        // identical widen for the pixel math.
                        width: 64px;
                        icon: "‹";
                        icon_size: Theme.size-display;
                        icon_color: Theme.brand-signal;
                        clicked => { root.back_pressed(); }
                    }

                    Text {
                        text: contact_name;
                        font-size: Theme.size-body-lg;
                        font-weight: 600;
                        color: Theme.text-primary;
                        horizontal-stretch: 1.0;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }

                    // Right-pinned pair — see `message_view.rs`'s identical
                    // spacer for the edge-alignment mission's rationale.
                    Rectangle {
                        width: 64px; height: 36px;
                        BatteryIndicator {
                            battery-level: root.battery_level;
                            width: 14px;
                            height: 9px;
                            x: parent.width - self.width;
                            y: (parent.height - self.height) / 2;
                        }
                        SignalMeter {
                            signal-level: root.signal_level;
                            width: 16px;
                            height: 14px;
                            x: parent.width - self.width - 14px - 2px;
                            y: (parent.height - self.height) / 2;
                        }
                    }
                }

                CometOnNotify {
                    x: 0px;
                    y: parent.height - 14px;
                    play: root.notify_trigger;
                }
            }

            flick := Flickable {
                vertical-stretch: 1.0;
                VerticalLayout {
                    padding: 4px;
                    for m[i] in messages : MessageBubble {
                        text:         m.text;
                        from_name:    m.from_name;
                        time_str:     m.time_str;
                        is_ours:      m.is_ours;
                        delivery_state: m.delivery_state;
                        mention_tier: m.mention_tier;
                    }
                }
            }

            Rectangle {
                height: 40px;
                background: Theme.surface.with-alpha(0.55);

                Rectangle {
                    y: 0px;
                    height: 1px;
                    width: parent.width;
                    background: Theme.surface-raised;
                }

                HorizontalLayout {
                    alignment: center;
                    padding: 6px;

                    Rectangle {
                        width: 120px;
                        height: 28px;
                        background: write_touch.has-hover ? Theme.brand-signal-bright : Theme.brand-signal;
                        animate background { duration: 120ms; easing: ease-out; }
                        border-radius: 14px;
                        Text {
                            text: "✏ Write";
                            font-size: Theme.size-body;
                            font-weight: 600;
                            color: Theme.bg-space;
                            horizontal-alignment: center;
                            vertical-alignment: center;
                        }
                        write_touch := TouchArea {
                            width: parent.width;
                            height: parent.height;
                            clicked => {
                                root.compose_pressed();
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
        }
    }
}

/// One seeded message bubble for the promo screenshot.
///
/// `delivery_state` is the real screen's own tri-state contract: 0 =
/// Pending (grey), 1 = Acked (blue), 2 = Undelivered (red) — see
/// `MessageEntry`'s doc in the markup above / `message_view.rs`'s
/// `MessageEntry.delivery_state`. Exposed here as the real `i32`, not a
/// `bool acked` seed knob (an earlier version of this rig had only
/// `acked: bool`, which meant the Undelivered state was structurally
/// unreachable through this API even after the SLINT property itself was
/// widened to tri-state) — see
/// `ui_sim/tests/message_view_promo_undelivered_state.rs` for the
/// regression guard this closes.
pub struct PromoMessage {
    pub text: &'static str,
    pub from_name: &'static str,
    pub time_str: &'static str,
    pub is_ours: bool,
    pub delivery_state: i32,
}

struct MessageViewPromoPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for MessageViewPromoPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Render rig for the message-view promo screenshot.
///
/// # Panics
/// Panics if a Slint platform is already installed in this process — see
/// `compose_send.rs::ComposeSendFrame::new`'s identical note. Callers must
/// ensure exactly one [`MessageViewPromoFrame::new`] runs per process.
pub struct MessageViewPromoFrame {
    window: Rc<MinimalSoftwareWindow>,
    ui: MessageViewPromoUi,
}

impl MessageViewPromoFrame {
    pub fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        slint::platform::set_platform(Box::new(MessageViewPromoPlatform {
            window: window.clone(),
            start: Instant::now(),
        }))
        .expect("Slint platform already set in this process");

        let ui = MessageViewPromoUi::new().expect("MessageViewPromoUi::new");
        ui.show().expect("MessageViewPromoUi::show");

        MessageViewPromoFrame { window, ui }
    }

    /// Set the header's repeater signal-meter reading (ADR-0010): 0 =
    /// direct-only ring, 1..=5 = filled-bar count.
    pub fn set_signal_level(&self, bars: i32) {
        self.ui.set_signal_level(bars);
    }

    /// Set the header's coarse battery-level bucket
    /// (`meshcadet-battery-glanceable-indicator`): 0 = Unknown, 1 =
    /// Charging, 2..=5 = Critical/Low/Medium/High.
    pub fn set_battery_level(&self, level: i32) {
        self.ui.set_battery_level(level);
    }

    /// Seed the thread with `contact_name` + `messages`, and scroll to the
    /// bottom the same way `MessageViewScreen::set_messages` does.
    pub fn set_thread(&self, contact_name: &str, messages: &[PromoMessage]) {
        self.ui.set_contact_name(contact_name.into());
        let model: VecModel<MessageEntry> = VecModel::default();
        for m in messages {
            model.push(MessageEntry {
                text: m.text.into(),
                from_name: m.from_name.into(),
                time_str: m.time_str.into(),
                is_ours: m.is_ours,
                delivery_state: m.delivery_state,
                mention_tier: 0,
            });
        }
        self.ui.set_messages(ModelRc::new(model));
        self.ui.invoke_scroll_to_bottom();
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
            "message-view promo frame was not dirty — nothing painted"
        );
        framebuffer
    }
}

impl Default for MessageViewPromoFrame {
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
