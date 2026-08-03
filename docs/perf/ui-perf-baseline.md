# MeshCadet performance — state of record

**Currency: 2026-08-03, post-review.** This is the single authoritative
performance document for MeshCadet. It supersedes and absorbs the earlier
"Phase-1 measurement contract & baseline" and every in-place annotation and
retraction that accumulated on top of it. There is one ledger here, not a
stack of superseded layers — where an earlier finding was wrong, the body
carries the *corrected* account and §11 records the correction in one line so
nobody re-derives a retracted number from an old harness output.

Every number below was re-measured, re-derived or re-simulated at the commit
this document lands on. Every `path:line` citation was re-resolved against the
same tree.

## How the performance documents fit together

| Document | What it is | Currency |
|---|---|---|
| **`ui-perf-baseline.md`** (this one) | **The state of record.** Current numbers, current code shape, the ledger, and the one deferred-predicate register (§9). Start here. | Live — updated in place |
| `perf-loop-model-baseline.md` | The loop model's own M0 baseline run and its method write-up | Snapshot, M0 |
| `spi2-arbitration-r1.md` | Static SPI2 bus-arbitration analysis (risk R1), source + datasheet | Stable — analysis, not measurement |
| `task-split-host-validation.md` | The M1 task/core split's four host-validation legs, incl. the 58-row functional-parity matrix | Snapshot, M1 |
| `radio-host-validation.md` | The M2 DIO1 wait's host quantification + ISR-safety static audit | Snapshot, M2 |
| `ui-residual-opt-r1.md` | The M3 re-ranking of the two residual UI items (a documented no-op) | Snapshot, M3 |
| `collection-kit.md` | The operator-executed on-device procedure. The only way any §9 predicate closes | Live |
| `docs/adr/0012-dispatcher-ui-task-split.md` | The design decision behind the current two-task shape | Stable — decision record |

**The four snapshot documents print the model numbers as they stood at their
own milestone.** Later milestones re-parameterised the shared model, so those
tables no longer match a fresh run — see §5.5, which explains exactly why and
by how much. When a snapshot and this document disagree, **this document is
current**.

---

## 0. Provenance legend — read this before quoting any number

Every quantity in this document carries exactly one tag. A number without a
tag is a bug in this document.

| Tag | Meaning |
|---|---|
| **[HOST]** | Really executed on an x86-64 host by a committed test/bench in this repo. Reproducible by a command in §2. |
| **[SIM]** | Produced by `perf_loop_model`, a host discrete-event model of the firmware's two topologies driving the **real** `firmware-core` state machines. Not an execution of the shipped firmware. |
| **[ANALYTICAL]** | Computed from a formula or datasheet constant that is itself in-repo and cited. Exact, but not an execution of anything. |
| **[ESTIMATE]** | A projection combining tagged inputs, or a reasoned bound. Never presented as measured. |
| **[DEFERRED-DEVICE]** | Not measured, because it needs a flashed T-Deck Plus (and sometimes a second peer node). Every one is enumerated in §9 with the procedure that closes it. |

**A [SIM] number may never be presented as a device measurement, and never
closes a [DEFERRED-DEVICE] row.** Simulation bounds a device measurement; it
does not replace one. This is the strictest rule in this document: the whole
review ran with no hardware in the loop, so the line between "modelled" and
"measured on silicon" is the line that keeps the record honest.

### Why the host/device split is architectural, not a convenience

`firmware/` cross-compiles for `xtensa-esp32s3-espidf` and links
`esp-idf-svc`/`esp-idf-hal`; its `[[bin]]` sets `harness = false`, so
`cargo test` in `firmware/` only *type-checks* its `#[cfg(test)]` blocks — the
resulting binary is an Xtensa ELF that cannot execute on the host. Anything
that must actually *run* has to live outside `firmware/`. That is why the
host-testable logic was extracted into `firmware-core` (a root-workspace
crate, no Slint, no esp-idf) and why the render harnesses drive Slint directly
rather than importing firmware types.

Consequence: **CI's `firmware build gate (check-all-features.sh)` job is the
compile/type oracle for firmware.** A change whose only remaining risk is
"does it cross-compile" is finished when it is pushed, not when it is guessed
at locally.

---

## 1. What the system is — the shape that produces every number below

**Two application tasks on two cores.** This is the post-split shape landed by
ADR-0012 (`docs/adr/0012-dispatcher-ui-task-split.md`); the single-superloop
shape every earlier revision of this document described is gone (§11).

**Dispatcher task** — the ESP-IDF main task, explicitly pinned to **CPU0**
(`firmware/sdkconfig.defaults:94`, `CONFIG_ESP_MAIN_TASK_AFFINITY_CPU0=y`),
49 152 B stack (`sdkconfig.defaults:81`). Its loop (`firmware/src/main.rs:1901`)
runs, per iteration, in order:

```
WDT pet                       (main.rs:1908 — esp_task_wdt_reset)
  → GPS poll                  (main.rs:1910 — duty-cycled UART1 NMEA read)
  → tx-timestamp rebase       (main.rs:1917)
  → battery poll              (main.rs:1973 — throttled ADC)
  → room keep-alive scheduler (main.rs:2093)
  → CAD + TX                  (main.rs:2396 — SPI cmds, a DIO1 wait with a
                               20 ms hard deadline (radio.rs:676), then
                               radio.transmit() blocks for FULL AIRTIME
                               (radio.rs:488))
  → RX poll                   (main.rs:2543 — radio.try_receive, DIO1 wait
                               ≤ RX_POLL_YIELD_MS = 20 ms, main.rs:1748)
  → periodic stats/HWM/perf   (main.rs:2728, every 30 s)
  → drain UiCommand           (main.rs:2858)
```

**`ui_task`** — explicitly pinned to **core 1**
(`firmware/src/ui_task.rs:213`, `pin_to_core: Some(Core::Core1)`), 32 768 B
stack (`ui_task.rs:134`), priority 5 (`ui_task.rs:139`), spawned from the
dispatcher at `main.rs:911`. It owns touch (I2C1), the keyboard, the trackball,
the buzzer, the LCD and **all** of Slint. Its loop blocks on
`evt_rx.recv_timeout(UI_TICK_MS)` with `UI_TICK_MS = 16`
(`ui_task.rs:121`, `:371`), so it wakes on a queued event **or** on a 16 ms
tick, whichever comes first.

**The boundary is two bounded `std::sync::mpsc::sync_channel`s**
(`ui_task.rs:182-183`), both driven by `try_send`, which never blocks:
dispatcher → UI events (`EVENT_QUEUE_CAP = 32`, `ui_task.rs:126`) and UI →
dispatcher commands (`COMMAND_QUEUE_CAP = 16`, `ui_task.rs:129`). **The
dispatcher never blocks on the UI.** A full command queue surfaces a
user-visible refusal — a "send queue is busy" row in the conversation
(`ui/mod.rs:2664`) — rather than silently dropping a send.

