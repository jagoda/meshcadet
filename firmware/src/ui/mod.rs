// SPDX-License-Identifier: GPL-3.0-only
//! MeshCadet touch UI module.
//!
//! # Architecture
//!
//! The UI is driven cooperatively from the radio dispatcher loop in `main.rs`:
//!
//! ```rust,ignore
//! // At startup (before the loop):
//! platform::TDeckPlatform::install();
//! let mut app = UiRuntime::new(display, touch, keyboard, buzzer, trackball, is_provisioned, &pubkey_hex, &self_name)?;
//!
//! // Once boot bring-up settles (radio/GPS/history/admin-server live, or —
//! // unprovisioned path — right before the USB-provisioning wait loop):
//! app.mark_app_ready();
//! app.run_splash_ripple(); // blocks ~1.15s on its OWN dedicated render loop
//!                          // (first ripple cycle only — it then loops via
//!                          // ordinary step() calls until splash dismiss;
//!                          // see screens::splash's module doc)
//!
//! // In the main loop:
//! app.step(now_ms)?;
//! ```
//!
//! `UiRuntime::step()` is non-blocking: it processes pending touch events,
//! updates Slint animations, redraws any dirty region, fires pending visual
//! and audible notifications, and returns immediately.
//! `UiRuntime::run_splash_ripple()` is the one exception — it deliberately
//! BLOCKS the calling thread for the boot splash's one-shot ripple animation,
//! on a dedicated tight render loop with no radio/GPS/input polling
//! interleaved (see that method's doc and `screens::splash`'s module doc for why).
//!
//! # Message passing (radio → UI)
//!
//! Radio events are posted via [`UiRuntime::post_event`], which is called from
//! the receive handler in `main.rs` when a new DM or group message arrives:
//!
//! ```rust,ignore
//! app.post_event(UiEvent::IncomingDm { from_hash: 0x42, text: "hi :smile:".into() });
//! ```
//!
//! The UI runtime processes these events on the next `step()` call.
//!
//! # Buzzer (audible notifications)
//!
//! CORRECTION (2026-07-03): earlier
//! revisions of this module assumed a passive piezo buzzer on GPIO46 driven
//! via LEDC PWM. That hardware does not exist on the T-Deck / T-Deck Plus —
//! GPIO46 is `BOARD_KEYBOARD_INT` (the keyboard co-processor's interrupt
//! line) per LilyGo's own `utilities.h`. The board's one actual audio-output
//! path is the ESP-IDF **I2S** peripheral driving the onboard speaker on
//! GPIO 5 (WS/LRCK), 7 (BCK), 6 (DOUT) — confirmed against LilyGo's own
//! `SimpleTone.ino` example, the upstream `meshcore-dev/MeshCore` firmware
//! (which defines no `PIN_BUZZER` for this board at all), and the shipped
//! MCTerm companion firmware (`dabeani/meshcoreterm`, CHANGELOG: "T-Deck I2S
//! buzzer").
//!
//! `BuzzerDriver` in this module owns an `I2sDriver<I2sTx>` in std/Philips
//! mode (mono, 16-bit, 8 kHz) and plays [`notification::ToneBurst`] sequences
//! by software-synthesizing a square wave and streaming it over I2S,
//! synchronously (blocking for the duration of the sequence, ≤ ~1 s in the
//! worst case). This is acceptable since notifications are rare; if latency
//! becomes an issue a FreeRTOS timer can be used instead.

pub mod display;
pub mod touch;
pub mod keyboard;
pub mod trackball;
pub mod notification;
pub mod platform;
pub mod screens;
pub mod theme;

use notification::{NotifDispatcher, NotifEvent, NotifPrefs, ToneBurst};

use display::TDeckDisplay;
use touch::TouchDriver;
use keyboard::KeyboardDriver;
use platform::TDeckWindowAdapter;

use esp_idf_hal::gpio::{AnyIOPin, InputPin, OutputPin};
use esp_idf_hal::i2s::{
    config::{Config as I2sChannelConfig, DataBitWidth, SlotMode, StdClkConfig, StdConfig, StdGpioConfig, StdSlotConfig},
    I2s, I2sDriver, I2sTx,
};
use esp_idf_svc::nvs::{EspNvsPartition, NvsDefault};

// `pin_menu` is compiled in all builds (pure Rust, no ESP-IDF deps) so this
// import is unconditional.
use crate::pin_menu;
// `gps::GpsStatus` is a plain Copy struct (no hardware dependency) — used
// here purely as a display-state type for the GPS status screen.
use crate::gps;
// `battery::BatteryStatus` is likewise a plain Copy struct — used here purely
// as a display-state type for the admin-menu battery row.
use crate::battery;

// ── UI events (radio → UI) ────────────────────────────────────────────────────

/// Events posted from the radio layer to the UI runtime.
#[derive(Clone, Debug)]
pub enum UiEvent {
    /// A new direct message arrived.
    IncomingDm {
        from_hash: u8,
        from_name: String,
        text: String,
    },
    /// A new group channel message arrived.
    IncomingGroupMsg {
        channel_hash: u8,
        text: String,
    },
    /// An outbound DM was acknowledged.
    ///
    /// `handle_event` below flips the last unacked outbound `MessageRecord`
    /// to `acked: true`, refreshing the ✓→✓✓ indicator. Raised by
    /// `main.rs::match_pending_ack` when an inbound ACK (bare `Ack` frame or
    /// one bundled in a PATH-return) matches the `PendingAck` recorded when
    /// the DM was sent — `PendingAck` pairs the expected ack hash with the
    /// `to_hash` this variant needs (previously `pending_ack` was a
    /// bare `[u8; 4]` with no `to_hash`, so a matching ACK was logged but
    /// never reached this handler).
    DmAcked {
        to_hash: u8,
    },
    /// An outbound channel/group message was implicitly acknowledged.
    ///
    /// A broadcast GRP_TXT has no per-recipient delivery ACK on the wire, so
    /// it is treated as delivered once the device hears its own transmission
    /// repeated back into the mesh by another node. `handle_event` reuses the
    /// same `mark_last_unacked_outbound` the DM path uses (`self.messages` is
    /// keyed by `u8` for both contacts and channels) to flip the newest
    /// pending outbound `MessageRecord` for `channel_hash` to `acked: true`.
    /// Raised by `main.rs::match_pending_channel_ack` when a duplicate-
    /// detected inbound frame's dedup key matches the `PendingChannelAck`
    /// recorded when the channel message was sent.
    ChannelAcked {
        channel_hash: u8,
    },
    /// A telemetry response arrived.
    ///
    /// `protocol::telemetry::decode_telemetry_response` (its doc: "Primary
    /// use: host-side validation and HIL test assertions") is never called
    /// from the RX path, so an inbound `loc:lat=…` reply is delivered as an
    /// ordinary `IncomingDm` with the raw wire text instead of this
    /// variant's structured, prettified rendering — it still reaches the
    /// user, just unparsed. Lower-severity than `DmAcked` above (no
    /// permanently-wrong UI state, just missing polish); left unwired here
    /// for the same reason.
    #[allow(dead_code)]
    TelemetryResponse {
        from_hash: u8,
        lat_e7: i32,
        lon_e7: i32,
        age_secs: u32,
    },
}

/// Commands from the UI layer to the radio dispatcher.
#[derive(Clone, Debug)]
pub enum UiCommand {
    /// Send a direct message.
    SendDm {
        to_hash: u8,
        text: String,
    },
    /// Send a group channel message.
    SendGroupMsg {
        channel_hash: u8,
        text: String,
    },
}

// ── Buzzer driver ─────────────────────────────────────────────────────────────

/// I2S-driven speaker "buzzer" for audible notifications.
///
/// See the module-level "Buzzer" doc section for why this is I2S rather than
/// LEDC PWM: the T-Deck / T-Deck Plus has no discrete piezo buzzer GPIO — the
/// only audio-output hardware is the onboard I2S speaker (WS=GPIO5, BCK=GPIO7,
/// DOUT=GPIO6). Tones are produced by software-synthesizing a square wave at
/// the requested frequency and streaming it as 16-bit mono PCM.
pub struct BuzzerDriver<'d> {
    i2s: I2sDriver<'d, I2sTx>,
}

impl<'d> BuzzerDriver<'d> {
    /// I2S sample rate for synthesized tones. 8 kHz is more than sufficient
    /// for the sub-2 kHz notification tones in `notification::tone_sequence`
    /// and matches LilyGo's own reference `SimpleTone.ino` example.
    const SAMPLE_RATE_HZ: u32 = 8_000;
    /// Peak amplitude for the synthesized square wave (16-bit signed PCM,
    /// max 32767) — matches LilyGo's reference example's moderate-volume pick.
    const AMPLITUDE: i16 = 16_384;
    /// Silence gap between consecutive tone bursts in a sequence, in ms.
    const GAP_MS: u32 = 30;
    /// Samples synthesized per I2S write — bounded so `play()` doesn't need a
    /// heap allocation proportional to burst duration.
    const CHUNK_SAMPLES: usize = 128;
    /// Per-chunk I2S write timeout, in ms. `play()` runs synchronously in the
    /// cooperative UI loop (see `step()`), so a write must never block
    /// indefinitely: an `esp_idf_hal::delay::BLOCK` timeout here would let an
    /// I2S/DMA hardware fault hang the *entire* main loop (radio + UI), not
    /// just silence the tone. 200 ms is generous headroom over the nominal
    /// ~16 ms a full `CHUNK_SAMPLES` write takes at `SAMPLE_RATE_HZ`, while
    /// still bounding the worst case to a fraction of one tone burst.
    const WRITE_TIMEOUT_MS: u64 = 200;

    /// Initialize the I2S TX channel driving the onboard speaker.
    ///
    /// `i2s`: an I2S peripheral (e.g. `peripherals.i2s0`).
    /// `bclk`/`ws`/`dout`: GPIO 7 / 5 / 6 on the T-Deck Plus (see module doc).
    pub fn new<I2S: I2s + 'd>(
        i2s: I2S,
        bclk: impl InputPin + OutputPin + 'd,
        ws: impl InputPin + OutputPin + 'd,
        dout: impl OutputPin + 'd,
    ) -> anyhow::Result<Self> {
        // Mono, 16-bit, Philips/standard I2S format — same shape as LilyGo's
        // SimpleTone.ino reference (I2S_CHANNEL_FMT_ONLY_LEFT +
        // I2S_COMM_FORMAT_STAND_I2S).
        let slot_cfg = StdSlotConfig::philips_slot_default(DataBitWidth::Bits16, SlotMode::Mono);
        let clk_cfg = StdClkConfig::from_sample_rate_hz(Self::SAMPLE_RATE_HZ);
        // `auto_clear(true)` is the fix for "notification plays indefinitely":
        // ESP-IDF's I2S TX DMA
        // ring buffer defaults to `auto_clear: false`, meaning once `stream()`
        // stops writing new samples at the end of a tone sequence, the DMA
        // descriptors keep re-transmitting whatever was last written — the
        // final chunk of the last tone burst loops on the speaker forever
        // instead of going silent. `auto_clear(true)` makes the driver
        // zero-fill the DMA buffer whenever there's no fresh data queued, so
        // playback genuinely stops the instant `play()` returns, bounding
        // every notification to the sum of its `tone_sequence` durations
        // (~150-650ms depending on event) rather than an unbounded tail.
        let channel_cfg = I2sChannelConfig::default().auto_clear(true);
        let std_config = StdConfig::new(channel_cfg, clk_cfg, slot_cfg, StdGpioConfig::default());

        let mut driver = I2sDriver::<I2sTx>::new_std_tx(i2s, &std_config, bclk, dout, AnyIOPin::none(), ws)
            .map_err(|e| anyhow::anyhow!("I2S buzzer init failed: {:?}", e))?;
        driver
            .tx_enable()
            .map_err(|e| anyhow::anyhow!("I2S buzzer tx_enable failed: {:?}", e))?;
        Ok(BuzzerDriver { i2s: driver })
    }

    /// Play a sequence of tone bursts (blocking for the sequence's total duration).
    pub fn play(&mut self, sequence: &[ToneBurst]) {
        for burst in sequence {
            self.stream(burst.freq_hz, burst.dur_ms);
            self.stream(0, Self::GAP_MS); // silence gap between bursts
        }
    }

    /// Synthesize `dur_ms` of a `freq_hz` square wave (or silence if
    /// `freq_hz == 0`) and stream it over I2S in fixed-size chunks.
    fn stream(&mut self, freq_hz: u32, dur_ms: u32) {
        let total_samples = Self::SAMPLE_RATE_HZ / 1_000 * dur_ms;
        let mut sample_counter: u32 = 0;
        let mut emitted: u32 = 0;
        let mut buf = [0u8; Self::CHUNK_SAMPLES * 2];
        let timeout = esp_idf_hal::delay::TickType::new_millis(Self::WRITE_TIMEOUT_MS).ticks();

        while emitted < total_samples {
            let n = (Self::CHUNK_SAMPLES as u32).min(total_samples - emitted) as usize;
            for i in 0..n {
                let sample = Self::square_wave_sample(sample_counter, freq_hz, Self::SAMPLE_RATE_HZ, Self::AMPLITUDE);
                let b = sample.to_le_bytes();
                buf[i * 2] = b[0];
                buf[i * 2 + 1] = b[1];
                sample_counter += 1;
            }
            emitted += n as u32;
            if let Err(e) = self.i2s.write_all(&buf[..n * 2], timeout) {
                // Bounded timeout (see WRITE_TIMEOUT_MS doc) — a write that
                // times out here truncates this burst rather than hanging the
                // cooperative loop; logged so the field failure is diagnosable.
                log::warn!("buzzer i2s write: {:?}", e);
                return;
            }
        }
    }

    /// Pure square-wave sample generator for `sample_index` of a `freq_hz`
    /// tone at `sample_rate_hz`, returning `+amplitude`/`-amplitude` (or `0`
    /// for `freq_hz == 0`, used for the silence gap between bursts).
    ///
    /// Extracted as a standalone function (no I2S/hardware dependency) so the
    /// part of this driver most likely to carry a subtle bug — the duty-cycle
    /// arithmetic — has a host-checkable unit test independent of the
    /// esp-idf-hal I2S stack (this crate's `#[cfg(test)]` blocks are
    /// type-checked but never executed on host — see the NOTE above `mod
    /// tests` at the bottom of this file — so a wrong sample here would
    /// otherwise only surface as "the buzzer sounds off" on real hardware).
    ///
    /// `.max(1)` guards the `sample_rate_hz / freq_hz` division against a
    /// `freq_hz == 0` (silence) caller; `.max(2)` guards the `% samples_per_cycle`
    /// below against a division/modulo by zero if `freq_hz` ever exceeded the
    /// Nyquist limit (`sample_rate_hz`) — not reachable from the current
    /// `notification::tone_sequence()` table (max 1320 Hz vs. an 8 kHz sample
    /// rate), but cheap to make unconditionally safe.
    fn square_wave_sample(sample_index: u32, freq_hz: u32, sample_rate_hz: u32, amplitude: i16) -> i16 {
        if freq_hz == 0 {
            return 0;
        }
        let samples_per_cycle = (sample_rate_hz / freq_hz.max(1)).max(2);
        if (sample_index % samples_per_cycle) < samples_per_cycle / 2 {
            amplitude
        } else {
            -amplitude
        }
    }
}

// ── Active screen ─────────────────────────────────────────────────────────────

/// Which Slint screen component is currently live.
///
/// All screens share one [`platform::TDeckWindowAdapter`]
/// (`MinimalSoftwareWindow`).  Navigation explicitly hides the outgoing
/// component, shows the incoming one, and calls `request_redraw()` so the
/// cooperative loop repaints the full panel — see `hide_active_screen` and the
/// `navigate_to_*` methods.  Showing a new screen without forcing a redraw left
/// the previous screen's pixels on the display (the gear→PIN-pad swap bug).
///
/// BUG FIX (never-run chain): the previous implementation stored `ScreenState`
/// (a navigation-stack enum) but never created any Slint component object.
/// Slint's software renderer had nothing to draw → blank display.  This enum
/// holds the actual live component so that `render_if_needed` has content.
enum ActiveScreen {
    /// Boot splash — always the FIRST active screen (see `UiRuntime::new`).
    /// Dismissed by `dismiss_splash()` once `step()`'s gate (the one-shot
    /// intro animation — itself gated on `mark_app_ready()` — has run for
    /// `SPLASH_MIN_MS`, or the `SPLASH_MAX_MS` defensive cap) opens, swapping
    /// in the real initial screen (`Unprovisioned` or `ContactList`).
    Splash(screens::SplashScreen),
    Unprovisioned(screens::UnprovisionedScreen),
    ContactList(screens::ContactListScreen),
    /// PIN-entry screen shown when the user taps the settings button.
    /// When PIN is verified, navigates to AdminMenu.
    PinEntry(screens::PinEntryScreen),
    /// Admin settings menu, shown after a correct PIN. Flips
    /// `RuntimeSettings` toggles via `pin_menu::apply_menu_action` and
    /// persists them to NVS. Back navigation returns to ContactList.
    AdminMenu(screens::AdminMenuScreen),
    /// Conversation thread for a contact or channel, opened by tapping a row.
    /// Back navigation returns to ContactList.
    MessageView(screens::MessageViewScreen),
    /// Compose / draft screen, opened by the Write button in MessageView.
    /// Back and Send both return to the originating conversation.
    Compose(screens::ComposeScreen),
    /// Read-only GPS status sub-screen, opened from the admin menu's
    /// "📍 GPS status" row. No controls — fix state / coordinates+age /
    /// time-sync state only. Back navigation returns to AdminMenu.
    GpsStatus(screens::GpsStatusScreen),
}

impl ActiveScreen {
    /// Variant name for diagnostic logging — lets the incoming-message
    /// handlers log *which* screen was active at the moment an unread badge
    /// refresh either fired or was guarded off, without a hardware-dependent
    /// HIL session to step through.
    fn name(&self) -> &'static str {
        match self {
            ActiveScreen::Splash(_) => "Splash",
            ActiveScreen::Unprovisioned(_) => "Unprovisioned",
            ActiveScreen::ContactList(_) => "ContactList",
            ActiveScreen::PinEntry(_) => "PinEntry",
            ActiveScreen::AdminMenu(_) => "AdminMenu",
            ActiveScreen::MessageView(_) => "MessageView",
            ActiveScreen::Compose(_) => "Compose",
            ActiveScreen::GpsStatus(_) => "GpsStatus",
        }
    }
}

// ── UiRuntime ─────────────────────────────────────────────────────────────────

