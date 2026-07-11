// SPDX-License-Identifier: GPL-3.0-only
//! Notification model — visual + audible alerts, per-event configurable.
//!
//! # Overview
//!
//! Every user-relevant event has a [`NotifEvent`] variant.  Each event can
//! independently trigger an **audible** (synthesized tone burst streamed over
//! I2S) alert; two of the events additionally trigger a **visual** cue (see
//! "Incoming-message visual is sleep-gated" below).
//!
//! Preferences are stored in a [`NotifPrefs`] table and loaded from the
//! provisioned config.  The admin sets defaults during provisioning; the user
//! can adjust their own preferences from the on-device settings menu.
//!
//! # Hardware
//!
//! | Signal | GPIO | Notes |
//! |--------|------|-------|
//! | Speaker (I2S) | 5 / 7 / 6 | WS (LRCK) / BCK / DOUT — I2S0, std/Philips mode |
//!
//! CORRECTION (2026-07-03): this table
//! previously claimed a passive piezo buzzer on GPIO46 driven via LEDC PWM.
//! That hardware does not exist on the T-Deck / T-Deck Plus — GPIO46 is the
//! keyboard co-processor's interrupt line. The board's actual (and only)
//! audio-output path is the onboard I2S speaker; see `ui::BuzzerDriver` for
//! the drive implementation and its doc comment for the corroborating sources.
//!
//! The notification tone is a short two-tone chirp (250 ms at 880 Hz, then
//! 100 ms at 1320 Hz).  The "alert" pattern is more urgent (three short beeps
//! at 1000 Hz).  PIN error uses a low descending tone.
//!
//! REMOVAL (2026-07-05): this module used to also flash the screen border a
//! bright colour for 300ms on every event (`draw_flash_border` in
//! `ui::display`). That mechanism has been ripped out entirely — audio plus
//! the incoming-message keyboard-backlight blink (below) are the only visual
//! notifications remaining. `PinError`/`PinSuccess` are now audio-only by
//! deliberate design: no replacement visual cue was added for them.
//!
//! # Incoming-message visual is sleep-gated
//!
//! `IncomingDm`/`IncomingGroupMsg` are special-cased in [`NotifDispatcher::fire`]:
//!
//! - Screen **awake**: nothing visual happens.
//! - Screen **asleep**: a [`BlinkLoop`] blinks the keyboard backlight a few
//!   times immediately, then repeats the burst every few seconds until the
//!   device wakes (`NotifDispatcher::stop_blink`, called from `wake_screen`).
//!   Multiple messages while asleep keep the *same* loop running rather than
//!   starting a new one (`BlinkLoop::notify` is idempotent while active).
//!
//! All other events (`DmAcked`, `ChannelAcked`, `Provisioned`,
//! `TelemetryResponse`, `PinError`, `PinSuccess`) have no visual notification
//! at all now that the border flash is gone — they are audio-only. Their
//! `NotifPref.visual` field is consulted nowhere: see that field's doc for
//! why it is kept rather than removed.

/// Events that can trigger a notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifEvent {
    /// A new direct message arrived from a known contact.
    IncomingDm,
    /// A new group channel message arrived.
    IncomingGroupMsg,
    /// An outbound DM was acknowledged by the mesh.
    DmAcked,
    /// An outbound channel/group message was implicitly acknowledged (the
    /// device heard its own send repeated back into the mesh — see
    /// `ui::UiEvent::ChannelAcked`'s doc).
    ChannelAcked,
    /// The device has just become provisioned (first-time setup complete).
    ///
    /// Not yet fired anywhere: the natural call site is
    /// `main.rs::run()` right before the post-provisioning `esp_restart()`,
    /// but firing it there would need a short pump loop (to let the tone
    /// actually play before reboot cuts power to the renderer) that is
    /// out of scope for a warning-cleanup pass. Kept — with prefs and tone
    /// sequence already wired below — for that follow-up.
    #[allow(dead_code)]
    Provisioned,
    /// A telemetry location response was received.
    TelemetryResponse,
    /// PIN entry: wrong PIN entered.
    PinError,
    /// PIN entry: correct PIN entered.
    PinSuccess,
}