Both tasks are Task-WDT subscribed (`main.rs:1792`, `ui_task.rs:331`); both log
their own stack high-water mark every 30 s (`main.rs:2776`, `ui_task.rs:423`).

Two auxiliary threads still exist (`provisioning_server`, `main.rs:1011`;
`admin_server`, `main.rs:1628`), unpinned, at the pthread default. Cross-task
state is four `static std::sync::Mutex<…>` snapshots written by the dispatcher
and read by those threads (`main.rs:198`, `:207`, `:216`, `:228`). **`ui_task`
touches none of them** — the split added zero new participants to those four
locks; its only cross-core objects are the two channels above.

Two buses matter, and they are not the same bus:

- **SPI2** — shared by the LCD (ST7789, 40 MHz, registered `main.rs:829`) and
  the radio (SX1262, 8 MHz, registered `main.rs:752`) as two
  `SpiDeviceDriver`s on one `Box::leak`'d `&'static SpiDriver`
  (`main.rs:735`). **Both devices are registered on the dispatcher task,
  before `ui_task` exists** — the one-task-per-device precondition ESP-IDF's
  per-bus arbitration guarantee needs. Only SPI *transactions* ever go
  concurrent (see §4.3).
- **I2C1** — touch (GT911) and the keyboard co-processor. Physically separate;
  **input polling never contends with radio or display**, at any rate.

---

## 2. Reproducing every number in this document

```sh
# Slint-based harnesses need a system sans-serif. In a container without
# fontconfig defaults, slint-build fails with "could not determine a default
# font for sans-serif" unless you set this:
export SLINT_DEFAULT_FONT=/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf

cargo run  -p ui_perf --release --bin ui_perf_bench        # §3.1
cargo test -p ui_perf --tests -- --nocapture               # §3.3, §3.4
cargo test -p ui_sim  --test perf_profile -- --nocapture   # §3.2
cargo run  -p perf_loop_model --bin loop_model_report      # §5
cargo test --workspace                                     # everything else
```

Host timing figures move run-to-run and machine-to-machine — the §3.1 bench in
particular is ~2–3× slower on a loaded container than on an idle dev box, so
quote its *shape* (linearity, relative cost), never its absolute nanoseconds.
Line counts and allocation counts are exact and reproduce bit-for-bit; when one
of those moves, the code moved (§11's last entry is exactly that case).

Cross-compilation for the device is CI's job
(`.github/workflows/ci.yml`, `firmware build gate (check-all-features.sh)`).

---

## 3. Measured numbers — [HOST]

All re-run 2026-08-03 at this commit.

### 3.1 Render-logic cost (`ui_perf_bench`, release profile) — [HOST]

```
render_mentions[plain]:          ns_per_op = 51.6
render_mentions[other_mention]:  ns_per_op = 69.5
render_mentions[self_mention]:   ns_per_op = 64.9