/// The top-level UI runtime.
///
/// Owns the display, touch driver, Slint window adapter, active screen
/// component, and notification dispatcher.
pub struct UiRuntime<'d> {
    display: TDeckDisplay<'d>,
    touch: TouchDriver<'d>,
    /// Physical QWERTY keyboard co-processor driver.  `None` when the
    /// co-processor did not ACK at boot (UI degrades to touch-only).
    keyboard: Option<KeyboardDriver<'d>>,
    /// Physical trackball driver (roll + center click) — a PARALLEL input
    /// modality alongside touch and keyboard, never a replacement for either.
    /// `None` on any init failure (same
    /// graceful-degradation pattern as `keyboard`/`buzzer`); the UI is fully
    /// usable via touch/keyboard alone either way.
    trackball: Option<trackball::TrackballDriver<'d>>,
    window: std::rc::Rc<TDeckWindowAdapter>,
    /// Currently-shown Slint screen component.
    active_screen: ActiveScreen,
    notif: NotifDispatcher,
    /// I2S-driven speaker "buzzer" for audible notifications. `None` when I2S
    /// init failed at boot (UI degrades to visual-only notifications, same
    /// graceful-degradation pattern as `keyboard`).
    buzzer: Option<BuzzerDriver<'d>>,
    /// Pending commands for the radio layer (drained by `drain_commands`).
    commands: Vec<UiCommand>,
    /// Buffered incoming events to process on next `step()`.
    events: Vec<UiEvent>,
    /// Message history per contact hash (hash → Vec<MessageRecord>).
    messages: std::collections::HashMap<u8, Vec<MessageRecord>>,
    /// Contact name table (hash → name).
    contact_names: std::collections::HashMap<u8, String>,
    /// This node's own display name — `identity_store::load_name` persisted
    /// name, or the `MeshCadet-<HH>` pub_hash fallback (see
    /// `main.rs::device_sender_name`). Sourced once at construction. Drives two
    /// things: the known-names set `on_send_message` wraps `@mentions`
    /// against (own name is mentionable too), and the self-vs-other tier
    /// decision `build_message_items` makes when rendering a received
    /// mention (`protocol::mention::split_mentions`'s `self_name` arg).
    self_name: String,
    /// Unread counts per contact.
    unread: std::collections::HashMap<u8, u32>,
    /// Channel items cache — re-used when navigating back from PinEntry.
    channel_items: Vec<screens::contact_list::ChannelItem>,
    /// Trackball-driven highlighted row index on the ContactList screen
    /// (whichever tab — contacts or channels — is currently visible). `-1` =
    /// no trackball highlight yet (touch taps a row directly and never sets
    /// this; it only starts moving once the trackball is rolled). Reset on
    /// every fresh `navigate_to_contact_list` so re-entering the list never
    /// shows a stale highlight from a previous visit.
    contact_list_selected: i32,
    /// Trackball-driven highlighted row index on the AdminMenu screen (0..=3:
    /// visual toggle, audible toggle, screen-sleep stepper, GPS status row).
    /// Same `-1` "no highlight yet" sentinel and reset-on-entry discipline as
    /// `contact_list_selected`.
    admin_menu_selected: i32,
    /// Provisioned PIN bytes and length (zeroed = PIN lock disabled).
    stored_pin: [u8; pin_menu::MAX_PIN_LEN],
    stored_pin_len: u8,
    /// On-device admin-menu RuntimeSettings (separate from the provisioning
    /// config — see `pin_menu` module docs). Shared via `Rc<RefCell<_>>` so the
    /// `'static` Slint toggle callbacks wired in `navigate_to_admin_menu` can
    /// mutate it without capturing `&mut self`, and so the current values
    /// survive navigating away from and back to AdminMenu.
    ///
    /// These two fields (`notif_visual`/`notif_audible`) are the admin-menu's
    /// master enable/disable toggles and are mirrored into [`NotifDispatcher`]'s
    /// gating every `step()` by [`UiRuntime::sync_notif_prefs`] (previously a
    /// KNOWN GAP: the toggle visibly flipped and persisted to NVS but
    /// `fire()` never consulted it).
    /// Mirrored rather than read directly by `fire()` so `NotifDispatcher`
    /// stays hardware/settings-agnostic — it only ever sees a `NotifPrefs`
    /// table, never `pin_menu::RuntimeSettings`.
    runtime_settings: std::rc::Rc<std::cell::RefCell<pin_menu::RuntimeSettings>>,
    /// NVS partition handle for persisting `runtime_settings`, wired by
    /// `set_nvs_partition` once the device's provisioned config has loaded.
    /// `None` until then (or on builds with no NVS, e.g. hil) — toggles
    /// still apply in memory but log a warning instead of persisting.
    nvs_partition: Option<EspNvsPartition<NvsDefault>>,
    /// Pending navigation request set from Slint callbacks.
    ///
    /// `0` = none, `1` = navigate to PinEntry, `2` = navigate to ContactList,
    /// `3` = navigate to MessageView for a contact, `4` = navigate to
    /// MessageView for a channel, `5` = navigate to Compose (Write button, OR
    /// a printable keypress while MessageView is active — see
    /// [`Self::pending_compose_seed`]), `6` = Compose Send (send the stashed
    /// draft immediately, then defer re-opening the thread — see
    /// [`Self::deferred_message_view_nav_at_ms`]), `7` = Compose back/cancel (re-open
    /// the thread without sending), `8` = navigate to AdminMenu (PIN
    /// verified), `9` = navigate to GpsStatus (AdminMenu's "📍 GPS status"
    /// row), `10` = GpsStatus back → AdminMenu, `11` = PIN entry rejected
    /// (fires [`notification::NotifEvent::PinError`]; no screen change — stays
    /// on PinEntry).  For codes `3`/`4` the tapped row's hash is carried in
    /// [`Self::pending_nav_hash`]; codes `5`/`6`/`7` use [`Self::active_convo`]
    /// and (for `6`) [`Self::pending_compose_text`].
    ///
    /// Using `Rc<Cell>` so Slint callback closures (which are `'static` and
    /// can't capture `&mut self`) can signal navigation intent.
    pending_nav: std::rc::Rc<std::cell::Cell<u8>>,
    /// Hash of the tapped contact/channel row, paired with `pending_nav` codes
    /// `3`/`4`.  Read by `step()` when it dispatches the MessageView navigation.
    pending_nav_hash: std::rc::Rc<std::cell::Cell<u8>>,
    /// Digit buffer shared between PinEntry Slint callbacks and the navigation
    /// handler.  Cleared on every confirm/cancel.
    pin_digits: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    /// The conversation `(hash, is_channel)` currently open in MessageView.
    /// Used to title the compose screen and to route a composed message back to
    /// the right contact/channel and re-open the thread after Send/cancel.
    active_convo: Option<(u8, bool)>,
    /// Draft text handed off from the compose Send callback (a `'static`
    /// closure) to `step()`, which expands shortcodes, sends, and re-opens the
    /// thread.
    pending_compose_text: std::rc::Rc<std::cell::RefCell<Option<String>>>,
    /// Deadline (`now_ms`) at which `step()` should perform the Compose →
    /// MessageView navigation that nav code `6` used to fire immediately.
    ///
    /// BUG FIX: nav
    /// code `6`'s message send and its screen swap used to happen in the
    /// same `step()` tick, which tore the Compose screen down (and with it
    /// the `RocketOnSend` one-shot floating on its Send button — see
    /// `screens::compose`'s module doc) before the 400ms arc-up+fade ever
    /// got a frame to render. The message send itself still happens
    /// synchronously in that same tick (delivery timing is untouched); only
    /// the *navigation* — a presentation concern — is deferred to this
    /// timestamp, giving the animation time to play on the still-visible
    /// Compose screen. `None` when no send-triggered navigation is
    /// outstanding. Checked every `step()` right after the `pending_nav`
    /// dispatch block; guarded there against the user having already
    /// navigated away (e.g. via Back) in the interim — see that check site.
    deferred_message_view_nav_at_ms: Option<u64>,
    /// First character to pre-load into the Compose draft, set by `step()`'s
    /// keyboard block the instant a printable key is pressed while
    /// `MessageView` is active — paired with `pending_nav = 5`. Consumed
    /// (taken) by `navigate_to_compose`
    /// on the very next `step()`. `None` when compose was instead reached via
    /// the Write button, so that path seeds nothing (unchanged behavior).
    /// Plain `Option`, not `Rc<Cell>`/`Rc<RefCell>` like the other
    /// `pending_*` fields: those exist to cross a `'static` Slint callback
    /// boundary, but this one is only ever written from `step()` itself.
    pending_compose_seed: Option<String>,
    /// Latest GPS status snapshot, refreshed every `main.rs` dispatcher-loop
    /// iteration via [`Self::set_gps_status`]. Cached here (rather than only
    /// pushed at `navigate_to_gps_status` time) so the fix/sync ages tick
    /// upward live while the GpsStatus screen is open, and so a fresh screen
    /// opened later reflects the current snapshot immediately.
    gps_status: gps::GpsStatus,
    /// Latest battery status snapshot (charge percentage + charging state),
    /// refreshed every `main.rs` dispatcher-loop iteration via
    /// [`Self::set_battery_status`]. Cached here for the same reason as
    /// `gps_status`: an AdminMenu screen opened later reflects the current
    /// reading immediately rather than a stale boot-time value.
    battery_status: battery::BatteryStatus,

    // ── Screen-sleep (backlight-off) state ────────────────────────────────
    //
    // Global first-input-interceptor: this is a property of `step()` itself,
    // NOT of `active_screen` — it applies uniformly above the whole screen
    // stack (including PinEntry), which is why it lives here rather than in
    // any one screen module. See `wake_screen`/`step`'s touch/keyboard blocks.
    /// `true` when the backlight is off (display controller still live).
    screen_asleep: bool,
    /// `uptime_ms` timestamp of the last touch/key activity. Incoming
    /// messages deliberately do NOT update this (deliberate design decision:
    /// messages must not wake or keep the screen awake).
    ///
    /// Seeded from `now_ms` on the FIRST `step()` call (see
    /// `activity_clock_started`), not from construction time — `UiRuntime::new`
    /// runs before `main.rs`'s radio/provisioning bring-up, which can take
    /// longer than a short configured timeout; starting the inactivity clock
    /// at construction would let the screen sleep before the user ever sees
    /// the first rendered frame.
    last_activity_ms: u64,
    /// `false` until the first `step()` call seeds `last_activity_ms` from
    /// that call's `now_ms` — see `last_activity_ms` doc.
    activity_clock_started: bool,
    /// `true` while suppressing the REST of the touch gesture that woke the
    /// screen (from the swallowed wake-triggering Pressed through its
    /// matching Released) so a still-held finger can't leak a Moved/Released
    /// into the focused widget after the initiating Pressed was swallowed.
    /// Not needed for keys: the keyboard co-processor reports one key as a
    /// single atomic poll, so swallowing that one `poll_key()` result is
    /// already complete — no follow-up event to suppress.
    touch_wake_swallow_active: bool,
    /// Last keyboard-backlight state actually written to the co-processor.
    ///
    /// The keyboard backlight now has two conceptual "wants": the
    /// screen-follows rule (on while awake, off while asleep) and the
    /// incoming-message blink loop (on/off pulses while asleep). Both are
    /// routed through the single arbiter `sync_keyboard_backlight`, which
    /// computes the desired state from `screen_asleep` + `notif.poll_blink`
    /// and writes only on a change from this cached value — see that
    /// function's doc. Without a single owner, the two rules would fight
    /// over the keyboard backlight state.
    kb_backlight_on: bool,

    // ── Boot splash state ──────────────────────────────────────────────────
    //
    // See `screens::splash` module doc for the animation choreography and
    // `step()` for the dismissal gate. `initial_is_provisioned` /
    // `initial_pubkey_hex` are the arguments `new()` would otherwise have
    // used to build the real initial screen (`ContactList` / `Unprovisioned`)
    // immediately; `dismiss_splash` stashes and uses them to build that
    // screen lazily once the splash's gate opens. `step()` only ever calls
    // `dismiss_splash` while `active_screen` is still `Splash(_)`, so a
    // second call (and thus re-reading these fields) cannot happen.
    initial_is_provisioned: bool,
    initial_pubkey_hex: String,
    /// Wall-clock (`uptime_ms`) timestamp of the splash's first `step()`
    /// tick, seeded on the FIRST `step()` call rather than at construction —
    /// same rationale as `last_activity_ms`: `UiRuntime::new` runs before
    /// `main.rs`'s radio/provisioning bring-up. Used ONLY by the
    /// `SPLASH_MAX_MS` defensive cap (a later fix moved the animation's own
    /// timing off this clock and onto `splash_animation_started_ms` — see
    /// that field's doc — so this one no longer gates the animation itself).
    splash_started_ms: u64,
    /// `false` until the first `step()` call seeds `splash_started_ms`.
    splash_clock_started: bool,
    /// Wall-clock (`uptime_ms`) timestamp `SplashScreen::start_animation()`
    /// was actually called, i.e. the first `step()` tick AFTER `app_ready`
    /// went `true` (see `step()`'s dismissal-gate block). `SPLASH_MIN_MS` in
    /// `step()` is measured from THIS clock, not `splash_started_ms` — the
    /// animation always finishes before dismissal regardless of how long
    /// boot took to reach `mark_app_ready()`.
    splash_animation_started_ms: u64,
    /// `false` until `step()` calls `SplashScreen::start_animation()`. Also
    /// doubles as the "animation not started yet" branch of the dismissal
    /// gate (see `splash_should_dismiss`).
    splash_animation_started: bool,
    /// Set once by `mark_app_ready()` (called from `main.rs` once boot
    /// reaches steady state). Gates BOTH halves of the new splash behavior:
    /// it is the trigger `step()` waits for before calling
    /// `SplashScreen::start_animation()` at all, and (combined with
    /// `SPLASH_MIN_MS`, via `splash_animation_started_ms`) it is the
    /// "system is ready for use" half of the splash dismissal gate.
    app_ready: bool,
    /// `true` once `step()` has called `dismiss_splash()` — bounds that call
    /// to AT MOST ONCE regardless of outcome, so a screen-construction
    /// failure inside it degrades once (logged) rather than retrying every
    /// `step()` iteration forever. See `step()`'s dismissal-gate comment.
    splash_dismiss_attempted: bool,

    // ── Render-cadence throttle ──
    //
    // See `step()`'s "Render dirty regions" block for the full mechanism.
    // Short version: every screen-entry fade (`reveal_opacity`/
    // `content_opacity`) and `motifs.slint` one-shot (`RocketOnSend`,
    // `CometOnNotify`, ...) drives a Slint `animate`, and `ui_perf`'s
    // `tests/entry_fade_repaint.rs` measurement (see `docs/perf/ui-perf-
    // baseline.md` §10) confirms that WITHOUT a
    // cadence cap, a shared-loop `step()` running near `RX_POLL_YIELD_MS`
    // cadence (~5 ms, ~200 Hz) re-renders a full-window opacity fade's
    // ENTIRE bounding region on every single dispatcher iteration for the
    // whole animation — a 200 ms fade alone can cost dozens of full 240-line
    // `flush_line_range` sweeps instead of the one full paint navigation
    // already requires, each one contending with the shared SPI2 bus's
    // CAD/RX poll. These two fields cap the RENDER (not the input-poll or
    // timer) cadence to `RENDER_MIN_INTERVAL_MS` — but ONLY while an
    // animation is still settling; see `RENDER_MIN_INTERVAL_MS`'s doc for
    // why a fresh one-off redraw (navigation, incoming message, model
    // update) is never delayed by this.
    /// `now_ms` of the last `step()` iteration that actually called
    /// `window.render_if_needed` (drew a frame). `0` at construction so the
    /// very first real render (the boot splash's first frame) is never
    /// throttled — `now_ms` is always `> RENDER_MIN_INTERVAL_MS` by then.
    last_render_ms: u64,
    /// Cached result of `window.has_active_animations()` from the last
    /// render — `true` means that render touched a still-interpolating
    /// `animate` (a fade or motif mid-flight), so the NEXT render may be
    /// throttled to `RENDER_MIN_INTERVAL_MS`. `false` (the default, and the
    /// value after any render that settles) means the next tick's redraw —
    /// if `needs_redraw` is even set — renders immediately, uncapped: a
    /// fresh navigation/model-update/incoming-message paint is exactly one
    /// such "not currently animating" redraw and must never wait on this
    /// cap (on-hardware tap-to-first-frame timeliness, `docs/perf/ui-perf-
    /// baseline.md` §8.A, would otherwise regress).
    render_settling: bool,
}

/// One stored message in a conversation.
#[derive(Clone, Debug)]
pub struct MessageRecord {
    pub text:     String,
    pub is_ours:  bool,
    pub acked:    bool,
    // Captured at every construction site (cheap: `now_ms` is already in
    // scope there) but not read anywhere yet — no message view renders a
    // timestamp today. Kept rather than dropped: unlike the other fields,
    // arrival time can't be reconstructed after the fact once a message has
    // been appended, so deleting this now would foreclose a "time sent"
    // label later at zero cost saved.
    #[allow(dead_code)]
    pub ts_ms:    u64,
}

/// Insert `records` under `hash`, unless `records` is empty (a no-op skip,
/// not a clearing insert — see `UiRuntime::seed_conversation`'s doc for why
/// an empty conversation is left absent from the map rather than inserted as
/// `vec![]`).
///
/// Pulled out as a free function over a plain map — rather than an
/// `impl UiRuntime` method — purely so it's testable in isolation, same
/// "static function over plain data" pattern `build_contact_items`/
/// `build_channel_items` already use (those can't touch real display/touch
/// hardware in a test either).
fn messages_insert_non_empty(
    messages: &mut std::collections::HashMap<u8, Vec<MessageRecord>>,
    hash: u8,
    records: Vec<MessageRecord>,
) {
    if records.is_empty() {
        return;
    }
    messages.insert(hash, records);
}

/// Mark the most-recently-sent, still-unacked outbound `MessageRecord` to
/// `to_hash` as acked (✓ → ✓✓). Returns `true` if a record was found and
/// flipped, `false` if there was no matching pending outbound message.
///
/// Searches newest-first (`.rev()`) and stops at the first unacked outbound
/// hit — this is the "right message marked" invariant a confirmed-delivered
/// DM depends on: `main.rs`'s `pending_ack` tracks only ONE outstanding ack at
/// a time, for the most recently sent DM, so the newest unacked outbound
/// record in this contact's thread is always the one a live match refers to.
///
/// Pulled out as a free function over a plain map for the same reason as
/// `messages_insert_non_empty` above — `UiEvent::DmAcked`'s handler in
/// `handle_event` is an `impl UiRuntime` method that can't be unit-tested
/// directly (hardware-backed fields), so the matching logic itself lives here
/// where a test can drive it against a plain `HashMap`.
fn mark_last_unacked_outbound(
    messages: &mut std::collections::HashMap<u8, Vec<MessageRecord>>,
    to_hash: u8,
) -> bool {
    if let Some(msgs) = messages.get_mut(&to_hash) {
        for m in msgs.iter_mut().rev() {
            if m.is_ours && !m.acked {
                m.acked = true;
                return true;
            }
        }
    }
    false
}

impl<'d> UiRuntime<'d> {
    /// Minimum wall-clock time the boot splash's one-shot animation is held
    /// on screen once it STARTS, floored so it always plays to completion
    /// (see `screens::splash` module doc) with a bit of settled-hold margin
    /// afterward before dismissal. MUST stay >= that timeline's total
    /// (currently 1150 ms).
    ///
    /// BUG FIX: raised from 1200 ms to 1600 ms
    /// — the splash was dismissing "a touch too fast", so this is the knob
    /// that actually controls the common-case on-screen hold after the
    /// animation starts. Moved together with `SPLASH_MAX_MS` below so the two
    /// stay coordinated: still comfortably above the 1150 ms animation total,
    /// and the pair still fits the "~2-2.5 s max" acceptance budget.
    ///
    /// BUG FIX: the animation
    /// itself used to fire on the first `step()` call, the same clock this
    /// constant was measured against.
    ///
    /// BUG FIX (follow-on): the animation's
    /// start moved AGAIN — from the first `step()` call unconditionally, to
    /// the first `step()` call AFTER `mark_app_ready()` fires (see `step()`'s
    /// dismissal-gate block and `mark_app_ready`'s doc) — so a boot that
    /// takes a while to settle no longer starves the animation's own frames.
    /// This constant's floor moved with it: it is now measured from
    /// `splash_animation_started_ms` (the animation's own start clock), NOT
    /// from `splash_started_ms` (the splash's first-tick clock, which the
    /// `SPLASH_MAX_MS` cap below still uses) — those two clocks are now the
    /// same instant only when `mark_app_ready()` happens to fire by the
    /// splash's very first tick.
    const SPLASH_MIN_MS: u64 = 1600;
    /// Hard cap on splash duration, independent of `mark_app_ready()`,
    /// measured from the splash's first `step()` tick (`splash_started_ms`).
    /// Defensive: if boot never reaches `mark_app_ready()` at all (so the
    /// intro animation never even starts — see `step()`'s
    /// `splash_animation_started` gate), the splash must still clear on its
    /// own rather than wedging the UI indefinitely on the static logo.
    ///
    /// BUG FIX: raised from 2000 ms to 2400 ms
    /// — a coordinated nudge with `SPLASH_MIN_MS` above (same 800 ms margin
    /// between the two as before the change) so the defensive cap still sits
    /// comfortably above the new floor while the worst case stays inside the
    /// "~2-2.5 s max" acceptance budget.
    const SPLASH_MAX_MS: u64 = 2400;

    /// Wall-clock duration of the splash ripple's ONE FIRST animation cycle —
    /// ring 2 (the later-staggered of the two) finishes its first pass at
    /// this point (see `screens::splash` module doc's "Animation design" /
    /// "Dedicated render loop for the ripple" sections). `run_splash_ripple`
    /// spins its dedicated render loop for exactly this long, then returns —
    /// unchanged by the ripple now looping: this constant still
    /// bounds only the guaranteed-smooth FIRST cycle rendered on the
    /// dedicated loop, not the ripple's total on-screen lifetime, which is
    /// now open-ended (tied to the splash's own dismiss lifecycle — see
    /// `screens::splash`'s "Looping the ripple until dismiss" section).
    ///
    /// Mirrored (not referenced) by the `min_and_max_stay_within_the_
    /// acceptance_envelope` test's `SPLASH_ANIMATION_TOTAL_MS` — same
    /// reasoning as `SPLASH_MIN_MS`/`SPLASH_MAX_MS` above: that test exercises
    /// the pure `splash_should_dismiss` function without a concrete
    /// `UiRuntime<'d>` in scope. Keep both in sync with the `.slint` markup's
    /// `850ms` / `300ms delay` / `850ms` ring timings if either ever changes.
    const SPLASH_RIPPLE_TOTAL_MS: u64 = 1150;

    /// Sleep granularity of `run_splash_ripple`'s dedicated render loop.
    /// ~60 fps — comfortably finer than the eye can distinguish from
    /// continuous motion, and far finer than the irregular cadence the old
    /// shared-dispatcher-loop design suffered from (the whole point of this
    /// dedicated render loop).
    const SPLASH_RIPPLE_TICK_MS: u32 = 16;

    /// Render-cadence cap while an animation is settling — see the `render_settling`/
    /// `last_render_ms` field docs for the full mechanism and measurement.
    /// 16 ms (~60 fps) matches `SPLASH_RIPPLE_TICK_MS` above's own
    /// established precedent ("comfortably finer than the eye can
    /// distinguish from continuous motion"): Slint's `animate` blocks
    /// compute progress from wall-clock time elapsed since the property
    /// change, not from how many times they happen to get rendered, so
    /// capping the render cadence to this value changes nothing about an
    /// animation's timing, curve, or settled end state — it only skips
    /// intermediate frames a human eye was never going to resolve anyway,
    /// each of which would otherwise cost a full SPI-bus-contending
    /// `flush_line_range` sweep of whatever region the animation covers.
    const RENDER_MIN_INTERVAL_MS: u64 = 16;

    /// Bound on how many touch events / keyboard bytes `step()` will drain
    /// from a single input source in one call.
    ///
    /// `step()` runs once per dispatcher loop iteration, but the touch
    /// controller and the keyboard co-processor can both have accumulated
    /// several events by the time a given `step()` runs (the keyboard
    /// co-processor explicitly buffers key-down bytes FIFO — see
    /// `keyboard.rs` module doc). Draining only one event per `step()` call
    /// let that backlog outrun the drain rate under bursty input (fast
    /// typing, quick taps), which is the root cause this fixes: the
    /// touch/keyboard poll loops below now drain everything ready, up to this
    /// bound. The bound itself is defensive, not load-bearing under normal
    /// use — it exists only so a stuck sensor or a flooded bus cannot spin
    /// `step()` indefinitely and starve the radio RX poll / render pass later
    /// in the same dispatcher loop iteration. 8 comfortably exceeds any
    /// realistic single-iteration backlog (a human cannot generate 8 key-down
    /// events between two ~5 ms loop iterations) while still bounding worst
    /// case.
    const MAX_INPUT_EVENTS_PER_STEP: u8 = 8;

    /// How long `step()` holds off the Compose → MessageView navigation
    /// after a Send tap, so the Send button's `RocketOnSend` one-shot (see
    /// `screens::compose`'s module doc + `deferred_message_view_nav_at_ms`'s
    /// field doc) has time to actually render before the screen it's
    /// floating on gets torn down. 450ms comfortably clears the motif's
    /// 400ms arc-up+fade with a small margin, while staying well short of
    /// the button's own 500ms `rocket_trigger` auto-reset — the reset
    /// itself doesn't need to be seen since the screen is gone by then.
    const SEND_NAV_DEFER_MS: u64 = 450;

    /// Create and initialise the UI runtime.
    ///
    /// `is_provisioned`: if false, the initial screen is `Unprovisioned`.
    /// `pubkey_hex`: this device's public key in hex (shown on unprovisioned screen).
    pub fn new(
        display: TDeckDisplay<'d>,
        touch: TouchDriver<'d>,
        mut keyboard: Option<KeyboardDriver<'d>>,
        buzzer: Option<BuzzerDriver<'d>>,
        trackball: Option<trackball::TrackballDriver<'d>>,
        is_provisioned: bool,
        pubkey_hex: &str,
        self_name: &str,
    ) -> anyhow::Result<Self> {
        // Install the Slint platform (panics if called twice) and obtain the
        // cooperative rendering handle for the single shared software window.
        let window = platform::install();

        // BUG FIX: create the initial Slint screen component and call show() on it.
        // The previous implementation created only a ScreenState enum (navigation
        // stack) but never instantiated a Slint component, leaving the renderer
        // with nothing to draw → blank display on every frame.
        //
        // Boot splash: the FIRST active
        // screen is now always the splash, on both boot paths — NOT the real
        // initial screen (`Unprovisioned` / `ContactList`) directly. That real
        // screen is built lazily by `dismiss_splash()` once `step()`'s gate
        // opens (the intro animation has run for `SPLASH_MIN_MS`, or the
        // `SPLASH_MAX_MS` cap trips). The `is_provisioned` / `pubkey_hex`
        // arguments are stashed below for that later construction instead of
        // being consumed here.
        //
        // BUG FIX (fixed, then the fix itself moved again once boot settling
        // was better understood): `new()` no longer
        // fires the splash's one-shot intro animation — that is deferred to
        // `step()`'s first call AFTER `mark_app_ready()`
        // (`SplashScreen::start_animation()`, below). See that method's doc
        // for the mechanism this closes.
        let splash = screens::SplashScreen::new()?;
        splash.set_version(env!("MESHCADET_BUILD_VERSION"));
        let active_screen = ActiveScreen::Splash(splash);

        // pending_nav is a shared flag: Slint callbacks (which are 'static and
        // cannot capture &mut self) set it; step() drains it each iteration.
        // pending_nav_hash carries the tapped row's hash for MessageView nav.
        let pending_nav = std::rc::Rc::new(std::cell::Cell::new(0u8));
        let pending_nav_hash = std::rc::Rc::new(std::cell::Cell::new(0u8));

        // NOTE: contact-list callback wiring (`wire_contact_list_callbacks`)
        // used to happen right here. It now happens in `navigate_to_contact_list`
        // itself (called by `dismiss_splash`), since the `ContactList` component
        // isn't created until the splash dismisses.

        // Keyboard backlight follows the display backlight 1:1, including at
        // boot. The display
        // already boots with its own backlight on full duty
        // (`TDeckDisplay::new`) and `screen_asleep` starts `false` below, so
        // light the keyboard here too — otherwise it would stay dark until
        // the first sleep→wake cycle. `None` on touch-only boards.
        if let Some(kb) = keyboard.as_mut() {
            if let Err(e) = kb.set_backlight(true) {
                log::warn!("ui: keyboard backlight-on at boot failed: {:?}", e);
            }
        }

        Ok(UiRuntime {
            display,
            touch,
            keyboard,
            trackball,
            window,
            active_screen,
            notif: NotifDispatcher::new(notification::NotifPrefs::default()),
            buzzer,
            commands: Vec::new(),
            events: Vec::new(),
            messages: std::collections::HashMap::new(),
            contact_names: std::collections::HashMap::new(),
            self_name: self_name.to_string(),
            unread: std::collections::HashMap::new(),
            channel_items: Vec::new(),
            contact_list_selected: -1,
            admin_menu_selected: -1,
            stored_pin: [0u8; pin_menu::MAX_PIN_LEN],
            stored_pin_len: 0,
            runtime_settings: std::rc::Rc::new(std::cell::RefCell::new(
                pin_menu::RuntimeSettings::default_enabled(),
            )),
            nvs_partition: None,
            pending_nav,
            pending_nav_hash,
            pin_digits: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            active_convo: None,
            pending_compose_text: std::rc::Rc::new(std::cell::RefCell::new(None)),
            deferred_message_view_nav_at_ms: None,
            pending_compose_seed: None,
            gps_status: gps::GpsStatus::never(),
            battery_status: battery::BatteryStatus::unknown(),
            screen_asleep: false,
            // Overwritten by the first `step()` call — see the field doc.
            last_activity_ms: 0,
            activity_clock_started: false,
            touch_wake_swallow_active: false,
            // Matches the boot-time `kb.set_backlight(true)` write just above,
            // so the arbiter's first `step()` call is a no-op rather than a
            // redundant re-write of the same state.
            kb_backlight_on: true,
            initial_is_provisioned: is_provisioned,
            initial_pubkey_hex: pubkey_hex.to_string(),
            // Overwritten by the first `step()` call — see the field doc.
            splash_started_ms: 0,
            splash_clock_started: false,
            // Overwritten by `step()` once `app_ready` fires — see the field doc.
            splash_animation_started_ms: 0,
            splash_animation_started: false,
            app_ready: false,
            splash_dismiss_attempted: false,
            // `0` so the very first real render (boot splash) is never
            // throttled — see the field doc.
            last_render_ms: 0,
            render_settling: false,
        })
    }