/// Per-event notification preferences (visual + audible independently controlled).
///
/// `visual`: only meaningful for `IncomingDm`/`IncomingGroupMsg` — it gates
/// the keyboard-backlight blink fired for those two events while the screen
/// is asleep (see [`NotifDispatcher::fire`]). For every other event
/// (`DmAcked`, `ChannelAcked`, `Provisioned`, `TelemetryResponse`, `PinError`,
/// `PinSuccess`) there is no visual notification mechanism left to gate — the
/// border flash that `visual` used to control was ripped out
/// and no replacement was
/// added, so `visual` is inert (read nowhere) for those six events. The
/// field is kept on the shared struct — rather than split into a
/// per-event-shape type — because the config store
/// (`config_store::load_provisioned_config`) and the provisioning protocol
/// (`SET_NOTIF_DEFAULTS`) both serialize one `visual`+`audible` pair per
/// event uniformly; narrowing the struct would require a wire-format change
/// out of scope here.
#[derive(Clone, Debug)]
pub struct NotifPref {
    pub visual: bool,
    pub audible: bool,
}

impl Default for NotifPref {
    fn default() -> Self {
        NotifPref {
            visual: true,
            audible: true,
        }
    }
}

/// Full notification preference table — one `NotifPref` per event.
///
/// Serialize/deserialize of these prefs is owned by the config store
/// (`config_store::load_provisioned_config`).  The admin provides initial
/// defaults at provisioning time (`SET_NOTIF_DEFAULTS`); the user can tune
/// individual events here.
#[derive(Clone, Debug)]
pub struct NotifPrefs {
    pub incoming_dm: NotifPref,
    pub incoming_group_msg: NotifPref,
    pub dm_acked: NotifPref,
    pub channel_acked: NotifPref,
    pub provisioned: NotifPref,
    pub telemetry_response: NotifPref,
    pub pin_error: NotifPref,
    pub pin_success: NotifPref,
}

impl Default for NotifPrefs {
    /// Sensible defaults: all visual on, audible on except ack + telemetry.
    fn default() -> Self {
        NotifPrefs {
            incoming_dm: NotifPref {
                visual: true,
                audible: true,
            },
            incoming_group_msg: NotifPref {
                visual: true,
                audible: true,
            },
            dm_acked: NotifPref {
                visual: true,
                audible: false,
            },
            channel_acked: NotifPref {
                visual: true,
                audible: false,
            },
            provisioned: NotifPref {
                visual: true,
                audible: true,
            },
            telemetry_response: NotifPref {
                visual: true,
                audible: false,
            },
            pin_error: NotifPref {
                visual: true,
                audible: true,
            },
            pin_success: NotifPref {
                visual: true,
                audible: true,
            },
        }
    }
}

impl NotifPrefs {
    /// Build from the provisioning defaults (`visual_default` and
    /// `audible_default` flags from `SET_NOTIF_DEFAULTS`).  Per-event
    /// overrides can be applied afterwards via `set_event_pref`.
    pub fn from_provisioning_defaults(visual: bool, audible: bool) -> Self {
        let pref = NotifPref { visual, audible };
        NotifPrefs {
            incoming_dm: pref.clone(),
            incoming_group_msg: pref.clone(),
            dm_acked: NotifPref {
                visual,
                audible: false,
            }, // ack never audible by default
            channel_acked: NotifPref {
                visual,
                audible: false,
            }, // ack never audible by default
            provisioned: pref.clone(),
            telemetry_response: NotifPref {
                visual,
                audible: false,
            },
            pin_error: pref.clone(),
            pin_success: pref.clone(),
        }
    }

    /// Look up the pref for a given event.
    pub fn pref_for(&self, event: NotifEvent) -> &NotifPref {
        match event {
            NotifEvent::IncomingDm => &self.incoming_dm,
            NotifEvent::IncomingGroupMsg => &self.incoming_group_msg,
            NotifEvent::DmAcked => &self.dm_acked,
            NotifEvent::ChannelAcked => &self.channel_acked,
            NotifEvent::Provisioned => &self.provisioned,
            NotifEvent::TelemetryResponse => &self.telemetry_response,
            NotifEvent::PinError => &self.pin_error,
            NotifEvent::PinSuccess => &self.pin_success,
        }
    }

