// SPDX-License-Identifier: GPL-3.0-only
//! Repeater-signal tracker — a phone-style "how audible is the nearest
//! repeater" meter, built entirely from data every received packet already
//! carries.
//!
//! # Why hop count alone is enough (no advert parsing)
//!
//! MeshCore companion nodes never relay traffic; only designated repeaters
//! retransmit. So a received packet whose path length is **hop count ≥ 1**
//! was, by construction, last transmitted by a repeater — its RSSI/SNR is
//! therefore the downlink quality from the nearest audible repeater to this
//! device. A **zero-hop** packet came straight from its origin companion and
//! says nothing about repeater audibility. This module never parses adverts
//! and knows nothing about *which* repeater or how many are in range — only
//! "was the strongest recent hop≥1 packet strong", which is exactly what a
//! signal-bars meter needs.
//!
//! The two inputs this module consumes are already computed by the firmware
//! radio/protocol layers and simply not currently kept anywhere:
//! - RSSI dBm = `-(rssi_raw) / 2`, SNR dB = `snr_raw / 4`, both decoded in
//!   `radio.rs`'s `get_packet_status()` and read at `main.rs`'s RX-poll site
//!   (before the dedup drop, so duplicates are visible here too).
//! - Hop count parses out of the path-length header byte via
//!   `protocol::header::PathLen::hop_count()`.
//!
//! Wiring those call sites into this tracker (the "rx-tap") and rendering a
//! Slint `SignalMeter` widget from [`SignalTracker::level`] are both out of
//! scope here — that is the UI child of the `meshcadet-signal-meter`
//! campaign, consuming the contract this module freezes. See
//! `docs/adr/0010-signal-meter.md`.
//!
//! # Honest proxy, not a delivery guarantee
//!
//! The meter measures **downlink audibility**: how well this device can hear
//! the nearest repeater. It says nothing about whether that repeater — or any
//! further hop — can hear a message *this* device transmits, and nothing
//! about the health of the rest of the mesh beyond the first hop. Treat it as
//! "will my outbound message likely reach the first repeater", not "will my
//! message reach its destination". See ADR-0010 for the full caveat text
//! surfaced to operators.
//!
//! # No reboot persistence
//!
//! [`SignalTracker`] is in-memory, live state only — it starts at
//! [`SignalLevel::DirectOnly`] on construction (mirroring "device just
//! booted, no repeater heard yet") and carries nothing across a restart. This
//! is a deliberate scoping decision (ADR-0010), not an oversight: the meter
//! is meant to reflect *current* conditions, and a stale pre-reboot peak
//! surviving a restart would misrepresent them.

// This module is pure Rust with no esp-idf/radio dependency — time is a
// caller-supplied monotonic `u64` millisecond value (`now_ms`), not read from
// any clock here, so the whole tracker is host-testable. The real rx-tap
// (firmware/src/main.rs) is responsible for supplying a real monotonic clock
// (e.g. `esp_timer_get_time`), not the loop counter.

/// Number of discrete signal-bar levels above [`SignalLevel::DirectOnly`].
pub const MAX_BARS: u8 = 5;

/// Signal-meter reading: either no repeater has been heard recently
/// (`DirectOnly` — this device's own transmissions are unassisted, zero-hop),
/// or the strongest recent repeater-relayed packet mapped to `1..=MAX_BARS`
/// bars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalLevel {
    /// No hop≥1 packet has been heard within the decay window (including "no
    /// packet ever" at boot). Zero bars — the meter's empty state.
    DirectOnly,
    /// `1..=MAX_BARS` bars of the strongest recent repeater-relayed signal.
    Bars(u8),
}

