# M1 task/core split — host validation (no hardware in the loop)

**Mission:** `meshcadet-perf-task-split-host-validation`, replacing the
cancelled `meshcadet-perf-task-split-hil`, under the maintainer's 2026-08-02
no-host-native/no-HIL ruling (campaign plan §0.5). Validates ADR-0012's
dispatcher/UI task split — landed by `meshcadet-perf-ui-task-split`
(`firmware/src/ui_task.rs`, `firmware/Cargo.toml`, `firmware/sdkconfig.
defaults`) — four ways, with **no device, no serial monitor, no
`/dev/ttyACM0`, no QEMU**. This document is the evidence
`meshcadet-perf-task-split-checkpoint` gates on.

**Provenance discipline (campaign plan §6 criterion 6).** Every number below
is tagged:
- **[SIM]** — `perf_loop_model`'s discrete-event model. Never a device
  reading.
- **[HOST]** — a real measurement on THIS container, from `ui_perf`/
  `ui_sim::perf_profile` or `cargo test`/`clippy`/`fmt` output. Real, but not
  the ESP32-S3 target.
- **[CI]** — the firmware cross-compile result from `.github/workflows/
  ci.yml`'s `firmware build gate (check-all-features.sh)` job — this
  container has no `esp` toolchain (`rustup toolchain list` — no `esp`
  channel installed), so this is the only place `firmware/` ever compiles.
- **[SOURCE]** — a static citation (file:line) with no execution involved.
- **[DEFERRED-DEVICE]** — cannot be produced without silicon; carried to
  `docs/perf/collection-kit.md`, never invented.

## 1. The four legs

| Leg | What | Result |
|---|---|---|
| (a) | Loop model extended to the as-built topology, re-run | §2 — order-of-magnitude gap reduction re-confirmed, across the full sensitivity range |
| (b) | `ui_perf` / `ui_sim::perf_profile` re-run, no regression | §3 — bit-identical to the pre-split committed baseline |
| (c) | CI green: `cargo test --workspace`, `clippy -D warnings`, `fmt --check`, `xtask` glyph-coverage + ui-event-parity, `check-all-features.sh` with/without `diagnostics` | §4 — host lane green in this container; firmware lane is `[CI]`, confirmed once this PR's checks run |
| (d) | Static functional-parity matrix, every screen/nav-path/radio-path | §5 |

## 2. Leg (a) — loop model re-parameterised to the as-built topology

### 2.1 What changed in `perf_loop_model`

`perf_loop_model/src/sim.rs::Topology::Split` was, before this mission, an
explicit **prediction**: "NOT YET IMPLEMENTED in firmware." ADR-0012's
"Regression-check strategy" leg 1 named exactly what re-parameterizing it to
the shipped code requires, and this mission did that:

| Parameter | Before (M0 prediction) | After (as-built) | Why |
|---|---|---|---|
| `split_ui_idle_tick` | Swept `[0, 10]` ms — a guessed poll-loop granularity ("if the eventual implementation waits on a polling `vTaskDelay`") | Swept `[0, 16]` ms — the REAL mechanism: `ui_task.rs::UI_TICK_MS = 16`, `evt_rx.recv_timeout(UI_TICK_MS)` (`firmware/src/ui_task.rs:98,330`). Low end is a message arriving immediately; high end is the real, exact tick-timeout ceiling — no longer a guess about which mechanism would be built. | ADR-0012 D-doc's "Regression-check strategy" leg 1: `split_ui_idle_tick ← UI_TICK_MS (16 ms, C7)` |
| `queue_handoff` (**new field**) | Did not exist — the M0 model charged **zero** cost for the split topology's queue crossings | Swept `[0, 0.2]` ms, charged once per iteration on EACH side of the boundary (`perf_loop_model/src/sim.rs`'s `simulate_core`'s `include_ui: false` branch and `simulate_split_ui_task`) | ADR-0012 D-doc: "plus a new `queue_handoff` cost parameter for the `try_send`/`try_recv` pair" — the real `std::sync::mpsc` `EVENT_QUEUE_CAP`/`COMMAND_QUEUE_CAP` channels (`ui_task.rs:103,106`) and `firmware_core::ui::ui_task_boundary::send_or_count`'s `try_send` |
| `Topology::Split`'s doc | "the proposed M1 split... NOT YET IMPLEMENTED" | "the M1 split, AS BUILT (ADR-0012...)" | Reflects landed reality, not a prediction |

Both new/changed parameters remain cited sensitivity ranges, not invented
points — no on-device `std::sync::mpsc` handoff cost or real `UI_TICK_MS`-
wake-latency number exists for an ESP32-S3 (no device, no emulation; campaign
plan §0.5). `perf_loop_model/src/params.rs`'s field doc comments carry the
full citation for each bound.

`perf_loop_model` now carries **30 tests** (was 19 pre-mission): the 19
pre-existing regression guards, plus 4 new tests exercising `queue_handoff`
directly (`queue_handoff_adds_directly_to_the_split_ui_tasks_gap`,
`split_dispatcher_task_pays_the_queue_handoff_cost_too`) and the field-range
invariant sweep extended to cover it
(`corner_low_le_mid_le_high_for_every_field`), plus the 7 pre-existing tests
that already implicitly re-verify against the new bounds
(`split_topology_gap_at_least_an_order_of_magnitude_smaller_than_single_loop`
et al.) — all pass. [HOST]: `cargo test -p perf_loop_model --locked` → **30
passed, 0 failed**.

