# Residual UI-side optimization — R1 re-ranking (M3)

**Mission:** `meshcadet-perf-ui-residual-opt` — M3 of the
`meshcadet-perf-rearchitecture` campaign
(campaign plan §M3, §6 criterion 6).
**Date:** 2026-08-03.
**Outcome: NO optimization landed. Both candidate items are demoted, on
fresh numbers and a source-level argument.** This document is the record of
why — the milestone's charter is explicit that a documented no-op is a valid
landing and that nothing may be optimized on the strength of the superseded
pre-split ranking.

---

## 0. Provenance legend

Same tags as `docs/perf/ui-perf-baseline.md` §0, which governs. Every
quantity below carries exactly one.

| Tag | Meaning |
|---|---|
| **[HOST]** | Really executed on an x86-64 host by a committed test in this repo, re-run for this mission. |
| **[ANALYTICAL]** | Computed from an in-repo formula or datasheet constant. |
| **[SIM]** | Produced by `perf_loop_model`'s host discrete-event model. Never a device measurement. |
| **[ESTIMATE]** | A projection combining tagged inputs, or a reasoned bound. |
| **[SOURCE]** | A claim settled by reading the code as it stands, cited to `path:line`. Not a measurement. |
| **[DEFERRED-DEVICE]** | Not measured; requires the T-Deck. Enumerated in `ui-perf-baseline.md` §8. |

No number in this document is device-measured, and none is presented as
though it were.

---

## 1. What M3 was asked to re-rank

Two survivors from the pre-split UI ledger:

- **(a)** `firmware/src/ui/platform.rs::process_line`'s per-dirty-line heap
  `Vec<Rgb565>` allocation.
- **(b)** Translate + fade repaint scope — the screen-entry `opacity` fade's
  full-window re-dirty, partly addressed by the landed
  `RENDER_MIN_INTERVAL_MS` render-cadence throttle.

Both measured **0.18–3.1 ms** in the pre-split record, against a structural
item (`radio.transmit()` blocking the one shared task for full LoRa airtime)
of **83–800 ms** [ANALYTICAL]. The charter: re-rank against the post-split
numbers and implement only what those still justify.

## 2. Method

Re-ran the two host instruments against the current tree — post-M1
(`meshcadet-perf-ui-task-split`, ADR-0012) and post-M2
(`meshcadet-perf-radio-dio1-interrupt`):

```sh
export SLINT_DEFAULT_FONT=/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf
cargo test -p ui_perf --tests -- --nocapture
cargo test -p ui_sim  --test perf_profile -- --nocapture
```

The post-split comparison baseline is `docs/perf/task-split-host-validation.md`
§2.2/§2.3 [SIM] and `docs/perf/spi2-arbitration-r1.md` Q5 [SOURCE].

## 3. Item (a) — per-dirty-line `Vec<Rgb565>`: CLOSED, not demoted

**The allocation does not exist.** `firmware/src/ui/platform.rs:260`'s
`process_line` renders into a stack `[Rgb565Pixel; DISPLAY_WIDTH]` and passes
a lazy `.map(..)` iterator to `TDeckDisplay::flush_line_range`, which streams
into `mipidsi::fill_contiguous`. There is no intermediate heap buffer on any
path.

Re-run verbatim, this mission [HOST]:

```
[flush-alloc] per-frame allocation projection (old vs. new flush path):
  idle (no dirty lines)            lines=0    old_allocs=0     new_allocs=0
  CometOnNotify peak frame         lines=14   old_allocs=14    new_allocs=0
  live message append (in-place)   lines=22   old_allocs=22    new_allocs=0
  RocketOnSend peak frame          lines=28   old_allocs=28    new_allocs=0
  full-window navigation paint     lines=240  old_allocs=240   new_allocs=0
```

`ui_perf/tests/flush_line_alloc.rs` additionally asserts the two paths emit
byte-identical pixels, so the reduction carries no visual cost.

**Ranking: not applicable — the item is closed at zero.** An item measured at
zero cannot be optimized further, and the number is pinned by a committed
test that fails if it ever regresses. This is not the charter's "beneath
notice after M1" outcome; it is "already landed before M3 opened", recorded
in `ui-perf-baseline.md` §3.4 and §9's last bullet. **No work is possible
here.**

## 4. Item (b) — fade repaint scope: DEMOTED, on three independent grounds

The repaint-scope numbers themselves are unchanged post-split [HOST], re-run
this mission:

```
frame0 (initial full paint)        lines = 240/240  px = 76800  widest = 320
frame1 (idle, no property change)  lines =   0/240  px =     0  widest =   0
RocketOnSend  peak dirty frame     lines =  28/240  px =   560  widest =  20
CometOnNotify peak dirty frame     lines =  14/240  px =   700  widest =  50
[entry-fade] unthrottled: 40 frames rendered, 40 of them full-window (320x240)
[entry-fade] throttled:   11 frames rendered, 11 of them full-window (320x240)
```

What changed is not the repaint scope. It is **who pays for it, and whether
anything can still be done about it.**

### 4.1 The split already reproduces the throttle's cadence cap [SOURCE]

