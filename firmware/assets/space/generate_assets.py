#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Deterministic, stdlib-only generator for the space theme's art.

M1 authored this file and its first three PNGs (`cadet_idle.png`,
`starfield.png`, `planet_corner.png`) as a walking-skeleton asset set. M2
(this amendment) EXTENDS it — with the full baked celestial
set (crescent moon, comet, rocket) and the remaining Cadet mascot poses
(wave, thumbs-up, sleeping, peeking). The original three M1 functions
(`gen_cadet_idle`, `gen_starfield`, `gen_planet_corner`) are UNTOUCHED below
(byte-identical output — already verified against the landed M1 commit;
this amendment is purely additive) — new poses/motifs get their own sibling
functions rather than a shared-helper refactor of the proven ones, to keep
that guarantee mechanical rather than "trust the diff".

No PIL/Pillow or any other third-party dependency — just `zlib` + `struct`
from the standard library, in the same "small, auditable, host-runnable"
spirit as `firmware/gen_emoji_font.c` (that file's own doc comment). The
PNGs are the *build input*; `Image::from_rgb8`+ `@image-url` in the
`slint::slint!{}` macros / `firmware/src/ui/motifs.slint` consumes them
(either compiled in via `SLINT_EMBED_RESOURCES=embed-for-software-renderer`,
or decoded to a runtime `SharedPixelBuffer` on the build.rs->byte-array
fallback path — see that path's own generator in `ui_sim/build.rs`).

Usage:
    python3 firmware/assets/space/generate_assets.py
"""
import struct
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent

# ── Widened space palette (must match firmware/src/ui/theme.slint exactly —
#    these are literal hex twins of that file's new `out property <color>`
#    tokens, kept here ONLY as generator input; theme.slint remains the single
#    source of truth for what firmware/host code actually consults) ─────────
SPACE_DEEP = (0x07, 0x0A, 0x12)
NEBULA_VIOLET = (0x7C, 0x5C, 0xFF)
NEBULA_VIOLET_DEEP = (0x3A, 0x2A, 0x6B)
PLANET_WARM = (0xE0, 0x8A, 0x4C)
PLANET_WARM_DEEP = (0xA8, 0x5A, 0x2C)
MOON_SILVER = (0xC8, 0xD0, 0xE0)
STAR_GOLD = (0xFF, 0xD6, 0x6B)
STAR_WHITE = (0xF4, 0xF7, 0xFF)
BRAND_SIGNAL = (0x00, 0xB4, 0xFF)
COMET_TEAL = (0x5C, 0xF0, 0xD0)


class Canvas:
    """Minimal RGBA8 pixel buffer with alpha-composited drawing primitives."""

    def __init__(self, w: int, h: int):
        self.w = w
        self.h = h
        self.px = bytearray(w * h * 4)  # transparent black

    def set(self, x: int, y: int, rgb, a: int = 255):
        if x < 0 or y < 0 or x >= self.w or y >= self.h:
            return
        i = (y * self.w + x) * 4
        if a >= 255:
            self.px[i:i + 4] = bytes((*rgb, 255))
            return
        # Simple over-compositing against whatever is already there.
        dr, dg, db, da = self.px[i:i + 4]
        sa = a / 255.0
        nr = int(rgb[0] * sa + dr * (1 - sa))
        ng = int(rgb[1] * sa + dg * (1 - sa))
        nb = int(rgb[2] * sa + db * (1 - sa))
        na = int(255 * sa + da * (1 - sa))
        self.px[i:i + 4] = bytes((nr, ng, nb, na))

    def filled_circle(self, cx: float, cy: float, r: float, rgb, a: int = 255):
        x0, x1 = int(cx - r - 1), int(cx + r + 1)
        y0, y1 = int(cy - r - 1), int(cy + r + 1)
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                if (x - cx) ** 2 + (y - cy) ** 2 <= r * r:
                    self.set(x, y, rgb, a)

    def filled_ellipse(self, cx: float, cy: float, rx: float, ry: float, rgb, a: int = 255):
        x0, x1 = int(cx - rx - 1), int(cx + rx + 1)
        y0, y1 = int(cy - ry - 1), int(cy + ry + 1)
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                nx = (x - cx) / rx
                ny = (y - cy) / ry
                if nx * nx + ny * ny <= 1.0:
                    self.set(x, y, rgb, a)

    def ring(self, cx: float, cy: float, rx: float, ry: float, thickness: float, rgb, a: int = 255):
        """An elliptical ring (annulus) — used for the corner planet's ring."""
        x0, x1 = int(cx - rx - 1), int(cx + rx + 1)
        y0, y1 = int(cy - ry - 1), int(cy + ry + 1)
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                nx = (x - cx) / rx
                ny = (y - cy) / ry
                d = nx * nx + ny * ny
                inner = ((rx - thickness) / rx) ** 2 if rx > thickness else 0
                if inner <= d <= 1.0:
                    self.set(x, y, rgb, a)

    def rect(self, x0: int, y0: int, x1: int, y1: int, rgb, a: int = 255):
        for y in range(y0, y1):
            for x in range(x0, x1):
                self.set(x, y, rgb, a)

    def erase_circle(self, cx: float, cy: float, r: float) -> None:
        """Punch a fully-transparent hole (not alpha-blended) — used to carve
        a crescent out of a filled disc. `set()` always OVER-composites, so a
        dedicated eraser that writes (0,0,0,0) directly is needed here."""
        x0, x1 = int(cx - r - 1), int(cx + r + 1)
        y0, y1 = int(cy - r - 1), int(cy + r + 1)
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                if (x - cx) ** 2 + (y - cy) ** 2 <= r * r:
                    if 0 <= x < self.w and 0 <= y < self.h:
                        i = (y * self.w + x) * 4
                        self.px[i:i + 4] = bytes((0, 0, 0, 0))


def write_png(path: Path, canvas: Canvas) -> None:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    w, h = canvas.w, canvas.h
    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)  # 8-bit RGBA, no interlace

    raw = bytearray()
    for y in range(h):
        raw.append(0)  # filter type 0 (None) per scanline
        row_start = y * w * 4
        raw += canvas.px[row_start:row_start + w * 4]
    idat = zlib.compress(bytes(raw), 9)

    data = sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")
    path.write_bytes(data)


def gen_cadet_idle() -> None:
    """64x64 canonical 'Cadet' idle pose — round-helmeted astronaut cadet figure.

    Simple silhouette: helmet (moon-silver ring) + visor (brand-signal disc
    with a star-white glint) + suit body (nebula-violet-deep trapezoid) +
    two small side arms. Transparent background so it composites over the
    starfield/space-deep backdrop.
    """
    c = Canvas(64, 64)
    cx = 32.0

    # Suit body (below the helmet).
    c.filled_ellipse(cx, 46, 14, 15, NEBULA_VIOLET_DEEP)
    # Arms (idle, at sides).
    c.filled_ellipse(cx - 15, 44, 5, 9, NEBULA_VIOLET_DEEP)
    c.filled_ellipse(cx + 15, 44, 5, 9, NEBULA_VIOLET_DEEP)
    # Helmet shell.
    c.filled_circle(cx, 24, 20, MOON_SILVER)
    # Visor (the "cyan signal-glint" that ties the mascot to the radio brand).
    c.filled_circle(cx, 25, 14, BRAND_SIGNAL)
    # Visor glint highlight.
    c.filled_circle(cx - 5, 19, 4, STAR_WHITE)
    # Helmet rim shadow (thin darker silver arc along the bottom of the shell).
    c.ring(cx, 24, 20, 20, 2.5, NEBULA_VIOLET_DEEP, a=90)

    write_png(HERE / "cadet_idle.png", c)


def gen_starfield() -> None:
    """320x40 sparse gold/white starfield header strip (transparent bg).

    Deterministic pseudo-random placement (fixed linear-congruential seed,
    no `random` module needed) so re-running this script is byte-identical.
    """
    c = Canvas(320, 40)
    seed = 20260706
    stars = []
    state = seed
    for _ in range(48):
        state = (1103515245 * state + 12345) & 0x7FFFFFFF
        x = state % 320
        state = (1103515245 * state + 12345) & 0x7FFFFFFF
        y = state % 40
        state = (1103515245 * state + 12345) & 0x7FFFFFFF
        gold = (state % 3) == 0
        stars.append((x, y, gold))

    for x, y, gold in stars:
        color = STAR_GOLD if gold else STAR_WHITE
        c.set(x, y, color, a=255)
        # A handful of stars get one lit neighbour pixel for a tiny sparkle.
        if (x + y) % 5 == 0:
            c.set(x + 1, y, color, a=120)

    write_png(HERE / "starfield.png", c)


def gen_planet_corner() -> None:
    """40x40 ringed planet corner accent (transparent bg)."""
    c = Canvas(40, 40)
    cx, cy = 22.0, 20.0
    # Ring (drawn first so the planet body overlaps its near edge).
    c.ring(cx, cy, 17, 6, 2, MOON_SILVER, a=200)
    # Planet body.
    c.filled_circle(cx, cy, 11, PLANET_WARM)
    # Terminator shadow (bottom-right crescent).
    c.filled_circle(cx + 4, cy + 3, 10, PLANET_WARM_DEEP, a=140)

    write_png(HERE / "planet_corner.png", c)


# ── Motif-library additions (M2) ────────────────────────────────────────────
# Everything below is NEW: the full celestial set (crescent moon, comet,
# rocket) plus the remaining Cadet mascot poses. `gen_cadet_idle` above is
# never called from here or modified — these poses have their own
# independent, self-contained drawing code, deliberately not refactored
# through a shared helper with the proven idle-pose function (see module
# doc: keeps "M1's pixels are untouched" a mechanical, not diff-reviewed,
# guarantee).

def gen_crescent_moon() -> None:
    """28x28 crescent moon — lock/quiet-state motif (moon-silver).

    A filled disc with a same-radius disc erased mostly-overlapping, leaving
    a thin sliver — the classic two-circle crescent construction.
    """
    c = Canvas(28, 28)
    c.filled_circle(13, 14, 12, MOON_SILVER)
    c.erase_circle(13 + 9, 14 - 2, 12)
    write_png(HERE / "crescent_moon.png", c)


def gen_comet() -> None:
    """28x14 comet — star-gold head + comet-teal fading tail.

    Used both as a static corner accent (`Comet`) and as the moving element
    inside the `CometOnNotify` one-shot motion helper (`motifs.slint`).
    """
    c = Canvas(28, 14)
    # Tail: a row of shrinking, fading ellipses trailing left of the head.
    tail_steps = 6
    for i in range(tail_steps, 0, -1):
        t = i / tail_steps
        cx = 20 - i * 2.6
        r = 3.2 * t
        alpha = int(60 + 140 * t)
        c.filled_circle(cx, 7, r, COMET_TEAL, a=alpha)
    # Head.
    c.filled_circle(21, 7, 4.2, STAR_GOLD)
    c.filled_circle(21 - 1.2, 7 - 1.2, 1.4, STAR_WHITE)
    write_png(HERE / "comet.png", c)


def gen_rocket() -> None:
    """20x24 small rocket — the `RocketOnSend` one-shot motion helper's
    bitmap (brand-signal body, star-gold nose cone, comet-teal flame)."""
    c = Canvas(20, 24)
    cx = 10.0
    # Flame (drawn first, sits behind/below the body).
    c.filled_ellipse(cx, 20, 3.5, 4.5, COMET_TEAL, a=220)
    # Body.
    c.filled_ellipse(cx, 12, 5, 9, BRAND_SIGNAL)
    # Nose cone.
    c.filled_circle(cx, 4.5, 4.5, STAR_GOLD)
    # Fins.
    c.filled_ellipse(cx - 6, 17, 2.5, 4, NEBULA_VIOLET_DEEP)
    c.filled_ellipse(cx + 6, 17, 2.5, 4, NEBULA_VIOLET_DEEP)
    # Window.
    c.filled_circle(cx, 11, 2, STAR_WHITE)
    write_png(HERE / "rocket.png", c)


def gen_cadet_wave() -> None:
    """64x64 Cadet, one arm raised in a wave — reuses the idle silhouette's
    proportions with the right arm lifted alongside the helmet."""
    c = Canvas(64, 64)
    cx = 32.0
    c.filled_ellipse(cx, 46, 14, 15, NEBULA_VIOLET_DEEP)
    c.filled_ellipse(cx - 15, 44, 5, 9, NEBULA_VIOLET_DEEP)
    # Raised (waving) arm — up and out, beside the helmet.
    c.filled_ellipse(cx + 18, 28, 5, 9, NEBULA_VIOLET_DEEP)
    c.filled_circle(cx, 24, 20, MOON_SILVER)
    c.filled_circle(cx, 25, 14, BRAND_SIGNAL)
    c.filled_circle(cx - 5, 19, 4, STAR_WHITE)
    c.ring(cx, 24, 20, 20, 2.5, NEBULA_VIOLET_DEEP, a=90)
    write_png(HERE / "cadet_wave.png", c)


def gen_cadet_thumbsup() -> None:
    """64x64 Cadet giving a thumbs-up — forward arm with a star-gold thumb
    accent."""
    c = Canvas(64, 64)
    cx = 32.0
    c.filled_ellipse(cx, 46, 14, 15, NEBULA_VIOLET_DEEP)
    c.filled_ellipse(cx - 15, 44, 5, 9, NEBULA_VIOLET_DEEP)
    # Forward arm (thumbs-up), held higher/closer to center than idle.
    c.filled_ellipse(cx + 14, 38, 5, 8, NEBULA_VIOLET_DEEP)
    c.filled_circle(cx + 14, 30, 3, STAR_GOLD)
    c.filled_circle(cx, 24, 20, MOON_SILVER)
    c.filled_circle(cx, 25, 14, BRAND_SIGNAL)
    c.filled_circle(cx - 5, 19, 4, STAR_WHITE)
    c.ring(cx, 24, 20, 20, 2.5, NEBULA_VIOLET_DEEP, a=90)
    write_png(HERE / "cadet_thumbsup.png", c)


def gen_cadet_sleeping() -> None:
    """64x64 Cadet asleep — dimmed/closed visor + drifting "Zzz" marks."""
    c = Canvas(64, 64)
    cx = 32.0
    c.filled_ellipse(cx, 46, 14, 15, NEBULA_VIOLET_DEEP)
    c.filled_ellipse(cx - 15, 44, 5, 9, NEBULA_VIOLET_DEEP)
    c.filled_ellipse(cx + 15, 44, 5, 9, NEBULA_VIOLET_DEEP)
    c.filled_circle(cx, 24, 20, MOON_SILVER)
    # Closed/dimmed visor (nebula-violet-deep instead of the lit brand-signal
    # cyan) — no glint, since there's nothing to catch the signal while
    # asleep.
    c.filled_circle(cx, 25, 14, NEBULA_VIOLET_DEEP)
    c.ring(cx, 24, 20, 20, 2.5, NEBULA_VIOLET_DEEP, a=90)
    # Drifting "Zzz" — three small star-white squares of increasing size.
    c.rect(44, 6, 47, 9, STAR_WHITE)
    c.rect(48, 10, 52, 14, STAR_WHITE)
    c.rect(53, 15, 58, 20, STAR_WHITE)
    write_png(HERE / "cadet_sleeping.png", c)


def gen_cadet_peeking() -> None:
    """64x64 Cadet peeking — only the helmet/visor visible over a ledge, the
    suit body shifted below the visible canvas."""
    c = Canvas(64, 64)
    cx = 32.0
    cy_helmet = 40.0
    # Helmet + visor only, lower in the frame (as if peeking over an edge);
    # no arms/body — those are the part being "hidden".
    c.filled_circle(cx, cy_helmet, 20, MOON_SILVER)
    c.filled_circle(cx, cy_helmet + 1, 14, BRAND_SIGNAL)
    c.filled_circle(cx - 5, cy_helmet - 5, 4, STAR_WHITE)
    c.ring(cx, cy_helmet, 20, 20, 2.5, NEBULA_VIOLET_DEEP, a=90)
    write_png(HERE / "cadet_peeking.png", c)


# ── Full-window backdrop + lower-half line art ──────────────────────────────
# Everything below is NEW, additive-only: every function above this section
# (through `gen_cadet_peeking`) is byte-identical, untouched output — the
# same "M1/M2's pixels are a mechanical guarantee, not a diff-reviewed one"
# discipline the module doc already established. These two generators feed
# `SpaceBackdrop` / `PlanetHorizon` in `firmware/src/ui/motifs.slint`.

def gen_starfield_full() -> None:
    """320x240 full-window dim backdrop starfield (transparent bg).

    Much lower overall density than the header strip's 48-stars/40px
    (~1.2 stars per pixel-row) — this covers the WHOLE window at ~60 stars
    total (~0.25 stars per pixel-row), so it reads as a faint field, not
    clutter. Placement is deliberately NOT uniform-random:

    - stars are biased toward the top and the left/right edges (a
      concave power-curve fold on each axis's random fraction), leaving the
      vertical/horizontal center of the window sparser than its border;
    - a central vertical band (y=88..152 — the zone every static consumer
      screen's title/body text, status rows, or numpad digits actually
      occupies) gets an ADDITIONAL ~4x thinning pass, and any star that
      survives the thinning is relocated to just outside the band's near
      edge rather than left mid-band — so no star ever lands directly
      under a glyph's exact center.

    Per-star alpha values here are the SAME full/sparkle values
    `gen_starfield` already uses; the ≤0.35 overall dimming is
    `SpaceBackdrop`'s job (a component-level `opacity`), not this PNG's —
    keeping the legibility contract in ONE place (the Slint component)
    rather than duplicated/diverging between generator and consumer.
    """
    c = Canvas(320, 240)
    seed = 20260706_02  # distinct seed from gen_starfield's own 20260706
    state = seed

    def next_frac() -> float:
        nonlocal state
        state = (1103515245 * state + 12345) & 0x7FFFFFFF
        return state / 0x7FFFFFFF

    target = 60
    band_lo, band_hi = 88, 152
    stars = []
    while len(stars) < target:
        ux = next_frac()
        uy = next_frac()

        # Edge-bias x: fold the fraction's distance from center through a
        # pow<1 curve, which GROWS small distances faster than large ones —
        # net effect, mass moves from the middle toward the left/right edges.
        d = abs(ux - 0.5) * 2.0
        d = d ** 0.6
        x_frac = 0.5 + (d / 2.0) * (1.0 if ux >= 0.5 else -1.0)
        # Top-bias y: pow>1 shrinks the fraction toward 0 (the top edge).
        y_frac = uy ** 1.7

        x = min(int(x_frac * 320), 319)
        y = min(int(y_frac * 240), 239)

        if band_lo <= y <= band_hi:
            thin_roll = next_frac()
            if thin_roll >= 0.25:
                continue  # 3-in-4 candidates in the text band are dropped
            # A survivor gets pushed to just outside the band's NEAR edge,
            # never left sitting mid-band.
            y = band_lo - 6 if y < (band_lo + band_hi) // 2 else band_hi + 6
            y = max(0, min(y, 239))

        gold = next_frac() < 0.3
        stars.append((x, y, gold))

    for x, y, gold in stars:
        color = STAR_GOLD if gold else STAR_WHITE
        c.set(x, y, color, a=255)
        if (x + y) % 5 == 0:
            c.set(min(x + 1, 319), y, color, a=110)

    write_png(HERE / "starfield_full.png", c)


def gen_planet_horizon() -> None:
    """320x72 dim outline planet-limb + orbit line (transparent bg).

    Deliberately LINE ART, not a filled disc: both the limb and the orbit
    are `ring()` strokes (thin annuli), each a small slice of a much larger
    circle/ellipse whose center sits below (limb) or level with (orbit) the
    visible canvas — only the strokes' upper arcs cross into frame, giving a
    receding "horizon curve" read instead of a filled planet competing with
    whatever static content (e.g. splash's wordmark) sits above this band.
    Both strokes render at low alpha directly in the generator (this asset
    has no further per-consumer dimming step the way `starfield_full.png`
    does via `SpaceBackdrop` — `PlanetHorizon` just re-exports these pixels).
    """
    c = Canvas(320, 72)
    # Orbit line: a wide, flat ellipse stroke riding above the limb — thin
    # and faint, the "orbital ring" read. Center above frame; only its
    # shallow lower arc is visible, cresting near the top of the band and
    # tapering off toward (but staying inside) both side edges.
    c.ring(160, 256, 500, 250, 1.5, MOON_SILVER, a=45)
    # Planet limb: a large ring stroke, center pushed well below the bottom
    # edge so only its upper arc crosses into frame — cresting mid-band and
    # tapering down toward the bottom corners.
    c.ring(160, 300, 280, 280, 2, PLANET_WARM_DEEP, a=80)

    write_png(HERE / "planet_horizon.png", c)


if __name__ == "__main__":
    gen_cadet_idle()
    gen_starfield()
    gen_planet_corner()
    gen_crescent_moon()
    gen_comet()
    gen_rocket()
    gen_cadet_wave()
    gen_cadet_thumbsup()
    gen_cadet_sleeping()
    gen_cadet_peeking()
    gen_starfield_full()
    gen_planet_horizon()
    print("wrote 12 space assets to", HERE)