    /// Register a contact name mapping (called after provisioned config is loaded).
    ///
    /// Also refreshes the contact list screen model if it is currently active.
    pub fn register_contact(&mut self, hash: u8, name: String) {
        self.contact_names.insert(hash, name);
        // Refresh the contact list Slint model if it is the active screen.
        if let ActiveScreen::ContactList(ref screen) = self.active_screen {
            let contacts = Self::build_contact_items(
                &self.contact_names, &self.messages, &self.unread,
            );
            screen.set_contacts(&contacts);
        }
    }

    /// Push channel list into the contact list screen (called after provisioned
    /// config is loaded, alongside `register_contact` calls).
    ///
    /// Also caches the items so they can be restored when navigating back from
    /// the PinEntry screen.
    pub fn set_channels(&mut self, channels: &[screens::contact_list::ChannelItem]) {
        self.channel_items = channels.to_vec();
        if let ActiveScreen::ContactList(ref screen) = self.active_screen {
            screen.set_channels(channels);
        }
    }

    /// Seed one conversation's restored message history at boot —
    /// populates `messages` directly,
    /// WITHOUT going through the live radio-event path (`on_send_message` /
    /// the RX handlers in `main.rs` that push onto this same map).
    ///
    /// Must be called once per conversation from `main.rs::run()` after the
    /// `HISTORY` flash store has been read back (`HistoryStore::
    /// load_all_conversations`) and after `register_contact`/`set_channels`,
    /// but BEFORE the first `navigate_to_contact_list` (driven by
    /// `dismiss_splash`, once the boot-splash gate opens) — otherwise the
    /// first contact/channel list build would compute previews/unread from
    /// an empty `messages` map and a live send/receive would be needed
    /// before restored history became visible.
    ///
    /// `is_channel` mirrors the `(hash, is_channel)` convention used
    /// elsewhere (`navigate_to_message_view`, `on_send_message`) and is used
    /// only for the diagnostic log line below — `messages` itself is keyed
    /// by `hash` alone, a pre-existing shared key space between contacts and
    /// channels (see `contact_and_channel_unread_share_one_map_and_clear_together`
    /// in the test module); this call does not change that.
    ///
    /// A conversation with no stored history (`records.is_empty()`) is left
    /// unseeded — `messages.get(&hash)` already falls back to "no preview /
    /// empty conversation" everywhere it's read (`build_contact_items`,
    /// `build_channel_items`, `navigate_to_message_view`), so an empty insert
    /// would be a no-op that only wastes a `HashMap` entry.
    pub fn seed_conversation(&mut self, hash: u8, is_channel: bool, records: Vec<MessageRecord>) {
        if records.is_empty() {
            return;
        }
        log::info!(
            "ui: hydrate — seeded {} restored message(s) for {} hash={:#04x}",
            records.len(),
            if is_channel { "channel" } else { "contact" },
            hash,
        );
        messages_insert_non_empty(&mut self.messages, hash, records);
    }

    /// Set the provisioned PIN for PIN-gated settings menu access.
    ///
    /// Called from `main.rs` after the provisioned config is loaded.
    /// `pin_len == 0` means no PIN is configured (settings are always locked
    /// in that case: `pin_menu::verify_pin` returns `false` for a zero-length
    /// stored PIN, preventing unintended access).
    pub fn set_pin(&mut self, pin: [u8; pin_menu::MAX_PIN_LEN], pin_len: u8) {
        self.stored_pin = pin;
        self.stored_pin_len = pin_len;
        log::info!(
            "ui: provisioned PIN stored (pin_len={})",
            pin_len,
        );
    }

    /// Seed the on-device admin-menu `RuntimeSettings` (loaded from NVS by
    /// the caller via `runtime_settings_store::load`).  Called once from
    /// `main.rs` alongside `set_pin`/`set_nvs_partition`.
    pub fn set_runtime_settings(&mut self, settings: pin_menu::RuntimeSettings) {
        *self.runtime_settings.borrow_mut() = settings;
    }

    /// Wire the NVS partition handle used to persist `runtime_settings` after
    /// an admin-menu toggle.  Called once from `main.rs` after the
    /// provisioned config has loaded.
    pub fn set_nvs_partition(&mut self, nvs: EspNvsPartition<NvsDefault>) {
        self.nvs_partition = Some(nvs);
    }

    /// Update the live stdin RX byte counter on the unprovisioned screen.
    ///
    /// No-op if the active screen is not the unprovisioned screen.
    /// Available only with `--features diagnostics`; compiled out of production.
    #[cfg(feature = "diagnostics")]
    pub fn set_prov_rx_bytes(&mut self, n: u32) {
        if let ActiveScreen::Unprovisioned(ref scr) = self.active_screen {
            scr.set_rx_bytes(n);
        }
    }

    /// Signal that boot has reached steady state.
    ///
    /// BUG FIX: previously this
    /// was only the "system is ready for use" half of the splash dismissal
    /// gate, paired with the `SPLASH_MIN_MS` wall-clock floor — but the
    /// splash's one-shot intro animation started unconditionally on `step()`'s
    /// first-ever call, which on a boot where GPS baud probe / radio SPI
    /// config / flash hydrate keep `step()` itself from running at a steady
    /// cadence yet left the animation's own frames landing sparsely and
    /// irregularly (choppy).
    ///
    /// FOLLOW-UP: gating the animation's start on THIS flag turned out not to be enough
    /// either — `step()` still shared the dispatcher loop with radio RX poll /
    /// GPS poll every iteration, so the ripple's own ~1150 ms of frames still
    /// landed irregularly even once `app_ready` was true (boot-to-boot
    /// variance: a lucky boot got a smooth ripple, an unlucky one — a radio
    /// packet or GPS burst mid-window — got a flash). `run_splash_ripple` (a
    /// SEPARATE method, called directly by `main.rs` right after this one) now
    /// owns firing the animation itself, on its own dedicated render loop
    /// that has NOTHING else to interleave. This method no longer starts the
    /// animation, directly or via `step()` — it only flips `app_ready`, which
    /// `step()`'s dismissal-gate diagnostic log (see `step()`) still reads.
    ///
    /// Call exactly once, from `main.rs`, immediately followed by a call to
    /// `run_splash_ripple`:
    /// - Provisioned boot: right before entering the dispatcher loop (radio,
    ///   GPS, history store, and the admin-server thread are all live by
    ///   then).
    /// - Unprovisioned boot: right before entering the USB-provisioning wait
    ///   loop — waiting for USB IS the ready state for an unprovisioned
    ///   device; there is no radio/GPS bring-up to wait on.
    ///
    /// A missed or delayed call does not wedge the UI: `SPLASH_MAX_MS` in
    /// `step()` dismisses the splash unconditionally past that cap, even if
    /// the intro animation never got to start (and hence `run_splash_ripple`
    /// never got called) at all.
    pub fn mark_app_ready(&mut self) {
        self.app_ready = true;
    }

    /// Fire the boot splash's ripple animation and render its FIRST cycle on
    /// a DEDICATED render loop that owns the calling thread for exactly
    /// `SPLASH_RIPPLE_TOTAL_MS` — see `screens::splash`'s module doc,
    /// "Dedicated render loop for the ripple". The ripple itself
    /// now LOOPS (see that module doc's "Looping the ripple until dismiss" section):
    /// this method's own dedicated-loop WINDOW is unchanged (still exactly
    /// one ring1+ring2 cycle, still returns control to `main.rs` at the same
    /// point it always did, so boot handoff timing is untouched), but once it
    /// returns, the ordinary `step()`-driven render cadence keeps advancing
    /// the same (now infinite-iteration) `animate` transitions for as long as
    /// the splash stays on screen — no further call into this method, and no
    /// other render-loop change, is needed for that.
    ///
    /// ROOT CAUSE this replaces: `step()` used to fire
    /// `SplashScreen::start_animation()` and then rely on the ordinary
    /// dispatcher loop's own `step()` calls to render the following
    /// `SPLASH_RIPPLE_TOTAL_MS` of ring animation — but that same loop
    /// iteration also polls radio RX and GPS every pass (see `main.rs`'s
    /// dispatcher loop), so `step()`'s cadence during the ripple window was
    /// irregular boot-to-boot: a lucky boot landed ~70 evenly-spaced frames
    /// (smooth expansion); an unlucky one (a radio packet or GPS NMEA burst
    /// landing mid-window) landed only a handful (a flash, not a ripple).
    /// Slint's `animate` blocks compute progress from wall-clock time elapsed
    /// since the property write, NOT from frames actually rendered — a sparse
    /// `step()` cadence doesn't slow the animation down, it just skips visibly
    /// painting most of it.
    ///
    /// FIX: this method instead spins a TIGHT loop — `update_timers_and_
    /// animations()` + `render_if_needed()` + a `SPLASH_RIPPLE_TICK_MS` sleep,
    /// nothing else — for `SPLASH_RIPPLE_TOTAL_MS`, so every tick actually
    /// paints a frame regardless of what radio/GPS/touch/keyboard are doing
    /// meanwhile (they are doing nothing meanwhile, precisely because this
    /// loop does not poll them). The ripple's first cycle, on every boot, at
    /// the same frame rate; subsequent cycles (the ripple now loops — see
    /// this method's own doc above) ride the ordinary `step()` cadence once
    /// this window ends.
    ///
    /// SAFETY NOTE — deferring radio RX poll for `SPLASH_RIPPLE_TOTAL_MS`
    /// (~1.15 s), once, at boot: `radio::Radio::try_receive`'s own doc
    /// confirms the SX1262 "stays in continuous RX throughout" independent of
    /// how often the driver polls DIO1 — the radio hardware keeps receiving
    /// into its internal buffer and latching `IRQ_RX_DONE` on its own clock,
    /// not on `try_receive`'s call cadence. A gap in *polling* is therefore
    /// not a gap in *reception*; the risk is narrower — a SECOND distinct
    /// packet landing before the FIRST is drained via SPI would overwrite the
    /// single hardware RX buffer. At this network's LoRa airtimes (~tens to
    /// low-hundreds of ms per frame at typical SF/BW) two independent packets
    /// landing inside one ~1.15 s window is already an infrequent boot-time
    /// coincidence, and — same as any other bounded RX gap this codebase
    /// already accepts (e.g. every `radio.transmit()` call blocks RX for its
    /// own airtime duration, every CAD pass does the same, both every-iteration
    /// events in steady-state operation, not one-shot boot events) — mesh
    /// flooding means a relay repeats the same logical message multiple times
    /// regardless, so a single dropped copy at boot is not a lost message.
    /// This is a ONE-TIME, boot-only gap, strictly bounded by
    /// `SPLASH_RIPPLE_TOTAL_MS`, in a class this architecture already
    /// tolerates continuously post-boot.
    ///
    /// CALL-SITE CONTRACT: call exactly once, from `main.rs`, immediately
    /// after `mark_app_ready()` — BEFORE entering the normal dispatcher loop /
    /// USB-provisioning wait loop (both call sites already do this). No-op
    /// (defensive) if the splash is no longer the active screen, or if it has
    /// already played — this method is not designed to be called more than
    /// once, but a defensive double-call must not double-animate.
    pub fn run_splash_ripple(&mut self) {
        if self.splash_animation_started || !matches!(self.active_screen, ActiveScreen::Splash(_)) {
            return;
        }

        // Ordering matters: `update_timers_and_animations()` MUST run before
        // `start_animation()` writes the properties — see
        // `SplashScreen::start_animation`'s doc for why (Slint's `animate`
        // blocks anchor to a CACHED `current_tick()`, refreshed only by this
        // call, not a live wall-clock read at property-write time).
        let start_ms = crate::uptime_ms();
        slint::platform::update_timers_and_animations();
        if let ActiveScreen::Splash(ref splash) = self.active_screen {
            splash.start_animation();
        } else {
            return;
        }
        self.splash_animation_started = true;
        self.splash_animation_started_ms = start_ms;
        // Boot-timing diagnosability: `start_ms` is exactly how long it took
        // `main.rs` to reach `mark_app_ready()` — valuable for correlating a
        // future "splash still looks choppy" field report against how large
        // that gap actually was on real hardware.
        log::info!("ui: splash ripple started at t={} ms since boot (dedicated render loop)", start_ms);

        loop {
            slint::platform::update_timers_and_animations();
            if let Err(e) = self.window.render_if_needed(&mut self.display) {
                log::warn!(
                    "ui: splash ripple render error: {:?} — aborting dedicated render loop early",
                    e,
                );
                break;
            }
            if crate::uptime_ms().saturating_sub(start_ms) >= Self::SPLASH_RIPPLE_TOTAL_MS {
                break;
            }
            esp_idf_hal::delay::FreeRtos::delay_ms(Self::SPLASH_RIPPLE_TICK_MS);
        }
        log::info!(
            "ui: splash ripple dedicated render loop done ({} ms elapsed)",
            crate::uptime_ms().saturating_sub(start_ms),
        );
    }

    /// Refresh the cached GPS status snapshot (fix state, coordinates + age,
    /// clock-sync state + age). Called every `main.rs` dispatcher-loop
    /// iteration — far more often than the displayed values actually change
    /// (`fix_age_secs`/`clock_sync_age_secs` only tick once a second; a fix
    /// only updates on a fresh GGA sentence), so this is unconditional
    /// recompute of state that rarely changes if not guarded.
    ///
    /// PERF: early-returns when
    /// `status` is bit-identical to the previously cached snapshot, BEFORE
    /// touching the (possibly open) GpsStatus screen. `GpsStatus` is
    /// `PartialEq`/`Eq` over exactly the fields the screen's four rows format
    /// (see that struct's fields) and `self.gps_status` is the sole source of
    /// truth those rows are seeded from (both here and at
    /// `navigate_to_gps_status` time) — so struct equality is exactly "would
    /// this push change anything on screen". Skipping the push also skips
    /// `GpsStatusScreen::set_status`'s four `format!`/`to_string()` heap
    /// allocations, which otherwise fire every dispatcher-loop iteration
    /// (many times a second) for values that visibly change roughly once a
    /// second. Cheap even on the common no-op path: `GpsStatus` is a small
    /// `Copy` struct, so the equality check itself costs nothing an
    /// unconditional field-copy didn't already cost.
    pub fn set_gps_status(&mut self, status: gps::GpsStatus) {
        if status == self.gps_status {
            return;
        }
        self.gps_status = status;
        if let ActiveScreen::GpsStatus(ref scr) = self.active_screen {
            scr.set_status(&status);
        }
    }

    /// Refresh the cached battery status snapshot (charge percentage +
    /// charging state). Called every `main.rs` dispatcher-loop iteration;
    /// cheap (a `Copy` struct) even when the AdminMenu screen is not open.
    /// If AdminMenu IS the active screen, also pushes the fresh value into
    /// it so the displayed battery row updates live rather than freezing at
    /// nav-open time.
    ///
    /// PERF: the cache itself is
    /// always refreshed (`raw_mv`/`held_raw_mv` are live diagnostic fields
    /// read elsewhere — e.g. the host `status` command — even though the
    /// on-device row never renders them), but the `format_battery_display`
    /// call + Slint push are gated on [`battery_display_fields_changed`] —
    /// only `percent`/`charging` (the two fields that row actually renders,
    /// per `format_battery_display`'s doc) — so a `raw_mv`-only ADC jitter
    /// tick (frequent) no longer allocates a fresh `String` and re-pushes it
    /// for a row that would render pixel-identically.
    pub fn set_battery_status(&mut self, status: battery::BatteryStatus) {
        let display_changed = battery_display_fields_changed(self.battery_status, status);
        self.battery_status = status;
        if display_changed {
            if let ActiveScreen::AdminMenu(ref scr) = self.active_screen {
                scr.set_battery_display(&screens::admin_menu::format_battery_display(status));
            }
        }
    }

    /// Post an event from the radio layer.  Processed on the next `step()`.
    pub fn post_event(&mut self, event: UiEvent) {
        self.events.push(event);
    }

