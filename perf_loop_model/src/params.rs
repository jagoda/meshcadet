// SPDX-License-Identifier: GPL-3.0-only
//! Sensitivity-range parameters for every per-iteration cost this crate
//! cannot measure on a device (this project's no-hardware-in-the-loop
//! constraint closes flashing, the serial monitor, and QEMU alike — see the
//! crate root doc comment in `lib.rs`). Each field is a cited
//! `[low_ms, high_ms]` range, never a single invented number: a parameter
//! this crate does not have a real measurement for is always a swept range,
//! never a point estimate presented as fact.
//!
//! Two kinds of parameter live here:
//! - **Genuinely unmeasured constants** (GPS/battery poll cost, SPI command
//!   overhead, `ui.step()` cost, frame-encode cost) — real ESP32-S3
//!   wall-clock numbers this container cannot produce (no device, no
//!   emulation). Each range is anchored on an in-repo fact (a duty-cycle
//!   window, a throttle interval, a measured host redraw-scope number) and
//!   the doc comment says exactly where the bound came from.
//! - **In-repo, exact constants** (`RX_POLL_YIELD_MS`, the CAD 20 ms hard
//!   deadline, the room keep-alive cadence) — these are not swept; they are
//!   compiled-in firmware literals cited by file:line, used verbatim.

/// One parameter's sensitivity range, in milliseconds.
///
/// `mid_ms` is normally `None`, meaning [`Corner::Mid`] resolves to the
/// average of `low_ms`/`high_ms` — the sensitivity-sweep meaning documented
/// on [`Corner`]. [`Self::measured`] sets `mid_ms` explicitly instead: see
/// its own doc for why a calibrated field's three corners are NOT that
/// average.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamRangeMs {
    pub low_ms: f64,
    pub high_ms: f64,
    pub mid_ms: Option<f64>,
}

impl ParamRangeMs {
    pub const fn new(low_ms: f64, high_ms: f64) -> Self {
        Self {
            low_ms,
            high_ms,
            mid_ms: None,
        }
    }

    /// A MEASURED (not swept) point, built from a device report's phase
    /// rollup: `mean_ms` -> [`Corner::Low`], `p95_ms` -> [`Corner::Mid`],
    /// `max_ms` -> [`Corner::High`]. This is [`crate::calibration::
    /// calibrate`]'s hook for the `ui_step`/`gps_poll`/`battery_poll`
    /// fields per `docs/perf/collection-kit.md` Part D's derivation table
    /// ("replace the whole range with these three points") — the three
    /// corners read as real percentiles of one measured distribution here,
    /// not as a low/high sensitivity extreme with an averaged midpoint.
    pub const fn measured(mean_ms: f64, p95_ms: f64, max_ms: f64) -> Self {
        Self {
            low_ms: mean_ms,
            high_ms: max_ms,
            mid_ms: Some(p95_ms),
        }
    }

    /// Resolve this range at `corner`.
    pub fn at(&self, corner: Corner) -> f64 {
        match corner {
            Corner::Low => self.low_ms,
            Corner::High => self.high_ms,
            Corner::Mid => self.mid_ms.unwrap_or((self.low_ms + self.high_ms) / 2.0),
        }
    }
}

/// Which end of every swept [`ParamRangeMs`] a simulation run resolves
/// against. A run always applies ONE corner uniformly across every unknown
/// parameter — not an independent per-parameter factorial sweep — because
/// the question this model exists to answer (does radio-TX blocking
/// dominate the UI-unserviced gap across the full plausible range?) only
/// needs the two extremes bounded: [`Corner::Low`] is the most favorable
/// case for a busy UI (every unknown cost minimal), [`Corner::High`] is the
/// most ADVERSARIAL case against the dominance claim (every non-radio cost
/// maximal, so if radio still dominates there, it dominates everywhere in
/// between). [`Corner::Mid`] gives a representative headline number. A finer
/// per-parameter combinatorial sweep is future work if a later checkpoint
/// needs it — `sim::tests::dominance_holds_across_every_corner_for_the_
/// smallest_payload` confirms the three corners already answer the
/// dominance question this M0 gate needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    Low,
    Mid,
    High,
}

impl Corner {
    pub const ALL: [Corner; 3] = [Corner::Low, Corner::Mid, Corner::High];
}

