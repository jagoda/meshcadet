# `ui_sim` — host-sim render rig for the space-theme image-asset pipeline

Built for the space-theme UI's walking-skeleton milestone M1.

## Why this crate exists

`firmware/` is a DETACHED Cargo workspace: it cross-compiles for
`xtensa-esp32s3-espidf` under the `esp` toolchain and links `esp-idf-svc`/
`esp-idf-hal`, neither of which builds for the host (see
`firmware/Cargo.toml`'s own doc comment). That means there is no way to run
the actual firmware UI on this machine, or in CI, to *see* whether a given
`slint::slint!{}` markup change renders correctly — short of flashing real
hardware, which the project's development process explicitly reserves for a
single hardware-acceptance test at the end of the theme rollout.

This crate closes that gap for the one previously-unproven mechanism that
needed de-risking before committing to it across 7 more screens:
**Slint `Image` assets through the inline `slint!{}` macro**, both ways
described in the design's asset-architecture options table:

1. **PRIMARY** — `@image-url("...")` + `SLINT_EMBED_RESOURCES=embed-for-
   software-renderer`, which decodes a real PNG at HOST compile time (via
   the `image` crate, already pulled in transitively by
   `i-slint-compiler`'s `software-renderer`/`image` features — no new Cargo
   dependency was needed to unlock this) into raw pixel data baked into the
   component's init code. This is the path `firmware/src/ui/screens/
   unprovisioned.rs` ships in production (`firmware/.cargo/config.toml` sets
   the identical env var for the esp target build).
2. **FALLBACK** — `build.rs` computes a packed-RGB565 `u16` array directly
   (same "generate a byte array" shape as `firmware/gen_emoji_font.c`'s font
   pipeline, minus the external FreeType tool since there's no text to
   rasterise), unpacked at runtime into a `SharedPixelBuffer<Rgb8Pixel>` fed
   to `slint::Image::from_rgb8`. Not shipped in `unprovisioned.rs` — the
   primary path built and rendered correctly, so there was no need to ship a
   second production image-loading mechanism — but proven end-to-end here as
   the documented, exercised fallback the design called for.

Both mechanisms render into ONE frame (`ui_sim::render_host_sim_frame`),
importing the REAL `firmware/src/ui/theme.slint` by relative path (not a
fork) and referencing the REAL `firmware/assets/space/*.png` files
`unprovisioned.rs` also uses, so this is genuinely proving the mechanism the
production screen depends on — not a look-alike.

## What shipped vs. what's exercised

**Shipped** (in `firmware/src/ui/screens/unprovisioned.rs`, the pilot
screen): the PRIMARY `@image-url` +
`SLINT_EMBED_RESOURCES=embed-for-software-renderer` path, for all three
assets (starfield header, corner planet, Cadet mascot hero). Confirmed to
build AND render correctly both here (host-sim) and on the real
`xtensa-esp32s3-espidf` cross-compile target (`cargo build --release`
in `firmware/`; the measured flash delta is documented in ADR-0003).

**Exercised, not shipped**: the FALLBACK build.rs-byte-array +
`SharedPixelBuffer` path, proven by this crate's "moon" swatch. Kept
in reserve per the design ("if the primary path fails both ways, abort") —
it did not fail, so there was no reason to carry a second, unused runtime
image-loading code path into the shipped screen.

## Reproducing the evidence

```sh
cargo test -p ui_sim   # asserts non-blank pixels for all three regions
cargo run  -p ui_sim   # regenerates docs/renders/unprovisioned-space-host-sim.png
```

The PNG this produces is checked into the repo at
`docs/renders/unprovisioned-space-host-sim.png` — the human-visible
host-sim render deliverable this crate exists to produce. It shows (left to
right, top to bottom): the sparse gold/white starfield header strip and
ringed corner planet (both PRIMARY path), and the moon-silver swatch
standing in for the mascot position (FALLBACK path) — deliberately NOT a
pixel-for-pixel mirror of `unprovisioned.rs`'s full markup (no wordmark
text, no animations — those are plain Slint features every other themed
screen already proves), isolating exactly the one mechanism this crate
exists to de-risk.

## Motif-library render rig (M2)

`ui_sim::motif_library` is a SECOND, independent render path added by M2 —
it proves the shared `firmware/src/ui/motifs.slint` asset+motion-helper
library (the full celestial set: starfield, ringed planet, crescent moon,
comet, rocket; the remaining Cadet mascot poses: wave, thumbs-up, sleeping,
peeking; and the four one-shot motion helpers named in the design:
`MascotBob`, `Twinkle`, `RocketOnSend`, `CometOnNotify`), as opposed to the
`HostSimUi`/`render_host_sim_frame` pair above, which remains M1's untouched
walking-skeleton proof (already independently re-verified against a specific
landed commit).

It is a separate component, a separate `Platform` installer
(`MotifLibraryFrame`), and a separate Cargo integration-test file
(`tests/motif_library.rs`, not a `#[cfg(test)]` module in `lib.rs`) —
Slint enforces one `Platform` singleton per process, and `tests/*.rs` files
each get their own process, which is what keeps the two render rigs from
colliding when `cargo test -p ui_sim` runs both.

```sh
cargo test -p ui_sim --test motif_library        # asserts every asset/motion-helper
cargo run  -p ui_sim --bin motif_library_render  # regenerates docs/renders/motif-library-host-sim.png
```

The one-shot motion helpers are exercised for BOTH their rest and settled/
fired states (`RocketOnSend`/`CometOnNotify` are also captured mid-flight,
since a comet that sweeps and fades has no single frame between "off left"
and "off right, invisible" that alone proves motion occurred — see the
test's own comments for the exact timing rationale). `firmware/src/ui/
screens/unprovisioned.rs` (M1's pilot) does NOT import `motifs.slint` — the
design excludes it from the 7-screen fan-out, so it is left exactly
as originally verified.

## Landing-page promo screenshots

Four more render rigs (`contact_list_promo`, `message_view_promo`,
`compose_promo`, `splash_promo`) produce the promotional screenshots on
`site/index.html`'s `#screenshots` gallery. Unlike every rig above — each a
narrow, single-mechanism proof — these copy their target production
screen's full `slint::slint!{}` markup (`firmware/src/ui/screens/
{contact_list,message_view,compose,splash}.rs`) VERBATIM, because the
deliverable is a screenshot of the REAL screen, seeded with tasteful,
OSS-appropriate sample data (space-mission callsigns, no PII, no internal
vernacular), not a narrow proof. Same real `theme.slint`/`motifs.slint`
relative-path imports as every rig above.

```sh
cargo run -p ui_sim --bin contact_list_promo_render  # site/assets/screenshot-contacts.png
cargo run -p ui_sim --bin message_view_promo_render  # site/assets/screenshot-messages.png
cargo run -p ui_sim --bin compose_promo_render       # site/assets/screenshot-compose.png
cargo run -p ui_sim --bin splash_promo_render         # site/assets/screenshot-splash.png
```

Regenerate all four after any change to one of those four screens' markup,
`theme.slint`, or `motifs.slint` by re-copying the updated markup into the
corresponding `ui_sim::*_promo` module and re-running its binary — see
`site/README.md`'s `assets/` bullet for the site-side half of this
contract.

## Env requirements

`SLINT_EMBED_TEXTURES=1` and `SLINT_EMBED_RESOURCES=embed-for-software-renderer`
must be visible as real process env vars when this crate's `slint::slint!{}`
macro expands. Cargo config resolution walks UP from the invoking CWD, so
a root `cargo test`/`cargo run -p ui_sim` (CWD = repo root) only ever reads
the ROOT `.cargo/config.toml` — that's where these two vars live (see that
file's own comment for why a nested `ui_sim/.cargo/config.toml` would
silently not apply to that invocation).
