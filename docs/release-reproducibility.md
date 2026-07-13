# Reproducing and verifying a MeshCadet firmware release

This is the recipe for (a) rebuilding a published `vX.Y.Z` release's merged
firmware image byte-for-byte on your own machine, and (b) verifying its SLSA
build provenance attestation. It describes exactly what
`.github/workflows/release.yml` does — if the two ever disagree, the workflow
is buggy, not this doc. See `docs/adr/0004-release-architecture.md` §7/§8 for
the design rationale.

## Why this is reproducible at all

A firmware image is a lot of bytes with a lot of opportunities for a rebuild
to differ from the original — compile timestamps, absolute build paths baked
into debug info or panic strings, and (specific to this project) a
build-time-generated bitmap font. Five levers close each hole:

1. **`MESHCADET_RELEASE_VERSION`** (`firmware/build.rs::emit_build_version`,
   the phase-2 seam) — the release build stamps the exact tag string into the
   boot-version string instead of deriving it from `git describe`/`git
   rev-parse`, so the binary carries no build-machine-dependent git state.
2. **`SOURCE_DATE_EPOCH`** — set to the *tag commit's own* commit date (not
   "now"), the conventional env var several toolchain components honor in
   place of a live timestamp.
3. **`RUSTFLAGS=--remap-path-prefix=...`** — rustc embeds the absolute source
   path of every compiled file (`file!()`, debug info). Every build — CI and
   your own local reproduction — mounts the checkout at the same
   container-internal path (`/build`) and uses the same fixed `CARGO_HOME`
   (`/opt/cargo`, baked into the image), so the embedded paths come out
   identical regardless of where your own checkout or cargo registry
   actually live on disk.
4. **`CONFIG_APP_REPRODUCIBLE_BUILD=y`** (`firmware/sdkconfig.defaults`) —
   esp-idf's own equivalent of (2)+(3): strips the `esp_app_desc` compile
   date/time stamp and hides absolute paths in the C components' debug
   macros. Applies to every build config, not just release.
5. **The pinned release container** (`firmware/release-container/`) — the one
   hazard (1)-(4) cannot reach: `firmware/build.rs`'s `build_emoji_font()`
   shells out to the *host's own* `gcc` + FreeType against the *host's own*
   `/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf` to rasterize the bundled
   emoji/UI font at compile time. FreeType's hinting/rasterization output can
   change across FreeType or DejaVu Sans versions even with (1)-(4) all held
   constant, so the container pins `libfreetype6-dev` and `fonts-dejavu-core`
   to exact package versions (see the Dockerfile's header comment for the
   exact versions and where they were resolved from).

The ESP-IDF version (`v5.2.2`) and the `esp` Rust toolchain fork version are
*already* pinned independent of any of this — see
`firmware/.cargo/config.toml`'s `ESP_IDF_VERSION` and the Dockerfile's
`ESP_TOOLCHAIN_VERSION` build-arg.

## Rebuilding a release

```sh
git clone https://github.com/jagoda/meshcadet.git
cd meshcadet
git checkout vX.Y.Z          # the tag you want to reproduce

# The tag's commit date, in Unix epoch seconds — same value release.yml uses.
SOURCE_DATE_EPOCH=$(git log -1 --format=%ct HEAD)

docker build -f firmware/release-container/Dockerfile -t meshcadet-release-builder firmware

mkdir -p dist
docker run --rm -v "$PWD:/build" meshcadet-release-builder "vX.Y.Z" "$SOURCE_DATE_EPOCH"

sha256sum "dist/meshcadet-vX.Y.Z-merged.bin" "dist/meshcadet-vX.Y.Z-app.bin"
```

Compare that `sha256sum` output against the `SHA256SUMS` file attached to the
`vX.Y.Z` GitHub Release. They must match exactly. The same `docker run` also
writes `dist/update-meta.json` (`layout_hash`/`upgrade_safe` — see
`docs/adr/0008-nondestructive-update-artifacts.md`); `manifest.json` and
`manifest-update.json` are generated separately, by `release.yml` itself,
not by this container (they're pure text, no build output needed) — so
there's nothing to reproduce-and-compare for those two, only to diff against
the published copy.

**If `docker build` fails to install the pinned `libfreetype6-dev`/
`fonts-dejavu-core` versions** because Ubuntu's archive has since pruned
them: the exact versions are still available via
[snapshot.ubuntu.com](https://snapshot.ubuntu.com/) — pin an
`archive.ubuntu.com` snapshot timestamp in `firmware/release-container/
Dockerfile`'s `apt-get` step (via an additional `/etc/apt/sources.list.d/
snapshot.list` pointing at the matching `https://snapshot.ubuntu.com/ubuntu/
<timestamp>/` mirror) instead of the live archive, then re-resolve and update
this doc + the Dockerfile's `ARG` defaults together in one PR.

## Verifying the provenance attestation

Every release is attested via `actions/attest-build-provenance` over the six
published assets (`meshcadet-vX.Y.Z-merged.bin`, `meshcadet-vX.Y.Z-app.bin`,
`manifest.json`, `manifest-update.json`, `update-meta.json`, `SHA256SUMS` —
see `docs/adr/0008-nondestructive-update-artifacts.md` for the three assets
added alongside the original merged-image/`manifest.json`/`SHA256SUMS` set
ADR-0004 §7/§8 established). Verify a downloaded asset against GitHub's
attestation transparency log with the `gh` CLI:

```sh
gh attestation verify meshcadet-vX.Y.Z-merged.bin --repo jagoda/meshcadet
```

A passing verification proves the file was built by the
`.github/workflows/release.yml` workflow, from the `jagoda/meshcadet`
repository, at the commit tagged `vX.Y.Z` — it does not, by itself, prove
byte-reproducibility (that's what the rebuild-and-`sha256sum`-compare
recipe above is for). Run both checks for full confidence: attestation
proves *provenance* (who built it, from what source); the rebuild proves
*reproducibility* (that anyone else, not just GitHub's runners, gets the
same bytes from that source).

## What's flashed where

The merged image is a single flat binary meant to be written starting at
flash offset `0x0`. It concatenates, at the fixed offsets
`firmware/partitions.csv` and `firmware/scripts/flash-with-partition-table.sh`
already document for this hardware:

| Offset   | Content                                              |
|----------|-------------------------------------------------------|
| `0x0`    | bootloader (ESP32-S3's bootloader offset — not `0x1000`, that's original ESP32) |
| `0x8000` | the custom `partition-table.bin` (carries the `mc_hist` history partition) |
| `0x10000`| the app image (`factory` partition)                  |

A bare app `.bin` flashed alone onto a device that does **not already**
carry this project's custom partition table (a fresh/unprovisioned device,
or one whose installed layout doesn't match) will not boot correctly — see
`firmware/scripts/flash-with-partition-table.sh`'s header comment for the
full story on why this project's partition table can't just be passed to
`espflash --partition-table` directly. As of
`docs/adr/0008-nondestructive-update-artifacts.md`, the app image
(`meshcadet-vX.Y.Z-app.bin`) is ALSO published standalone, specifically to be
flashed alone at `0x10000` over a device that already runs a
layout-compatible MeshCadet build (i.e. an in-place, non-destructive
upgrade) — that ADR is the compatibility-gate design (`layout_hash`/
`upgrade_safe`) governing when doing so is safe.