/// All thresholds/hold/decay tunables for [`SignalTracker`], as field values
/// rather than baked-in constants, so they can be tuned (e.g. from HIL
/// feedback) without touching the tracker logic itself.
///
/// Construct via [`SignalConfig::new`] (validates/clamps) or
/// [`SignalConfig::default`] for the documented defaults below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalConfig {
    /// Minimum RSSI (dBm) required for each bar count, **strongest-first**:
    /// `bar_floor_dbm[0]` is the floor for `MAX_BARS` (5) bars,
    /// `bar_floor_dbm[MAX_BARS as usize - 1]` is the floor for 1 bar. A
    /// reading at or above `bar_floor_dbm[i]` (and below `bar_floor_dbm[i-1]`,
    /// checked in order) maps to `MAX_BARS - i` bars; a reading below every
    /// floor maps to 0 bars (`SignalLevel::DirectOnly`).
    ///
    /// Default: `[-70, -85, -100, -110, -120]` — see the module's ADR
    /// (`docs/adr/0010-signal-meter.md`) for the bars table this encodes.
    pub bar_floor_dbm: [i16; MAX_BARS as usize],
    /// SNR (dB) below which one bar is knocked off an otherwise-qualifying
    /// reading — a strong-RSSI-but-noisy link reads worse than raw RSSI alone
    /// would suggest. Default `0`.
    pub snr_knockdown_db: i8,
    /// A second, lower SNR (dB) floor — below this, one *additional* bar is
    /// knocked off (two total), for a link close to the LoRa noise floor
    /// (~-20 dB). Must be strictly below `snr_knockdown_db`; construction
    /// clamps it if not. Default `-10`.
    pub snr_floor_db: i8,
    /// How long (ms) the peak reading is held at full strength before decay
    /// begins. Default `60_000` (60 s).
    pub hold_full_ms: u64,
    /// How long (ms) each one-bar decay step takes once decay begins.
    /// Clamped to a minimum of `1` at construction (a `0` step would be a
    /// division-by-zero in [`SignalTracker::level`]). Default `45_000` (45 s).
    pub decay_step_ms: u64,
}

impl SignalConfig {
    /// Build a config, validating/clamping any out-of-order or degenerate
    /// input rather than letting it produce nonsensical bars/decay behavior:
    /// - `bar_floor_dbm` is forced strictly descending (each entry clamped
    ///   below the previous if the caller supplied it out of order).
    /// - `snr_floor_db` is forced strictly below `snr_knockdown_db`.
    /// - `decay_step_ms` is floored at `1` (never zero).
    pub fn new(
        mut bar_floor_dbm: [i16; MAX_BARS as usize],
        snr_knockdown_db: i8,
        mut snr_floor_db: i8,
        hold_full_ms: u64,
        decay_step_ms: u64,
    ) -> Self {
        for i in 1..bar_floor_dbm.len() {
            if bar_floor_dbm[i] >= bar_floor_dbm[i - 1] {
                bar_floor_dbm[i] = bar_floor_dbm[i - 1].saturating_sub(1);
            }
        }
        if snr_floor_db >= snr_knockdown_db {
            snr_floor_db = snr_knockdown_db.saturating_sub(1);
        }
        SignalConfig {
            bar_floor_dbm,
            snr_knockdown_db,
            snr_floor_db,
            hold_full_ms,
            decay_step_ms: decay_step_ms.max(1),
        }
    }

    /// RSSI-only bar count, `0..=MAX_BARS`, before any SNR knock-down.
    fn rssi_bars(&self, rssi_dbm: i16) -> u8 {
        for (i, &floor) in self.bar_floor_dbm.iter().enumerate() {
            if rssi_dbm >= floor {
                return MAX_BARS - i as u8;
            }
        }
        0
    }

    /// Full bar count for one sample: RSSI floor lookup, then SNR knock-down.
    /// A sample that clears at least one RSSI floor (`rssi_bars() >= 1`) is a
    /// genuinely-heard repeater and is never knocked below 1 bar by SNR
    /// alone — only decay (aging with no fresher packet) can bring it to 0.
    fn bars_for(&self, rssi_dbm: i16, snr_db: i8) -> u8 {
        let base = self.rssi_bars(rssi_dbm);
        if base == 0 {
            return 0;
        }
        let mut knockdown = 0u8;
        if snr_db < self.snr_knockdown_db {
            knockdown += 1;
        }
        if snr_db < self.snr_floor_db {
            knockdown += 1;
        }
        base.saturating_sub(knockdown).max(1)
    }
}

impl Default for SignalConfig {
    /// Documented defaults — see `docs/adr/0010-signal-meter.md`'s bars table
    /// and decay curve.
    fn default() -> Self {
        SignalConfig::new([-70, -85, -100, -110, -120], 0, -10, 60_000, 45_000)
    }
}

/// Tracks the strongest recently-heard repeater signal and its arrival time,
/// exposing a decayed "current" level so one lucky strong packet cannot pin
/// full bars indefinitely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalTracker {
    config: SignalConfig,
    /// `0` (no repeater heard / decayed to nothing) or `1..=MAX_BARS`.
    peak_bars: u8,
    /// Arrival time (caller's monotonic ms clock) of `peak_bars`. Decay is
    /// computed from THIS timestamp, not a separate "last packet" time — so a
    /// stronger-or-equal later reading resets both together (see [`record`](Self::record)).
    peak_at_ms: u64,
}

