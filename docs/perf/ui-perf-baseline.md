# UI performance pass — Phase-1 measurement contract & baseline

Phase 1 of the UI performance work. This is the shared measurement CONTRACT
every downstream optimization phase (`repaint-scope-opt`, `alloc-and-tick-opt`,
`render-radio-contention-opt`) measures its own before/after numbers against.

Runs parallel to the in-flight theme work — this document, `ui_perf/`, and
`ui_sim/src/perf_profile.rs` + `ui_sim/src/alloc_count.rs` add ONLY analysis +
host-side benchmarks; no edits land on `firmware/src/ui/mod.rs` or
`firmware/src/ui/screens/*.rs`.

## 1. Why "automatable" and "on-hardware" are a hard split here

`firmware/` cross-compiles for `xtensa-esp32s3-espidf` and links
`esp-idf-svc`/`esp-idf-hal`. Its `[[bin]]` target sets `harness = false`
(`main()` must be the esp-idf entry point, not a synthesized libtest runner),
so `cargo test` in `firmware/` only **type-checks** its `#[cfg(test)]` blocks
— confirmed in this sandbox: `cargo test --no-run` in `firmware/` compiles a
valid Xtensa ELF test binary (`file` reports "ELF 32-bit LSB executable,
Tensilica Xtensa"), which cannot execute on this x86_64 host (no
qemu-xtensa). Firmware's own tests, including the very fixtures this
baseline pins against, have **never actually run** except on real hardware.

That makes the automatable/on-hardware line a hard architectural boundary,
not a convenience split:
- **Automatable (this document + `ui_perf/`)**: anything that is pure logic
  with no Slint/esp-idf types, ported to a host-native workspace crate and
  pinned against firmware's own fixture values. Real numbers, real host
  execution, zero hardware needed.
- **On-hardware (an operator, protocol in §8)**: anything that touches actual
  timing of the display SPI flush, the radio SPI/RF path, or felt
  UI responsiveness. No amount of host analysis substitutes for this — the
  numbers below for those axes are **analytical estimates** anchored on
  in-repo doc comments and formulas, explicitly flagged as such, not
  measurements.

## 2. Measurement harness — `ui_perf/` (new workspace crate)

Added to the root workspace (`Cargo.toml` members). Ports the two pure
render/state-build hot paths named in this phase's objective:

- `ui_perf::render_logic::render_mentions` — host port of
  `UiRuntime::render_mentions` (`firmware/src/ui/mod.rs:2514`).
- `ui_perf::render_logic::build_message_items` — host port of
  `UiRuntime::build_message_items` (`firmware/src/ui/mod.rs:2477`).
- `ui_perf::counting_alloc::CountingAllocator` — a generic counting
  `GlobalAlloc` wrapper (alloc/dealloc/realloc counts + bytes), installed as
  this crate's global allocator. This is the "per-`step()` allocation-count
  hook" the acceptance criteria ask for; its on-target wiring (an
  identically-shaped allocator behind firmware's `diagnostics` feature,
  bracketing one real `UiRuntime::step()` call) is **deferred to
  `alloc-and-tick-opt`** per this phase's scope note — see the module doc
  in `ui_perf/src/counting_alloc.rs` for the exact hand-off design.

Both ported functions are algorithmically identical to the firmware
originals (same `protocol::codec`/`protocol::mention` calls — already
host-buildable, no_std, unit-tested in `protocol/` itself — same
String/Vec assembly order). Correctness is pinned against the **exact**
fixture input/output pairs firmware's own `#[cfg(test)]` module in
`firmware/src/ui/mod.rs` asserts (each ported test cites its firmware
mirror by line number). Since firmware's own tests never execute (§1), this
is the only place those fixtures are actually verified to hold at all,
until on-hardware confirmation.

DRIFT CONTRACT (historical — see UPDATE below): if `build_message_items`/
`render_mentions` change behavior in a later change, mirror the change in
`ui_perf/src/render_logic.rs` too — the pinned tests here are the tripwire.

**UPDATE (`firmware-core-extract-ui-runtime` increment):** `build_message_items`/
`render_mentions` (and the `MessageRecord`/`MessageItem` types they operate
over) moved from `firmware/src/ui/mod.rs` into `firmware_core::ui::
message_view` — a root-workspace crate, host-testable under `cargo test
--workspace`, with no Slint dependency. `firmware`'s own tests (§1's "never
execute" problem) now execute for real, in `firmware-core`, closing the gap
this section describes. `ui_perf::render_logic` no longer ports these two
functions; it `pub use`s the real ones from `firmware-core` and keeps only
the synthetic `bench_fixtures` generator, so the DRIFT CONTRACT above no
longer applies — there is exactly one implementation and one set of
correctness tests, in `firmware-core`, not two.