build_message_items[n=10]:   ns_per_op =  3951   alloc=37  bytes=2597   net_live=0
build_message_items[n=50]:   ns_per_op = 20097   alloc=184 bytes=12936  net_live=0
build_message_items[n=200]:  ns_per_op = 90717   alloc=734 bytes=52086  net_live=0
```

Reading them: cost is **linear** in conversation size (~395–454 ns/record
across a 20× range, no quadratic blowup); ~3.7 allocator calls and ~260 bytes
per message; `net_live_bytes == 0` in every case (nothing leaks). Crucially
this cost is paid on `navigate_to_message_view` / `refresh_message_view_for` —
**once per conversation open or per new-message refresh, not per frame**. Even
at n=200 that is ~91 µs on this host, three orders of magnitude under any
frame budget that matters.

These functions live in `firmware_core::ui::message_view` and are exercised by
`firmware-core`'s own tests under `cargo test --workspace`;
`ui_perf::render_logic` re-exports them rather than porting them, so there is
exactly one implementation and one set of correctness tests.

**Portability caveat [ESTIMATE]:** the Xtensa LX7 @ 240 MHz will be slower in
absolute terms and firmware ships `opt-level = "z"` while this bench runs the
workspace release profile. Treat §3.1 as *relative/shape* truth (linearity,
allocation counts, which branch costs more), not as an on-target prediction.
Absolute on-target timing is [DEFERRED-DEVICE] (§9, D1).

### 3.2 Repaint scope — real Slint renderer, real `.slint` assets — [HOST]

`ui_sim/tests/perf_profile.rs` and `ui_perf/tests/motif_repaint.rs` drive the
production `firmware/src/ui/motifs.slint` scene through
`MinimalSoftwareWindow` + `RepaintBufferType::ReusedBuffer` + `render_by_line`
— the identical API `TDeckWindowAdapter::render_if_needed` calls — with a
counting `LineBufferProvider` standing in for the SPI write. The dirty-region
*decision* is made inside Slint, not approximated here, so these are real
measurements of production repaint scope.

```
frame0 (initial full paint)        lines = 240/240   px = 76800   widest = 320
frame1 (idle, no property change)  lines =   0/240   px =     0   widest =   0
CometOnNotify peak dirty frame     lines =  14/240   px =   700   widest =  50   (31 dirty ticks)
RocketOnSend  peak dirty frame     lines =  29/240   px =   580   widest =  20   (21 dirty ticks)
comet sweep (motif_repaint)        39 animated frames, worst frame 14 lines, tallest bbox 14 px
```

- **Idle is a true no-op** (0 lines, 0 px) — a screen with no animation in
  flight and no navigation pending costs nothing per tick on the render side,
  at any tick rate.
- **The foreground motifs are small.** `CometOnNotify` peaks at ~6 % of the
  frame; `RocketOnSend` at ~12 %, only 20 px wide. Every one-shot settles back
  to a 0-dirty steady state — the "never an infinite `animate`" contract
  `motifs.slint` claims, confirmed from the render side.
- **There is no full-window animated-backdrop problem.** `Starfield`, the
  window fill and the planet corner are static; they paint once on navigation
  and are not re-flushed while a motif moves.

One bookkeeping note: `ui_perf/tests/flush_line_alloc.rs:253` projects the
`RocketOnSend` peak as a **28**-line fixture. The live `ui_sim` capture above
now measures **29**. The fixture is a projection scenario, not a measurement,
and one line changes nothing it asserts (0 allocations on the new flush path
at any line count) — but the two should be reconciled the next time either is
touched.

### 3.3 The real repaint cost was the screen-entry fade — found, fixed, pinned — [HOST]

Every themed screen wraps its content in an `opacity: content_opacity` /
`reveal_opacity` binding (`contact_list.rs`, `message_view.rs`, `pin_entry.rs`,
`gps_status.rs`, `admin_menu.rs`, `unprovisioned.rs`, and — scoped to its
emoji-picker overlay — `compose.rs`).

**Mechanism, confirmed against `i-slint-core`'s own
`partial_renderer.rs::compute_dirty_regions`:** when an item's `opacity`
changes, Slint marks `must_refresh_children` for the whole subtree — "this will
impact all the children … regardless if they are themselves dirty or not". A
near-full-window `VerticalLayout { opacity: content_opacity; … }` therefore
re-dirties its **entire** bounding region on every tick the fade is still
interpolating. When this was first measured, `ui.step()` ran once per
dispatcher iteration, which idled near the then-5 ms `RX_POLL_YIELD_MS`
(~200 Hz), so an unthrottled render flushed the full region ~40 times for one
200 ms transition.

```
[entry-fade] unthrottled: 40 frames rendered, 40 of them full-window (320x240)
[entry-fade] throttled:   11 frames rendered, 11 of them full-window (320x240)
```

**Fix (landed):** `RENDER_MIN_INTERVAL_MS = 16` (~60 fps) in `UiRuntime::step()`
(`firmware/src/ui/mod.rs:1088`) plus `TDeckWindowAdapter::has_active_animations`.
`slint::platform::update_timers_and_animations()` still runs every tick
unconditionally — every animated property stays exactly on its wall-clock curve
— and only the act of *flushing* a frame is capped, and only while an animation
is still settling. A fresh one-off redraw (navigation, incoming message, model
update) renders on the very next tick, uncapped, so tap-to-first-frame is
untouched. **72 % fewer full-window flushes, with the final settled framebuffer
asserted bit-for-bit identical (FNV-1a) between throttled and unthrottled
runs.**

**Post-split attribution, corrected:** `step()` now runs on `ui_task`, whose
`recv_timeout` ceiling `UI_TICK_MS` is **16 ms** — *identical* to
`RENDER_MIN_INTERVAL_MS`. The split therefore supplies the same cadence cap by
construction in a quiet steady state, and the `40 → 11` win is now
overwhelmingly attributable to the split rather than to the throttle. The
throttle is still load-bearing under an event burst (`ui_task` also wakes per
queued event, up to `EVENT_QUEUE_CAP = 32`) and **must not be removed**;
`docs/perf/ui-residual-opt-r1.md` §4.1/§5 carries the full argument, and both
constants' doc comments now cross-reference each other so retuning either one
cannot silently falsify the other.

### 3.4 Landed allocation and repaint-scope fixes, with their pinned numbers — [HOST]

| Fix | Site | Before → after | Pinned by |
|---|---|---|---|
| Per-dirty-line heap `Vec<Rgb565>` removed from the flush path | `ui/platform.rs::process_line` (`:260`) + `ui/display.rs::flush_line_range` (`:276`, takes an `impl ExactSizeIterator` and streams into `mipidsi::fill_contiguous`) | **240 allocs → 0** per full-window paint; 14 → 0 per `CometOnNotify` frame; 28 → 0 per `RocketOnSend` frame | `ui_perf/tests/flush_line_alloc.rs` — also asserts byte-identical pixels on both paths |
| GPS/battery status setters deduped | `UiRuntime::set_gps_status` / `set_battery_status` | GPS row: **700 allocs / 12 000 B → 35 allocs / 600 B** per 100 ticks. Battery row: **55 → 5** allocs per 50 ticks | `ui_perf/tests/alloc_tick_dedup.rs` |
| Live message-list update reconciled in place instead of wholesale model replace | `ui/screens/message_view.rs::set_messages` | **240 lines → 22 lines** flushed per incoming message (90 % fewer SPI line-flush cycles); static backdrop + header no longer re-flushed | `ui_perf/tests/model_update_repaint.rs` — also asserts pixel-identical final framebuffer |
| Screen-entry fade render-cadence throttle | `ui/mod.rs::step()` (§3.3) | **40 → 11** full-window flushes per 200 ms fade | `ui_perf/tests/entry_fade_repaint.rs` |

Each is a strict reduction in work with an asserted-identical visual result, so
none has a plausible regression direction for radio timeliness — only a
magnitude, which is [DEFERRED-DEVICE] (§9, D2).

---

## 4. Derived numbers — [ANALYTICAL]

### 4.1 Display SPI floor — [ANALYTICAL]

A 320-pixel RGB565 line is 640 bytes; its pure data transfer at 40 MHz SPI2 is
**~128 µs** (640 B × 8 bits / 40 MHz), issued by `display-interface-spi` as ten
64-byte chunks. `flush_line_range` (`ui/display.rs:276`) is called once per
dirty line and additionally issues the ST7789 CASET/RASET/RAMWR window-set
commands per call; that per-transaction command overhead is **not** quantified
here and is [DEFERRED-DEVICE] (§9, D2).

| Dirty lines this frame | Data-only SPI floor (128 µs × lines) |
|---|---|
| 14 (`CometOnNotify` peak, §3.2) | ~1.8 ms |
| 22 (in-place message append, §3.4) | ~2.8 ms |
| 29 (`RocketOnSend` peak, §3.2) | ~3.7 ms |
| 240 (full `DISPLAY_HEIGHT`, navigation paint) | ~30.7 ms |

Rows 1–3 are [ESTIMATE]: an [ANALYTICAL] per-line floor multiplied by a
[HOST]-measured line count.

### 4.2 LoRa airtime — the dominant number in this document — [ANALYTICAL]

`firmware-core/src/dispatcher.rs:316::lora_airtime_ms`, Semtech AN1200.13 §4,
at the locked SF7 / BW 62.5 kHz / CR 4:5 / 8-symbol-preamble / explicit-header /
CRC-on preset (`firmware/src/radio.rs:800`). All four rows re-verified against
the formula on 2026-08-03.

| Payload | Airtime — `radio.transmit()` blocks the dispatcher task this long |
|---|---|
| 10 B (ACK-shaped) | **83 ms** |
| 40 B (typical DM) | **165 ms** |
| 100 B | **349 ms** |
| 255 B (max) | **800 ms** |

This is RF airtime — the SX1262 transmitting — **not** SPI-bus-hold time. SPI2
is touched only for the initial `WRITE_BUFFER`/`SetTx` commands; the task then
blocks on the **DIO1 GPIO** for `TxDone` (`radio.rs:488` → `wait_high`,
`radio.rs:387`), not on SPI. `try_receive`'s 20 ms `RX_POLL_YIELD_MS` window
and `channel_activity_detection`'s 20 ms deadline (`radio.rs:676`) are likewise
DIO1 GPIO waits, not SPI holds.

### 4.3 SPI2 bus-hold bound — [ANALYTICAL]

ESP-IDF's `spi_master` serialises transactions per bus across devices added
with `spi_bus_add_device`, and `esp-idf-hal`'s `SpiDeviceDriver` wraps exactly
that; each device here is touched by exactly one task (§1), which is the
supported pattern. The 40 MHz/8 MHz clock switch is reconfigured strictly
inside the held lock.

**The longest a display flush can hold SPI2 against the radio is one 64-byte
chunk at 40 MHz — ≤ 12.8 µs**, not a line and not a repaint, because
`display-interface-spi` chunks every line into independent 64-byte writes that
each re-arbitrate the bus. That is 4–5 orders of magnitude below CAD (≤20 ms)
and airtime (83–800 ms). Full derivation with source and datasheet citations:
`docs/perf/spi2-arbitration-r1.md`. Empirical confirmation under real
concurrent load is [DEFERRED-DEVICE] (§9, D9) and is confirmatory only — the
correctness argument does not depend on it.

---

## 5. Modelled numbers — [SIM]

### 5.1 What the model is, and what it is not

`perf_loop_model` is a host discrete-event model of the dispatcher, in both
topologies (single superloop, and the as-built split), driving the **real**
`firmware-core` state machines — `TxQueue`, `AirtimeBudget`, `DuplicateFilter`,
`lora_airtime_ms`. Constants it cannot measure on a host (per-phase durations)
are carried as **cited ranges swept at three corners** (`low`/`mid`/`high`),
never as point estimates: `perf_loop_model/src/params.rs`. Every figure in this
section is **[SIM]** and closes no [DEFERRED-DEVICE] row. The model's own
calibration hook (`perf_loop_model::calibration`) is what replaces a swept
range with a device-measured point once §9's Part D data exists.

Reproduce: `cargo run -p perf_loop_model --bin loop_model_report`.

### 5.2 [SIM] Longest UI-unserviced gap — the headline metric

The gap is "how long can the UI go completely unserviced" — no touch, no
keyboard, no render. Full 24-cell sweep, re-run at this commit:

| Corner | Payload | Single-loop (pre-split) | Split (as built) | Improvement |
|---|---|---|---|---|
| low | 10 B | 254.19 ms | **0.00 ms** | unbounded |
| low | 40 B | 254.19 ms | **0.00 ms** | unbounded |
| low | 100 B | 377.19 ms | **0.00 ms** | unbounded |
| low | 255 B | 828.19 ms | **0.00 ms** | unbounded |
| mid | 10 B | 263.90 ms | **8.10 ms** | 32.6× |
| mid | 40 B | 263.65 ms | **8.10 ms** | 32.6× |
| mid | 100 B | 385.90 ms | **8.10 ms** | 47.6× |
| mid | 255 B | 836.65 ms | **8.10 ms** | 103.3× |
| high | 10 B | 273.61 ms | **16.20 ms** | **16.9×** ← binding worst cell |
| high | 40 B | 273.61 ms | **16.20 ms** | **16.9×** |
| high | 100 B | 394.61 ms | **16.20 ms** | 24.4× |
| high | 255 B | 843.11 ms | **16.20 ms** | 52.0× |

Two properties, both required, both hold **at every cell of the sweep** rather
than at a favourable point estimate:

1. **At least an order of magnitude better everywhere.** The binding worst case
   is 16.9× (high corner, 10 B and 40 B). The quotable 100× figures come from
   the most favourable payload and should not be used as the headline.
2. **The gap no longer scales with payload size.** Split gap is flat —
   0.00 / 8.10 / 16.20 ms per corner — identical from 10 B to 255 B, while the
   single-loop gap grows 254 ms → 828–843 ms over the same range. That is the
   structural property: the UI's service interval is now bounded by
   `UI_TICK_MS`, not by what the radio is transmitting.

**Dominance / reroute check**, the question M0 opened with — does one radio TX
alone exceed the worst UI gap achievable with *zero* radio traffic? Yes, at all
12 corner/payload cells, by 3.5×–40.0×. Radio TX blocking, not UI-side cost,
was the thing to fix.

### 5.3 [SIM] Dispatcher-task cadence — the honest half

The split's effect on the *dispatcher's own* loop is smaller and more mixed
than its effect on the UI, and is reported here plainly rather than dressed up.

| Corner | Iteration rate, single-loop → split | Longest dispatcher gap, single-loop → split |
|---|---|---|
| low | 48.47 → 48.59 Hz (flat) | 254.19 → 254.19 ms (unchanged) |
| mid | 26.26 → 44.84 Hz (**1.71×**) | 263.90 → 261.98 ms (−1.9 ms) |
| high | 17.99 → 41.56 Hz (**2.31×**) | 273.61 → 273.26 ms (−0.35 ms) |

(10 B payload rows; the pattern holds across payloads.)

- **Iteration cadence improves substantially under UI load** — up to 2.3× at
  the high corner — because the dispatcher no longer pays `ui.step()` inline.
  That is the mechanism by which RX-poll and CAD-attempt *opportunities* get
  more frequent.
- **The dispatcher's longest single gap does not improve**, and at the
  high-corner/255 B cell it is 2.15 ms *worse* (843.11 → 845.26 ms). This is
  not a regression signal: that gap is airtime-bound by construction — the task
  is inside `transmit()` for the full 800 ms either way — and 2.15 ms is model
  structure, not measured behaviour. **The split does not shorten airtime and
  was never going to.**
- **RX-poll cadence is flat by model construction**, not by measurement: the
  model's RX poll is driven by the same loop the cadence column reports.

### 5.4 [SIM] DIO1 wait — what removing the 1 ms spin-poll bought

The M2 rework replaced three `while !dio1.is_high() { delay_ms(1) }` spin-polls
with an interrupt/notification-driven wait (`GpioDio1Wait`, `radio.rs:327`).
Modelled against the removed spin-poll, split topology:

| Metric | Result |
|---|---|
| CAD-attempt latency | **−0.808 ms per attempt**, fixed, at 11 of 12 swept cells (−0.308 ms at the twelfth) |
| Dispatcher iteration rate | +0.01–0.03 Hz — i.e. no measurable cadence change |
| TX wait | genuine no-op: `lora_airtime_ms` already ceils to whole milliseconds, so a 1 ms tick never quantized it |

The honest reading: this bought a **fixed, small, deterministic** latency
reduction on the CAD path and removed a per-millisecond scheduler wake. It did
not change throughput. Its larger value is structural — the wait is now a
blocking primitive with a testable postcondition (§6.1, and
`radio-host-validation.md` §4's ISR-safety audit).

### 5.5 Why the milestone documents print different numbers

The model was re-parameterised twice after its M0 baseline run, so the M0 and
M1 snapshot documents print figures a fresh run no longer reproduces. This is
expected and is *not* a correction to any verdict — but it will trip a reader
who quotes the wrong table:

| Change | Effect on the printed numbers |
|---|---|
| The `ui_step` upper bound was corrected 5.0 ms → 30.72 ms (the display SPI floor was ~10× understated; §11) | Raised the single-loop idle-floor gap: M0's `perf-loop-model-baseline.md` prints 8.620 ms at the high corner, a fresh run prints 23.620 ms |
| The M1 split was re-parameterised to the as-built firmware (`UI_TICK_MS = 16` anchor, a new `queue_handoff` parameter) | Introduced the split rows entirely |
| M2 retuned `RX_POLL_YIELD_MS` 5 → 20 ms, which the model applies to **both** topologies | Raised the single-loop baseline: `task-split-host-validation.md` §2.3 prints 258.61 ms at high/10 B, a fresh run prints 273.61 ms. The M1-era "51×–101×" headline is now 16.9×–103.3× |

None of these moves the qualitative result: the split wins at every cell, by at
least an order of magnitude, and payload-independently. §5.2's table is the
current one.

---

## 6. Ledger — what is landed, what is open, what is settled

### 6.1 LANDED

**Structural (this review):**

- **The dispatcher/UI task split** (ADR-0012). Touch, keyboard, trackball,
  buzzer, LCD and all of Slint moved to a core-1-pinned `ui_task`; the
  dispatcher keeps radio, GPS, battery, room keep-alive and NVS. The UI's
  longest unserviced gap is now bounded by `UI_TICK_MS`, not by LoRa airtime
  (§5.2). Two secondary wins fell out, both serving delivery: the ~1.15 s
  boot-time RX gap from `run_splash_ripple` is gone, and the one confirmed
  release-only stack-overflow site (`navigate_to_pin_entry`) moved off the
  dispatcher task. TWDT coverage went from one task to two.
- **Explicit core affinity, both directions.** `CONFIG_ESP_MAIN_TASK_AFFINITY_CPU0`
  + `pin_to_core: Some(Core::Core1)`. Core 1 now carries real application work
  instead of being left to scheduler chance.
- **Interrupt-driven DIO1 waits** replacing three 1 ms spin-polls, with
  `RX_POLL_YIELD_MS` retuned 5 → 20 ms now that the UI no longer shares the
  task (§5.4).
- **A postcondition on the DIO1 wait.** `wait_high` re-reads `is_high()` after
  every wake and honours an absolute deadline (`radio.rs:387`), so `Asserted`
  means "DIO1 is asserted right now". This closed a genuine, review-found
  defect introduced by the interrupt rework itself — a stale FreeRTOS task
  notification could satisfy a later, unrelated wait, which in `transmit()`
  reads as `TxDone` on a frame still in the air (silent outbound message loss)
  and in CAD reads as "channel clear" (listen-before-talk bypassed). See §11.
- **On-device timing instrumentation**, `--features diagnostics`-gated (§8).

**UI-side (measured, pinned by committed host tests):** everything in §3.4,
plus the demotions measurement produced:

- **`build_message_items` / `render_mentions` allocation churn — COLD.** §3.1:
  cheap, linear, per-navigation rather than per-frame. Not an optimization
  target.
- **"Full-window animated backdrop" — does not exist.** §3.2: the backdrop
  layers are static and paint once per navigation.
- **CAD backoff blocking sleep — fixed before this record began.** The old
  `FreeRtos::delay_ms(backoff_ms)` full-task stall on CAD-busy is now a
  non-blocking deadline (`cad_backoff_until_ms`).
- **Both residual UI items are closed or demoted — the final UI-optimization
  pass deliberately landed no optimization.** The per-dirty-line `Vec<Rgb565>`
  is measurably at **zero** (nothing left to do); the fade's repaint scope is
  demoted on three independent grounds — the split already supplies the cadence
  cap, `RENDER_MIN_INTERVAL_MS` is provably un-tightenable (a full-window flush
  is longer than any cap worth setting), and post-split the fade's worst-case
  cost to the *radio* is 12.8 µs, not 30.7 ms (§4.3). Full argument:
  `docs/perf/ui-residual-opt-r1.md`.

### 6.2 OPEN

- **The dispatcher task is still fully blocked for the duration of a TX.**
  83–800 ms (§4.2), during which GPS, battery, room keep-alive, CAD and RX poll
  do not run. The split removed the *UI* from behind that block; it did not
  remove the block. Whether this matters is a delivery question, and its
  measurement is [DEFERRED-DEVICE] (§9, D5/D6). Nothing in this review proposes
  changing it — doing so means an async/queued TX path, a materially larger
  change than a task split.
- **`ui_perf/tests/flush_line_alloc.rs`'s 28-line `RocketOnSend` fixture** vs.
  the live 29-line measurement (§3.2). Cosmetic; reconcile on next touch.
- **Three predicates are blocked on missing code, not on missing hardware**
  (§9.2). They will not close by running the collection kit as it stands.

### 6.3 SETTLED — do not re-litigate

- **Touch/keyboard vs. radio/display.** I2C1 is a physically separate bus from
  SPI2. Zero contention at any poll rate.
- **TX airtime vs. the SPI bus.** §4.2: airtime is RF, not bus-hold. The UI
  cannot make airtime longer or shorter. (It was still the dominant *task*
  blocker before the split — "not SPI contention" and "not a problem" are
  different claims, and conflating them is the error §11 records.)
- **SPI2 arbitration correctness.** §4.3 and `spi2-arbitration-r1.md`: settled
  by source and datasheet, not deferred. Only the empirical margin is deferred.
- **UI render cost as an input to radio timeliness.** Post-split it is a *sign*
  claim only, bounded at 12.8 µs (§4.3). Any analysis treating UI render cost
  as a first-order input to radio timeliness is reasoning from the pre-split
  world.

---

## 7. UI ↔ radio coupling map, post-split

The shared resource is **SPI2**, and after the split the two devices are on
different tasks and different cores, so they can genuinely contend — but only
per elementary 64-byte transaction (§4.3), and ESP-IDF serialises those.

```
core 0 — dispatcher task (main.rs:1901)
  [ WDT ] [ GPS ] [ tx-rebase ] [ Batt ] [ room keep-alive ]
     → [ CAD/TX ]  SPI cmds + DIO1 wait ≤20 ms when a TX is pending,
                   THEN transmit() blocks 83–800 ms of airtime   ← still dominant here
     → [ RX poll ] DIO1 wait ≤20 ms (GPIO, not SPI)
     → [ periodic stats ] [ drain UiCommand ]
     → (loop)

           ↕  two bounded mpsc channels, non-blocking on the dispatcher side
              (32 events out, 16 commands in)