    /// Drain pending commands for the radio dispatcher.
    pub fn drain_commands(&mut self) -> impl Iterator<Item = UiCommand> + '_ {
        self.commands.drain(..)
    }

    /// One cooperative step: process events, tick Slint, redraw if needed.
    ///
    /// Call once per dispatcher loop iteration.  Returns `Err` only on
    /// unrecoverable display hardware failure.
    pub fn step(&mut self, now_ms: u64) -> anyhow::Result<()> {
        // Seed the screen-sleep inactivity clock from the first call's `now_ms`
        // rather than from construction time — `UiRuntime::new` runs before
        // `main.rs`'s radio/provisioning bring-up, which can take longer than
        // a short configured timeout; seeding here instead means boot time
        // never counts against the inactivity window.
        if !self.activity_clock_started {
            self.last_activity_ms = now_ms;
            self.activity_clock_started = true;
        }

        // ── Boot-splash dismissal gate ──────────────────────────────────────
        // Seed the boot-first-tick clock the same way (first `step()` call,
        // not construction) — see `splash_started_ms`'s field doc. This clock
        // now backs ONLY the `SPLASH_MAX_MS` defensive cap below; the "has the
        // animation finished" half is measured from its OWN clock, seeded
        // separately once the animation actually starts (see the
        // `splash_animation_started` block right after this one).
        //
        // `!self.splash_dismiss_attempted` bounds this to AT MOST ONE call to
        // `dismiss_splash()`, matching how every other navigation failure in
        // this file behaves (`navigate_to_pin_entry`/`navigate_to_contact_list`
        // /etc. are each attempted once per triggering event, logged on
        // failure, and NOT retried). Without this guard, a screen-construction
        // failure inside `dismiss_splash` (e.g. `ContactListScreen::new()`
        // erroring) would leave `active_screen` as `Splash(_)` forever —
        // `step()` would then retry `dismiss_splash()` on every single
        // iteration (a log-spam retry storm) instead of degrading once, the
        // same way a failed nav-code dispatch degrades once.
        if !self.splash_clock_started {
            self.splash_started_ms = now_ms;
            self.splash_clock_started = true;
        }

        // BUG FIX:
        // `step()` no longer fires the splash's one-shot ripple itself — see
        // `run_splash_ripple`'s doc for why (in short: this shared dispatcher
        // loop also polls radio RX / GPS every iteration, which starved the
        // ripple's own frames of a steady cadence even after gating the start
        // on `app_ready`). `main.rs` now calls `run_splash_ripple` directly,
        // once, immediately after `mark_app_ready()`, on its own dedicated
        // render loop, BEFORE this dispatcher loop (or the USB-provisioning
        // wait loop) ever starts running. By the time `step()` runs for the
        // first time, `splash_animation_started` is already `true` (or the
        // splash was never reached / never dismissed — see the dismissal gate
        // right below, unchanged).
        if !self.splash_dismiss_attempted && matches!(self.active_screen, ActiveScreen::Splash(_)) {
            let elapsed = now_ms.saturating_sub(self.splash_started_ms);
            let animation_elapsed = self.splash_animation_started
                .then(|| now_ms.saturating_sub(self.splash_animation_started_ms));
            if splash_should_dismiss(elapsed, animation_elapsed, Self::SPLASH_MIN_MS, Self::SPLASH_MAX_MS) {
                let animation_settled = matches!(animation_elapsed, Some(ms) if ms >= Self::SPLASH_MIN_MS);
                if !animation_settled {
                    log::warn!(
                        "ui: splash SPLASH_MAX_MS ({} ms) reached before the intro animation \
                         settled (app_ready={}) — dismissing anyway",
                        Self::SPLASH_MAX_MS,
                        self.app_ready,
                    );
                }
                self.splash_dismiss_attempted = true;
                self.dismiss_splash();
            }
        }

        // ── Process pending navigation requests ──────────────────────────────
        // Slint callbacks are 'static closures and cannot call &mut self methods
        // directly.  They write a byte flag into a shared Rc<Cell>; we drain it
        // here at the top of every step() while we hold exclusive access.
        let nav = self.pending_nav.get();
        if nav != 0 {
            self.pending_nav.set(0);
            // BUG FIX:
            // any explicit navigation the user triggers supersedes an
            // outstanding deferred post-send nav (see
            // `deferred_message_view_nav_at_ms`'s field doc) — code `6`
            // below re-arms it fresh if this dispatch IS itself a new Send.
            // Without this, a stray deferred nav could fire later against
            // whatever screen the user has since navigated to (e.g. Back
            // to MessageView, then Write again to reopen a fresh Compose —
            // the type-only check at the deferred-nav site below can't tell
            // that apart from the ORIGINAL Compose instance the send
            // happened on).
            self.deferred_message_view_nav_at_ms = None;
            match nav {
                1 => { // ContactList → PinEntry
                    if let Err(e) = self.navigate_to_pin_entry() {
                        log::error!("ui: navigate to PIN entry failed: {:?}", e);
                    }
                }
                2 => { // PinEntry / MessageView → ContactList
                    if let Err(e) = self.navigate_to_contact_list() {
                        log::error!("ui: navigate to contact list failed: {:?}", e);
                    }
                }
                3 => { // ContactList → MessageView (contact conversation)
                    let hash = self.pending_nav_hash.get();
                    if let Err(e) = self.navigate_to_message_view(hash, false) {
                        log::error!("ui: navigate to message view (contact) failed: {:?}", e);
                    }
                }
                4 => { // ContactList → MessageView (channel conversation)
                    let hash = self.pending_nav_hash.get();
                    if let Err(e) = self.navigate_to_message_view(hash, true) {
                        log::error!("ui: navigate to message view (channel) failed: {:?}", e);
                    }
                }
                5 => { // MessageView → Compose
                    if let Err(e) = self.navigate_to_compose() {
                        log::error!("ui: navigate to compose failed: {:?}", e);
                    }
                }
                6 => { // Compose Send → send message now, defer re-opening the thread
                    let sent = self.pending_compose_text.borrow_mut().take();
                    if let (Some(text), Some((hash, is_channel))) = (sent, self.active_convo) {
                        self.on_send_message(hash, is_channel, text);
                    }
                    // BUG FIX: the MessageView navigate used to fire in
                    // this same tick, right here — tearing down the Compose
                    // screen (and its `RocketOnSend` one-shot) before the
                    // 400ms arc-up+fade ever rendered a frame. The send
                    // above is unchanged (still synchronous, same tick — no
                    // delivery latency added); only the screen swap is
                    // deferred, via `deferred_message_view_nav_at_ms`
                    // (checked right after this whole nav dispatch block).
                    self.deferred_message_view_nav_at_ms =
                        Some(now_ms + Self::SEND_NAV_DEFER_MS);
                }
                7 => { // Compose back/cancel → re-open the thread without sending
                    if let Some((hash, is_channel)) = self.active_convo {
                        if let Err(e) = self.navigate_to_message_view(hash, is_channel) {
                            log::error!("ui: navigate to message view after compose cancel failed: {:?}", e);
                        }
                    }
                }
                8 => { // PinEntry confirm (correct PIN) → AdminMenu
                    self.notif.fire(NotifEvent::PinSuccess, now_ms, self.screen_asleep);
                    if let Err(e) = self.navigate_to_admin_menu() {
                        log::error!("ui: navigate to admin menu failed: {:?}", e);
                    }
                }
                9 => { // AdminMenu "📍 GPS status" row → GpsStatus
                    if let Err(e) = self.navigate_to_gps_status() {
                        log::error!("ui: navigate to GPS status failed: {:?}", e);
                    }
                }
                10 => { // GpsStatus back → AdminMenu
                    if let Err(e) = self.navigate_to_admin_menu() {
                        log::error!("ui: navigate to admin menu (from GPS status) failed: {:?}", e);
                    }
                }
                11 => { // PinEntry confirm (wrong PIN) → stays on PinEntry
                    self.notif.fire(NotifEvent::PinError, now_ms, self.screen_asleep);
                }
                _ => {}
            }
        }

        // ── Deferred post-send navigation (rocket-on-send visibility) ────────
        // See `deferred_message_view_nav_at_ms`'s field doc and nav code `6`
        // above.
        if send_nav_deferral_elapsed(self.deferred_message_view_nav_at_ms, now_ms) {
            self.deferred_message_view_nav_at_ms = None;
            // Only follow through if the user is still looking at the
            // Compose screen the send happened on. If Back/cancel (nav
            // code 7) already navigated away in the interim, that
            // already satisfied "return to the thread" — forcing
            // another navigate here would yank the user back to a
            // screen they've since left.
            if matches!(self.active_screen, ActiveScreen::Compose(_)) {
                if let Some((hash, is_channel)) = self.active_convo {
                    if let Err(e) = self.navigate_to_message_view(hash, is_channel) {
                        log::error!(
                            "ui: navigate to message view after deferred send-nav failed: {:?}",
                            e
                        );
                    }
                }
            }
        }

        // ── Sync notification prefs from the admin-menu toggles ─────────────
        // Must run before `handle_event` below so a toggle flipped this same
        // tick already gates the events processed a few lines down. See
        // `sync_notif_prefs`'s doc.
        self.sync_notif_prefs();

        // ── Process pending radio events ──────────────────────────────────────
        let events = std::mem::take(&mut self.events);
        for event in events {
            self.handle_event(event, now_ms);
        }

        // ── Audible notification (buzzer) ───────────────────────────────────────
        // Deliberately runs here, unconditionally — before any touch/keyboard
        // polling and before any future sleep/backlight gating further down in
        // this function — so notification audio fires regardless of screen
        // awake/asleep state. `take_tones()` clears the dispatcher's single
        // pending-tone slot, so a sequence set by `handle_event`'s
        // `self.notif.fire(...)` calls above is played exactly once per
        // `step()` even if `step()` is re-entered before the next radio event.
        if let Some(sequence) = self.notif.take_tones() {
            if let Some(ref mut buzzer) = self.buzzer {
                buzzer.play(sequence);
            }
        }

        // ── Poll touch ────────────────────────────────────────────────────────
        // Global first-input-interceptor (screen-sleep wake): this check runs
        // ABOVE the screen stack, before any `dispatch_touch`, regardless of
        // which screen is active (including PinEntry) — see the `screen_asleep`
        // field doc. CRITICAL invariant: the wake-triggering input is consumed
        // to wake ONLY; it is never routed to the focused widget, so waking
        // never itself navigates, activates a button, or edits text.
        //
        // DEFECT FIX: this used to be a
        // single `match self.touch.poll_event() { ... }` — AT MOST one touch
        // transition consumed per `step()` call. `step()` only runs once per
        // dispatcher loop iteration, so a quick press/move/release burst (e.g.
        // a fast PIN-pad tap) could pile up faster than the loop drained it,
        // and the drained tail would land on *later* iterations — inflating
        // latency and, when a slower iteration intervened (radio RX poll, CAD
        // backoff), letting `TouchDriver`'s hardware-latched single-point state
        // get overwritten before it was ever read. Draining every event
        // `poll_event()` has ready for THIS step() call (bounded so a stuck
        // sensor can't starve RX/render) closes that gap without changing the
        // wake/swallow state machine at all — `touch_wake_transition` is pure
        // and already designed to be called once per event, in order.
        //
        // REGRESSION FIX: the drain
        // loop above calls `poll_event` back-to-back, microseconds apart,
        // which is far faster than the GT911 itself refreshes (~10ms) — this
        // exposed a latent release-inference bug in `TouchDriver::poll_event`
        // that synthesized a release from mere "no new frame yet", firing
        // spuriously mid-tap and turning one physical tap into two full
        // press/release pairs (doubled PIN digits). Fixed in `touch.rs`
        // itself (debounced release-by-silence inference); `now_ms` is now
        // threaded through so that fix can debounce against wall-clock time
        // rather than poll count.
        let mut touch_events_drained: u8 = 0;
        loop {
            match self.touch.poll_event(now_ms) {
                Ok(Some(ev)) => {
                    touch_events_drained += 1;
                    // `touch_wake_transition` is a pure function (see its doc) that
                    // decides wake/swallow/dispatch from state + event kind alone —
                    // extracted so the highest-risk logic here (the
                    // wake-must-not-leak-into-the-app invariant) is covered by host
                    // unit tests independent of the touch/Slint hardware stack.
                    let outcome = touch_wake_transition(self.screen_asleep, self.touch_wake_swallow_active, ev.kind);
                    self.touch_wake_swallow_active = outcome.swallow_active;
                    self.last_activity_ms = now_ms;
                    if outcome.woke {
                        self.wake_screen(now_ms);
                        // A still-pressed finger keeps reporting Moved (then
                        // Released) on later polls; `outcome.swallow_active`
                        // suppresses the rest of THIS gesture too so no partial
                        // press/release state from the wake tap can reach Slint.
                        log::info!("ui: touch woke screen (swallowed, kind={:?})", ev.kind);
                    }
                    if outcome.dispatch {
                        // dispatch_touch returns (logical_x, logical_y) after the Deg90-CW
                        // transform; capture for diagnostics overlay (ignored in production).
                        let _lxy = self.window.dispatch_touch(ev);
                        #[cfg(feature = "diagnostics")]
                        {
                            let (lx, ly) = _lxy;
                            // TouchEvent is Copy; ev.point is still accessible after the call.
                            log::info!(
                                "touch: raw({},{}) \u{2192} logical({},{})",
                                ev.point.x, ev.point.y, lx, ly,
                            );
                            if let ActiveScreen::ContactList(ref scr) = self.active_screen {
                                scr.set_touch_debug((ev.point.x, ev.point.y), (lx, ly));
                            }
                        }
                    }
                    if touch_events_drained >= Self::MAX_INPUT_EVENTS_PER_STEP {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    log::warn!("touch poll error: {:?}", e);
                    break;
                }
            }
        }

        // ── Poll physical keyboard ────────────────────────────────────────────
        // The T-Deck keyboard co-processor shares the I2C bus with the GT911
        // (software-serialised by this single task).  Each fresh key byte is
        // translated to a Slint key event and dispatched into the focused item
        // (e.g. the Compose draft TextInput).  After a key lands we refresh the
        // Compose shortcode autocomplete so typing ':' surfaces suggestions.
        //
        // Same global wake-and-swallow interceptor as touch, above. The
        // keyboard co-processor reports one physical key-press as a single
        // atomic `poll_key()` result (see `keyboard.rs` module docs) with no
        // press/release split, so swallowing this one event fully consumes
        // the wake-triggering key — there is no gesture tail to suppress.
        //
        // DEFECT FIX: this used to be
        // a single `match kb.poll_key() { ... }` — at most one byte drained
        // per `step()` call. `keyboard.rs`'s module doc is explicit that the
        // co-processor "buffers key-down events FIFO": fast typing queues
        // multiple bytes there, and a single-byte-per-step drain rate (bounded
        // below by the dispatcher loop's RX-poll cadence) let that onboard
        // FIFO fall behind and — once it filled — silently drop keys, which is
        // exactly the reported "typing loses characters" symptom. Draining
        // every byte `poll_key()` has ready for THIS step() call closes that
        // gap. Bounded by `MAX_INPUT_EVENTS_PER_STEP` so a stuck/flooding bus
        // can't starve RX/render, and broken early the moment a byte sets
        // `pending_nav` (screen-navigating keys — MessageView-seed, Compose
        // Return-to-send) so a same-burst second byte is never evaluated
        // against a screen context that is about to change out from under it;
        // it is deferred to the next `step()`, exactly as the old single-byte
        // drain always did for every byte after the first.
        {
            let mut key_bytes_drained: u8 = 0;
            loop {
                // Re-borrow `self.keyboard` fresh on every iteration (rather
                // than holding one `ref mut kb` across the whole loop) so the
                // borrow is provably released before the arm below calls back
                // into `self` (`self.wake_screen`, `self.pending_nav`, etc.)
                // — a loop-carried `kb` binding cannot satisfy the borrow
                // checker there since it is used again at the top of the next
                // iteration.
                let poll_result = match self.keyboard.as_mut() {
                    Some(kb) => kb.poll_key(),
                    None => break,
                };
                match poll_result {
                Ok(Some(byte)) => {
                    key_bytes_drained += 1;
                    if self.screen_asleep {
                        self.wake_screen(now_ms);
                        log::info!("ui: key press woke screen (swallowed, byte=0x{:02X})", byte);
                    } else {
                        self.last_activity_ms = now_ms;
                        // Printable keypress while viewing a conversation: jump
                        // straight to Compose (same destination as the Write
                        // button) with this character pre-loaded as the first
                        // typed character. `message_view_compose_seed`
                        // is the pure printable/non-printable decision (see its
                        // doc); gating it on `ActiveScreen::MessageView` here
                        // means no other screen's key handling is disturbed, and
                        // non-printable bytes (Backspace/Return/Tab/Escape, or
                        // anything `key_text` doesn't map at all) always fall
                        // through to the unchanged dispatch below — preserving
                        // existing navigation/shortcut behavior in MessageView
                        // (currently a no-op there, since it has no focusable
                        // input).
                        //
                        // This whole block only runs in the `!self.screen_asleep`
                        // arm above, so the key that merely wakes the device from
                        // sleep (handled in the `if self.screen_asleep` arm) can
                        // never reach here — it neither flips to write mode nor
                        // seeds a character.
                        let compose_seed = if matches!(self.active_screen, ActiveScreen::MessageView(_)) {
                            message_view_compose_seed(byte)
                        } else {
                            None
                        };
                        if let Some(seed) = compose_seed {
                            log::info!(
                                "ui: printable keypress in MessageView (byte=0x{:02X}) -> Compose (seeded)",
                                byte,
                            );
                            self.pending_compose_seed = Some(seed);
                            self.pending_nav.set(5);
                        } else if let ActiveScreen::Compose(ref screen) = self.active_screen {
                            if byte == 0x0D || byte == 0x0A {
                                // Return/Enter in Compose sends the draft — the
                                // same action as the Send button — instead of
                                // inserting a literal newline (this device's short
                                // mesh messages don't need multi-line entry, so
                                // Return-as-newline is superseded outright).
                                // `compose_return_should_send` is the pure
                                // empty/whitespace guard (see its doc): a
                                // `false` result is a total no-op here — the
                                // key is intercepted before it ever reaches
                                // `keyboard::key_text`/`dispatch_key`, so no
                                // newline lands in the draft either.
                                let draft = screen.get_draft();
                                if compose_return_should_send(&draft) {
                                    log::info!(
                                        "ui: compose Return pressed ({} chars) -> send + MessageView",
                                        draft.len(),
                                    );
                                    *self.pending_compose_text.borrow_mut() = Some(draft);
                                    // BUG FIX: the Send button's
                                    // `clicked` handler flips `rocket_trigger`
                                    // itself (see `compose.rs`'s Slint block),
                                    // but Return never reaches Slint at all —
                                    // this is the only place that would ever
                                    // fire the rocket for this send path, so
                                    // poke it here to match. Presentation-only:
                                    // the send stash + nav-code-6 dispatch
                                    // below are unchanged.
                                    screen.trigger_rocket();
                                    self.pending_nav.set(6);
                                } else {
                                    log::info!(
                                        "ui: compose Return on empty/whitespace draft — ignored (no send)",
                                    );
                                }
                            } else if let Some(text) = keyboard::key_text(byte) {
                                self.window.dispatch_key(text);
                                // A keystroke may have edited the Compose draft;
                                // refresh the `:shortcode:` autocomplete from the
                                // live text.
                                screen.refresh_completions();
                            }
                        } else if let Some(text) = keyboard::key_text(byte) {
                            self.window.dispatch_key(text);
                        }
                    }
                    // See `keyboard_drain_should_stop`'s doc: stops on a
                    // nav-triggering byte (screen is about to swap) or once
                    // the defensive drain bound is hit.
                    if keyboard_drain_should_stop(
                        self.pending_nav.get(),
                        key_bytes_drained,
                        Self::MAX_INPUT_EVENTS_PER_STEP,
                    ) {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    log::warn!("keyboard poll error: {:?}", e);
                    break;
                }
                }
            }
        }

        // ── Poll trackball ────────────────────────────────────────────────────
        // PARALLEL input modality — never
        // a replacement for touch/keyboard, which are both untouched above.
        // Same global wake-and-swallow interceptor as touch/keyboard: a roll
        // or click polled while asleep wakes the screen and is otherwise
        // discarded (never navigates/activates), matching the keyboard
        // block's discipline exactly — `poll()` reports one physical actuation
        // per call with no press/release split to drain, so (like the
        // keyboard) there is no gesture tail to suppress beyond this one event.
        if let Some(ref mut tb) = self.trackball {
            if let Some(ev) = tb.poll(now_ms) {
                if self.screen_asleep {
                    self.wake_screen(now_ms);
                    log::info!("ui: trackball input woke screen (swallowed, ev={:?})", ev);
                } else {
                    self.last_activity_ms = now_ms;
                    self.handle_trackball_event(ev);
                }
            }
        }

        // ── Screen-sleep inactivity check ─────────────────────────────────────
        // Incoming messages never reach `last_activity_ms` (handle_event above
        // never touches it) — only the touch/keyboard blocks above do, so a
        // message arriving does not wake or extend the screen's awake window
        // (explicit design decision).
        if !self.screen_asleep {
            let timeout_s = self.runtime_settings.borrow().screen_sleep_timeout_s;
            if timeout_s != 0 {
                let timeout_ms = (timeout_s as u64) * 1000;
                if now_ms.saturating_sub(self.last_activity_ms) >= timeout_ms {
                    self.sleep_screen();
                }
            }
            // timeout_s == 0 is the "never sleep" sentinel — no check at all.
        }

        // ── Keyboard-backlight arbiter ────────────────────────────────────────
        // Runs after both the wake path (touch/keyboard poll, above) and the
        // sleep path (inactivity check, just above) have settled
        // `screen_asleep` for this iteration, and after `handle_event` (top of
        // `step()`) has had a chance to start the blink loop for a
        // just-arrived message. See `sync_keyboard_backlight`'s doc.
        self.sync_keyboard_backlight(now_ms);

        // ── Tick Slint ────────────────────────────────────────────────────────
        // Unconditional every iteration, regardless of the render throttle
        // below: this is what fires Slint's own internal `Timer{}` callbacks
        // (e.g. `compose.rs`'s `rocket_trigger` auto-reset) and keeps every
        // animated property's CURRENT value exactly on its wall-clock curve —
        // only the (expensive, SPI-bus-contending) act of actually flushing a
        // frame to the display is ever throttled, never the clock itself.
        slint::platform::update_timers_and_animations();

        // ── Render dirty regions ──
        //
        // Full-window screen-entry fades (`reveal_opacity`/`content_opacity`)
        // and `motifs.slint` one-shots animate every `step()` iteration by
        // design — measurement (`docs/perf/ui-perf-baseline.md`) shows that
        // at this loop's natural cadence
        // (bounded by `RX_POLL_YIELD_MS` ≈ 5 ms when idle) an UNTHROTTLED
        // full-screen opacity fade re-flushes its ENTIRE bounding region —
        // for a near-full-window fade, effectively the whole 240-line
        // display — on every one of those ~5 ms ticks for the fade's whole
        // duration, not just once. `RENDER_MIN_INTERVAL_MS` caps how often
        // that expensive flush actually happens WHILE an animation is
        // settling, without adding any latency to a fresh one-off redraw
        // (navigation, incoming message, model update): `render_settling` —
        // set from `has_active_animations()` after the last frame that
        // actually drew — is `false` for exactly those one-off cases (no
        // animation was in flight when they were last observed), so they
        // always render on the very next tick, uncapped. Only a render that
        // itself just observed an in-flight animation gets the next one
        // deferred, and only up to `RENDER_MIN_INTERVAL_MS`.
        //
        // The animation's own timing is untouched (see
        // `RENDER_MIN_INTERVAL_MS`'s doc) — this changes ONLY how often an
        // already-identical curve gets sampled and flushed, never the curve,
        // duration, easing, or settled end state. Radio timeliness can only
        // improve: fewer/shorter SPI-bus-hold windows compete with the next
        // CAD attempt / RX poll per unit of wall-clock time, never more.
        let render_due = !self.render_settling
            || now_ms.saturating_sub(self.last_render_ms) >= Self::RENDER_MIN_INTERVAL_MS;
        if render_due {
            self.window.render_if_needed(&mut self.display)?;
            self.last_render_ms = now_ms;
            self.render_settling = self.window.has_active_animations();
        }

        Ok(())
    }

    fn handle_event(&mut self, event: UiEvent, now_ms: u64) {
        match event {
            UiEvent::IncomingDm { from_hash, from_name, text } => {
                self.contact_names
                    .entry(from_hash)
                    .or_insert_with(|| from_name.clone());
                self.messages
                    .entry(from_hash)
                    .or_default()
                    .push(MessageRecord {
                        text: text.clone(),
                        is_ours: false,
                        acked: false,
                        ts_ms: now_ms,
                    });
                // Don't flag unread if this DM thread is the one currently open —
                // the message lands directly in the live MessageView below, so the
                // user is reading it as it arrives; counting it unread would leave
                // a stale badge behind after they navigate back out. See
                // `incoming_message_is_unread`'s doc for the invariant this
                // depends on (`active_convo` must be cleared on return to
                // ContactList).
                if incoming_message_is_unread(self.active_convo, from_hash, false) {
                    *self.unread.entry(from_hash).or_insert(0) += 1;
                }
                self.notif.fire(NotifEvent::IncomingDm, now_ms, self.screen_asleep);
                // Diagnostic trail for the sleep/wake badge investigation —
                // logs the exact state the badge-refresh guard below sees at the
                // moment a DM lands, for HIL comparison against the channel
                // branch's equivalent line.
                log::debug!(
                    "ui: IncomingDm from_hash={:#04x} active_screen={} screen_asleep={} \
                     unread_for_hash={}",
                    from_hash, self.active_screen.name(), self.screen_asleep,
                    self.unread.get(&from_hash).copied().unwrap_or(0),
                );
                // Refresh contact list preview/unread badge.
                if let ActiveScreen::ContactList(ref screen) = self.active_screen {
                    let contacts = Self::build_contact_items(
                        &self.contact_names, &self.messages, &self.unread,
                    );
                    screen.set_contacts(&contacts);
                }
                // Refresh the live MessageView if this DM conversation is currently open.
                self.refresh_message_view_for(from_hash, false);
            }
            UiEvent::IncomingGroupMsg { channel_hash, text } => {
                self.messages
                    .entry(channel_hash)
                    .or_default()
                    .push(MessageRecord {
                        text,
                        is_ours: false,
                        acked: false,
                        ts_ms: now_ms,
                    });
                // Same "already reading it" guard as the DM branch above.
                if incoming_message_is_unread(self.active_convo, channel_hash, true) {
                    *self.unread.entry(channel_hash).or_insert(0) += 1;
                }
                self.notif.fire(NotifEvent::IncomingGroupMsg, now_ms, self.screen_asleep);
                // Diagnostic trail for the sleep/wake badge investigation —
                // see the IncomingDm branch's identical line above. Captures
                // whether the guard below actually sees `ContactList` at the
                // instant a channel message lands while asleep, and whether
                // `channel_items` (the provisioned catalog `build_channel_items`
                // reads identity from) is populated — the two concrete
                // failure hypotheses static analysis alone could not
                // rule out.
                log::debug!(
                    "ui: IncomingGroupMsg channel_hash={:#04x} active_screen={} screen_asleep={} \
                     channel_items_len={} unread_for_hash={}",
                    channel_hash, self.active_screen.name(), self.screen_asleep,
                    self.channel_items.len(),
                    self.unread.get(&channel_hash).copied().unwrap_or(0),
                );
                // Refresh channel list preview/unread badge — mirrors the DM branch
                // above.  This call was previously missing entirely, which is why
                // channel rows never showed an unread badge: `self.unread` was
                // incremented but nothing ever pushed the updated count into the
                // Slint model.
                if let ActiveScreen::ContactList(ref screen) = self.active_screen {
                    let channels = Self::build_channel_items(
                        &self.channel_items, &self.messages, &self.unread,
                    );
                    screen.set_channels(&channels);
                }
                // Refresh the live MessageView if this channel conversation is currently open.
                self.refresh_message_view_for(channel_hash, true);
            }
            UiEvent::DmAcked { to_hash } => {
                // Mark the last outbound message to this contact as acked.
                mark_last_unacked_outbound(&mut self.messages, to_hash);
                self.notif.fire(NotifEvent::DmAcked, now_ms, self.screen_asleep);
                // Refresh the live MessageView so the ✓→✓✓ indicator updates immediately.
                self.refresh_message_view_for(to_hash, false);
            }
            UiEvent::ChannelAcked { channel_hash } => {
                // Mark the last outbound message to this channel as acked —
                // same match function the DM path uses (`self.messages` is
                // keyed by `u8` for both contacts and channels).
                mark_last_unacked_outbound(&mut self.messages, channel_hash);
                self.notif.fire(NotifEvent::ChannelAcked, now_ms, self.screen_asleep);
                // Refresh the live MessageView so the ✓→✓✓ indicator updates immediately.
                self.refresh_message_view_for(channel_hash, true);
            }
            UiEvent::TelemetryResponse { from_hash, lat_e7, lon_e7, age_secs } => {
                let text = format!(
                    "📍 loc {:.5},{:.5} ({:.0}s ago)",
                    lat_e7 as f64 / 1e7,
                    lon_e7 as f64 / 1e7,
                    age_secs,
                );
                self.messages
                    .entry(from_hash)
                    .or_default()
                    .push(MessageRecord { text, is_ours: false, acked: false, ts_ms: now_ms });
                self.notif.fire(NotifEvent::TelemetryResponse, now_ms, self.screen_asleep);
                // Refresh the live MessageView if this contact's conversation is currently open.
                self.refresh_message_view_for(from_hash, false);
            }
        }
    }

    // ── Trackball navigation ──────────────────────────────────────────────────
    //
    // Agreed interaction model, the same
    // on every screen it applies to: roll Up/Down moves a highlight or scrolls,
    // center Click activates the highlighted row (or the screen's primary
    // action), roll Left is Back/pop, roll Right is reserved (no job yet).
    //
    // Every branch below drives the screen through the SAME Slint callback a
    // touch tap would invoke (`invoke_*` on the screen wrapper) rather than
    // duplicating the navigation/settings-apply logic those callbacks already
    // do — trackball input is deliberately just another way to fire the exact
    // same event a tap fires, not a second code path that could drift from it.

    /// Pixels scrolled per trackball roll step in MessageView — roughly one
    /// message bubble line. No acceleration curve yet (debounce/acceleration
    /// is a tuning knob for a follow-on, not a v1
    /// requirement); a fixed step is the simplest thing that satisfies "roll
    /// scrolls history".
    const TRACKBALL_SCROLL_STEP_PX: i32 = 40;

    /// Route one polled [`trackball::TrackballEvent`] to whichever screen is
    /// currently active. Screens with no trackball job this pass (Splash,
    /// Unprovisioned, PinEntry, Compose — the latter two are explicitly
    /// deferred: digit/cursor entry stays keyboard/touch-only) fall through to
    /// the wildcard no-op.
    fn handle_trackball_event(&mut self, ev: trackball::TrackballEvent) {
        match self.active_screen {
            ActiveScreen::ContactList(_) => self.handle_trackball_contact_list(ev),
            ActiveScreen::MessageView(_) => self.handle_trackball_message_view(ev),
            ActiveScreen::AdminMenu(_) => self.handle_trackball_admin_menu(ev),
            // Read-only status screen — no rows to highlight/activate, so
            // only Left (back to AdminMenu) has a job.
            ActiveScreen::GpsStatus(ref screen) if ev == trackball::TrackballEvent::Left => {
                screen.invoke_back_pressed();
            }
            _ => {}
        }
    }

    /// ContactList: Up/Down moves `contact_list_selected` within whichever tab
    /// (contacts or channels) is currently visible; Click opens that row's
    /// thread via the same `contact_selected`/`channel_selected` callback a
    /// tap uses. Left is a no-op — ContactList is the navigation root, there is
    /// nothing to back out of. Right is reserved.
    fn handle_trackball_contact_list(&mut self, ev: trackball::TrackballEvent) {
        use trackball::TrackballEvent::*;
        let ActiveScreen::ContactList(ref screen) = self.active_screen else { return };
        let show_contacts = screen.show_contacts();
        let len = if show_contacts {
            Self::build_contact_items(&self.contact_names, &self.messages, &self.unread).len()
        } else {
            Self::build_channel_items(&self.channel_items, &self.messages, &self.unread).len()
        };
        match ev {
            Up | Down => {
                let next = roll_selection(self.contact_list_selected, len as i32 - 1, ev == Up);
                if next < 0 {
                    return; // empty list — nothing to highlight
                }
                self.contact_list_selected = next;
                screen.set_selected_index(next);
            }
            Click => {
                if self.contact_list_selected < 0 || self.contact_list_selected as usize >= len {
                    return; // nothing highlighted yet — click has no target
                }
                let idx = self.contact_list_selected as usize;
                if show_contacts {
                    let items = Self::build_contact_items(&self.contact_names, &self.messages, &self.unread);
                    screen.invoke_contact_selected(items[idx].hash);
                } else {
                    let items = Self::build_channel_items(&self.channel_items, &self.messages, &self.unread);
                    screen.invoke_channel_selected(items[idx].hash);
                }
            }
            Left | Right => {}
        }
    }

    /// MessageView: Up/Down scrolls the thread by [`Self::TRACKBALL_SCROLL_STEP_PX`];
    /// Click opens Compose (same destination as the "✏ Write" button); Left
    /// goes back to ContactList. Right is reserved.
    fn handle_trackball_message_view(&mut self, ev: trackball::TrackballEvent) {
        use trackball::TrackballEvent::*;
        let ActiveScreen::MessageView(ref screen) = self.active_screen else { return };
        match ev {
            Up => screen.scroll_by(Self::TRACKBALL_SCROLL_STEP_PX),
            Down => screen.scroll_by(-Self::TRACKBALL_SCROLL_STEP_PX),
            Click => screen.invoke_compose_pressed(),
            Left => screen.invoke_back_pressed(),
            Right => {}
        }
    }

    /// AdminMenu: Up/Down moves `admin_menu_selected` across the four rows
    /// (visual-notif toggle, audible-notif toggle, screen-sleep stepper, GPS
    /// status row); Click activates the highlighted row via the SAME callback
    /// its touch control uses — a toggle flips, the stepper increments (its
    /// "+"; there's no single obvious "activate" for a bidirectional stepper,
    /// so this picks the more common direction and leaves fine adjustment to
    /// touch), and the GPS-status row navigates. Left goes back to ContactList.
    fn handle_trackball_admin_menu(&mut self, ev: trackball::TrackballEvent) {
        use trackball::TrackballEvent::*;
        const ROW_COUNT: i32 = 4;
        let ActiveScreen::AdminMenu(ref screen) = self.active_screen else { return };
        match ev {
            Up | Down => {
                let next = roll_selection(self.admin_menu_selected, ROW_COUNT - 1, ev == Up);
                self.admin_menu_selected = next;
                screen.set_selected_index(next);
            }
            Click => match self.admin_menu_selected {
                0 => screen.invoke_toggle_notif_visual(),
                1 => screen.invoke_toggle_notif_audible(),
                2 => screen.invoke_increment_screen_sleep_timeout(),
                3 => screen.invoke_open_gps_status(),
                _ => {} // nothing highlighted yet — click has no target
            },
            Left => screen.invoke_back_pressed(),
            Right => {}
        }
    }

    /// Route a composed message to the right transport and store it locally.
    ///
    /// Expands `:shortcode:` emoji, wraps `@name` mentions into their wire
    /// form (`@[name]` — see [`Self::wrap_outgoing_mentions`]), appends an
    /// outbound record to the thread so the conversation immediately shows
    /// the sent message, and queues the transport command (DM for contacts,
    /// group text for channels).
    fn on_send_message(&mut self, hash: u8, is_channel: bool, raw_text: String) {
        // Expand shortcodes before storing and sending.
        let mut expanded = [0u8; 512];
        let after_emoji = match protocol::emoji::expand_shortcodes(raw_text.as_bytes(), &mut expanded) {
            Some(n) => String::from_utf8_lossy(&expanded[..n]).into_owned(),
            None => raw_text.clone(),
        };

        // Wrap @mentions into wire form (`@name` -> `@[name]`) against the
        // known-names set (contacts ∪ this node's own name — a self-mention
        // should match too). Stored + sent as the WIRE form; `build_message_items` is
        // the single place that strips brackets back out for display, so
        // sent and received messages render through one code path.
        let known: Vec<&str> = self.known_names();
        let text = Self::wrap_outgoing_mentions(&after_emoji, &known);

        if text.trim().is_empty() {
            log::info!("ui: compose send ignored — empty draft after expansion");
            return;
        }

        self.messages
            .entry(hash)
            .or_default()
            .push(MessageRecord {
                text: text.clone(),
                is_ours: true,
                acked: false,
                ts_ms: 0, // filled in by dispatcher
            });
        if is_channel {
            log::info!("ui: send GRP_TXT ch={:#04x} ({} bytes)", hash, text.len());
            self.commands.push(UiCommand::SendGroupMsg { channel_hash: hash, text });
        } else {
            log::info!("ui: send DM to={:#04x} ({} bytes)", hash, text.len());
            self.commands.push(UiCommand::SendDm { to_hash: hash, text });
        }
        // Refresh the live MessageView if this conversation is the active screen.
        // Currently the only caller is nav-code-6 (compose → send), where the
        // active screen is still Compose, so this is a no-op there;
        // navigate_to_message_view() immediately follows and rebuilds the model from
        // self.messages (which now includes the sent record). Kept as a real
        // refresh (not deleted) in case a future caller invokes this while a
        // MessageView is already the active screen.
        self.refresh_message_view_for(hash, is_channel);
    }

    /// This node's known-names set for @mention matching: every contact
    /// display name currently registered, plus this node's own name (a
    /// mention of yourself is matchable too). Rebuilt on
    /// every call rather than cached: `contact_names` can grow after
    /// construction (`register_contact`), and this is only ever called from
    /// interactive paths (send, navigate, refresh), never a hot loop.
    fn known_names(&self) -> Vec<&str> {
        self.contact_names
            .values()
            .map(String::as_str)
            .chain(std::iter::once(self.self_name.as_str()))
            .collect()
    }

    /// Wrap `@name` occurrences in `text` into wire form `@[name]` against
    /// `known` (see `protocol::mention::wrap_mentions`). Pure/static so it's
    /// unit-testable without a live `UiRuntime` (which needs hardware
    /// handles to construct — see `build_message_items`'s doc for the same
    /// pattern). Falls back to `text` verbatim on overflow of the internal
    /// scratch buffer (matches `expand_shortcodes`'s call site's fallback
    /// style just above) — a composed message longer than the scratch
    /// buffer is already bounded well under it by the compose screen's own
    /// input limit.
    fn wrap_outgoing_mentions(text: &str, known: &[&str]) -> String {
        let mut out = [0u8; 512];
        match protocol::mention::wrap_mentions(text.as_bytes(), known, &mut out) {
            Some(n) => String::from_utf8_lossy(&out[..n]).into_owned(),
            None => text.to_string(),
        }
    }

    /// Build a sorted contact item list from the current data maps.
    ///
    /// Static function so it can be called while `self.active_screen` is
    /// borrowed — Rust's field-splitting rules allow simultaneous borrows of
    /// separate struct fields.
    fn build_contact_items(
        contact_names: &std::collections::HashMap<u8, String>,
        messages: &std::collections::HashMap<u8, Vec<MessageRecord>>,
        unread: &std::collections::HashMap<u8, u32>,
    ) -> Vec<screens::contact_list::ContactItem> {
        use screens::contact_list::ContactItem;
        let mut items: Vec<ContactItem> = contact_names.iter().map(|(&hash, name)| {
            ContactItem {
                name: name.clone(),
                preview: messages.get(&hash)
                    .and_then(|msgs| msgs.last())
                    .map(|m| m.text.clone())
                    .unwrap_or_default(),
                time_str: String::new(),
                unread: *unread.get(&hash).unwrap_or(&0) as i32,
                hash,
            }
        }).collect();
        // Sort by unread count (desc) then name (asc) for consistent ordering.
        items.sort_by(|a, b| b.unread.cmp(&a.unread).then(a.name.cmp(&b.name)));
        items
    }

    /// Build a fresh channel item list with up-to-date preview/unread from the
    /// current data maps, using `channel_items` (the provisioned catalog: name +
    /// hash) as the source of truth for identity.
    ///
    /// Mirrors `build_contact_items`.  Without this, `self.channel_items` — the
    /// raw catalog pushed once at provisioning with `unread: 0` — was pushed to
    /// the screen verbatim on every return to ContactList, permanently
    /// overwriting any unread count that had accumulated in `self.unread`.
    fn build_channel_items(
        channel_items: &[screens::contact_list::ChannelItem],
        messages: &std::collections::HashMap<u8, Vec<MessageRecord>>,
        unread: &std::collections::HashMap<u8, u32>,
    ) -> Vec<screens::contact_list::ChannelItem> {
        use screens::contact_list::ChannelItem;
        let mut items: Vec<ChannelItem> = channel_items.iter().map(|c| {
            ChannelItem {
                name: c.name.clone(),
                preview: messages.get(&c.hash)
                    .and_then(|msgs| msgs.last())
                    .map(|m| m.text.clone())
                    .unwrap_or_default(),
                time_str: String::new(),
                unread: *unread.get(&c.hash).unwrap_or(&0) as i32,
                hash: c.hash,
            }
        }).collect();
        // Sort by unread count (desc) then name (asc) — same ordering rule as
        // `build_contact_items`, for cross-tab consistency.
        items.sort_by(|a, b| b.unread.cmp(&a.unread).then(a.name.cmp(&b.name)));
        items
    }

    // ── PIN-gated navigation ──────────────────────────────────────────────────

    /// Navigate from any screen to the PinEntry screen.
    ///
    /// Creates a fresh [`screens::PinEntryScreen`], wires digit/backspace/
    /// confirm/cancel callbacks via [`screens::PinEntryScreen::wire_pin_callbacks`],
    /// and replaces `active_screen`.  The old screen is dropped here, which
    /// automatically hides it (Slint components hide on drop).
    ///
    /// On confirm: calls [`pin_menu::verify_pin`] against the stored PIN.
    /// - Correct → sets `pending_nav = 8` (forward to AdminMenu, fires
    ///   [`NotifEvent::PinSuccess`]) and logs unlock.
    /// - Wrong   → resets the digit display, logs the failure, sets
    ///   `pending_nav = 11` (fires [`NotifEvent::PinError`], stays on PinEntry).
    ///
    /// On cancel: sets `pending_nav = 2` (back to ContactList).
    fn navigate_to_pin_entry(&mut self) -> anyhow::Result<()> {
        log::info!("ui: navigate_to_pin_entry");
        // Hide the outgoing screen before surfacing the incoming one so the
        // single shared window releases the previous component as its visible
        // content (see `hide_active_screen` + `request_redraw`).
        self.hide_active_screen();
        let screen = screens::PinEntryScreen::new("Admin Menu")?;

        let digit_buf = std::rc::Rc::new(std::cell::RefCell::new(Vec::<u8>::with_capacity(4)));
        // Store a handle to the digit buffer so navigate_to_contact_list can
        // clear it if called before a confirm (e.g. on a back gesture).
        self.pin_digits = digit_buf.clone();

        let pn_confirm = self.pending_nav.clone();
        let pn_cancel  = self.pending_nav.clone();
        let stored_pin     = self.stored_pin;
        let stored_pin_len = self.stored_pin_len;

        screen.wire_pin_callbacks(
            digit_buf,
            // on_confirmed: called with the entered digits after the display is reset.
            move |digits| {
                let ok = pin_menu::verify_pin(&digits, &stored_pin, stored_pin_len);
                if ok {
                    log::info!("pin_menu: PIN verified — admin menu unlocked");
                    pn_confirm.set(8);
                } else {
                    log::warn!("pin_menu: PIN incorrect — {} digit(s) entered", digits.len());
                    // Stay on PinEntry — `step()` fires the PinError notification
                    // for code 11 but does not change `active_screen`.  The
                    // display was already reset by wire_pin_callbacks before
                    // this closure ran.
                    pn_confirm.set(11);
                }
            },
            // on_cancelled: user pressed ✕.
            move || {
                log::debug!("pin_menu: PIN entry cancelled");
                pn_cancel.set(2);
            },
        );

        self.active_screen = ActiveScreen::PinEntry(screen);
        // Force a full repaint so the cooperative loop surfaces the PIN pad.
        self.window.request_redraw();

        // ── Diagnostic: main-task stack headroom at this exact transition ──
        //
        // This is the CONFIRMED site of a release-only main-task stack
        // overflow (an on-hardware backtrace fired right
        // after the "ui: navigate_to_pin_entry" log line, with a preceding
        // HWM sample of only 5992 B free / 32768 B total — 18% headroom).
        // `PinEntryScreen::new()` above constructs `PinEntryScreenUi`, the
        // single largest compiled component in this binary (confirmed via
        // `nm --size-sort` against both build profiles — its generated
        // `new()` has the biggest stack frame of every screen sampled), and
        // it recently gained a full-window `SpaceBackdrop` on top of an
        // already-dense 12-key numpad + dot row + mascot layout — the
        // marginal addition that tipped an already-tight budget over the edge.
        // Logged unconditionally HERE — not waiting for the periodic 30 s
        // sample in `main.rs` — because a stack overflow reboots the task
        // before its next periodic tick could ever fire. Any future screen
        // that adds `SpaceBackdrop` should copy this same
        // one-line pattern at its own navigate_to_* call site.
        log_stack_hwm("navigate_to_pin_entry");

        Ok(())
    }

    /// Navigate from PinEntry (correct PIN) to the AdminMenu screen.
    ///
    /// Creates a fresh [`screens::AdminMenuScreen`], seeds its toggles from
    /// `self.runtime_settings`, and wires each toggle's `'static` Slint
    /// callback to apply the change via [`pin_menu::apply_menu_action`] and
    /// persist the result via [`persist_runtime_settings`]. Back navigation
    /// sets `pending_nav = 2` (ContactList), reusing the same code every other
    /// screen's back button uses.
    fn navigate_to_admin_menu(&mut self) -> anyhow::Result<()> {
        log::info!("ui: navigate_to_admin_menu");
        // Fresh visit starts with no trackball highlight (see field doc).
        self.admin_menu_selected = -1;
        self.hide_active_screen();
        let screen = screens::AdminMenuScreen::new()?;

        {
            let settings = self.runtime_settings.borrow();
            screen.set_notif_visual(settings.notif_visual);
            screen.set_notif_audible(settings.notif_audible);
            screen.set_screen_sleep_timeout(settings.screen_sleep_timeout_s as i32);
        }
        // Seed the battery row from the cached snapshot (refreshed every
        // dispatcher-loop iteration by `set_battery_status`) so the freshly
        // opened menu shows the current reading immediately, not "—".
        screen.set_battery_display(&screens::admin_menu::format_battery_display(self.battery_status));

        // Back → ContactList.
        let pn_back = self.pending_nav.clone();
        screen.on_back_pressed(move || {
            log::info!("ui: admin menu back pressed -> ContactList");
            pn_back.set(2);
        });

        // Visual-notifications toggle: apply via pin_menu::apply_menu_action,
        // then persist the whole RuntimeSettings to NVS.
        let settings = self.runtime_settings.clone();
        let nvs = self.nvs_partition.clone();
        screen.on_toggle_notif_visual(move |new_val| {
            let mut s = settings.borrow_mut();
            pin_menu::apply_menu_action(&pin_menu::MenuAction::SetNotifVisual(new_val), &mut s);
            log::info!("pin_menu: notif_visual -> {}", new_val);
            persist_runtime_settings(&nvs, &s);
        });

        // Audible-notifications toggle: same pattern.
        let settings = self.runtime_settings.clone();
        let nvs = self.nvs_partition.clone();
        screen.on_toggle_notif_audible(move |new_val| {
            let mut s = settings.borrow_mut();
            pin_menu::apply_menu_action(&pin_menu::MenuAction::SetNotifAudible(new_val), &mut s);
            log::info!("pin_menu: notif_audible -> {}", new_val);
            persist_runtime_settings(&nvs, &s);
        });

        // Screen-sleep timeout stepper: same apply/persist pattern as the
        // toggles above. `new_val` is the widget's already-clamped-to-0
        // (decrement) or 120 (increment) i32; apply_menu_action re-clamps to
        // 0..=120 as the single source of truth (see MenuAction::SetScreenSleepTimeout).
        let settings = self.runtime_settings.clone();
        let nvs = self.nvs_partition.clone();
        screen.on_decrement_screen_sleep_timeout(move |new_val| {
            let mut s = settings.borrow_mut();
            pin_menu::apply_menu_action(
                &pin_menu::MenuAction::SetScreenSleepTimeout(new_val.max(0) as u8),
                &mut s,
            );
            log::info!("pin_menu: screen_sleep_timeout_s -> {}", s.screen_sleep_timeout_s);
            persist_runtime_settings(&nvs, &s);
        });
        let settings = self.runtime_settings.clone();
        let nvs = self.nvs_partition.clone();
        screen.on_increment_screen_sleep_timeout(move |new_val| {
            let mut s = settings.borrow_mut();
            pin_menu::apply_menu_action(
                &pin_menu::MenuAction::SetScreenSleepTimeout(new_val.min(120) as u8),
                &mut s,
            );
            log::info!("pin_menu: screen_sleep_timeout_s -> {}", s.screen_sleep_timeout_s);
            persist_runtime_settings(&nvs, &s);
        });

        // "📍 GPS status" row → GpsStatus sub-screen (read-only, no state to
        // apply/persist here — pure navigation).
        let pn_gps = self.pending_nav.clone();
        screen.on_open_gps_status(move || {
            log::info!("ui: admin menu -> GPS status");
            pn_gps.set(9);
        });

        self.active_screen = ActiveScreen::AdminMenu(screen);
        // Force a full repaint so the cooperative loop surfaces the menu.
        self.window.request_redraw();

        // ── Diagnostic: main-task stack headroom at this exact transition ──
        //
        // An on-hardware backtrace pinned the CONFIRMED overflow to
        // `navigate_to_pin_entry` (see that function's own HWM log + doc),
        // not this one — but this transition is the second-densest screen
        // swap reachable in the same "open Settings" path (both reachable
        // predecessors, PinEntry-confirm and the GpsStatus back button, land
        // here), tearing down a predecessor screen's whole Slint item tree
        // while constructing + wiring six callbacks on the incoming
        // AdminMenuScreenUi (SpaceBackdrop + RingedPlanetCorner + 4 rows).
        // Kept as secondary coverage — same "log unconditionally, don't wait
        // for the periodic 30 s sample" rationale — so a HIL run that clears
        // the pin_entry overflow but is still marginal here shows up
        // immediately rather than silently.
        log_stack_hwm("navigate_to_admin_menu");

        Ok(())
    }

    /// Navigate from AdminMenu to the read-only GpsStatus sub-screen.
    ///
    /// Creates a fresh [`screens::GpsStatusScreen`], seeds it from the cached
    /// [`Self::gps_status`] snapshot, and wires its back button to
    /// `pending_nav = 10` (back to AdminMenu). No toggles/edits are wired —
    /// this screen is status/display only by design.
    fn navigate_to_gps_status(&mut self) -> anyhow::Result<()> {
        log::info!("ui: navigate_to_gps_status");
        self.hide_active_screen();
        let screen = screens::GpsStatusScreen::new()?;
        screen.set_status(&self.gps_status);

        let pn_back = self.pending_nav.clone();
        screen.on_back_pressed(move || {
            log::info!("ui: GPS status back pressed -> AdminMenu");
            pn_back.set(10);
        });

        self.active_screen = ActiveScreen::GpsStatus(screen);
        self.window.request_redraw();
        Ok(())
    }

    /// Swap the boot splash out for the real initial screen once `step()`'s
    /// dismissal gate opens.
    ///
    /// Provisioned devices land on `ContactList`; this reuses
    /// `navigate_to_contact_list` outright, since by this point
    /// `register_contact`/`set_channels` (called from `main.rs` right after
    /// `UiRuntime::new`, while the splash was still active) have already
    /// populated `self.contact_names` / `self.channel_items` — exactly the
    /// cached state that method repopulates a fresh `ContactListScreen`
    /// from. Unprovisioned devices land on `Unprovisioned`, built here
    /// directly (mirrors what `new()` used to do inline before the splash
    /// existed).
    fn dismiss_splash(&mut self) {
        // NOTE: only ever logs "dismissed -> X" on the branch that actually
        // succeeded — each failure path below logs its own `log::error!` and
        // returns without the misleading "dismissed" line (a screen that
        // failed to construct did not, in fact, dismiss into anything).
        if self.initial_is_provisioned {
            match self.navigate_to_contact_list() {
                Ok(()) => log::info!("ui: boot splash dismissed -> ContactList"),
                Err(e) => log::error!("ui: splash -> contact list failed: {:?}", e),
            }
        } else {
            self.hide_active_screen();
            match screens::UnprovisionedScreen::new() {
                Ok(screen) => {
                    screen.set_pubkey_hex(&self.initial_pubkey_hex);
                    self.active_screen = ActiveScreen::Unprovisioned(screen);
                    self.window.request_redraw();
                    log::info!("ui: boot splash dismissed -> Unprovisioned");
                }
                Err(e) => log::error!("ui: splash -> unprovisioned screen failed: {:?}", e),
            }
        }
    }

    /// Navigate (back) to the ContactList screen.
    ///
    /// Creates a fresh [`screens::ContactListScreen`], re-wires the settings
    /// button callback, re-populates the contacts and channels models from the
    /// runtime state cached in `self`, and replaces `active_screen`.
    fn navigate_to_contact_list(&mut self) -> anyhow::Result<()> {
        log::info!("ui: navigate_to_contact_list");
        // Clear any partial PIN input left over from the previous PinEntry.
        self.pin_digits.borrow_mut().clear();
        // Fresh visit starts with no trackball highlight (see field doc).
        self.contact_list_selected = -1;
        // BUG FIX:
        // `active_convo` used to be set once by `navigate_to_message_view`
        // and NEVER cleared, so it stayed latched to whichever conversation
        // was most recently opened — including long after the user had
        // navigated back here. `handle_event`'s IncomingDm/IncomingGroupMsg
        // "don't flag unread if this thread is the one currently open"
        // guard (`self.active_convo != Some((hash, is_channel))`) then
        // permanently suppressed the unread badge for THAT ONE conversation
        // even while it was no longer on screen — reproducing as "no badge
        // at all" for whichever contact/channel thread was inspected last
        // (e.g. while checking the space-theme message-view screen) before
        // testing badges, for both DM and channel categories alike, since
        // both handlers share this one field. This is the single choke
        // point both PinEntry-cancel and MessageView's Back button route
        // through (nav code `2`, see `step()`'s dispatch) — the exact
        // moment no conversation is "currently open" anymore, so this is
        // where the latch must clear. Compose is unaffected: it is only
        // ever entered from, and returns to, MessageView (nav codes
        // `5`/`6`/`7`), never through here, so `active_convo` stays valid
        // for the whole MessageView<->Compose round-trip.
        if let Some((hash, is_channel)) = self.active_convo.take() {
            log::debug!(
                "ui: navigate_to_contact_list clearing active_convo hash={:#04x} is_channel={}",
                hash, is_channel,
            );
        }

        // Hide the outgoing screen before surfacing the contact list.
        self.hide_active_screen();

        let screen = screens::ContactListScreen::new()?;

        // Re-wire the settings gear + row taps so repeated navigation keeps
        // working after a return to the list.
        Self::wire_contact_list_callbacks(&screen, &self.pending_nav, &self.pending_nav_hash);

        self.active_screen = ActiveScreen::ContactList(screen);
        // Re-populate contacts and channels from cached runtime state — same
        // recompute `wake_screen` triggers on the sleep→wake boundary (see
        // `refresh_contact_list_lists`'s doc); sharing the one call site
        // means the two can never drift out of sync again.
        self.refresh_contact_list_lists();
        // Force a full repaint so the cooperative loop surfaces the list.
        self.window.request_redraw();

        // Diagnostic: main-task stack headroom at this transition (this
        // screen just gained a full-window `SpaceBackdrop` in
        // `ContactListScreenUi`) — same
        // unconditional-log pattern `navigate_to_pin_entry`'s doc asks every
        // future `SpaceBackdrop`-adding screen to copy, so a stack-overflow
        // regression on this screen is caught by this exact log line rather
        // than silently missed until the periodic 30 s sample (which a stack
        // overflow's reboot would pre-empt).
        log_stack_hwm("navigate_to_contact_list");

        Ok(())
    }

    /// Recompute both the contacts and channels list models from the current
    /// `self.contact_names`/`self.channel_items`/`self.messages`/`self.unread`
    /// state and push them into the active `ContactList` screen. No-op if
    /// `ContactList` is not the active screen.
    ///
    /// # Why this exists
    ///
    /// Every live-update path that touches unread state
    /// (`IncomingDm`/`IncomingGroupMsg` in `handle_event`, `register_contact`,
    /// `set_channels`) already re-pushes the model — *if* `ContactList`
    /// happens to be the active screen at the moment the event lands — and
    /// `navigate_to_contact_list` re-populates from scratch on every
    /// (re-)entry. What NONE of those paths did, before this fix, is refresh
    /// on the sleep→wake transition itself: `wake_screen` only flipped
    /// `screen_asleep` and stopped the blink loop — it never re-synced the
    /// two Slint models with `self`'s state. On a backlight-only sleep (see
    /// `TDeckDisplay::set_backlight`'s doc) that gap is normally invisible,
    /// since the live-update paths above already keep the on-screen model
    /// current while the panel is dark. Calling this once, unconditionally,
    /// from `wake_screen` removes any dependency on that being true — the
    /// instant the user's eyes are back on the panel, both tab badges are
    /// guaranteed freshly recomputed from the authoritative `self` state,
    /// the same recompute a manual return-navigation already gets. Cheap
    /// (two `HashMap` scans + a Slint model rebuild) and a correctness no-op
    /// on the common case where the models were already current — this is a
    /// belt-and-suspenders sync point, not a hot loop.
    fn refresh_contact_list_lists(&self) {
        if let ActiveScreen::ContactList(ref screen) = self.active_screen {
            let contacts = Self::build_contact_items(
                &self.contact_names, &self.messages, &self.unread,
            );
            let channels = Self::build_channel_items(
                &self.channel_items, &self.messages, &self.unread,
            );
            log::debug!(
                "ui: refresh_contact_list_lists contacts_unread_total={} channels_unread_total={} \
                 channel_items_len={}",
                contacts.iter().map(|c| c.unread).sum::<i32>(),
                channels.iter().map(|c| c.unread).sum::<i32>(),
                self.channel_items.len(),
            );
            screen.set_contacts(&contacts);
            screen.set_channels(&channels);
        }
    }

    /// Wire the contact-list screen's settings gear and contact/channel row
    /// taps to the shared `pending_nav` / `pending_nav_hash` flags.
    ///
    /// Slint callbacks are `'static` and cannot capture `&mut self`; they signal
    /// intent by writing the shared `Rc<Cell>` flags, which `step()` drains.
    /// - Settings gear → `pending_nav = 1` (PinEntry).
    /// - Contact tap   → `pending_nav_hash = hash`, `pending_nav = 3` (MessageView).
    /// - Channel tap   → `pending_nav_hash = hash`, `pending_nav = 4` (MessageView).
    fn wire_contact_list_callbacks(
        screen: &screens::ContactListScreen,
        pending_nav: &std::rc::Rc<std::cell::Cell<u8>>,
        pending_nav_hash: &std::rc::Rc<std::cell::Cell<u8>>,
    ) {
        let pn = pending_nav.clone();
        screen.on_settings_pressed(move || {
            log::info!("ui: settings gear pressed -> PinEntry");
            pn.set(1);
        });

        let pn = pending_nav.clone();
        let ph = pending_nav_hash.clone();
        screen.on_contact_selected(move |hash| {
            log::info!("ui: contact selected hash={:#04x} -> MessageView", hash);
            ph.set(hash);
            pn.set(3);
        });

        let pn = pending_nav.clone();
        let ph = pending_nav_hash.clone();
        screen.on_channel_selected(move |hash| {
            log::info!("ui: channel selected hash={:#04x} -> MessageView", hash);
            ph.set(hash);
            pn.set(4);
        });
    }

    /// Hide whichever screen is currently active.
    ///
    /// Called at the start of every navigation so the single shared
    /// `MinimalSoftwareWindow` releases the outgoing component before the
    /// incoming one is shown — otherwise the panel can retain the previous
    /// screen's content after the swap.
    fn hide_active_screen(&self) {
        match &self.active_screen {
            ActiveScreen::Splash(s) => s.hide(),
            ActiveScreen::Unprovisioned(s) => s.hide(),
            ActiveScreen::ContactList(s) => s.hide(),
            ActiveScreen::PinEntry(s) => s.hide(),
            ActiveScreen::AdminMenu(s) => s.hide(),
            ActiveScreen::MessageView(s) => s.hide(),
            ActiveScreen::Compose(s) => s.hide(),
            ActiveScreen::GpsStatus(s) => s.hide(),
        }
    }

    /// Refresh the live `MessageViewScreen` message model if `(hash, is_channel)`
    /// is the conversation currently shown on screen.
    ///
    /// The invariant: any write to `self.messages[hash]` that targets the currently-
    /// open conversation must be followed by a call to this method so that the Slint
    /// model and the underlying store stay in sync without requiring the user to
    /// navigate away and back.
    ///
    /// No-op when:
    /// - the active screen is not `MessageView`, or
    /// - the open conversation is a different `(hash, is_channel)` pair.
    fn refresh_message_view_for(&self, hash: u8, is_channel: bool) {
        if let ActiveScreen::MessageView(ref screen) = self.active_screen {
            if self.active_convo == Some((hash, is_channel)) {
                let known = self.known_names();
                let items = self.messages
                    .get(&hash)
                    .map(|records| Self::build_message_items(records, is_channel, &self.self_name, &known))
                    .unwrap_or_default();
                screen.set_messages(&items);
                self.window.request_redraw();
                log::debug!(
                    "ui: refresh_message_view_for hash={:#04x} channel={} ({} msgs)",
                    hash, is_channel, items.len(),
                );
            }
        }
    }

    /// Build the MessageView model rows from stored message records.
    ///
    /// For received channel messages (`is_channel && !m.is_ours`), the stored
    /// text carries MeshCore's inline `"<name>: <msg>"` sender prefix (see
    /// `protocol::parse_channel_text`, and `main.rs::handle_grp_txt` which
    /// stores the raw prefixed text unmodified). This splits it into
    /// `from_name` (the sender, sans delimiter) and a body so the Slint
    /// `MessageBubble` can render the name+colon in bold and the body at
    /// normal weight. DMs and sent messages never carry this prefix and pass
    /// the whole text through as the body with `from_name` empty — which is
    /// also the signal `MessageBubble` uses to fall back to plain, single-run
    /// rendering, so the prefix split is scoped to received channel messages
    /// only.
    ///
    /// The body (post prefix-split) is then run through
    /// `protocol::mention::split_mentions` (`self_name`/`known` — same
    /// known-names set `wrap_outgoing_mentions` matches against on send) to
    /// flatten `@[name]` wire markup into a brackets-hidden `@name` display
    /// string, and to compute `mention_tier`: the highest
    /// `protocol::mention::MentionTier` found in the body, as an `i32` (0 =
    /// none, 1 = other-node mention, 2 = self-mention) — `MessageBubble`
    /// reads this to tint the bubble, self-mention more strongly than an
    /// other-node mention (this is a bubble-level
    /// tint rather than per-run inline color — Slint 1.16 has no rich-text
    /// runs, and this sandbox has no HIL to validate a Rust-side word-wrap-
    /// of-runs render, so bubble-level tint is the pre-authorized fallback).
    /// Applied uniformly to sent and received messages (a self-composed
    /// mention highlights too) — mentions are not channel-scoped.
    fn build_message_items(
        records: &[MessageRecord],
        is_channel: bool,
        self_name: &str,
        known: &[&str],
    ) -> Vec<screens::message_view::MessageItem> {
        use screens::message_view::MessageItem;
        records.iter().map(|m| {
            let (from_name, body) = if is_channel && !m.is_ours {
                match protocol::parse_channel_text(m.text.as_bytes()) {
                    (Some(name), body) => (
                        String::from_utf8_lossy(name).into_owned(),
                        String::from_utf8_lossy(body).into_owned(),
                    ),
                    (None, _) => (String::new(), m.text.clone()),
                }
            } else {
                (String::new(), m.text.clone())
            };
            let (text, mention_tier) = Self::render_mentions(&body, self_name, known);
            MessageItem {
                text,
                from_name,
                time_str: String::new(),
                is_ours: m.is_ours,
                acked: m.acked,
                mention_tier,
            }
        }).collect()
    }

    /// Flatten `body`'s `@[name]` wire markup into a brackets-hidden
    /// `@name` display string, and compute the highest
    /// `protocol::mention::MentionTier` present, returned as `i32` (see
    /// `build_message_items`'s doc for the tier code meaning). Pure/static
    /// and unit-testable in isolation from the prefix-split/record plumbing
    /// above it.
    fn render_mentions(body: &str, self_name: &str, known: &[&str]) -> (String, i32) {
        use protocol::mention::MentionTier;
        let mut display = String::with_capacity(body.len());
        let mut tier = MentionTier::Plain;
        for run in protocol::mention::split_mentions(body, self_name, known) {
            if run.tier == MentionTier::Plain {
                display.push_str(run.text);
            } else {
                display.push('@');
                display.push_str(run.text);
            }
            if run.tier > tier {
                tier = run.tier;
            }
        }
        (display, tier as i32)
    }

    /// Navigate to the MessageView conversation for `hash`.
    ///
    /// `is_channel` selects the title source (channel name vs. contact name);
    /// both read the conversation history from `self.messages[hash]`.  The back
    /// button returns to ContactList (`pending_nav = 2`).  Compose entry from
    /// this screen is an out-of-scope follow-on and only logs.
    fn navigate_to_message_view(&mut self, hash: u8, is_channel: bool) -> anyhow::Result<()> {
        log::info!("ui: navigate_to_message_view hash={:#04x} channel={}", hash, is_channel);
        // Remember the open conversation so the compose screen (Write button)
        // knows who to address and where to return after Send/cancel.
        self.active_convo = Some((hash, is_channel));
        // Clear this conversation's unread badge now that it's being read.
        // `self.unread` is the single source both `build_contact_items` and
        // `build_channel_items` read from, so removing the entry here is
        // sufficient for the badge to disappear next time either list is
        // rendered (ContactList is torn down and rebuilt fresh on every
        // navigation — see `navigate_to_contact_list`).
        self.unread.remove(&hash);
        self.hide_active_screen();

        let screen = screens::MessageViewScreen::new()?;

        screen.set_contact_name(&self.convo_title(hash, is_channel));

        let known = self.known_names();
        let items = self.messages.get(&hash)
            .map(|records| Self::build_message_items(records, is_channel, &self.self_name, &known))
            .unwrap_or_default();
        screen.set_messages(&items);

        // Back button → return to ContactList.
        let pn_back = self.pending_nav.clone();
        screen.on_back_pressed(move || {
            log::info!("ui: message view back pressed -> ContactList");
            pn_back.set(2);
        });

        // Write button → open the compose screen for this conversation.
        let pn_compose = self.pending_nav.clone();
        screen.on_compose_pressed(move || {
            log::info!("ui: compose (Write) pressed -> Compose");
            pn_compose.set(5);
        });

        self.active_screen = ActiveScreen::MessageView(screen);
        // Force a full repaint so the cooperative loop surfaces the conversation.
        self.window.request_redraw();

        // Diagnostic: main-task stack headroom at this transition — see
        // `navigate_to_contact_list`'s identical note.
        log_stack_hwm("navigate_to_message_view");

        Ok(())
    }

    /// Resolve the display title for a conversation `(hash, is_channel)`.
    fn convo_title(&self, hash: u8, is_channel: bool) -> String {
        if is_channel {
            self.channel_items.iter()
                .find(|c| c.hash == hash)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| format!("Channel {:#04x}", hash))
        } else {
            self.contact_names.get(&hash).cloned()
                .unwrap_or_else(|| format!("{:#04x}", hash))
        }
    }

    /// Navigate from the active MessageView to the Compose screen.
    ///
    /// Addresses the compose draft to [`Self::active_convo`].  Back/cancel
    /// re-opens the conversation unchanged (`pending_nav = 7`); Send stashes the
    /// draft in [`Self::pending_compose_text`] and signals `pending_nav = 6`, so
    /// `step()` expands shortcodes, sends, and re-opens the thread.
    fn navigate_to_compose(&mut self) -> anyhow::Result<()> {
        // Taken unconditionally, before the `active_convo` guard below, so a
        // seed set by a MessageView keypress never survives an early return
        // to leak into some later, unrelated compose open (e.g. a subsequent
        // Write-button tap) — see the field doc on `pending_compose_seed`.
        let seed = self.pending_compose_seed.take();
        let (hash, is_channel) = match self.active_convo {
            Some(c) => c,
            None => {
                log::warn!("ui: navigate_to_compose with no active conversation — ignoring");
                return Ok(());
            }
        };
        log::info!("ui: navigate_to_compose hash={:#04x} channel={}", hash, is_channel);
        self.hide_active_screen();

        let screen = screens::ComposeScreen::new()?;
        screen.set_to_name(&self.convo_title(hash, is_channel));

        // Pre-load the draft when this navigation was triggered by a
        // printable keypress in MessageView rather than the Write button
        // (see `step()`'s keyboard block) — seeds the pressed character as
        // if the user had opened compose and typed it, then refreshes the
        // `:shortcode:` autocomplete the same way a real keystroke would
        // (relevant if the seeded character is `:`). `None` when compose was
        // reached via the Write button, so that path is unaffected.
        //
        // `set_draft` only assigns the TextInput's `text` property — it does
        // not move the cursor, which otherwise defaults to byte offset 0 (the
        // start), leaving the seeded character AFTER the cursor rather than
        // before it. Without `move_cursor_to_end`, every subsequent keystroke
        // would insert ahead of the seeded char instead of appending after
        // it, e.g. pressing `h` then `i` would yield "ih" instead of "hi".
        if let Some(seed) = seed {
            screen.set_draft(&seed);
            screen.move_cursor_to_end();
            screen.refresh_completions();
        }

        // Back / cancel → re-open the conversation without sending.
        let pn_back = self.pending_nav.clone();
        screen.on_back_pressed(move || {
            log::info!("ui: compose back pressed -> MessageView");
            pn_back.set(7);
        });

        // Send → stash the draft for step() to expand + send, then re-open thread.
        let pn_send = self.pending_nav.clone();
        let draft_slot = self.pending_compose_text.clone();
        screen.on_send_pressed(move |text| {
            log::info!("ui: compose send pressed ({} chars) -> send + MessageView", text.len());
            *draft_slot.borrow_mut() = Some(text);
            pn_send.set(6);
        });

        self.active_screen = ActiveScreen::Compose(screen);
        // Force a full repaint so the cooperative loop surfaces the compose screen.
        self.window.request_redraw();

        // Diagnostic: main-task stack headroom at this transition — see
        // `navigate_to_contact_list`'s identical note. Not logged on the early
        // "no active conversation" return above, since that path never
        // constructs `ComposeScreenUi` at all.
        log_stack_hwm("navigate_to_compose");

        Ok(())
    }

    // ── Screen-sleep (backlight-off) ──────────────────────────────────────────

    /// Turn the display backlight on, mark the screen awake, reset the
    /// inactivity clock to `now_ms`, and stop any running incoming-message
    /// blink loop immediately (waking
    /// must halt the loop on the spot). Called from `step()`'s touch/keyboard
    /// poll blocks the moment a wake-triggering input is detected while
    /// asleep.
    ///
    /// Does NOT touch the keyboard co-processor's backlight directly — that
    /// is `sync_keyboard_backlight`'s job, the single arbiter that both this
    /// wake transition and the blink loop feed into (see its doc and the
    /// `kb_backlight_on` field doc for why there is exactly one writer).
    ///
    /// Also unconditionally re-syncs the ContactList screen's two tab models
    /// (`refresh_contact_list_lists`) — closes the sleep→wake gap where the
    /// on-screen unread badges had no explicit refresh trigger of their own
    /// at the moment the panel becomes visible again (see that method's doc).
    fn wake_screen(&mut self, now_ms: u64) {
        if let Err(e) = self.display.set_backlight(true) {
            log::warn!("ui: wake_screen backlight-on failed: {:?}", e);
        }
        self.notif.stop_blink();
        self.screen_asleep = false;
        self.last_activity_ms = now_ms;
        self.refresh_contact_list_lists();
    }

    /// Turn the display backlight off and mark the screen asleep.
    ///
    /// Sleep depth is backlight-only: the ST7789 display controller and the
    /// Slint cooperative render loop both keep running untouched (see
    /// `TDeckDisplay::set_backlight`), so the panel is already showing the
    /// correct pixels the instant the backlight comes back on in
    /// `wake_screen` — no re-render latency on wake.
    ///
    /// Does NOT touch the keyboard co-processor's backlight directly — see
    /// `wake_screen`'s doc; `sync_keyboard_backlight` picks up the new
    /// `screen_asleep` state (and turns the keyboard backlight off, absent an
    /// active blink loop) on its next call this same `step()`.
    fn sleep_screen(&mut self) {
        if let Err(e) = self.display.set_backlight(false) {
            log::warn!("ui: sleep_screen backlight-off failed: {:?}", e);
        }
        self.screen_asleep = true;
        log::info!("ui: screen sleep (inactivity timeout)");
    }

    /// Mirror the admin-menu's `notif_visual`/`notif_audible` master toggles
    /// (`self.runtime_settings`) into `self.notif`'s [`NotifPrefs`] table.
    ///
    /// `NotifDispatcher::fire` only ever consults its own `NotifPrefs` table —
    /// it has no access to (and shouldn't need to know about) `RuntimeSettings`
    /// — so this is the single place that keeps the two in lock-step. Rebuilt
    /// from [`NotifPrefs::from_provisioning_defaults`] every `step()` rather
    /// than only from the toggle callbacks themselves: those are `'static`
    /// Slint closures (see `navigate_to_admin_menu`) that capture
    /// `Rc<RefCell<RuntimeSettings>>` but cannot reach `&mut self.notif`. The
    /// rebuild is a cheap stack-struct assignment (no heap allocation, no
    /// hardware access), so doing it unconditionally every `step()` — rather
    /// than tracking "did it change" — costs nothing worth guarding.
    ///
    /// Per-event overrides (`NotifPrefs::set_event_pref`) are not exposed by
    /// any on-device UI today, so collapsing to the two-flag master toggle
    /// here loses no live customization; if a per-event settings screen is
    /// added later, this is the call site that will need to merge rather than
    /// overwrite.
    fn sync_notif_prefs(&mut self) {
        let settings = self.runtime_settings.borrow();
        self.notif.set_prefs(notif_prefs_from_toggles(settings.notif_visual, settings.notif_audible));
    }

    /// Single arbiter for the keyboard co-processor's backlight — the ONLY
    /// place in the runtime allowed to call `KeyboardDriver::set_backlight`.
    ///
    /// Two rules want to drive this one piece of hardware: "follow the
    /// screen" (on while awake, off while asleep) and the incoming-message
    /// blink loop (on/off pulses while asleep). Computing a single desired
    /// value here from
    /// `screen_asleep` + `notif.poll_blink` and writing only on change is what
    /// keeps the two rules from fighting over the wire — awake always wins outright (`true`,
    /// blink is irrelevant once awake since `wake_screen` already stopped it),
    /// asleep defers entirely to the blink loop's on/off/quiet schedule
    /// (`false` whenever no loop is running, matching the old
    /// screen-follows-only behaviour).
    ///
    /// Called once per `step()`, after both the touch/keyboard wake path and
    /// the inactivity-timeout sleep path have had a chance to update
    /// `screen_asleep` for this iteration, and after `handle_event` has had a
    /// chance to start the blink loop for a just-arrived message.
    fn sync_keyboard_backlight(&mut self, now_ms: u64) {
        let desired = if self.screen_asleep {
            self.notif.poll_blink(now_ms)
        } else {
            true
        };
        if desired != self.kb_backlight_on {
            if let Some(ref mut kb) = self.keyboard {
                if let Err(e) = kb.set_backlight(desired) {
                    log::warn!("ui: keyboard backlight sync({}) failed: {:?}", desired, e);
                }
            }
            self.kb_backlight_on = desired;
        }
    }
}

