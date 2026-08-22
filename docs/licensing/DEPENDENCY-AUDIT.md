# Dependency license audit — GPLv3 compatibility

- **Status:** Audit performed 2026-07-07, against `main@9906dae` (the OSS-publish source);
  "Bundled font assets" section added 2026-07-08 to close a gap the original pass missed
  (Rust crates + Slint only — bundled font binaries were not in scope of that pass).
  "Bundled font assets" §1 (Noto Emoji) refreshed 2026-08-02 for the upstream font-asset
  upgrade — coverage-neutral, license terms unchanged (OFL 1.1); see that entry for detail.
  **2026-08-02:** the remaining gap noted in §1 below — the license text living in the git
  tree but not traveling with the *published release binaries* — is now closed:
  `.github/workflows/release.yml`'s `release` job attaches `NOTICE`,
  `firmware/assets/NotoEmoji-LICENSE.txt`, and `docs/licensing/DejaVuSans-LICENSE.txt` to
  every GitHub Release alongside the firmware images, covering the OFL 1.1 condition-2 and
  Bitstream-Vera-license redistribution obligations for the shipped binaries, not just the
  repo. See `docs/release-reproducibility.md` for the updated (nine-asset) list.
  **2026-08-03:** §3 (Noto Color Emoji) added by `meshcadet-emoji-picker-color-cells` — a
  second, independent OFL 1.1 font asset (`firmware/assets/NotoColorEmoji.ttf`), pulled from
  the same `googlefonts/noto-emoji` upstream, whose CBDT color-bitmap glyphs feed a NEW,
  separate build-time pipeline (`firmware/build_emoji_color.rs`) generating the emoji
  picker's color-cell rasters — verified OFL 1.1, not Apache-2.0 (that repo's top-level
  tooling is Apache-2.0; only `fonts/`, including this file, is OFL 1.1). Release-artifact
  attachment extended to a tenth asset, `NotoColorEmoji-LICENSE.txt` — see
  `docs/release-reproducibility.md`'s updated (ten-asset) list.
- **Result: PASS.** Every dependency in both Cargo graphs is compatible with shipping
  MeshCadet under **GPLv3** (`LICENSE` at repo root). No dependency forces a different
  outcome and none needs to be swapped out or re-licensed.

## Why this audit exists