core 1 — ui_task (ui_task.rs:371)
  recv_timeout(16 ms) → touch/kbd (I2C1, separate bus) → Slint tick
     → render_if_needed → flush_line_range × dirty_lines
        (§3.2: 0 idle / 14 comet / 22 msg-append / 29 rocket / 240 nav)
     → (loop)
```

**Direction A — UI delays radio.** Bounded at **≤12.8 µs** per radio SPI
command (§4.3), 4–5 orders below CAD and airtime. This direction is
effectively closed: the pre-split mechanism (a full-window paint sitting
inline ahead of the next RX poll, ~30.7 ms) no longer exists.

**Direction B — radio delays UI.** Bounded at `UI_TICK_MS`-plus-one-tick's
work, and no longer scales with payload (§5.2). The pre-split mechanisms —
CAD's ≤20 ms window and `transmit()`'s 83–800 ms airtime, both sitting inline
ahead of `ui.step()` — no longer sit in the UI's path at all.

Both directions are now bounded by construction rather than by measurement.
What remains unbounded on paper is the dispatcher's own inability to service
*itself* during a TX (§6.2), which is a delivery question (§9, D5/D6), not a
UI question.

---

## 8. Instrumentation status — what the device can report today

Landed and merged, `--features diagnostics`-gated (no cost in default builds —
the whole module compiles to nothing without the feature):

| Signal | Log line | Where |
|---|---|---|
| Per-phase dispatcher timing (GPS, battery, CAD, TX, RX poll) — n/min/mean/max/p95 | `PERF phase=<name>: …` | `main.rs:2806` |
| RX-notice latency proxy | `PERF rx-notice-latency: …` | `main.rs:2812` |
| Per-core utilization (`vTaskGetRunTimeStats()`) | `PERF core-utilization: core0=… core1=…` | `main.rs:2846` |
| `ui.step()` phase timing | `PERF phase=ui_step: …` | `ui_task.rs:442` |
| UI-starvation counter (cumulative + longest gap) | `PERF ui-starvation: cumulative=…ms longest=…ms` | `ui_task.rs:450` |
| Input-to-first-paint latency | `PERF input-to-first-paint: …` | `ui_task.rs:436`, sampled in `ui/mod.rs:1371` |
| Per-task stack high-water mark | `stack HWM` lines, every 30 s | `main.rs:2776`, `ui_task.rs:423` |

The pure computation behind these (fixed-memory histogram, percentile math, the
runtime-stats text parser) lives host-tested in `firmware_core::perf` — no
per-sample storage, given the dispatcher's stack budget. Three FreeRTOS Kconfig
options back the core-utilization reading
(`sdkconfig.defaults:173-175`: `GENERATE_RUN_TIME_STATS`, `USE_TRACE_FACILITY`,
`USE_STATS_FORMATTING_FUNCTIONS`).

**The split moved `ui_step`/`ui-starvation` from the dispatcher's rollup to
`ui_task`'s own.** For one commit window they were dropped entirely with no
replacement; that was caught at the M1 boundary and restored before this
document landed. `collection-kit.md` reads the restored format.

**Not instrumented, and therefore not closable by the kit:** free internal-heap
headroom (§9.2, D-H) and a per-frame dirty-line *count* (only a duration proxy
exists — §9.1, D3).

---

## 9. Deferred-predicate register — the one page

**This is the consolidated list.** Every predicate this review could not
execute is here, exactly once, with the procedure that closes it. Nothing
needing a device is recorded anywhere else in this document, and no other
document carries a competing list — `collection-kit.md` §0's table is the same
set, indexed the other way (by kit part).

**Prerequisites for everything in §9.1:** a T-Deck Plus flashed from a ref that
carries the diagnostics instrumentation (`collection-kit.md` §2 has the exact
check), built `--features diagnostics`, plus — for D4/D5/D6 only — a second
MeshCore-speaking peer node.

### 9.1 Hardware-only — runnable today, with the kit

| # | Predicate | What it closes | Procedure |
|---|---|---|---|
| **D1** | On-target cost of §3.1's render logic under `opt-level = "z"` on Xtensa LX7 @ 240 MHz | §3.1's portability caveat | `collection-kit.md` **Part C**, scripted scenario (`PERF phase=ui_step`, ContactList idle vs. a 200-message conversation open) |
| **D2** | Real per-`flush_line_range` SPI command overhead (CASET/RASET/RAMWR) beyond §4.1's data-only floor | §4.1's error bar; §3.4's "no plausible regression direction" magnitude | **Part C**, same run as D1 |
| **D3** | Real dirty-line-count distribution in use (idle / navigation / send-tap / incoming message) | §3.2's synthetic-scene → real-usage gap | **Part C**, *partial only* — the restored `ui_step` timing is a duration **proxy**, never a direct line count. A true per-frame counter does not exist on either topology |
| **D4** | Longest UI-unserviced gap and its scaling with payload size | §5.2's headline [SIM] claim, on silicon. Also validates the model itself | **Part G step 8** (`PERF ui-starvation`), 20 DMs at 10 B / 40 B / 255 B |
| **D5** | **Delivery success rate** — DM TX-with-ACK, DM RX, GRP_TXT, room push, vs. the pre-change baseline | The hardest constraint in this review: delivery must not regress. A tie passes; any degradation fails regardless of UI gain | **Part G**, two-device, 20-DM exchange each way, UI idle *and* under navigation load |
| **D6** | RX-notice latency, UI-idle vs. UI-active; CAD-busy and TX-retry counts | §5.3's modelled cadence improvement, on silicon | **Part G**, same run as D5, differenced |
| **D7** | Per-core utilization | §1's core-affinity claim, and §6.1's "core 1 carries real work" | **Part C** — every 30 s window reports it for free (`PERF core-utilization`) |
| **D8** | Post-split per-task stack high-water mark, dispatcher **and** `ui_task` | The 49 152 B / 32 768 B budgets; unblocks the deliberately deferred dispatcher-stack trim | **Part E** — both tasks now log their own HWM |
| **D10** | Felt frame rate / tap-to-first-frame; splash-ripple smoothness on the concurrent boot path | Human-perceptible responsiveness, which no counter captures. The only instrument that could contradict the decision not to re-architect the renderer | **Part D10** (stopwatch + 120/240 fps video), plus the automatic `PERF input-to-first-paint` block |
| **D12** | Bounded latency of a real concurrent NVS write masking the DIO1 GPIO ISR | `radio-host-validation.md` §4.1's one open ISR-safety item. Bounded and self-recovering by argument; this measures the bound | **Part F addendum** — timed capture around an admin-CLI edit issued while radio traffic is in flight |
| **P1** | Hands-on functional sweep: every screen, every navigation path, every radio path, on the device | The device-side half of functional parity. Its host-side half — a 58-row static parity matrix with a source citation per row — is met (`task-split-host-validation.md` §5) | **Part H** — walk `task-split-host-validation.md` §5's matrix row by row on hardware |
| **P2** | The loop model's swept constants (`perf_loop_model/src/params.rs`) — every real ESP32-S3 wall-clock figure it currently carries as a cited *range* | Replaces §5's sensitivity sweep with a calibrated point model. Consumed automatically by `perf_loop_model::calibration` | **Part D** — the calibration table (reuses Part C's log) |

### 9.2 Blocked on missing code, not on missing hardware

**These three do not close by running the kit.** They are listed here so the
register is complete, and they are deliberately *not* filed as hardware
deferrals — the distinction matters, because a hardware deferral is closed by
the operator and these are closed by a maintainer first.

| # | Predicate | What is missing | Then what |
|---|---|---|---|
| **D9** | SPI2 bus-hold behaviour under a *concurrent* full LCD repaint and radio TX, at 40 MHz and 8 MHz | A GPIO-toggle probe in `radio.rs` (bracket the SPI transaction with a scope-visible pin) — it does not exist | **Part F**, once the probe lands. Confirmatory only: §4.3's ≤12.8 µs bound is settled by source and datasheet |
| **D11** | DIO1 GPIO ISR IRAM-safety confirmatory reading | The same probe as D9, plus a link-section audit | **Part F addendum**. The static audit already found no NO-GO condition; this is empirical margin |
| **D-H** | Free internal-heap headroom after the split's +32 768 B of task stack | A `heap_caps_get_free_size(MALLOC_CAP_INTERNAL)` reading — the firmware logs none. One line in the diagnostics rollup would close it | Add the log line to `main.rs`'s 30 s diagnostics block, then read it in **Part C** |

### 9.3 One observation to fold into any Part G run

`radio.rs:437` emits `radio: stale DIO1 notification observed with the line low`
whenever the postcondition described in §6.1 actually fires. It is a
`debug!`-level line and no kit section names it. **When running Part G, grep the
serial log for `stale DIO1`** — a nonzero count on real traffic is the field
evidence that the defect class was real, and a zero count over a long run is
weak evidence it is rare. Either way, record the count in the report block.

### 9.4 If you came here from ADR-0012

ADR-0012 numbered its own deferred predicates `D-A`…`D-H`. They are the same
set, under different labels — resolved here so nobody hunts for a second
register:

| ADR-0012 | Here |
|---|---|
| D-A — post-split per-task stack HWM | **D8** |
| D-B — the ≤12.8 µs bus-hold bound, empirically | **D9** (code-blocked, §9.2) |
| D-C — delivery success rate vs. baseline | **D5** |
| D-D — real per-core utilization | **D7** |
| D-E — device UI-unserviced gap vs. the model | **D4** |
| D-F — device input-to-first-paint | **D10** (the automatic `PERF input-to-first-paint` half) |
| D-G — splash-ripple visual confirmation on the concurrent boot path | **D10** (the stopwatch/video half) |
| D-H — free internal-heap headroom after +32 768 B of stack | **D-H** (code-blocked, §9.2 — the one that keeps its ADR label, because nothing here supersedes it) |

### 9.5 Emulation does not close any of these

Espressif's QEMU fork emulates the ESP32-S3 CPU, memory, flash-SPI, PSRAM,
crypto and timers, but models no general-purpose SPI2 slave devices (no ST7789,
no SX1262), no DIO1/BUSY GPIO semantics, no I2C touch/keyboard, and no RF — and
is documented as non-cycle-accurate. It cannot boot this application past
display/radio init, and any timing number it produced would be fiction.

### 9.6 When a predicate closes

Move its number into the body of this document with a **[DEVICE]** tag naming
the build ref and the date, strike its row from §9, and add a §11 line if it
contradicts anything. `collection-kit.md` §9's report block is machine-parseable
by the `perf_device_report` crate, which archives it under
`docs/perf/device-reports/` and can feed the calibration hook (P2)
automatically.

---

## 10. What this review established, and what it did not

Seven acceptance criteria were set for the performance review before it began.
Each has a *gating* form — what could be established without hardware — and,
for the first five, a *deferred* form that only silicon closes. Nothing below
launders a deferred form into a met one.

| # | Criterion | Gating form | Deferred form |
|---|---|---|---|
| 1 | **Delivery must not regress** | **MET.** Source-level argument accepted at both structural boundaries: one `SpiDriver`, both devices registered on the dispatcher before `ui_task` exists (§1) so §4.3's ≤12.8 µs bound survives contact with the as-built code; `ui_task` participates in none of the four cross-task mutexes; no Slint symbol outside `firmware/src/ui*`; dedup / airtime-budget / TX-queue behaviour unchanged and dispatcher-exclusive; both queue directions non-blocking on the dispatcher with a user-visible refusal on overflow | **Open — D5.** Device delivery success rate against a real peer, versus the pre-change baseline |
| 2 | **Delivery should improve** | **MET, and reported plainly.** CAD-attempt latency improves by a fixed 0.808 ms/attempt (§5.4); dispatcher iteration cadence under UI load improves 1.7×–2.3× at the mid/high corners and is flat at the low corner (§5.3). The dispatcher's *longest* gap does **not** improve — it is airtime-bound, and at one swept cell is 2.15 ms worse. RX-poll cadence is flat by model construction, not by measurement | **Open — D6.** Device RX-notice latency, CAD-busy and TX-retry counts |
| 3 | **The UI-starvation window is gone** | **MET.** Modelled longest UI-unserviced gap drops by ≥1 order of magnitude at **every** cell of the 24-cell sweep (binding worst case 16.9×; unbounded at the low corner) and no longer scales with payload size — flat from 10 B to 255 B (§5.2) | **Open — D4.** The same numbers from the on-device UI-starvation counter |
| 4 | **Both cores carry real work** | **MET.** Explicit affinities in code, both directions: `CONFIG_ESP_MAIN_TASK_AFFINITY_CPU0=y` (`sdkconfig.defaults:94`) and `pin_to_core: Some(Core::Core1)` (`ui_task.rs:213`), cross-compiled green in CI on every merged change | **Open — D7.** Per-core utilization from `vTaskGetRunTimeStats()` |
| 5 | **Functional parity** | **MET.** A 58-row static parity matrix covering every screen, navigation path, input, radio path, peripheral, persistence and boot item, each with a source-level preservation argument and citation (`task-split-host-validation.md` §5). All CI gates green, re-run at this commit: `cargo test --workspace` 63 binaries / **1134 passed / 0 failed**, `clippy -D warnings` clean, `fmt --check` clean, `xtask` glyph-coverage and ui-event-parity green. The firmware cross-compile gate is green on the last firmware-touching merged change (`firmware build gate (check-all-features.sh)`, 11 m 56 s, pass) | **Open — P1.** The hands-on sweep |
| 6 | **Provenance on every number** | **MET.** This document. Five tags, defined in §0, exactly one per quantity; an untagged number is defined as a document bug. [SIM] is used, is defined as distinct from a device measurement, and closes no deferred row | — |
| 7 | **The deferred set is complete and actionable** | **MET, with the boundary drawn explicitly.** §9 is one page: 12 hardware-only predicates each paired with the exact kit part that closes them, plus — separately and labelled as such — three that are blocked on *missing code* rather than missing hardware (§9.2), each with the specific change that unblocks it, plus one log line to watch (§9.3). A register that quietly listed the code-blocked three alongside the rest would have implied the kit closes them | — |

**What the review did not do.** It did not measure anything on silicon — by
design, no hardware was in the loop at any point. It did not change airtime,
and it did not make the dispatcher able to work during a TX (§6.2). It landed
no UI-side optimization in its final pass, and says so: both remaining
candidates were re-measured and found already-closed or not worth the change.

**The strongest evidence that criterion 1's gating form was enforced rather
than rubber-stamped** is that it caught a defect this review's own work
introduced: the interrupt-driven DIO1 rework shipped a wait that could report
`Asserted` on a stale notification with the line low, which reads as `TxDone`
on a frame still in the air. The boundary review rejected the milestone, the
root cause was fixed rather than papered over, and the reference model's tests —
which had *certified* the defect as intended behaviour — were rewritten to pin
the correct postcondition (§11).

---

## 11. Corrections history

Kept deliberately short. This exists so a retracted number cannot be
resurrected from an old harness output, not as a second narrative.

- **`RocketOnSend`'s 86-line / 200 px-wide peak (earliest revision) is
  retracted.** It was an artifact of `ui_sim`'s *shared* scene, which also
  carried `MascotBob` (450 ms) and `Twinkle` (900 ms) entry-settle animations
  still in flight when `RocketOnSend` fired; the software renderer's
  `DirtyRegion` 3-rectangle cap (`PHYSICAL_REGION_MAX_SIZE = 3`) merged their
  unrelated dirty rects into one inflated bounding box. Neither `compose.rs`
  nor `message_view.rs` — the only real `RocketOnSend` consumers — imports
  `MascotBob`/`Twinkle`, so this never occurred in production. Current measured
  peak: **29 lines / 20 px wide** (§3.2). Any output showing 86 is stale.
- **The "prime suspect = translate+fade one-shot motif" framing is retracted.**
  The real cost was the screen-entry `opacity` fade, one level up (§3.3).
- **"TX airtime is out of scope" is superseded, not retracted.** It was correct
  as scoped ("no *UI* change can affect this") and wrong as a system-level
  conclusion. The fix was a task split, not a UI change.
- **"`ui.step()` shares a task with `radio.transmit()`" is superseded.** True
  when written; ADR-0012 split them (§1).
- **The `while !dio1.is_high() { FreeRtos::delay_ms(1) }` spin-poll quote is
  retracted — the code it quoted is gone.** Replaced by `GpioDio1Wait`
  (`radio.rs:327`). The blocking *duration* §4.2 reports is unchanged (still
  the full analytical airtime); only the wait mechanism changed.
- **The first `GpioDio1Wait` is retracted as correct.** It treated a FreeRTOS
  task notification — a sticky, per-task flag — as proof that DIO1 asserted,
  with no post-wake level re-check, so a leftover notification from an
  already-serviced assertion could satisfy a later, unrelated wait. In
  `transmit()` that reads as `TxDone` on a frame still in the air (the frame is
  popped, never retried, and `SetRx` is issued mid-transmission); in CAD it
  reads as "channel clear". Fixed at the root: `wait_high` (`radio.rs:387`) now
  runs an absolute deadline and re-reads `is_high()` after **every** wake, so
  `Asserted` ⇒ asserted now. The worst-case behaviour change is a `TxTimeout`,
  which retries. `firmware-core`'s reference model and its three stale-wake
  tests — which had certified the old behaviour as intentional — were rewritten
  to pin the postcondition.
- **The per-line display SPI floor was ~10× understated.** `display.rs`'s
  comment quoted 13 µs, which is `display-interface-spi`'s 64-byte *chunk*
  time, not a 640-byte line (~128 µs). Corrected at the comment and propagated
  through §4.1's table, the loop model's `ui_step` bound (5.0 → 30.72 ms) and
  every derived ratio. The full-window navigation paint is ~30.7 ms, not
  ~3.1 ms.
- **"UI service cadence improves alongside" is retracted as an unqualified
  claim.** At the high corner the split's dispatcher cadence advantage narrows
  and, for the smallest payload, the two topologies essentially tie. The
  headline longest-gap win is unaffected and holds everywhere (§5.2). §5.3 is
  the corrected account.
- **The "throttle bought 40 → 11" attribution is superseded, not retracted.**
  The measurement reproduces exactly; the credit moved. `UI_TICK_MS` (16 ms)
  now equals `RENDER_MIN_INTERVAL_MS` (16 ms), so the split supplies the same
  cap by construction in a quiet steady state (§3.3).
- **"Radio timeliness can only improve from a UI render throttle" is retracted
  as a *magnitude* claim.** It survives only as a sign claim, bounded at
  12.8 µs (§4.3).
- **§5's earlier "CONFIRMED, concrete" open hotspots — the per-dirty-line
  `Vec<Rgb565>` and the unconditional per-tick GPS/battery recompute — are
  closed, not open.** Both landed and are pinned (§3.4). The document had gone
  stale in both directions at once: retracted findings left standing, and
  landed fixes unrecorded. **When a number here changes, edit the body; do not
  append a layer.**
- **§3.1's bench figures were stale and are re-measured (2026-08-03).** The
  previously recorded `build_message_items` allocation counts (27 at n=10, 534
  at n=200) no longer reproduce: they are now **37** and **734**, with bytes
  per message up ~59 B. Cause: `normalize_emoji_for_display` was added to
  `build_message_items`' path by the inbound-emoji normalization work, adding
  one allocation per message. Nothing regressed structurally — cost is still
  linear, still per-navigation, still leak-free — but allocation counts are
  this document's "exact, does not move" class, and they moved without anyone
  noticing for several commits. That is the failure mode §2's last paragraph
  now warns about.