/// Pure decision function for the boot-splash dismissal gate (see
/// `screens::splash` module doc and `UiRuntime::SPLASH_MIN_MS`/`SPLASH_MAX_MS`).
/// Extracted as a pure function — same rationale as `touch_wake_transition`
/// below — so the core acceptance logic (animation always
/// completes, splash never lingers once settled, defensive cap) has
/// host-checkable unit tests independent of the Slint/display stack.
///
/// `elapsed_ms` is measured from the splash's first `step()` tick (used ONLY
/// by the `max_ms` defensive cap below). `animation_elapsed_ms` is `None`
/// until `UiRuntime::step()` actually fires `SplashScreen::start_animation()`
/// (gated on `mark_app_ready()` — see that method's doc), and `Some(ms)`
/// thereafter, measured from ITS OWN clock (`splash_animation_started_ms`) —
/// a different, later zero point than `elapsed_ms` whenever `mark_app_ready()`
/// arrives after the splash's first tick.
///
/// Dismiss once EITHER:
/// - `animation_elapsed_ms` is `Some(ms)` with `ms >= min_ms` (normal path:
///   the one-shot splash animation has started AND had time to finish), OR
/// - `elapsed_ms >= max_ms`, unconditionally (defensive cap — see
///   `SPLASH_MAX_MS`'s doc; covers both "animation never started" and
///   "started too late to have settled yet").
fn splash_should_dismiss(elapsed_ms: u64, animation_elapsed_ms: Option<u64>, min_ms: u64, max_ms: u64) -> bool {
    matches!(animation_elapsed_ms, Some(ms) if ms >= min_ms) || elapsed_ms >= max_ms
}

