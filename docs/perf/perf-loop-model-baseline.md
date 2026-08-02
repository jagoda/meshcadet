# Dispatcher superloop model — SIMULATED baseline (M0, no hardware in the loop)

**Status update, M1 landed:** the `split` topology this document predicted
(§2) has since been implemented (ADR-0012 /
`meshcadet-perf-ui-task-split`, `firmware/src/ui_task.rs`). Every number
below is preserved verbatim as the historical **M0 prediction** — nothing
here is retracted or wrong, per this repo's correction convention
(`ui-perf-baseline.md` §9: edit the body only for a retracted/wrong number,
not for a milestone landing). The **as-built** re-run, re-parameterised to
the real `ui_task.rs` constants
(`UI_TICK_MS`, the new `queue_handoff` cost for the `std::sync::mpsc`
boundary) and diffed against the prediction below, lives in
`docs/perf/task-split-host-validation.md`.

**Every number in this document is SIMULATED.** It comes from
`perf_loop_model`, a host discrete-event model of the firmware dispatcher
superloop, not from a flashed device, not from an emulator, and not from an
analytical formula alone. "Simulated" is a distinct provenance from a real
device measurement or a pure analytical estimate (`docs/perf/ui-perf-
baseline.md` §4's airtime/SPI-floor tables) and must never be read as
either. Where this document leans on an in-repo analytical fact (e.g. the
exact LoRa airtime formula) it says so explicitly; everything else here is
this crate's own discrete-event replay.

This is part of the same investigation `docs/perf/ui-perf-baseline.md`
started: `radio.transmit()` (`firmware/src/radio.rs:276`) blocks the
dispatcher loop for the FULL LoRa airtime (83–800 ms depending on payload
size), and `ui.step()` — the only place touch/keyboard/render happen — runs
after it in the SAME loop, so the UI is not merely slow during that window,
it is not sampled at all. That earlier document flagged this as "out of
scope" for a UI-only optimization pass (§5 item 6) and separately as the
dominant contention mechanism worth a structural fix (§7). This document is
the baseline that decides whether decoupling the UI onto its own task
materially fixes it, BEFORE any firmware change is written.

## 1. Why no device, and why this is still a real measurement

The device side of this investigation carries a **no hardware-in-the-loop**
constraint: no flashing, no serial monitor, no second peer node, and
deliberately no QEMU either — Espressif's QEMU fork models no
general-purpose SPI2 slave devices (no ST7789, no SX1262), no DIO1/BUSY GPIO
semantics, and is documented non-cycle-accurate ("your 1 second delay is now
4–5 seconds"), so it cannot stand in for real dispatcher-loop timing.

What makes a host-side model worth trusting anyway: the entire radio-timing
state machine already lives in `firmware-core`, a root-workspace crate that
compiles and tests on the host with no `esp-idf`/Slint dependency —
`firmware_core::dispatcher::lora_airtime_ms` (exact LoRa airtime from
payload size at the locked SF7 / BW 62.5 kHz / CR 4:5 preset), `TxQueue`
(the real FIFO pending-TX queue, including its drop-oldest-when-full
behaviour), and `AirtimeBudget` (the real 10 %-duty-cycle sliding-window
enforcer). `perf_loop_model` **calls these real functions and types** for
every simulated CAD+TX phase — it does not reimplement or approximate the
frame-queueing, duty-cycle-budget, or airtime-formula logic. This is the
same discipline `ui_sim::perf_profile` established for the UI half of this
investigation: drive the real thing instead of modelling its decisions.

On top of those real state machines, the model replays the dispatcher
loop's documented per-iteration phase order (verified against
`firmware/src/main.rs::run()`'s `loop {}` at ~line 1784):

```
WDT pet → GPS poll → tx-timestamp rebase → battery poll → room keep-alive
  → CAD + TX (SPI cmds + DIO1 poll <=20 ms; then radio.transmit() blocks
              for FULL AIRTIME — firmware/src/radio.rs:276, the delay_ms(1)
              spin)
  → RX poll (DIO1 watch <=5 ms) → periodic stats → ui.step() → drain
    UiCommand
```

Every real ESP32-S3 wall-clock number this container cannot produce (GPS
poll cost, battery poll cost, radio-SPI command overhead, `ui.step()` cost)
enters as an explicit, cited **sensitivity range**, never an invented point
estimate — see `perf_loop_model/src/params.rs` for every range and exactly
where its bound came from (a duty-cycle window, a throttle interval, a
measured host redraw-scope number, or a documented hard deadline). Every
simulation run below is repeated at three corners of that range: `low`
(every unknown minimal — most favorable to a busy UI), `high` (every
unknown maximal — the adversarial case AGAINST the dominance claim below),
and `mid` (a representative headline number).

Full module design doc, "what this does NOT measure", and the determinism
argument: `perf_loop_model/src/lib.rs`.

## 2. What is modelled — two topologies, one harness

- **`single-loop` (current)** — today's shipped topology: radio + UI in one
  task, one core. `ui.step()` runs once per dispatcher-loop iteration, after
  CAD+TX and RX poll.
- **`split` (proposed M1)** — the proposed task/core split (UI on its own
  task/core, radio+dispatcher on core 0, message queues across the
  boundary) — **NOT YET IMPLEMENTED in firmware at the time this document
  was written.** This was a *predicted* delta, not a measurement of real
  code, and was labelled as such throughout. **It has since been built**
  (ADR-0012 / `meshcadet-perf-ui-task-split`); the as-built re-run and its
  diff against the prediction below live in
  `docs/perf/task-split-host-validation.md`. The numbers below are kept
  unchanged as the historical M0 prediction.

Traffic workload (`perf_loop_model::workload::Workload::payload_sweep`): an
"active conversation" scenario — inbound DM every 5 s (each arrival decodes
and enqueues one ACK, sized at the sweep's payload axis), a background
GRP_TXT every 20 s (60 B), and the real routine room keep-alive cadence
(`firmware/src/main.rs:421`, "Phase C keep-alive cadence: 5 minutes
(300 000 ms)", 9 B). Arrivals are deterministic (evenly spaced, not
PRNG-drawn) so every number below is reproducible byte-for-byte, run to run.
The payload axis sweeps the same four sizes `docs/perf/ui-perf-baseline.md`
§4 uses: 10 B (ACK-shaped), 40 B (typical DM), 100 B, 255 B (max frame).

## 3. Reproduce

```sh
cargo test -p perf_loop_model                                # 19 tests — correctness + regression guards
cargo run  -p perf_loop_model --release --bin loop_model_report  # the report below
```

## 4. SIMULATED — dominance check (the abort/reroute question)

This is the milestone's own reroute condition: does a single radio-TX
block, **by itself**, already exceed the WORST UI-unserviced gap achievable
from routine per-iteration overhead alone (WDT/GPS/battery/room-sched/
RX-poll/stats/drain — `ui.step()`'s own duration is excluded by
construction, since the gap is measured up to the start of the next
`ui.step()` call, not across it) with **zero radio traffic at all**? If not
— at every corner of the sensitivity range — later structural work should
reroute to local UI-side optimization instead of a task/core split.

```
=== perf_loop_model — M0 SIMULATED baseline ===
no device, no HIL, no QEMU — host discrete-event model over real firmware-core state machines

-- dominance check (abort/reroute condition): does a single radio-TX
   block, alone, exceed the WORST UI-unserviced gap achievable with ZERO radio
   traffic at all? --
corner    payload_B     airtime_ms    idle_floor_gap_ms    ratio_x  dominates
low              10             83                5.000       16.6       true
low              40            165                5.000       33.0       true
low             100            349                5.000       69.8       true
low             255            800                5.000      160.0       true
mid              10             83                6.810       12.2       true
mid              40            165                6.810       24.2       true
mid             100            349                6.810       51.2       true
mid             255            800                6.810      117.5       true
high             10             83                8.620        9.6       true
high             40            165                8.620       19.1       true
high            100            349                8.620       40.5       true
high            255            800                8.620       92.8       true
```

**Verdict: dominance holds at every corner, for every payload size,
including the smallest and hardest-to-dominate case (10 B ACK, low corner:
16.6x the idle floor; high corner, most adversarial to this claim: still
9.6x).** Radio-TX blocking dominates the UI-unserviced gap across the full
plausible range of the un-measured constants. **The reroute condition does
NOT fire** — a task/core split remains worth pursuing on this model's
evidence.

## 5. SIMULATED — UI-unserviced-gap sweep (headline metric)

```
-- UI-unserviced-gap sweep (headline metric) --
topology                 corner    payload_B     longest_ms       p95_ms      mean_ms   cumul_unsvc_ms   service_hz
single-loop (current)    low              10         239.19         5.00         5.15         178270.3       192.45
split (proposed M1)      low              10           0.00         0.00         0.00              0.0     20000.00
single-loop (current)    low              40         239.19         5.00         5.23         178300.3       189.29
split (proposed M1)      low              40           0.00         0.00         0.00              0.0     20000.00
single-loop (current)    low             100         362.19         5.00         5.44         178360.3       182.21
split (proposed M1)      low             100           0.00         0.00         0.00              0.0     20000.00
single-loop (current)    low             255         813.19         5.00         5.56         178398.6       178.17
split (proposed M1)      low             255           0.00         0.00         0.00              0.0     20000.00
single-loop (current)    mid              10         248.90         6.56         7.23          57567.7        44.21
split (proposed M1)      mid              10           5.00         5.00         5.00          44150.0        49.06
single-loop (current)    mid              40         248.90         6.56         7.61          59585.9        43.48
split (proposed M1)      mid              40           5.00         5.00         5.00          44150.0        49.06
single-loop (current)    mid             100         369.90         6.56         8.51          64096.2        41.85
split (proposed M1)      mid             100           5.00         5.00         5.00          44150.0        49.06
single-loop (current)    mid             255         820.65         6.56         9.14          67105.5        40.77
split (proposed M1)      mid             255           5.00         5.00         5.00          44150.0        49.06
single-loop (current)    high             10         258.61         8.12         9.38          42111.8        24.94
split (proposed M1)      high             10          10.00        10.00        10.00          44200.0        24.56
single-loop (current)    high             40         258.61         8.12        10.05          44380.9        24.53
split (proposed M1)      high             40          10.00        10.00        10.00          44200.0        24.56
single-loop (current)    high            100         377.61         8.12        11.64          49473.0        23.61
split (proposed M1)      high            100          10.00        10.00        10.00          44200.0        24.56
single-loop (current)    high            255         828.11         8.12        12.94          53368.0        22.90
split (proposed M1)      high            255          10.00        10.00        10.00          44200.0        24.56
```

**Reading these numbers:**

- **`single-loop`'s longest gap scales with payload size, monotonically, at
  every corner** — 239 ms → 813 ms (low corner), 249 ms → 821 ms (mid), 259
  ms → 828 ms (high), moving from a 10 B ACK to a 255 B frame. This is
  exactly the mechanism `docs/perf/ui-perf-baseline.md` named: a queued
  outbound frame's `radio.transmit()` call blocks the WHOLE loop — including
  `ui.step()` — for the frame's full LoRa airtime, and a bigger frame means
  a bigger block.
- **`split`'s longest gap does NOT scale with payload size at all** — flat
  at 5.00 ms (mid corner) and 10.00 ms (high corner) across all four payload
  sizes, by construction: the split topology's UI task never touches
  `TxQueue`/`lora_airtime_ms`/payload size in this model at all, so this
  isn't asserted, it's structurally true. (The `low`-corner 0.00 ms reading
  is the degenerate case where every unknown UI-task cost — `ui.step()`'s
  low bound and the split UI task's idle-tick low bound — is swept to
  exactly zero; not a claim that a real implementation achieves zero.)
- **Order-of-magnitude improvement, at every corner, for the worst-case (255
  B) payload:** 813.19 ms → 5.00 ms (low, 163x), 820.65 ms → 5.00 ms (mid,
  164x), 828.11 ms → 10.00 ms (high, 83x). All comfortably clear a 10x bar.
- **UI service cadence does NOT uniformly improve alongside the longest-gap
  win above** — this claim did not survive re-anchoring `ui_step`'s high
  bound on the corrected ~30.72 ms full-repaint floor (§4.1; the earlier
  ~3.1 ms floor understated it by ~10×, see the correction note there). At
  the **low** corner split's cadence is still enormously higher (20 000 Hz
  vs single-loop's ~178–192 Hz) and at the **mid** corner split is still
  ahead (49.06 Hz vs single-loop's ~41–44 Hz). But at the **high** corner —
  where `ui_step` is now large enough to dominate the decoupled UI task's
  own loop period, with no CAD/TX/RX-poll cost around it to dilute it the
  way single-loop's much larger fixed cost diluted the old, too-small
  `ui_step` — split's cadence (24.56 Hz, flat across all four payload sizes,
  by construction) is essentially tied with single-loop's, and for the
  smallest payload is measurably **below** it: 24.56 Hz vs single-loop's
  24.94 Hz at 10 B, though single-loop falls further as payload grows
  (22.90 Hz at 255 B, still below split). The split topology still wins
  decisively on **longest-gap** — the headline metric above, and the
  correctness-relevant one: how long the UI can go completely unserviced —
  at every corner and every payload size; it does not also uniformly win on
  **average cadence** at the high corner.

## 6. SIMULATED — radio/dispatcher-task cadence

```
-- radio/dispatcher-task cadence (same loop as UI for single-loop; the
   decoupled radio/dispatcher task under the split topology) --
topology                 corner    payload_B     longest_ms       p95_ms      iter_hz
single-loop (current)    low              10         239.19         5.00       192.45
split (proposed M1)      low              10         239.19         5.00       194.37
single-loop (current)    low              40         239.19         5.00       189.29
split (proposed M1)      low              40         239.19         5.00       191.18
single-loop (current)    low             100         362.19         5.00       182.21
split (proposed M1)      low             100         362.19         5.00       184.03
single-loop (current)    low             255         813.19         5.00       178.17
split (proposed M1)      low             255         813.19         5.00       179.95
single-loop (current)    mid              10         248.90         6.56        44.21
split (proposed M1)      mid              10         246.63         6.54       148.46
single-loop (current)    mid              40         248.90         6.56        43.48
split (proposed M1)      mid              40         248.88         6.54       145.83
single-loop (current)    mid             100         369.90         6.56        41.85
split (proposed M1)      mid             100         370.88         6.54       140.55
single-loop (current)    mid             255         820.65         6.56        40.77
split (proposed M1)      mid             255         821.63         6.54       136.88
single-loop (current)    high             10         258.61         8.12        24.94
split (proposed M1)      high             10         258.56         8.07       119.85
single-loop (current)    high             40         258.61         8.12        24.53
split (proposed M1)      high             40         258.56         8.07       118.04
single-loop (current)    high            100         377.61         8.12        23.61
split (proposed M1)      high            100         379.56         8.07       113.61
single-loop (current)    high            255         828.11         8.12        22.90
split (proposed M1)      high            255         830.06         8.07       110.21
```

For `single-loop`, this is the identical loop, so identical numbers to §5.
For `split`, this is the decoupled radio/dispatcher task's OWN cadence, with
`ui.step()`/drain removed from it — its iteration duration is unchanged from
`single-loop`'s (radio-side cost is the same either way; only the coupling
to the UI is what changes), but its throughput (`iter_hz`) improves modestly
because it is no longer periodically stretched by `ui.step()`'s own cost.
This is a smaller effect than §5's headline UI-side improvement, and is
reported here as this model's answer to "does RX-poll cadence and
CAD-attempt latency under UI load improve" for the radio side — a later
task-split validation pass re-runs this exact model against the as-built
topology for its own before/after.

## 7. What this does NOT model — explicitly deferred

- **SPI2 bus arbitration / real contention behaviour.** This model always
  treats CAD as finding the channel clear (the conservative,
  worst-for-UI-starvation choice — it maximizes how many TX events actually
  fire and block `ui.step()`). Whether the LCD and radio's two
  `SpiDeviceDriver`s on one shared bus actually serialise correctly once
  genuinely running on two different tasks/cores is a separate
  source-and-datasheet analysis, not this model's job.
- **Real per-transaction SPI command overhead** beyond the analytically
  computed 4-symbol CAD-active time (`docs/perf/ui-perf-baseline.md` §4's
  ~128 µs/line figure is the 40 MHz DISPLAY bus, not the 8 MHz radio bus, so
  it is not reused here — the unknown remainder is a swept range,
  `cad_spi_overhead`).
- **Packet loss / retry storms / CAD-busy collisions** under real RF
  conditions.
- **Real ESP32-S3 wall-clock** for every swept parameter (§1). No device, no
  emulation — deferred to a real on-device capture, whenever one becomes
  available, which can then narrow every range in
  `perf_loop_model/src/params.rs` from a sensitivity sweep to a measured
  point.

## 8. Status

SIMULATED half: **DONE** — harness committed (`perf_loop_model/`, then 18
tests, now 30 after the M1 as-built re-parameterization —
`cargo test --workspace`-covered), sensitivity sweep run at three corners
across the full payload-size range, dominance verdict holds everywhere
(§4), order-of-magnitude UI-unserviced-gap improvement holds everywhere
(§5). **M1 update:** the split topology is no longer hypothetical — see
`docs/perf/task-split-host-validation.md` for the as-built re-run, its diff
against the prediction here, and the order-of-magnitude verdict re-confirmed
against the real `ui_task.rs` constants. On-device confirmation of every
swept range remains open regardless (deferred predicate, collection kit) —
this document, and its M1 successor, predict a delta; neither confirms one
on real hardware.
