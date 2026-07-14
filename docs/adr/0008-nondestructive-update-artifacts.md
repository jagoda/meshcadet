# ADR-0008 — Non-Destructive Update Artifacts: App-Only Image + Layout Compatibility Gate

- **Status:** Accepted (2026-07-13)
- **Deciders:** Maintainer design review (`meshcadet-nondestructive-firmware-updates` campaign)
- **Supersedes:** —
- **Implements:** the release-artifacts child of the
  `meshcadet-nondestructive-firmware-updates` campaign. **This ADR IS the
  contract** the site-flasher child (`meshcadet-flasher-two-path-upgrade`)
  consumes — its schemas below (manifest-update.json, update-meta.json) are
  frozen as of this ADR's acceptance; a breaking change to either needs its
  own ADR revision, not a silent edit.
- **Code:** `firmware/release-container/build.sh`,
  `firmware/release-container/generate-update-meta.sh` (+
  `generate-update-meta.test.sh`), `firmware/release-container/Dockerfile`,
  `firmware/layout-baseline.txt`, `.github/workflows/release.yml`. Extends
  ADR-0004 §7 (tag-fired release artifacts) and is consumed by the flasher
  design in ADR-0006.

## Context

ADR-0004 §7 publishes exactly one flashable artifact per release: a merged
image (bootloader@0x0 + `firmware/partitions.csv`'s custom partition
table@0x8000, carrying `mc_hist` + app@0x10000, `esptool merge_bin`,
0xFF-padded, spanning `0x0..0x610000`). Flashing that image is *inherently*
destructive to on-device state: its 0xFF padding blankets `nvs`@0x9000
(`mc_id`/`mc_cfg`/runtime config — ESP-IDF NVS treats an 0xFF-filled page as
erased), and ADR-0006's web flasher additionally full-erases the chip before
writing, wiping `mc_hist`@0x610000 (256 KB of per-conversation history) too.
So today, "upgrade firmware" and "factory reset" are the same operation.
`firmware/release-container/build.sh` already builds a standalone app image
(the `factory` partition, via `esptool elf2image`) as an intermediate step
of producing the merged image — it discarded that image (`rm -f
"$APP_BIN"`) rather than publishing it, because nothing needed it before now.