A second harness, `ui_sim/src/perf_profile.rs` + `ui_sim/src/alloc_count.rs`
(added in this same pass), covers the OTHER half of the acceptance criteria
that `ui_perf` cannot: redraw-scope (dirty-region size per screen/animation)
and render-path allocation counts. It does NOT port/reimplement anything —
it drives the REAL `firmware/src/ui/motifs.slint` components through the
IDENTICAL Slint renderer API (`MinimalSoftwareWindow` + `ReusedBuffer` +
`render_by_line`) production code uses, with a counting `LineBufferProvider`
standing in for the SPI write. See §3b below and `perf_profile.rs`'s own
module doc.

Reproduce:
```sh
cargo test -p ui_perf                                # 15 tests, correctness + allocator unit tests
cargo run  -p ui_perf --release --bin ui_perf_bench  # timed benchmarks + allocation counts (§3)
cargo test -p ui_sim --test perf_profile -- --nocapture  # redraw-scope capture (§3b)
```

## 3. Baseline numbers — AUTOMATABLE (measured, this host, release profile)

```
=== ui_perf_bench — Phase-1 host baseline ===
profile=release

-- render_mentions (firmware/src/ui/mod.rs:2514) --
render_mentions[plain]:         iters=200000  ns_per_op=45.8
render_mentions[other_mention]: iters=200000  ns_per_op=63.4
render_mentions[self_mention]:  iters=200000  ns_per_op=56.4

-- build_message_items (firmware/src/ui/mod.rs:2477) --
build_message_items[n=10]:   ns_per_op=1330.7   (~133 ns/record)
build_message_items[n=10].alloc:  alloc=27  dealloc=27  bytes_allocated=2010   net_live_bytes=0
build_message_items[n=50]:   ns_per_op=7870.0   (~157 ns/record)
build_message_items[n=50].alloc:  alloc=134 dealloc=134 bytes_allocated=10023  net_live_bytes=0
build_message_items[n=200]:  ns_per_op=29343.0  (~147 ns/record)
build_message_items[n=200].alloc: alloc=534 dealloc=534 bytes_allocated=40323  net_live_bytes=0
```

Full run archived by re-running the command above (deterministic modulo host
scheduling noise; ±~15% run-to-run on this box).

**Reading these numbers:**
- Cost scales **linearly** with conversation size (~133–157 ns/record across
  a 20x size range, no quadratic blowup) — not a hotspot at any conversation
  size this app plausibly reaches (tens to low hundreds of messages).
- ~2.5–2.7 allocator calls per message, ~75 bytes/message, **net_live_bytes
  == 0 in every case** (no leak-shaped growth — every allocation is freed
  within the same call).
- Crucially: this cost is paid on `navigate_to_message_view` /
  `refresh_message_view_for` (`firmware/src/ui/mod.rs:2538`/`:2430`) — i.e.
  once per conversation OPEN or per new-message REFRESH, **not once per
  `step()`/frame**. Even at n=200 (worst plausible conversation size), the
  one-time cost is ~29 µs — three orders of magnitude below a single video
  frame budget. See §5 for how this re-ranks the alloc-and-tick suspect.

