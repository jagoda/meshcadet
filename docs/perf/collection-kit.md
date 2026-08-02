# On-device performance collection kit

**Audience: a human operator with a T-Deck Plus, a terminal, and (for one
part only) a second MeshCore-speaking node. Not an agent — no automated
process runs this document, ever.** Every step below produces a value you
read off the serial console and paste into the report block at the end. There
is no "does it feel faster" step anywhere in this document.

**Regenerated for the post-split (M1) build,
2026-08-02, by `meshcadet-perf-task-split-host-validation`.** ADR-0012's
dispatcher/UI task split (`meshcadet-perf-ui-task-split`, PR
jagoda/meshcadet#134) changed the diagnostics log format this kit reads from
— see Part C and Part G step 8 below for the specifics, and §0's table for
which predicates that affects. Everything not called out as changed is
unchanged from the M0 version of this document.

## 0. What this closes, and what it doesn't

This is the one place in the `meshcadet-perf-rearchitecture` performance
review that needs real silicon. Everything host-testable already has an
answer: `docs/perf/ui-perf-baseline.md` (real measurements + analytical
derivations) and `docs/perf/perf-loop-model-baseline.md` (a host discrete-event
simulation). What remains is the handful of numbers only a flashed device can
produce — listed in `ui-perf-baseline.md` §8's deferred-predicate register —
plus the constants `perf_loop_model` currently carries as a cited sensitivity
*range* rather than a measured point.

This document **supersedes** `ui-perf-baseline.md` §8.1's interim procedure.
Where that procedure was already correct, this one reuses it verbatim; where
it was vague ("time tap-to-first-frame", no format for what comes back), this
one tightens it. Once a section here closes a predicate, strike that row from
`ui-perf-baseline.md` §8 and move the number into the document body with a
**[DEVICE]** tag, per that document's own instruction.

**Quick reference — which part closes what:**

| Predicate (from `ui-perf-baseline.md` §8) | Closed by |
|---|---|
| D1 — on-target render cost, idle vs. 200-msg conversation | **[BLOCKED as of the M1 ref]** — needed the dispatcher's `ui_step` phase timing, which post-split neither the dispatcher nor `ui_task` records (see `task-split-host-validation.md` §6). Was: Part C, scripted scenario. |
| D2 — real per-flush SPI command overhead | **[BLOCKED as of the M1 ref]** — same root cause as D1. Was: Part C, scripted scenario (same run as D1). |
| D3 — real dirty-line-count distribution in use | **[BLOCKED as of the M1 ref]** — same root cause as D1/D2 (the `ui_step`-duration proxy this predicate leaned on no longer exists). Was: Part C, **partial** — see that section's honesty note. |
| D4 — longest UI-unserviced gap vs. payload size | **[BLOCKED as of the M1 ref]** — Part G step 8 reads the `PERF ui-starvation` line, which the M1 split removed from the dispatcher's rollup with no replacement (`meshcadet-perf-task-split-host-validation`'s `docs/perf/task-split-host-validation.md` §6 — same underlying number as ADR-0012's deferred predicate D-E). Needs a follow-up instrumentation call (`firmware_core::perf::PerfRollup::record_ui_starvation`, already written and unit-tested, just uncalled from `ui_task.rs`) before this row is runnable again. |
| D5 — delivery success rate (the hard constraint) | Part G (two-device) — unaffected by the M1 log-format change |
| D6 — RX-notice latency, idle vs. UI-active | Part G (two-device) — unaffected; `rx-notice-latency` is still one of the dispatcher's 5 post-split `PERF phase=`/`PERF rx-notice-latency` lines (Part C) |
| D7 — per-core utilization | Part C (every capture window reports it for free) — unaffected |
| D8 — post-change stack high-water mark, per task | Part E — **expanded** for the M1 ref: `ui_task` now has its own periodic HWM log alongside `main`/`admin_server`/`prov_server`'s pre-existing ones (see Part E) |
| D9 — SPI2 concurrent-access confirmatory reading | Part F — **still not runnable**, reason changed: the M1 split now provides the concurrency Part F's reading needs, but the GPIO-toggle probe it also needs does not exist yet in `radio.rs`; see that section |
| D10 — felt frame rate / tap-to-first-frame | Part D |
| The loop model's swept constants (`perf_loop_model/src/params.rs`) | Part D — calibration table |

## 1. Prerequisites and time budget

**Hardware:**
- Always needed: one T-Deck Plus, a USB cable, a host machine with the
  `esp` Rust toolchain (`docs/../README.md`'s "Building and flashing"
  section — `espup install`, `. ~/export-esp.sh`, `cargo install espflash
  --locked`).
- Needed **only** for Part G (delivery + radio timing under UI load): a
  second MeshCore-speaking node (a stock companion-app phone, or another
  MeshCadet unit) on the same channel/preset, per
  `docs/hil-real-mesh-procedure.md`. Parts A–F need nothing but the T-Deck.

**Time budget** (wall-clock, approximate — this is guidance for scheduling
the session, not a device measurement):

| Part | What | Needs peer? | Approx. time |
|---|---|---|---|
| A | Build, flash, confirm | no | 10–20 min (longer on a from-scratch toolchain/dependency build) |
| B | Provision (contacts/channel) | no | 5 min |
| C | M0 per-phase baseline capture | no | 10–15 min |
| D | Loop-model calibration | no | 10 min (reuses Part C's log) |
| E | Stack high-water-mark reading | no | 5 min |
| F | SPI2 confirmatory reading | no | 0 min — not runnable yet, see below |
| D10 | Felt-snappiness stopwatch/video steps | no | 10 min |
| G | Delivery + radio timing under UI load | **yes** | 20–30 min |

Parts A–D10 (everything except G) can be run solo, in one sitting, without
waiting for a second node or another operator.

## 2. Which build to check out — parameterised by ref

This kit is run **once now**, for the M0 baseline, and **again** after each of
the campaign's structural milestones lands, for a before/after comparison. It
is written once, against a `<REF>` placeholder, rather than as three
near-duplicate documents:

- **M0 run (now):** `<REF>` = `main` at the commit that carries the on-device
  diagnostics instrumentation (`firmware/src/perf.rs`,
  `firmware-core/src/perf.rs`, and the `--features diagnostics` call sites in
  `firmware/src/main.rs` / `firmware/src/ui/mod.rs`). Confirm it's present
  before you start:
  ```sh
  git -C firmware log --oneline -1
  test -f firmware/src/perf.rs && grep -n "PERF phase=" firmware/src/main.rs
  ```
  This checks for the actual PR jagoda/meshcadet#120 artifacts — the
  `perf.rs` module and the `PERF phase=` rollup log line — rather than the
  `diagnostics` Cargo feature name, which exists in `firmware/Cargo.toml`
  independent of whether this ref's instrumentation code has landed. If
  `perf.rs` doesn't exist, or the grep prints nothing, this ref predates the
  instrumentation and this kit cannot run against it.
- **M1 run (after the task/core split lands):** `<REF>` = the merge commit
  that lands the split — concretely, `meshcadet-perf-ui-task-split`'s merged
  PR jagoda/meshcadet#134 (or any later ref on `main`, since the split's
  `ui_task.rs`/log-format shape is unchanged by this document's own PR).
  Confirm you're on a post-split ref before running Part C: `grep -n "fn
  spawn" firmware/src/ui_task.rs` should find it; a pre-split ref has no
  such file.
- **M2 run (after the radio-path timeliness work lands):** `<REF>` = the merge
  commit that lands it.

Check it out and confirm you're on it:

```sh
git checkout <REF>
git rev-parse --short HEAD    # this is the value you paste as build_ref below
git diff --quiet HEAD || echo "-dirty"   # append -dirty to the ref above if this prints
```

The report-back block (§9) carries this exact value in its `build_ref` field
— that is what lets `meshcadet-perf-device-report-ingest` (and the loop-model
re-calibration it drives) tell an M0 reading apart from an M1 or M2 one.

## Part A — Build, flash, confirm

Single device. No peer needed.

```sh
cd firmware
cargo run --release --features diagnostics 2>&1 | tee ../meshcadet-capture-<REF>-baseline.log
```

If `espflash` can't auto-detect the port, set `ESPFLASH_PORT` rather than
passing `-- --port ...`: the firmware target's custom flash runner
(`firmware/.cargo/config.toml` → `scripts/flash-with-partition-table.sh`)
only ever forwards its first positional arg (the built ELF) to `espflash` —
anything `cargo run` appends after `--` is silently dropped before it
reaches `espflash flash`/`write-bin`/`monitor`, so `-- --port ...` has no
effect here even though the identical-looking form works for the `host`
crate's own CLI (Part B below) and for `README.md`'s top-level "Building and
flashing" section, neither of which goes through this runner:

```sh
ESPFLASH_PORT=/dev/ttyACM0 cargo run --release --features diagnostics 2>&1 | tee ../meshcadet-capture-<REF>-baseline.log
```

(Swap `/dev/ttyACM0` for `COMx` on Windows or `/dev/cu.usbmodem*` on macOS.)

**Confirm the flash landed this exact ref.** At boot, expect:

```
firmware build: <short SHA[-dirty]>
identity ready: pub_hash=0x<hh>, pubkey=<64 hex chars>
```

The `firmware build:` line must match the ref you checked out in §2. If it
doesn't, the flash didn't land — re-run `cargo run` (the `--no-skip` runner
forces a fresh write every time, so this is never a stale-cache problem, only
a "did the command actually run" one).

**If the boot screen shows "Ask an admin to connect via USB"** the device is
unprovisioned — expected on first boot or after an NVS erase. Proceed to Part
B before continuing. If it goes straight to a contact list, it's already
provisioned from a prior session; skip to Part C.

## Part B — Provision (single device)

Needed once, from a fresh/erased device. In a **second terminal**, from the
repo root (not `firmware/`):

```sh
cargo run -p host -- --port /dev/ttyACM0 status
cargo run -p host -- --port /dev/ttyACM0 identity
```

Provision one **dummy** contact — Part D's loop-model calibration needs
something to address a DM to, but does **not** need it to be a real,
listening device (a queued DM still triggers a real CAD attempt and a real
`radio.transmit()` block regardless of whether anything answers it). Generate
32 random bytes for the fake pubkey:

```sh
openssl rand -hex 32
```

```sh
cargo run -p host -- --port /dev/ttyACM0 add-contact --pubkey <the 64 hex chars just printed> --name "calibration-target"
cargo run -p host -- --port /dev/ttyACM0 commit
```

The device reboots into the contact list. This step is skippable if the
device is already provisioned with at least one contact from a prior session
— reuse that one.

## Part C — M0 per-phase baseline capture (closes D1, D2, D3-partial, D7)

Single device. This is the highest-value section: it produces the actual
per-phase superloop numbers the loop model (Part D) currently only has as a
sensitivity range.

**What you're reading — CHANGED for the M1 (post-split) ref.** Every 30
seconds the **dispatcher** (`main` task) logs one rollup block, and
**separately**, every 30 seconds, `ui_task` logs its own, shorter block —
two independent log sources now, not one (ADR-0012 D9 row 10: "`ui_task`
gets its own rollup; the two are never shared"). Exact format:

Dispatcher (`firmware/src/main.rs`, diagnostics-gated) — **`ui_step` and
`ui-starvation` are GONE, not zeroed:** the dispatcher no longer calls
`ui.step()` at all post-split, so neither line is coherent there any more
and both are removed outright (do not wait for them; their absence is
correct, not a capture failure):

```
PERF phase=gps: n=<count> min=<us> mean=<us> max=<us> p95=<us>
PERF phase=battery: n=<count> min=<us> mean=<us> max=<us> p95=<us>
PERF phase=cad: n=<count> min=<us> mean=<us> max=<us> p95=<us>
PERF phase=tx: n=<count> min=<us> mean=<us> max=<us> p95=<us>
PERF phase=rx_poll: n=<count> min=<us> mean=<us> max=<us> p95=<us>
PERF rx-notice-latency: n=<count> min=<us> mean=<us> max=<us> p95=<us>
PERF core-utilization: core0=<pct or n/a> core1=<pct or n/a>
```

`ui_task` (`firmware/src/ui_task.rs`, **new** for M1) — once, at boot, then
every 30 s:

```
ui_task: subscribed to Task WDT (30 s timeout)
ui_task: stack HWM: <free_B> B free / 32768 B total = <peak_B> B peak (<pct>% headroom)
PERF input-to-first-paint: n=<count> min=<ms> mean=<ms> max=<ms> p95=<ms>
```

(`input-to-first-paint` moved here from the dispatcher's rollup, same
format, `diagnostics`-gated same as before — D9 row 10.)

**D4/D-E's `PERF ui-starvation` line does not exist anywhere post-split** —
see §0's table. Nothing in this kit currently produces the number that row
used to close; skip straight to reporting `n/a — removed by the M1 split,
see docs/perf/task-split-host-validation.md §6` for that field rather than
waiting for a line that will never print.

All phase values are microseconds; input-to-first-paint is milliseconds.
`n=0` on any phase (e.g. `tx`/`cad` with nothing queued) reports `min=0
mean=0 max=0 p95=0` — that is the "no samples" case, not a real zero cost;
don't read it as one.

**A correct run looks like — CHANGED for the M1 ref.** Idle windows with
`cad`/`tx` at `n=0` in the dispatcher block, and **`core1` now showing REAL
utilization** (a nonzero percentage), not `n/a` — `ui_task` genuinely runs
on core 1 post-split, so a `core1=n/a` reading on an M1+ ref is itself a red
flag (it would mean `ui_task` never spawned, or the core-affinity pin
silently failed — see `ui_task.rs`'s headless-fallback doc for when spawn is
skipped). There is no `ui_step` or `ui-starvation` reading to sanity-check
any more (§0's D1-D4 rows, all `[BLOCKED]`). **A failed run** looks like the
device resetting mid-capture (a fresh `firmware build:` / `identity ready:`
pair appearing unexpectedly), or every phase reading `n=0` forever including
`gps`/`battery`/`rx_poll` (those three should never be zero — they run every
iteration unconditionally) — that means the `diagnostics` feature didn't
actually compile in; re-check Part A's build command. A missing `ui_task:
subscribed to Task WDT` line at boot means the UI half never came up at all
(headless boot) — check for the "I2C/touch init failed" or "LCD SPI init
failed" log lines just before it.

**[BLOCKED as of the M1 ref] — this procedure closes D1/D2/D3 only on a
pre-split (M0) ref.** Steps 2-4 below read the `ui_step` phase's `max` from
the dispatcher's rollup; that phase does not exist post-split (§0's table,
`task-split-host-validation.md` §6). Run it anyway for `gps`/`battery`/
`rx_poll`/`cad`/`tx`/`rx-notice-latency`/`core-utilization` (still valid and
useful on an M1+ ref, and `core-utilization` now shows real work on core 1 —
see the "correct run looks like" note above), but record D1/D2/D3 as `n/a —
blocked, see docs/perf/task-split-host-validation.md §6` rather than
guessing at a `ui_step` reading that will not appear. **Procedure — one
scripted 5-minute window**, deliberately touching every scenario D1/D2/D3
ask for in one pass rather than three separate captures:

1. [ ] Leave the device idle at the contact list for at least one full 30 s
       window with no taps. This is your **idle baseline** window.
2. [ ] Open a conversation with 20+ messages (any real conversation you have,
       or send yourself several DMs first via Compose to build one up).
       **Expected:** the log line for `ui_step`'s next 30 s window has a
       visibly higher `max` than the idle window's `max` — that single spike
       IS the navigation repaint (§4.1 of `ui-perf-baseline.md`: idle repaint
       cost is 0, a full-window navigation paint is a real, one-time cost;
       `max` isolates it from the `mean`/`p95` of the surrounding idle
       iterations in the same window).
3. [ ] If you have 200+ messages in one conversation, repeat step 2
       specifically against that conversation — **this exact scenario is
       D1's own citation** ("ContactList idle vs. a 200-message conversation
       open"). If you don't have 200 messages sitting in history and don't
       want to generate them, note in the report block that this specific
       sub-case was skipped; D1's headline order-of-magnitude claim doesn't
       depend on the exact conversation size, but the paired reading is more
       useful if you have it.
4. [ ] Trigger at least one incoming-message notification (send yourself a
       channel post from another provisioned contact if you have one, or ask
       whoever ran Part B's dummy-contact step to also add a second real
       contact ahead of time) so `CometOnNotify` fires at least once within a
       captured window.
5. [ ] Let the capture run at least 3 full 30 s windows total (idle + the
       navigation events above), so `gps`/`battery`/`rx_poll`'s percentile
       fields rest on more than a handful of samples.

**D3 honesty note — doubly blocked on an M1+ ref.** D3 asks for the real
*dirty-line-count* distribution in use. On the pre-split (M0) build, the
instrumentation landed by PR jagoda/meshcadet#120 times phases but does not
count dirty lines per frame, so `ui_step`'s duration was already only a
*proxy* (via `ui-perf-baseline.md` §4.1's ~128 µs/line data-only floor,
e.g. a `max` reading of ~2.8 ms roughly consistent with a ~22-line in-place
message-append repaint vs. a ~30.7 ms 240-line full-window one) — an
inference from timing, never a direct count. **On the M1+ ref the proxy
itself is gone too** (§0's table, `task-split-host-validation.md` §6): there
is no `ui_step` reading anywhere to infer from. Closing D3 exactly needs two
follow-ups now, not one: (1) restore a `ui_step`-equivalent phase timing on
`ui_task` (closes D1/D2 as a side effect), and (2) a genuinely new per-frame
dirty-line counter this instrumentation has never had, on either topology.
Record `n/a — blocked` for this predicate on an M1+ ref rather than
inferring from a reading that doesn't exist.

**D7 (per-core utilization)** needs no extra steps — every 30 s dispatcher
rollup block already includes the `core-utilization` line, unaffected by
the M1 log-format change. **Reading changed for the M1 ref:** expect
`core1` to show REAL, nonzero utilization now (`ui_task` genuinely runs
there) — confirming §1's *pre-split* finding is no longer the point of this
reading on an M1+ ref; it now confirms priority 3's "both cores carry real
work" claim (campaign plan §6 criterion 4) instead. A `core1=n/a` reading on
an M1+ ref is a red flag, not the expected baseline — see the "correct run
looks like" note above.

## Part D — Loop-model calibration (highest leverage)

Single device. This converts `perf_loop_model`'s ranged sensitivity
parameters (`perf_loop_model/src/params.rs::LoopModelParams::documented_
defaults()`) into measured points. Each field below has a real citation to a
`docs/perf/perf-loop-model-baseline.md` sensitivity range **or** is not
directly instrumented — this table says which, honestly.

**Procedure to get non-idle `cad`/`tx` samples:** from the touch UI, open
Compose against the dummy contact from Part B and send 5–10 short DMs, a few
seconds apart, during a capture window. Nothing needs to answer them — the
device still runs a real CAD attempt and blocks for the real LoRa airtime for
each one, because the frame is genuinely handed to the radio regardless of
whether a peer decodes it. Expect several `TX: <n> bytes, <airtime>ms
airtime` lines and a nonzero `n` on the `cad`/`tx` PERF phases in the window
that follows.

| `LoopModelParams` field | How to derive it from Part C's log |
|---|---|
| `ui_step` | **[BLOCKED as of the M1 ref]** — was: directly, the `ui_step` phase's `mean`/`p95`/`max` from Part C, in µs → convert to ms, replacing the whole `[0.05, 30.72]` ms range with these three points. That phase no longer exists on either task post-split (§0's table, `task-split-host-validation.md` §6). Leave at the documented range until the follow-up instrumentation lands. |
| `cad_spi_overhead` | Directly: take the `cad` phase's `mean` (µs → ms), subtract the analytical `CAD_ACTIVE_MS` constant (8.192 ms, `perf_loop_model/src/sim.rs::CAD_ACTIVE_MS`), floor at 0. That's the real SPI-command overhead ahead of the CAD-active window. **Unaffected by the M1 split** — `cad` is still one of the dispatcher's 5 post-split `PERF phase=` lines. |
| `gps_poll` | Directly: the `gps` phase's `mean`/`p95`/`max`, µs → ms. Note whether your capture window landed in GPS's quiet or active duty-cycle phase (`ui-perf-baseline.md`'s dispatcher-loop description) — if you only captured the quiet window, note that in the report; the active-window number needs a capture that happens to straddle a GPS active cycle. **Unaffected by the M1 split.** |
| `battery_poll` | Directly: the `battery` phase's `mean`/`p95`/`max`, µs → ms. **Unaffected by the M1 split.** |
| `frame_encode` | **Not directly instrumented**, unaffected by the M1 split. The `tx` phase's own duration is `radio.transmit()`'s block time (airtime), not the crypto/encode step ahead of it — those are two different call sites and only one is timed. Leave at the documented `[0, 2.0]` ms range; closing this exactly needs a follow-up timer around the `encode_room_keep_alive_frame`-style call sites. |
| `wdt_pet`, `tx_timestamp_rebase`, `room_keepalive_sched_check`, `drain_ui_command`, `periodic_stats` | **Not directly instrumented**, unaffected by the M1 split — no phase in `PerfRollup` covers any of these individually; they're folded into the untimed portions of each loop iteration. Leave at their documented ranges (all already small, sub-millisecond bounds anchored to in-repo constants — see each field's doc comment in `params.rs`). |
| `split_ui_idle_tick` | **Now applies, on an M1+ ref — but still not directly instrumented.** `ui_task.rs`'s real `UI_TICK_MS = 16` constant is already the exact high bound (`perf_loop_model/src/params.rs`'s field doc); no device reading narrows it further today, since nothing times the `evt_rx.recv_timeout` wake itself. Leave at the documented `[0, 16]` ms range. |
| `queue_handoff` (**new field**, added by `meshcadet-perf-task-split-host-validation`) | **Not directly instrumented.** No phase times the `std::sync::mpsc` `try_send`/`try_recv`/`recv_timeout` boundary crossing (ADR-0012 D3). Leave at the documented `[0, 0.2]` ms range; closing this needs a follow-up timer bracketing those calls in both `main.rs::send_ui_event` and `ui_task.rs`'s loop. |

Report every field you derived a real point for, and every field you left at
its documented range (with the reason), in the report block (§9) — this is
what lets `device-report-ingest` re-run `perf_loop_model` at the calibrated
point instead of the three-corner sweep.

## Part E — Stack high-water-mark reading (closes D8, plus R3)

Single device. Reads the existing periodic and one-shot stack-headroom logs.
No new steps beyond "leave it running and touch the admin path once":

- **Main task** (now the dispatcher only, post-split — Slint/UI locals no
  longer live on this stack, ADR-0012 D6), every 30 s, alongside the Part C
  dispatcher rollup:
  ```
  main-task: stack HWM: <free_B> B free / 49152 B total = <peak_B> B peak (<pct>% headroom)
  ```
  If `<pct>` reads under 8% (i.e. under ~4096 B free — `ui-perf-baseline.md`
  §7's stated re-evaluation threshold), flag it in the report notes.
- **`ui_task`** (**new for M1**, ADR-0012 D6/D-A), every 30 s, alongside its
  own Part C block — a 32 768 B budget, a strict subset of the pre-split
  main task's UI-side share:
  ```
  ui_task: stack HWM: <free_B> B free / 32768 B total = <peak_B> B peak (<pct>% headroom)
  ```
- **UI navigation one-shot samples** — tap into the admin path once during
  this session (gear icon → PIN entry → admin menu). **These now report
  `ui_task`'s HWM, not the main task's** — the call sites are unchanged
  (`firmware/src/ui/mod.rs`'s `log_stack_hwm`, which reports the CALLING
  task, D6), and `ui/mod.rs` now runs exclusively on `ui_task` (D4.2), so
  this is automatic, no new instrumentation needed:
  ```
  ui: navigate_to_pin_entry stack HWM: <free_B> B free
  ui: navigate_to_admin_menu stack HWM: <free_B> B free
  ```
  These fire at the exact call sites an earlier release-build overflow was
  traced to, so they are the most sensitive samples this kit can take —
  cross-check them against `ui_task`'s own periodic reading above, not
  against the main task's.
- **Admin-server / provisioning-server threads** — run any host CLI command
  against the device (e.g. `cargo run -p host -- --port /dev/ttyACM0 status`
  from Part B) to get one fresh sample of each — unaffected by the split:
  ```
  admin_server: stack HWM: <free_B> B free / 12288 B total = <peak_B> B peak (<pct>% headroom)
  prov_server: stack HWM: <free_B> B free / 8192 B total = <peak_B> B peak (<pct>% headroom)
  ```

Paste all of the above (main-task, `ui_task`, both UI one-shots, both server
threads) in the report block — that's D8/R3 closed, per task (now five
readings instead of four), in one pass.

## Part F — SPI2 concurrent-access confirmatory reading (D9 / ADR-0012 D-B)

**Still not runnable on the M1 ref — but the REASON changed. Read this
section again even if you read the pre-split version.**
`docs/perf/spi2-arbitration-r1.md`'s "What still needs silicon" section is
explicit: the correctness question (does `spi_master` serialise the LCD and
radio transactions correctly) is settled by source-and-datasheet reading
alone and needs no device reading at all. The one item named there is a
**confidence check, not a gate**: toggling a spare GPIO around the radio's
SPI wait point while a full-window repaint runs *concurrently, on a different
task/core*.

- **Pre-split (M0):** no such concurrency existed in the shipped firmware at
  all (everything was one task, one core) — a reading taken then would just
  show the existing sequential (never concurrent) execution, answering a
  different, already-settled question. That was the blocker.
- **Post-split (M1, this ref):** the concurrency this reading needs **now
  genuinely exists** — `ui_task` (core 1) and the dispatcher (core 0) run
  independently, per `meshcadet-perf-ui-task-split`/ADR-0012, confirmed by
  `meshcadet-perf-task-split-host-validation`'s parity matrix (`docs/perf/
  task-split-host-validation.md` §5.3/§5.4). **The blocker now is only the
  probe itself:** `radio.rs` still has no GPIO-toggle instrumentation around
  the SPI acquire/release points (PR #120's instrumentation is timer-based,
  not GPIO-based) — unchanged by either the split or this document's own
  regeneration, since neither mission touched `radio.rs`.

**Run this once the probe exists.** Steps, unchanged from the pre-split
version of this document:
1. Add a GPIO-toggle probe around the radio's SPI acquire/release points in
   `radio.rs` — a small follow-up addition, not yet built.
2. With that probe in place: toggle the GPIO immediately before the radio's
   SPI wait point and again immediately after the transaction returns, while
   triggering a full-window repaint (a contact-list navigation) on `ui_task`
   — the concurrency now exists to do this for real, unlike on M0. Expected
   reading: every interval ≤ ~15–20 µs (the analytically derived 12.8 µs
   bound plus scheduler/ISR jitter headroom, per `spi2-arbitration-r1.md`
   §Q5). A reading an order of magnitude past that is the signal that an
   assumption in that analysis (chunk size, DMA state, device count) doesn't
   hold on real hardware and needs re-derivation — not a correctness failure
   on its own, since §Q1-Q4's argument doesn't depend on this number.

Until the probe exists, record this section in the report block as `n/a —
post-split, concurrency exists, no probe yet` (not the pre-split `n/a —
pre-split, no probe exists` — the reason has changed) and move on.

## Part D10 — Felt snappiness (single device)

Reuses `ui-perf-baseline.md` §8.1.A verbatim — it was already correct and
copy-pasteable:

1. [ ] Cold boot → wall-clock backlight-on → splash first frame, and
       splash-dismiss → ContactList first frame (two stopwatch numbers).
2. [ ] From ContactList, tap into a 20+ message conversation with **no**
       motif firing — time tap-to-first-frame.
3. [ ] Repeat immediately after a message arrives (`CometOnNotify` active) —
       compare against step 2.
4. [ ] Compose → Send → time tap-to-`RocketOnSend`-first-frame and
       first-frame-to-MessageView-return; confirm the animation completes
       before the screen swaps.
5. [ ] Record slow-motion video (120/240 fps) of one full screen transition
       and one motif firing; count frames input→first visible change.

**One addition this kit makes over the original §8.1.A:** you don't have to
rely on the stopwatch alone for the aggregate number — Part C's
`input-to-first-paint` PERF line already logs an automatic, continuous
latency reading (min/mean/max/p95, ms) for every touch/keyboard input across
the whole session, including these five taps. Report both: the stopwatch
numbers for the specific named transitions above (steps 1–5, which the
automatic counter can't distinguish from each other — it's one pooled stat),
and the automatic `input-to-first-paint` block from whichever 30 s window
contained this sequence, as a sanity cross-check.

## Part G — Delivery + radio timing under UI load (closes D4, D5, D6)

**Needs a second MeshCore-speaking node.** Build/setup follows
`docs/hil-real-mesh-procedure.md` §§1–3 exactly (populate `hil_keys.rs`,
build with `--features hil,diagnostics` — a combination already exercised by
CI's `check-all-features.sh`), then register the printed pubkey in the peer's
companion app. Do that first if you haven't already; it's out of scope to
repeat here.

```sh
cd firmware
cargo run --release --features hil,diagnostics 2>&1 | tee ../meshcadet-capture-<REF>-delivery.log
```

This reuses `ui-perf-baseline.md` §8.1.B verbatim — that protocol was already
correct; nothing here changes its steps, only tightens what to report:

6. [ ] With the peer, send 20 DMs peer→T-Deck **while idly navigating** the
       T-Deck UI (tap between ContactList/MessageView every few seconds). Log
       CAD-busy count, TX-retry count and RxDone timestamps from the serial
       console (`RX RxDone: <n> bytes, rssi=<x>dBm snr=<y>dB (raw <a>/<b>)`,
       `CAD: channel busy, deferring retry <ms>ms`, `TX error: ... retained
       for retry in <ms>ms`).
7. [ ] Repeat step 6 with the T-Deck UI fully idle (screen asleep, no taps)
       as the CONTROL. Difference the two runs for D6 (RX-notice latency,
       idle vs. active) — Part C's `rx-notice-latency` PERF line gives you
       the aggregate number directly for each run; no manual timestamp
       differencing needed if you captured both windows.
8. [ ] Repeat step 6 at 10 B, 40 B and 255 B payloads for D4's payload-size
       scaling — one full 20-DM run **per payload size**, each producing its
       own report block (§9) with `payload_bytes` set accordingly.
       **[BLOCKED as of the M1 ref]** — was: read the `ui-starvation` PERF
       line's `longest` field after each, D4's number directly. That line no
       longer prints post-split (§0's table, `task-split-host-validation.md`
       §6). Still run this step for its OTHER value — steps 6-8 are also
       where D5/D6's delivery-success and RX-notice-latency counts come from
       — but record D4 as `n/a — blocked, see docs/perf/task-split-host-
       validation.md §6` for each payload size rather than waiting for a
       line that will not appear.
9. [ ] Trigger a T-Deck→peer DM **while** a screen transition or motif is
       mid-flight; confirm no new error class in the CAD/TX log lines and
       that the peer receives the DM correctly (correctness, not timing).
10. [ ] With a DM queued (send one, then immediately tap around before its
        ACK lands), repeat Part D10 step 2's tap-timing for taps landing
        while a CAD attempt is in flight vs. no TX pending.

**D5 (delivery success rate — the hard constraint).** For each of steps 6–8's
runs, count from the serial log: DMs sent (`TX: <n> bytes` lines addressed to
the peer), ACKs received (`ACK received: matches last-sent DM`), DMs received
from the peer (`RX DM from 0x<hh> ...`), and — if a channel is shared —
`RX GRP_TXT` lines for a channel post exchange. Report all four counts per
run; the success rate is ACKed-sends / attempted-sends and
received-and-decoded / peer-claimed-sent, computed by whoever ingests this
report, not something to compute by hand here (this avoids an operator
arithmetic slip becoming the number the campaign gates M1/M2 delivery
correctness against).

## 9. Report-back format

One block per run — one for the single-device baseline/calibration/stack-HWM
pass (Parts A–D10), and one **per payload size per UI-load state** for Part
G's sweep (so up to 6 additional blocks: 3 payload sizes × {idle, navigating}
— plus the felt-snappiness numbers can ride in whichever block's session they
were captured in). Named fields, not prose — paste this exact shape:

```meshcadet-perf-report
kit_version: 1
build_ref: <short SHA[-dirty], from §2>
capture_date: <YYYY-MM-DD, UTC>
section: baseline | calibration | stack-hwm | felt-snappiness | two-device-delivery
payload_bytes: <10 | 40 | 100 | 255 | n/a>
ui_load: <idle | navigating | n/a>
peer_present: <yes | no>
notes: <optional free text — anomalies, skipped sub-steps, reboots>
--- raw-serial-log ---
<paste the full serial console capture for this run, unmodified,
chronological>
--- end-raw-serial-log ---
```

Everything the report needs (phase rollups, TX/RX/CAD/stack-HWM lines) is
already in the pasted raw log in its own self-describing `key=value` shape —
the header above is what tells a mechanical reader which section, ref, and
scenario this particular paste belongs to; it does not need to re-parse or
summarize the log itself.

For Part D10's stopwatch/video numbers, which don't come from the serial
console, add them as additional `notes:` lines in the `felt-snappiness`
block, one line per named step (e.g. `notes: step2_tap_to_frame_ms=180;
step3_tap_to_frame_ms=210; step4_tap_to_rocket_ms=95`).

**Where this goes.** Paste the block(s) above wherever is convenient (a
tracking note, a file) and hand it to `perf_device_report` — `cargo run
-p perf_device_report --bin ingest_device_report -- <path>` — which parses,
archives it under `docs/perf/device-reports/` (schema in that directory's
`README.md`), and, for a `calibration` block, reports which of
`perf_loop_model`'s swept constants it can now replace with a measured
point. This crate never touches a serial device itself; it starts only
once a human has already produced the pasted text above.

## 10. Safety / recovery — returning to a normal build

`--features diagnostics` and `--features hil` are both compile-time flags,
not persistent device state — provisioning (contacts, channels, rooms, PIN)
lives in NVS and survives any reflash that doesn't erase it. To return to a
normal production build afterwards:

```sh
cd firmware
cargo run --release
```

This reflashes the plain production image (no `hil`, no `diagnostics`),
retaining the NVS-persisted identity and provisioning from before — the `hil`
build's fixed compiled seed does **not** overwrite the real, persisted
identity; it's read only while the `hil` feature is active. If you provisioned
the Part B dummy contact and don't want it lingering, remove it with the host
CLI's `del-contact` subcommand before or after returning to production:

```sh
cargo run -p host -- --port /dev/ttyACM0 del-contact --pubkey <the same 64 hex chars from Part B>
cargo run -p host -- --port /dev/ttyACM0 commit
```

It is otherwise harmless (an unreachable allowlist entry, never emitting
anything on its own).

If anything in this session left the device in a confusing state (stuck
mid-provisioning, a wedged screen), a full NVS erase and re-provision is
always safe and always recoverable — see
`docs/hil-real-mesh-procedure.md`'s Touch UI HIL procedure §A for the erase
command and what a correct first-boot screen looks like afterward.
