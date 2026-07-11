# ADR-0003 — UI Toolkit: Slint with SoftwareRenderer

- **Status:** Accepted (2026-06-13)
- **Deciders:** Maintainer evaluation (Slint chosen)
- **Supersedes:** —
- **Implements:** ADR-0001 §3 (UI — touch-first, icon/image-rich; toolkit choice was a
  go/no-go gate during the touch-UI build-out)
- **Code:** `firmware/src/ui/` (platform backend, screen modules, notification)

## Context

MeshCadet needs a full interactive UI on the T-Deck Plus's 320×240 capacitive
touchscreen for these surfaces:

- **First-boot / unprovisioned** — prompt the admin to connect via USB
- **Contact / channel list** — select a conversation to open
- **Message view** — read a conversation thread with emoji rendered inline
- **Compose** — draft a message; Slack-style `:shortcode:` emoji entry over a
  curated set
- **PIN entry** — 4-digit numeric pad; shared surface with the PIN-menu-and-history
  work (that work owns the menu logic; this ADR's UI toolkit owns the pad widget)
- **Notifications** — visual (screen flash) + audible (PWM buzzer) per-event

Constraints:
- Target: ESP32-S3 / xtensa-esp32s3-espidf, `std` runtime via esp-idf-svc
- Rust-first: the whole codebase is Rust; C FFI adds maintenance burden
- No heap pressure concern (ESP32-S3 with PSRAM); real-time radio loop runs
  cooperatively in the same thread — UI must not block
- Final acceptance is HIL (real hardware), not an emulator

Three toolkit candidates were evaluated:

| Toolkit | Maturity | Rust-native | Widget set | T-Deck ESP32 support |
|---------|----------|-------------|------------|----------------------|
| **Slint** | v1.x, maturing embedded | ✅ pure Rust | declarative, composable | Via SoftwareRenderer + mipidsi |
| **LVGL** | battle-tested, widest C ecosystem | ❌ C + unsafe FFI (`lvgl-rs` bindings) | richest, proven on LilyGo | Official ESP-IDF port, proven |
| **embedded-graphics** | stable, minimal | ✅ pure Rust | primitives only; no widget layer | ✅ native; used here as the flush sink |

## Decision

**Chosen: Slint v1.x with SoftwareRenderer → mipidsi ST7789 on SPI2**

Architecture:

```
┌──────────────────────┐
│  Slint slint!{} DSL  │  ← screen definitions (compile-time)
├──────────────────────┤
│  SoftwareRenderer    │  ← Slint render pass → RGB565 line buffer
├──────────────────────┤
│  mipidsi ST7789      │  ← SPI flush per dirty line strip
├──────────────────────┤
│  esp-idf-hal SPI     │  ← hardware driver
└──────────────────────┘

┌───────────────────────┐
│  GT911 I2C touch      │  ← polled in main loop (I2C1, SDA=GPIO18, SCL=GPIO8)
└───────────────────────┘

┌───────────────────────┐
│  I2S speaker           │  ← notification audio (WS=GPIO5, BCK=GPIO7, DOUT=GPIO6)
└───────────────────────┘
```

### Why Slint over LVGL

LVGL is the obvious practical choice for LilyGo hardware — there are working examples
and the LilyGo SDK ships LVGL configs. But the `lvgl-rs` Rust binding crate has
significant maintenance lag, requires `unsafe` throughout widget callbacks, and the
C event model is awkward to drive from Rust closures. Every widget touch callback
crosses the FFI boundary. For a project that is explicitly Rust-first, this adds
permanent friction and a hidden unsafe surface.

Slint provides:
- A safe, typed Rust API for every property and callback
- Compile-time verification of the UI model via the `slint!{}` macro
- `SoftwareRenderer` that targets any `DrawTarget` implementing display (including
  `mipidsi`) — no custom C driver needed
- `slint::platform::update_timers_and_animations()` fits the cooperative loop model;
  Slint does NOT require its own thread

The one-time cost is a custom `Platform` impl (~150 LOC in `platform.rs`) that wires
`Duration` via `esp_timer_get_time`, `create_window_adapter` to our `TDeckWindow`, and
optionally `run_event_loop` (unused — we drive the loop ourselves).

### Why not embedded-graphics alone

embedded-graphics provides drawing primitives (pixels, shapes, text, images) but NO
widget layer. Building a scrollable conversation list, emoji picker grid, and PIN
numpad from raw primitives would require a bespoke widget framework of equivalent
complexity to Slint's. embedded-graphics is used here as the **rendering sink** for
Slint's SoftwareRenderer (via `mipidsi`'s `DrawTarget` impl), not as the UI toolkit.

### Emoji rendering

Slint's default font does not cover the full Unicode emoji range. We ship the
**Noto Emoji** subset covering only our curated 40-character set, compiled into the
binary via `slint`'s font embedding. Shortcode-to-codepoint mapping lives in
`protocol/src/emoji.rs` (shared between firmware and host CLI).