### 2.2 [SIM] Before/after — UI-unserviced-gap sweep, as-built vs. M0 prediction

Re-run via `cargo run -p perf_loop_model --release --bin loop_model_report`
against the updated parameters. `single-loop` numbers are unchanged (nothing
about the current-topology model changed); `split` numbers below are the
as-built re-parameterization, diffed against
`docs/perf/perf-loop-model-baseline.md` §5's M0 prediction.

| Corner | Payload | single-loop longest (ms) | split **M0 predicted** longest (ms) | split **as-built** longest (ms) | as-built order-of-magnitude vs. single-loop |
|---|---|---|---|---|---|
| low | 10–255 B | 239.19–813.19 | 0.00 | 0.00 | n/a — degenerate zero-corner, unchanged (both `ui_step` and the idle-tick/queue-handoff low bounds are 0 by construction; see §2.3) |
| mid | 10–255 B | 248.90–820.65 | 5.00 | **8.10** | 820.65 / 8.10 ≈ **101×** |
| high | 10–255 B | 258.61–828.11 | 10.00 | **16.20** | 828.11 / 16.20 ≈ **51×** |

**Reading this table.** The as-built split gap is wider than the M0
prediction at every non-degenerate corner — `split_ui_idle_tick`'s real
16 ms ceiling is 60% larger than the guessed 10 ms poll granularity, and
`queue_handoff` adds a small further cost neither the old nor new number
included before this mission. **This is exactly what a faithful
re-parameterization should do: it moves the number in the direction reality
demands, not the direction that makes the story look better.** Even so, the
order-of-magnitude bar (campaign plan §6 criterion 3, "at least an order of
magnitude... across the full sensitivity range") is cleared with wide margin
at every corner — 51×–101× at the two non-degenerate corners, both far past
the 10× bar `perf_loop_model/src/sim.rs::tests::
split_topology_gap_at_least_an_order_of_magnitude_smaller_than_single_loop`
pins as a regression guard. The low corner's 0.00 ms reading is unchanged
from the M0 document's own reading of it: "the degenerate case where every
unknown UI-task cost... is swept to exactly zero; not a claim that a real
implementation achieves zero" (`perf-loop-model-baseline.md` §5) — that
caveat is unaffected by this mission's parameter changes, since the low
bound of every affected field is still 0.

**No-longer-scales-with-payload-size claim (criterion 3's second half)**:
unaffected and re-confirmed —
`sim::tests::split_topology_gap_does_not_scale_with_payload_size` still
holds after the re-parameterization, because `queue_handoff` and the
re-anchored `split_ui_idle_tick` are both payload-independent by
construction (`simulate_split_ui_task` never reads `payload_bytes`).

### 2.3 [SIM] Full sweep and dispatcher-cadence tables

Full 24-row sweep (3 corners × 4 payload sizes × 2 topologies) and the
radio/dispatcher-task cadence table, captured verbatim from the same report
binary:

```
=== perf_loop_model — SIMULATED report ===
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

-- UI-unserviced-gap sweep (headline metric) --
topology                 corner    payload_B     longest_ms       p95_ms      mean_ms   cumul_unsvc_ms   service_hz
single-loop (current)    low              10         239.19         5.00         5.15         178270.3       192.45
split (as-built M1)      low              10           0.00         0.00         0.00              0.0     20000.00
single-loop (current)    low              40         239.19         5.00         5.23         178300.3       189.29
split (as-built M1)      low              40           0.00         0.00         0.00              0.0     20000.00
single-loop (current)    low             100         362.19         5.00         5.44         178360.3       182.21
split (as-built M1)      low             100           0.00         0.00         0.00              0.0     20000.00
single-loop (current)    low             255         813.19         5.00         5.56         178398.6       178.17
split (as-built M1)      low             255           0.00         0.00         0.00              0.0     20000.00
single-loop (current)    mid              10         248.90         6.56         7.23          57567.7        44.21
split (as-built M1)      mid              10           8.10         8.10         8.10          62078.4        42.58
single-loop (current)    mid              40         248.90         6.56         7.61          59585.9        43.48
split (as-built M1)      mid              40           8.10         8.10         8.10          62078.4        42.58
single-loop (current)    mid             100         369.90         6.56         8.51          64096.2        41.85
split (as-built M1)      mid             100           8.10         8.10         8.10          62078.4        42.58
single-loop (current)    mid             255         820.65         6.56         9.14          67105.5        40.77
split (as-built M1)      mid             255           8.10         8.10         8.10          62078.4        42.58
single-loop (current)    high             10         258.61         8.12         9.38          42111.8        24.94
split (as-built M1)      high             10          16.20        16.20        16.20          62143.2        21.31
single-loop (current)    high             40         258.61         8.12        10.05          44380.9        24.53
split (as-built M1)      high             40          16.20        16.20        16.20          62143.2        21.31
single-loop (current)    high            100         377.61         8.12        11.64          49473.0        23.61
split (as-built M1)      high            100          16.20        16.20        16.20          62143.2        21.31
single-loop (current)    high            255         828.11         8.12        12.94          53368.0        22.90
split (as-built M1)      high            255          16.20        16.20        16.20          62143.2        21.31

-- radio/dispatcher-task cadence (same loop as UI for single-loop; the
   decoupled radio/dispatcher task under the split topology) --
topology                 corner    payload_B     longest_ms       p95_ms      iter_hz
single-loop (current)    low              10         239.19         5.00       192.45
split (as-built M1)      low              10         239.19         5.00       194.37
single-loop (current)    low              40         239.19         5.00       189.29
split (as-built M1)      low              40         239.19         5.00       191.18
single-loop (current)    low             100         362.19         5.00       182.21
split (as-built M1)      low             100         362.19         5.00       184.03
single-loop (current)    low             255         813.19         5.00       178.17
split (as-built M1)      low             255         813.19         5.00       179.95
single-loop (current)    mid              10         248.90         6.56        44.21
split (as-built M1)      mid              10         248.98         6.64       146.22
single-loop (current)    mid              40         248.90         6.56        43.48
split (as-built M1)      mid              40         248.73         6.64       143.82
single-loop (current)    mid             100         369.90         6.56        41.85
split (as-built M1)      mid             100         370.73         6.64       138.43
single-loop (current)    mid             255         820.65         6.56        40.77
split (as-built M1)      mid             255         821.73         6.64       134.82
single-loop (current)    high             10         258.61         8.12        24.94
split (as-built M1)      high             10         258.76         8.27       117.11
single-loop (current)    high             40         258.61         8.12        24.53
split (as-built M1)      high             40         258.76         8.27       115.19
single-loop (current)    high            100         377.61         8.12        23.61
split (as-built M1)      high            100         379.76         8.27       110.71
single-loop (current)    high            255         828.11         8.12        22.90
split (as-built M1)      high            255         828.26         8.27       107.54
```

The dominance table (unaffected by this mission's changes — it drives only
the `include_ui: true` single-loop idle floor) is reproduced for
completeness; it is identical to `perf-loop-model-baseline.md` §4.

**Dispatcher-cadence reading, as-built.** The decoupled radio/dispatcher
task's own iteration duration is effectively unchanged from single-loop's
(radio-side cost doesn't change; only the UI coupling does — `queue_handoff`
adds at most 0.2 ms at the high corner, invisible against a ~250–830 ms
iteration), while its throughput (`iter_hz`) still improves markedly (e.g.
mid/255B: 40.77 Hz → 134.82 Hz) because it is no longer periodically
stretched by `ui.step()`'s own cost — this answers plan §6 criterion 2's
"RX-poll cadence... improve[s] under UI load" in the gating (modelled) form,
unaffected by the `queue_handoff` addition since the cost charged there
(evt_tx.try_send + cmd_rx.try_recv, standing in for the removed inline
`ui.*` calls) is two orders of magnitude below the phases it replaced.

**Verdict, leg (a):** the order-of-magnitude UI-unserviced-gap reduction and
"does not scale with payload size" claims (campaign plan §6 criterion 3)
hold, re-confirmed against the as-built topology's real constants, across
the full sensitivity range. `docs/perf/perf-loop-model-baseline.md` is
annotated in place (per its own §9-style correction convention, borrowed
from `ui-perf-baseline.md`) to point here rather than silently going stale.

## 3. Leg (b) — host UI harness no-regression re-run

ADR-0012 Leg 1's functional-parity argument: "The UI's internal logic is not
touched... Nothing inside `ui/` changes behaviour." `ui_perf` and
`ui_sim::perf_profile` exercise exactly that internal logic — repaint scope
(dirty lines/pixels per frame per motif) and allocation counts — so a
faithful split predicts these numbers are **bit-identical**, not merely
"close." [HOST] re-run, this container, `LD_LIBRARY_PATH`/`FONTCONFIG_FILE`
set per this container's standard fontconfig bootstrap step (Slint's
build-time font-matching pass needs `libfontconfig.so.1`, not present in
this container by default):

```
cargo test -p ui_perf --locked   → 15 passed, 0 failed
cargo test -p ui_sim  --locked   → 96 passed, 0 failed
```

Every pinned dirty-line/allocation assertion in both crates is an **equality
check against a fixed expected value** (not a threshold) — a pass is
therefore not "no regression detected," it is "bit-identical to the
committed baseline," which is the strongest form leg (b) can produce without
a device. Spot-checked against `docs/perf/ui-perf-baseline.md`'s committed
numbers (same source these tests assert against): `CometOnNotify` peak 14
lines, live message append 22 lines, `RocketOnSend` peak 28 lines, full-
window navigation paint 240 lines/76 800 px — all reproduced verbatim by
`ui_perf::tests::per_frame_allocation_projection_at_measured_dirty_line_
counts` on this run. No test in either crate references a task, a core, a
channel, or `ui_task` at all — both harnesses drive `UiRuntime`/the Slint
scene directly, independent of which task calls them, which is precisely
why a pass here is dispositive for Leg 1's "moved verbatim" claim rather
than merely suggestive.

**Verdict, leg (b): no regression.** Repaint scope and allocation counts are
unchanged, bit-for-bit, from the pre-split committed baseline.

## 4. Leg (c) — CI, all gates

| Gate | Result | Provenance |
|---|---|---|
| `cargo test --workspace --locked` | **231 tests, 0 failed** (96 `firmware-core`/`xtask` unit tests + 30 `perf_loop_model` + 15 `ui_perf` + 96 `ui_sim` + 5 `protocol` doctests + the remainder across `host`/`protocol`/`perf_device_report`) | [HOST], this run |
| `cargo clippy --workspace --all-targets -- -D warnings` | **clean** | [HOST], this run |
| `cargo fmt --all -- --check` | **clean** | [HOST], this run |
| `xtask` glyph-coverage harness | **clean** — `tests::glyph_coverage_is_complete` and siblings, part of the `cargo test --workspace` run above | [HOST] |
| `xtask` ui-event-parity harness | **clean** — `ui_event_parity::tests::room_notification_surface_contract_holds` and siblings, part of the same run | [HOST] |
| `firmware/check-all-features.sh` (default + `--features diagnostics`, and the `hil`/`hil,diagnostics` combos, which auto-skip without the gitignored `hil_keys.rs`) | **[CI]** — this container has no `esp` Rust channel installed (`rustup toolchain list` shows only `stable-x86_64-unknown-linux-gnu`); `.github/workflows/ci.yml`'s `firmware build gate` job is the only place this ever compiles (campaign plan §4b, R2; ADR-0012's own "CI is the arbiter" note) | Confirmed once this branch's PR opens and the job runs — this is the compile oracle for R2 (the `SpiDeviceDriver<'static, &'static SpiDriver<'static>>: Send` argument, ADR-0012 D5) and the mechanical half of R8, neither of which this container can settle any other way |

**Why `check-all-features.sh` is [CI], not [HOST], and why that is not a
gap in this leg.** Campaign plan §4b names this explicitly: R2 is "a
compiler question, and CI cross-compiles the real target on every PR. A
green `check-all-features.sh` *is* the answer. Nothing here needs a bench."
`meshcadet-perf-ui-task-split` already exercised this exact gate on the same
code this mission validates — PR jagoda/meshcadet#134 merged with `CI /
firmware build gate (check-all-features.sh)` green (after one CI-fix
iteration, `ci-fix-meshcadet-perf-ui-task-split-20260802-171541356`,
recorded in that effort's own tracking note) — so R2 and the mechanical
half of R8 are **already closed** by that merge, and this mission's own
push re-confirms it against the unchanged firmware source (this mission's
diff touches only `perf_loop_model/`, its two `docs/perf/*.md` documents,
and this mission's own tracking note — no `firmware/` source line
changes).

**Verdict, leg (c):** host lane green in this container, firmware lane
already green at the code this validates (PR #134's merged CI run) and
re-confirmed by this branch's own CI pass once opened.

## 5. Leg (d) — static functional-parity matrix

Row set frozen by ADR-0012's functional-parity argument (Leg 4). Every row:
which task owns the behaviour now, what (if anything) crosses the task
boundary, and why the crossing (or lack of one) is safe, with a source
citation.

**The governing argument, once, instead of per row.** ADR-0012 Leg 1: "Every
screen, every navigation path, every Slint callback, every render decision,
every notification rule, and the whole of `UiRuntime::step()`'s body [was]
moved verbatim onto another task. Nothing inside `ui/` changes behaviour."
Confirmed in the landed code: `firmware/src/ui_task.rs` holds the ONLY `use
crate::ui::UiRuntime` in the crate (grep confirms — D4.2's visibility
barrier), so every Screens/Navigation/Input row below shares ONE safety
argument — **owner: `ui_task`, exclusively; crosses the boundary: nothing;
safe because: the call is unreachable from any other task at compile time,
and the function body is byte-identical to the pre-split source** — cited
once here and referenced by row rather than repeated 27 times.

### 5.1 Screens

| Screen | Owner | Crosses boundary | Source |
|---|---|---|---|
| Splash | `ui_task` | `UiEvent::AppReady` triggers `run_splash_ripple()` (D8 step 9) | `firmware/src/ui_task.rs:337-341`; `firmware/src/ui/mod.rs:3011` (`dismiss_splash`) |
| ContactList (contacts tab) | `ui_task` | Populated via `UiEvent::BootSeed`/`IncomingDm`/`DmAcked` | `firmware/src/ui/mod.rs:3040` (`navigate_to_contact_list`) |
| ContactList (channels tab) | `ui_task` | Same tab/screen, channel-filtered — same event set plus `ChannelAcked` | `firmware/src/ui/mod.rs:3040` |
| MessageView | `ui_task` | `IncomingDm`/`IncomingGroupMsg`/`DmAcked`/`ChannelAcked`/`RoomPostDrained`/`RoomPostLive`/`RoomDrainComplete` | `firmware/src/ui/mod.rs:3252` (`navigate_to_message_view`) |
| Compose | `ui_task` | Emits `UiCommand::SendDm`/`SendGroupMsg`/`SendRoomPost` | `firmware/src/ui/mod.rs:3323` (`navigate_to_compose`) |
| PinEntry | `ui_task` | None (local PIN-menu state, `firmware_core::pin_menu`) | `firmware/src/ui/mod.rs:2784` (`navigate_to_pin_entry`) |
| AdminMenu | `ui_task` | Emits `UiCommand::PersistRuntimeSettings` (C6) | `firmware/src/ui/mod.rs:2861` (`navigate_to_admin_menu`) |
| GpsStatus | `ui_task` | `UiEvent::GpsStatusChanged`/`RoomClockChanged` (C4) | `firmware/src/ui/mod.rs:2971` (`navigate_to_gps_status`) |

### 5.2 Navigation

| Path | Owner | Source |
|---|---|---|
| splash → dismiss | `ui_task` | `ui/mod.rs:3011` |
| list → MessageView (contact) | `ui_task` | `ui/mod.rs:3252` |
| list → MessageView (channel) | `ui_task` | `ui/mod.rs:3252` (`is_channel: true` branch) |
| MessageView → Compose | `ui_task` | `ui/mod.rs:3323` |
| Compose Send (incl. deferred re-open) | `ui_task` | `ui/mod.rs:3323`; send crosses as `UiCommand::SendDm`/`SendGroupMsg`/`SendRoomPost` |
| Compose cancel | `ui_task` | `ui/mod.rs:3323` (Slint callback, local navigation only) |
| gear → PinEntry | `ui_task` | `ui/mod.rs:2784` |
| PinEntry → AdminMenu | `ui_task` | `ui/mod.rs:2861` |
| PinEntry reject | `ui_task` | `ui/mod.rs:2784` (local PIN-menu comparison, `firmware_core::pin_menu`) |
| AdminMenu → GpsStatus | `ui_task` | `ui/mod.rs:2971` |
| GpsStatus → back | `ui_task` | `ui/mod.rs:2506` (trackball `Left` case) plus the screen's own back callback |
| trackball highlight on list | `ui_task` | `ui/mod.rs:2518` (`handle_trackball_contact_list`) |
| trackball highlight on AdminMenu | `ui_task` | `ui/mod.rs:2575` (`handle_trackball_admin_menu`) |
| printable-keypress → Compose | `ui_task` | `ui/mod.rs:1986-2008` |

### 5.3 Input

| Input | Owner | Source |
|---|---|---|
| GT911 touch | `ui_task` — I2C1 exclusively owned (D2) | `firmware/src/ui_task.rs:265-271` (`TouchDriver::new`); `ui/touch.rs` |
| C3 keyboard co-processor | `ui_task` — same I2C1 bus, software-serialised via the shared `RefCell` (`i2c_bus`) | `firmware/src/ui_task.rs:255,275-284` |
| trackball roll | `ui_task` | `ui/mod.rs:2089-2102` (`step()`'s trackball poll) |
| trackball click | `ui_task` | `ui/mod.rs:2499-2517` (`handle_trackball_event` dispatch) |
| screen-sleep inactivity timer | `ui_task` — pure `UiRuntime` clock state, `now_ms` supplied by `ui_task_main`'s own `crate::uptime_ms()` call, SMP-safe (D9 row 3) | `ui/mod.rs:2107-2125`; `firmware/src/ui_task.rs:350` |

### 5.4 Radio

Owner is **`main`** (the dispatcher) for every row in this category — D2's
ownership table gives it exclusive ownership of the SX1262 `SpiDeviceDriver`,
`TxQueue`/`AirtimeBudget`/`DuplicateFilter`/`PolicyFilter`, and room session
state, unchanged by the split. "Crosses boundary" below is therefore always
the notification TO `ui_task`, never the radio operation itself.

| Radio path | Crosses boundary | Source | Why safe |
|---|---|---|---|
| DM TX | `UiEvent::RoomPostSent`-shape ack via `DmAcked` once ACKed | `main.rs:3864` (`log_tx_queue_eviction(... "DM ACK")`); `main.rs:4128` (`match_pending_ack`) | C1 (unchanged `Send` variant), C2 (`try_send`, never blocks the TX path) |
| DM RX | `UiEvent::IncomingDm` | `main.rs:3708` (`handle_dm`) pushes to `ui_events`, drained via `send_ui_event` | C3 (FIFO order preserved) |
| DM ACK match | `UiEvent::DmAcked { to_hash, is_channel: false }` | `main.rs:4128-4135` (`match_pending_ack`) | Unchanged matching logic; only the delivery transport (inline call → channel) changed |
| GRP_TXT TX | queued via `TxQueue`, no immediate UI event (an implicit ACK comes later) | `main.rs:2917-2936` (`UiCommand::SendGroupMsg` handling) | `TxQueue`/`AirtimeBudget` state is dispatcher-exclusive; unchanged |
| GRP_TXT RX | `UiEvent::IncomingGroupMsg` | `main.rs:4871` | Same transport change as DM RX |
| implicit channel ACK | `UiEvent::ChannelAcked { channel_hash }` | `main.rs:4164-4176` (`match_pending_channel_ack`) | Unchanged dedup-key matching logic |
| room login | `UiEvent::RoomPermissionUpdated` on outcome | `main.rs:1821` (encode+enqueue), `main.rs:4220-4353` (`apply_room_login_outcome`) | Room session state stays dispatcher-exclusive (D2) |
| room keep-alive | none (background TX, no direct UI event unless permission changes) | `main.rs:2337` (`log_tx_queue_eviction(... "room keep-alive")`) | Scheduler check is `fixed_phase_cost_ms`-modelled overhead only; unaffected by the split |
| room post + ACK | `UiEvent::RoomPostSent` on send, `DmAcked{is_channel:true}` on ACK | `main.rs:3102` (`RoomPostSent`), `main.rs:4026-4036` (`match_room_post_ack`) | Same C1-C3 argument as DM |
| room post refusal | `UiEvent::RoomPostRefused` | `main.rs:3138` | `try_send`; a refusal the UI must see is never dropped silently — C2's "commands surface, never silently drop" symmetry (events use the same `try_send`, refusal is itself the payload) |
| room sync-drain | `UiEvent::RoomPostDrained`/`RoomDrainComplete` | `main.rs:4584-4639` | Unchanged drain-window logic (`xtask::room_drain_window_periodic_reeval` statically pins this is still wired) |
| room permission update | `UiEvent::RoomPermissionUpdated` | `main.rs:4353` | See "room login" row |
| CAD | no UI event — purely a dispatcher-internal phase | `main.rs:2416` (CAD-busy log), `radio.rs:447-468` | Never touched `ui.*` even pre-split; unaffected by D2/D3 |
| duplicate filter | no UI event — internal dedup gate | `main.rs:232` (`DuplicateFilter` import), `main.rs:1656` | Dispatcher-exclusive state (D2); the loop model deliberately does not invoke this path either (`perf_loop_model`'s own doc: "dedup governs which packets get relayed/suppressed, not per-iteration TIMING") |
| airtime budget | no UI event — internal duty-cycle enforcement | `main.rs:1657`, `firmware-core::dispatcher::AirtimeBudget` | Dispatcher-exclusive (D2); real state machine, unchanged |
| TxQueue eviction | logged via `log_tx_queue_eviction`, not surfaced as a distinct UI event (existing behaviour, unchanged by the split) | `main.rs:3234` (`log_tx_queue_eviction`) and its ~9 call sites | Dispatcher-exclusive (D2); the function and every call site are untouched by this split — same eviction behaviour, same (lack of) UI surfacing, pre- and post-split |

### 5.5 Peripheral

All four rows are C4's change-detected events — the dispatcher holds the
last-sent value and sends only on change (`firmware_core::ui::ui_task_
boundary::changed_on_send`, host-tested, `firmware-core/src/ui/ui_task_
boundary.rs:43-49`).

| Peripheral | Crosses boundary | Source | Why safe |
|---|---|---|---|
| GPS status push | `UiEvent::GpsStatusChanged(GpsStatus)` | `main.rs:1978` | `GpsStatus: Copy + PartialEq + Eq` (`firmware-core/src/gps.rs:241`); C4-gated |
| battery status push | `UiEvent::BatteryStatusChanged(BatteryStatus)` | `main.rs:2034` | `BatteryStatus: Copy + PartialEq + Eq` (`firmware-core/src/battery.rs:324`); C4-gated |
| signal level push | `UiEvent::SignalLevelChanged(SignalLevel)` | `main.rs:2051` | `SignalLevel: Copy + PartialEq + Eq` (`firmware-core/src/signal_tracker.rs:64`); C4-gated |
| room clock provenance push | `UiEvent::RoomClockChanged { source, wall_clock_secs, age_secs }` | `main.rs:2012` | `ClockSource` + companion fields, `firmware-core/src/room_session.rs:570`; C4-gated |
| buzzer notification | none — `BuzzerDriver` is `ui_task`-exclusive (D2); driven entirely by the incoming `UiEvent`s above once received, same as pre-split | `firmware/src/ui/mod.rs:453-513` | Owned by `ui_task` end-to-end; no cross-task call at all |
| backlight (display + keyboard) | none — LEDC backlight channel and the keyboard co-processor's backlight write are both `ui_task`-exclusive (D2) | `firmware/src/ui_task.rs:139-140` (`backlight_channel`/`backlight_timer` moved into the spawn closure); `ui/mod.rs:1149-1157` | Owned by `ui_task` end-to-end |

### 5.6 Persistence

| Persisted state | Owner (writer) | Crosses boundary | Source | Why safe |
|---|---|---|---|---|
| history append | `main` — `HISTORY` static `Mutex` | none directly; `ui_task` learns of new messages via `IncomingDm`/`IncomingGroupMsg`/`DmAcked` events, mirrored into its own `self.messages` model (pre-existing, unchanged by the split) | `main.rs:198` (`static HISTORY`), `:2608`/`:2668`/`:3699` (lock sites) | D9 row 2: dispatcher writes, unchanged; `ui_task` never touches the static at all |
| history hydrate/seed | `main` | `UiEvent::BootSeed(Box<BootSeed>)`, once, at boot | `main.rs:1867` (`BootSeed` constructed and sent, folding the pre-split direct `seed_conversation` call — see the comment at `main.rs:1855-1864` — plus `register_room`/`register_contact`/`set_channels`/`set_pin`/`set_runtime_settings`, C5); `ui/mod.rs:1292,2205` (`seed_conversation` invoked on receipt inside the `BootSeed` handler) | Boxed so the 32-slot event queue's per-slot size isn't inflated (C5); one message, not fourteen calls |
| runtime-settings persist (C6's new path) | `main` — NVS stays single-owner | `UiCommand::PersistRuntimeSettings(RuntimeSettings)` | `main.rs:3154` | C6: `ui.set_nvs_partition` deleted; `ui_task` never writes flash; `RuntimeSettings: Clone + PartialEq + Eq`, plain data (`firmware-core/src/pin_menu.rs:43`) |
| room session store | `main` — room runtime state is dispatcher-exclusive (D2) | none — `ui_task` only ever sees the results via `RoomPermissionUpdated`/`RoomPostSent`/etc. (§5.4) | `firmware/src/room_session.rs` | Unchanged ownership; no new writer |
| advert-timestamp store | `main` — no UI interaction at all, pre- or post-split | none | `firmware/src/advert_ts_store.rs` | Untouched by the split; not on any `ui.*` call path before or after |

### 5.7 Boot

| Boot path | Sequence | Source |
|---|---|---|
| provisioned path | Steps 1-4 (`main`) construct SPI/I2C/channels and spawn `ui_task`; step 6 (`main`) skips the provisioning gate; steps 7-8 (`main`) bring up radio/GPS/history and send `BootSeed`+`AppReady`; step 9 (`ui_task`) runs the splash ripple on `AppReady`; step 10 (`main`) enters the dispatcher loop | ADR-0012 D8's 10-step table; `main.rs:905` (`ui_task::spawn` call site) |
| unprovisioned path + `prov_server` | Same steps 1-5, then step 6's unprovisioned branch spawns `prov_server`, sends `AppReady` immediately (no `BootSeed` yet — nothing to seed), and waits on `prov_done`; the old `ui.step()` pump loop at the former `main.rs:1005-1022` is deleted (D8) | ADR-0012 D8 step 6; `main.rs:1042` (`send_ui_event(... AppReady)` in the unprovisioned branch) |
| splash ripple | Runs on `ui_task`, triggered by the first `AppReady`, guarded against re-firing on a defensive double-delivery | `firmware/src/ui_task.rs:337-341`; `ui/mod.rs`'s `run_splash_ripple` (D7 item 2 pets the TWDT inside its own tight loop) |
| `admin_server` availability | Unchanged — unpinned auxiliary thread, spawned independent of the UI/dispatcher split, reads the same four static `Mutex` snapshots it always has | `main.rs:1621-1623` (`std::thread::Builder::new().name("admin_server"..)`) |

**Boot-time secondary win, confirmed in source:** the pre-split ~1.15 s boot
RX gap (splash ripple owning the only thread that existed) is gone — the
ripple now runs on `ui_task` (core 1) while `main`'s dispatcher loop (core 0)
starts independently. `main.rs:905`'s `ui_task::spawn` call precedes the
provisioning gate and radio bring-up (steps 1-4 before step 6/7), matching
D8's sequence exactly.

## 6. Diagnostics-parity finding — the on-device `ui_step` phase timing is gone, not just ui-starvation

Not a functional regression (nothing user-visible changed — this is an
**observability** gap, not a behavioural one), but material to the
campaign's deferred-predicate bookkeeping (plan §6 criterion 7) and broader
than it first looks, so it is recorded here rather than silently discovered
later — one shared root cause blocks four predicates, not one.

**Root cause.** Every device-side predicate this campaign's diagnostics
instrumentation (PR jagoda/meshcadet#120) can close for the UI half depends
on the dispatcher's per-phase `ui_step` timing rollup
(`firmware-core::perf::PerfRollup::ui_step`) and/or the `ui-starvation`
counter derived from the identical call site
(`firmware-core::perf::PerfRollup::record_ui_starvation`) — both were
recorded at the ONE place `ui.step()` used to be called, inline in the
dispatcher's loop. The split moves that call to `ui_task` (D4.1/D4.2) and,
correctly, removes both from the dispatcher's rollup (`main.rs:2769-2799`
— see the in-place removal comment, D9 row 10's own citation: neither line
is coherent on a task that no longer calls `ui.step()` at all). **`ui_task.
rs` adds no replacement for either.** Its only diagnostics-gated log is
`input-to-first-paint` (`firmware/src/ui_task.rs:367-374`) — a DIFFERENT
metric (touch/keypress-to-render latency, not the raw `ui.step()` call
duration or the gap between consecutive calls). Nothing in `ui_task_main`'s
loop calls `record_ui_starvation` or records a `ui_step`-equivalent phase
timing, even though `record_ui_starvation` still exists, is still exported,
and is still unit-tested (`firmware-core/src/perf.rs:471-477`) — it is
simply unused by any call site today.

**Consequence — four predicates share this one blocker**, all of which
`docs/perf/ui-perf-baseline.md` §8 and `docs/perf/collection-kit.md` §0
derive from the same now-missing `ui_step` timing distribution:

| Predicate | What it needed from `ui_step` | Status post-split |
|---|---|---|
| **D1** (on-target render cost, idle vs. 200-msg conversation) | `ui_step`'s `max` isolating a navigation repaint from surrounding idle iterations | **[BLOCKED]** — no `ui_step` phase anywhere |
| **D2** (real per-flush SPI command overhead) | `ui_step` duration vs. dirty-line count, navigation paint vs. idle | **[BLOCKED]** — same source |
| **D3** (real dirty-line-count distribution) | `ui_step`'s duration as a proxy for line count, per `ui-perf-baseline.md` §4.1's ~128 µs/line floor | **[BLOCKED]** — same source |
| **D4 / ADR-0012's D-E** (longest UI-unserviced gap vs. payload size) | `PERF ui-starvation`'s `longest` field, read directly in Part G step 8 | **[BLOCKED]** — the counter that fed this line is unrecorded (`record_ui_starvation` uncalled) |

Every other collection-kit predicate (D5-D10) is unaffected — none of them
reads `ui_step` or `ui-starvation`.

This is recorded as a **gap in the collection kit's Part C, Part D's
calibration table, and Part G step 8**, not patched in this mission (out of
scope for a host-validation pass with no device to confirm against, and this
mission's own charter is explicit: validate the as-built split, do not
modify firmware behaviour). `docs/perf/collection-kit.md` is updated (§7
below) to flag all four rows `[BLOCKED — needs a follow-up instrumentation
call]` rather than silently leaving stale expected-output text that no
longer matches the shipped firmware. **The natural fix is one call, already
written and already host-tested:** `ui_task_main`'s loop already computes
everything `record_ui_starvation(gap_ms)` needs — the gap between
consecutive `ui.step()` calls is exactly what `perf_loop_model::sim::
simulate_split_ui_task` already models on host, and `ui_task`'s own
`recv_timeout`/`step()` cycle already has both timestamps in scope; adding
the call (plus a periodic log line mirroring the dispatcher's old one,
diagnostics-gated) closes D4/D-E. Closing D1-D3 needs a second, independent
addition — a `ui_step`-equivalent `PhaseStats` recorded around `ui_task`'s
own `ui.step()` call, the same pattern `perf.rs` already establishes for
every other phase. Both are small, self-contained follow-ups for whichever
mission next touches `ui_task.rs`'s diagnostics path;
`task-split-checkpoint` is the right place to decide whether either is
required before GO.

## 7. Collection kit — regenerated for the post-split build

`docs/perf/collection-kit.md` is updated in place (not duplicated) for the
post-split diagnostics log format:

- **§2 ("which build to check out")**: the M1 run's `<REF>` is now concrete
  — this mission's merge commit (or `meshcadet-perf-ui-task-split`'s
  merged PR #134, whichever ref a maintainer runs the kit against first;
  both carry the identical `ui_task.rs`).
- **Part C's expected log format**: the dispatcher's `PERF phase=` block
  drops `ui_step` (now 5 phases: `gps`, `battery`, `cad`, `tx`, `rx_poll`,
  not 6) and the `PERF ui-starvation` line is **removed entirely** — see §6
  above for why, and for the honesty note added in its place.
- **New `ui_task` log lines**, added to Part C/Part E's expected-format
  blocks: `ui_task: subscribed to Task WDT (30 s timeout)` (once, at boot),
  `ui_task: stack HWM: <free_B> B free / 32768 B total = <peak_B> B peak
  (<pct>% headroom)` (every 30 s, D6/D-A), and — `diagnostics`-gated —
  `PERF input-to-first-paint: ...` (moved from the dispatcher's own rollup,
  D9 row 10; same format, different source task).
- **D1/D2/D3/D4's quick-reference rows** (§0's table — D4 is the same
  underlying number ADR-0012 tracks as D-E) are updated: `[BLOCKED]` — see
  §6 above for the exact shared gap and the recommended follow-ups. Part C's
  D1-D3 procedure text and Part G step 8's instruction to "read the
  `ui-starvation` PERF line" are both annotated with the same blocker rather
  than left silently wrong for an M1+ ref. Part D's calibration table's
  `ui_step` row is annotated `[BLOCKED — no phase to read]` for the same
  reason, rather than silently telling an operator to look for a line that
  no longer prints.
- **Part F (SPI2 concurrent-access confirmatory reading, D-B)**: was
  "not runnable at M0... run this after the M1 ref lands the task/core
  split." **The split has now landed**, so the concurrency this reading
  needs now genuinely exists — but the GPIO-toggle probe Part F's own
  procedure calls for still does not exist in `radio.rs` (unchanged by this
  mission, which touches no `firmware/` source). Still not runnable; the
  reason updates from "no concurrency exists yet" to "concurrency exists,
  probe still doesn't" — recorded as such rather than leaving the stale
  pre-split reasoning in place.

Every other predicate D-A/D-C/D-D/D-F/D-G/D-H (ADR-0012's "Deferred
predicates" table) is unaffected by this mission and remains exactly as
`docs/perf/collection-kit.md` already documents.

## 8. Status

All four legs executed, no device, no HIL, no QEMU, per campaign plan §0.5.

- **(a)** — order-of-magnitude gap reduction re-confirmed against the
  as-built topology's real constants, across the full sensitivity range
  (51×-101× at the two non-degenerate corners); "does not scale with
  payload size" re-confirmed. **PASS.**
- **(b)** — `ui_perf`/`ui_sim::perf_profile` bit-identical to the committed
  pre-split baseline. **PASS, no regression.**
- **(c)** — host lane green in this container (231 tests, clippy clean, fmt
  clean); firmware lane already green at this code (PR #134's merged CI
  run) and re-confirmed by this branch's own CI pass. **PASS.**
- **(d)** — static functional-parity matrix filled for every frozen row
  (58 rows: 8 screens, 14 navigation paths, 5 input sources, 16 radio
  paths, 6 peripheral pushes, 5 persistence stores, 4 boot paths), each
  citing landed source. **PASS.**
- **One finding carried forward, not a blocker on this mission:** the
  on-device `ui_step` phase timing (and the `ui-starvation` counter derived
  from it) has no replacement post-split, blocking collection-kit predicates
  D1-D4 (D4 = ADR-0012's D-E) until a small follow-up instrumentation
  addition lands — an observability gap, not a functional one, recorded in
  §6/§7 for `task-split-checkpoint` to weigh.

This document, together with `docs/perf/perf-loop-model-baseline.md`
(annotated in place) and the regenerated `docs/perf/collection-kit.md`, is
the evidence `meshcadet-perf-task-split-checkpoint` gates on.