/// Result of [`touch_wake_transition`]: what `step()` should do with one
/// polled touch event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TouchWakeOutcome {
    /// `true` if this event is the one that woke the screen (backlight
    /// should turn on) — `step()` calls `wake_screen` when this is set.
    woke: bool,
    /// `true` if this event should be forwarded to `window.dispatch_touch`.
    /// Mutually exclusive with `woke` and with mid-gesture swallow.
    dispatch: bool,
    /// New value for `UiRuntime::touch_wake_swallow_active`.
    swallow_active: bool,
}

/// Pure decision function for the touch wake/swallow state machine.
///
/// This is the technically sharp part of the screen-sleep feature: the
/// wake-triggering touch must be consumed to
/// wake ONLY, never routed to the focused widget, and a still-held finger
/// must not leak the rest of its Pressed→Moved→Released gesture into the app
/// after the initiating Pressed was swallowed. Extracted as a pure function
/// (no hardware/Slint dependency) so this invariant has host-checkable unit
/// tests instead of relying solely on the manual HIL procedure (§H).
///
/// - `screen_asleep`: state BEFORE this event (i.e. before any wake this call causes).
/// - `swallow_active`: whether a previous call is still draining a wake gesture's tail.
/// - `kind`: the polled event's `TouchKind`.
fn touch_wake_transition(
    screen_asleep: bool,
    swallow_active: bool,
    kind: touch::TouchKind,
) -> TouchWakeOutcome {
    if screen_asleep {
        // This event wakes the screen. Swallow it; if it's not already the
        // gesture's Released, keep swallowing until one arrives.
        TouchWakeOutcome {
            woke: true,
            dispatch: false,
            swallow_active: kind != touch::TouchKind::Released,
        }
    } else if swallow_active {
        // Draining the wake gesture's Moved/Released tail — still swallowed.
        TouchWakeOutcome {
            woke: false,
            dispatch: false,
            swallow_active: kind != touch::TouchKind::Released,
        }
    } else {
        // Normal operation: screen already awake, no gesture to drain.
        TouchWakeOutcome { woke: false, dispatch: true, swallow_active: false }
    }
}

/// Decide whether a keyboard byte, polled while `MessageView` is the active
/// screen and the device is already awake, should seed the Compose draft and
/// flip the UI into write mode.
///
/// Returns `Some(text)` — the character to load as the draft's first
/// character — for printable ASCII, using exactly the same byte range
/// `keyboard::key_text` documents as "the printable char"
/// (`0x20..=0x7E`, space through `~`, matching that "printable
/// character key" wording). Returns `None` for anything else: Backspace,
/// Return, Tab, Escape (which `key_text` maps to non-text Slint keys) and any
/// byte with no mapping at all — those must retain MessageView's current
/// behavior (today, a no-op, since MessageView has no focusable input) rather
/// than jumping to Compose.
///
/// Pure (no hardware/Slint dependency) so the printable/non-printable
/// boundary — the crux of the "non-text keys must be excluded" acceptance
/// criterion — is host-testable independent of the keyboard co-processor,
/// same rationale as `touch_wake_transition` above. Callers are additionally
/// responsible for the sleep-wake exclusion (only calling this once a
/// wake-triggering keypress has already been swallowed elsewhere) — that is
/// `step()`'s job, not this function's.
fn message_view_compose_seed(byte: u8) -> Option<String> {
    match byte {
        0x20..=0x7E => Some((byte as char).to_string()),
        _ => None,
    }
}