/// The full set of swept + fixed per-iteration cost parameters, documented
/// against the dispatcher loop's phase order (see the crate root doc's
/// "What to model" summary, verified against `firmware/src/main.rs::run()`'s
/// `loop {}` at ~line 1784).
#[derive(Debug, Clone, Copy)]
pub struct LoopModelParams {
    /// `esp_task_wdt_reset()` (`firmware/src/main.rs:1791`) — one FFI call
    /// that performs a single register/atomic write in the target's
    /// watchdog driver. Not a bus transaction of any kind. No on-device
    /// number exists; bounded generously above a sub-microsecond floor.
    pub wdt_pet: ParamRangeMs,

    /// `gps.poll(now)` (`firmware/src/main.rs:1794`, pure duty-cycle math in
    /// `firmware_core::gps`). Most iterations land in the QUIET window
    /// (`GPS_QUIET_INTERVAL_MS` = 120 000 ms out of a ~150 000 ms cycle —
    /// `firmware-core/src/gps.rs:196`) where this is a cheap state check
    /// with NO UART I/O. During the 30 000 ms ACTIVE window
    /// (`GPS_ACTIVE_WINDOW_MS`, `firmware-core/src/gps.rs:193`) it reads an
    /// NMEA sentence over UART1 at 9600–115200 bps
    /// (`GPS_BAUD_CANDIDATES`, `firmware-core/src/gps.rs:29`). No on-device
    /// timing exists for either case (no hardware in the loop for this
    /// model — see the crate root doc); swept from a gate-only tick up to a
    /// conservative bounded NMEA-line-read ceiling.
    pub gps_poll: ParamRangeMs,

    /// TX-timestamp rebase against the GPS-synced wall clock
    /// (`firmware/src/main.rs:1817-1834`) — a handful of comparisons and
    /// `wrapping_sub`/`wrapping_add` `u32` ops, no I/O. Negligible but not
    /// zero.
    pub tx_timestamp_rebase: ParamRangeMs,

    /// `battery.poll(now)` (`firmware/src/main.rs:1853`) — throttled ADC
    /// read, `BATTERY_POLL_INTERVAL_MS = 2_000` ms
    /// (`firmware/src/battery.rs:41`). Most iterations are the throttled
    /// no-op branch; every 2 s it takes a real ADC sample. No on-device ADC
    /// timing exists; swept from a throttle-check floor to a conservative
    /// ADC-sample ceiling.
    pub battery_poll: ParamRangeMs,

    /// Per-room keep-alive SCHEDULER check (`firmware/src/main.rs:1960` ff)
    /// — pure in-memory comparisons over `room_runtime` (typically 0-4
    /// entries), NOT the encode+enqueue cost when a keep-alive actually
    /// fires (see [`Self::frame_encode`]). Cheap; swept generously anyway
    /// since no host number exists.
    pub room_keepalive_sched_check: ParamRangeMs,

    /// SPI command overhead ahead of a CAD attempt
    /// (`SetStandby`/`SetCadParams`/`ClearIrq`/`SetDioIrqParams`/`SetCad`,
    /// `firmware/src/radio.rs:415-464`) — several synchronous `write_cmd`
    /// round-trips over the 8 MHz radio SPI bus (`main.rs:1401`). No
    /// per-transaction number exists for this bus/speed (§4's ~128 µs/line
    /// floor is the 40 MHz DISPLAY bus, a different device). Bounded above
    /// by [`crate::sim::CAD_HARD_DEADLINE_MS`] minus the analytically
    /// computed 4-symbol CAD-active time
    /// ([`crate::sim::CAD_ACTIVE_MS`]) — see that constant's doc for why
    /// the two together can never exceed the real code's own 20 ms poll
    /// deadline (`firmware/src/radio.rs:468`).
    pub cad_spi_overhead: ParamRangeMs,

    /// Crypto/encode cost for ONE outbound frame this model generates (DM
    /// ACK, GRP_TXT, room keep-alive, room login) — ECDH shared-secret
    /// derivation + AEAD encode, e.g. `room_session::encode_room_keep_
    /// alive_frame` (`firmware/src/main.rs:2216`). No on-device crypto
    /// timing exists for this target (ESP32-S3 @ 240 MHz, crypto
    /// accelerator usage here unconfirmed); this is a handful of curve
    /// operations — orders of magnitude below the radio airtime this crate
    /// exists to compare against regardless of the exact figure, but
    /// genuinely unmeasured, so it is swept rather than assumed negligible.
    pub frame_encode: ParamRangeMs,