xtensa-esp32s3 (Xtensa LX7 @ 240 MHz, no hardware FPU acceleration comparable
to this host's x86_64) will be slower in absolute terms and the release
profile differs (firmware ships `opt-level = "z"`, size-optimized; this bench
ran the workspace default `opt-level = 3`/`lto` release profile) — treat
these as **relative/shape** baselines (linear scaling, allocation counts,
which branch costs more) rather than an absolute on-target prediction.
Absolute on-target timing is an on-hardware item (§5).

## 3b. Baseline numbers — MEASURED redraw-scope (dirty-region, real Slint renderer)

Added in this baseline pass: `ui_sim/src/perf_profile.rs` +
`ui_sim/tests/perf_profile.rs`. Unlike §3 (pure logic) and §5.3's original
STATIC audit below (grep-level reasoning about `.slint` markup), this rig
drives the **actual production renderer**: a real `firmware/src/ui/
motifs.slint` scene rendered through `MinimalSoftwareWindow` +
`RepaintBufferType::ReusedBuffer` + `render_by_line` — bit-for-bit the same
API `firmware/src/ui/platform.rs::TDeckWindowAdapter::render_if_needed`
calls in production. A counting `LineBufferProvider` reports how many lines
Slint's own dirty-tracker decided to touch per frame, instead of writing
pixels to an SPI display. Because the dirty-region DECISION is made entirely
inside Slint (not reimplemented or approximated here), this is a real
measurement of production repaint scope, not a guess.

Reproduce:
```sh
cargo test -p ui_sim --test perf_profile -- --nocapture
```

```
=== ui-perf-baseline: redraw-scope (dirty-region) capture ===
  frame0 (initial full paint)      lines=240/240  px= 76800  widest_line_px=320
  frame1 (idle, no property change) lines=  0/240  px=     0  widest_line_px=  0
  RocketOnSend peak dirty frame    lines= 86/240  px= 16473  widest_line_px=200
  RocketOnSend: 40 dirty ticks over its one-shot transition
  CometOnNotify peak dirty frame   lines= 14/240  px=   700  widest_line_px= 50
  CometOnNotify: 31 dirty ticks over its one-shot transition
=== end redraw-scope capture ===
```

**Reading these numbers:**
- **Idle is confirmed a true no-op** (0 lines, 0 px) — directly confirms
  `platform.rs`'s own doc claim ("at idle... `render_if_needed` is a
  no-op") for THIS scene shape (static backdrop + settled one-shot motifs).
  A screen with no animation in flight and no navigation pending costs
  nothing per iteration on the render side, regardless of loop rate.
- **CometOnNotify stays small**: 14/240 lines (~6% of the frame), 700 px
  peak — matches the "small, bounded motif" expectation from the original
  static audit (§5.3 below). A header-band-height sweep animation is cheap.
- **RocketOnSend is BIGGER than the static audit's "small nested motif"
  framing assumed**: 86/240 lines (~36%), 16473 px peak, 200px-wide single
  lines. `RocketOnSend`'s own markup (`motifs.slint`) is a compact 20×24
  `Rectangle`, but its child `Image` translates `y` from `0` to `-40px`
  while fading opacity — Slint's dirty-region tracker computes the union of
  the item's bounding boxes across the frame delta, and (per this
  measurement) that union — plus whatever the software renderer
  conservatively re-composites underneath a moving semi-transparent layer —
  is materially larger than the component's own static footprint. This is
  new information the static audit could not have found by reading the
  `.slint` source alone. It is still well short of a full 240-line repaint,
  but **this REVISES §5.3/§7's "softened, not demoted" framing upward**:
  `repaint-scope-opt` has a real, measured, non-trivial target in
  `RocketOnSend` specifically (and by extension any other translate+fade
  one-shot built the same way) — see the updated §5 item 3 and §7 below.
- Every one-shot motif settles back to a 0-dirty steady state once its
  `animate` transition completes (`rocket_ticks`/`comet_ticks` above are
  finite, not infinite) — confirms the "never an infinite loop" contract
  `motifs.slint`'s own module doc claims, from the render side rather than
  just reading the markup.

What this does NOT give: SPI transfer time or per-transaction bus-hold
overhead (host has no SPI bus) — §4's formula converts a dirty-line count to
an estimated SPI-hold time; combining §4's per-line floor with THIS
section's measured line counts gives a much tighter automatable estimate
than either alone (e.g. RocketOnSend's 86-line peak × ~13 µs/line ≈ 1.1 ms
data-only floor, once per send-tap — see the revised §5/§6 below).

## 4. Baseline numbers — ANALYTICAL (formula/doc-anchored, NOT measured; on-hardware confirms)

These convert existing in-repo doc comments and formulas into concrete
numbers, so "no regression" has a bound to check against later — they are
NOT host-measured and NOT a substitute for §5's on-hardware capture.

**Display SPI flush cost** (`firmware/src/ui/display.rs:36`, `:227-255`):
40 MHz SPI2, a 320-pixel RGB565 line's pure data transfer is documented at
"≤ 13 µs". `flush_line_range` is called once per dirty line
(`firmware/src/ui/platform.rs:239-292`, `process_line`), each call also
issuing the ST7789 CASET/RASET/RAMWR window-set commands via
`mipidsi::fill_contiguous` (overhead not quantified here — no hardware to
measure the actual per-transaction command cost).

| Dirty lines this frame | Data-only SPI floor (13 µs × lines) |
|---|---|
| 1 (single small motif, e.g. one `Twinkle` cell) | ~13 µs |
| ~40 (one motif's height, e.g. a `MascotBob`/`CometOnNotify` band) | ~0.5 ms |
| 240 (full `DISPLAY_HEIGHT`, worst case) | ~3.1 ms |

**Radio operation timing** (`firmware/src/dispatcher.rs:277`
`lora_airtime_ms`, SF7/BW62.5kHz/CR4:5 — the locked preset,
`firmware/src/radio.rs:611`), computed for representative frame sizes:

| Payload | Airtime (`radio.transmit` blocks the dispatcher loop this long) |
|---|---|
| 10 B (ACK-shaped) | 83 ms |
| 40 B (typical DM) | 165 ms |
| 100 B | 349 ms |
| 255 B (max) | 800 ms |

These are 2–5 ORDERS OF MAGNITUDE larger than any display-flush cost above —
`radio.transmit()`'s blocking window is RF airtime (the SX1262 chip
transmitting), not SPI-bus-hold time; the SPI bus itself is only touched for
the initial `WRITE_BUFFER`/`SetTx` commands (`firmware/src/radio.rs:262-296`),
then the loop polls the `DIO1` GPIO pin (not SPI) for `TxDone`. **The UI
render path cannot make this worse or better** — it is out of scope for
this analysis's contention story (§5 narrows the real contention point
accordingly).

