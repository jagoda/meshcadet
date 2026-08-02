// SPDX-License-Identifier: GPL-3.0-only
//! Host discrete-event model of the firmware dispatcher superloop — part of
//! the ongoing radio/UI performance investigation (see the "dominant
//! finding" summary below), built under a deliberate **no hardware-in-the-
//! loop** constraint: no flashing, no serial monitor, no peer node, and no
//! QEMU (Espressif's fork models no general-purpose SPI2 slave devices — no
//! ST7789, no SX1262 — and is documented non-cycle-accurate, so it cannot
//! stand in for real timing here). This crate is the baseline producer for
//! that investigation and runs under `cargo test --workspace`, on the host,
//! in CI, with no device at all.
//!
//! **The dominant finding this investigation is measuring against:**
//! `radio.transmit()` (`firmware/src/radio.rs:276`) blocks the dispatcher
//! loop for the FULL LoRa airtime (83–800 ms depending on payload size),
//! and `ui.step()` — the only place touch/keyboard/render happen — runs
//! after it in the same loop, so the UI is not merely slow during that
//! window, it is not sampled at all. Whether that is the real bottleneck,
//! and whether decoupling UI onto its own task materially fixes it, is
//! exactly what this crate's sensitivity sweep exists to answer before any
//! firmware change is written.
//!
//! # Why this measurement is representative, not a guess
//!
//! The entire radio-timing state machine already lives in `firmware-core`,
//! a root-workspace crate that compiles and tests on the host:
//! [`firmware_core::dispatcher::lora_airtime_ms`] (exact LoRa airtime from
//! payload size at the locked SF7 / BW 62.5 kHz / CR 4:5 preset — the
//! preset `firmware/src/radio.rs` programs into the SX1262),
//! [`firmware_core::dispatcher::TxQueue`], and
//! [`firmware_core::dispatcher::AirtimeBudget`]. This crate CALLS those
//! real functions/types for every simulated CAD+TX phase — it does not
//! reimplement or approximate the frame-queueing, duty-cycle-budget, or
//! airtime-formula logic. This is the same discipline `ui_sim::
//! perf_profile` establishes for the UI half of this pass's
//! predecessor: drive the REAL renderer instead of modelling its
//! dirty-region decisions (`ui_sim/src/perf_profile.rs`'s own module doc is
//! the house style this doc follows). `firmware_core::dispatcher::
//! DuplicateFilter` is deliberately NOT invoked: dedup governs which
//! packets get relayed/suppressed, not per-iteration TIMING (the dispatcher
//! loop's own documented phase order, replayed below, does not list a
//! "dedup check" as a timed phase either), and its cost — a fixed-size
//! 128-slot array compare — is orders of magnitude below every other cost
//! this model already charges, so folding it in would not move any number
//! this crate reports.
//!
//! On top of those real state machines, [`sim`] replays the dispatcher
//! loop's documented per-iteration phase order (verified against
//! `firmware/src/main.rs::run()`'s `loop {}` at ~line 1784):
//!
//! ```text
//! WDT pet → GPS poll → tx-timestamp rebase → battery poll → room keep-alive
//!   → CAD + TX (SPI cmds + DIO1 poll <=20 ms; then radio.transmit() blocks
//!               for FULL AIRTIME — firmware/src/radio.rs:276, the
//!               delay_ms(1) spin)
//!   → RX poll (DIO1 watch <=5 ms) → periodic stats → ui.step()
//!   → drain UiCommand
//! ```
//!
//! for a parameterised traffic [`workload::Workload`] (inbound DM rate —
//! each arrival enqueues an ACK, an independent GRP_TXT stream, the real
//! room keep-alive cadence, and a swept payload-size axis), reporting the
//! **UI-unserviced-gap distribution**: longest gap, p95, mean, cumulative
//! unserviced time, and UI service cadence — for both today's single
//! superloop topology and the proposed M1 split (see [`sim::Topology`]).
//!
//! # What this does NOT measure
//!
//! - **Real ESP32-S3 wall-clock** for GPS poll, battery poll, SPI command
//!   overhead, and `ui.step()` cost. No device and deliberately no emulation
//!   either: Espressif's QEMU fork models no general-purpose SPI2 slave
//!   devices (no ST7789, no SX1262), no DIO1/BUSY GPIO semantics, and is
//!   documented non-cycle-accurate, so it cannot stand in for real timing
//!   here. Every one of these enters as an explicit, cited
//!   [`params::ParamRangeMs`] swept across a [`params::Corner`], never a
//!   single invented number — see `params.rs` for every range and exactly
//!   where its bound came from.
//! - **SPI2 bus arbitration / contention behaviour.** Whether the LCD and
//!   radio's two `SpiDeviceDriver`s on one shared bus actually serialise
//!   correctly under the split topology is a separate, dedicated
//!   source-and-datasheet analysis, not this crate's job — this model
//!   always treats CAD as finding the channel clear (see
//!   `sim::attempt_cad_tx`'s doc), which is also the conservative
//!   (worst-for-UI-starvation) modelling choice.
//! - **Real per-transaction SPI command overhead** beyond the analytically
//!   computed CAD-symbol time (`sim::CAD_ACTIVE_MS`) — `docs/perf/ui-perf-
//!   baseline.md` §4's ~128 µs/line figure is the 40 MHz DISPLAY bus, not
//!   the 8 MHz radio bus, so it is not reused here; the unknown remainder
//!   is `params::LoopModelParams::cad_spi_overhead`, a swept range.
//! - **Packet loss / retry storms / CAD-busy collisions** under real RF
//!   conditions — out of scope for a TIMING model of one node's own loop.
//!
//! # Determinism
//!
//! Traffic arrivals are evenly spaced at each stream's configured interval,
//! not drawn from a PRNG (see `workload.rs`'s "Determinism" note) — the
//! same principle `ui_perf::Harness::advance` uses a manually stepped clock
//! for instead of wall-clock: a given (topology, params-corner, workload)
//! combination always produces the identical event sequence and therefore
//! the identical longest-gap/p95 numbers, run to run. `cargo test -p
//! perf_loop_model` is reproducible byte-for-byte, not noisy.
//!
//! # Two topologies, one harness
//!
//! [`sim::Topology::SingleLoop`] models today's shipped superloop.
//! [`sim::Topology::Split`] models the proposed M1 split (UI on its own
//! task/core, radio+dispatcher on core 0, message queues across the
//! boundary) — NOT YET IMPLEMENTED in firmware. This gives a **predicted**
//! delta before a line of firmware changes, and is meant to become the
//! permanent regression harness a later host-validation pass re-runs
//! against the as-built topology once the split actually lands.
//!
//! # Every number this crate prints is SIMULATED, never a device measurement
//!
//! "Simulated" is a distinct provenance from a real device measurement or
//! an analytical estimate, and must be labelled as such wherever it
//! appears — never presented as a device measurement. See
//! `docs/perf/perf-loop-model-baseline.md`'s own provenance banner.

pub mod params;
pub mod report;
pub mod sim;
pub mod workload;

pub use params::{Corner, LoopModelParams, ParamRangeMs, ResolvedParams};
pub use sim::{dominance_check, simulate, DominanceVerdict, GapStats, SimResult, Topology};
pub use workload::{TrafficStream, Workload};
