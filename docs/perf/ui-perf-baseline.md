# MeshCadet UI & dispatcher performance — state of record

**Currency: 2026-08-02.** This is the single authoritative performance
document for MeshCadet. It supersedes and absorbs the earlier "Phase-1
measurement contract & baseline", including every in-place annotation and
retraction that had accumulated on top of it. There is one ledger here, not a
stack of superseded layers — where an earlier finding was wrong, the body
carries the *corrected* account and §9 records the correction in one line so
nobody re-derives the retracted number from an old harness output.

**Scope note.** This document covers what has been *measured or derived*. It
does not describe the in-flight `meshcadet-perf-rearchitecture` campaign's
proposed task/core split; that lives in that campaign's plan and its ADR. §6
states the structural finding that motivates it, because that finding is a
property of the code as it stands today and belongs in the record regardless
of what the campaign decides to do about it.

---

## 0. Provenance legend — read this before quoting any number

Every quantity in this document carries exactly one tag. A number without a
tag is a bug in this document.

| Tag | Meaning |
|---|---|
| **[HOST]** | Really executed on an x86-64 host by a committed test/bench in this repo. Reproducible by the command given in §2. |
| **[ANALYTICAL]** | Computed from a formula or datasheet constant that is itself in-repo and cited. Exact, but not an execution of the shipped firmware. |
| **[SIM]** | Produced by a host model of firmware behaviour rather than by running firmware. **No [SIM] number currently exists** — the loop model that will produce them is not yet built (`meshcadet-perf-loop-model-harness`). This row is here so the tag has a defined meaning before the first one lands. |
| **[ESTIMATE]** | A projection combining tagged inputs, or a reasoned bound. Never presented as measured. |
| **[DEFERRED-DEVICE]** | Not measured, because it requires the T-Deck (and sometimes a second peer node). Every one of these is enumerated in §8 with the procedure that closes it. |

**A [SIM] number may never be presented as a [DEFERRED-DEVICE] number
closed.** Simulation bounds a device measurement; it does not replace one.

### Why the host/device split is architectural, not a convenience

`firmware/` cross-compiles for `xtensa-esp32s3-espidf` and links
`esp-idf-svc`/`esp-idf-hal`; its `[[bin]]` sets `harness = false`, so
`cargo test` in `firmware/` only *type-checks* its `#[cfg(test)]` blocks —
the resulting binary is an Xtensa ELF that cannot execute on the host.
Anything that must actually *run* has to live outside `firmware/`. That is
why the host-testable logic was extracted into `firmware-core` (a root-
workspace crate, no Slint, no esp-idf) and why the render harnesses drive
Slint directly rather than importing firmware types.

Consequence for this campaign and every future one: **CI's
`firmware build gate (check-all-features.sh)` job is the compile/type oracle
for firmware.** This container has no `esp` toolchain; a change whose only
remaining risk is "does it cross-compile" is finished when it is pushed, not
when it is guessed at locally.

---

## 1. What the system is — the shape that produces every number below

One task does almost everything. `firmware/src/main.rs::run()` ends in a
single `loop {}` (`main.rs:1784`) on the ESP-IDF main task. Per iteration, in
order:

```
WDT pet (main.rs:1791)
  → GPS poll                     (gps.poll — duty-cycled UART1 NMEA read)
  → tx-timestamp rebase
  → battery poll                 (throttled ADC)
  → room keep-alive scheduler
  → CAD + TX                     (main.rs:2271 — SPI cmds + a DIO1 poll with a
                                  20 ms hard deadline, radio.rs:467-477; then
                                  radio.transmit() blocks for FULL AIRTIME,
                                  radio.rs:312-321)
  → RX poll                      (radio.try_receive, DIO1 watch ≤ RX_POLL_YIELD_MS
                                  = 5 ms, main.rs:1643/2393)
  → periodic RX stats / stack HWM (every 30 s)
  → ui.step()                    (main.rs:2593 — I2C1 touch + keyboard poll,
                                  Slint tick, render_if_needed → SPI2 line flushes)
  → drain UiCommand / handle events
```

Two auxiliary threads exist (`admin_server`, `provisioning_server`).
Cross-task state is four `static std::sync::Mutex<…>` snapshots written by
the main loop and read by those threads.

