// SPDX-License-Identifier: GPL-3.0-only
//! Parameterised traffic workload the loop model replays against the
//! dispatcher's real state machines (this milestone's scope: "inbound DM
//! rate, each inbound DM enqueuing an ACK, GRP_TXT, room keep-alive cadence,
//! payload-size mix").
//!
//! Arrivals are DETERMINISTIC — evenly spaced at each stream's configured
//! interval, not drawn from a PRNG — for the same reason
//! `ui_perf::Harness::advance` uses a manually stepped clock instead of
//! wall-clock: a given workload always produces the identical event
//! sequence run-to-run, so a longest-gap or p95 number is reproducible, not
//! a roll of the dice. See `crate` module doc's "Determinism" section.

/// One traffic stream: fires every `interval_ms`, enqueuing a frame of
/// `payload_bytes`. `None` disables the stream entirely (used by
/// [`Workload::idle`] to measure the per-iteration overhead FLOOR with zero
/// radio activity — see `crate::sim`'s dominance check).
#[derive(Debug, Clone, Copy)]
pub struct TrafficStream {
    pub interval_ms: Option<f64>,
    pub payload_bytes: usize,
}

impl TrafficStream {
    pub const fn disabled() -> Self {
        Self {
            interval_ms: None,
            payload_bytes: 0,
        }
    }

    pub const fn every(interval_ms: f64, payload_bytes: usize) -> Self {
        Self {
            interval_ms: Some(interval_ms),
            payload_bytes,
        }
    }
}

/// The full traffic mix for one simulation run.
#[derive(Debug, Clone, Copy)]
pub struct Workload {
    /// Inbound DMs — each arrival is handled inline during the RX-poll
    /// phase (mirrors `firmware/src/main.rs`'s `handle_dm` call site, which
    /// runs synchronously inside the `Ok(Some(n))` RX-poll match arm) and
    /// enqueues one ACK frame for a LATER iteration's CAD+TX phase to
    /// drain — CAD+TX runs BEFORE RX poll in the documented phase order, so
    /// an ACK generated this iteration cannot be sent until the next one,
    /// same as the real loop.
    pub inbound_dm: TrafficStream,
    /// Own-initiated outbound GRP_TXT sends — user chat activity, not
    /// reactive to any inbound event. Checked alongside the room
    /// keep-alive scheduler, before CAD+TX, in this model.
    pub grp_txt: TrafficStream,
    /// Room keep-alive — cadence-based, not rate-based, mirroring
    /// `room_session::room_keep_alive_interval_ms`'s real scheduling. The
    /// routine (post-drain) cadence is 5 minutes (`firmware/src/main.rs:421`,
    /// "Phase C keep-alive cadence: 5 minutes (300_000 ms)") — used here
    /// as the in-repo, EXACT constant it is, not swept.
    pub room_keepalive: TrafficStream,
}

/// Real in-repo constant, not swept: `firmware/src/main.rs:421`'s "Phase C
/// keep-alive cadence: 5 minutes (300_000 ms)".
pub const ROOM_KEEPALIVE_ROUTINE_INTERVAL_MS: f64 = 300_000.0;

/// Real in-repo constant, not swept: `RX_POLL_YIELD_MS = 20`
/// (`firmware/src/main.rs:1748`) — the DIO1-watch yield window `radio.
/// try_receive` polls for. This model conservatively charges the FULL
/// window every iteration (worst case for the RX-poll phase's contribution
/// to the UI-unserviced gap), since the real function returns early only on
/// a DIO1 edge and this model does not simulate sub-window packet-arrival
/// timing.
///
/// **Retuned 5 -> 20 by `meshcadet-perf-radio-dio1-interrupt`, re-anchored
/// here by its sibling `meshcadet-perf-radio-host-validation`** (this is the
/// tool `main.rs:1744`'s own retune comment names as "the tool that
/// measures this window's actual effect"). The retune's justification
/// (`main.rs:1736-1747`) is that `try_receive`'s DIO1 wait moved from a
/// `FreeRtos::delay_ms(1)` spin (up to 5 separate 1 ms sleep/wake cycles to
/// find nothing) to one interrupt/notification-driven blocking wait (one
/// wake), so widening the window trades nothing this model tracks: none of
/// GPS poll/battery poll/room keep-alive/the 30 s RX-stats rollup are gated
/// on wall-clock elapsed time rather than iteration count. Every published
/// number in `docs/perf/perf-loop-model-baseline.md` and `docs/perf/
/// task-split-host-validation.md` that depended on the OLD 5 ms value is
/// stale as of this change — both documents carry an in-place correction
/// note pointing here, per their own `ui-perf-baseline.md`-derived §9-style
/// convention, rather than being silently wrong for a post-M2 ref.
pub const RX_POLL_YIELD_MS: f64 = 20.0;

impl Workload {
    /// No traffic at all — every stream disabled. Used to measure the
    /// per-iteration overhead FLOOR (WDT/GPS/battery/room-sched/RX-poll/
    /// stats/ui.step()/drain, with zero radio activity) that the dominance
    /// check compares the smallest real airtime block against.
    pub const fn idle() -> Self {
        Self {
            inbound_dm: TrafficStream::disabled(),
            grp_txt: TrafficStream::disabled(),
            room_keepalive: TrafficStream::disabled(),
        }
    }

    /// The headline payload-size sweep scenario: an "active conversation"
    /// mesh (inbound DM every 5 s — a representative busy-chat rate, a
    /// WORKLOAD SCENARIO CHOICE this crate documents explicitly rather than
    /// treating as a measured constant), a background GRP_TXT every 20 s,
    /// and the real routine room-keep-alive cadence — with the inbound
    /// DM's ACK sized at `payload_bytes`, the dimension this crate's
    /// payload-size sweep varies (10 B ACK-shaped through 255 B).
    pub const fn payload_sweep(payload_bytes: usize) -> Self {
        Self {
            inbound_dm: TrafficStream::every(5_000.0, payload_bytes),
            grp_txt: TrafficStream::every(20_000.0, 60),
            room_keepalive: TrafficStream::every(ROOM_KEEPALIVE_ROUTINE_INTERVAL_MS, 9),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_workload_disables_every_stream() {
        let w = Workload::idle();
        assert!(w.inbound_dm.interval_ms.is_none());
        assert!(w.grp_txt.interval_ms.is_none());
        assert!(w.room_keepalive.interval_ms.is_none());
    }

    #[test]
    fn payload_sweep_varies_only_the_inbound_dm_payload() {
        let small = Workload::payload_sweep(10);
        let large = Workload::payload_sweep(255);
        assert_eq!(small.inbound_dm.payload_bytes, 10);
        assert_eq!(large.inbound_dm.payload_bytes, 255);
        assert_eq!(small.grp_txt.payload_bytes, large.grp_txt.payload_bytes);
        assert_eq!(
            small.room_keepalive.payload_bytes,
            large.room_keepalive.payload_bytes
        );
        assert_eq!(small.inbound_dm.interval_ms, large.inbound_dm.interval_ms);
    }
}
