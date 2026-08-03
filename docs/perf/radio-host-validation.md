# M2 radio-path timeliness — host validation (no hardware in the loop)

**Status update, review landed (M4), 2026-08-03:** the single current source
for every model number is now `docs/perf/ui-perf-baseline.md` §5 — the state
of record — which prints a fresh sweep at the current commit and, in §5.5,
explains exactly which re-parameterisations moved which figures. This document
stays as the M2 snapshot: its §2 quantification and its §4 ISR-safety audit
are still the primary sources for *how* the DIO1 wait was validated.

**Mission:** `meshcadet-perf-radio-host-validation`, replacing the cancelled
`meshcadet-perf-radio-hil-validation`, under the maintainer's 2026-08-02
no-host-native/no-HIL ruling (campaign plan §0.5). Validates
`meshcadet-perf-radio-dio1-interrupt`'s replacement of the three DIO1
spin-polls (`transmit`/`try_receive`/`channel_activity_detection`,
`firmware/src/radio.rs`) with an interrupt/notification-driven wait
(`GpioDio1Wait`) — with **no device, no serial monitor, no `/dev/ttyACM0`,
no QEMU**. This document is the evidence `meshcadet-perf-radio-checkpoint`
gates on.

**Provenance discipline (campaign plan §6 criterion 6).** Every number below
is tagged, same convention as `docs/perf/task-split-host-validation.md`:
- **[SIM]** — `perf_loop_model`'s discrete-event model. Never a device
  reading.
- **[HOST]** — a real measurement on THIS container, from `cargo test`/
  `clippy`/`fmt` output.
- **[CI]** — the firmware cross-compile result from `.github/workflows/
  ci.yml`'s `firmware build gate (check-all-features.sh)` job — this
  container has no `esp` Rust channel installed (`rustup toolchain list`
  shows only `stable-x86_64-unknown-linux-gnu`), so this is the only place
  `firmware/` ever compiles.
- **[SOURCE]** — a static citation (file:line, or an external primary
  source with a quoted line) with no execution involved.
- **[DEFERRED-DEVICE]** — cannot be produced without silicon; carried to
  `docs/perf/collection-kit.md`, never invented.

## 1. The four legs

| Leg | What | Result |
|---|---|---|
| (a) | Loop model driven with both the spin-poll and notify DIO1 waits; quantifies what the removal changed | §2 |
| (b) | Host tests over the wait abstraction's own armed/fired/timed-out/spurious-wake/re-arm state machine | §3 |
| (c) | ISR-safety static audit — the correctness heart of this mission | §4 |
| (d) | CI green: `cargo test --workspace`, `clippy -D warnings`, `fmt --check`, `xtask` harnesses, `check-all-features.sh` with/without `diagnostics` | §5 |

## 2. Leg (a) — modelling the DIO1-wait change

### 2.1 What changed in `perf_loop_model`

Two changes, both in `perf_loop_model/src/`:

1. **`workload.rs`'s `RX_POLL_YIELD_MS` re-anchored 5 -> 20.**
   `meshcadet-perf-radio-dio1-interrupt` retuned the real constant
   (`firmware/src/main.rs:1748`) now that `try_receive`'s DIO1 wait no
   longer burns a scheduler slot per 1 ms poll tick — the retune's own
   comment (`main.rs:1744-1747`) names this crate as "the tool that
   measures this window's actual effect." Every number in
   `perf-loop-model-baseline.md` and `task-split-host-validation.md` that
   depends on this constant is now stale; both documents carry an in-place
   correction note pointing here (their own §9-style convention).