**No core affinity is set anywhere in the repository.** `pin_to_core`,
`ThreadSpawnConfiguration`, `Core::` return zero hits across `firmware/`, and
`sdkconfig.defaults` sets no `CONFIG_PTHREAD_TASK_CORE_ID` /
`CONFIG_ESP_MAIN_TASK_AFFINITY`. The main task therefore takes the IDF
default (CPU0); the aux threads take the pthread default (unpinned). Core 1
carries no application work of consequence today.

Two buses matter, and they are not the same bus:
- **SPI2** — shared by the LCD (ST7789, 40 MHz, `main.rs:741`) and the radio
  (SX1262, 8 MHz, `main.rs:1401`) as two `SpiDeviceDriver`s on one
  `SpiDriver` (`main.rs:676`).
- **I2C1** — touch (GT911) and the keyboard co-processor. Physically
  separate; **input polling never contends with radio or display**,
  regardless of rate.

---

## 2. Reproducing every number in this document

```sh
# Slint-based harnesses need a system sans-serif. In a container without
# fontconfig defaults, slint-build fails with "could not determine a default
# font for sans-serif" unless you set this:
export SLINT_DEFAULT_FONT=/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf

cargo test -p ui_perf --tests -- --nocapture          # §3.2, §3.3, §3.4, §4
cargo run  -p ui_perf --release --bin ui_perf_bench   # §3.1
cargo test -p ui_sim  --test perf_profile -- --nocapture  # §3.2
cargo test --workspace                                # everything else
```

Host numbers are deterministic modulo scheduling noise; timing figures move
±~15 % run-to-run on a typical dev box. Line/allocation counts are exact and
do not move.

---

## 3. Measured numbers — [HOST]

### 3.1 Render-logic cost (`ui_perf_bench`, release profile)

```
render_mentions[plain]:          ns_per_op = 40.1
render_mentions[other_mention]:  ns_per_op = 60.1
render_mentions[self_mention]:   ns_per_op = 59.8

build_message_items[n=10]:   ns_per_op =  1409   alloc=27  bytes=2010   net_live=0
build_message_items[n=50]:   ns_per_op =  8196   alloc=134 bytes=10023  net_live=0
build_message_items[n=200]:  ns_per_op = 31784   alloc=534 bytes=40323  net_live=0
```

Reading them: cost is **linear** in conversation size (~141–159 ns/record
across a 20× range, no quadratic blowup); ~2.7 allocator calls and ~75 bytes
per message; `net_live_bytes == 0` in every case (nothing leaks). Crucially
this cost is paid on `navigate_to_message_view` / `refresh_message_view_for`
— **once per conversation open or per new-message refresh, not per frame**.
Even at n=200 that is ~32 µs, three orders of magnitude under a frame budget.

These functions now live in `firmware_core::ui::message_view` and are
exercised by `firmware-core`'s own tests under `cargo test --workspace`;
`ui_perf::render_logic` re-exports them rather than porting them, so there is
exactly one implementation and one set of correctness tests.

**Portability caveat [ESTIMATE]:** the Xtensa LX7 @ 240 MHz will be slower in
absolute terms and firmware ships `opt-level = "z"` while this bench runs the
workspace release profile. Treat §3.1 as *relative/shape* truth (linearity,
allocation counts, which branch costs more), not as an on-target prediction.
Absolute on-target timing is [DEFERRED-DEVICE] (§8, D1).