impl SignalTracker {
    /// A tracker with no repeater heard yet — `level()` returns `DirectOnly`
    /// for any `now_ms`, matching device-just-booted behavior.
    pub fn new(config: SignalConfig) -> Self {
        SignalTracker {
            config,
            peak_bars: 0,
            peak_at_ms: 0,
        }
    }

    /// Record one received packet's link quality.
    ///
    /// - `hop_count == 0` is ignored outright: a zero-hop packet came
    ///   straight from its origin companion, not a repeater, and says nothing
    ///   about repeater audibility.
    /// - Otherwise, the reading is mapped to a bar count (RSSI floor + SNR
    ///   knock-down) and compared against the CURRENT decayed level at
    ///   `now_ms` — a stronger-or-equal reading resets both the peak and its
    ///   timestamp (so a repeated/duplicate packet at the same strength still
    ///   refreshes the hold window instead of being discarded as "no
    ///   improvement"). A weaker reading than the current decayed level is
    ///   dropped: the existing peak is still the best evidence of repeater
    ///   audibility until it, too, decays past this new sample.
    pub fn record(&mut self, rssi_dbm: i16, snr_db: i8, hop_count: u8, now_ms: u64) {
        if hop_count == 0 {
            return;
        }
        let candidate = self.config.bars_for(rssi_dbm, snr_db);
        if candidate >= self.decayed_bars(now_ms) {
            self.peak_bars = candidate;
            self.peak_at_ms = now_ms;
        }
    }

    /// The current signal level at `now_ms`: the tracked peak, aged by
    /// max-with-decay (full for `hold_full_ms`, then one bar down every
    /// `decay_step_ms`) — NOT a hard window that snaps straight to empty.
    pub fn level(&self, now_ms: u64) -> SignalLevel {
        match self.decayed_bars(now_ms) {
            0 => SignalLevel::DirectOnly,
            n => SignalLevel::Bars(n),
        }
    }