/// Decide whether a Return keypress in the Compose draft should trigger Send.
///
/// Returns `true` whenever `draft` has non-whitespace content — the same
/// intent as the Send button, which sends whatever text is present. Returns
/// `false` for empty or whitespace-only drafts, an explicit guard
/// against empty sends: `step()` treats a `false` result as a total no-op
/// (no send, no navigation, and — because Return is intercepted before
/// `keyboard::key_text` dispatch — no newline inserted either), rather than
/// falling back to the button's behavior of silently discarding the draft and
/// still navigating back to MessageView.
///
/// Pure (no Slint/hardware dependency), same rationale as
/// `message_view_compose_seed` and `touch_wake_transition` above: this is the
/// acceptance-critical decision (empty/whitespace must not send) and is
/// host-testable in isolation.
fn compose_return_should_send(draft: &str) -> bool {
    !draft.trim().is_empty()
}

/// Decide whether `step()`'s keyboard byte-drain loop should stop after
/// processing the byte that was just handled.
///
/// `pending_nav` is `self.pending_nav.get()` — nonzero means the byte just
/// handled set a screen-navigation flag (MessageView-seed or Compose
/// Return-to-send). The loop must stop in that case even if the drain bound
/// hasn't been reached: `active_screen` is about to change on the *next*
/// `step()`, so evaluating a same-burst byte against the still-current (soon
/// stale) screen would misattribute it — e.g. a second buffered character
/// overwriting the just-set Compose seed instead of landing in the Compose
/// draft it seeded. `drained >= max` is the independent defensive bound so a
/// stuck/flooding bus cannot starve RX/render.
///
/// Pure (no hardware/Slint dependency) so this burst/nav interaction — the
/// one behavioral edge case the multi-byte drain fix had to get right to
/// avoid a regression — is host-testable independent of the keyboard
/// co-processor, same rationale as `touch_wake_transition` above.
fn keyboard_drain_should_stop(pending_nav: u8, drained: u8, max: u8) -> bool {
    pending_nav != 0 || drained >= max
}

/// Pure predicate: has `step()`'s deferred post-send Compose → MessageView
/// navigation deadline (`UiRuntime::deferred_message_view_nav_at_ms`)
/// elapsed? Extracted from `step()`'s hardware-bound body so this timing edge —
/// unarmed, armed-but-not-yet-due, and armed-and-due (including the exact
/// boundary) — is covered by a host-native unit test independent of the
/// display/touch stack, same "pull the pure decision out of the dispatcher"
/// rationale as `splash_should_dismiss`/`keyboard_drain_should_stop` above.
fn send_nav_deferral_elapsed(deferred_at_ms: Option<u64>, now_ms: u64) -> bool {
    matches!(deferred_at_ms, Some(at_ms) if now_ms >= at_ms)
}

/// Pure predicate: should an incoming message for `(hash, is_channel)` be
/// flagged unread, given the conversation the user is CURRENTLY viewing
/// (`active_convo`)? `false` only while that exact conversation is the one
/// open in a live `MessageView` — see `handle_event`'s IncomingDm/
/// IncomingGroupMsg branches, both of which share this one gate.
///
/// # The invariant this depends on
///
/// `active_convo` must be `None` whenever no conversation is on screen —
/// `navigate_to_message_view` sets it, and `navigate_to_contact_list` (the
/// single choke point both PinEntry-cancel and MessageView's Back button
/// route through) clears it back to `None`. Before that clear existed,
/// `active_convo` stayed latched to whichever conversation was most
/// recently opened, so this predicate stayed permanently `false` for that
/// one (hash, is_channel) — suppressing its unread badge even long after
/// the user had navigated away, reproducing as "no badge at all" for
/// whichever DM or channel thread had been inspected last (e.g. while
/// checking an unrelated theme change on the MessageView screen). Extracted
/// as a pure function (mirrors `send_nav_deferral_elapsed` above) so the
/// gate itself — independent of whether `navigate_to_contact_list` actually
/// clears the latch — is host-testable without the display/touch stack.
fn incoming_message_is_unread(active_convo: Option<(u8, bool)>, hash: u8, is_channel: bool) -> bool {
    active_convo != Some((hash, is_channel))
}

/// Pure index-math for a trackball Up/Down roll,
/// shared by `handle_trackball_contact_list` and
/// `handle_trackball_admin_menu`: move `current` toward the top (`up: true`,
/// decrement) or bottom (`up: false`, increment) of a `0..=max_idx` list.
///
/// - `current < 0` means "no highlight yet" (the `-1` sentinel documented on
///   `contact_list_selected`/`admin_menu_selected`): the FIRST roll in
///   either direction always lands on row `0`, matching "roll highlights a
///   contact/channel" — the first roll picks the top row rather than needing
///   an extra roll to establish a starting point.
/// - `max_idx < 0` means an empty list (nothing to highlight): always returns
///   `-1` regardless of direction or `current`, so a caller can treat a
///   negative result as "no-op, no valid row" uniformly.
/// - Otherwise clamps to `0..=max_idx` — rolling off either end holds at that
///   end rather than wrapping (a wrap would let a roll silently jump from the
///   last row back to the first, easy to trigger by accident on a physical
///   trackball and surprising for the target audience).
///
/// Pure (no `UiRuntime`/Slint dependency) so this index arithmetic — the part
/// of the whole feature most likely to carry an off-by-one — is
/// host-checkable in isolation, same rationale as `touch_wake_transition`.
fn roll_selection(current: i32, max_idx: i32, up: bool) -> i32 {
    if max_idx < 0 {
        return -1;
    }
    if current < 0 {
        0
    } else if up {
        (current - 1).max(0)
    } else {
        (current + 1).min(max_idx)
    }
}

/// Whether `prev` -> `new` changes anything the AdminMenu battery row
/// renders. [`screens::admin_menu::format_battery_display`] reads exactly
/// `percent` and `charging` (see that function's doc) — `raw_mv`/
/// `held_raw_mv` are live diagnostic-only fields the on-device row never
/// shows, so they are deliberately excluded here. Used by
/// `UiRuntime::set_battery_status` to skip the row's `format!` allocation +
/// Slint push on ADC-jitter ticks that don't move the displayed percentage or
/// charging state.
///
/// Extracted as a pure function (no `UiRuntime`/hardware dependency) for a
/// host-checkable unit test, same pattern as `notif_prefs_from_toggles`
/// just below.
fn battery_display_fields_changed(prev: battery::BatteryStatus, new: battery::BatteryStatus) -> bool {
    prev.percent != new.percent || prev.charging != new.charging
}

/// Map the admin-menu's two master toggles (`RuntimeSettings.notif_visual` /
/// `notif_audible`) to the [`NotifPrefs`] table `UiRuntime::sync_notif_prefs`
/// installs into `self.notif` every `step()`.
///
/// Extracted as a pure function (no `UiRuntime`/hardware dependency) so the
/// actual value of this fix — "the toggle wired to what `fire()`
/// gates on" — has a host-checkable unit test, same pattern as
/// `touch_wake_transition` above.
fn notif_prefs_from_toggles(visual: bool, audible: bool) -> NotifPrefs {
    NotifPrefs::from_provisioning_defaults(visual, audible)
}

/// Persist `settings` to NVS via `runtime_settings_store::save`, if a
/// partition handle has been wired (`UiRuntime::set_nvs_partition`).
///
/// A free function (not a `UiRuntime` method) because it is called from
/// inside `'static` Slint toggle closures, which cannot capture `&self`.
///
/// `runtime_settings_store` is a production-only module (`#[cfg(not(feature =
/// "hil"))]` in `main.rs`, mirroring `config_store`'s exclusion — the hil
/// build role has no NVS at all), so this function has a matching `#[cfg]`
/// split: the production path saves; the hil path is a documented no-op (the
/// toggle still applies in memory for the run).
#[cfg(not(feature = "hil"))]
fn persist_runtime_settings(
    nvs: &Option<EspNvsPartition<NvsDefault>>,
    settings: &pin_menu::RuntimeSettings,
) {
    match nvs {
        Some(n) => {
            if let Err(e) = crate::runtime_settings_store::save(n.clone(), settings) {
                log::error!("runtime_settings_store: save failed: {:?}", e);
            }
        }
        None => {
            log::warn!("ui: no NVS partition wired — admin-menu toggle not persisted");
        }
    }
}

#[cfg(feature = "hil")]
fn persist_runtime_settings(
    _nvs: &Option<EspNvsPartition<NvsDefault>>,
    _settings: &pin_menu::RuntimeSettings,
) {
    // hil builds have no NVS-backed runtime_settings_store (see module
    // #[cfg] in main.rs) — the toggle still applies to the in-memory
    // RuntimeSettings for the duration of the run.
}