MeshCadet ships under GPLv3 because the firmware crate reaches into `i-slint-core`
internals (`RendererSealed`, `BitmapFont`/`BitmapGlyph`/`CharacterMapEntry` — see
`firmware/Cargo.toml`'s `i-slint-core` dependency comment) to register build-time-generated
bitmap fonts. Slint's own license is a triple choice — `GPL-3.0-only OR
LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` — and the two
non-GPL options either require a signed royalty-free agreement or restrict use to Slint's
own "Software" terms, neither of which fits a hobbyist open-source project reaching into
sealed internals. GPL-3.0-only is the option Slint offers with no such strings attached,
so MeshCadet takes it. Every other dependency then has to clear the bar of "does this
license permit combination with a GPLv3 work" — this document is that check.

## Method

`cargo metadata --format-version 1` was run against both of MeshCadent's Cargo graphs
(the firmware crate is a *detached* workspace — see the root `Cargo.toml` banner comment —
so it resolves its own dependency tree under the `esp` toolchain, separately from the root
workspace's `protocol`/`host`/`xtask`/`ui_sim`/`ui_perf` members):

```sh
cargo metadata --format-version 1 > /tmp/root_meta.json          # protocol, host, xtask, ui_sim, ui_perf
(cd firmware && cargo metadata --format-version 1 > /tmp/fw_meta.json)  # firmware (esp target)
```

Every resolved package's `license` field was extracted and deduplicated by
`(name, version)` across both graphs: **691 unique crate/version pairs total, 579 in the
root graph, 643 in the firmware graph** (heavy overlap — `protocol`, `slint`, and friends
are shared). **Zero packages had a missing/unresolved `license` field** — nothing required
guessing from a `LICENSE` file. The full merged table lives in
`docs/licensing/THIRD-PARTY-LICENSES.md`.

## License families present, and why each is GPLv3-safe

| Family | Count (root graph) | GPLv3-compatible because |
|---|---|---|
| `MIT`, `MIT OR Apache-2.0` (and reorderings/slash variants) | ~460 | Permissive, no restriction on combination with a copyleft work. |
| `Apache-2.0` alone or `OR`'d with MIT/BSD/Zlib | ~30 | Apache-2.0 is explicitly listed by the FSF as GPLv3-compatible (GPLv2 is the one with friction; v3 added the patent-clause language precisely to fix that). |
| `BSD-2-Clause`, `BSD-3-Clause` (and `OR` combos) | ~25 | Permissive, FSF-listed GPL-compatible. |
| `Zlib`, `Zlib OR Apache-2.0 OR MIT` | ~18 | Permissive, FSF-listed GPL-compatible. |
| `Unlicense`, `Unlicense OR MIT`, `0BSD OR MIT OR Apache-2.0` | ~12 | Public-domain-equivalent; strictly more permissive than MIT. |
| `ISC`, `CC0-1.0 OR Apache-2.0`, `NCSA`-tagged combos | 3 | Permissive, MIT-equivalent terms. |
| `Unicode-3.0` (the `icu_*` crate family: `icu_collections`, `icu_normalizer`, `icu_properties`, `zerovec`, `yoke`, etc.) | 22 | Modern (2023) OSI-approved permissive license, pulled in only as **build-time tooling** (`i-slint-compiler`'s Unicode data tables, via `slint-build`'s build-dependency edge — see `cargo tree -i icu_normalizer`); not itself a runtime/copyleft concern. |
| `BSL-1.0` (`clipboard-win`, `error-code`) | 2 | Boost Software License — permissive, FSF-listed GPL-compatible. |
| `MPL-2.0` (**`serialport 4.9.0`**, direct dep of `host/Cargo.toml`, used for USB-serial provisioning) | 1 | File-level (not whole-work) copyleft. FSF and the license's own text (MPL §3.3) treat MPL-2.0 as GPL-combination-safe by default (no "Incompatible With Secondary Licenses" notice on the crate) — this is the standard "use MPL library from a GPL project" pattern, not a conflict. |
| `LGPL-2.1-or-later` as one arm of a triple `MIT OR Apache-2.0 OR LGPL-2.1-or-later` (`r-efi`) | 2 (2 versions) | We take the MIT arm; even the LGPL arm alone is GPLv3-combinable (LGPL code may always be used under the terms of the encompassing GPL). |
| `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` (the Slint crate family: `slint`, `i-slint-core`, `i-slint-backend-*`, `i-slint-renderer-*`, `i-slint-common`, `slint-macros`, `slint-build`, `i-slint-compiler`) | 12 | **This is the load-bearing dependency the whole GPLv3 decision is built around.** We take the `GPL-3.0-only` arm. See "Why this audit exists" above. |
| `GPL-3.0/MIT` (`unescaper 0.1.8`, transitive via `serialport`) | 1 | Old-style slash-separated **dual** license (either arm, chooser's pick) — MIT arm alone clears it trivially, and the GPL-3.0 arm is our own project license anyway. |
| `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | 19 (root) | The LLVM exception only *loosens* Apache-2.0's patent-retaliation clause for compiler-runtime linking; the plain `Apache-2.0` and `MIT` arms are both available and both clear as above. |

**Nothing in either tree carries `GPL-2.0-only` (without `-or-later`), `CDDL`, `EPL`,
`AGPL`, `SSPL`, `BUSL`, a Commons-Clause rider, or a proprietary/no-license marker** — the
license families that would actually need a swap-out or a special exception under a GPLv3
combination. Confirmed by grepping the full merged appendix
(`docs/licensing/THIRD-PARTY-LICENSES.md`) for those patterns: no hits.

## Flagged dependencies (closer look, not blockers)

1. **Slint (`slint`, `i-slint-core`, and the `i-slint-*` family)** — firmware's UI
   toolkit, and the reason MeshCadet is GPLv3 at all (see above). Firmware additionally
   depends directly on `i-slint-core` (pinned `=1.16.1`, matching the `slint` crate's own
   pin — see `firmware/Cargo.toml`'s "PIN-TRACKING" comment) to reach `RendererSealed` and
   the bitmap-font types. Both crates resolve to the same triple-license choice; both are
   taken under `GPL-3.0-only`. **This is a dependency to keep version-locked in step**
   (the existing PIN-TRACKING comment already enforces that discipline) — a future `cargo
   update` on either without the other risks a feature-flag mismatch, not a license
   mismatch.
2. **`serialport 4.9.0`** (`host/Cargo.toml`, direct dep) — MPL-2.0, and its own transitive
   dependency `unescaper 0.1.8` — dual `GPL-3.0/MIT`. Both clear as noted in the table
   above. No source modification is made to either; they are used as ordinary linked
   libraries, which is exactly the combination pattern both licenses' compatibility
   provisions anticipate.
3. **The `icu_*` / Unicode-3.0 family** — confirmed **build-time-only** (host-side
   `i-slint-compiler` tooling pulled in via the `slint-build` build-dependency edge in
   `firmware/Cargo.toml`, `ui_sim/Cargo.toml`, and `ui_perf/Cargo.toml`), not linked into
   the on-device firmware binary's runtime dependency graph.

## MeshCore / RadioLib — attribution, not a licensing conflict

`protocol/` is a byte-exact Rust reimplementation of the MeshCore v1.15.0 wire format
(framing, crypto, codec — see `protocol/src/lib.rs`'s module-level doc), prefix-compatible
with v1.16/v1.17 (the current protocol target, v1.17.1 verified wire-unchanged from
v1.16.0 — see `docs/adr/0001-charter.md` §1), and
`firmware/src/radio.rs` cites specific RadioLib source behavior (register values, timing
constants) it mirrors for SX1262 bring-up (see `firmware/src/radio.rs:38-39`). Both
upstream projects — **MeshCore** (github.com/meshcore-dev/MeshCore) and **RadioLib**
(github.com/jgromes/RadioLib) — are **MIT-licensed**, which is unconditionally compatible
with GPLv3 combination or reference. Neither is vendored or copy-pasted into this repo
(both are independent Rust ports/reimplementations informed by reading the upstream C++
source, not derived-by-copying source trees), so no upstream license text needs to travel
with this repo as a *combined-work* obligation — but attribution is owed as a matter of
provenance and good practice, and is recorded in `NOTICE` at the repo root.

## Bundled font assets

The Cargo-graph audit above (crates + Slint) does not cover binary font
assets, which are a separate redistribution question from Rust crate
licensing. Checked here for completeness:

1. **NotoEmoji-Regular.ttf** (`firmware/assets/NotoEmoji-Regular.ttf`) — **is
   bundled** (tracked in git; confirmed via `git ls-tree -r HEAD | grep -i
   font`). Copyright 2013 Google LLC. Licensed under the SIL Open Font
   License, Version 1.1 (OFL 1.1). OFL 1.1 condition (2) requires the
   license text + copyright notice travel with every redistributed copy of
   the font. That text now accompanies the font at
   `firmware/assets/NotoEmoji-LICENSE.txt`, and the copyright/license is
   additionally credited in `NOTICE`. **Prior to the 2026-07 audit update,
   this file was bundled with no accompanying license text anywhere in the
   tree — a real (if low-severity) OFL redistribution-compliance gap,
   closed then.**
   **2026-08-02 upgrade:** the bundled font asset itself was swapped for a
   current upstream Noto Emoji release (pulled from google/fonts,
   `ofl/notoemoji/NotoEmoji[wght].ttf` Version 3.002) — coverage-neutral
   (`EMOJI_CPS`/`EMOJI_TABLE`/`UI_EXTRA_CPS`/`BMP_SYMBOLS` unchanged; see
   `firmware/assets/NotoEmoji-LICENSE.txt` for the verification record).
   License terms are unchanged (OFL 1.1, same condition-2 obligation, same
   copyright holder); the only textual delta is the entity name in the
   font's own copyright string, which now reads "Google LLC" rather than
   "Google Inc." on this vintage — this is the 2017 Google Inc. -> Google
   LLC rename already anticipated in the pre-upgrade text of this file, not
   a new fact. The OFL<->GPLv3 embedding analysis below is unaffected and
   was not re-litigated.

2. **NotoColorEmoji.ttf** (`firmware/assets/NotoColorEmoji.ttf`) — **is
   bundled** (tracked in git). Added by `meshcadet-emoji-picker-color-cells`
   (2026-08-03) as the source for the emoji picker's color-cell rasters — a
   SEPARATE build-time pipeline (`firmware/build_emoji_color.rs`, invoked
   from `firmware/build.rs` alongside, not instead of,
   `gen_emoji_font.c`'s existing mono-glyph pipeline) decodes its CBDT
   color-bitmap glyphs and re-encodes them as small palette-indexed rasters;
   the font file itself is a build-time-only asset (never embedded — only
   the tiny generated output is). Pulled 2026-08-03 from
   `googlefonts/noto-emoji`, `fonts/NotoColorEmoji.ttf`
   (sha256 `72a635cb3d2f3524c51620cdde406b217204e8a6a06c6a096ff8ed4b5fd6e27b`),
   embedded `name`-table copyright "Copyright 2022 Google Inc.", Version
   string `2.051;GOOG;noto-emoji:20250818:e92753bfa55fd449e427d4d325f9c8c40408c74e`.
   Licensed under the SIL Open Font License, Version 1.1 (OFL 1.1) —
   **verified from the same repo's `fonts/LICENSE`, not assumed**: that
   repo's top-level tooling (Python build scripts) is Apache-2.0, but
   everything under `fonts/`, including this file, ships OFL 1.1 via its own
   `fonts/LICENSE` (per campaign D4's explicit "verify OFL 1.1, not
   Apache-2.0" instruction — this is the same license family and the same
   upstream project as (1) above, just a different font file within it, so
   the OFL<->GPLv3 embedding analysis already established for (1) applies
   identically and was not re-litigated). License text accompanies the font
   at `firmware/assets/NotoColorEmoji-LICENSE.txt`, and the copyright/license
   is additionally credited in `NOTICE`.

3. **DejaVu Sans** — used by `firmware/gen_emoji_font.c` (invoked from
   `firmware/build.rs`) as the source for the ASCII/Latin glyphs in the same
   build-time-generated bitmap font that also carries the Noto Emoji glyphs.
   **Not bundled**: `git ls-tree -r HEAD` shows no DejaVu file tracked in
   this repository; `firmware/build.rs` reads it from a hardcoded
   system-installed path (`/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf`,
   the `fonts-dejavu-core` package on Debian/Ubuntu) at build time only, and
   its own comment there explicitly documents this as an intentional
   system-font dependency, not an in-tree asset. Because the *rasterized
   output* of DejaVu Sans's glyphs is nonetheless compiled into the shipped
   firmware binary (a Modified Version under the Bitstream Vera Fonts
   Copyright license DejaVu is built on), its copyright notice and license
   are recorded in `NOTICE` and `docs/licensing/DejaVuSans-LICENSE.txt` out
   of the same redistribution-compliance caution as (1), even though no
   DejaVu font file itself needs to (or does) travel in this git tree.

**2026-08-02 — release-binary redistribution gap closed.** (1) and (3) above
established that the license text lives in this git tree; neither closed the
separate question of whether it travels with the *shipped release binaries*
(the merged/app firmware images `.github/workflows/release.yml` uploads to
each GitHub Release), which is what OFL 1.1 condition (2) actually requires
of a redistributed copy. It did not, until now: the `release` job's "Copy
OFL/attribution files for release attachment" step attaches `NOTICE`,
`firmware/assets/NotoEmoji-LICENSE.txt`, and
`docs/licensing/DejaVuSans-LICENSE.txt` to every published Release alongside
the firmware images (covered by the same `SHA256SUMS` and provenance
attestation as the binaries themselves) — closing the gap. This does not
reopen or change the OFL<->GPLv3 embedding analysis above; it is a packaging
fix, not a licensing-conclusion change. **2026-08-03:** the same step now
also attaches `firmware/assets/NotoColorEmoji-LICENSE.txt` for (2) above,
closing the identical gap for the newly-bundled color-emoji font asset at
the moment it landed, rather than after the fact.

## Conclusion

**No dependency in either Cargo graph blocks or complicates shipping MeshCadet under
GPLv3.** The one dependency that mattered for the license *choice* (Slint) is accounted
for by taking its `GPL-3.0-only` license arm. The full crate-by-crate table is in
`docs/licensing/THIRD-PARTY-LICENSES.md` for anyone who wants to re-verify by hand or
re-run the audit after a `cargo update`.