### 3.2 Repaint scope — real Slint renderer, real `.slint` assets

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
RocketOnSend  peak dirty frame     lines =  28/240   px =   560   widest =  20   (21 dirty ticks)
comet sweep (motif_repaint)        39 animated frames, worst frame 14 lines, tallest bbox 14 px
```

- **Idle is a true no-op** (0 lines, 0 px) — a screen with no animation in
  flight and no navigation pending costs nothing per iteration on the render
  side, at any loop rate.
- **The foreground motifs are small.** `CometOnNotify` peaks at ~6 % of the
  frame; `RocketOnSend` at ~12 %, only 20 px wide. Every one-shot settles back
  to a 0-dirty steady state — the "never an infinite `animate`" contract
  `motifs.slint` claims, confirmed from the render side.
- **There is no full-window animated-backdrop problem.** `Starfield`, the
  window fill and the planet corner are static; they paint once on navigation
  and are not re-flushed while a motif moves.

### 3.3 The real repaint cost was the screen-entry fade — found, fixed, pinned

Every themed screen wraps its content in an `opacity: content_opacity` /
`reveal_opacity` binding (`contact_list.rs`, `message_view.rs`,
`pin_entry.rs`, `gps_status.rs`, `admin_menu.rs`, `unprovisioned.rs`, and —
scoped to its emoji-picker overlay — `compose.rs`).

**Mechanism, confirmed against `i-slint-core`'s own
`partial_renderer.rs::compute_dirty_regions`:** when an item's `opacity`
changes, Slint marks `must_refresh_children` for the whole subtree — "this
will impact all the children … regardless if they are themselves dirty or
not". A near-full-window `VerticalLayout { opacity: content_opacity; … }`
therefore re-dirties its **entire** bounding region on every tick the fade is
still interpolating. When this was measured `ui.step()` ran once per
dispatcher iteration, which idled near `RX_POLL_YIELD_MS` (then 5 ms,
~200 Hz), so an unthrottled render flushed the full region ~40 times for one
200 ms transition.

**Post-split (`meshcadet-perf-ui-residual-opt`, §9):** `step()` now runs on
`ui_task`, whose `recv_timeout` ceiling `UI_TICK_MS` is **16 ms** —
*identical* to `RENDER_MIN_INTERVAL_MS` below. The split therefore supplies
the same cadence cap by construction in a quiet steady state, and the
`40 → 11` win is now overwhelmingly attributable to M1 rather than to the
throttle. The throttle is still load-bearing under an event burst (`ui_task`
also wakes per queued event) and must not be removed;
`docs/perf/ui-residual-opt-r1.md` §4.1/§5 carries the full argument.

```
[entry-fade] unthrottled: 40 frames rendered, 40 of them full-window (320x240)
[entry-fade] throttled:   11 frames rendered, 11 of them full-window (320x240)
```

**Fix (landed):** `RENDER_MIN_INTERVAL_MS = 16` (~60 fps) in
`UiRuntime::step()` (`firmware/src/ui/mod.rs:907`, `:1984`) plus
`TDeckWindowAdapter::has_active_animations`.
`slint::platform::update_timers_and_animations()` still runs every iteration
unconditionally — every animated property stays exactly on its wall-clock
curve — and only the act of *flushing* a frame is capped, and only while an
animation is still settling. A fresh one-off redraw (navigation, incoming
message, model update) renders on the very next tick, uncapped, so
tap-to-first-frame is untouched. **72 % fewer full-window flushes, with the
final settled framebuffer asserted bit-for-bit identical (FNV-1a) between
throttled and unthrottled runs** — same curve, same duration, same easing,
same end state; only the sampling rate of an already-identical curve changes.

### 3.4 Landed allocation and repaint-scope fixes, with their pinned numbers

| Fix | Site | Before → after | Pinned by |
|---|---|---|---|
| Per-dirty-line heap `Vec<Rgb565>` removed from the flush path | `ui/platform.rs::process_line` + `ui/display.rs::flush_line_range` (`:271`, now takes an `impl ExactSizeIterator` and streams into `mipidsi::fill_contiguous`) | **240 allocs → 0** per full-window paint; 14 → 0 per `CometOnNotify` frame; 28 → 0 per `RocketOnSend` frame | `ui_perf/tests/flush_line_alloc.rs` — also asserts byte-identical pixels on both paths |
| GPS/battery status setters deduped | `UiRuntime::set_gps_status` / `set_battery_status` | GPS row: **700 allocs / 12 000 B → 35 allocs / 600 B** per 100 ticks. Battery row: **55 → 5** allocs per 50 ticks | `ui_perf/tests/alloc_tick_dedup.rs` |
| Live message-list update reconciled in place instead of wholesale model replace | `ui/screens/message_view.rs::set_messages` | **240 lines → 22 lines** flushed per incoming message (90 % fewer SPI line-flush cycles); static backdrop + header no longer re-flushed | `ui_perf/tests/model_update_repaint.rs` — also asserts pixel-identical final framebuffer |
| Screen-entry fade render-cadence throttle | `ui/mod.rs::step()` (§3.3) | **40 → 11** full-window flushes per 200 ms fade | `ui_perf/tests/entry_fade_repaint.rs` |

Each of these is a strict reduction in work with an asserted-identical visual
result, so none of them has a plausible regression direction for radio
timeliness — only a magnitude, which is [DEFERRED-DEVICE] (§8, D2).

---

## 4. Derived numbers — [ANALYTICAL]

### 4.1 Display SPI floor

A 320-pixel RGB565 line is 640 bytes; its pure data transfer at 40 MHz SPI2
is **~128 µs** (640 B × 8 bits / 40 MHz). `firmware/src/ui/display.rs:38`
previously documented this as "≤ 13 µs" — that figure is off by ~10×: it is
the per-64-byte-chunk transfer time `display-interface-spi` buffers each
line's writes into (`docs/perf/spi2-arbitration-r1.md` §Q5), not the whole
line, and has been corrected in that comment. `flush_line_range` is called
once per dirty line and additionally issues the ST7789 CASET/RASET/RAMWR
window-set commands per call; that per-transaction command overhead is
**not** quantified here and is [DEFERRED-DEVICE] (§8, D2).

| Dirty lines this frame | Data-only SPI floor (128 µs × lines) |
|---|---|
| 14 (`CometOnNotify` peak, §3.2) | ~1.8 ms |
| 22 (in-place message append, §3.4) | ~2.8 ms |
| 28 (`RocketOnSend` peak, §3.2) | ~3.6 ms |
| 240 (full `DISPLAY_HEIGHT`, navigation paint) | ~30.7 ms |

The middle two rows are [ESTIMATE]: an [ANALYTICAL] per-line floor multiplied
by a [HOST]-measured line count.

### 4.2 LoRa airtime — the dominant number in this document

`firmware-core/src/dispatcher.rs:316::lora_airtime_ms`, Semtech AN1200.13 §4,
at the locked SF7 / BW 62.5 kHz / CR 4:5 / 8-symbol-preamble / explicit-header
/ CRC-on preset (`firmware/src/radio.rs:611`). All four rows re-verified
against the formula on 2026-08-02.

| Payload | Airtime — `radio.transmit()` blocks the dispatcher loop this long |
|---|---|
| 10 B (ACK-shaped) | **83 ms** |
| 40 B (typical DM) | **165 ms** |
| 100 B | **349 ms** |
| 255 B (max) | **800 ms** |

This is RF airtime — the SX1262 transmitting — **not** SPI-bus-hold time. SPI2
is touched only for the initial `WRITE_BUFFER`/`SetTx` commands; the loop
then waits on the **DIO1 GPIO** for `TxDone`, not SPI (interrupt/
notification-driven as of `meshcadet-perf-radio-dio1-interrupt` — see §9 —
so "polls" above is no longer literal, though the RF-not-SPI distinction the
sentence exists to make still holds). `radio.try_receive`'s
`RX_POLL_YIELD_MS` = 20 ms window (retuned from 5 ms by the same mission) and
`channel_activity_detection`'s 20 ms deadline are likewise DIO1 GPIO watches,
not SPI holds.

---

## 5. Ledger — what is landed, what is open, what is out of scope

### 5.1 LANDED (measured, pinned by a committed host test)

Everything in §3.4, plus the demotions that measurement produced:

- **`build_message_items` / `render_mentions` allocation churn — COLD.**
  §3.1: cheap, linear, and per-navigation rather than per-frame. Not an
  optimization target. (It remains useful as the fixture set that keeps the
  extracted `firmware-core` logic honest.)
- **"Full-window animated backdrop" — does not exist.** §3.2: the backdrop
  layers are static and are painted once per navigation.
- **CAD backoff blocking sleep — already fixed before this record began.**
  `main.rs:2271`/`:2301`: the old `FreeRtos::delay_ms(backoff_ms)` full-thread
  stall on CAD-busy is a non-blocking deadline (`cad_backoff_until_ms`).
  `run_splash_ripple` remains a one-time ~1.15 s boot-only blocking window on
  its own dedicated render loop, analysed and accepted in that method's doc
  (RX stays correct — continuous-RX latching — only a bounded, boot-only
  polling gap).
- **The two residual UI-side items are CLOSED and DEMOTED respectively — M3
  landed no optimization, deliberately.** `meshcadet-perf-ui-residual-opt`
  re-ran both host instruments against the post-split tree and re-ranked:
  the per-dirty-line `Vec<Rgb565>` is measurably at **zero** (nothing left to
  do), and the fade's repaint scope is demoted on three grounds — the split
  already supplies the cadence cap, `RENDER_MIN_INTERVAL_MS` is provably
  un-tightenable (a full-window flush is longer than any cap worth setting),
  and post-split the fade's worst-case cost to the *radio* is **12.8 µs**,
  not 30.7 ms. Full argument and verdict table:
  `docs/perf/ui-residual-opt-r1.md`.

### 5.2 OPEN — the structural item, and it dwarfs everything above

**`radio.transmit()` blocks the dispatcher task for the full LoRa
airtime** — 83 ms for an ACK, up to 800 ms for a 255 B frame (§4.2). The
`ui.step()`/single-shared-task framing this paragraph originally argued from
is itself superseded: ADR-0012 (`meshcadet-perf-ui-task-split`) moved
touch/keyboard/render onto their own core-1-pinned `ui_task`, so `ui.step()`
no longer shares a task with `radio.transmit()` at all — see §9. What
remains open, post-split, is narrower: the dispatcher task still spends the
full airtime unable to service GPS/battery/room-keepalive/CAD/RX, which is
why RX-notice latency and
CAD-attempt cadence under load are still live questions for this campaign
(plan §6 criterion 2).

**CORRECTION (`meshcadet-perf-radio-dio1-interrupt`):** the
`while !dio1.is_high() { FreeRtos::delay_ms(1) }` spin-poll this paragraph
used to quote (at the old `radio.rs:312-321`) no longer exists — `transmit`/
`try_receive`/`channel_activity_detection` now block on an interrupt/
notification-driven wait (`firmware/src/radio.rs`'s `GpioDio1Wait`). The
BLOCKING DURATION is unchanged (still the full analytically-computed
airtime, per the table above) — only the wait MECHANISM changed, from a
1 ms-quantized busy-poll to a single blocking wait with no polling
quantization and no per-tick scheduler wake. See §9.

Set against the largest UI-side cost ever measured here (§4.1: ~30.7 ms for a
full-window navigation paint), **the structural item is 2.7×–26× larger.**

The earlier record classified TX airtime as "out of scope — no UI change can
affect this." That was correct *for a pass scoped to UI changes*, and it is
the wrong frame for the system as a whole: the change that fixes it is a
task/core split, not a UI change. That is the subject of the in-flight
`meshcadet-perf-rearchitecture` campaign, and it is why this item is recorded
as OPEN here rather than as out of scope.

**Secondary open item, same root:** `main.rs:1784`'s loop is pinned to CPU0 by
IDF default and nothing else runs on CPU1 (§1). Core 1 is idle capacity that
no measurement here has ever been able to use.

### 5.3 NOT CONTENTION — settled, do not re-litigate

- **Touch/keyboard vs. radio/display.** I2C1 is a physically separate bus from
  SPI2. Zero contention at any poll rate.
- **TX airtime vs. the SPI bus.** §4.2: airtime is RF, not bus-hold. The UI
  cannot make airtime longer or shorter. (It is still the dominant *task*
  blocker — §5.2. "Not SPI contention" and "not a problem" are different
  claims, and conflating them is exactly the error §9 records.)

---

## 6. UI ↔ radio contention map

The shared resource is **SPI2**. On one task the two devices are
software-serialised and never *concurrently* contend; the coupling is
**sequential latency injection** — whichever operation runs first in an
iteration delays everything after it in that iteration, and delays the next
iteration by the same amount.

```
[ WDT ] [ GPS ] [ tx-rebase ] [ Batt ] [ room keep-alive ]
   → [ CAD/TX ]  SPI cmds + DIO1 watch ≤20 ms when a TX is pending,
                 THEN transmit() blocks 83–800 ms of airtime  ← DOMINANT (§5.2)
   → [ RX poll ] DIO1 watch ≤5 ms (GPIO, not SPI)
   → [ ui.step() ] touch/kbd (I2C1, separate bus) + Slint tick
                   + render_if_needed → flush_line_range × dirty_lines
                     (§3.2: 0 idle / 14 comet / 22 msg-append / 28 rocket / 240 nav)
   → (loop)  next iteration's CAD attempt + RX poll start only after step() returns