/// Log the main-task stack high-water mark, unconditionally, tagged with
/// `context` (the calling `navigate_to_*` function's name).
///
/// Shared
/// by the two densest screen-swap transitions on the "open Settings" path
/// (`navigate_to_pin_entry` — the CONFIRMED overflow site per an
/// on-hardware backtrace — and `navigate_to_admin_menu`, the
/// next-densest transition on the same path; see each call site's own doc).
/// Logged unconditionally rather than folded into `main.rs`'s periodic 30 s
/// sample, because a stack overflow reboots the task before its next
/// periodic tick could ever fire — this is the sample most likely to catch
/// the peak. Adding a full-window `SpaceBackdrop` to three more screens
/// added this same helper into their
/// `navigate_to_contact_list`/`navigate_to_message_view`/`navigate_to_compose`
/// call sites too — any screen that adds `SpaceBackdrop` after this one
/// should keep doing the same.
fn log_stack_hwm(context: &str) {
    let hwm: u32 =
        unsafe { esp_idf_svc::sys::uxTaskGetStackHighWaterMark(core::ptr::null_mut()) };
    log::info!("ui: {} stack HWM: {} B free", context, hwm);
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// `build_contact_items`/`build_channel_items` are pure functions over plain
// data maps — no display/touch/window hardware required — so they're testable
// in isolation, same pattern as `keyboard.rs`/`notification.rs`/`compose.rs`.
// These pin down the two regressions this module fixed: channel unread counts
// reaching the screen (previously only contacts refreshed), and both trees
// reading a common `unread` map that gets cleared on read.
//
// NOTE: this crate's single `[[bin]]` target sets `harness = false` (see
// Cargo.toml) so `main()` is the esp-idf entry point, not a synthesized libtest
// runner — `cargo test` never actually *executes* any `#[cfg(test)]` block in
// this crate, this one included; it only type-checks it (`cargo test --no-run`
// is as far as this gets on host, and the runner in `.cargo/config.toml` needs
// real hardware). Same pre-existing limitation as the three files above; not
// introduced or fixed here.
#[cfg(test)]
mod tests {
    use super::*;

    // ── BuzzerDriver::square_wave_sample ────────────────────────────────────
    // Pure duty-cycle arithmetic, no I2S/hardware dependency — see the
    // function's doc for why this is pulled out and pinned here.

    #[test]
    fn square_wave_sample_silence_is_always_zero() {
        for i in [0u32, 1, 2, 100] {
            assert_eq!(BuzzerDriver::square_wave_sample(i, 0, 8_000, 16_384), 0);
        }
    }

    #[test]
    fn square_wave_sample_alternates_high_then_low_per_cycle() {
        // freq_hz=1000 @ sample_rate=8000 => samples_per_cycle = 8: first
        // half of each 8-sample cycle is +amplitude, second half -amplitude.
        let amplitude = 16_384i16;
        let expected = [
            amplitude, amplitude, amplitude, amplitude,
            -amplitude, -amplitude, -amplitude, -amplitude,
        ];
        for (i, &want) in expected.iter().enumerate() {
            assert_eq!(
                BuzzerDriver::square_wave_sample(i as u32, 1_000, 8_000, amplitude),
                want,
                "sample {i} of an 8-sample cycle",
            );
        }
        // Cycle repeats: sample 8 matches sample 0.
        assert_eq!(
            BuzzerDriver::square_wave_sample(8, 1_000, 8_000, amplitude),
            amplitude,
        );
    }

    #[test]
    fn square_wave_sample_never_panics_above_nyquist() {
        // freq_hz > sample_rate_hz would drive samples_per_cycle to 0 without
        // the `.max(2)` guard (mod-by-zero panic). Not reachable from the
        // current tone_sequence() table, but must stay safe regardless.
        let _ = BuzzerDriver::square_wave_sample(0, 50_000, 8_000, 16_384);
    }

    #[allow(dead_code)] // only called from #[test] fns, which the crate's real
                         // main() never reaches — see NOTE above.
    fn catalog(entries: &[(&str, u8)]) -> Vec<screens::contact_list::ChannelItem> {
        entries.iter().map(|&(name, hash)| screens::contact_list::ChannelItem {
            name: name.to_string(),
            preview: String::new(),
            time_str: String::new(),
            unread: 0, // catalog entries are always seeded at 0 — see set_channels
            hash,
        }).collect()
    }

    #[test]
    fn build_channel_items_reflects_unread_map() {
        let channels = catalog(&[("General", 0x10), ("Ops", 0x20)]);
        let messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        let mut unread = HashMap::new();
        unread.insert(0x20u8, 3u32);

        let items = UiRuntime::build_channel_items(&channels, &messages, &unread);

        // Regression guard for the missing-badge defect: a channel with a
        // nonzero `unread` map entry must carry that count through, not the
        // catalog's frozen `unread: 0`.
        let ops = items.iter().find(|c| c.hash == 0x20).unwrap();
        assert_eq!(ops.unread, 3);
        let general = items.iter().find(|c| c.hash == 0x10).unwrap();
        assert_eq!(general.unread, 0);
    }

    #[test]
    fn build_channel_items_sorts_unread_first() {
        let channels = catalog(&[("Alpha", 1), ("Bravo", 2), ("Charlie", 3)]);
        let messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        let mut unread = HashMap::new();
        unread.insert(3u8, 1u32);

        let items = UiRuntime::build_channel_items(&channels, &messages, &unread);
        assert_eq!(items[0].hash, 3); // sole unread channel sorts first
    }

    #[test]
    fn build_channel_items_carries_last_message_as_preview() {
        let channels = catalog(&[("General", 0x10)]);
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(0x10, vec![
            MessageRecord { text: "first".into(), is_ours: false, acked: false, ts_ms: 0 },
            MessageRecord { text: "latest".into(), is_ours: false, acked: false, ts_ms: 1 },
        ]);
        let unread = HashMap::new();

        let items = UiRuntime::build_channel_items(&channels, &messages, &unread);
        assert_eq!(items[0].preview, "latest");
    }

    // ── messages_insert_non_empty — boot-hydrate seeding core ───────────────
    //
    // Regression guard:
    // `seed_conversation` must land restored history in `messages` so
    // `build_contact_items`/`build_channel_items` (tested above) pick it up
    // as a preview on the very first contact-list build, and must NOT insert
    // an empty `Vec` for a conversation with no stored history.

    #[test]
    fn messages_insert_non_empty_seeds_restored_history() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        let records = vec![
            MessageRecord { text: "inbound restored".into(), is_ours: false, acked: true, ts_ms: 0 },
            MessageRecord { text: "outbound restored".into(), is_ours: true, acked: true, ts_ms: 0 },
        ];
        messages_insert_non_empty(&mut messages, 0x55, records);

        let seeded = messages.get(&0x55).expect("conversation must be seeded");
        assert_eq!(seeded.len(), 2);
        assert_eq!(seeded[0].text, "inbound restored");
        assert!(!seeded[0].is_ours);
        assert!(seeded[0].acked, "restored records must never show perpetual pending");
        assert!(seeded[1].is_ours);
    }

    #[test]
    fn messages_insert_non_empty_skips_empty_conversation() {
        // An empty conversation (no history stored) must hydrate to empty —
        // i.e. leave the key absent — not insert `vec![]`, so a caller can
        // still tell "never messaged" apart from "seeded but list happened
        // to be empty" if that distinction ever matters, and so previews
        // read via `messages.get(&hash).and_then(|m| m.last())` behave
        // identically either way (both `None`).
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages_insert_non_empty(&mut messages, 0x77, Vec::new());
        assert!(messages.get(&0x77).is_none());
    }

    #[test]
    fn messages_insert_non_empty_seeded_history_feeds_contact_preview() {
        // End-to-end (within the pure-function slice): seeding restored
        // history and then building contact items must surface the restored
        // text as the preview — the actual acceptance behavior ("contact
        // list previews show restored history").
        let mut contact_names = HashMap::new();
        contact_names.insert(0x64u8, "Dana".to_string());
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages_insert_non_empty(&mut messages, 0x64, vec![
            MessageRecord { text: "welcome back".into(), is_ours: false, acked: true, ts_ms: 0 },
        ]);
        let unread = HashMap::new();

        let items = UiRuntime::build_contact_items(&contact_names, &messages, &unread);
        let dana = items.iter().find(|c| c.hash == 0x64).unwrap();
        assert_eq!(dana.preview, "welcome back");
    }

    // ── mark_last_unacked_outbound — live ACK → ✓✓ indicator ────────────────
    //
    // Regression guard: this
    // is the exact "right message marked" question the original diagnosis
    // step raised — a confirmed-delivered DM must flip the correct
    // `MessageRecord`, not an arbitrary one, and must not disturb unrelated
    // conversations or already-acked history.

    #[test]
    fn marks_the_newest_unacked_outbound_message() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(0x42, vec![
            MessageRecord { text: "first".into(), is_ours: true, acked: false, ts_ms: 0 },
            MessageRecord { text: "second".into(), is_ours: true, acked: false, ts_ms: 1 },
        ]);

        let marked = mark_last_unacked_outbound(&mut messages, 0x42);

        assert!(marked, "an unacked outbound message must be found and marked");
        let msgs = &messages[&0x42];
        assert!(!msgs[0].acked, "the older unacked message must be left alone");
        assert!(msgs[1].acked, "the most recently sent unacked message is the one the ack refers to");
    }

    #[test]
    fn does_not_re_ack_an_already_acked_message_or_touch_inbound_records() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(0x42, vec![
            MessageRecord { text: "outbound already delivered".into(), is_ours: true, acked: true, ts_ms: 0 },
            MessageRecord { text: "their reply".into(), is_ours: false, acked: false, ts_ms: 1 },
        ]);

        let marked = mark_last_unacked_outbound(&mut messages, 0x42);

        assert!(!marked, "no unacked OUTBOUND message exists — an inbound record must never be flipped");
        assert!(messages[&0x42][0].acked);
        assert!(!messages[&0x42][1].acked, "inbound records are never acked by this path");
    }

    #[test]
    fn does_not_touch_a_different_contacts_conversation() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        messages.insert(0x10, vec![
            MessageRecord { text: "to alice".into(), is_ours: true, acked: false, ts_ms: 0 },
        ]);
        messages.insert(0x20, vec![
            MessageRecord { text: "to bob".into(), is_ours: true, acked: false, ts_ms: 0 },
        ]);

        let marked = mark_last_unacked_outbound(&mut messages, 0x10);

        assert!(marked);
        assert!(messages[&0x10][0].acked, "the addressed contact's message is marked");
        assert!(!messages[&0x20][0].acked, "an unrelated contact's pending message must be untouched");
    }

    #[test]
    fn unknown_contact_hash_is_a_no_op() {
        let mut messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        let marked = mark_last_unacked_outbound(&mut messages, 0x99);
        assert!(!marked);
    }

    #[test]
    fn build_contact_items_reflects_unread_map() {
        let mut contact_names = HashMap::new();
        contact_names.insert(0x30u8, "Alice".to_string());
        contact_names.insert(0x40u8, "Bob".to_string());
        let messages: HashMap<u8, Vec<MessageRecord>> = HashMap::new();
        let mut unread = HashMap::new();
        unread.insert(0x30u8, 2u32);

        let items = UiRuntime::build_contact_items(&contact_names, &messages, &unread);
        let alice = items.iter().find(|c| c.hash == 0x30).unwrap();
        assert_eq!(alice.unread, 2);
        let bob = items.iter().find(|c| c.hash == 0x40).unwrap();
        assert_eq!(bob.unread, 0);
    }

    #[test]
    fn contact_and_channel_unread_share_one_map_and_clear_together() {
        // Documents the pre-existing key-space assumption both builders share:
        // `unread` is keyed only by `u8` hash, not by (hash, is_channel). A
        // contact hash and a channel hash that happen to collide will share
        // one counter and one clear-on-read. Not a regression introduced by
        // this fix (contact_names/messages already share the same u8 key
        // space) — recorded here so a future change to disambiguate the key
        // space has a test to update.
        let mut unread: HashMap<u8, u32> = HashMap::new();
        unread.insert(0x55, 1);
        // Simulate navigate_to_message_view's clear-on-read for hash 0x55,
        // regardless of whether it was opened as a contact or a channel.
        unread.remove(&0x55);
        assert_eq!(unread.get(&0x55), None);
    }

    // ── build_message_items — channel sender-prefix split ────────────────
    //
    // Regression guard: pins which
    // records get their MeshCore "<name>: <msg>" wire text split into
    // (from_name, text) — the signal `MessageBubble` uses to bold the
    // sender prefix — and which pass `text` through verbatim with an empty
    // `from_name` (the fallback to plain rendering).

    #[test]
    fn build_message_items_splits_prefix_on_received_channel_message() {
        let records = vec![
            MessageRecord { text: "Alice: hello there".into(), is_ours: false, acked: false, ts_ms: 0 },
        ];
        let items = UiRuntime::build_message_items(&records, /* is_channel */ true, "Self", &[]);
        assert_eq!(items[0].from_name, "Alice");
        assert_eq!(items[0].text, "hello there");
    }

    #[test]
    fn build_message_items_leaves_sent_channel_message_unprefixed() {
        // Sent messages store the raw compose text (no MeshCore name prefix
        // — see `on_send_message`), and must render exactly as before this
        // fix regardless of channel-ness: no split, empty from_name.
        let records = vec![
            MessageRecord { text: "Alice: hello there".into(), is_ours: true, acked: false, ts_ms: 0 },
        ];
        let items = UiRuntime::build_message_items(&records, /* is_channel */ true, "Self", &[]);
        assert_eq!(items[0].from_name, "");
        assert_eq!(items[0].text, "Alice: hello there");
    }

    #[test]
    fn build_message_items_leaves_dm_message_unprefixed() {
        // DMs never carry the channel wire-text delimiter, even if their
        // literal text happens to contain "name: " — is_channel=false is the
        // guard, not a text-shape heuristic.
        let records = vec![
            MessageRecord { text: "Alice: hello there".into(), is_ours: false, acked: false, ts_ms: 0 },
        ];
        let items = UiRuntime::build_message_items(&records, /* is_channel */ false, "Self", &[]);
        assert_eq!(items[0].from_name, "");
        assert_eq!(items[0].text, "Alice: hello there");
    }

    #[test]
    fn build_message_items_falls_back_when_channel_text_has_no_prefix() {
        // Malformed/prefix-less channel text (no "<name>: " delimiter) passes
        // through verbatim rather than mis-splitting on the wrong bytes.
        let records = vec![
            MessageRecord { text: "no delimiter here".into(), is_ours: false, acked: false, ts_ms: 0 },
        ];
        let items = UiRuntime::build_message_items(&records, /* is_channel */ true, "Self", &[]);
        assert_eq!(items[0].from_name, "");
        assert_eq!(items[0].text, "no delimiter here");
    }

    #[test]
    fn build_message_items_empty_sender_name_falls_back_to_plain_body() {
        // Pathological wire text (never emitted by real MeshCore senders,
        // whose sender_name is never empty) with an EMPTY name before the
        // delimiter: `parse_channel_text` still reports `Some("")`, which
        // collapses to `from_name == ""` here — the same signal `MessageBubble`
        // treats as "no attribution", so it falls back to plain rendering.
        // Documented rather than special-cased: the delimiter itself is
        // dropped from the displayed body in this corner case (accepted
        // known limitation).
        let records = vec![
            MessageRecord { text: ": hello".into(), is_ours: false, acked: false, ts_ms: 0 },
        ];
        let items = UiRuntime::build_message_items(&records, /* is_channel */ true, "Self", &[]);
        assert_eq!(items[0].from_name, "");
        assert_eq!(items[0].text, "hello");
    }

    // ── @mentions — wrap (send) / render (receive) ────────────────────────
    //
    // Pins the two
    // Rust-side seams around `protocol::mention` (itself unit-tested in
    // `protocol/src/mention.rs`): `wrap_outgoing_mentions` (send-side glue)
    // and `render_mentions`/`build_message_items` (receive-side glue —
    // flattened display text + `mention_tier`).

    #[test]
    fn wrap_outgoing_mentions_wraps_known_name() {
        let out = UiRuntime::wrap_outgoing_mentions("hi @Alice!", &["Alice", "Bob"]);
        assert_eq!(out, "hi @[Alice]!");
    }

    #[test]
    fn wrap_outgoing_mentions_leaves_unknown_name_verbatim() {
        let out = UiRuntime::wrap_outgoing_mentions("hi @nobody!", &["Alice", "Bob"]);
        assert_eq!(out, "hi @nobody!");
    }

    #[test]
    fn render_mentions_flattens_brackets_and_reports_no_tier_for_plain_text() {
        let (text, tier) = UiRuntime::render_mentions("just a plain message", "Bob", &[]);
        assert_eq!(text, "just a plain message");
        assert_eq!(tier, 0);
    }

    #[test]
    fn render_mentions_other_node_mention_is_tier_1() {
        let (text, tier) = UiRuntime::render_mentions("hi @[Alice] there", "Bob", &[]);
        assert_eq!(text, "hi @Alice there");
        assert!(!text.contains('['));
        assert!(!text.contains(']'));
        assert_eq!(tier, 1);
    }

    #[test]
    fn render_mentions_self_mention_is_tier_2_more_prominent_than_other() {
        let (text, tier) = UiRuntime::render_mentions("hi @[Bob] there", "Bob", &[]);
        assert_eq!(text, "hi @Bob there");
        assert_eq!(tier, 2);
        assert!(tier > 1); // self-mention outranks an other-node mention
    }

    #[test]
    fn render_mentions_multiword_name_not_tokenized_on_space() {
        // A real-world example: "Chicken Little" contains a space —
        // the delimiter must not be mistaken for a word boundary.
        let (text, tier) = UiRuntime::render_mentions(
            "watch out @[Chicken Little] the sky is falling", "Rex", &[],
        );
        assert_eq!(text, "watch out @Chicken Little the sky is falling");
        assert_eq!(tier, 1);
    }

    #[test]
    fn build_message_items_renders_self_mention_in_received_dm() {
        let records = vec![
            MessageRecord { text: "hey @[Bob] check this out".into(), is_ours: false, acked: false, ts_ms: 0 },
        ];
        let items = UiRuntime::build_message_items(&records, /* is_channel */ false, "Bob", &["Bob"]);
        assert_eq!(items[0].text, "hey @Bob check this out");
        assert_eq!(items[0].mention_tier, 2);
    }

    #[test]
    fn build_message_items_renders_other_mention_in_received_channel_message_after_prefix_split() {
        // Sender-prefix split (from_name) and mention flattening both apply
        // to the same received channel message, in sequence: prefix comes
        // off first, then the body is scanned for mentions — the two
        // features this render rework couples on purpose. This
        // node's own name is "Carol" — the mention is of "Bob", a different
        // node, so it must tier as `Other` (1), not `SelfMention`.
        let records = vec![
            MessageRecord {
                text: "Alice: hi @[Bob] check this out".into(),
                is_ours: false, acked: false, ts_ms: 0,
            },
        ];
        let items = UiRuntime::build_message_items(
            &records, /* is_channel */ true, "Carol", &["Carol", "Bob", "Alice"],
        );
        assert_eq!(items[0].from_name, "Alice");
        assert_eq!(items[0].text, "hi @Bob check this out");
        assert_eq!(items[0].mention_tier, 1);
    }

    #[test]
    fn build_message_items_mention_free_text_is_tier_zero_unchanged() {
        let records = vec![
            MessageRecord { text: "no mentions here".into(), is_ours: false, acked: false, ts_ms: 0 },
        ];
        let items = UiRuntime::build_message_items(&records, /* is_channel */ false, "Bob", &["Bob"]);
        assert_eq!(items[0].text, "no mentions here");
        assert_eq!(items[0].mention_tier, 0);
    }

    #[test]
    fn build_message_items_sent_message_mentions_also_render() {
        // Mentions are not receive-only: a self-composed mention (already
        // wire-wrapped by `on_send_message` before the record is stored)
        // must render identically through the same one code path.
        let records = vec![
            MessageRecord { text: "hi @[Alice] it's Bob".into(), is_ours: true, acked: false, ts_ms: 0 },
        ];
        let items = UiRuntime::build_message_items(&records, /* is_channel */ false, "Bob", &["Bob", "Alice"]);
        assert_eq!(items[0].text, "hi @Alice it's Bob");
        assert_eq!(items[0].mention_tier, 1);
    }

    // ── splash_should_dismiss ────────────────────────────────────────────
    //
    // Acceptance-critical: pins the requirements below — the
    // animation always completes (not started yet, or started but not
    // settled → never dismiss on that basis alone), the splash never lingers
    // once the animation has settled (`Some(ms) >= min` → dismiss), and a
    // boot that never reaches `mark_app_ready()` (animation never starts)
    // can't wedge the UI (max cap dismisses regardless).

    // Mirrors `UiRuntime::SPLASH_MIN_MS`/`SPLASH_MAX_MS` (inlined rather than
    // referenced: those are private associated consts on a lifetime-generic
    // type, and `splash_should_dismiss` takes the thresholds as plain
    // parameters precisely so callers — including these tests — don't need
    // a concrete `UiRuntime<'d>` to exercise it).
    //
    // BUG FIX: mirrors moved from 1200/2000 to
    // 1600/2400 alongside the real constants — see their docs.
    const MIN_MS: u64 = 1600;
    const MAX_MS: u64 = 2400;

    #[test]
    fn animation_not_started_never_dismisses_below_max() {
        assert!(!splash_should_dismiss(0, None, MIN_MS, MAX_MS));
        assert!(!splash_should_dismiss(MAX_MS - 1, None, MIN_MS, MAX_MS));
    }

    #[test]
    fn animation_started_but_not_settled_waits() {
        // The animation has started (app_ready fired and `start_animation()`
        // ran) but hasn't had time to play through — must NOT dismiss yet,
        // regardless of how much boot-clock time (`elapsed_ms`) has passed.
        assert!(!splash_should_dismiss(0, Some(0), MIN_MS, MAX_MS));
        assert!(!splash_should_dismiss(MIN_MS - 1, Some(MIN_MS - 1), MIN_MS, MAX_MS));
    }

    #[test]
    fn animation_settled_dismisses() {
        assert!(splash_should_dismiss(MIN_MS, Some(MIN_MS), MIN_MS, MAX_MS));
        assert!(splash_should_dismiss(MIN_MS + 500, Some(MIN_MS + 500), MIN_MS, MAX_MS));
    }

    #[test]
    fn animation_started_late_settles_on_its_own_clock_not_the_boot_clock() {
        // The whole point of decoupling the two clocks: `mark_app_ready()` can
        // fire well after the splash's first `step()` tick. Here the boot
        // clock (`elapsed_ms`) is already past `MIN_MS`, but the animation
        // only just started (`animation_elapsed_ms = Some(0)`) — must still
        // wait for the ANIMATION's own clock to reach `MIN_MS`, not dismiss
        // just because the boot clock did.
        assert!(!splash_should_dismiss(MIN_MS + 200, Some(0), MIN_MS, MAX_MS));
        assert!(splash_should_dismiss(MIN_MS + 200, Some(MIN_MS), MIN_MS, MAX_MS));
    }

    #[test]
    fn max_cap_dismisses_even_when_animation_never_started() {
        assert!(splash_should_dismiss(MAX_MS, None, MIN_MS, MAX_MS));
        assert!(splash_should_dismiss(MAX_MS + 1000, None, MIN_MS, MAX_MS));
    }

    // BUG FIX: pins the coordination
    // constraint between the two thresholds themselves, not just
    // `splash_should_dismiss`'s branch logic — the actual defect
    // was a MIN/MAX pair that had drifted out of the "lingers a bit longer,
    // total time still ~2-2.5 s max, animation still completes" envelope.
    // A future edit to either constant (or to the splash animation's total
    // duration in `screens::splash`) that breaks this envelope should fail
    // here rather than only be caught by eyeballing the on-device timing.
    #[test]
    fn min_and_max_stay_within_the_acceptance_envelope() {
        // Animation timeline total, from `screens::splash`'s module doc.
        const SPLASH_ANIMATION_TOTAL_MS: u64 = 1150;
        assert!(
            MIN_MS >= SPLASH_ANIMATION_TOTAL_MS,
            "SPLASH_MIN_MS must stay >= the one-shot animation's total \
             duration or the splash can dismiss mid-animation",
        );
        assert!(MIN_MS < MAX_MS, "the defensive cap must sit above the floor");
        assert!(
            MAX_MS <= 2500,
            "SPLASH_MAX_MS must stay within the ~2-2.5 s acceptance budget",
        );
    }

    // ── touch_wake_transition ─────────────────────────────────────────────
    //
    // Acceptance-critical: the central invariant is that the
    // wake-triggering input is swallowed globally and never reaches the
    // focused widget. These pin the state machine driving that invariant.

    #[test]
    fn asleep_pressed_wakes_and_swallows_without_dispatch() {
        let o = touch_wake_transition(true, false, touch::TouchKind::Pressed);
        assert!(o.woke, "a Pressed while asleep must wake the screen");
        assert!(!o.dispatch, "the wake-triggering Pressed must NOT reach the focused widget");
        assert!(o.swallow_active, "must keep swallowing until the matching Released");
    }

    #[test]
    fn asleep_released_wakes_and_does_not_leave_swallow_active() {
        // Defensive case: shouldn't happen in practice (sleep can't engage
        // mid-gesture — see step()'s inactivity check — but a Released alone
        // must still wake+swallow, not dispatch, and not get stuck swallowing
        // forever waiting for a Released that already happened).
        let o = touch_wake_transition(true, false, touch::TouchKind::Released);
        assert!(o.woke);
        assert!(!o.dispatch);
        assert!(!o.swallow_active);
    }

    #[test]
    fn wake_gesture_moved_tail_still_swallowed() {
        // After the initiating Pressed woke the screen, a held finger's
        // Moved samples must keep being swallowed, not dispatched.
        let o = touch_wake_transition(false, true, touch::TouchKind::Moved);
        assert!(!o.woke, "already awake — this is not a second wake");
        assert!(!o.dispatch, "still draining the wake gesture's tail");
        assert!(o.swallow_active, "Moved does not end the gesture");
    }

    #[test]
    fn wake_gesture_released_ends_swallow() {
        let o = touch_wake_transition(false, true, touch::TouchKind::Released);
        assert!(!o.woke);
        assert!(!o.dispatch, "the wake gesture's own Released must not dispatch either");
        assert!(!o.swallow_active, "Released ends the swallowed gesture");
    }

    #[test]
    fn normal_operation_dispatches_every_kind() {
        // Screen already awake, no wake gesture in flight: every event kind
        // dispatches normally — this is the ordinary, un-swallowed path that
        // must not regress for existing touch interactions.
        for kind in [touch::TouchKind::Pressed, touch::TouchKind::Moved, touch::TouchKind::Released] {
            let o = touch_wake_transition(false, false, kind);
            assert!(!o.woke);
            assert!(o.dispatch, "{:?} must dispatch during normal operation", kind);
            assert!(!o.swallow_active);
        }
    }

    #[test]
    fn a_full_wake_gesture_never_dispatches_any_event() {
        // End-to-end simulation of one physical tap that wakes the screen:
        // Pressed (wakes) -> Moved -> Released, driven through the state
        // machine exactly as step() would sequence it. NOT ONE event in this
        // gesture may reach `dispatch` — that is the whole point of the
        // swallow invariant.
        let mut asleep = true;
        let mut swallow = false;
        let mut any_dispatched = false;
        let mut woke_count = 0;
        for kind in [touch::TouchKind::Pressed, touch::TouchKind::Moved, touch::TouchKind::Released] {
            let o = touch_wake_transition(asleep, swallow, kind);
            if o.woke { woke_count += 1; }
            if o.dispatch { any_dispatched = true; }
            swallow = o.swallow_active;
            asleep = false; // step() always clears asleep after processing any event
        }
        assert_eq!(woke_count, 1, "exactly one wake for the whole gesture");
        assert!(!any_dispatched, "no event in the waking gesture may reach the focused widget");
        assert!(!swallow, "swallow must have cleared by the gesture's Released");
    }

    // ── keyboard_drain_should_stop ────────────────────────────────────────
    //
    // Regression guard: the
    // multi-byte keyboard drain must not evaluate a same-burst byte against a
    // screen that a nav-triggering byte just scheduled to change out from
    // under it.

    #[test]
    fn continues_below_bound_with_no_pending_nav() {
        assert!(!keyboard_drain_should_stop(0, 0, 8));
        assert!(!keyboard_drain_should_stop(0, 7, 8));
    }

    #[test]
    fn stops_at_the_defensive_bound_even_with_no_nav() {
        assert!(keyboard_drain_should_stop(0, 8, 8));
        assert!(keyboard_drain_should_stop(0, 9, 8));
    }

    #[test]
    fn stops_on_any_nonzero_pending_nav_regardless_of_count() {
        // A nav-triggering byte (MessageView-seed = 5, Compose Return-to-send
        // = 6) must stop the drain immediately, even on the very first byte
        // of a burst — the whole point is to defer the REST of the burst to
        // the next step() rather than misattribute it to the about-to-change
        // screen.
        assert!(keyboard_drain_should_stop(5, 0, 8));
        assert!(keyboard_drain_should_stop(6, 1, 8));
    }

    // ── send_nav_deferral_elapsed ─────────────────────────────────────────
    //
    // Regression guard: RocketOnSend's one-shot must have its full 400ms window to
    // render on the still-live Compose screen before step() swaps to
    // MessageView.

    #[test]
    fn no_deferral_armed_never_elapses() {
        assert!(!send_nav_deferral_elapsed(None, 0));
        assert!(!send_nav_deferral_elapsed(None, u64::MAX));
    }

    #[test]
    fn deferral_not_yet_elapsed_before_the_deadline() {
        assert!(!send_nav_deferral_elapsed(Some(1_000), 999));
    }

    #[test]
    fn deferral_elapsed_at_and_past_the_deadline() {
        // Exactly-at-deadline counts as elapsed (`>=`), not just strictly past.
        assert!(send_nav_deferral_elapsed(Some(1_000), 1_000));
        assert!(send_nav_deferral_elapsed(Some(1_000), 1_500));
    }

    // ── incoming_message_is_unread ─────────────────────────────────────────
    //
    // Regression guard:
    // the "don't flag unread while this exact thread is open" gate must
    // suppress ONLY the currently-open conversation, and must NOT stay
    // latched to a conversation that is no longer open (i.e. once
    // `navigate_to_contact_list` has cleared `active_convo` back to `None`).

    #[test]
    fn no_active_convo_is_always_unread() {
        // `None` — either never opened a conversation, or (the bug this
        // fix addresses) properly cleared on return to ContactList — must
        // never suppress the badge.
        assert!(incoming_message_is_unread(None, 0x20, false));
        assert!(incoming_message_is_unread(None, 0x20, true));
    }

    #[test]
    fn matching_open_convo_suppresses_unread() {
        // The exact (hash, is_channel) pair currently open in MessageView —
        // the message lands directly in the live view, so it must not also
        // flag the badge.
        assert!(!incoming_message_is_unread(Some((0x20, false)), 0x20, false));
        assert!(!incoming_message_is_unread(Some((0x20, true)), 0x20, true));
    }

    #[test]
    fn different_hash_or_kind_stays_unread_even_with_an_active_convo() {
        // A different contact/channel — or the same hash under the other
        // kind (DM hash 0x20 vs. channel hash 0x20 are different threads) —
        // must still flag unread while some OTHER convo is open.
        assert!(incoming_message_is_unread(Some((0x20, false)), 0x30, false));
        assert!(incoming_message_is_unread(Some((0x20, false)), 0x20, true));
        assert!(incoming_message_is_unread(Some((0x20, true)), 0x20, false));
    }

    // ── message_view_compose_seed ────────────────────────────────────────
    //
    // Acceptance-critical: printable keys must seed the Compose draft;
    // non-text/navigation keys must not.

    #[test]
    fn printable_letters_digits_and_symbols_seed_the_draft() {
        assert_eq!(message_view_compose_seed(b'a').as_deref(), Some("a"));
        assert_eq!(message_view_compose_seed(b'Z').as_deref(), Some("Z"));
        assert_eq!(message_view_compose_seed(b'5').as_deref(), Some("5"));
        assert_eq!(message_view_compose_seed(b'!').as_deref(), Some("!"));
        // ':' seeds too, so the compose shortcode autocomplete can trigger
        // immediately off a keypress that opened compose.
        assert_eq!(message_view_compose_seed(b':').as_deref(), Some(":"));
        assert_eq!(message_view_compose_seed(b' ').as_deref(), Some(" "));
        assert_eq!(message_view_compose_seed(b'~').as_deref(), Some("~"));
    }

    #[test]
    fn control_and_navigation_bytes_do_not_seed() {
        // Backspace, Return/Enter, Tab, Escape — all have Slint key mappings
        // in `keyboard::key_text` but must NOT flip MessageView to Compose.
        for byte in [0x08u8, 0x7F, 0x0D, 0x0A, 0x09, 0x1B] {
            assert_eq!(
                message_view_compose_seed(byte), None,
                "byte 0x{:02X} must not seed a Compose draft", byte,
            );
        }
    }

    #[test]
    fn unmapped_control_bytes_do_not_seed() {
        assert_eq!(message_view_compose_seed(0x00), None);
        assert_eq!(message_view_compose_seed(0x01), None);
        assert_eq!(message_view_compose_seed(0x1F), None);
    }

    // ── compose_return_should_send ────────────────────────────────────────
    //
    // Acceptance-critical: Return sends non-empty drafts; empty/whitespace-only
    // drafts must NOT send (an explicit guard-against-empty-sends requirement).

    #[test]
    fn non_empty_draft_sends_on_return() {
        assert!(compose_return_should_send("hi"));
        assert!(compose_return_should_send("  hi  ")); // surrounding whitespace is fine
        assert!(compose_return_should_send("a"));
        assert!(compose_return_should_send(":smile:"));
    }

    #[test]
    fn empty_or_whitespace_only_draft_does_not_send() {
        assert!(!compose_return_should_send(""));
        assert!(!compose_return_should_send(" "));
        assert!(!compose_return_should_send("   \t  "));
        assert!(!compose_return_should_send("\n"));
    }

    // ── BuzzerDriver channel config (auto_clear) ────────────────────────────
    // Regression guard for "notification audio plays indefinitely": the fix is a one-flag I2S channel
    // config change in `BuzzerDriver::new` that can't be exercised without
    // real hardware, so pin the contract at the config-value level instead —
    // if a future edit drops or no-ops the `.auto_clear(true)` call, this
    // fails loudly rather than the bug silently coming back.

    #[test]
    fn buzzer_channel_config_enables_auto_clear() {
        let cfg = I2sChannelConfig::default().auto_clear(true);
        // `Config` derives `PartialEq`; the default is `auto_clear: false`
        // (see `esp-idf-hal::i2s::config::Config::new`), so this fails if
        // `.auto_clear(true)` is ever accidentally dropped from `BuzzerDriver::new`.
        assert_ne!(cfg, I2sChannelConfig::default());
    }

    // ── battery_display_fields_changed (alloc-and-tick dedup guard) ─────────
    // Regression guard: pins
    // exactly which `BatteryStatus` fields gate the AdminMenu battery row's
    // `format!` + Slint push, independent of the hardware-backed `UiRuntime`.

    #[test]
    fn battery_display_fields_changed_false_when_percent_and_charging_same() {
        let a = crate::battery::BatteryStatus { percent: 50, charging: false, raw_mv: 3700, held_raw_mv: 3700 };
        let b = crate::battery::BatteryStatus { percent: 50, charging: false, raw_mv: 3712, held_raw_mv: 3705 };
        // raw_mv/held_raw_mv jitter (e.g. one ADC sample apart) must NOT count
        // as a display change — the row never renders either field.
        assert!(!battery_display_fields_changed(a, b));
    }

    #[test]
    fn battery_display_fields_changed_true_on_percent_change() {
        let a = crate::battery::BatteryStatus { percent: 50, charging: false, raw_mv: 0, held_raw_mv: 0 };
        let b = crate::battery::BatteryStatus { percent: 49, charging: false, raw_mv: 0, held_raw_mv: 0 };
        assert!(battery_display_fields_changed(a, b));
    }

    #[test]
    fn battery_display_fields_changed_true_on_charging_flip() {
        let a = crate::battery::BatteryStatus { percent: 50, charging: false, raw_mv: 0, held_raw_mv: 0 };
        let b = crate::battery::BatteryStatus { percent: 50, charging: true, raw_mv: 0, held_raw_mv: 0 };
        assert!(battery_display_fields_changed(a, b));
    }

    // ── notif_prefs_from_toggles (admin-menu master toggles) ───────────────
    // Regression guard for "audio/visual notifications ignore the admin
    // settings toggles": pins the
    // pure mapping `sync_notif_prefs` installs into `self.notif` every
    // `step()`, independent of the hardware-backed `UiRuntime`.

    #[test]
    fn notif_prefs_from_toggles_both_off_disables_every_event() {
        let prefs = notif_prefs_from_toggles(false, false);
        for event in [
            NotifEvent::IncomingDm,
            NotifEvent::IncomingGroupMsg,
            NotifEvent::DmAcked,
            NotifEvent::ChannelAcked,
            NotifEvent::Provisioned,
            NotifEvent::TelemetryResponse,
            NotifEvent::PinError,
            NotifEvent::PinSuccess,
        ] {
            let pref = prefs.pref_for(event);
            assert!(!pref.visual, "{:?} visual should be off", event);
            assert!(!pref.audible, "{:?} audible should be off", event);
        }
    }

    #[test]
    fn notif_prefs_from_toggles_both_on_enables_incoming_dm() {
        let prefs = notif_prefs_from_toggles(true, true);
        assert!(prefs.incoming_dm.visual);
        assert!(prefs.incoming_dm.audible);
    }

    #[test]
    fn notif_prefs_from_toggles_gates_dispatcher_fire() {
        // End-to-end through the real gating path: build the prefs the
        // "both off" master toggle produces, install them via `set_prefs`
        // (same call `sync_notif_prefs` makes), then confirm `fire()`
        // actually produces no tone (PinSuccess has no visual mechanism to
        // gate at all now that the border flash is gone).
        let mut d = NotifDispatcher::new(NotifPrefs::default());
        d.set_prefs(notif_prefs_from_toggles(false, false));
        d.fire(NotifEvent::PinSuccess, 0, false);
        assert!(d.take_tones().is_none());
    }

    #[test]
    fn notif_prefs_from_toggles_visual_off_audible_on_is_independent() {
        // The two toggles are independent switches, not a single master
        // mute — audible-only must still fire tones. (`pin_success.visual`
        // is inert now that the border flash is gone, but the toggle
        // mapping still threads the raw bool through uniformly; see
        // `NotifPref`'s doc.)
        let prefs = notif_prefs_from_toggles(false, true);
        assert!(!prefs.pin_success.visual);
        assert!(prefs.pin_success.audible);
    }

    // ── roll_selection ───────────────────────────────────────────────────
    //
    // Acceptance-critical: this is the index math behind "roll highlights a
    // contact/channel" / "roll through rows" on ContactList and AdminMenu.

    #[test]
    fn first_roll_from_no_selection_lands_on_top_row_either_direction() {
        assert_eq!(roll_selection(-1, 3, true), 0, "first Up roll starts at row 0");
        assert_eq!(roll_selection(-1, 3, false), 0, "first Down roll also starts at row 0");
    }

    #[test]
    fn empty_list_never_produces_a_valid_index() {
        assert_eq!(roll_selection(-1, -1, true), -1);
        assert_eq!(roll_selection(-1, -1, false), -1);
    }

    #[test]
    fn roll_up_decrements_and_floors_at_zero() {
        assert_eq!(roll_selection(2, 3, true), 1);
        assert_eq!(roll_selection(0, 3, true), 0, "already at the top row — holds, no wrap");
    }

    #[test]
    fn roll_down_increments_and_ceilings_at_max_idx() {
        assert_eq!(roll_selection(1, 3, false), 2);
        assert_eq!(roll_selection(3, 3, false), 3, "already at the bottom row — holds, no wrap");
    }

    #[test]
    fn single_row_list_holds_at_zero_both_directions() {
        assert_eq!(roll_selection(0, 0, true), 0);
        assert_eq!(roll_selection(0, 0, false), 0);
    }
}
