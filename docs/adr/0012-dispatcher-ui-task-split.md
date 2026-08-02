# ADR-0012 — Dispatcher/UI Task Split: Radio on Core 0, UI on Core 1, Message Queues Across the Boundary

- **Status:** Accepted (2026-08-02)
- **Deciders:** Maintainer design review (`meshcadet-perf-rearchitecture` campaign, M1)
- **Supersedes:** —
- **Implements:** the M1 design child (`meshcadet-perf-task-split-adr`) of the
  `meshcadet-perf-rearchitecture` campaign. **This ADR IS the contract** the
  implementation child (`meshcadet-perf-ui-task-split`) consumes and the
  validation child (`meshcadet-perf-task-split-host-validation`) checks
  against. The task topology, the ownership partition, the queue-boundary
  message contract, the per-task stack budgets, and the boot sequence below
  are frozen as of this ADR's acceptance; a breaking change to any of them
  needs an ADR revision, not a silent edit.
- **Consumes (do not re-derive):**
  - `docs/perf/spi2-arbitration-r1.md` — R1's verdict, with citations. This
    ADR cites it; it does not restate its analysis.
  - `docs/perf/perf-loop-model-baseline.md` — the simulated M0 baseline that
    predicts this split's delta.
  - `docs/perf/ui-perf-baseline.md` — the analytical airtime/SPI-floor tables.
  - `docs/perf/collection-kit.md` — where this ADR's device-only predicates
    are handed off.
- **Code:** none. This is a design-only ADR; no implementation lands with it.

## Context

`firmware/src/main.rs::run()` ends in a single `loop {}` (line 1811) running
on the ESP-IDF main task. That one loop does GPS poll, battery poll, room
keep-alive, CAD + TX, RX poll, periodic stats, `ui.step()`, and the
`UiCommand` drain — in that order, every iteration. `radio.transmit()`
(`firmware/src/radio.rs:276`) issues the SPI write and then spins
`while !dio1.is_high() { FreeRtos::delay_ms(1) }` for the **full LoRa
airtime**: 83 ms for an ACK-shaped 10 B frame, up to 800 ms for a 255 B
frame at the locked SF7 / BW 62.5 kHz / CR 4:5 preset. `ui.step()` — the
only place touch, keyboard, trackball and render happen — runs *after* that
in the same iteration. For the whole airtime window the UI is not slow, it
is **not sampled at all**.

The M0 loop model quantified it (`docs/perf/perf-loop-model-baseline.md`):
longest UI-unserviced gap **828.11 ms** for the current single-loop topology
at the high corner with 255 B payloads, versus **10.00 ms** for the modelled
split — an 83× improvement at that cell, 164× at another, and the split's
gap does not scale with LoRa payload size *at all* because its UI task never
touches `lora_airtime_ms`/`TxQueue`. The M0 checkpoint
(`meshcadet-perf-baseline-checkpoint`) returned **GO** and confirmed the
campaign's reroute condition does **not** fire: radio-TX blocking dominates
the UI-unserviced gap at all 12 corner/payload cells.

Three maintainer-set constraints govern, in this priority order:

1. **Radio responsiveness — messages must ALWAYS be transmitted and
   received.** A correctness requirement, not a latency one.
2. **UI responsiveness.**
3. **Maximize use of both cores.** No core affinity is set anywhere in this
   repository today; core 1 runs no application work of consequence.

Plus one overriding constraint: **all functionality must remain, nothing may
regress**, with an explicit functional-parity argument and regression-check
strategy attached.

And one procedural constraint (campaign plan §0.5): **no hardware in the
loop.** No mission in this campaign flashes a device. Every claim below is
settled by source reading, by CI's xtensa cross-compile, by the host loop
model, or by the host harnesses — or it is recorded under "Deferred
predicates" below as an operator-run item and handed to the collection kit.
Nothing
is invented.

**The de-risker that makes a change this size reasonable rather than
reckless:** `UiRuntime` already talks to the dispatcher through `UiEvent`
in (`post_event`, `firmware/src/ui/mod.rs:1495`) and `UiCommand` out
(`drain_commands`, `:1500`), buffered in two plain `Vec`s
(`ui/mod.rs:533-535`). The exchange is *already* message-shaped. This ADR is
largely "make an existing synchronous message exchange asynchronous across a
queue" — not a rewrite.

## Decision

### D1 — Task topology and core affinity

Post-split the firmware runs **two application tasks** plus the two
pre-existing auxiliary threads:

| Task | Core | Priority | Stack | Created by |
|---|---|---|---|---|
| `main` (dispatcher: radio, GPS, battery, room, history) | **0** | 1 (`CONFIG_ESP_MAIN_TASK_PRIORITY`) | 49 152 B (`CONFIG_ESP_MAIN_TASK_STACK_SIZE`, unchanged) | ESP-IDF startup |
| `ui_task` (**NEW**: display, touch, keyboard, trackball, buzzer, all of Slint) | **1** | 5 (pthread default) | **32 768 B** (see D6) | `std::thread::Builder` under `ThreadSpawnConfiguration` |
| `admin_server` | unpinned (unchanged) | 5 | 12 288 B (unchanged) | `main.rs:1577` |
| `prov_server` (unprovisioned boot only) | unpinned (unchanged) | 5 | 8 192 B (unchanged) | `main.rs:958` |

**The UI moves; the radio stays.** The dispatcher keeps the ESP-IDF main
task. Rationale: the dispatcher's state is large and deeply entangled with
`run()`'s locals (`PolicyFilter`, `TxQueue`, `AirtimeBudget`,
`DuplicateFilter`, `pending_ack`, room runtime, identity, NVS handles) with
no pre-existing message boundary, and the main task's 49 152 B Kconfig stack
was sized for exactly that path plus crypto. The UI, by contrast, is a
single self-contained `UiRuntime` object behind a ready-made event/command
interface. Moving the half that already has the interface is the smaller,
safer change. See "Alternatives considered" (A2) for the rejected inverse.

**Affinity is set explicitly, not inherited.** `ui_task` is pinned with
`esp_idf_hal::task::thread::ThreadSpawnConfiguration { name, stack_size,
priority, inherit: false, pin_to_core: Some(Core::Core1) }`, `.set()` before
`std::thread::Builder::spawn` and restored to `Default` immediately after
(the config is a pending thread-local applied to the *next* spawn). The main
task's CPU0 affinity is today an inherited IDF default; the implementation
**adds `CONFIG_ESP_MAIN_TASK_AFFINITY_CPU0=y` to
`firmware/sdkconfig.defaults`** so the invariant this ADR depends on is
stated in the repo rather than assumed from a default.