```

**Direction A — UI delays radio.** `ui.step()`'s SPI-hold time is subtracted
from how soon the loop reaches the next CAD attempt and RX poll. §3.2's
measured line counts × §4.1's floor give [ESTIMATE]: idle ≈ 0 ms, comet
≈ 1.8 ms, message append ≈ 2.8 ms, rocket ≈ 3.6 ms, full navigation paint
≈ 30.7 ms. Correctness is not at risk — `try_receive`'s continuous-RX latching
means no packet is missed, only *when this task notices it* shifts. The three
animation-triggered costs (comet/message-append/rocket) stay below the 5 ms
`RX_POLL_YIELD_MS` window and cannot starve even a single RX-poll iteration;
**the full-window navigation paint (~30.7 ms) now exceeds that window by
~6×**, so a navigation transition can push one RX-poll iteration's notice out
by roughly that much — a timing shift, not a correctness gap, and still
roughly one order of magnitude below the airtime blocks that dominate this
budget regardless (§5.2: the structural item is 2.7×–26× larger).

**Direction B — radio delays UI.** Two mechanisms, an order of magnitude
apart:
- CAD's own ≤20 ms blocking window (`radio.rs:467-477`) runs *before*
  `ui.step()`, so a pending outbound message can delay that iteration's
  render/input poll by up to ~20 ms. That is ~6× direction A's worst measured
  cost.
- `transmit()`'s 83–800 ms airtime block (§5.2), which is 4–40× worse again
  and is the reason this section exists at all.

**Direction B dominates by construction**, and the dominant term within it is
airtime, not SPI. Any future analysis that measures only direction A is
measuring the smaller half.

---

## 7. Instrumentation status

The device carries **no timing instrumentation on `main`** today: the only
on-device telemetry is a 30 s RxDone/CrcErr/none counter rollup and a stack
high-water-mark log. Every §8 predicate is unmeasurable until that changes.

**In flight, not yet merged:** `--features diagnostics`-gated on-device
instrumentation — per-phase superloop min/mean/max/p95 (GPS, battery, CAD, TX,
RX-poll, `ui.step()`), a UI-starvation counter (cumulative ms + longest gap),
input-to-first-paint latency, an RX-notice-latency proxy, and per-core
utilization via `vTaskGetRunTimeStats()` plus three new FreeRTOS Kconfig
options. Its pure computation (histogram, percentile math, runtime-stats text
parser) lives host-tested in `firmware-core::perf`. **PR
jagoda/meshcadet#120** — CI green, awaiting review/merge. **Do not treat any
§8 predicate as runnable until it merges**; the collection kit's log format
depends on it.

Stack budget, for context on why the instrumentation stores histograms rather
than samples: the main task is at 49 152 B after one confirmed release-only
overflow, with HWM sampled around 26.8 KB peak.

---

## 8. Deferred-predicate register — everything that needs silicon

This is the consolidated list. Nothing needing a device is recorded anywhere
else in this document; every [DEFERRED-DEVICE] tag above points here.

**Prerequisites for all of them:** PR #120 merged (§7), a flashed T-Deck Plus
built `--features diagnostics`, and — for D4–D6 — a second MeshCore-speaking
peer node (`docs/hil-real-mesh-procedure.md`). D3 needs only the diagnostics
build and a scripted usage session on the single flashed device, no peer.

**Procedure source.** §8.1 below is the interim operator procedure. The
campaign's planned `docs/perf/collection-kit.md` supersedes it once authored;
until then this is what an operator runs.

| # | Predicate | Closes | Procedure |
|---|---|---|---|
| **D1** | Absolute on-target cost of §3.1's render logic under `opt-level = "z"` on Xtensa LX7 @ 240 MHz | The §3.1 portability caveat | Diagnostics per-phase `ui.step()` rollup, ContactList idle vs. a 200-message conversation open |
| **D2** | Real per-`flush_line_range` SPI command overhead (CASET/RASET/RAMWR), beyond §4.1's data-only floor | The §4.1 floor's error bar; §3.4's "no plausible regression direction" magnitude | Diagnostics per-phase `ui.step()` rollup vs. dirty-line count, navigation paint vs. idle |
| **D3** | Real dirty-line-count distribution in actual use (idle / navigation / send-tap / incoming message) | §3.2's synthetic-scene → real-usage gap | Diagnostics rollup over a 5-minute scripted usage session |
| **D4** | Longest UI-unserviced gap and its scaling with payload size | §5.2's structural claim, on silicon | Diagnostics UI-starvation counter while the peer sends 20 DMs of 10 B / 40 B / 255 B |
| **D5** | Delivery success rate — DM TX-with-ACK, DM RX, GRP_TXT, room push | The hard no-regression constraint (any future change diffs against this) | 20-DM exchange each way with the peer, UI idle *and* UI under navigation load; count RxDone / CAD-busy / TX-retry from the serial log |
| **D6** | RX-notice latency: peer send timestamp → this device's `RX RxDone` line, UI-idle vs. UI-active | Direction A's real magnitude | Same run as D5, differenced |
| **D7** | Per-core utilization | §1's "core 1 carries no work" claim, and any future split's core assignment | Diagnostics `vTaskGetRunTimeStats()` rollup (`IDLE0`/`IDLE1` rows) |
| **D8** | Post-change stack high-water mark per task | §7's 49 152 B / ~26.8 KB budget after any task restructure | Existing 30 s stack-HWM log |
| **D9** | SPI2 bus arbitration behaviour under a *concurrent* LCD flush and radio TX from different tasks/cores | The one risk that threatens delivery correctness if the loop is ever split. **The static half is a source-and-datasheet question and is not deferred** — ESP-IDF `spi_master` serialises transactions per bus across devices added with `spi_bus_add_device`, and `esp-idf-hal`'s `SpiDeviceDriver` wraps exactly that; what cannot be settled on paper is real contention *behaviour* at two different baudrates (40 MHz LCD / 8 MHz radio) | Only meaningful after a split exists. Probe to be specified by the SPI2 arbitration analysis |
| **D10** | Felt frame rate / tap-to-first-frame | Human-perceptible responsiveness, which no counter captures | §8.1.A below |

**Emulation does not close any of these.** Espressif's QEMU fork emulates the
ESP32-S3 CPU, memory, flash-SPI, PSRAM, crypto and timers, but models no
general-purpose SPI2 slave devices (no ST7789, no SX1262), no DIO1/BUSY GPIO
semantics, no I2C touch/keyboard, and no RF — and is documented as
non-cycle-accurate. It cannot boot this application past display/radio init,
and any timing number it produced would be fiction.

### 8.1 Interim operator procedure

**A. Felt snappiness (D10)**
1. [ ] Cold boot → wall-clock backlight-on → splash first frame, and
       splash-dismiss → ContactList first frame (two numbers).
2. [ ] From ContactList, tap into a ~20+ message conversation with **no**
       motif firing — time tap-to-first-frame (isolates nav-only cost).
3. [ ] Repeat immediately after a message arrives (so `CometOnNotify` IS
       active) — compare against step 2 (isolates the motif's added cost).
4. [ ] Compose → Send → time tap-to-`RocketOnSend`-first-frame and
       first-frame-to-MessageView-return; confirm the animation completes
       before the screen swaps (no visible pop/skip).
5. [ ] Record slow-motion video (120/240 fps) of one full screen transition
       and one motif firing; count frames input→first visible change.

**B. Radio timing under UI load (D4, D5, D6)**

6. [ ] With the peer, send 20 DMs peer→T-Deck **while idly navigating** the
       T-Deck UI (tap between ContactList/MessageView every few seconds).
       Log CAD-busy count, TX-retry count and RxDone timestamps from the
       serial console.
7. [ ] Repeat step 6 with the T-Deck UI fully idle (screen asleep, no taps)
       as the CONTROL. Difference the two for D6.
8. [ ] Repeat step 6 at 10 B, 40 B and 255 B payloads for D4's payload-size
       scaling; read the UI-starvation counter after each.
9. [ ] Trigger a T-Deck→peer DM **while** a screen transition or motif is
       mid-flight; confirm no new error class in the CAD/TX log lines and
       that the peer receives the DM correctly (correctness, not timing).
10. [ ] With a DM queued (`txq.has_pending()`), repeat step 2's tap-timing for
       taps landing while a CAD attempt is in flight vs. no TX pending —
       direction B's CAD component, analytically bounded at ≤20 ms.

**C. Recording**

11. [ ] Paste the raw serial console excerpt (per-phase rollup + CAD/TX/RX
        lines with timestamps) and the slow-mo frame counts into the record
        for the change under test. When a predicate closes, move its number
        into the body of this document with a **[HOST]**-equivalent
        **[DEVICE]** tag and strike its row from §8.

---

## 9. Corrections history

Kept deliberately short. This exists so a retracted number cannot be
resurrected from an old harness output, not as a second narrative.

- **`RocketOnSend`'s 86-line / 200 px-wide peak (earlier §3b/§5.3/§7) is
  retracted.** It was a measurement artifact of `ui_sim`'s *shared* scene,
  which also carried `MascotBob` (450 ms) and `Twinkle` (900 ms) entry-settle
  animations still in flight when `RocketOnSend` fired; the software
  renderer's `DirtyRegion` 3-rectangle cap
  (`PHYSICAL_REGION_MAX_SIZE = 3`) merged their unrelated dirty rects into one
  inflated bounding box. Neither `compose.rs` nor `message_view.rs` — the only
  real `RocketOnSend` consumers — imports `MascotBob`/`Twinkle`, so this never
  occurred in production. The harness has since been corrected; the current
  measured peak is **28 lines / 20 px wide** (§3.2). Any output still showing
  86 is stale.
- **The "prime suspect = translate+fade one-shot motif" framing is
  retracted.** The real cost was the screen-entry `opacity` fade, one level up
  (§3.3), and it is fixed.
- **"TX airtime is out of scope" is superseded, not retracted.** It was
  correct as scoped ("no *UI* change can affect this") and wrong as a
  system-level conclusion. See §5.2 and §5.3's second bullet.
- **§5.2's "`ui.step()` shares a task with `radio.transmit()`" framing is
  superseded, not retracted.** It was correct when written. ADR-0012
  (`meshcadet-perf-ui-task-split`) moved touch/keyboard/render onto their own
  core-1-pinned `ui_task`; the dispatcher task (radio + GPS + battery + room
  keep-alive) can no longer even name `UiRuntime` (main.rs's own `mod ui_task`
  boundary comment). §5.2's edited to reflect this; the narrower item that
  remains open — the dispatcher task itself still can't service anything else
  during a TX/RX/CAD window — is unaffected by the split and is what this
  campaign's M2 (`meshcadet-perf-radio-dio1-interrupt`) and its host-validation
  sibling are measuring.
- **§5.2's `while !dio1.is_high() { FreeRtos::delay_ms(1) }` spin-poll quote
  is retracted — the code it quoted is gone.** `meshcadet-perf-radio-dio1-
  interrupt` replaced all three DIO1 spin-polls (`transmit`/`try_receive`/
  `channel_activity_detection`) with an interrupt/notification-driven wait
  (`firmware/src/radio.rs`'s `GpioDio1Wait`). The blocking DURATION §5.2's
  table reports is unchanged (still the full analytical airtime); only the
  wait mechanism changed. Every `radio.rs:NNN-NNN` line citation elsewhere in
  this document predates that edit and should be treated as approximate, not
  re-verified line-by-line here — the M4 campaign synthesis
  (`meshcadet-perf-campaign-synthesis`) is where this document's citations get
  a single consolidated re-pass.
- **§5's earlier "CONFIRMED, concrete" open hotspots — the per-dirty-line
  `Vec<Rgb565>` and the unconditional per-tick GPS/battery recompute — are
  closed, not open.** Both landed and are pinned (§3.4). The earlier document
  went stale in both directions at once: retracted findings left standing, and
  landed fixes unrecorded. That is the failure mode this consolidated record
  exists to prevent — **when a number here changes, edit the body; do not
  append a layer.**
- **§3.3's "the throttle bought 40 → 11" attribution is superseded, not
  retracted.** The measurement is exactly reproducible and unchanged; what
  changed is who deserves the credit. `ui_task::UI_TICK_MS` (16 ms) now
  equals `RENDER_MIN_INTERVAL_MS` (16 ms), so M1's split supplies the same
  cap by construction in a quiet steady state. §3.3's body is edited;
  `meshcadet-perf-ui-residual-opt` (`docs/perf/ui-residual-opt-r1.md`) carries
  the derivation, and §5.1's last bullet the verdict.
- **"Radio timeliness can only improve from a UI render throttle" is
  retracted as a *magnitude* claim.** It survives only as a sign claim. Post
  ADR-0012 the LCD and the radio are on different tasks and cores, SPI2 is
  re-arbitrated after every elementary transaction, and the worst case a
  full-window flush can impose on a radio SPI command is one 64-byte chunk at
  40 MHz — **12.8 µs**, not the flush's ~30.7 ms
  (`spi2-arbitration-r1.md` Q5). Any reasoning that treats UI render cost as
  a first-order input to radio timeliness is reasoning from the pre-split
  world.