    /// Periodic RX-stats / stack-HWM log, gated at `RX_STATS_INTERVAL_MS =
    /// 30_000` ms (`firmware/src/main.rs:1622`) — a handful of `log::info!`
    /// calls and a stack high-water-mark read, paid once per 30 s, not
    /// every iteration.
    pub periodic_stats: ParamRangeMs,

    /// `ui.step()` (`firmware/src/main.rs:2593`) — I2C1 touch + keyboard
    /// poll, Slint tick, and a conditional `render_if_needed`. No on-device
    /// number exists for the I2C1 poll itself. The render half IS
    /// host-measured: `docs/perf/ui-perf-baseline.md` §3b/§10 give a real
    /// idle no-op (0 ms) up to a ~30.72 ms DATA-ONLY SPI floor for a full
    /// 240-line repaint (§4.1: 240 lines × 640 B/line × 8 bits / 40 MHz —
    /// corrected from an earlier ~3.1 ms figure that mistook the
    /// `display-interface-spi` 64-byte chunk time for the whole line) — a
    /// floor, not a ceiling, since it excludes undocumented per-transaction
    /// command overhead. Range: a low bound near the measured idle no-op
    /// plus a minimal I2C1-poll floor, to a high bound set at the measured
    /// full-repaint data floor itself — the undocumented per-transaction
    /// command overhead (§4.1, [DEFERRED-DEVICE] D2) is real but unquantified
    /// headroom on top of this, not folded into this range.
    pub ui_step: ParamRangeMs,

    /// Drain `UiCommand` / handle events (`firmware/src/main.rs`, tail of
    /// the loop) — a bounded pop-and-match over whatever `ui.step()` just
    /// produced. Small, non-zero.
    pub drain_ui_command: ParamRangeMs,

    /// Split-topology ONLY: the UI task's own idle-loop granularity once it
    /// is decoupled from the radio/dispatcher task (the proposed M1 split,
    /// not yet implemented — see the crate root doc's "Two topologies, one
    /// harness" section). A cooperatively-yielding task still has SOME
    /// minimal loop granularity
    /// — this repo sets no `CONFIG_FREERTOS_HZ` override
    /// (`grep -rn FREERTOS_HZ sdkconfig.defaults` is empty), so ESP-IDF's
    /// documented default of 100 Hz (10 ms tick) applies if the eventual
    /// implementation waits on a polling `vTaskDelay`. Swept from 0 (a
    /// queue/notification-driven wait can in principle render on the very
    /// next scheduler pass with no forced delay) up to the full 10 ms tick
    /// (the conservative case: a `vTaskDelay(1)`-style poll loop instead of
    /// a notify-driven wait). This parameter does not exist in the current
    /// single-superloop topology at all.
    pub split_ui_idle_tick: ParamRangeMs,
}

impl LoopModelParams {
    /// The documented default range set — see each field's doc comment for
    /// its citation. This is the ONLY constructor; there is no "just pick a
    /// number" path in this crate.
    pub const fn documented_defaults() -> Self {
        Self {
            wdt_pet: ParamRangeMs::new(0.0, 0.01),
            gps_poll: ParamRangeMs::new(0.0, 2.0),
            tx_timestamp_rebase: ParamRangeMs::new(0.0, 0.01),
            battery_poll: ParamRangeMs::new(0.0, 1.0),
            room_keepalive_sched_check: ParamRangeMs::new(0.0, 0.05),
            cad_spi_overhead: ParamRangeMs::new(0.0, 11.8), // see resolve()'s cap
            frame_encode: ParamRangeMs::new(0.0, 2.0),
            periodic_stats: ParamRangeMs::new(0.0, 0.5),
            ui_step: ParamRangeMs::new(0.05, 30.72),
            drain_ui_command: ParamRangeMs::new(0.0, 0.05),
            split_ui_idle_tick: ParamRangeMs::new(0.0, 10.0),
        }
    }