The `40 → 11` win was measured against a `step()` running on the shared
dispatcher loop, whose idle cadence was then `RX_POLL_YIELD_MS` ≈ 5 ms
(~200 Hz) — ~40 render opportunities inside a 200 ms fade.

Post-split, `step()` is called once per `ui_task` loop iteration
(`firmware/src/ui_task.rs:380`), and that loop's ceiling is
`UI_TICK_MS = 16` ms (`ui_task.rs:107`, `evt_rx.recv_timeout`). That is
**exactly** `RENDER_MIN_INTERVAL_MS = 16` ms (`firmware/src/ui/mod.rs:1058`).

So in a quiet steady state the split alone caps the fade at ~12 render
opportunities per 200 ms, and the throttle removes at most one further frame
from that. The headline `40 → 11` is now overwhelmingly attributable to M1,
not to the throttle.

**This is not an argument for removing the throttle** — see §5.

### 4.2 The throttle cannot usefully be tightened: a full-window flush is longer than any cap worth setting [SOURCE] + [ESTIMATE]

The obvious "residual win" would be raising `RENDER_MIN_INTERVAL_MS` above
16 ms so fewer full-window fade frames are flushed. **It is a provable no-op
for exactly the frames it targets.**

`UiRuntime::step(now_ms)` reads `now_ms` once, at the top of the call
(`ui_task.rs:380` passes `crate::uptime_ms()`), and the render block sets
`self.last_render_ms = now_ms` (`ui/mod.rs:2171`) from that same value.
`self.window.render_if_needed(&mut self.display)` on line 2170 **blocks for
the whole flush** it issues. Therefore the interval the throttle predicate
(`ui/mod.rs:2167-2168`) observes on the next tick already *includes* the
previous flush's full duration.

A full-window (240-line) flush's SPI data floor is **~30.7 ms**
([ESTIMATE]: `ui-perf-baseline.md` §4.1's [ANALYTICAL] 128 µs/line ×
[HOST]-measured 240 lines), before the per-line CASET/RASET/RAMWR command
overhead, which is [DEFERRED-DEVICE]. 30.7 ms > 16 ms, and 30.7 ms exceeds
any cadence cap one would plausibly set (33 ms ≈ 30 fps is already at the
edge of visible judder for a screen transition).

**Consequence:** after any full-window paint, `render_due` is already
unconditionally true on the next tick. Raising the constant to anything at or
below the flush's own duration changes nothing about the fade frames it was
meant to suppress; raising it *above* ~31 ms would start dropping frames from
the small, cheap motif animations (`CometOnNotify` at 14 lines ≈ 1.8 ms,
`RocketOnSend` at 28 lines ≈ 3.6 ms) that cost almost nothing and that the
constant was never aimed at. The change has no upside and a visible-judder
downside.

### 4.3 Post-split, the fade's cost is no longer a radio cost — it is ≤12.8 µs, not 30.7 ms [SOURCE]

This is the decisive one, and it is why the item drops out of contention
rather than merely shrinking.

Pre-split, every millisecond `ui.step()` spent flushing was a millisecond the
*same task* could not spend on the next CAD attempt or RX poll. The fade was
a **priority-1** cost (message delivery timeliness) wearing a priority-2
costume.

Post-split, `ui_task` owns the LCD on core 1 and the dispatcher owns the
radio on core 0 (ADR-0012). The only remaining coupling is SPI2, and
`docs/perf/spi2-arbitration-r1.md` Q5 settles the magnitude from ESP-IDF and
`esp-idf-hal` source: the bus is released and re-arbitrated after **every**
elementary transaction, and the largest such transaction on this bus is one
64-byte LCD chunk at 40 MHz. Worst case, a radio SPI command waits

> 64 B × 8 bits / 40 MHz = **12.8 µs** [ANALYTICAL]

behind a full-window fade — not 30.7 ms, and not the ~368 ms of aggregate
bus time a whole fade represents. That is **~2400× smaller** than the
pre-split framing assumed, and it is four orders of magnitude below the
83–800 ms airtime that still dominates the dispatcher task
(`ui-perf-baseline.md` §4.2).

Set against the post-split UI-unserviced gap the campaign actually gates on —
**8.10 ms (mid corner) to 16.20 ms (high corner)** [SIM],
`task-split-host-validation.md` §2.2 — the fade's cost no longer appears in
the radio-timeliness ledger at all.

### 4.4 What would actually reduce the fade's cost, and why M3 is the wrong place for it

The fade's cost is not cadence; it is **scope**. Slint's
`partial_renderer.rs::compute_dirty_regions` marks `must_refresh_children`
for an entire subtree when its `opacity` changes
(`ui-perf-baseline.md` §3.3), so a near-full-window
`VerticalLayout { opacity: content_opacity; … }` re-dirties ~240 lines per
rendered frame no matter how often it renders. Only two things change that:

1. **Narrow the opacity subtree** — fade a smaller region, or drop the
   screen-entry fade. This is a **visual/product change**, and the
   Commander's standing constraint on this campaign is that *all
   functionality must remain, nothing may regress*. It needs a design ruling,
   not a perf mission's judgement.