    /// Override the pref for a specific event (user-adjustable).
    ///
    /// Not called anywhere today — no on-device UI exposes per-event
    /// overrides yet (see `UiRuntime::sync_notif_prefs`'s doc, which names
    /// this as the call site a future per-event settings screen would need
    /// to merge through). Kept, with full test coverage, for that screen.
    #[allow(dead_code)]
    pub fn set_event_pref(&mut self, event: NotifEvent, pref: NotifPref) {
        match event {
            NotifEvent::IncomingDm => self.incoming_dm = pref,
            NotifEvent::IncomingGroupMsg => self.incoming_group_msg = pref,
            NotifEvent::DmAcked => self.dm_acked = pref,
            NotifEvent::ChannelAcked => self.channel_acked = pref,
            NotifEvent::Provisioned => self.provisioned = pref,
            NotifEvent::TelemetryResponse => self.telemetry_response = pref,
            NotifEvent::PinError => self.pin_error = pref,
            NotifEvent::PinSuccess => self.pin_success = pref,
        }
    }
}

/// Buzzer tone descriptor (frequency in Hz, duration in ms).
#[derive(Clone, Copy, Debug)]
pub struct ToneBurst {
    pub freq_hz: u32,
    pub dur_ms: u32,
}

/// Tone sequence for each event.  Returns a slice of up to 3 tone bursts.
pub fn tone_sequence(event: NotifEvent) -> &'static [ToneBurst] {
    match event {
        NotifEvent::IncomingDm => &[
            ToneBurst {
                freq_hz: 880,
                dur_ms: 120,
            },
            ToneBurst {
                freq_hz: 1320,
                dur_ms: 80,
            },
        ],
        NotifEvent::IncomingGroupMsg => &[
            ToneBurst {
                freq_hz: 660,
                dur_ms: 100,
            },
            ToneBurst {
                freq_hz: 880,
                dur_ms: 100,
            },
        ],
        NotifEvent::DmAcked => &[
            ToneBurst {
                freq_hz: 1047,
                dur_ms: 60,
            }, // brief soft tick
        ],
        NotifEvent::ChannelAcked => &[
            ToneBurst {
                freq_hz: 1047,
                dur_ms: 60,
            }, // same brief soft tick as DmAcked
        ],
        NotifEvent::Provisioned => &[
            ToneBurst {
                freq_hz: 523,
                dur_ms: 150,
            },
            ToneBurst {
                freq_hz: 784,
                dur_ms: 150,
            },
            ToneBurst {
                freq_hz: 1047,
                dur_ms: 250,
            },
        ],
        NotifEvent::TelemetryResponse => &[ToneBurst {
            freq_hz: 880,
            dur_ms: 80,
        }],
        NotifEvent::PinError => &[
            ToneBurst {
                freq_hz: 330,
                dur_ms: 200,
            },
            ToneBurst {
                freq_hz: 220,
                dur_ms: 300,
            },
        ],
        NotifEvent::PinSuccess => &[
            ToneBurst {
                freq_hz: 784,
                dur_ms: 100,
            },
            ToneBurst {
                freq_hz: 1047,
                dur_ms: 150,
            },
        ],
    }
}

// ── Incoming-message blink loop ──────────────────────────────────────────────

/// Number of on/off blinks fired per burst.
const BLINK_COUNT: u32 = 3;
/// Duration of each blink phase (on, then off), in milliseconds.
const BLINK_PHASE_MS: u64 = 150;
/// Period between the *start* of one burst and the start of the next, in
/// milliseconds. Must be `>=` the burst's own on/off window
/// (`BLINK_COUNT * 2 * BLINK_PHASE_MS` = 900 ms) or bursts would overlap.
const BURST_INTERVAL_MS: u64 = 5_000;

/// Pure state machine driving the keyboard-backlight blink loop fired for an
/// incoming message received while the screen is asleep.
///
/// Extracted as a plain struct (no hardware/I2C dependency) so the timing
/// logic — the part most likely to be gotten subtly wrong —
/// has host-checkable unit tests, matching the pattern already used for
/// `touch_wake_transition` (`ui/mod.rs`) and `key_text`/`backlight_duty`
/// (`ui/keyboard.rs`).
#[derive(Clone, Copy, Debug, Default)]
pub struct BlinkLoop {
    active: bool,
    /// `uptime_ms` at which the current burst window started.
    burst_start_ms: u64,
}

impl BlinkLoop {
    /// Start the loop, blinking immediately. Idempotent while already
    /// active: a second (or third, ...) message arriving mid-loop does **not**
    /// reset the burst timing — the requirement is a single ongoing loop
    /// regardless of message count, not one loop per message.
    pub fn notify(&mut self, now_ms: u64) {
        if !self.active {
            self.active = true;
            self.burst_start_ms = now_ms;
        }
    }