    /// Resolve every field at `corner` into a plain [`ResolvedParams`] —
    /// done once per simulation run rather than re-resolving per iteration.
    pub fn resolve(&self, corner: Corner) -> ResolvedParams {
        // `cad_spi_overhead` is capped so `CAD_ACTIVE_MS + overhead` can
        // never exceed the real code's own 20 ms poll deadline
        // (`firmware/src/radio.rs:468`) — see that constant's doc in
        // `sim.rs`. The stored range's high bound is already set to exactly
        // that headroom, but the cap is applied here too so a future edit
        // to either constant cannot silently desync the two.
        let cad_overhead_cap =
            (crate::sim::CAD_HARD_DEADLINE_MS - crate::sim::CAD_ACTIVE_MS).max(0.0);
        ResolvedParams {
            wdt_pet: self.wdt_pet.at(corner),
            gps_poll: self.gps_poll.at(corner),
            tx_timestamp_rebase: self.tx_timestamp_rebase.at(corner),
            battery_poll: self.battery_poll.at(corner),
            room_keepalive_sched_check: self.room_keepalive_sched_check.at(corner),
            cad_spi_overhead: self.cad_spi_overhead.at(corner).min(cad_overhead_cap),
            frame_encode: self.frame_encode.at(corner),
            periodic_stats: self.periodic_stats.at(corner),
            ui_step: self.ui_step.at(corner),
            drain_ui_command: self.drain_ui_command.at(corner),
            split_ui_idle_tick: self.split_ui_idle_tick.at(corner),
            corner,
        }
    }
}

impl Default for LoopModelParams {
    fn default() -> Self {
        Self::documented_defaults()
    }
}

/// [`LoopModelParams`] resolved at one [`Corner`] — plain `f64` milliseconds,
/// ready for the simulator's hot loop.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedParams {
    pub wdt_pet: f64,
    pub gps_poll: f64,
    pub tx_timestamp_rebase: f64,
    pub battery_poll: f64,
    pub room_keepalive_sched_check: f64,
    pub cad_spi_overhead: f64,
    pub frame_encode: f64,
    pub periodic_stats: f64,
    pub ui_step: f64,
    pub drain_ui_command: f64,
    pub split_ui_idle_tick: f64,
    pub corner: Corner,
}

impl ResolvedParams {
    /// Sum of every phase cost that is NOT CAD/TX/RX-poll — the "fixed
    /// per-iteration overhead" paid every iteration regardless of traffic:
    /// WDT pet, GPS poll, tx-timestamp rebase, battery poll, room
    /// keep-alive scheduler check.
    pub fn fixed_phase_cost_ms(&self) -> f64 {
        self.wdt_pet
            + self.gps_poll
            + self.tx_timestamp_rebase
            + self.battery_poll
            + self.room_keepalive_sched_check
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_low_le_mid_le_high_for_every_field() {
        let p = LoopModelParams::documented_defaults();
        for range in [
            p.wdt_pet,
            p.gps_poll,
            p.tx_timestamp_rebase,
            p.battery_poll,
            p.room_keepalive_sched_check,
            p.cad_spi_overhead,
            p.frame_encode,
            p.periodic_stats,
            p.ui_step,
            p.drain_ui_command,
            p.split_ui_idle_tick,
        ] {
            let low = range.at(Corner::Low);
            let mid = range.at(Corner::Mid);
            let high = range.at(Corner::High);
            assert!(low <= mid, "{:?}: low > mid", range);
            assert!(mid <= high, "{:?}: mid > high", range);
            assert!(low >= 0.0, "{:?}: negative low bound", range);
        }
    }

    #[test]
    fn cad_overhead_never_pushes_cad_phase_past_the_real_20ms_deadline() {
        let p = LoopModelParams::documented_defaults();
        for corner in Corner::ALL {
            let r = p.resolve(corner);
            let total = crate::sim::CAD_ACTIVE_MS + r.cad_spi_overhead;
            assert!(
                total <= crate::sim::CAD_HARD_DEADLINE_MS + 1e-9,
                "corner {:?}: CAD phase {} ms exceeds the real 20 ms deadline",
                corner,
                total,
            );
        }
    }

    #[test]
    fn fixed_phase_cost_is_small_relative_to_the_smallest_airtime_block() {
        // Sanity bound, not the dominance proof itself (that lives in
        // sim.rs's tests) — even summed at the HIGH corner, the small
        // per-iteration overheads should be a small fraction of a single
        // ACK-shaped (10 B) TX block, which is what makes the dominance
        // question interesting to ask at all.
        let p = LoopModelParams::documented_defaults();
        let r = p.resolve(Corner::High);
        let ack_airtime = firmware_core::dispatcher::lora_airtime_ms(10) as f64;
        assert!(
            r.fixed_phase_cost_ms() < ack_airtime,
            "fixed per-iteration overhead ({} ms) should be well under even the \
             smallest ACK-shaped airtime block ({} ms)",
            r.fixed_phase_cost_ms(),
            ack_airtime,
        );
    }
}