This ADR (mission-scoped: `meshcadet-nondestructive-update-artifacts`) makes
that discarded intermediate a first-class, published, kept build output, and
defines the compatibility metadata a consumer needs to know when flashing it
*alone* (i.e. without the bootloader/partition-table it was built against) is
safe. It does **not** touch the merged/Fresh-install image or path at all —
by design (see Scope in the owning mission's dossier) — and it does not
implement the actual non-erasing flash flow; that is the site-flasher
child's job (ADR-0006 is the design record for it), consuming exactly the
artifacts and metadata this ADR freezes.

## Decision

### D1 — App-only image, not an OTA-partition redesign

Publish the app image (`factory` partition, flashed at `0x10000`) as a
second release asset, `meshcadet-vX.Y.Z-app.bin`, alongside the unchanged
`meshcadet-vX.Y.Z-merged.bin`. Flashing only the app region touches neither
`nvs`@0x9000 nor `mc_hist`@0x610000 — a device's identity, config, and
history all survive the write.

**Alternatives Considered — dual-bank OTA (`ota_0`/`ota_1`/`otadata`),
rejected.** ESP-IDF's OTA partition scheme buys atomic rollback for
*on-device, over-the-network self-update* — a capability MeshCadet does not
have and this mission does not add. Adopting it here would be strictly
worse for this specific problem: repartitioning `factory` into
`ota_0`/`ota_1` **changes the partition table itself**, and a partition-table
change is, by this same ADR's own D2 gate, a *layout-incompatible* — i.e.
erasing — migration. Adopting OTA to eliminate destructive updates would
therefore require exactly one more destructive update to get there, and
would permanently halve available app space (`factory` is 6 MB single-bank
today; two OTA banks big enough for the ~2.25 MB Slint app image would still
cost meaningfully more flash than one 6 MB region for no benefit this
project uses). Revisit only if on-device self-update becomes an actual goal.

### D2 — Compatibility gate: committed-baseline `layout_hash` + `upgrade_safe` (revises the original plan's D2 per R1)

An app-only image flashed at `0x10000` is valid **only** when the target
device's installed bootloader + partition table are byte-identical to the
ones this release's app image was built against — a resized or moved
partition changes where `factory` starts/ends, what `mc_hist`'s actual
offset is, or how big the bootloader region is, and blindly writing app
bytes at a hardcoded `0x10000` over an incompatible layout can corrupt
either the app itself or an adjacent partition.

- **`layout_hash`** = `sha256(bootloader.bin || partition-table.bin)`
  (raw file bytes, concatenated, not a slice of the merged/padded image),
  computed from the same `TARGET_DIR/bootloader.bin` +
  `TARGET_DIR/partition-table.bin` cmake outputs the merged image is built
  from.
- **`upgrade_safe`** is `true` **iff** this release's `layout_hash` equals
  the value committed at `firmware/layout-baseline.txt` — a checked-in file,
  not a runtime query.

The comparison itself lives in its own script,
`firmware/release-container/generate-update-meta.sh` (invoked by `build.sh`,
COPYed into the release container by `./Dockerfile`), specifically so this
one piece of the pipeline — the thing that decides whether an app-only flash
is offered as a non-destructive Upgrade — has test coverage
(`generate-update-meta.test.sh`, synthetic fixtures) independent of a full
ESP-IDF/docker build. It fails the build outright (not a soft warning) if
the baseline file is missing or has no non-comment hash line — a missing
compatibility baseline is a build defect, not a case to silently default
either way on.

**Why a committed baseline, not "the previous release's metadata" (R1 —
revises the originating plan's original design):** the original plan
sketched computing `upgrade_safe` by comparing against the *previous
release's* published `layout_hash` (via `gh api`/`gh release download`
mid-build). That doesn't work for the *first* app-only-artifact-bearing
release: no release shipped before this ADR ever emitted a `layout_hash` —
v0.1.0 and v0.2.0's Releases carry only the merged image, `manifest.json`,
and `SHA256SUMS` (ADR-0004 §7, unmodified by this ADR). Querying a
non-existent predecessor value would either crash the build or default to
`upgrade_safe:false`, needlessly forcing every device to Fresh-install even
though `firmware/partitions.csv` has been byte-identical since the initial
commit. A committed baseline sidesteps this: it needs no releases-API
round-trip mid-build, and it is deterministic — the same release, rebuilt
identically (ADR-0004 §8), always computes the same `layout_hash` and
compares against the same committed value.

**Baseline provenance (`firmware/layout-baseline.txt`):** seeded with
`8c2f016672527d802f58b67787e2d63f441191f0b6d4f0dc49140d5d66eeff36`, verified
— not guessed — by downloading both published `v0.1.0` and `v0.2.0` merged
release images and extracting the exact `bootloader.bin`/`partition-table.bin`
byte ranges from each (`esptool`'s image parser locates the bootloader
image's true end — file offset `0x5130` in both — ahead of the 0xFF
`merge_bin` gap-fill up to `0x8000`; the partition-table region at
`0x8000..0x80a0` is exactly 160 bytes: 4 partition entries + 1 MD5-sum
trailer, `gen_esp32part.py`'s standard output shape, also 0xFF-padded out to
`nvs`'s `0x9000` offset). Both tags hash **identically**, empirically
confirming `firmware/partitions.csv`'s "unchanged since the initial commit"
claim and validating this baseline as this release's own layout. This makes
the first app-only release correctly `upgrade_safe:true`.

**Ownership:** this mission (and, going forward, whichever mission
intentionally changes `firmware/partitions.csv` or the bootloader layout)
owns bumping `firmware/layout-baseline.txt` — and marking that specific
release `upgrade_safe:false` — in the same PR as the layout change, per
`layout-baseline.txt`'s own header comment. A layout-changing release is
**not** required to omit the `app.bin`/update-manifest assets (see
Consequences) — `upgrade_safe:false` alone is the gate a consumer must
honor; the site-flasher child (ADR-0006) must not offer the Upgrade path
for a release whose `upgrade_safe` is `false`.

**Correction (2026-07-14) — the baseline-provenance methodology above was
wrong, and mis-gated v0.3.0.** `firmware/v0.3.0/update-meta.json` published
`layout_hash: 011e0de9..` against the baseline `8c2f016672..` above,
`upgrade_safe: false` — read by a reasonable consumer as "v0.3.0's bootloader
changed, app-only upgrade is genuinely unsafe." It hadn't, and it wasn't.
Root cause: **the two hashes were computed over a different number of bytes
of the same underlying content**, not over different content.

`generate-update-meta.sh` hashes the RAW `TARGET_DIR/bootloader.bin` +
`TARGET_DIR/partition-table.bin` cmake build outputs directly (D2's own
text, correctly). But the baseline's "verified" provenance procedure
described above did *not* do that — it manually extracted byte ranges out of
a downloaded, merged/padded image: `bootloader.bin` correctly (0x0..0x5130,
which does happen to equal that raw file's true length), but
`partition-table.bin` as a **160-byte slice** (0x8000..0x80a0 — just the 4
partition entries + 1 MD5-sum trailer, the table's *used* content). The real
raw `partition-table.bin` `gen_esp32part.py` emits — the exact file
`generate-update-meta.sh` hashes — is **3072 bytes** (`0xC00`,
`gen_esp32part.py`'s own `MAX_PARTITION_LENGTH`, 0xFF-padded past the used
content); merge_bin places that whole 3072-byte file at `0x8000` verbatim,
then itself pads *further* with 0xFF out to `nvs`'s `0x9000`. Hashing a
160-byte slice of partition-table content can never equal hashing the real
3072-byte file, for **any** build, layout-changed or not — the baseline was
structurally incapable of ever matching `generate-update-meta.sh`'s live
output, from the moment it was seeded. v0.3.0 wasn't caught by a real
compatibility break; it was the first release the gate ever actually ran
against, so it was the first release to expose a bug that had existed since
the baseline was committed.

**Verification the real layout never changed:** downloaded the actual
published `v0.1.0`, `v0.2.0`, and `v0.3.0` `-merged.bin` release assets and
diffed the full `[0x0, 0x9000)` span (bootloader + partition-table + all
gap-fill up to `nvs`) byte-for-byte across all three — identical. Separately,
reproduced the real `v0.3.0` release build end-to-end (`docker build` +
`docker run` against the pinned release container, tag `v0.3.0`, real
`SOURCE_DATE_EPOCH`) and confirmed its `generate-update-meta.sh` output
reproduces the exact `011e0de9..` value CI published — not a fluke, and not
a genuine layout break; a real, deterministic build output, just compared
against a baseline that was never computable to match it.

**Fix:** `firmware/layout-baseline.txt` is re-seeded to `011e0de9..` —
derived by actually running `generate-update-meta.sh` against a real build's
raw outputs, per the corrected baseline-derivation rule now in that file's
own header comment (never hand-slice a merged image again). This is
verified to restore `upgrade_safe:true` for an (still) unchanged layout —
confirmed by re-running `generate-update-meta.sh` against the real
reproduced `v0.3.0` build outputs and the corrected baseline.

**v0.3.0's already-published `update-meta.json` is not retroactively
edited by this correction.** It was built, checksummed, and attested by
`release.yml` as `upgrade_safe:false`; hand-patching a live release asset
outside that pipeline would satisfy the JSON content but leave its
`SHA256SUMS` entry and `actions/attest-build-provenance` attestation stale —
exactly the invariant D5 exists to prevent. v0.2.0 users therefore still see
"Fresh install only" for the v0.3.0 hop specifically; this correction's
effect is forward from the next tagged release built at or after this
commit (recommended: a `v0.3.1` patch release, cut through the normal
tag-triggered pipeline, so the corrected gate goes live with a properly
attested `update-meta.json`) — not a mutation of a past one. See the owning
mission's dossier for the full incident record.

### D3 — `manifest-update.json`: esp-web-tools manifest shaped for a non-erasing flash

A second esp-web-tools manifest, `manifest-update.json`, generated in
`.github/workflows/release.yml` (text-only, no binary access needed — same
treatment as the existing `manifest.json`):

```json
{
  "name": "MeshCadet (Upgrade)",
  "version": "vX.Y.Z",
  "new_install_prompt_erase": false,
  "builds": [
    {
      "chipFamily": "ESP32-S3",
      "parts": [
        { "path": "meshcadet-vX.Y.Z-app.bin", "offset": 65536 }
      ]
    }
  ]
}
```

Single part, offset `65536` (`0x10000`, the `factory` partition offset —
`firmware/partitions.csv`), `chipFamily: "ESP32-S3"` matching `manifest.json`.
`new_install_prompt_erase: false` is set so the manifest *shape* does not
itself force a full-chip erase prompt — this is necessary but not
sufficient for a genuinely non-destructive write: whether
`<esp-web-install-button>` can be constrained to a non-erasing, single-part
write end to end (or needs an esptool-js fallback) is the load-bearing
unknown the site-flasher child resolves live (plan D4) — this ADR only
freezes the manifest contract that resolution consumes, not the resolution
itself.

### D4 — `update-meta.json`: machine-readable compatibility metadata

A small JSON asset, written by `build.sh` (where the raw bootloader/
partition-table bytes are available) and published unmodified by
`release.yml`:

```json
{
  "version": "vX.Y.Z",
  "layout_hash": "<sha256 hex of bootloader.bin || partition-table.bin>",
  "layout_baseline": "<sha256 hex from firmware/layout-baseline.txt>",
  "upgrade_safe": true,
  "app_asset": "meshcadet-vX.Y.Z-app.bin",
  "app_offset": 65536
}
```

`layout_baseline` is echoed (not just `upgrade_safe`) so a consumer — or a
human debugging a mis-gated release — can see which committed value a given
release's `layout_hash` was actually compared against, without needing a
separate checkout. `app_asset`/`app_offset` are included so the site child
does not need to hardcode the app-asset naming convention or offset
separately from this file — `update-meta.json` alone is sufficient to
locate and gate the update artifact.

### D5 — Checksums + provenance cover all six assets

`SHA256SUMS` and the `actions/attest-build-provenance` subject list both
extend from the original three assets (merged image, `manifest.json`,
`SHA256SUMS` itself is not self-listed as a provenance subject, obviously)
to all six published assets: `meshcadet-vX.Y.Z-merged.bin`,
`meshcadet-vX.Y.Z-app.bin`, `manifest.json`, `manifest-update.json`,
`update-meta.json`, `SHA256SUMS`. No asset is published without both a
checksum entry and a provenance attestation — the same invariant ADR-0004
§8 established for the original three, just widened.

## Consequences

- A layout-changing release publishes `app.bin`/`manifest-update.json`/
  `update-meta.json` unconditionally (the build always produces an app
  image), but with `upgrade_safe:false` — the gate lives entirely in that
  boolean, not in asset presence/absence. This is simpler to build and
  reason about than conditional asset omission (no "was this asset skipped
  on purpose or did the step fail?" ambiguity for either build.sh or a
  consumer), at the cost of relying on every consumer to actually check the
  flag rather than inferring safety from what got published. The
  site-flasher child (ADR-0006) is the load-bearing enforcement point — it
  MUST NOT offer Upgrade for `upgrade_safe:false`.
- `firmware/layout-baseline.txt` is a manually-owned invariant, same shape
  as ADR-0006 §4's `MAX_VERSIONS` pairing: nothing mechanically forces a
  layout-changing PR to also bump it. A missed bump would make a real
  layout change compute `upgrade_safe:true` incorrectly — the highest-risk
  failure mode this ADR introduces. Mitigated today by `build.sh` failing
  loudly on a missing/empty baseline file (can't silently skip the gate
  entirely) and by this ADR's explicit ownership note; revisit with an
  automated partitions.csv-hash-vs-baseline CI check if a layout change ever
  lands without the bump in practice.
- The merged/Fresh-install image, `manifest.json`, and their build path are
  byte-identical to ADR-0004 §7 — this ADR is additive only. Existing
  third-party reproducers following `docs/release-reproducibility.md`'s
  merged-image recipe are unaffected.
- `docs/release-reproducibility.md` is updated to describe the widened
  six-asset provenance/checksum surface, keeping that doc's "describes
  exactly what release.yml does" promise (ADR-0004 §8) accurate.
- **Verified no breakage of ADR-0006's existing site mirror:**
  `.github/workflows/pages-deploy.yml`'s "Mirror recent release firmware
  assets" step downloads with three explicit `--pattern` flags
  (`manifest.json`, `meshcadet-*-merged.bin`, `SHA256SUMS`) and never runs a
  `sha256sum -c` against the mirrored `SHA256SUMS` — so the three new assets
  this ADR adds are simply invisible to it (the `-app.bin` suffix does not
  match the `-merged.bin` glob) until the site-flasher child explicitly adds
  patterns for `manifest-update.json`/`update-meta.json`/the app image. The
  existing Fresh-install-only flasher keeps working unmodified.

## Alternatives Considered

### A. OTA-partition redesign

See D1 — rejected: repartitioning is itself a one-time erasing migration,
and this project has no on-device self-update capability to justify the
permanent app-space cost.

### B. Compute `upgrade_safe` against the previous release's own metadata (original plan D2)

See D2/R1 — rejected: no release before this ADR ever emitted a
`layout_hash`, so there is no predecessor value for the first app-only
release to compare against; a committed baseline is deterministic and needs
no mid-build GitHub API round-trip.

### C. Device-probed compatibility (read the installed layout from the browser)

Not attempted. A browser flashing over Web Serial cannot reliably read an
arbitrary flash region's contents back into a structured comparison without
significant added complexity (reading raw flash bytes, parsing the
partition-table format client-side, handling read failures) for a gate that
release-time metadata already answers deterministically for every case that
matters (was this device provisioned from a compatible release or not). The
gate is release-metadata-driven + user-attested by design (site-flasher
child, ADR-0006) — not because device-probing is impossible, but because
it's unnecessary complexity for the actual failure mode being guarded
against (a user attempting to Upgrade across a layout-incompatible jump).

### D. Omit the app-only assets entirely on a layout-changing release

Considered per the owning mission's scope note ("MAY omit the app-only
asset"). Rejected in favor of always publishing + gating on
`upgrade_safe:false` (D2/Consequences) — conditional asset omission adds a
second signal (presence) that has to stay consistent with the boolean, for
no benefit: any consumer correctly checking `upgrade_safe` behaves
identically either way, and a consumer that ISN'T checking it is equally
unsafe whether or not the asset exists.
