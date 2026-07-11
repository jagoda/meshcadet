# `ui_perf` — host-native REPAINT-SCOPE measurement rig

The "prime lever" optimization rig for the UI-performance work.

## Why this crate exists

The UI performance work needs "snappy" and "radio not regressed" to be
**measured, not vibed**. The firmware crate cross-compiles for
`xtensa-esp32s3-espidf` and cannot run on the host or in CI (see
`firmware/Cargo.toml` / `ui_sim/README.md`), so frame-time and radio-op timing
can only be captured **on hardware**. But the *cause* of both choppiness and
radio contention — how many display lines the Slint renderer flushes over the
shared SPI2 bus per frame — is a pure function of the render markup and the
dirty-region renderer, and **that is measurable on the host**.

This crate renders the REAL `firmware/src/ui/motifs.slint` animations, and a
message-view-shaped backdrop+list scene, through the IDENTICAL Slint
`SoftwareRenderer` in `RepaintBufferType::ReusedBuffer` mode the firmware's
`ui/platform.rs` drives on-target, and reports the exact dirty `PhysicalRegion`
the renderer would flush per frame.

### What the numbers mean (and why they're faithful)

The firmware's `TDeckWindowAdapter::render_if_needed` calls
`window.draw_if_needed(|r| r.render_by_line(..))`; `render_by_line` walks the
SAME per-frame dirty region `render(buffer, stride)` returns here and issues one
`flush_line_range` SPI transaction per dirty scanline. So:

- **lines-flushed / frame** = distinct dirty scanlines = number of
  `flush_line_range` SPI window-set+write cycles the firmware pays that frame =
  the SPI-hold count competing with the SX1262 radio on SPI2.
- **dirty pixels / frame** = pixels recomposited + shipped.
- **bbox** = repaint-scope bounding box.

Frame-time (ms) and radio-op timing are NOT measured here — they need the real
ESP32-S3 CPU + SPI2 bus. This rig quantifies the *cause* (flush scope); an
on-hardware capture confirms the *effect*.

Determinism: the rig's `Platform` returns a manually advanced clock (not
wall-clock), so a given animation always samples the same frames run-to-run.

## Reproducing the evidence

```sh
cargo test -p ui_perf -- --nocapture
```

### `tests/motif_repaint.rs` — foreground motif over a static backdrop

Fires the `CometOnNotify` sweep over the static space backdrop + starfield.

```
[motif] settle (navigation full paint): 240 lines, 76800 px, bbox 320x240
[motif] comet sweep: 39 animated frames, worst-frame 14 lines flushed, tallest bbox 14px
```

**Finding:** the space theme has **no full-window-animated-backdrop problem**.
The backdrop layers are static; only the foreground motif animates, and
ReusedBuffer scopes each animation frame's flush to the motif's motion band
(≤14 of 240 lines). The initial "prime-lever = full-window backdrop"
hypothesis is **DEMOTED by measurement** — animation repaint scope is already
near-minimal.

### `tests/model_update_repaint.rs` — live message-list update

The real repaint-scope waste was on the message-view live-update path. Under
ReusedBuffer, a **wholesale `VecModel` replace** (what `set_messages` did) makes
the renderer conservatively invalidate the WHOLE window; an **in-place update**
(push the new row / `set_row_data` a changed row) dirties only the changed rows.

```
[model] settle (navigation full paint): 240 lines
[model] live append, IN-PLACE (fixed): 22 lines, bbox 304x22
[model] live ack-flip, IN-PLACE single-row edit: 22 lines, bbox 304x22
[model] same state, WHOLESALE REPLACE (old firmware): 240 lines
[model] REPAINT-SCOPE WIN: 22 vs 240 — 218 fewer SPI line-flush cycles/message
        (90% reduction); static backdrop + header NOT re-flushed
```

The test also asserts **PARITY** — the in-place-updated framebuffer is
pixel-for-pixel identical (FNV-1a hash) to a wholesale-replace render of the
same final state — and **BEHAVIOR-SAFETY** — the in-place updates self-dirty and
paint with no `request_redraw()`, so nothing stalls.

## The shipped optimization

`firmware/src/ui/screens/message_view.rs::set_messages` now holds a **persistent
`VecModel`** and reconciles it in place (push new rows, `set_row_data` changed
rows, leave unchanged rows untouched) instead of replacing it wholesale on every
call. Pure flush-scope change: identical final model content, identical pixels.
On every incoming message to the open conversation, the flush drops from a
full-window 240-line repaint to a ~22-line scoped one — 90% fewer SPI holds
contending with the radio exactly during message traffic.

`request_redraw()` on the live-update paths was verified NOT to force a
full-window flush under ReusedBuffer (a scoped change + `request_redraw` still
flushes only the scoped region), so it was left untouched — the model-update
strategy is the whole lever.

## HW-gated (not measurable here — needs an on-hardware capture)

- Real frame time (ms) per transition and felt snappiness.
- Radio op timing (CAD latency, TX jitter, RX poll-to-decode) under UI load —
  the no-regression bound. This change strictly *reduces* SPI-bus hold time
  per message, so the expectation is no-regression-or-better; an on-hardware
  checkpoint verifies it.

## Env requirements

Same as `ui_sim`: `SLINT_EMBED_TEXTURES=1` and
`SLINT_EMBED_RESOURCES=embed-for-software-renderer` must be process env vars when
the `slint!{}` macro expands. The repo-root `.cargo/config.toml` sets both, and a
root `cargo test` (CWD = repo root) reads that file. See that file's comment.