**Priority is not the arbiter here — affinity is.** Two tasks pinned to
different cores do not compete for a run slot, so the fact that `ui_task`
runs at pthread priority 5 above the main task's priority 1 is not a
priority-inversion hazard on the hot path. It is worth recording that
`admin_server` and `prov_server` *already* run at priority 5, unpinned, and
can therefore already preempt the priority-1 dispatcher on core 0 today —
unchanged by this ADR, and out of scope for it.

**The auxiliary threads stay unpinned.** They spend nearly all their time
blocked on I/O, and there is no measurement that says otherwise. Pinning
them is a knob for a later pass, with the collection kit's
`PERF core-utilization` reading as the input that would justify it.

### D2 — Ownership partition: every peripheral handle has exactly one owner

**Rule: no peripheral driver handle is shared between tasks.** Each is moved
into, and thereafter exclusively owned by, one task. This is precisely the
usage pattern `docs/perf/spi2-arbitration-r1.md` §Q1-Q2 identifies as the
one ESP-IDF documents and supports ("each device touched by exactly one
task").

| Owner | Peripherals / state |
|---|---|
| `main` (dispatcher) | SX1262 `SpiDeviceDriver` @ 8 MHz (CS GPIO9) + RST/BUSY/DIO1; GPS UART1; battery ADC; `HISTORY`/`GPS_STATUS`/`BATTERY_STATUS`/`ROOM_CLOCK_SOURCE` writes; all NVS writes; `TxQueue`/`AirtimeBudget`/`DuplicateFilter`/`PolicyFilter`; room session state |
| `ui_task` | ST7789 `SpiDeviceDriver` @ 40 MHz (CS GPIO12) + DC/RST; LEDC backlight (GPIO42, timer1/ch1); I2C1 (GT911 touch @0x5D, ESP32-C3 keyboard @0x55, trackball); I2S0 buzzer; **the entire Slint runtime and every `UiRuntime` field** |
| `admin_server` | its own `Box<ProvisionedConfig>` + NVS handle (unchanged); reads the four `static Mutex` snapshots (unchanged) |

The one deliberately shared object is the SPI2 **bus** (`SpiDriver`,
`main.rs:683`), reached only through two independent `SpiDeviceDriver`s, one
per task — which is the supported model, with in-driver per-bus arbitration
(R1, D10).

**Corollary that removes a boot-time question outright: every
`spi_bus_add_device` call happens on the main task, before `ui_task`
exists.** The implementation moves the radio's `SpiDeviceDriver::new`
(`main.rs:1405`) up beside the LCD's (`main.rs:745`) — both need only the
`&SpiDriver` and their CS pin, neither depends on the provisioning gate —
so device *registration* stays strictly sequential and single-threaded
exactly as today, and only SPI *transactions* ever become concurrent. That
is the narrow thing R1 actually cleared; do not widen it. `Radio::init`'s
chip bring-up stays where it is.

### D3 — The queue-boundary contract

This replaces the synchronous exchange at `main.rs:2762-2797`
(`ui.step()` → `ui.drain_commands()`) and the `Vec` buffers at
`ui/mod.rs:533-535`.

**Two `std::sync::mpsc` bounded channels, one per direction:**

```
dispatcher ──Sender<UiEvent>────────▶ Receiver<UiEvent>   ui_task   (cap 32)
dispatcher ◀─Receiver<UiCommand>───── Sender<UiCommand>   ui_task   (cap 16)
```

Chosen over FreeRTOS native queues because `UiEvent`/`UiCommand` carry
`String` payloads: `xQueueSend` copies bytes and would force fixed-size
arrays or pointer smuggling, while mpsc *moves* the heap-owned payload for
free. See "Alternatives considered" (A4).

**C1 — Both types are already `Send`, unchanged.** Every `UiEvent` variant
(`ui/mod.rs:132-299`) and every `UiCommand` variant (`:303-325`) is built
from `u8`/`u32`/`i32`/`bool`/`String` only. No `Rc`, no raw pointer, no
peripheral handle. Sending them across a task boundary requires **no change
to either enum's existing variants**. The new variants added by C4/C5 below
must preserve this property, and the implementation must not add a variant
carrying an `Rc`, a Slint type, or a peripheral handle.

**C2 — Neither task may ever block on the other.** Both directions use
`SyncSender::try_send`. This is the hard rule that makes R4 (priority
inversion) unreachable across this boundary: a full queue degrades, it never
stalls.

- **Dispatcher → UI (events), on full:** drop the event, increment a
  counter, log at `warn` once per rollup window. Losing a UI notification is
  a cosmetic degradation; blocking the dispatcher would risk a missed CAD
  window or a late RX drain, which is a priority-1 violation. Capacity 32
  against a steady-state production of ≲2 events per dispatcher iteration
  makes this unreachable in practice; it is a safety valve, not a design
  path.
- **UI → dispatcher (commands), on full:** the UI must **surface the
  failure to the user**, not silently drop. Capacity 16 against
  human-typing-rate production (one command per Send press) makes this
  unreachable in practice — but the degradation, if it ever happened, is the
  same class the system already accepts one stage downstream, where
  `TxQueue` evicts and `log_tx_queue_eviction` (`main.rs:2815`) records it.
  The implementation surfaces it via the existing refusal path shape
  (`UiEvent::RoomPostRefused`, `ui/mod.rs:295-298`) generalised to DMs and
  channel messages.

**C3 — Ordering is preserved exactly.** Each channel has a single producer,
so mpsc's FIFO gives total order per direction — identical to the `Vec`
push/drain it replaces. The only ordering change is that an event posted
mid-iteration (e.g. `RoomPostSent` at `main.rs:3045`) becomes visible to the
UI as soon as it is sent rather than at end-of-iteration; the UI still
consumes it on its next tick, so observable semantics are unchanged.

**C4 — Per-iteration state snapshots become change-detected events.** Four
setters are called every dispatcher iteration today with almost always
unchanged values:

| Today (inline, every iteration) | New event |
|---|---|
| `ui.set_gps_status(..)` `main.rs:1915` | `UiEvent::GpsStatusChanged(GpsStatus)` |
| `ui.set_room_clock_source(..)` `main.rs:1948` | `UiEvent::RoomClockChanged { source, wall_clock_secs, age_secs }` |
| `ui.set_battery_status(..)` `main.rs:1966` | `UiEvent::BatteryStatusChanged(BatteryStatus)` |
| `ui.set_signal_level(..)` `main.rs:1981` | `UiEvent::SignalLevelChanged(SignalLevel)` |

**The dispatcher holds the last-sent value and sends only on change.** All
four payload types live in `firmware-core`, are `Copy + PartialEq + Eq`
(`firmware-core/src/gps.rs:241`, `battery.rs:324`, `signal_tracker.rs:64`,
`room_session.rs:570`), and carry no `esp-idf` dependency — so the
comparison is a cheap host-testable equality on plain data. This is the
*same* comparison `UiRuntime::set_gps_status` already performs as an early
return (`ui/mod.rs:1385-1396`), relocated to the sender; the UI keeps its own
early return as defence in depth. Net effect: queue traffic drops far below
today's call rate rather than rising.

Chosen over a `Mutex<Snapshot>` + dirty flag specifically to keep R4's "no
shared locks on the hot path" absolute rather than nearly-absolute.

**C5 — Boot-time seed is one message, not fourteen.** The setters that run
exactly once during bring-up (`register_room` `main.rs:1117`,
`register_contact` `:1154`, `set_channels` `:1194`, `set_pin` `:1199`,
`set_runtime_settings` `:1215`, `seed_conversation` `:1552`) are bundled
into a single `UiEvent::BootSeed(Box<BootSeed>)`. Boxed so the enum's size
— and therefore every one of the 32 queue slots — is not inflated by the
largest variant, which matters against the DRAM budget in D6.

**C6 — NVS stays single-owner: the UI never writes flash.**
`ui.set_nvs_partition(..)` (`main.rs:1211`) is **deleted**, along with
`UiRuntime`'s `nvs_partition` field (`ui/mod.rs:594`) and its four
persistence call sites (`ui/mod.rs:2695/2705/2718/2729`). The admin menu's
runtime-settings toggles instead emit
`UiCommand::PersistRuntimeSettings(RuntimeSettings)`; the dispatcher
persists to the `mc_rts` namespace
(`firmware/src/runtime_settings_store.rs:22`). `RuntimeSettings` is plain
`Clone + PartialEq + Eq` data in `firmware-core`
(`firmware-core/src/pin_menu.rs:43`), so it crosses the queue trivially.

Three things this buys: it removes any question of whether
`EspNvsPartition<NvsDefault>` is `Send`; it removes cross-task NVS
concurrency entirely (one writer, ever); and it keeps a flash write — a
multi-millisecond, erase-block-bound operation — off the render task. The
in-memory toggle still applies immediately on the UI task, so the visible
behaviour of flipping a toggle is unchanged.

**C7 — Wake and idle discipline.**

- `ui_task`: `rx.recv_timeout(UI_TICK_MS)`, `UI_TICK_MS = 16` (~60 Hz
  ceiling). It wakes on a message *or* on the tick deadline, so animations
  advance on a steady cadence with no busy-wait. The existing
  `RENDER_MIN_INTERVAL_MS` throttle (`ui/mod.rs:791-820`) still gates actual
  repaint, unchanged. This constant is the direct analogue of the loop
  model's `split_ui_idle_tick` parameter
  (`perf_loop_model/src/sim.rs:356`), which is what lets the as-built
  topology be re-simulated against the M0 prediction ("Regression-check
  strategy", leg 1).
- Dispatcher: a non-blocking `while let Ok(cmd) = cmd_rx.try_recv()` drain,
  at exactly the point `drain_commands()` runs today (`main.rs:2797`).

**C8 — The queue is the *only* channel between the two tasks.** No shared
`Mutex`, no `Arc<AtomicX>`, no `static mut` is added by this split. See D9.

### D4 — Slint is owned end-to-end by `ui_task`, enforced by a static guard

`firmware/Cargo.toml:89-105` selects `unsafe-single-threaded` on **both**
`slint` and `i-slint-core`. That feature *removes* Slint's thread-affinity
checks rather than satisfying them. Every Slint interaction — platform
registration (`ui/platform.rs:98`), bitmap-font registration (`:110`),
window-adapter creation, component construction, **every property write**,
every render — must occur on one and the same thread, and nothing in the
build will tell you if it does not.

**D4.1 — `UiRuntime` is constructed *on* `ui_task`, not moved onto it.**
The task is spawned first; its entry point performs display bring-up,
`TDeckPlatform::install()`, and `UiRuntime::new()`. Whether Slint's platform
singleton lives in a `thread_local!` or in an unsafe-`Sync` `static` is a
slint-internal implementation detail that this firmware must not depend on;
constructing *and* using on one thread is correct under either. Moving an
already-constructed `UiRuntime` across a task boundary would be correct only
under one of those two readings, and the build would not tell us which.

**D4.2 — The rule is a convention, mechanically checked by a static guard,
not by Rust visibility.** The implementation adds `firmware/src/ui_task.rs`,
which holds the **only** `use crate::ui::UiRuntime` in the crate and exposes
exactly one item:

```rust
pub(crate) fn spawn(/* peripherals + channel endpoints */)
    -> anyhow::Result<(SyncSender<UiEvent>, Receiver<UiCommand>)>;
```

This is *not* a compiler-enforced boundary: `mod ui;` is declared at the
crate root (`main.rs`) and `UiRuntime` is plain `pub`, so privacy rules make
it visible to the crate root's defining scope and every module beneath
it — which is the whole crate. `main.rs` naming `crate::ui::UiRuntime`
directly compiles today exactly as it would before this split; nothing in
the type system stops it. A stray Slint call from the dispatcher is
therefore silent UB at runtime (per D4's opening paragraph), not a compile
error.

What *does* catch it mechanically is `meshcadet-slint-affinity-static-guard`'s
`xtask` harness (`xtask/src/slint_thread_affinity.rs`, run by
`cargo test -p xtask`): it scans every `.rs` file under `firmware/src/`
outside `ui/` and `ui_task.rs` for `UiRuntime`, `slint::`, or any `i_slint*`
symbol in non-comment source and fails the test if one appears. A CI grep
guard is exactly what closes this gap — the ADR's original text asserted one
was unnecessary because visibility already did the job; it does not.

**D4.3 — The full migration list.** Every inline `ui.*` call site in
`main.rs` today, and its disposition. This is the implementation child's
checklist; the validation child checks it is exhaustive.

| `main.rs` | Call | Disposition |
|---|---|---|
| 993, 1799 | `mark_app_ready()` | → `UiEvent::AppReady` |
| 1000, 1807 | `run_splash_ripple()` | → runs on `ui_task` (D8) |
| 1013 | `set_prov_rx_bytes(n)` | → `UiEvent::ProvRxBytes(u32)` (diagnostics only) |
| 1017, 2762 | `step(now)` | → the `ui_task` loop body; both call sites deleted |
| 1117 | `register_room(..)` | → `UiEvent::BootSeed` (C5) |
| 1154 | `register_contact(..)` | → `UiEvent::BootSeed` (C5) |
| 1194 | `set_channels(..)` | → `UiEvent::BootSeed` (C5) |
| 1199 | `set_pin(..)` | → `UiEvent::BootSeed` (C5) |
| 1211 | `set_nvs_partition(..)` | **deleted** (C6) |
| 1215 | `set_runtime_settings(..)` | → `UiEvent::BootSeed` (C5) |
| 1552 | `seed_conversation(..)` | → `UiEvent::BootSeed` (C5) |
| 1915 | `set_gps_status(..)` | → change-detected event (C4) |
| 1948 | `set_room_clock_source(..)` | → change-detected event (C4) |
| 1966 | `set_battery_status(..)` | → change-detected event (C4) |
| 1981 | `set_signal_level(..)` | → change-detected event (C4) |
| 2058, 2205, 2531, 2614, 3045, 3081 | `post_event(ev)` | → `evt_tx.try_send(ev)`; **no variant changes** |
| 2717 | `take_input_paint_stats()` | → `ui_task`'s own rollup (D9, item 10) |
| 2797 | `drain_commands()` | → `cmd_rx.try_recv()` drain loop |

### D5 — R2's lifetime/`Send` resolution: leak the bus, move the device

`SpiDeviceDriver<'_, &SpiDriver<'_>>` borrows a `run()` stack local
(`main.rs:683`), so neither device can be captured by a `'static` thread
closure as written.

**Primary resolution — `Box::leak` the bus to `'static`:**

```rust
let spi_driver: &'static SpiDriver<'static> =
    Box::leak(Box::new(SpiDriver::new(peripherals.spi2, /* … */)?));
```

Both `SpiDeviceDriver`s are then constructed from `spi_driver` (still on the
main task, per D2's corollary), and the LCD device — type
`SpiDeviceDriver<'static, &'static SpiDriver<'static>>` — is moved into
`ui_task`'s closure.

**This type is `Send`, and the chain is verifiable from vendored source:**

1. `unsafe impl<'d, T> Send for SpiDeviceDriver<'d, T> where T: Send + Borrow<SpiDriver<'d>> + 'd`
   — `esp-idf-hal-0.46.2/src/spi.rs:1282`.
2. So the question reduces to `&'static SpiDriver<'static>: Send`, i.e.
   `SpiDriver<'static>: Sync`.
3. `SpiDriver<'d>`'s four fields (`spi.rs:419-424`) are `host: u8`,
   `max_transfer_size: usize`, `bus_async_lock: Mutex<EspRawMutex, ()>`, and
   `PhantomData<&'d mut ()>`. `embassy_sync::mutex::Mutex<M, T>` is `Sync`
   where `M: RawMutex + Sync, T: Send`
   (`embassy-sync-0.7.2/src/mutex.rs:50`), and `EspRawMutex` is
   `Sync` (`esp-idf-hal-0.46.2/src/task.rs:823`). Every field is therefore
   `Sync`, so **`SpiDriver<'d>: Sync` auto-derives**.

This refines the constraint `docs/perf/spi2-arbitration-r1.md` §Q3 flagged
for this ADR. That analysis correctly observed there is no explicit
`unsafe impl Sync for SpiDriver` in the crate and concluded `Arc<SpiDriver>`
would not be `Send`; it stopped short of checking structural
auto-derivation. **The absence of an explicit `unsafe impl Sync` is not
evidence that the type is `!Sync`** — auto traits are derived structurally
unless a field negates them, and here nothing does. (The explicit
`unsafe impl Send` at `spi.rs:655` is defensive; it does not imply the
matching `Sync` was withheld deliberately.) R1's substantive point is
untouched either way: whatever container resolves `'static`, the arbitration
mechanism underneath is identical — which is why the fallback below is
equally acceptable if CI disagrees with this reading.

**Soundness of leaking** (three conditions, all met):

1. The leaked `SpiDriver` is never dropped, so `spi_bus_free`
   (`spi.rs:648-652`) is never called and there is no cross-thread
   destructor race. This changes nothing observable: `run()` never returns
   today, so the driver already lives for the process lifetime.
2. `SpiDriver` is immutable after construction as far as `Borrow` consumers
   are concerned — `SpiDeviceDriver` reads only `driver.borrow().host()`
   (`spi.rs:1035`) and `max_transfer_size`. The one interior-mutable field,
   `bus_async_lock`, is an `EspRawMutex`-backed mutex used solely by the
   `*_async` paths this firmware never calls, and is itself thread-safe.
3. Every mutating SPI operation goes through `&mut SpiDeviceDriver`, and
   each device is exclusively owned by one task (D2); concurrency between
   the two devices is serialised by ESP-IDF's per-bus `spi_bus_lock` (R1).

**CI is the arbiter, per campaign plan §4b.** R2 is a compiler question and
this container has no `esp` toolchain; `.github/workflows/ci.yml`'s
`firmware build gate (check-all-features.sh)` is the oracle. A green
cross-compile *is* the answer.

**Pre-authorised fallback if CI disagrees** — no new decision needed:

```rust
#[derive(Clone, Copy)]
struct SpiBus(&'static SpiDriver<'static>);
unsafe impl Send for SpiBus {}
unsafe impl Sync for SpiBus {}
impl core::borrow::Borrow<SpiDriver<'static>> for SpiBus {
    fn borrow(&self) -> &SpiDriver<'static> { self.0 }
}
```

`SpiDeviceDriver<'static, SpiBus>` is then `Send` via `spi.rs:1282`. The
`unsafe impl`s are justified by exactly the three conditions above, which
are the conditions that make the auto-derivation sound in the first place —
the fallback asserts what the compiler would otherwise conclude, and is
sound for the same reasons. If the implementation takes this branch it must
carry those three conditions as a comment on the `unsafe impl`s.

### D6 — Per-task stack budgets

**Dispatcher: 49 152 B, unchanged. Do not trim in this change.** The split
*removes* load from this task — Slint's 3–6 KB render call depth, the
`process_line` line buffers, and the screen-construction frames including
`navigate_to_pin_entry`, the **confirmed** release-only overflow site
documented at length in `firmware/sdkconfig.defaults`. Trimming is
therefore justified, and is deliberately deferred: splitting and re-sizing
in one change would make any post-change stack overflow ambiguous between
the two causes. The trim is a follow-on driven by the collection kit's
post-split HWM reading ("Deferred predicates", D-A).

**`ui_task`: 32 768 B.** Derivation from the one hard data point this repo
has, without inventing a number:

- The last measured HWM was **26 776 B peak** on a 32 768 B budget, and that
  budget then overflowed at `navigate_to_pin_entry` — i.e. ~6 KB short at
  the single densest UI transition (`sdkconfig.defaults`, "32 768 B" note).
- That 26 776 B was carrying **everything**: the UI render path *and* the
  dispatcher locals (`PolicyFilter` ~600 B, `AirtimeBudget` ~390 B,
  `TxQueue` ~260 B, `frame_buf` 256 B, `DuplicateFilter` ~72 B) *and* the
  radio SPI/crypto call depth *and* the identity/provisioning boot path.
- Post-split `ui_task` carries a strict **subset** of that: the Slint render
  pipeline, the screen-construction frames, the I2C touch/keyboard/trackball
  transactions, and the mpsc receive. None of the dispatcher locals, none of
  the crypto path.
- So 32 768 B for the subset leaves, at minimum, the whole of what the
  dispatcher half was contributing as fresh headroom on top of the ~6 KB
  that was previously short.

The UI's *own* share of that 26 776 B has never been separately measured;
that is precisely why the number above is chosen conservatively rather than
tuned, and why the post-split reading is a deferred predicate rather than a
claim. `uxTaskGetStackHighWaterMark(NULL)` reports the *calling* task
(`main.rs:4855`), so the two existing unconditional logs at
`navigate_to_pin_entry` and `navigate_to_admin_menu` automatically start
reporting `ui_task`'s HWM once those transitions run there — no new
instrumentation is needed for the densest case. The implementation adds one
periodic `ui_task` HWM log to match the dispatcher's 30 s sample.

**DRAM cost: +32 768 B.** pthread task stacks come from internal DRAM, not
PSRAM. ESP-IDF ≥ 5.3 offers `ThreadSpawnConfiguration::stack_alloc_caps` to
place a task stack in PSRAM; this repo is pinned to **ESP-IDF v5.2.2**,
where that field is `cfg`-gated out (`esp-idf-hal-0.46.2/src/task.rs:359-365`),
so that lever is **not available** and must not be planned around. Free
internal-heap headroom after the split is a deferred predicate (D-H).

### D7 — TWDT coverage for both tasks

Today exactly one task is subscribed: `esp_task_wdt_add(NULL)` on the main
task (`main.rs:1714`), petted at the top of every dispatcher iteration
(`:1824`), with `CONFIG_ESP_TASK_WDT_EN=y` / `_PANIC=y` / `_TIMEOUT_S=30`.
A wedged render on a separate task would be completely invisible.

1. `ui_task` calls `esp_task_wdt_add(NULL)` as its **first** action after
   `UiRuntime` construction — before `run_splash_ripple`, so no window of
   the task's life is unwatched.
2. `esp_task_wdt_reset()` at the top of every `ui_task` loop iteration, and
   **inside `run_splash_ripple`'s tight render loop** (`ui/mod.rs:1358-1371`),
   which otherwise owns the task for ~1.15 s. 1.15 s is far inside the 30 s
   timeout, so this is defensive rather than necessary — but the loop is the
   one place on `ui_task` that does not return to the top for an extended
   period, and it costs one line.
3. `recv_timeout(16 ms)` guarantees a pet at least every 16 ms in steady
   state, with or without traffic.
4. The subscription is never removed; `ui_task` never returns. If it ever
   dies, the missing pet trips the TWDT and panics into a controlled reboot
   — which is the desired outcome and is *new* protection the single-task
   design could not offer for the UI half in isolation.
5. Failure handling mirrors `main.rs:1717-1726` exactly: a non-zero return
   from `esp_task_wdt_add` logs a warning and continues rather than aborting
   boot (HIL builds may not have the TWDT initialised).
6. `CONFIG_ESP_TASK_WDT_TIMEOUT_S=30` is unchanged. Both tasks are well
   inside it.

### D8 — Boot sequencing under the split

Today's ordering constraints, re-derived (`run_splash_ripple`'s
`ui/mod.rs:1269-1332` doc, and the two boot paths at `main.rs:940-1026`
and `:1795-1811`):

- The display must come up **before** the provisioning gate so the §A
  wordmark + pubkey screen renders on unprovisioned first boot.
- `run_splash_ripple` must own a thread exclusively for
  `SPLASH_RIPPLE_TOTAL_MS` (~1.15 s) so the ripple gets an even frame
  cadence, and must be called exactly once, right after `mark_app_ready()`,
  before any dispatcher loop begins.
- Today that ~1.15 s window **defers radio RX polling**, an explicitly
  documented and accepted one-time boot gap.

**New sequence:**

| # | Task | Step |
|---|---|---|
| 1 | main | board power (`gpio10`), `Box::leak`'d `SpiDriver` (`main.rs:683`), NVS, identity |
| 2 | main | construct **both** `SpiDeviceDriver`s — LCD and radio (D2 corollary) |
| 3 | main | construct I2C1, backlight LEDC, I2S buzzer, DC/RST pin drivers |
| 4 | main | create both channels; **spawn `ui_task`** pinned to core 1, moving the LCD device + I2C1 + backlight + buzzer + `Receiver<UiEvent>` + `SyncSender<UiCommand>` into it |
| 5 | `ui_task` | TWDT subscribe → `TDeckPlatform::install()` → `UiRuntime::new()` → splash screen up |
| 6 | main | provisioning gate. **Unprovisioned:** spawn `prov_server`, send `AppReady`, then wait on `prov_done` with a plain sleep — the `ui.step()` pump loop at `main.rs:1005-1022` is **deleted**; `set_prov_rx_bytes` becomes `UiEvent::ProvRxBytes`. **Provisioned:** continue |
| 7 | main | `Radio::init`, GPS bring-up, history store, contact/room/channel load |
| 8 | main | send `UiEvent::BootSeed(..)` then `UiEvent::AppReady` |
| 9 | `ui_task` | on `AppReady`: `run_splash_ripple()` on its own task, then enter the `recv_timeout` loop |
| 10 | main | room logins, then enter the dispatcher loop |

**Three things this changes, all in the right direction:**

- **The ~1.15 s boot RX gap disappears.** The ripple now runs on core 1
  while the dispatcher loop runs on core 0. A documented, accepted
  priority-1 compromise is simply removed. This is a real secondary win, not
  a rationalisation.
- **The ripple's cadence gets *better*, not worse.** Its whole purpose is an
  exclusively-owned render loop; on a dedicated pinned task it has one
  unconditionally, instead of one carved out of a shared thread.
- **The two boot paths converge.** The unprovisioned path no longer needs
  its own UI pump loop; one topology serves both.

**Accepted overlap:** from step 5, `ui_task`'s display-init SPI transactions
can overlap the main task's `Radio::init` SPI transactions (step 7). This is
exactly the two-device concurrent-transaction case R1 cleared, and device
*registration* was already serialised in step 2. If the collection kit ever
shows a boot-time SPI anomaly, the fallback is to gate step 7 on a one-shot
"display ready" signal — but that reintroduces a boot serialisation the
split exists to remove, and there is no evidence it is needed.

**Splash dismissal is unchanged.** `splash_should_dismiss`'s inputs
(`SPLASH_MIN_MS`, `SPLASH_MAX_MS`, `app_ready`) are the same; `app_ready`
now arrives as an event instead of a direct call. The `SPLASH_MAX_MS`
defensive cap still fires if `AppReady` never arrives — and it now covers a
*new* failure mode for free (a dispatcher that wedges before step 8 no
longer wedges the UI with it).

### D9 — SMP correctness audit (every item touched from both cores)

| # | State | Verdict |
|---|---|---|
| 1 | The two mpsc channels | `std::sync::mpsc` is SMP-safe by construction. The only new cross-core state, and it is the standard library's. |
| 2 | `HISTORY`, `GPS_STATUS`, `BATTERY_STATUS`, `ROOM_CLOCK_SOURCE` (`main.rs:185/194/203/215`) | **The split adds zero new participants.** Dispatcher writes, `admin_server` reads — exactly as today. `ui_task` never touches them; it gets its values via C4 events. And `admin_server` is unpinned, so cross-core `std::sync::Mutex` use *already* happens today and is already correct. Hold times are unchanged short snapshot copies. |
| 3 | `uptime_ms()` / `uptime_us()` (`main.rs:4834/4851`) | `esp_timer_get_time()`, SMP-safe by IDF construction. Now called from both cores; a monotonic read has no cross-core hazard. |
| 4 | `log::info!` / `esp_log` | Already called from three threads today (`main`, `admin_server`, `prov_server`); IDF's vprintf lock covers it. |
| 5 | The global allocator | Every `UiEvent::IncomingDm` allocates its `String` on core 0 and frees it on core 1. ESP-IDF's `heap_caps_*` allocator is spinlock-protected and not per-core, so cross-core alloc/free is sound. Worth stating because this boundary does it on every inbound message. |
| 6 | NVS | Single writer (the dispatcher) by C6. No concurrency at all. |
| 7 | Slint globals | Single thread by D4; D4.2's boundary is a convention, checked mechanically by `xtask`'s `slint_thread_affinity` static guard, not by Rust visibility. |
| 8 | TWDT | `esp_task_wdt_add`/`_reset` are IDF-internal and designed for multi-task subscription. |
| 9 | SPI2 | Two devices, one per task, serialised by `spi_bus_lock` (R1). |
| 10 | `perf::PerfRollup` (diagnostics) | **`ui_task` gets its own rollup; the two are never shared.** `ui_step` and input-to-first-paint move to it, and it logs its own `PERF` lines. Consequence: the collection kit's expected log format changes — regenerating it is already a planned M1 deliverable (`meshcadet-perf-task-split-host-validation`), and must not be forgotten. |

`grep` confirms the audit is exhaustive on the app side: the only `static`
mutable state in `main.rs` is the four `Mutex`es at row 2 (no `static mut`,
no hand-rolled lock-free state, no `unsafe impl Sync` on any application
type — except D5's fallback, if taken).

### D10 — R1 (SPI2 arbitration): consumed, not re-derived

`docs/perf/spi2-arbitration-r1.md` settles this from source and datasheet
(`esp-idf-hal` v0.46.2, ESP-IDF v5.2.2 `spi_master`/`spi_bus_lock`,
`mipidsi` v0.8.0, `display-interface-spi` v0.5.0, all pinned in
`firmware/Cargo.lock`). Its findings, cited not restated:

- ESP-IDF's per-bus arbitration **does** serialise LCD and radio
  transactions across tasks and cores, for devices registered with
  `spi_bus_add_device` and each touched by exactly one task — the pattern
  D2 mandates.
- The 40 MHz ↔ 8 MHz clock switch is reconfigured strictly **inside** the
  held bus lock.
- Worst-case bus-hold latency is bounded at **≤ 12.8 µs** — one 64-byte SPI
  chunk, not a line and not a repaint, because `display-interface-spi`
  already chunks every LCD line into independent 64-byte writes that each
  re-arbitrate the bus.
- No risky usage pattern (raw bus-acquire, `NO_DUMMY`, DMA, half-duplex,
  `unsafe`) appears anywhere in this repository.

**What this means for priority 1, stated plainly:** a full-screen repaint on
`ui_task` can delay a radio SPI transaction by at most ~12.8 µs. Against a
CAD window of ≤ 20 ms and an airtime block of 83–800 ms, that is four to
five orders of magnitude below the timings that govern message delivery. It
cannot cause a missed CAD window, a late TX, or a dropped RX drain.

R1's one named residual is a **confirmatory** empirical check of that 12.8 µs
bound under real concurrent load, explicitly "not a gate on M1". It is
carried forward unchanged as a deferred predicate (D-B).

## Risk register — R1 through R8

| # | Risk | Answer | Where |
|---|---|---|---|
| R1 | SPI2 bus arbitration | **Closed by static analysis.** Serialised per bus across tasks/cores; ≤12.8 µs bus-hold; 4–5 orders below anything that governs delivery. | D10, D2 |
| R2 | Rust lifetime / `Send` | **Closed by source, confirmed by CI.** `Box::leak` the bus; `SpiDriver: Sync` auto-derives; `SpiDeviceDriver<'static, &'static SpiDriver>: Send`. Pre-argued newtype fallback if CI disagrees. | D5 |
| R3 | Stack budget | **Design decision + deferred confirmation.** Dispatcher 49 152 B unchanged (trim deferred, deliberately); `ui_task` 32 768 B derived from the 26 776 B HWM data point as a strict subset. +32 768 B DRAM; PSRAM stacks unavailable on IDF 5.2.2. | D6; deferred predicates D-A, D-H |
| R4 | Priority inversion / lock hold | **Structurally unreachable across this boundary.** No lock crosses it — only `try_send` channels that degrade rather than block, in both directions. Zero new participants in the four existing mutexes. | D3 C2, D9 row 2 |
| R5 | TWDT coverage | **Both tasks subscribed**, both petting well inside 30 s, including inside the splash ripple's tight loop. Net *increase* in coverage. | D7 |
| R6 | Boot sequencing / `run_splash_ripple` | **Re-derived.** Ripple moves to `ui_task`; the documented ~1.15 s boot RX gap is eliminated; the two boot paths converge; dismissal logic unchanged. | D8 |
| R7 | SMP correctness | **Audited item by item**, ten rows, exhaustive by `grep`. The only new cross-core object is `std::sync::mpsc`. | D9 |
| R8 | Slint `unsafe-single-threaded` | **Hard requirement, a convention now mechanically checked.** `ui_task` constructs and owns Slint end-to-end; `main.rs` naming `UiRuntime` compiles (privacy does not block it) but is caught by `xtask`'s `slint_thread_affinity` static guard (`cargo test -p xtask`); full 18-row migration list. | D4 |

## Functional-parity argument

The governing constraint is that **nothing may regress**. The argument has
four legs.

**Leg 1 — The UI's internal logic is not touched.** Every screen, every
navigation path, every Slint callback, every render decision, every
notification rule, and the whole of `UiRuntime::step()`'s body is moved
verbatim onto another task. Nothing inside `ui/` changes behaviour. The
edits to `ui/mod.rs` are subtractive (C6 removes the NVS field and its four
call sites) or mechanical (the events `Vec` becomes a `Receiver`, the
commands `Vec` becomes a `SyncSender`).

**Leg 2 — The message contract is preserved variant-for-variant.** No
existing `UiEvent` or `UiCommand` variant changes shape (C1). Ordering is
preserved (C3). The producer/consumer relationship is unchanged; only the
buffer between them changes from a `Vec` drained by the same thread to a
channel drained by another. Every one of the 18 rows in D4.3's migration
table maps to a message with the same payload as the call it replaces.

**Leg 3 — The one behavioural difference is a strict improvement, and it is
named.** The dispatcher no longer waits for `ui.step()` before its next
iteration, and the UI no longer waits for CAD/TX/RX before its next service.
That *is* the mission. Everything else — what is rendered, when a
notification fires, what a tap does, which frame goes on the wire — is
unchanged by construction. The single incidental change is the removal of
the documented ~1.15 s boot RX gap (D8), which strictly serves priority 1.

**Leg 4 — The parity matrix.** `meshcadet-perf-task-split-host-validation`
produces a filled matrix with one row per screen, navigation path, and radio
path, each citing the source location where behaviour is preserved. This ADR
freezes its **row set and evidence contract** so the matrix cannot be
quietly narrowed:

| Category | Rows |
|---|---|
| Screens | Splash, ContactList (contacts tab), ContactList (channels tab), MessageView, Compose, PinEntry, AdminMenu, GpsStatus |
| Navigation | splash→dismiss, list→MessageView (contact), list→MessageView (channel), MessageView→Compose, Compose Send (incl. deferred re-open), Compose cancel, gear→PinEntry, PinEntry→AdminMenu, PinEntry reject, AdminMenu→GpsStatus, GpsStatus→back, trackball highlight on list, trackball highlight on AdminMenu, printable-keypress→Compose |
| Input | GT911 touch, C3 keyboard, trackball roll, trackball click, screen-sleep inactivity timer |
| Radio | DM TX, DM RX, DM ACK match, GRP_TXT TX, GRP_TXT RX, implicit channel ACK, room login, room keep-alive, room post + ACK, room post refusal, room sync-drain, room permission update, CAD, duplicate filter, airtime budget, TxQueue eviction |
| Peripheral | GPS status push, battery status push, signal level push, room clock provenance push, buzzer notification, backlight |
| Persistence | history append, history hydrate/seed, runtime-settings persist (C6's new path), room session store, advert-timestamp store |
| Boot | provisioned path, unprovisioned path + `prov_server`, splash ripple, `admin_server` availability |

**Evidence contract per row:** the source location that carries the
behaviour, plus which of the four legs applies. A row whose only evidence is
"looks the same" is not a filled row.

## Regression-check strategy

No bench exists (campaign plan §0.5). Five legs, all executable in this
container or in CI.

1. **Loop model, extended to the as-built topology.**
   `perf_loop_model`'s `Topology::Split` (`sim.rs:364-372`) is currently a
   *prediction*. After implementation it is re-parameterised to what was
   built: `split_ui_idle_tick` ← `UI_TICK_MS` (16 ms, C7), plus a new
   `queue_handoff` cost parameter for the `try_send`/`try_recv` pair. Re-run
   the full corner × payload sweep and diff against
   `perf-loop-model-baseline.md` §§4-6. **Pass condition:** the longest
   UI-unserviced gap still drops by ≥1 order of magnitude and still does not
   scale with payload size, across the *whole* sensitivity range — not at a
   point estimate.
2. **Host UI harnesses unchanged.** `ui_perf` and `ui_sim::perf_profile`
   re-run for repaint-scope and allocation counts. Leg 1 of the parity
   argument predicts these are bit-identical; anything else means UI logic
   moved when it should not have.
3. **CI is the compile oracle.** `check-all-features.sh`, green, **with and
   without `--features diagnostics`**. This is what settles R2 (D5) — the
   container has no `esp` toolchain and cannot substitute for it. R8's
   discipline is checked separately, host-natively, by `xtask`'s
   `slint_thread_affinity` static guard (`cargo test -p xtask`,
   `meshcadet-slint-affinity-static-guard`) — no `esp` toolchain needed,
   since it is a text scan over `firmware/src/`, not a compile.
4. **The static functional-parity matrix**, row set frozen above.
5. **New host unit tests for the boundary itself**, placed in
   `firmware-core` so they run on the host:
   - the C4 change-detector: sends on change, suppresses on equality, for
     all four snapshot types;
   - the C2 overflow policy: event-queue-full drops and counts;
     command-queue-full surfaces rather than silently drops;
   - `BootSeed` completeness: a compile-time-checked mapping proving every
     C5 setter has a corresponding field (a test that fails when someone
     adds a boot setter and forgets the seed).

## Consequences

**Positive.**

- The dominant structural finding of the whole investigation is addressed
  directly: the UI-unserviced gap goes from 828 ms (modelled, high corner,
  255 B) to ~10 ms and stops scaling with payload size.
- Priority 1 gains in two ways beyond "does not regress": `ui.step()`'s SPI
  hold no longer delays the next CAD attempt or RX poll, and the documented
  ~1.15 s boot RX gap disappears (D8).
- Priority 3 is addressed for the first time: core 1 gets real application
  work, and the two workloads that were serialised for no reason other than
  having been written into one loop now genuinely run in parallel.
- The confirmed release-only stack-overflow site (`navigate_to_pin_entry`)
  moves off the main task, and TWDT coverage increases from one task to two.
- The unprovisioned and provisioned boot paths converge on one topology.

**Negative / accepted.**

- **+32 768 B of internal DRAM** for `ui_task`'s stack, with no PSRAM option
  on IDF 5.2.2 (D6). Headroom is a deferred predicate.
- **Two new bounded queues are two new places work can be dropped.** Both
  are sized well above production rates and both degrade loudly (C2), but
  the failure mode is new.
- **A second task is a second thing that can wedge.** Mitigated by D7's TWDT
  subscription, which makes a wedged UI a controlled reboot rather than a
  silent freeze — strictly better than today's coupled behaviour, where a
  wedged render also wedges the radio.
- **The diagnostics log format changes** (D9 row 10) — the collection kit
  must be regenerated for the post-split build.
- **R8's discipline is permanent.** `unsafe-single-threaded` means the
  compiler will not catch a Slint call from the wrong task, and D4.2's
  module boundary is a convention, not a visibility barrier — nothing in
  Rust's privacy rules stops `main.rs` from naming `crate::ui::UiRuntime`.
  Keeping the boundary honoured is now a `cargo test -p xtask` obligation
  (`slint_thread_affinity`'s static guard), not a purely human review one;
  see `meshcadet-slint-affinity-static-guard`.
- **The 12.8 µs bus-hold bound is a reading of ESP-IDF v5.2.2's reference
  implementation, not a vendor timing SLA** (R1's own caveat). It is
  correct for the pinned version; an IDF bump should re-check it.

## Deferred predicates (device-only)

Per the campaign plan's §0.5 no-hardware-in-the-loop rule and its
deferred-predicate convention, these require silicon, are **not** gates on
M1, and are handed to `docs/perf/collection-kit.md` for maintainer-run
capture on a real board. None of them may be answered with an invented
number.

| # | Predicate | Closes |
|---|---|---|
| D-A | Post-split per-task stack HWM, dispatcher and `ui_task` (`PERF` periodic sample + the two `navigate_to_*` logs) | R3's sizing; unblocks the deferred dispatcher-stack trim |
| D-B | The ≤12.8 µs bus-hold bound confirmed empirically under a concurrent full repaint + radio TX | R1's named residual (carried forward verbatim) |
| D-C | Delivery success rate — DM TX-with-ACK, DM RX, GRP_TXT, room push — versus the M0 baseline against a real peer node | Campaign acceptance criterion 1. A tie passes; any degradation fails regardless of UI gain |
| D-D | Real per-core utilization post-split (`PERF core-utilization`) | Priority 3's actual confirmation, and the input that would justify pinning the aux threads (D1) |
| D-E | Device-measured UI-unserviced gap (`PERF ui-starvation`) versus the loop model's prediction | Validates the model itself, not just the change |
| D-F | Device-measured input-to-first-paint | Priority 2's user-visible metric |
| D-G | Visual confirmation that the splash ripple is unchanged or smoother on the concurrent boot path | D8's claim |
| D-H | Free internal-heap headroom after +32 768 B of task stack (`heap_caps_get_free_size(MALLOC_CAP_INTERNAL)`) | D6's DRAM cost |

## Alternatives considered

**A1 — Keep one task; make `radio.transmit()` non-blocking and call
`ui.step()` from inside the DIO1 wait.** Rejected. It still serialises both
workloads onto one core (priority 3 unaddressed), and calling the render
path from inside the radio driver's wait loop is *worse* coupling than what
exists today — a render fault would then live inside the TX path. The
interrupt-driven DIO1 wait is genuinely worth doing and is M2's mission; it
composes with this split rather than substituting for it.

**A2 — Move the radio to the new task; keep the UI on main.** Rejected. The
UI is the only half with a pre-existing message interface (`UiEvent`/
`UiCommand`) — that is the de-risker this whole design rests on. The
dispatcher's state is entangled with ~1 000 lines of `run()` bring-up
(policy, txq, budget, dedup, room runtime, identity, NVS) with no boundary
to move along, and the main task's Kconfig stack was sized for exactly that
path. Moving the radio means moving the larger, less-bounded half across a
boundary that does not exist yet.

**A3 — `Arc<Mutex<UiRuntime>>` shared between the two tasks.** Rejected
twice over. R8: a mutex grants *access*, not thread *identity* — Slint would
still be called from two threads, which `unsafe-single-threaded` makes
undefined rather than merely checked. R4: the lock would have to be held
across a full-screen repaint, blocking the dispatcher for the entire flush —
the exact priority inversion the risk register exists to prevent.

**A4 — FreeRTOS native queues (`xQueueSend`) instead of `std::sync::mpsc`.**
Rejected. FreeRTOS queues copy fixed-size byte payloads; `UiEvent`/
`UiCommand` carry `String`s, which would force either a fixed-capacity
array (truncating messages — a functional regression) or smuggling a
`Box::into_raw` pointer through the queue (reintroducing manual lifetime
management the type system currently handles). mpsc moves the payload for
free, and both enums are already `Send` (C1).

**A5 — Three tasks: radio, dispatcher, UI.** Rejected for M1. The radio and
the dispatcher share `TxQueue`, `AirtimeBudget`, `DuplicateFilter`, and
`pending_ack` with no existing message boundary; splitting them is a much
larger change with no evidence it is needed. M2's DIO1 interrupt work
addresses the radio's own blocking without a third task. Revisit only if
post-split measurements demand it.

**A6 — Do nothing structural; pursue the residual UI-side optimizations
only.** Rejected by the numbers. `docs/perf/ui-perf-baseline.md` §5's
surviving UI-side items are 0.18–3.1 ms; the structural item is 83–800 ms.
That earlier pass declared TX airtime out of scope, which was correct *for a
pass scoped to UI changes* and is the wrong frame here — a task split is
precisely the change that fixes it. The residual items remain worth
re-ranking against post-split numbers, which is M3's mission.

## Sources

- `firmware/src/main.rs` — dispatcher loop (`:1811`), `SpiDriver` (`:683`),
  LCD device (`:745`), radio device (`:1405`), TWDT (`:1714`, `:1824`),
  static mutexes (`:185/194/203/215`), boot paths (`:940-1026`,
  `:1795-1811`), `ui.*` call sites (D4.3's table), `uptime_ms`/`uptime_us`
  (`:4834/4851`), stack-HWM logger (`:4855`)
- `firmware/src/ui/mod.rs` — `UiEvent` (`:132`), `UiCommand` (`:303`),
  `UiRuntime` (`:512`), `post_event` (`:1495`), `drain_commands` (`:1500`),
  `step` (`:1508`), `run_splash_ripple` (`:1333`), NVS field (`:594`) and
  persistence sites (`:2695/2705/2718/2729`)
- `firmware/src/ui/platform.rs` — `set_platform` (`:98`),
  `register_bitmap_font` (`:110`)
- `firmware/Cargo.toml:89-105` — `unsafe-single-threaded` on `slint` and
  `i-slint-core`
- `firmware/sdkconfig.defaults` — main-task stack rationale and the
  confirmed overflow backtrace; TWDT config; FreeRTOS run-time stats
- `esp-idf-hal` v0.46.2 — `spi.rs:419-424` (`SpiDriver` fields), `:655`
  (`unsafe impl Send`), `:954-956`/`:1035` (`SpiDeviceDriver`,
  `Borrow` use), `:1282` (`unsafe impl Send for SpiDeviceDriver`);
  `task.rs:353-366` (`ThreadSpawnConfiguration`), `:823`
  (`unsafe impl Sync for EspRawMutex`)
- `embassy-sync` v0.7.2 — `mutex.rs:49-50` (`Mutex` `Send`/`Sync` bounds)
- `docs/perf/spi2-arbitration-r1.md` — R1's verdict and its named residual
- `docs/perf/perf-loop-model-baseline.md` — the simulated M0 baseline
- `docs/perf/ui-perf-baseline.md` — analytical airtime and SPI-floor tables
- `perf_loop_model/src/sim.rs:339-418`, `params.rs` — `Topology`,
  `split_ui_idle_tick`
- the `meshcadet-perf-rearchitecture` campaign plan, §§0, 0.5, 4, 4b, 5