2. **Composite off a retained framebuffer** instead of re-rendering — 320 ×
   240 × 2 B = **150 KB** [ANALYTICAL] of retained buffer, against a
   line-renderer architecture (`RepaintBufferType::ReusedBuffer` +
   `render_by_line`) chosen precisely to avoid holding one. That is an
   M-sized re-architecture with its own PSRAM-bandwidth question, not an M3
   residual.

Neither is "implement what the fresh numbers justify". Both are new missions
with their own charters, and neither is justified by anything measured here:
the cost they would remove is a priority-2 smoothness cost, on core 1, which
has nothing else to do.

## 5. Explicitly NOT recommended: removing or loosening the throttle

§4.1 shows the throttle's headline win is now largely M1's. That is **not** a
case for deleting it. `ui_task`'s loop is `recv_timeout`-driven
(`ui_task.rs:357`): it wakes on a message *or* the 16 ms ceiling, and
`EVENT_QUEUE_CAP = 32` (`ui_task.rs:112`) means a burst of dispatcher events
can drive up to 32 back-to-back iterations with no tick wait at all. Under
exactly that burst — the case a fade is most likely to coincide with, since
navigation and incoming traffic arrive together — `RENDER_MIN_INTERVAL_MS` is
the **only** thing bounding render cadence. Demotion of the item is not
deletion of the mitigation.

## 6. Source corrections made by this mission (no behaviour change)

The render throttle's own justification comments still argued from the
pre-split cadence — "a shared-loop `step()` running near `RX_POLL_YIELD_MS`
cadence (~5 ms, ~200 Hz)", "once per dispatcher loop iteration". Both facts
are now false twice over: `step()` runs on `ui_task` at a 16 ms ceiling, and
`RX_POLL_YIELD_MS` was itself retuned 5 → 20 ms by
`meshcadet-perf-radio-dio1-interrupt` (`firmware/src/main.rs:1748`).

Leaving them is the exact failure mode `ui-perf-baseline.md` §9 exists to
prevent — the next reader re-derives the superseded ranking from the source's
own comment. This mission corrected the present-tense claims at the sites
that carry M3's subject: `firmware/src/ui/mod.rs` `render_settling` /
`last_render_ms` field docs, `pending_input_ms`'s precision rationale,
`MAX_INPUT_EVENTS_PER_STEP`'s bound rationale, `step()`'s own doc, and the
"Render dirty regions" block. **Comments only — no code, no constant, and no
behaviour changed.**

### Handed to M4's consolidated citation re-pass

`ui-perf-baseline.md` §9 already names the M4 campaign synthesis as where
this document set's citations get one consolidated re-pass. These
`firmware/src/ui/mod.rs` sites are *historical narrative* ("this used to
be…", "ROOT CAUSE this replaces") that reads as present-tense on a skim, and
were deliberately left alone here rather than rewritten piecemeal:
lines **326, 1363, 1376, 1406, 1409, 1450, 1671, 1848, 1943**. They are
factually about the pre-split world and are not wrong as history; they are
listed so M4 has the line numbers rather than a scavenger hunt.

## 7. Verdict

| Item | Pre-split rank | Post-split disposition | Basis |
|---|---|---|---|
| (a) `Vec<Rgb565>` per dirty line | open hotspot, 240 allocs/full paint | **CLOSED at zero** — no work possible | [HOST] `flush_line_alloc.rs`, re-run §3 |
| (b) fade repaint scope, cadence half | open, `RENDER_MIN_INTERVAL_MS` partial fix | **DEMOTED** — tightening the cap is a provable no-op (§4.2); the split already supplies the cap (§4.1) | [SOURCE] `ui_task.rs:107`, `ui/mod.rs:1058/2167-2171` |
| (b) fade repaint scope, scope half | open | **DEMOTED out of this campaign** — the only real fixes are a product-design change or a 150 KB framebuffer re-architecture (§4.4) | [SOURCE] + [ANALYTICAL] |
| (b) as a *radio-timeliness* cost | priority-1 coupling | **RETRACTED** — worst-case radio exposure is 12.8 µs, not 30.7 ms | [SOURCE] `spi2-arbitration-r1.md` Q5 |

**M3 lands a documented no-op on the optimization axis.** No constant was
retuned, no allocation removed, no repaint scope narrowed — because the fresh
numbers justify none of it, and the charter is explicit that inventing work to
justify the milestone is the failure, not the no-op.

## 8. Deferred predicates (unchanged by this mission)

M3 adds no new deferred predicate. The two it leans on are already enumerated
in `ui-perf-baseline.md` §8:

- **D2** — the per-line CASET/RASET/RAMWR command overhead on top of the
  128 µs/line data floor, which sets the true full-window flush duration
  §4.2's argument bounds from below. The argument is *insensitive* to it:
  command overhead can only make the flush longer, which only strengthens
  "the flush already exceeds any cap worth setting".
- **D10** — felt frame rate / tap-to-first-frame for a screen transition,
  which is the only instrument that could contradict §4.4's judgement that
  the residual smoothness cost is not worth a re-architecture.