    /// Shared decay computation used by both `record` (to compare a new
    /// sample against the current state) and `level` (to report it).
    fn decayed_bars(&self, now_ms: u64) -> u8 {
        if self.peak_bars == 0 {
            return 0;
        }
        // Caller-supplied monotonic clock: `now_ms` should never precede
        // `peak_at_ms`, but `saturating_sub` keeps a clock hiccup from
        // panicking or wrapping instead of just reporting "no time elapsed".
        let elapsed = now_ms.saturating_sub(self.peak_at_ms);
        if elapsed <= self.config.hold_full_ms {
            return self.peak_bars;
        }
        let decayed_ms = elapsed - self.config.hold_full_ms;
        // Defense in depth: `SignalConfig::new` already floors `decay_step_ms`
        // at 1, but `bar_floor_dbm`/`decay_step_ms` etc. are `pub` fields, so a
        // `SignalConfig { .. }` struct literal built directly (bypassing
        // `new`'s validation) could still carry a `0` here. Re-floor at the
        // one call site that would otherwise divide by it, rather than trust
        // construction-time validation alone to prevent a panic.
        let decay_step_ms = self.config.decay_step_ms.max(1);
        let steps = (decayed_ms / decay_step_ms).min(self.peak_bars as u64) as u8;
        self.peak_bars - steps
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> SignalTracker {
        SignalTracker::new(SignalConfig::default())
    }

    // ── SignalConfig validation/clamping ─────────────────────────────────

    #[test]
    fn default_config_matches_documented_bars_table() {
        let cfg = SignalConfig::default();
        assert_eq!(cfg.bar_floor_dbm, [-70, -85, -100, -110, -120]);
        assert_eq!(cfg.snr_knockdown_db, 0);
        assert_eq!(cfg.snr_floor_db, -10);
        assert_eq!(cfg.hold_full_ms, 60_000);
        assert_eq!(cfg.decay_step_ms, 45_000);
    }

    #[test]
    fn out_of_order_bar_floors_are_forced_descending() {
        // A degenerate/misconfigured table (not strictly descending) must not
        // silently produce a non-monotonic bars mapping.
        let cfg = SignalConfig::new([-70, -70, -100, -50, -120], 0, -10, 60_000, 45_000);
        for i in 1..cfg.bar_floor_dbm.len() {
            assert!(
                cfg.bar_floor_dbm[i] < cfg.bar_floor_dbm[i - 1],
                "floors must be strictly descending: {:?}",
                cfg.bar_floor_dbm
            );
        }
    }

    #[test]
    fn snr_floor_is_forced_below_knockdown_threshold() {
        let cfg = SignalConfig::new([-70, -85, -100, -110, -120], -5, -5, 60_000, 45_000);
        assert!(cfg.snr_floor_db < cfg.snr_knockdown_db);
    }

    #[test]
    fn zero_decay_step_is_clamped_to_avoid_division_by_zero() {
        let cfg = SignalConfig::new([-70, -85, -100, -110, -120], 0, -10, 60_000, 0);
        assert_eq!(cfg.decay_step_ms, 1);
    }

    #[test]
    fn decay_never_panics_even_if_config_bypasses_new() {
        // `SignalConfig`'s fields are all `pub`, so a caller COULD build one
        // via a raw struct literal instead of `SignalConfig::new`, skipping
        // its validation entirely. `decayed_bars` must not divide by a `0`
        // `decay_step_ms` in that case — this must not panic.
        let cfg = SignalConfig {
            bar_floor_dbm: [-70, -85, -100, -110, -120],
            snr_knockdown_db: 0,
            snr_floor_db: -10,
            hold_full_ms: 60_000,
            decay_step_ms: 0, // bypasses `new`'s floor-at-1 clamp
        };
        let mut t = SignalTracker::new(cfg);
        t.record(-60, 10, 1, 0); // 5 bars at t=0
                                 // Must not panic. A `0` step is re-floored to `1` at the division
                                 // site, so decay is extremely fast (not instantaneous) once past the
                                 // hold window — 5 bars need 5ms of `decayed_ms` to fully empty.
        assert_eq!(t.level(60_000 + 5), SignalLevel::DirectOnly);
    }

    // ── Boot / no-signal state ────────────────────────────────────────────

    #[test]
    fn direct_only_at_boot() {
        let t = tracker();
        assert_eq!(t.level(0), SignalLevel::DirectOnly);
        assert_eq!(t.level(1_000_000), SignalLevel::DirectOnly);
    }

    // ── hop-count gating ──────────────────────────────────────────────────

    #[test]
    fn hop_zero_is_ignored() {
        let mut t = tracker();
        t.record(-60, 5, 0, 0); // strong signal, but zero-hop (direct, not a repeater)
        assert_eq!(t.level(0), SignalLevel::DirectOnly);
    }

    #[test]
    fn hop_one_or_more_records() {
        let mut t = tracker();
        t.record(-60, 5, 1, 0);
        assert_eq!(t.level(0), SignalLevel::Bars(5));
    }

    #[test]
    fn hop_ge_one_duplicate_still_records_and_resets_timer() {
        // A dedup'd duplicate at the SAME strength must still refresh the
        // hold window (decision 6) rather than being discarded as "no
        // improvement" — otherwise repeated identical packets would let the
        // meter decay even while the repeater keeps being heard.
        let mut t = tracker();
        t.record(-60, 5, 1, 0);
        assert_eq!(t.level(59_000), SignalLevel::Bars(5)); // still within hold, unchanged
        t.record(-60, 5, 1, 59_000); // duplicate: same rssi/snr/hop, later time
                                     // Held full for another 60s from the NEW timestamp, i.e. still full
                                     // well past the point the original packet alone would have decayed.
        assert_eq!(t.level(59_000 + 60_000), SignalLevel::Bars(5));
    }

    #[test]
    fn weaker_reading_does_not_override_a_stronger_undecayed_peak() {
        let mut t = tracker();
        t.record(-60, 5, 1, 0); // 5 bars
        t.record(-140, 5, 1, 100); // no repeater audible this packet (below every floor)
                                   // Peak must still be the earlier strong reading, undisturbed.
        assert_eq!(t.level(100), SignalLevel::Bars(5));
    }

    // ── Bars threshold boundaries ─────────────────────────────────────────

    #[test]
    fn bars_threshold_boundaries() {
        let cases: &[(i16, u8)] = &[
            (-60, 5), // well within >= -70
            (-70, 5), // exact >= -70 boundary
            (-71, 4), // just below -70
            (-85, 4), // exact boundary owned by the 4-bar tier
            (-86, 3),
            (-100, 3),
            (-101, 2),
            (-110, 2),
            (-111, 1),
            (-120, 1), // exact boundary owned by the 1-bar tier
            (-121, 0), // below every floor: DirectOnly
        ];
        for &(rssi, expected_bars) in cases {
            let mut t = tracker();
            // Neutral SNR (>= 0) so knock-down never fires here.
            t.record(rssi, 10, 1, 0);
            let expected = if expected_bars == 0 {
                SignalLevel::DirectOnly
            } else {
                SignalLevel::Bars(expected_bars)
            };
            assert_eq!(
                t.level(0),
                expected,
                "rssi={rssi}dBm expected {expected_bars} bars"
            );
        }
    }

    // ── SNR knock-down ─────────────────────────────────────────────────────

    #[test]
    fn negative_snr_knocks_down_one_bar() {
        let mut t = tracker();
        t.record(-60, -1, 1, 0); // would be 5 bars on RSSI alone, snr < 0
        assert_eq!(t.level(0), SignalLevel::Bars(4));
    }

    #[test]
    fn very_negative_snr_near_noise_floor_knocks_down_two_bars() {
        let mut t = tracker();
        t.record(-60, -11, 1, 0); // snr < -10: two knock-downs
        assert_eq!(t.level(0), SignalLevel::Bars(3));
    }

    #[test]
    fn snr_knockdown_never_drops_a_heard_repeater_below_one_bar() {
        // High RSSI but very noisy: RSSI alone says 5 bars, SNR knocks down 2,
        // but a genuinely-heard repeater (hop>=1, clears the weakest floor)
        // must never read as DirectOnly purely from an SNR knock-down.
        let mut t = tracker();
        t.record(-60, -15, 1, 0);
        assert_eq!(t.level(0), SignalLevel::Bars(3));

        // Even at the weakest qualifying RSSI floor, SNR knock-down floors at 1.
        let mut t2 = tracker();
        t2.record(-120, -15, 1, 0);
        assert_eq!(t2.level(0), SignalLevel::Bars(1));
    }

    #[test]
    fn snr_knockdown_never_creates_bars_from_no_signal() {
        // Below every RSSI floor: SNR knock-down must not matter (there is
        // nothing to knock down from).
        let mut t = tracker();
        t.record(-130, 10, 1, 0);
        assert_eq!(t.level(0), SignalLevel::DirectOnly);
    }

    // ── Decay ───────────────────────────────────────────────────────────────

    #[test]
    fn peak_holds_full_for_hold_window_then_decays() {
        let mut t = tracker();
        t.record(-60, 10, 1, 0); // 5 bars at t=0
        assert_eq!(t.level(0), SignalLevel::Bars(5));
        assert_eq!(t.level(59_999), SignalLevel::Bars(5)); // still within hold
        assert_eq!(t.level(60_000), SignalLevel::Bars(5)); // exact edge, still held
        assert_eq!(t.level(60_001), SignalLevel::Bars(5)); // 1ms into decay: not yet a full step
        assert_eq!(t.level(60_000 + 45_000), SignalLevel::Bars(4)); // one full decay step
    }

    #[test]
    fn one_strong_packet_ages_out_to_direct_only_by_about_five_minutes() {
        let mut t = tracker();
        t.record(-60, 10, 1, 0); // 5 bars, no further packets ever heard
        let hold = 60_000u64;
        let step = 45_000u64;
        assert_eq!(t.level(hold + 0 * step), SignalLevel::Bars(5));
        assert_eq!(t.level(hold + 1 * step), SignalLevel::Bars(4));
        assert_eq!(t.level(hold + 2 * step), SignalLevel::Bars(3));
        assert_eq!(t.level(hold + 3 * step), SignalLevel::Bars(2));
        assert_eq!(t.level(hold + 4 * step), SignalLevel::Bars(1));
        assert_eq!(t.level(hold + 5 * step), SignalLevel::DirectOnly); // ~4m45s: within the ~4-5 min spec
                                                                       // Stays DirectOnly indefinitely afterward — decay does not wrap/reset.
        assert_eq!(t.level(hold + 50 * step), SignalLevel::DirectOnly);
    }

    #[test]
    fn a_fresh_strong_packet_during_decay_resets_the_hold_window() {
        let mut t = tracker();
        t.record(-60, 10, 1, 0); // 5 bars at t=0
        let one_step_into_decay = 60_000 + 45_000;
        assert_eq!(t.level(one_step_into_decay), SignalLevel::Bars(4)); // decayed by one step, no new packet yet

        // A fresh equally-strong packet arriving now resets peak+timer back
        // to 5, full hold — the peak ages by ITS OWN timestamp, not the
        // original packet's.
        t.record(-60, 10, 1, one_step_into_decay);
        assert_eq!(t.level(one_step_into_decay), SignalLevel::Bars(5));
        assert_eq!(
            t.level(one_step_into_decay + 60_000),
            SignalLevel::Bars(5) // held full again, counted from the NEW timestamp
        );
    }
}