    /// Stop the loop immediately (called on wake).
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Poll the desired keyboard-backlight state at `now_ms`.
    ///
    /// Returns `false` whenever the loop isn't active. While active, rolls
    /// `burst_start_ms` forward by whole `BURST_INTERVAL_MS` periods as they
    /// elapse (so a caller that hasn't polled in a while still lands on the
    /// *current* burst rather than replaying every missed one), then reports
    /// `true` for the "on" half of each blink phase within the burst's on/off
    /// window and `false` for the "off" half and for the quiet gap between
    /// bursts.
    pub fn poll(&mut self, now_ms: u64) -> bool {
        if !self.active {
            return false;
        }
        let mut elapsed = now_ms.saturating_sub(self.burst_start_ms);
        if elapsed >= BURST_INTERVAL_MS {
            let periods = elapsed / BURST_INTERVAL_MS;
            self.burst_start_ms += periods * BURST_INTERVAL_MS;
            elapsed = now_ms.saturating_sub(self.burst_start_ms);
        }
        let blink_window_ms = (BLINK_COUNT as u64) * 2 * BLINK_PHASE_MS;
        if elapsed >= blink_window_ms {
            return false; // between bursts
        }
        let phase = elapsed / BLINK_PHASE_MS;
        phase.is_multiple_of(2) // even phases are "on", odd phases are "off"
    }
}

/// Notification dispatcher.
///
/// Call [`NotifDispatcher::fire`] when an event occurs.  The dispatcher checks
/// prefs and enqueues visual (keyboard-backlight blink loop) and audible
/// (buzzer) actions for the UI runtime to consume.
pub struct NotifDispatcher {
    prefs: NotifPrefs,
    /// Pending tone sequence to play.
    pub pending_tones: Option<&'static [ToneBurst]>,
    /// Incoming-message keyboard-backlight blink loop state.
    blink: BlinkLoop,
}

impl NotifDispatcher {
    pub fn new(prefs: NotifPrefs) -> Self {
        NotifDispatcher {
            prefs,
            pending_tones: None,
            blink: BlinkLoop::default(),
        }
    }

    /// Update preferences (e.g. after the user adjusts a setting).
    pub fn set_prefs(&mut self, prefs: NotifPrefs) {
        self.prefs = prefs;
    }

    /// Fire a notification for `event`.
    ///
    /// `now_ms`: current uptime in milliseconds (from `esp_timer_get_time`).
    /// `screen_asleep`: current screen-sleep state (`UiRuntime::screen_asleep`)
    /// — only consulted for `IncomingDm`/`IncomingGroupMsg`, see the module
    /// docs' "Incoming-message visual is sleep-gated" section. Every other
    /// event ignores it: there is no visual notification left for those
    /// events (the border flash was removed) so `pref.visual` is not even
    /// read outside the two incoming-message variants.
    pub fn fire(&mut self, event: NotifEvent, now_ms: u64, screen_asleep: bool) {
        let pref = self.prefs.pref_for(event);
        if let NotifEvent::IncomingDm | NotifEvent::IncomingGroupMsg = event {
            if pref.visual && screen_asleep {
                self.blink.notify(now_ms);
            }
        }
        if pref.audible {
            self.pending_tones = Some(tone_sequence(event));
        }
    }