`radio.try_receive`'s `poll_ms` window (`RX_POLL_YIELD_MS = 5 ms`,
`firmware/src/main.rs:1140`) is also a `DIO1` GPIO watch, not an SPI hold —
correctness is unaffected by poll cadence (`try_receive`'s own doc: the
radio "stays in continuous RX throughout"); the only radio-timing-relevant
effect of a slow loop iteration is added CAD-attempt latency and RX-poll
cadence jitter (§5).

## 5. Ranked hotspot ledger

Ranked by (a) confirmed vs. still-to-confirm, (b) how directly it maps to
radio timeliness (the hard constraint) vs. felt snappiness (secondary).

1. **CONFIRMED, concrete — per-dirty-line heap allocation in the render hot
   path.** `firmware/src/ui/platform.rs:277-283`: `process_line` allocates a
   fresh `Vec<Rgb565>` on **every** call, and Slint's `render_by_line` calls
   `process_line` once per dirty line — up to 240 times for a full-window
   repaint. This is a genuine per-FRAME allocation site (unlike
   `build_message_items`, which is per-navigation — see item 4 below). It
   was a deliberate, already-documented stack-vs-heap tradeoff (see that
   file's own "STACK-SAVING FIX" comment) justified as "dwarfed by the SPI
   transfer time" — true in absolute terms (§4: a 320-entry `Vec<Rgb565>`
   alloc is sub-microsecond; a line's SPI flush is ~13 µs+), but the
   **allocation count scales with dirty-line count**, which is exactly what
   item 2 (repaint-scope) controls — the two suspects are NOT independent.

2. **CONFIRMED as the real UI↔radio contention mechanism, but the "size of
   the problem" is unconfirmed.** `ui.step()` runs once per dispatcher-loop
   iteration, AFTER the RX poll and CAD/TX block (`firmware/src/main.rs:1564`,
   inside the loop starting at `:1216`). Its SPI-hold time (paid via
   `flush_line_range` calls) extends that iteration's wall-clock length,
   which is subtracted directly from how promptly the NEXT iteration's CAD
   attempt and RX poll run. §4's formula gives a worst-case ~3.1 ms floor
   for a full 240-line repaint (excluding per-line command overhead, which
   real hardware alone can quantify). Static analysis of the theme motifs
   (below) suggests actual per-frame dirty-line counts are usually much
   smaller than 240 — **on-hardware measurement (§8, checklist item 3) is
   needed to confirm the real distribution**, not assumed.

3. **RE-RANKED — softened for the static backdrop, but MEASURED-CONFIRMED
   as real for `RocketOnSend`.** Static audit of `firmware/src/ui/
   motifs.slint`: `Starfield` (the header backdrop) is a **static** `Image`
   component (no `animate` block at all — drawn once on screen entry, never
   re-dirtied on its own); `Twinkle`'s own doc states it fires via "a
   low-frequency Rust-driven timer rather than an infinite `animate`"
   (bounded trigger rate, not continuous); `CometOnNotify` measures small
   (§3b: 14/240 lines peak). **None of these match a naive "full-window
   animated backdrop" failure mode.** `RocketOnSend`, however, does NOT
   soften the same way once actually measured (§3b, added in this pass via
   `ui_sim/src/perf_profile.rs`'s real-renderer rig, not just reading the
   markup): its translate+fade one-shot peaks at 86/240 dirty lines
   (~36% of the frame, ~16.5K px) — its own static footprint (20×24 px) is
   tiny, but Slint's dirty-region union across the moving+fading frame delta
   is not. **Net verdict: the plan's "prime suspect" framing is confirmed
   for `RocketOnSend`-shaped animations (translate + fade together) and
   softened for the rest** (static backdrop, bounded-rate `Twinkle`, small
   `CometOnNotify`). `repaint-scope-opt` now has a measured, non-trivial,
   named target rather than an unconfirmed assumption either way.

4. **DEMOTED from "hot" to "cold" — `build_message_items`/`render_mentions`
   allocation churn.** §3's real measurement: ~133–157 ns/record, ~75
   bytes/record, called once per conversation open/refresh (NOT per
   `step()`/frame). At any plausible conversation size this is 2-4 orders
   of magnitude below a frame budget. Still worth porting a counting-
   allocator hook onto (§2) as the reusable mechanism `alloc-and-tick-opt`
   needs for the REAL per-frame allocator (item 1), but this specific
   function pair is not itself a meaningful optimization target.

5. **Confirmed non-issue, already fixed — CAD backoff / blocking sleeps.**
   `firmware/src/main.rs:1150-1168`'s own doc: the old
   `FreeRtos::delay_ms(backoff_ms)` full-thread stall on CAD-busy was
   already replaced with a non-blocking deadline (`cad_backoff_until_ms`) by
   a prior change. `run_splash_ripple` (`firmware/src/ui/mod.rs:1107`) is a
   ONE-TIME ~1.15 s boot-only blocking window on its own dedicated render
   loop, already analyzed and accepted in that method's own doc (RX stays
   correct — continuous-RX latching — only a bounded, boot-only polling gap).
   Neither is in scope for this pass.