On the wire, emoji travel as UTF-8 code points — no shortcode escaping at the wire
level. The shortcode `:smile:` → `'\u{1F60A}'` expansion happens at compose time on
the sending device and at display time on the receiving device (render Unicode
directly from the stored UTF-8 string).

### Cooperative integration with the radio loop

Slint does not spin its own thread on embedded. Integration in `firmware/src/main.rs`:

```
loop {
    gps.poll(now);
    txq / CAD / TX logic ...;
    radio.try_receive(...);

    // ── UI step (non-blocking) ─────────────────────────
    slint::platform::update_timers_and_animations();
    if ui_window.has_active_animations() || needs_redraw {
        ui_window.render_frame(&mut display);      // flush dirty lines
    }
    touch.poll_events(|e| ui_window.dispatch_event(e));
    // ──────────────────────────────────────────────────
}
```

Each pass costs ≤1 ms when no redraw is needed (timer check + touch I2C poll).
A full 320×240 frame flush via SPI at 40 MHz takes ~15 ms; dirty-region rendering
keeps this under 3 ms for typical partial updates.

## T-Deck Plus Pin Assignments (UI peripherals)

| Signal | GPIO | Notes |
|--------|------|-------|
| LCD CS | 12 | SPI2 shared with radio (GPIO9); separate CS |
| LCD DC | 11 | Data/Command |
| LCD RST | 16 | Display reset (radio RST = 17) |
| LCD BL | 42 | Backlight enable (active high) |
| LCD SCK/MOSI/MISO | 40/41/38 | Shared SPI2 bus |
| Touch SDA | 18 | I2C1 |
| Touch SCL | 8 | I2C1 |
| Touch INT | 3 | GT911 interrupt (polled, not wired to ISR) |
| Speaker (I2S) | 5 / 7 / 6 | WS (LRCK) / BCK / DOUT — I2S0, std/Philips mode |

> **CORRECTION (2026-07-03, notification-audio hardware audit):** this
> table previously listed "Buzzer | 46 | LEDC PWM channel 0, timer 0". That
> hardware does not exist — GPIO46 is `BOARD_KEYBOARD_INT` per LilyGo's own
> `utilities.h`, not a buzzer, and there is no discrete piezo on this board.
> The corrected row above (I2S speaker on GPIO 5/7/6) is confirmed against
> LilyGo's own `SimpleTone.ino` example, the upstream `meshcore-dev/MeshCore`
> firmware (which defines no `PIN_BUZZER` for this board), and the shipped
> MCTerm companion firmware (`dabeani/meshcoreterm`, which logs its mechanism
> as "T-Deck I2S buzzer"). See `firmware/src/ui/mod.rs`'s `BuzzerDriver` doc
> for the implementation.

> **Note:** Pin assignments reflect LilyGo T-Deck Plus v1.1 schematics as of
> 2026-Q1.  Verify against the schematic before first flash if using a different
> hardware revision.

## Alternatives Considered

### A. LVGL via `lvgl-rs`

Rejected: unsafe FFI throughout, C-style event model, binding crate maintenance
lag. Would work, but at the cost of Rust-first purity and ongoing unsafe surface.

### B. embedded-graphics widget layer (e.g. `embedded-graphics-ui`)

Third-party widget layers for embedded-graphics are experimental and lack the
composable layout engine Slint provides. Building the full screen set from
primitives would replicate most of Slint's widget work.

### C. Slint with no-std (embassy / bare-metal)

The embassy Slint MCU demos require embassy-executor and a no-std HAL. This
firmware uses esp-idf-svc (`std`), which provides FreeRTOS + ESP-IDF bindings.
Mixing embassy + esp-idf in the same binary is unsupported. The `std` Slint path
is fully supported and simpler.

## Consequences

- The `platform.rs` custom backend is load-bearing; changes to `esp-idf-hal` SPI
  or I2C APIs may require it to be updated.
- A full 320×240 frame buffer (150 KB RGB565) is too large for internal SRAM;
  the SoftwareRenderer is used in line-buffer mode (`MinimalSoftwareRenderer` with
  a 320-pixel-wide line buffer = 640 B). This constrains rendering to one dirty
  strip at a time, which is sufficient for this UI's update patterns.
- Font binary size: the Noto Emoji subset for 40 glyphs adds ~50 KB to the flash
  image. Acceptable given the 4 MB flash partition.
- HIL acceptance gate: the UI test procedure is appended to
  `docs/hil-real-mesh-procedure.md` under the "Touch UI" section.

## Amendment (2026-07-06) — image-asset pipeline

The outer-space UI theme (v2) introduced the firmware's first
raster-image assets (baked celestial scenery + the "Cadet" mascot bitmap).
This amendment records HOW images are embedded, extending this ADR's
SoftwareRenderer decision rather than superseding it.