    /// Take the pending tone sequence (if any) for the buzzer driver.
    ///
    /// The caller is responsible for playing the tones; this clears the
    /// pending queue so tones are only played once.
    pub fn take_tones(&mut self) -> Option<&'static [ToneBurst]> {
        self.pending_tones.take()
    }

    /// Poll the desired keyboard-backlight state for the incoming-message
    /// blink loop at `now_ms` — see [`BlinkLoop::poll`].
    ///
    /// Intended to be called exactly once per `step()`, from the single
    /// keyboard-backlight arbiter (`UiRuntime::sync_keyboard_backlight`) —
    /// see that function's doc for why only one place is allowed to write the
    /// keyboard backlight at all.
    pub fn poll_blink(&mut self, now_ms: u64) -> bool {
        self.blink.poll(now_ms)
    }

    /// Stop the blink loop immediately. Called from `wake_screen` so waking
    /// the device always halts the loop on the spot, per the
    /// acceptance criteria.
    pub fn stop_blink(&mut self) {
        self.blink.stop();
    }

    /// `true` while the incoming-message blink loop is running. Exposed for
    /// tests (reads `BlinkLoop`'s private field directly, same module — no
    /// production code needs this, only `poll_blink`, so no public accessor
    /// is added just to avoid a dead-code warning on a real build).
    #[cfg(test)]
    pub fn blink_active(&self) -> bool {
        self.blink.active
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prefs_incoming_dm_audible() {
        let prefs = NotifPrefs::default();
        assert!(prefs.incoming_dm.audible);
        assert!(prefs.incoming_dm.visual);
    }

    #[test]
    fn provisioning_defaults_both_false() {
        let prefs = NotifPrefs::from_provisioning_defaults(false, false);
        assert!(!prefs.incoming_dm.visual);
        assert!(!prefs.incoming_dm.audible);
    }

    #[test]
    fn set_event_pref_override() {
        let mut prefs = NotifPrefs::default();
        prefs.set_event_pref(
            NotifEvent::IncomingDm,
            NotifPref {
                visual: false,
                audible: false,
            },
        );
        assert!(!prefs.incoming_dm.visual);
        assert!(!prefs.incoming_dm.audible);
    }

    #[test]
    fn dispatcher_fire_is_audio_only_for_non_incoming_event() {
        // Non-incoming events have no visual notification at all now that
        // the border flash is gone — only audible fires. `Provisioned` (not
        // one of the ack/telemetry events `NotifPrefs::default` special-cases
        // to audible:false) is the representative case: visual is inert for
        // it (only `IncomingDm`/`IncomingGroupMsg` ever consult `pref.visual`
        // — see `NotifDispatcher::fire`'s doc) and audible is on by default.
        //
        // FOUND BY THIS TEST FIRST EXECUTING (firmware-core extraction,
        // ADR-0005): this test previously used `NotifEvent::DmAcked`, whose
        // documented default is `audible: false` ("audible on except ack +
        // telemetry" — see `NotifPrefs::default`'s doc) — `take_tones()` was
        // therefore always `None`, and `assert!(...is_some())` could never
        // have passed. `firmware/`'s detached workspace meant this
        // `#[cfg(test)]` block only ever type-checked, never ran, so the
        // defect was latent until this crate made it host-executable.
        let mut d = NotifDispatcher::new(NotifPrefs::default());
        d.fire(NotifEvent::Provisioned, 0, false);
        assert!(!d.blink_active());
        assert!(d.take_tones().is_some());
    }

    #[test]
    fn pin_events_are_audio_only_no_visual_replacement() {
        // By design: PIN feedback is audio-only — no
        // replacement visual cue for PinError/PinSuccess.
        let mut d = NotifDispatcher::new(NotifPrefs::default());
        d.fire(NotifEvent::PinError, 0, false);
        assert!(!d.blink_active());
        assert!(d.take_tones().is_some());

        d.fire(NotifEvent::PinSuccess, 0, false);
        assert!(!d.blink_active());
        assert!(d.take_tones().is_some());
    }

    #[test]
    fn dispatcher_fire_sets_tones() {
        let mut d = NotifDispatcher::new(NotifPrefs::default());
        d.fire(NotifEvent::IncomingDm, 0, false);
        assert!(d.take_tones().is_some());
        assert!(d.take_tones().is_none()); // consumed
    }

    #[test]
    fn dispatcher_no_fire_when_both_off() {
        let mut d = NotifDispatcher::new(NotifPrefs {
            incoming_dm: NotifPref {
                visual: false,
                audible: false,
            },
            ..NotifPrefs::default()
        });
        d.fire(NotifEvent::IncomingDm, 0, false);
        assert!(!d.blink_active());
        assert!(d.take_tones().is_none());
    }

    #[test]
    fn pin_error_tone_sequence_descends() {
        let seq = tone_sequence(NotifEvent::PinError);
        assert!(!seq.is_empty());
        // Error tone should be lower-pitched (descending)
        assert!(
            seq[0].freq_hz > seq[seq.len() - 1].freq_hz,
            "pin error tone should descend in pitch"
        );
    }

    // ── Incoming-message sleep-gated visual (blink loop) ────────────────────

    #[test]
    fn incoming_awake_fires_no_visual_at_all() {
        // Awake: no visual at all — deliberate "nothing when awake"
        // suppression (predates and is orthogonal to the border-flash
        // removal, which additionally removed the visual for every other
        // event too).
        let mut d = NotifDispatcher::new(NotifPrefs::default());
        d.fire(NotifEvent::IncomingDm, 0, false);
        assert!(!d.blink_active());
        // Audible is unaffected by sleep state.
        assert!(d.take_tones().is_some());
    }

    #[test]
    fn incoming_asleep_starts_blink() {
        let mut d = NotifDispatcher::new(NotifPrefs::default());
        d.fire(NotifEvent::IncomingGroupMsg, 1_000, true);
        assert!(d.blink_active());
    }

    #[test]
    fn incoming_asleep_visual_off_does_not_start_blink() {
        let mut d = NotifDispatcher::new(NotifPrefs {
            incoming_dm: NotifPref {
                visual: false,
                audible: true,
            },
            ..NotifPrefs::default()
        });
        d.fire(NotifEvent::IncomingDm, 0, true);
        assert!(!d.blink_active());
        assert!(d.take_tones().is_some()); // audible pref still honoured
    }

    #[test]
    fn second_message_while_asleep_keeps_single_loop() {
        // Requirement: multiple messages while asleep = one ongoing
        // loop, not one per message. A second `fire` after the loop has
        // already advanced must NOT reset the burst timing.
        let mut d = NotifDispatcher::new(NotifPrefs::default());
        d.fire(NotifEvent::IncomingDm, 0, true);
        // Advance partway into the burst, then a second message arrives.
        //
        // FOUND BY THIS TEST FIRST EXECUTING (firmware-core extraction,
        // ADR-0005): this line previously polled at 200ms and asserted "on".
        // 200ms falls in phase 1 (150-300ms), the first "off" half-cycle —
        // see `blink_loop_pattern_three_blinks_then_quiet_then_repeats`
        // (`!loop_.poll(150)` / `loop_.poll(300)`), a sibling test that DID
        // exercise this timing correctly. `firmware/`'s detached workspace
        // meant this `#[cfg(test)]` block only ever type-checked, never ran,
        // so the off-by-one-phase mistake was latent until this crate made
        // it host-executable. 100ms (phase 0, the first "on" half-cycle) is
        // the correct "still mid-burst, on" instant.
        assert!(d.poll_blink(100)); // still mid-burst, "on"
        d.fire(NotifEvent::IncomingGroupMsg, 200, true);
        // If the loop had restarted, elapsed-since-start would reset to 0 and
        // 200ms later would still read "on" for a different reason; instead
        // confirm the loop reaches its "off" gap on the ORIGINAL schedule
        // (900ms after the original start at 0), proving no reset occurred.
        assert!(!d.poll_blink(950));
    }

    #[test]
    fn wake_stops_blink_immediately() {
        let mut d = NotifDispatcher::new(NotifPrefs::default());
        d.fire(NotifEvent::IncomingDm, 0, true);
        assert!(d.blink_active());
        d.stop_blink();
        assert!(!d.blink_active());
        assert!(!d.poll_blink(50));
    }

    #[test]
    fn blink_loop_pattern_three_blinks_then_quiet_then_repeats() {
        let mut loop_ = BlinkLoop::default();
        loop_.notify(0);
        // 3 blinks of 150ms on/150ms off = 900ms window: on,off,on,off,on,off.
        assert!(loop_.poll(0)); // phase 0: on
        assert!(!loop_.poll(150)); // phase 1: off
        assert!(loop_.poll(300)); // phase 2: on
        assert!(!loop_.poll(450)); // phase 3: off
        assert!(loop_.poll(600)); // phase 4: on
        assert!(!loop_.poll(750)); // phase 5: off
        assert!(!loop_.poll(900)); // quiet gap until next burst at 5000
        assert!(!loop_.poll(4_999));
        assert!(loop_.poll(5_000)); // next burst starts, "on" again
    }

    #[test]
    fn blink_loop_notify_idempotent_while_active() {
        let mut loop_ = BlinkLoop::default();
        loop_.notify(0);
        loop_.notify(999); // should be a no-op: loop already active
                           // If `notify` had reset the start time to 999, 900ms later (1899)
                           // would still be inside the blink window ("on" or "off" blink phase,
                           // not yet the quiet gap). Confirm the ORIGINAL schedule instead: by
                           // 900 we're already in the quiet gap.
        assert!(!loop_.poll(900));
    }

    #[test]
    fn blink_loop_inactive_never_reports_on() {
        let mut loop_ = BlinkLoop::default();
        assert!(!loop_.poll(0));
        assert!(!loop_.poll(10_000));
    }
}