6. **Confirmed out of scope — radio TX airtime blocking.** §4:
   `radio.transmit()` blocks the dispatcher loop for the full LoRa airtime
   (83–800 ms depending on payload), which is RF physics (half-duplex LoRa),
   not SPI-bus contention with the UI. No UI change can affect this; not a
   UI↔radio contention point.

7. **NEW — confirmed, opposite-direction contention: CAD blocks `ui.step()`,
   not the other way round.** `channel_activity_detection`
   (`firmware/src/radio.rs:415-464`) runs BEFORE `ui.step()` in the
   dispatcher loop (`main.rs:1216` order: CAD+TX → RX poll → `ui.step()`,
   confirmed at `main.rs:1296`/`:1564`) and is itself a blocking call: several
   synchronous SPI command writes (`write_cmd` for `SetStandby`,
   `SetCadParams`, `ClearIrq`, `SetDioIrqParams`, `SetCad`) followed by a
   `DIO1`-watch poll loop with a **20 ms hard deadline**
   (`radio.rs:450-459`). Whenever `txq.has_pending()` and the CAD backoff
   window has elapsed, EVERY dispatcher iteration pays this cost BEFORE
   `ui.step()` gets to run at all that iteration — i.e. the contention this
   analysis originally framed as "UI redraw blocks radio" also runs in
   reverse: **a pending outbound message delays that iteration's touch/
   keyboard poll and render by up to ~20 ms** (plus the handful of SPI
   command round-trips before the poll loop even starts). Out of scope to
   fix here (CAD's 20 ms deadline is an SX1262 hardware/protocol constant,
   not a UI concern) but IN SCOPE to note: `render-radio-contention-opt`'s
   own before/after numbers (§7) should measure BOTH directions — UI dirty-
   line count's effect on CAD/RX-poll cadence, AND CAD-in-flight's effect on
   UI frame latency — not just the one direction the plan named.

## 6. UI↔radio contention map