**Decision: `Image` + `@image-url(...)` compiled via
`SLINT_EMBED_RESOURCES=embed-for-software-renderer`** (set in
`firmware/.cargo/config.toml`'s `[env]`, same mechanism/timing as the
existing `SLINT_EMBED_TEXTURES` font-embedding var this ADR already
documents). This decodes each referenced PNG (via the `image` crate, HOST
side, at proc-macro expansion time — already pulled in transitively through
`i-slint-compiler`'s `software-renderer`/`image` features once the
`slint-build` build-dependency trick this ADR's font section already uses is
in place; no new Cargo dependency was needed) into raw pixel data baked
directly into the generated component's init code. No runtime PNG decoder
ships on-target, consistent with this ADR's no-heap-pressure /
bare-metal-friendly posture.

**Fallback, proven but not shipped:** a `build.rs`-generated packed-RGB565
byte array (same shape as `gen_emoji_font.c`'s font-generation pipeline) fed
to a runtime `SharedPixelBuffer<Rgb8Pixel>` + `slint::Image::from_rgb8`. The
primary path built and rendered correctly (host-sim AND the real
`xtensa-esp32s3-espidf` cross-compile target), so this fallback was not
required in production; it is proven end-to-end in the host-native `ui_sim`
crate (repo root) as the documented, exercised de-risking evidence the
plan required before committing to the primary path across the
later 7-screen fan-out.

**Asset source + reproducibility:** `firmware/assets/space/generate_assets.py`
(stdlib-only: `zlib` + `struct`, no Pillow) deterministically generates the
walking-skeleton's three PNGs (Cadet idle pose, starfield strip, corner
planet) in the widened space palette (`ui/theme.slint`'s new tokens). This is
a placeholder asset set sized to prove the pipeline; later
work replaces/extends it with the full illustrated set.

**Measured flash cost:** adding the widened palette + these three bitmaps to
`unprovisioned.rs` moved the release image's flash-resident size (`.text` +
`.data`) from 2,441,181 to 2,488,817 bytes — a **+47,636 byte (~46.5 KiB)**
delta, comfortably inside the 3.75 MB partition headroom `partitions.csv`
documents.

**Host-sim as the per-child verification substitute:** since `firmware/`
cannot be exercised by a host-native `cargo test` (see this ADR's existing
"Consequences" + `firmware/Cargo.toml`'s doc), `ui_sim` is the host-side
proof that a given `slint!{}` image-embedding change actually renders
correct pixels, without requiring the on-hardware step this project
reserves for its single final operator-run acceptance test. See `ui_sim/README.md` for
the full design and reproduction steps.

## Amendment (2026-07-06) — shared motif library (M2)

Per M1's amendment above ("later work replaces/extends it with the full
illustrated set"), this amendment adds:

- **Assets** (`firmware/assets/space/generate_assets.py`, extended not
  forked — M1's three functions/PNGs are byte-identical, unmodified): the
  full celestial set (`crescent_moon.png`, `comet.png`, alongside M1's
  `starfield.png`/`planet_corner.png`), a `rocket.png` for the motion
  language's rocket-on-send helper, and the four remaining Cadet mascot
  poses (`cadet_wave.png`, `cadet_thumbsup.png`, `cadet_sleeping.png`,
  `cadet_peeking.png`, alongside M1's `cadet_idle.png`).
- **Shared component contract** (`firmware/src/ui/motifs.slint`, new file,
  same import mechanism `theme.slint`'s `Theme` global already
  establishes): thin `Image`-inheriting wrappers for every static asset/pose,
  plus the four one-shot motion helpers the design names —
  `MascotBob` and `Twinkle` (self-contained, fire once on `init`, same
  never-an-infinite-loop rule this ADR's UI toolkit decision and
  `unprovisioned.rs`'s module doc both establish) and `RocketOnSend` /
  `CometOnNotify` (retriggerable one-shots driven by an `in property <bool>
  play` a consuming screen binds to its own Rust-settable trigger property).
- **Not yet consumed in production**: no screen imports `motifs.slint` in
  this landing — `unprovisioned.rs` (M1's pilot, excluded from the 7-screen
  fan-out per the design plan) is deliberately left untouched, and the 7
  fan-out screens that DO consume this module are separate, later landings.
  This landing's own verification is `ui_sim::motif_library` (see
  `ui_sim/README.md`'s "Motif-library render rig" section) — a second,
  independent host-sim render path (separate `Platform` install, separate
  `tests/motif_library.rs` integration-test binary) that renders every
  static asset and both rest AND fired/settled states of every motion
  helper, asserted pixel-by-pixel, plus a checked-in render
  (`docs/renders/motif-library-host-sim.png`).
- **Flash impact:** none measured for this landing — `motifs.slint` and the
  7 new PNGs are unreferenced by any `firmware/` screen module, so the
  `xtensa-esp32s3-espidf` release build's flash-resident size is unchanged
  by this commit (confirmed: `cargo build --release` in `firmware/` stays
  green). Each fan-out screen's own landing measures its own delta as it
  starts consuming specific components from this library.