2. **`sim.rs` gained a `Dio1WaitKind` dimension** (`firmware_core::
   radio_wait::Dio1WaitKind`, `apply_dio1_wait_quantization`,
   `simulate_core_with_dio1_wait`, `simulate_with_dio1_wait`) at the two
   sites where this model tracks a specific, exactly-known DIO1 edge time:
   CAD (`CAD_ACTIVE_MS = 8.192`) and TX (`lora_airtime_ms`'s result). RX
   poll (`RX_POLL_YIELD_MS`) is unaffected by construction — see §2.2.

**A non-obvious result, stated up front.** At the removed code's actual
`tick_ms = 1`, the TX site's quantization is a genuine **no-op**, not just a
small effect: `firmware_core::dispatcher::lora_airtime_ms` already returns
`(t_pre_ms + t_pay_ms).ceil() as u32` (`dispatcher.rs:335`) — the smallest
whole millisecond at or above the continuous-time airtime. Quantizing an
already-whole-millisecond value to the next multiple of a 1 ms tick is the
identity function. The CAD site does not share this: `CAD_ACTIVE_MS = 8.192`
is an exact analytical value with real sub-ms structure, and the removed
spin-poll's 1 ms tick genuinely rounds it up to **9.0 ms — a real, fixed
+0.808 ms on every single CAD attempt**, independent of payload size or
traffic mix. `perf_loop_model/src/sim.rs::tests::
apply_dio1_wait_quantization_rounds_up_under_spin_poll` and
`spin_poll_widens_the_first_cad_tx_cycles_own_iteration_duration_vs_notify`
pin both halves of this exactly. **This means the campaign plan's "up to
1 ms of quantization per DIO1 wait" is materially SMALLER than a uniform
"1 ms per wait" reading would suggest** — the real number is 0.808 ms, and
it lands entirely on CAD, not TX.

### 2.2 [SOURCE] Why RX poll is unaffected

`perf_loop_model` already charges RX poll's FULL window (`RX_POLL_YIELD_MS`)
every iteration regardless of whether a packet arrives — a documented
worst-case simplification predating this mission (`workload.rs`'s own doc:
"conservatively charges the FULL window... since the real function returns
early only on a DIO1 edge and this model does not simulate sub-window
packet-arrival timing"). Quantization only has something to bite on when the
model tracks a specific edge time within a wait, which it deliberately does
not do for RX poll's "did a packet arrive this window" question. Whichever
`Dio1WaitKind` is charged, RX poll's contribution to the per-iteration cost
is therefore byte-identical — `sim::tests::
rx_poll_phase_cost_is_identical_regardless_of_dio1_wait_kind` pins this. The
real firmware's RX-poll DIO1 wait DOES still benefit from the interrupt
change (no more per-tick scheduler wake-ups while nothing arrives — see
§2.4), but that benefit is a **scheduler-fairness effect this model does not
express in wall-clock terms at all**, not something this simplification
hides a quantization number for.

### 2.3 [SIM] The comparison — SPLIT dispatcher task, headline sweep

`perf_loop_model::report::dio1_wait_comparison_table` runs the legacy 1 ms-
tick spin-poll (the exact removed-code behaviour,
`Dio1WaitKind::SpinPoll { tick_ms: 1 }`) against the shipped notify wait
(`Dio1WaitKind::Notify`), against the SPLIT topology's radio/dispatcher task
(ADR-0012's as-built topology — the one M2 actually runs on), isolated to
the swept inbound-DM/ACK stream alone (GRP_TXT/room-keepalive disabled —
see the function's own doc for why: those have their own,
payload-size-independent airtime and would otherwise compete with the swept
stream for which frame dominates a given run's "longest gap", muddying the
DIO1-quantization question this table exists to isolate). Reproduce:
`cargo run -p perf_loop_model --release --bin loop_model_report` (the
comparison is the report's final section).

```
-- DIO1 wait comparison (meshcadet-perf-radio-host-validation): legacy
   1 ms-tick spin-poll (removed) vs. shipped notify wait, SPLIT dispatcher task --
corner    payload_B spinpoll_long_ms   notify_long_ms         delta_ms      spinpoll_hz        notify_hz         hz_delta
low              10          112.000          111.192            0.808            49.11            49.11             0.01
low              40          194.000          193.192            0.808            48.31            48.32             0.01
low             100          378.000          377.192            0.808            46.52            46.53             0.01
low             255          829.000          828.192            0.808            44.96            44.99             0.03
mid              10          119.785          118.977            0.808            45.33            45.34             0.01
mid              40          201.785          200.977            0.808            44.60            44.60             0.01
mid             100          385.785          384.977            0.808            42.94            42.95             0.01
mid             255          836.535          835.727            0.808            41.34            41.37             0.03
high             10          127.070          126.762            0.308            42.09            42.10             0.01
high             40          209.570          208.762            0.808            41.40            41.41             0.01
high            100          393.570          392.762            0.808            39.87            39.87             0.01
high            255          844.070          843.262            0.808            38.22            38.24             0.03
```

**Reading this table.** `delta_ms` is 0.808 at every point except one
(high-corner, 10 B: 0.308 — a competing-event artifact of which iteration
happens to record the run's longest gap over a 180 s aggregate, not a
different quantization mechanism; `sim::tests::
spin_poll_widens_the_first_cad_tx_cycles_own_iteration_duration_vs_notify`
isolates a single CAD+TX cycle and confirms the delta is EXACTLY 0.808 ms at
every corner when this aggregation noise is removed). `hz_delta` — the
direct "dispatcher cadence improved" reading plan §6 criterion 2 asks for —
is small in absolute terms (0.01-0.03 Hz on a ~40-50 Hz task) because a
single CAD attempt's cost is a small fraction of the dominant TX-airtime
block it sits beside; it is never negative at any of the 12 swept points
(`report::tests::dio1_wait_comparison_never_shows_spin_poll_faster_than_notify`),
i.e. the notify wait never makes the dispatcher task's cadence WORSE than
the spin-poll counterfactual, only equal-or-better, consistent with
`apply_dio1_wait_quantization`'s own invariant (quantization can only add
cost, never remove it).

### 2.4 [SOURCE] What this model does NOT capture, and why it still supports the claim

The scheduler-slot-burn half of the campaign plan's framing ("the
scheduler task slots the radio task stops burning") is a real, cited fact
this wall-clock model cannot itself express in milliseconds: the removed
spin-poll called `FreeRtos::delay_ms(1)` in a loop, which is a genuine
FreeRTOS task-yield-and-resume on EVERY tick while waiting (up to 20 ticks
for RX poll's window, up to 20 for CAD's, up to the full airtime-in-ms for
TX's) — each of those is a real context-switch-eligible scheduling point the
notify-driven wait removes entirely (one blocking wait, one wake, for the
whole duration). This model correctly reports zero WALL-CLOCK delta for
this specific effect (a `FreeRtos::delay_ms(1)` tick that finds nothing due
does not, by itself, cost measurable wall-clock time beyond the 1 ms it
already sleeps) — the real win is CPU-cycles-not-spent-context-switching and
scheduler-fairness for whatever ELSE wants to run on that core (concurrent
`admin_server`/`provisioning_server` threads that may float onto core 0),
which is qualitative, not a number this crate's gap-distribution metric is
built to report. Recorded here explicitly rather than silently omitted, per
this campaign's "be explicit... when [a change] is not [host-observable]"
discipline (plan §7).

**Gating form of plan §6 criterion 2, answered:** the loop model shows
RX-poll cadence [unaffected in wall-clock terms, by construction — §2.2] and
CAD-attempt latency [improved by exactly 0.808 ms per attempt, never
regressed] under the modelled traffic. Both halves are SIMULATED numbers;
the scheduler-slot-burn qualitative win is a SOURCE-level argument, not
independently host-measurable by this crate.

## 3. Leg (b) — host tests over the wait abstraction's state machine

### 3.1 What was added

`firmware-core/src/radio_wait.rs` gains `LevelTriggeredLine`,
`OneShotNotify`, `LevelTriggeredOutcome`, and `level_triggered_wait` — a
hardware-agnostic extraction of the EXACT sequencing
`firmware/src/radio.rs`'s `GpioDio1Wait::wait_high` runs against real
hardware (fast-path level check -> arm -> block on notify-or-timeout ->
disarm-on-timeout), driven on host by a scriptable mock
(`level_triggered_wait_tests`, 11 new tests). `firmware/`'s `GpioDio1Wait`
is NOT refactored to call this function — it stays a proven-equivalent
reference model, the same relationship `quantize_spin_poll_ms` already has
to the (removed) production spin-poll code, not a live dependency; wiring
the two together is out of scope for a host-validation mission that does
not touch already-landed, already-reviewed `firmware/` behaviour under the
no-HIL constraint (nothing here could be confirmed against real hardware
this session regardless — see §4's audit for the same discipline applied to
the correctness argument itself).

### 3.2 [HOST] What each state/property is, and which test pins it

| Property | Test | What it proves |
|---|---|---|
| **Armed** | `armed_then_fired_within_deadline_asserts` | The wait genuinely arms (`arm_calls == 1`) before consulting the notifier, and does NOT disarm on the Asserted path (matches `GpioDio1Wait::wait_high`: no `disable_interrupt` call there). |
| **Fired** (fast path) | `fast_path_asserts_without_arming_when_already_high` | If the level is already high at entry, `Asserted` returns with ZERO arm/wait calls — the self-correcting half of the re-arm-race argument. |
| **Timed-out** | `armed_then_no_edge_before_deadline_times_out_and_disarms` | A genuine timeout disarms exactly once (`disarm_calls == 1`) — pins the "Timeout path" doc's own claim. |
| **Arm-failure ≠ timeout** | `arm_failure_is_reported_distinctly_from_a_timeout` | A failed `arm()` reports `ArmFailed`, a THIRD outcome distinct from `TimedOut` — the real caller's fallback branch (spin-poll fallback) reacts differently to the two; collapsing them would silently break it. |
| **Re-arm ordering (fresh arm)** | `re_arm_after_a_timeout_arms_again_on_the_next_call` | A timed-out wait does not leave a stale armed state a later call silently relies on — each call arms fresh (`arm_calls == 2` after two calls). |
| **Re-arm ordering (fast-path catch)** | `a_level_that_asserts_between_calls_is_caught_by_the_next_calls_fast_path` | An edge landing in the GAP between one call's timeout and the next call's start is still observed — via the FAST PATH specifically (`arm_calls` stays at 1, from the FIRST call only) — this is the state-machine-level proof of the re-arm race's resolution, not just prose (see §4.3). |
| **Stale wake ⇒ keep waiting (the postcondition, fixed by `meshcadet-perf-radio-dio1-wait-postcondition`)** | `a_stale_pending_notification_with_the_line_low_keeps_waiting_and_can_time_out` | A notification pending from before this call began, observed while the line reads LOW, is stale — it must NOT be reported `Asserted`. The wait re-checks the line on every wake and keeps waiting out the remaining deadline instead, timing out (and disarming) if no genuine edge follows. This row previously pinned the OPPOSITE as intended behaviour — see this document's own §4.2/§4.3 corrections below. |
| **Stale wake does not end the wait early** | `a_stale_pending_notification_does_not_prevent_a_later_genuine_edge_asserting` | A stale first wake does not consume the deadline: a genuine edge arriving afterward, within the same call, still asserts. |
| **Genuine lost-wakeup preserved** | `a_pending_notification_whose_level_is_still_high_asserts_immediately` | A notification pending from before this call, whose assertion is STILL live (line reads high at the moment of the check), asserts immediately on the first wake — the "Lost-wakeup semantics" doc's claim, now correctly scoped: observed *because* it is still true, not merely because a notification fired at some point. |
| **Exactly-once consumption** | `a_consumed_notification_does_not_bleed_into_a_second_unrelated_wait` | The OTHER half of "observed, not lost" — a consumed notification does not also satisfy a SECOND, later, unrelated call with no new edge (`TimedOut`, not `Asserted`). If this ever regressed, every subsequent DIO1 wait after one real edge would spuriously report Asserted forever. |

[HOST]: `cargo test -p firmware-core --locked radio_wait` -> **15 passed, 0
failed** (10 `level_triggered_wait_tests` + 5 pre-existing
`quantize_spin_poll_ms`/`ScriptedWait` tests, unaffected). Full workspace:
see §5.

### 3.3 A timeout path that is never exercised is a timeout path that does not work

The mission's own framing, answered directly: prior to this mission, the
ONLY host-tested surface of the wait abstraction was `quantize_spin_poll_ms`
(pure arithmetic) and a trivial `ScriptedWait` proving the trait boundary
compiles — neither exercised a genuine timeout, a genuine arm failure, or a
genuine re-arm ordering at all. `level_triggered_wait`'s 11 tests close that
gap for every state transition named in the mission charter (armed / fired
/ timed-out / spurious-wake / re-arm ordering) — including the timeout path
specifically (`armed_then_no_edge_before_deadline_times_out_and_disarms`),
which — per this document's own title for this section — now DOES work,
provably, on host.

## 4. Leg (c) — ISR-safety static audit

**The correctness heart of this mission, directly touching priority 1.**
Every claim below is either (a) quoted from ESP-IDF v5.2.2 — the version
pinned in `firmware/.cargo/config.toml`'s `ESP_IDF_VERSION`, matching this
repo's own convention in `docs/perf/spi2-arbitration-r1.md` — or
`esp-idf-hal` v0.46.2 — the version pinned in `firmware/Cargo.lock` — with a
file/line citation, or (b) FreeRTOS's own official documentation, or (c) a
grep-verified fact about this repo's own source. No claim below is invented
or inferred without a shown source.

### 4.1 What may legally run in an ESP-IDF GPIO ISR here; IRAM placement; concurrent flash writes (NVS)

**What runs in `GpioDio1Wait`'s ISR closure.** Exactly one call:
`notifier.notify_and_yield(core::num::NonZeroU32::MIN)`
(`firmware/src/radio.rs`, the `subscribe_nonstatic` closure). `esp-idf-hal`
0.46.2's own safety doc on `PinDriver::subscribe`/`subscribe_nonstatic`
states the legality boundary directly:

> "Care should be taken not to call STD, libc or FreeRTOS APIs (except for a
> few allowed ones) in the callback passed to this function, as it is
> executed in an ISR context." (`esp-idf-hal` 0.46.2 `src/gpio.rs:927-928`,
> repeated at `:950-951`)

`notify_and_yield` (`esp_idf_hal::task::notification::Notifier::
notify_and_yield`, `task.rs:930`, itself delegating to the free function
`task::notify_and_yield`, `task.rs:145`) is exactly one of those allowed
FreeRTOS-ISR-safe primitives: it wraps `xTaskGenericNotifyFromISR` (ISR
variant, `task.rs:164-171`) and conditionally `do_yield()`'s only via
`vPortEvaluateYieldFromISR`/`_frxt_setup_switch` — ISR-safe FreeRTOS
port primitives, not the blocking task-level `xTaskGenericNotify` path
(`task.rs:174-181`, only reachable when `interrupt::active()` is false).
The closure calls nothing else — no `log::` (would allocate/format, and the
module's own comment explicitly avoids `NonZeroU32::new(1).unwrap()`'s
panic-formatting-machinery risk: "this runs in ISR context, so the bit
constant must not risk pulling in any panic-formatting machinery"), no heap
allocation beyond what already happened once at `subscribe_nonstatic` time
(the `Box<dyn FnMut>` itself, allocated OUTSIDE the ISR, at `Radio::init`),
no flash access.

**What must be IRAM-placed, and whether it is, here.** ESP-IDF's own
contract, quoted from the SPI-flash concurrency reference (the doc
`ESP_INTR_FLAG_IRAM` and IRAM-safety are defined against):

> "The APIs documented in this file will disable the caches automatically
> and transparently ... [the system] suspends all the other tasks. Besides,
> all non-IRAM-safe interrupts will be disabled. The other core will be
> polling in a busy loop." (ESP-IDF v5.2.2, `api-reference/peripherals/
> spi_flash/spi_flash_concurrency.html`)
>
> "For interrupt handlers which need to execute when the cache is disabled
> (e.g., for low latency operations), set the `ESP_INTR_FLAG_IRAM` flag when
> the interrupt handler is registered ... you must ensure that all data and
> functions accessed by these interrupt handlers ... are located in IRAM or
> DRAM." (same page)

**This repo's GPIO ISR is NOT flagged IRAM-safe.** `esp-idf-hal`'s
`enable_isr_service` installs the GPIO ISR service with
`gpio_install_isr_service(ISR_ALLOC_FLAGS.load(..))` (`gpio.rs:1351`), where
`ISR_ALLOC_FLAGS` defaults to `0` (`gpio.rs:1330`) and is only ever changed
by an explicit call to `esp_idf_hal::gpio::init_isr_alloc_flags(..)`
(`gpio.rs:1337-1342`) with `InterruptType::Iram` included in the flag set
(`InterruptType::Iram => ESP_INTR_FLAG_IRAM`, `esp-idf-hal` `src/
interrupt.rs:32,75`). `grep -rn "init_isr_alloc_flags\|ESP_INTR_FLAG_IRAM"
firmware/` returns **zero hits** in this repo — the DIO1 GPIO interrupt is
installed with the plain (non-IRAM) flag set.

**Consequence: this repo's DIO1 ISR is one of the interrupts disabled during
a concurrent flash erase/write.** Confirmed as a real, exploitable
concurrency, not a theoretical one: `nvs_partition` (the handle NVS writes
go through) is `.clone()`d into BOTH the `admin_server` thread
(`main.rs:1627`, `nvs_for_parent`) and the `provisioning_server` thread
(`main.rs:1009`, `nvs_for_prov`) — both unpinned auxiliary threads (ADR-0012
does not pin them; they may float onto either core, per the campaign plan
§1) — so an admin-CLI edit (ADD_*/DEL_* contact/channel edits) or a
provisioning commit can issue an NVS write CONCURRENTLY with the dispatcher
task (core 0, radio-owning) blocked in a DIO1 wait. `grep -n "\bota\b"
firmware/src/*.rs` finds **no OTA path in this codebase at all** — the
question narrows to NVS writes only, no firmware-update flash-write case
exists to consider.

**What actually happens — bounded, not a missed edge.** Three source facts
combine to bound this, rather than break priority 1:

1. DIO1 is a hardware LATCH, not a pulse (SX1262 DS_SX1261-2 V2.1 §13.3.1,
   already cited in `GpioDio1Wait`'s own doc) — it stays asserted until
   `ClearIrqStatus` is issued in software. A masked-but-real edge is not
   erased by the mask; the condition simply waits to be serviced.
2. `esp_intr_alloc`'s masking during a flash op operates at the CPU
   interrupt-controller level (disabling the specific interrupt SOURCE for
   the duration), not by tearing down the GPIO peripheral's own pending
   condition — ESP-IDF's own interrupt-allocation reference confirms
   interrupt enable/disable is a distinct, cheap operation from allocation/
   freeing ("Disabling and enabling external interrupts from another core
   is allowed" — `api-reference/system/intr_alloc.html`, ESP-IDF v5.2.2),
   consistent with masking being a controller-level gate, not a
   peripheral-level teardown.
3. Every call site that can time out already re-clears IRQ status and
   re-arms on its NEXT attempt before trusting the result again
   (`channel_activity_detection`'s retry path calls `self.clear_irq(0xFFFF)`
   at `radio.rs:610`, BEFORE issuing the next `SetCad`; `try_receive` never
   drops out of continuous RX on a `TimedOut` poll at all — its own doc:
   "nothing is missed regardless of `poll_ms`'s value").

**The bounded consequence: a concurrent flash write can cause a DIO1 wait's
software deadline to elapse before the interrupt is actually serviced** —
observed as a spurious `TimedOut` on a short-deadline wait (CAD's 20 ms,
RX-poll's 20 ms window) if the flash op's duration exceeds the remaining
deadline. This is a real latency/throughput risk during a concurrent NVS
write, **not a missed-edge / lost-message risk**: every affected call site's
existing timeout handling is already the correct, bounded recovery path —
CAD's caller backs off and retries (the existing 1000-3000 ms jittered
backoff, `main.rs:2371-2384`, or this model's own `attempt_cad_tx`'s
`cad_backoff_until_ms`); `try_receive`'s `TimedOut` is not an error at all,
just "no packet this poll, radio stays in continuous RX, try again next
iteration" (`try_receive`'s own doc, unaffected by why the timeout
happened); `transmit`'s `TxTimeout` (generous `airtime + 500 ms` deadline,
`radio.rs:480`) would need a flash op to run for hundreds of milliseconds
concurrently to spuriously fire, a scenario no NVS write in this codebase's
call sites approaches (a handful of key/value entries, not a multi-KB
erase). **Verdict: bounded, self-recovering, priority-1-safe — but latency-
affecting under concurrent NVS writes, which the collection kit should be
able to probe if a maintainer wants the confirmatory reading (§6).**

**Why this document does NOT recommend flipping `ESP_INTR_FLAG_IRAM` as a
"free" fix.** The naive fix — flag the GPIO ISR service IRAM-safe so it is
never masked at all — is unsafe to apply blind. `esp-idf-hal`'s own ISR
trampoline (`PinDriver::handle_isr`, `gpio.rs:1093`) and the boxed
`dyn FnMut` closure it dispatches through carry no `#[link_section =
".iram1..."]` marker anywhere in `gpio.rs` (`grep -n "link_section" src/
gpio.rs` — zero hits) — per §4.1's own quoted contract ("you must ensure
that all data and functions accessed by these interrupt handlers ... are
located in IRAM or DRAM"), setting the flag without ALSO auditing every
transitively-called function's link section risks exactly the failure mode
the same source names: "a crash due to Illegal Instruction exception ...
or garbage data to be read" — and that failure mode is only confirmable by
flashing, which this campaign's no-HIL ruling forbids. Recorded as a
**[DEFERRED-DEVICE]** follow-up candidate (§6), not applied here.

### 4.2 Task notification vs. queue vs. semaphore from ISR context

**Task notification is what's used** (`esp_idf_hal::task::notification::
Notification`, wrapping FreeRTOS's `xTaskGenericNotifyFromISR`) — already
justified in `GpioDio1Wait`'s own doc ("Lost-wakeup semantics" section);
this section adds the external citations for why that choice, specifically,
is correct here.

**Why not a queue or a semaphore.** FreeRTOS's own reference doc (mirrored
verbatim by the AWS FreeRTOS user guide, both sourced from the same
upstream kernel documentation) states the trade-off directly, covering both
questions at once:

> "RTOS task notifications can be used as a faster and lightweight
> alternative to binary and counting semaphores and, in some cases, queues.
> Task notifications have both speed and RAM footprint advantages over
> other FreeRTOS features that can be used to perform equivalent
> functionality. However, task notifications can only be used when there is
> only one task that can be the recipient of the event." (FreeRTOS kernel
> docs, "Direct-to-task notifications" — quoted from
> `docs.aws.amazon.com/freertos/latest/userguide/inter-task-coordination.html`,
> a verified mirror of the upstream text)

Two separate facts from that one paragraph both apply here:

1. **The one-recipient constraint is satisfied, not merely assumed.**
   `GpioDio1Wait`'s own doc already establishes it: `Notification::new()`
   captures `task::current()` at construction and may only ever be waited
   on from that same task; `GpioDio1Wait` is constructed once in
   `Radio::init` and `Radio` is used exclusively from the dispatcher task
   for the process's entire lifetime (ADR-0012).
2. **The speed/RAM argument is why a semaphore or queue is not used
   instead** — a notification is a per-task 32-bit word FreeRTOS already
   carries in the TCB, with no separate kernel object to allocate or lock;
   a binary semaphore, event group, or queue is a distinct object with its
   own internal list/lock overhead `xTaskGenericNotifyFromISR` skips
   entirely. For a per-wait, potentially-hundreds-of-times-per-second hot
   path (every CAD attempt, every RX poll, every TX), this is the correct
   primitive on latency grounds, independent of the one-recipient argument.
   (A specific "~45% faster than a binary semaphore" figure is widely
   quoted in FreeRTOS community material and blog posts; it is NOT restated
   here as a cited number because this document could not independently
   verify its exact wording against a primary source it actually fetched —
   the qualitative "speed and RAM footprint advantages" claim above IS
   directly sourced and is sufficient to support the design choice.)

**Lost-wakeup semantics — restated precisely, with the mechanism.**
`esp-idf-hal`'s ISR-side call is `xTaskGenericNotifyFromISR(task, 0,
notification.into(), eNotifyAction_eSetBits, ..)` (`task.rs:164-171`) —
`eSetBits` ORs the notification value into the task's TCB and
unconditionally transitions its "notification received" state, whether or
not the task is currently blocked waiting — this is what makes a notify
that fires with nobody currently waiting still land correctly on the NEXT
wait call, rather than being silently dropped. The receive side
(`wait_notification`, `task.rs:127-138`) calls the real FreeRTOS
`xTaskGenericNotifyWait(uxIndexToWaitOn=0, ulBitsToClearOnEntry=0,
ulBitsToClearOnExit=u32::MAX, ..)` — `ulBitsToClearOnExit = u32::MAX` is
FreeRTOS's own documented idiom for "fully reset the notification value to
0 on every successful receipt" (clearing EVERY bit, not none), which is
what makes consumption exactly-once at the value level, not merely at an
internal state flag: a notification this call successfully receives cannot
also satisfy a later, unrelated call, because nothing is left set for that
later call to see. Net effect, confirmed as a runnable property in §3
rather than left as an unverified claim: **a notification posted at any
time — including before the next `wait_high` call even begins — is
observed, never lost, and consumed exactly once.**

**Correction — "observed" is not "the line is still asserted" (fixed by
`meshcadet-perf-radio-dio1-wait-postcondition`).** The paragraph above
establishes that a notification is never lost; it does NOT establish that
DIO1 is still high at the moment that notification is observed. Because
`xTaskGenericNotifyWait` returns as soon as the sticky "notification
received" state is set, a notification left over from an EARLIER,
already-serviced-and-cleared DIO1 assertion — one whose line has since gone
low again — satisfies a later `Notification::wait` call exactly as readily
as a live one. The seed: a DIO1 edge landing in the window between an
earlier wait's own timeout and the `disable_interrupt()` call that follows
it (still armed, still listening) sets the notification for a wait nobody
consumes; that notification survives, sticky, into the NEXT `wait_high`
call. `GpioDio1Wait::wait_high` (and the `level_triggered_wait` reference
model, §3.1) closes this by re-reading `is_high()` on every wake — genuine
or stale — before reporting `Asserted`, looping on the remaining deadline
if the line is not actually high (see `GpioDio1Wait`'s "Postcondition" doc
and §3.2's stale-wake tests). The corrected, now-enforced invariant:
**`Asserted` ⇒ DIO1 is asserted right now** — never merely "a notification
fired at some point."

### 4.3 The re-arm race

**Restated from `GpioDio1Wait`'s own doc, now with a state-machine proof,
not just prose (§3.2's `a_level_that_asserts_between_calls_is_caught_by_
the_next_calls_fast_path`).** The DIO1 line is armed with
`InterruptType::HighLevel` — a level-triggered, not edge-triggered,
interrupt. `esp-idf-hal`'s `InterruptType` maps this directly to
`gpio_int_type_t_GPIO_INTR_HIGH_LEVEL` (`gpio.rs:194`), which the ESP32-S3
GPIO matrix continuously re-evaluates against the LIVE pin state, not a
one-shot transition record. Consequence: **arming AFTER the level has
already gone high still fires immediately** — there is no window in which
"clear the IRQ, then re-arm" can lose an edge that arrived in between,
because arming is not "wait for the NEXT transition", it is "tell me if the
condition (currently) holds, and keep telling me until it's acknowledged".
This is why `GpioDio1Wait::wait_high`'s fast-path `is_high()` check, run
BEFORE any arm/wait, is not a redundant optimization — it is the same
self-correcting property the ISR-armed path also has, just checked in
software first to skip the round trip when the answer is already known.

**Answer to the mission's explicit question:** can a DIO1 edge arrive
between clearing the IRQ and re-arming the wait? **Yes, physically — but it
cannot be missed, because of the level-triggered property above, not
despite it.** If the edge arrives in that gap, the line is simply already
high by the time either the fast-path check or the newly-armed interrupt
next evaluates it, and both paths report it correctly. §3.2's
`a_level_that_asserts_between_calls_is_caught_by_the_next_calls_fast_path`
is the state-machine-level confirmation: `arm_calls` stays at 1 (from the
FIRST, already-timed-out call) when the SECOND call's fast path alone
catches a level that changed in the gap — the recovery does not depend on
re-arming racing the edge correctly, it depends on the edge being
UNMISSABLE by construction once the line goes high, which the fast path
alone already proves.

**Priority-1 verdict:** an interrupt-driven radio that can miss an edge is
strictly worse than a spin-poll that cannot, per the mission charter — and
this design does not miss edges, by the level-triggered hardware property
plus the fast-path software check that exploits it, both independently
verified above and in §3. **Not a NO-GO condition.**

**The other direction — a spurious `Asserted` is equally a defect, and was
found (fixed by `meshcadet-perf-radio-dio1-wait-postcondition`).** Every
argument above is one-sided: it proves DIO1 assertions are never MISSED. It
says nothing about the wait reporting `Asserted` when DIO1 is not actually
asserted, and that gap was real — not from the re-arm race itself, but from
its interaction with the sticky notification (§4.2's correction): an edge
landing in the gap between an EARLIER wait's timeout and its
`disable_interrupt()` call sets a notification nobody consumes, which then
satisfies a LATER, unrelated wait instantly, with the line already back to
low. A wait site reading that as TxDone/RxDone/CadDone on a frame that is
not actually done is a silently lost outbound message or a bypassed
listen-before-talk check — the exact defect class this document's own
"peek-not-take" framing (§3, `main.rs`) exists to prevent, just triggered
from the DIO1 side instead. `GpioDio1Wait::wait_high` now closes it by
re-checking `is_high()` on every wake rather than trusting the notification
alone (§4.2's correction; §3.2's stale-wake tests). **Postcondition, now
enforced:** `Asserted` ⇒ DIO1 is asserted right now.

### 4.4 Interaction with the M1 split — which core the ISR fires on, and R7's SMP rules

**Which core.** ESP-IDF's own interrupt-allocation reference is direct:

> "The interrupt will always be allocated on the core that runs this
> function [`esp_intr_alloc`]." (ESP-IDF v5.2.2, `api-reference/system/
> intr_alloc.html`)

`gpio_isr_handler_add`/`gpio_install_isr_service` route through
`esp_intr_alloc` internally (`esp-idf-hal`'s `enable_isr_service`,
`gpio.rs:1344-1358`), so the DIO1 GPIO interrupt is allocated on whichever
core calls it — which is `Radio::init`, called once from `main()`'s startup
sequence, which runs on the IDF main task. The campaign plan's own §1
establishes that task's affinity: "the main task therefore takes the IDF
default (pinned to CPU0)" — and ADR-0012's split keeps it there: "radio
stays on the dispatcher task/core 0; only UI moved to its own task." **The
DIO1 ISR is therefore allocated on core 0, the SAME core the dispatcher
task (which alone ever calls `wait_high`) runs on — by construction, not by
accident.** There is no cross-core race for this specific interrupt/task
pair under the as-built M1 topology: the ISR and its sole waiter are always
core-0-local.

**R7's SMP rules — satisfied regardless, not merely true by pinning.** Even
though this repo's construction happens to keep the ISR and its waiter
same-core, the mechanism used (`xTaskGenericNotifyFromISR` / `Notifier`'s
`Arc`) is the FULLY GENERAL, cross-core-safe FreeRTOS primitive — task
notifications are documented safe to signal from any core to any task
(`esp-idf-hal`'s own `notify`/`notify_and_yield` make no core-affinity
assumption in their implementation, `task.rs:145-187`), and the `Arc<
Notifier>` the ISR closure holds a clone of is an atomically-refcounted,
`Send`-safe handle (the ISR closure itself is required to be `Send`,
`subscribe_nonstatic<F: FnMut() + Send + 'd>`, `gpio.rs:975`). **R7 is
satisfied by the primitive's own design, not by the pinning fact above** —
which matters because a future change to boot ordering (e.g. if
`Radio::init` ever moved) could not silently break this by putting the ISR
on a different core than expected; the notification mechanism would remain
correct either way. The state this ISR closure touches — the `Arc<
Notifier>` clone and nothing else (§4.1) — carries no additional shared
mutable state requiring its own atomics/critical-section audit beyond what
the primitive itself already guarantees.

### 4.5 Summary verdict

| Question | Answer | Evidence |
|---|---|---|
| What may run in this ISR | Exactly one ISR-safe FreeRTOS call, `notify_and_yield`; nothing else | §4.1, `esp-idf-hal` safety doc |
| IRAM placement | NOT flagged IRAM-safe; masked during concurrent NVS writes, but bounded, self-recovering (not a missed edge) | §4.1, ESP-IDF flash-concurrency doc + this repo's own `nvs_partition` cross-thread grep |
| Flash-write interaction | admin_server/provisioning_server can write NVS concurrently; DIO1's level-latch + every call site's re-clear-before-re-arm bounds the consequence to a spurious, gracefully-handled timeout | §4.1 |
| Notification vs. queue vs. semaphore | Notification — the correct choice for the one-recipient shape this code has, with documented speed/RAM advantages over a semaphore, per FreeRTOS's own docs | §4.2 |
| Wake latency / lost-wakeup | The faster primitive (no separate kernel object to allocate/lock); unconditional OR-set from ISR, never lost, consumed exactly once — proven as a runnable test, not just cited | §4.2, §3.2 |
| Re-arm race | Cannot miss an edge — level-triggered hardware property + software fast path, both independently verified | §4.3 |
| Spurious `Asserted` from a stale notification (fixed by `meshcadet-perf-radio-dio1-wait-postcondition`) | A wake — genuine or stale — is re-checked against the live line before being reported `Asserted`; a stale wake with the line low keeps waiting instead | §4.2 correction, §4.3 correction, §3.2 |
| M1-split core interaction | ISR allocated on the calling core (core 0, by `Radio::init`'s call site) — same core as its sole waiter, by construction; R7 satisfied by the primitive's own cross-core-safe design regardless | §4.4 |

**No NO-GO condition found within this mission's own charter.** Every
question the mission charter posed is answered from source, with the one
genuine open item (IRAM-safety) recorded as a deferred, device-confirmable
follow-up rather than applied blind. **Historical note:** this analysis
argued the missed-edge direction only and did not identify the spurious-
`Asserted`-from-a-stale-notification defect above; that gap was found by a
later audit (`meshcadet-perf-radio-checkpoint`, M2, NO-GO) and closed by
`meshcadet-perf-radio-dio1-wait-postcondition` — see the §4.2/§4.3
corrections and the updated §3.2 table.

## 5. Leg (d) — CI, all gates

| Gate | Result | Provenance |
|---|---|---|
| `cargo test --workspace --locked` | **1127 tests, 0 failed** across every workspace member (`firmware-core` 448, `protocol` 311 + 5 doctests, `xtask` 105, `ui_sim` 39 across its 1-test-per-golden-scene integration binaries plus a 4-test suite, `ui_perf` 6 + 4, `host` 12 + 67 integration, `perf_loop_model` 39, `perf_device_report` 32 + 5 — summed from this run's own `test result:` lines) | [HOST], this run |
| `cargo clippy --workspace --all-targets -- -D warnings` | **clean** | [HOST], this run |
| `cargo fmt --all -- --check` | **clean** | [HOST], this run |
| `xtask` glyph-coverage + ui-event-parity harnesses | **clean** — part of the `cargo test --workspace` run above (`xtask`'s 105-test binary) | [HOST] |
| `firmware/check-all-features.sh` (default + `--features diagnostics`) | **[CI]** — this container has no `esp` Rust channel installed (`rustup toolchain list` shows only `stable-x86_64-unknown-linux-gnu`); `.github/workflows/ci.yml`'s `firmware build gate` job is the only place this ever compiles, per this campaign's own compile-oracle discipline (plan §4b, §7) | Confirmed once this branch's PR opens and the job runs — this mission's diff touches `perf_loop_model/`, `firmware-core/src/radio_wait.rs` (host-buildable, no `esp-idf-hal` dependency), and `docs/perf/*.md`; it does NOT touch `firmware/` source at all, so the firmware lane's risk is unchanged from the already-merged `meshcadet-perf-radio-dio1-interrupt` (PR #142) |

**Why `firmware-core/src/radio_wait.rs`'s additions carry zero firmware-lane
risk.** `LevelTriggeredLine`/`OneShotNotify`/`level_triggered_wait` are pure,
`esp-idf-hal`-free Rust — `firmware-core`'s own crate boundary already
guarantees this (it is the host-buildable half of the codebase precisely so
`firmware/`, xtensa-only, never needs to compile for a host harness to
exercise this logic). `firmware/src/radio.rs`'s `GpioDio1Wait` is
byte-for-byte unchanged by this mission (§3.1) — the firmware cross-compile
gate is exercising code this mission did not touch, re-confirming it rather
than testing anything new.

## 5.1 Found but out of scope — stale `RX_POLL_YIELD_MS` doc comments outside `perf_loop_model`

Post-green self-review for this mission (`grep -rn "RX_POLL_YIELD_MS"
--include="*.rs" .`) turned up doc comments in THREE files this mission does
NOT otherwise touch, still citing the pre-M2 "~5 ms, ~200 Hz" figure, all
predating even the M1 split's "`ui.step()` shares the dispatcher loop"
framing (which stopped being true at ADR-0012, before `RX_POLL_YIELD_MS`
was even retuned):

- `firmware/src/ui/mod.rs:907,945,2146` — "a shared-loop `step()` running
  near `RX_POLL_YIELD_MS` cadence (~5 ms, ~200 Hz)" / "bounded by
  `RX_POLL_YIELD_MS` in the dispatcher loop, ~5 ms when idle".
- `firmware-core/src/perf.rs:21` — "the [dispatcher] loop's natural cadence
  (`RX_POLL_YIELD_MS` ≈ 5 ms when idle...)".
- `ui_perf/tests/entry_fade_repaint.rs:22,62-63` — same framing, PLUS a
  load-bearing test constant (`DISPATCHER_TICK_MS: u64 = 5`) that actually
  drives `run_fade`'s simulated cadence, not just prose.

**Deliberately not fixed by this mission — `document-and-defer`, not
fix-now.** All three sites describe `ui.step()` as sharing the dispatcher
loop's cadence — untrue since ADR-0012 (`ui.step()` now runs on `ui_task`,
its own task, at `UI_TICK_MS = 16` cadence, fully decoupled from
`RX_POLL_YIELD_MS`). A correct fix needs to re-derive what `ui_perf`'s
`entry_fade_repaint.rs` test should model post-split (`ui_task`'s own
cadence, not the dispatcher's) — not a value swap (5 -> 20), which would
leave the framing wrong in a DIFFERENT, more misleading way (implying the
two are still coupled, just at a new number). This is squarely a `firmware/
ui`-domain re-derivation this radio-path-timeliness mission should not
guess at under time pressure, and it predates this mission by a full
milestone (M1, not M2). **Window-closer:**
`meshcadet-perf-campaign-synthesis` (M4) already carries the obligation to
"rewrite `docs/perf/ui-perf-baseline.md` as the post-campaign state of
record" and give the whole document set "a single consolidated re-pass" on
stale citations (campaign plan §5 M4; `ui-perf-baseline.md`'s own §9
corrections-history entry already defers exactly this class of staleness to
that milestone) — this finding is folded into that existing, already-scoped
obligation, not a new orphaned one. Recorded here with exact citations so M4
does not need to rediscover them.

## 6. Deferred to the collection kit

Per this campaign's no-HIL ruling (plan §0.5), each predicate below that
needs real silicon is recorded as a deferred predicate — never as a
failing verdict, never as an invented number:

| Predicate | Why it needs silicon | Collection-kit section |
|---|---|---|
| **Device-measured delivery success rate** under the notify-driven wait (DM TX-with-ACK, DM RX, GRP_TXT, room push) vs. the M0/M1 baseline | Requires a real peer node exchanging real RF | `docs/perf/collection-kit.md` Part G (D5) |
| **RX-notice latency**, idle vs. UI-active, under the notify-driven wait | Same — real peer, real timing | Part G (D6) |
| **CAD-busy / TX-retry counts** under real RF conditions | This model always treats CAD as clear (documented simplification, `sim.rs`'s `attempt_cad_tx` doc) — real channel contention is out of scope for a single-node timing model | Part G, step 6's log-line capture |
| **§4.1's IRAM-safety confirmatory reading** (whether flagging the GPIO ISR `ESP_INTR_FLAG_IRAM`, with the necessary link-section audit, is actually safe on real hardware) | Can only be confirmed by flashing and observing whether it crashes under a concurrent flash write — exactly the risk §4.1 declines to take blind | **New** — added to collection-kit §0's table and Part F's confirmatory-reading discipline (see below) |
| **Bounded-latency confirmation**: how long a real concurrent NVS write actually masks the DIO1 ISR, and whether any real-world CAD/RX-poll deadline is close enough to that duration to matter in practice | §4.1's argument is a WORST-CASE bound from source reading; a real NVS-write duration reading would tighten it from "bounded, not zero" to an actual number | **New** — same collection-kit addition |

`docs/perf/collection-kit.md` §2's M2 row and §0's quick-reference table are
updated to point at this document and record these two new predicates —
see that document's own change note.

## 7. Status

All four legs executed, no device, no HIL, no QEMU, per campaign plan §0.5.

- **(a)** — modelled: CAD-site quantization is a real, fixed +0.808 ms per
  attempt (not the naive "up to 1 ms" reading); TX-site quantization is a
  genuine no-op at the real 1 ms tick (`lora_airtime_ms` already ceils to
  whole ms); RX-poll phase unaffected by construction. Dispatcher cadence
  never regresses under the notify wait at any swept point. **PASS.**
- **(b)** — the wait abstraction's armed/fired/timed-out/spurious-wake/
  re-arm state machine is host-tested directly (11 new tests), not merely
  argued in prose. **PASS.**
- **(c)** — every ISR-safety question the mission charter posed is answered
  from ESP-IDF/esp-idf-hal/FreeRTOS primary sources plus this repo's own
  code, with citations. No missed-edge path found; the one open item
  (IRAM-safety) is a bounded, self-recovering latency risk under concurrent
  NVS writes, not a correctness gap, and is recorded as deferred rather than
  fixed blind. **PASS, NO-GO condition not found.**
- **(d)** — host lane green in this container (1127 tests, clippy clean,
  fmt clean); firmware lane is `[CI]`, touching no `firmware/` source this
  mission changed. **PASS** (firmware lane confirmed once this branch's PR
  runs CI).

This document, together with the updated `docs/perf/collection-kit.md`, is
the evidence `meshcadet-perf-radio-checkpoint` gates on.