Shared resource: **SPI2 bus** (LCD ST7789 + SX1262 radio, `firmware/src/ui/
display.rs:1-16`'s pin table). Software-serialised on the single main task —
never truly *concurrent* contention (impossible on one task/one bus), the
contention is **sequential latency injection**: whichever operation runs
first in a loop iteration delays the start of whatever runs after it, within
that iteration, and delays the *next* iteration's operations by the same
amount.

Per-iteration order (`firmware/src/main.rs:1216` loop body): WDT pet → GPS
poll → battery poll → CAD+TX (SPI + a 20 ms `DIO1`-watch deadline when a TX is
pending, §5 item 7) → RX poll (`DIO1` GPIO watch ≤ `RX_POLL_YIELD_MS`=5 ms,
SPI only on packet-found) → `ui.step()` (SPI writes via `flush_line_range`,
cost ∝ dirty-line count this frame — §3b gives measured per-motif counts).

```
[ WDT ] [ GPS ] [ Batt ]
   → [ CAD/TX: SPI cmds + DIO1 watch ≤20ms when TX pending (§5.7) — DELAYS ui.step() THIS iteration ]
   → [ RX poll: DIO1 watch ≤5ms (GPIO, not SPI) ]
   → [ ui.step(): touch/kbd (I2C1, SEPARATE bus) + Slint tick
                  + render_if_needed → flush_line_range × dirty_lines (SPI, §3b: 0/idle,
                    14 lines/CometOnNotify, 86 lines/RocketOnSend, 240 lines/full paint) ]
   → (loop) next iteration's CAD/RX poll starts only after step() returns —
     DELAYS next CAD attempt + RX-poll cadence by THIS iteration's dirty-line cost
```

- **Not contention**: touch (GT911) + keyboard co-processor are on I2C1, a
  physically separate bus from SPI2 — zero bus contention with the radio or
  display, regardless of poll rate.
- **Not contention** (§5 item 6): TX airtime — RF physics, not SPI-bus-hold.
- **Contention direction A (UI → radio, the plan's named direction)**:
  `ui.step()`'s SPI-hold time (from `flush_line_range`) is paid EVERY
  iteration it has dirty lines to flush, and that time is subtracted from
  how soon the loop gets back to the next CAD attempt / RX poll. A busier
  repaint (more dirty lines) → later next CAD attempt → added jitter to when
  a queued TX gets its channel-clear check, and added jitter to RX-poll-to-
  decode latency (correctness is not at risk — `try_receive`'s continuous-RX
  latching means no packet is missed — only *when this task notices it*
  shifts). §3b's measured line counts (0 idle / 14 CometOnNotify / 86
  RocketOnSend / 240 full-paint) × §4's ~13 µs/line data floor give a
  concrete estimate: idle ≈ 0 ms, CometOnNotify ≈ 0.18 ms, RocketOnSend ≈
  1.1 ms, full paint ≈ 3.1 ms — all still far below the 5 ms `RX_POLL_
  YIELD_MS` window, so even the worst measured UI cost this pass found does
  not, by itself, starve a single RX-poll iteration (an on-hardware run
  should still confirm real per-transaction SPI overhead beyond this
  data-only floor — checklist item 3 below).
- **Contention direction B (radio → UI, NEW this pass, §5 item 7)**: CAD's
  own ≤20 ms blocking window runs BEFORE `ui.step()`, so a pending outbound
  message can delay that iteration's render/input-poll by up to ~20 ms —
  four to six times direction A's worst measured UI cost. This is the
  LARGER of the two directions by construction (radio protocol timing vs.
  SPI line-flush cost) and was not named in this analysis's original framing.
- **Bound needing an on-hardware number**: real per-`flush_line_range`-call
  SPI command overhead (CASET/RASET/RAMWR setup, not just data transfer) and
  real dirty-line-count distribution during actual on-device use (idle vs.
  active navigation vs. send-tap) — checklist item 3 below is how an
  operator gets that multiplier; §3b already gives the automatable half of
  it (the multiplier itself, per motif).

## 7. Confirm / re-rank vs. the provisional optimization-phase ranking

| Phase | Provisional rank | This baseline's finding | Verdict |
|---|---|---|---|
| `repaint-scope-opt` (Phase 2) | 1 (prime suspect) | §3b (MEASURED, real renderer, added this pass): static backdrop/`Twinkle`/`CometOnNotify` are already small or one-shot-bounded — the naive "full-window animated backdrop" failure mode does not match those. `RocketOnSend`, however, measures 86/240 dirty lines at peak (~36% of frame) — materially bigger than its own static footprint, a real and now-QUANTIFIED target. | **CONFIRMED for `RocketOnSend`-shaped (translate+fade) one-shots, SOFTENED for the rest.** Keep as Phase 2; scope the win specifically at translate+fade motifs (start from `RocketOnSend`, audit any sibling built the same way) rather than the backdrop/header layer, which §3b shows is already near-optimal. |
| `alloc-and-tick-opt` (Phase 4) | 2 | `build_message_items`/`render_mentions` (the original named example) measured cheap and infrequent (§3/§5.4) — DEMOTE that specific pair. A NEW, more concrete per-frame alloc site was found: `platform.rs:277`'s per-dirty-line `Vec` (§5.1), directly coupled to Phase 2's dirty-line count (§3b now gives real per-motif line counts to multiply against). | **RE-SCOPE, not re-rank.** Point this phase's alloc-and-tick work at `platform.rs::process_line`'s per-line `Vec` allocation (and any other unconditional per-tick recompute found once instrumented) instead of the render-logic functions originally named as the example; re-baseline its allocation count AFTER Phase 2 lands (serial dependency already planned) since Phase 2 changes the very count this phase would optimize. |
| `render-radio-contention-opt` (Phase 6) | 3 | CONFIRMED, and WIDENED this pass: the original analysis named one direction (UI SPI-hold delaying the next CAD/RX-poll, §6 direction A — §3b + §4 now bound it at ≈0–3.1 ms depending on dirty-line count). §5 item 7 (NEW) confirms the OPPOSITE direction is real too and larger: CAD's own ≤20 ms blocking window (`radio.rs:450-459`) runs BEFORE `ui.step()` and delays that iteration's render/input-poll whenever a TX is pending. | **CONFIRMED, rank unchanged, SCOPE WIDENED.** Its own before-numbers should measure BOTH directions on-hardware: `ui.step()` wall-clock under representative dirty-line loads (direction A) AND CAD-in-flight's added latency to that same iteration's render/input responsiveness (direction B) — checklist item 6b below. |

Abort condition check: this baseline does NOT itself abort the work — a
reproducible ANALYTICAL bound exists (§4's airtime/SPI-floor formulas,
verifiable against the wire protocol's own locked SF7/BW62.5kHz preset and
this crate's timed host numbers), a MEASURED redraw-scope baseline exists
(§3b, real Slint renderer, not just formulas), and the on-hardware capture
protocol (§8) is the concrete path to the still-missing REAL numbers (SPI
per-transaction overhead beyond the data-only floor, and real dirty-line
distribution/CAD frequency under actual use). Per this phase's own abort
clause, later optimization phases should not release past their own gate
without that on-hardware confirmation existing at least for the metric they
touch.

## 8. On-hardware capture protocol (runnable checklist)

Run once now (Phase-1 baseline) and once after each optimization phase lands
(its own before/after). Needs: a flashed T-Deck Plus (or the branch under
test), a stopwatch/phone slow-mo camera for felt-frame timing, and — for the
radio numbers — a second MeshCore-speaking node (HIL peer, per
`docs/hil-real-mesh-procedure.md`) to exchange traffic with.

**A. Felt snappiness / frame rate per transition**
1. [ ] Cold boot → note wall-clock from backlight-on to boot splash first
       frame, and splash-dismiss to ContactList first frame (two numbers).
2. [ ] From ContactList, tap into a conversation with ~20+ messages (an
       animated motif, e.g. `CometOnNotify`, should NOT be actively firing
       during this specific tap — isolate the nav-only cost first) — time
       tap-to-first-frame.
3. [ ] Repeat step 2 immediately after a message arrives (so `CometOnNotify`
       or the unread-badge motif IS active) — time tap-to-first-frame again;
       compare against step 2's number (isolates the motif's added cost).
4. [ ] Compose → Send → note tap-to-`RocketOnSend`-first-frame and
       `RocketOnSend`-first-frame-to-MessageView-return timing (the deferred-
       nav window is `SEND_NAV_DEFER_MS`, already fixed at a known constant
       in code — confirm the felt animation completes before the screen
       swaps, no visible pop/skip).
5. [ ] Record a slow-motion (120/240 fps) video of one full screen
       transition and one motif firing; count frames between input and
       first visible change, and between animation start/settle, for a real
       frame-rate number (not just "felt smooth/choppy").

**B. Radio operation timing UNDER UI load**
6. [ ] With the HIL peer, send 20 DMs back-to-back from the peer to the
       T-Deck WHILE idly navigating the T-Deck UI (tap between ContactList/
       MessageView every few seconds) — log CAD-busy count, TX-retry count
       (`main.rs`'s `log::debug!`/`log::warn!` CAD/TX lines), and RxDone
       timestamps from the serial console. Compare against the same 20-DM
       exchange with the T-Deck UI fully idle (screen asleep, no taps).
7. [ ] Compute: mean/max gap between a physical DM send (peer's own send
       timestamp, if loggable) and this device's `RX RxDone` log line, for
       both the "UI active" and "UI idle" runs in step 6. A gap that grows
       materially under UI load is the regression bound this project's
       radio-no-regression criterion protects.
8. [ ] Trigger a TX (send a DM from the T-Deck) WHILE a screen transition or
       motif animation is mid-flight; confirm the CAD/TX log lines show no
       new error class and the transmitted DM is received correctly by the
       peer (correctness, not just timing).
9. [ ] Repeat steps 6-8 with the display fully static (no navigation, no
       motifs firing) as the CONTROL baseline every optimization phase
       diffs against.
6b. [ ] NEW (§5 item 7, opposite direction): with a DM queued for TX
       (`txq.has_pending()`), record UI-input-to-first-frame latency (repeat
       step 2's tap-timing) for taps that land WHILE a CAD attempt is in
       flight vs. taps with no TX pending at all. §5 item 7's analytical
       bound is ≤20 ms added latency per affected tap; this step gets the
       real on-hardware number to confirm or revise that bound.

**C. Recording**
10. [ ] Paste the raw serial console log excerpt (CAD/TX/RX lines +
        timestamps) for steps 6-9 into the relevant record for the change
        under test, plus the slow-mo frame counts from step 5. These are the
        REAL on-hardware numbers §4's formulas are estimating; once
        captured, replace/annotate §4 accordingly (or the first
        optimization phase's own record, whichever runs first).

## 9. Status

Automatable half: **DONE** (harness committed, numbers captured — §2/§3 for
pure render-logic, §3b for measured redraw-scope via the real Slint
renderer). On-hardware half: **PENDING** — protocol authored (§8, including
the new §5-item-7 CAD-direction capture at 6b), not yet run (no on-hardware
session in this phase's scope). Per this phase's own abort clause, this is a
PAUSE-not-abort: a reproducible ANALYTICAL bound exists for radio timing
(§4), a MEASURED redraw-scope baseline exists (§3b), and the mechanism is
confirmed BOTH directions (§5 items 2/7, §6), but a later optimization phase
should not claim its own radio-no-regression checkpoint is satisfiable
without an operator running §8 for at least the metric that phase touches.

## 10. Follow-on: screen-entry FADE repaint cost

§3b/§7 measured `RocketOnSend`'s translate+fade one-shot and (mistakenly, see
below) treated it as the fade "prime suspect." Re-investigating this pass
found the ACTUAL prime suspect one level up: the screen-entry fade
(`reveal_opacity`/`content_opacity`) every themed screen wraps its whole
content in (`contact_list.rs`, `message_view.rs`, `pin_entry.rs`,
`gps_status.rs`, `admin_menu.rs`, `unprovisioned.rs`, and — scoped to its
emoji-picker overlay — `compose.rs`).

**Mechanism (confirmed against `i-slint-core`'s own dirty-region source,
`partial_renderer.rs::compute_dirty_regions`):** when an item's `opacity`
value changes, Slint marks `must_refresh_children` for that whole subtree —
"this will impact all the children ... regardless if they are themselves
dirty or not" (that file's own comment). A near-full-window `VerticalLayout {
opacity: content_opacity; ... }` therefore re-dirties its ENTIRE bounding
region on every tick the fade is still interpolating, not just once at
navigation. `UiRuntime::step()` runs once per dispatcher-loop iteration,
which idles at ~`RX_POLL_YIELD_MS` (5 ms, ~200 Hz) cadence when nothing else
is pending — so an unthrottled render call flushes the fade's full region on
nearly every one of those iterations for the whole 200 ms transition.

**Measured (`ui_perf/tests/entry_fade_repaint.rs`, deterministic clock, real
renderer, `EntryFadeScene` — a `Starfield` backdrop + `content_opacity`-wrapped
header+rows, the exact idiom every real screen uses):** at 5 ms tick cadence,
one 200 ms screen-entry fade costs **40 full-window (240/240-line) flushes**,
not one.

**RocketOnSend re-examined:** isolating it from `ui_sim`'s shared
measurement scene (which also carries `MascotBob`/`Twinkle`, unused by any
real screen `RocketOnSend`/`CometOnNotify` actually ship on) showed its own
translate+fade footprint peaks at ~28 lines / 20px wide over its 400ms
one-shot — §3b's 86-line/200px-wide number was a MEASUREMENT ARTIFACT: the
shared scene's `MascotBob`(450ms)/`Twinkle`(900ms) entry-settle animations
were still in flight (real-wall-clock overlap with the harness's sleep-based
ticking) when `RocketOnSend` fired, and `DirtyRegion`'s 3-rectangle cap
(`i-slint-renderer-software`'s `PHYSICAL_REGION_MAX_SIZE = 3`) merged their
unrelated dirty rects into one inflated bounding box. Neither `compose.rs`
nor `message_view.rs` (the only two real `RocketOnSend` consumers) import
`MascotBob`/`Twinkle` at all, so this never occurs in production — §3b/§7's
"prime suspect" framing for `RocketOnSend` itself is **retracted**; the
correct target was the screen-entry fade documented above.

**Fix (`firmware/src/ui/mod.rs::UiRuntime::step()` +
`firmware/src/ui/platform.rs::TDeckWindowAdapter::has_active_animations`):** a
render-cadence throttle, `RENDER_MIN_INTERVAL_MS = 16` (~60 fps, matching
`SPLASH_RIPPLE_TICK_MS`'s established "comfortably finer than the eye can
distinguish" precedent). `slint::platform::update_timers_and_animations()`
still runs every dispatcher iteration unconditionally (Slint's own `Timer{}`
callbacks and every animated property's value stay exactly on their
wall-clock curve, undelayed) — only the act of actually flushing a frame to
the display (`render_if_needed`, the SPI-bus-contending part) is capped, and
ONLY while `Window::has_active_animations()` reports an animation still
settling from the last actual render. A fresh one-off redraw (navigation,
incoming message, model update — `has_active_animations()` reads `false` for
those) always renders on the very next tick, uncapped: tap-to-first-frame
timeliness (`docs/perf/ui-perf-baseline.md` §8.A) is untouched by this
change.

**Measured effect (`ui_perf/tests/entry_fade_repaint.rs`, same scene, same
5 ms tick cadence, throttle applied):** the 200 ms fade drops from 40
full-window flushes to **11** — a 72% reduction — while the FINAL settled
framebuffer is asserted bit-for-bit IDENTICAL (FNV-1a hash) between the
throttled and unthrottled runs, proving the "identical visual result" hard
constraint: the fade's curve, duration, easing, and end state are completely
unchanged; only how many times an already-identical curve gets sampled and
pushed over SPI changes. Since `RENDER_MIN_INTERVAL_MS` only ever makes
`render_if_needed` calls LESS frequent, radio timeliness (CAD/RX-poll
cadence, §5/§6) can only improve or stay flat, never regress — this needs
on-hardware confirmation (§8) same as every other item on this ledger, but
has no plausible regression direction to check against, only a magnitude.

Reproduce: `cargo test -p ui_perf --test entry_fade_repaint -- --nocapture`.
