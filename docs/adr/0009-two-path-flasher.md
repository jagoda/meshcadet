# ADR-0009 — Two-Path Web Flasher: Fresh Install vs Non-Destructive Upgrade

- **Status:** Accepted (2026-07-13)
- **Deciders:** Maintainer design review (`meshcadet-nondestructive-firmware-updates` campaign)
- **Supersedes:** —
- **Extends:** ADR-0006 (Web Flasher: Version Selector Over a Same-Origin
  Mirror), consuming ADR-0008's frozen `manifest-update.json` /
  `update-meta.json` contract.
- **Implements:** the site-flasher child (`meshcadet-flasher-two-path-upgrade`)
  of the `meshcadet-nondestructive-firmware-updates` campaign.
- **Code:** `site/flash.html`, `site/flash.js`, `site/styles.css`
  (`.path-choice`/`.path-option`/`.note-box` rules), `site/provisioner.html`
  (`#history-panel` cross-link target, pre-existing),
  `.github/workflows/pages-deploy.yml` ("Mirror recent release firmware
  assets" step).

A new ADR rather than an ADR-0006 revision, because ADR-0006 is Accepted and
describes a single-path (Fresh-install-only) flasher that remains correct and
unchanged for that path — this piece adds a second, materially different
path (a custom, non-esp-web-tools flash mechanism) rather than revising
ADR-0006's original decision. Keeping ADR-0006 as the historical record of
the original design and layering this ADR on top mirrors how ADR-0008 itself
was written as a new ADR against ADR-0004 rather than an ADR-0004 edit.

## Context

ADR-0008 makes an app-only image (`meshcadet-vX.Y.Z-app.bin`, flashed at
`0x10000`) a published release asset, plus `manifest-update.json` (an
esp-web-tools manifest shaped for a non-erasing write) and `update-meta.json`
(the `upgrade_safe`/`layout_hash` compatibility gate) — but explicitly
defers "whether `<esp-web-install-button>` can be constrained to a
non-erasing, single-part write end to end (or needs an esptool-js fallback)"
to this mission (ADR-0008 D3). This ADR resolves that question (D4) and
implements the two-path UI the campaign's overview asked for.

## Decision

### D1 — Two explicit paths, one shared version selector

`site/flash.html` gains a "3. Choose how to flash" step between the existing
version selector (step 2, unchanged — ADR-0006) and install (step 4): a
radio choice between **Fresh install** (today's behavior, relabeled, kept
byte-for-byte) and **Upgrade**. Selecting a path shows/hides the
corresponding install panel; the version selector itself is not duplicated
(ADR-0006's dropdown drives both paths, per the owning mission's scope).

### D2 — Upgrade-safety gating: `update-meta.json` per selected version

On every version-selector change, `flash.js` fetches
`firmware/<tag>/update-meta.json` (same-origin, mirrored per D5 below) and
enables the Upgrade radio **only** when the fetch succeeds and
`upgrade_safe === true`. Any other outcome — fetch fails/404s (an older,
pre-ADR-0008 release like v0.1.0/v0.2.0, or a mirror-step race), or the
release exists but `upgrade_safe === false` (a layout-changing release,
ADR-0008 D2) — disables the radio, force-selects Fresh install if Upgrade
was previously selected, and shows an inline note explaining which of the
two cases applies. This is the load-bearing enforcement point ADR-0008
D2/Consequences calls out: "the site-flasher child... MUST NOT offer Upgrade
for `upgrade_safe:false`."

Selecting Upgrade for a `upgrade_safe:true` version still shows an honest
caveat in copy (not just gated by the radio's disabled state): "preserves
data only if the device already runs a compatible build" — `upgrade_safe`
attests to the *release's* layout, not to what a specific, unprobed device
in front of the user is actually running (ADR-0008 D2 Alternative C:
device-probing was rejected as unnecessary complexity; the gate is
release-metadata-driven + user-attested by design).

### D3 — Pre-upgrade backup advisory (plan D3), not a hard block

The Upgrade panel's first sub-step links directly to
`provisioner.html#history-panel` ("Read history from device" / "Download
transcript" — pre-existing M2 sensitive-data feature) and requires an
acknowledgment checkbox ("I've backed up my history (or don't need to) —
let me continue") before the Connect & Upgrade button enables. This is
advisory, matching the owning mission's explicit scope constraint — the
checkbox is a friction/reminder gate, not a technical verification that a
backup actually happened (this page cannot know that), and resets on every
version switch so it can't silently carry over.

### D4 — Why `<esp-web-install-button>` cannot be used for Upgrade (the load-bearing finding of this ADR)

**Resolution: the install-button element is disqualified for the Upgrade
path. Upgrade uses a hand-rolled esptool-js flow instead (D5).**

This was determined by reading esp-web-tools' actual source
(`esphome/esp-web-tools`, the exact major version (`@10`) `flash.html`
already CDN-imports) rather than by a live hardware click-through — no
physical T-Deck Plus/browser-with-Web-Serial rig is available in this
execution environment. The reasoning is presented candidly as a
**static-but-deterministic** verification, not a substitute for eventual
hardware-in-the-loop confirmation (see Consequences):

- `src/install-dialog.ts`'s `_initialize()` opens an Improv Serial session
  against the connected device with a short timeout; on failure it sets
  `this._client = null` ("NOT_SUPPORTED") and falls through to
  `_renderDashboardNoImprov()`. MeshCadet's firmware implements its own
  custom provisioning wire protocol (`protocol::provisioning`, ADR-0002) —
  it does not speak Improv Serial at all (grepped for "improv" across the
  firmware tree; the only hits are unrelated substring matches inside the
  word "improve"). Every MeshCadet device therefore takes this branch,
  unconditionally, regardless of the manifest.
- `_renderDashboardNoImprov()`'s Install click handler
  (`install-dialog.ts:317-324`):
  ```ts
  if (this._manifest.new_install_prompt_erase) {
    this._state = "ASK_ERASE";
  } else {
    // Default is to erase a device that does not support Improv Serial
    this._startInstall(true);
  }
  ```
  ADR-0008's frozen `manifest-update.json` sets
  `"new_install_prompt_erase": false` (D3, chosen so the manifest shape
  doesn't force an erase *prompt* — necessary but, this ADR's finding shows,
  not sufficient). For a non-Improv device, `false` takes the `else`
  branch: `_startInstall(true)` — a full chip erase (`esploader.eraseFlash()`
  via `src/flash.ts`'s `eraseFirst` parameter), unconditionally, with **no
  prompt and no way for the user to opt out**. Setting it `true` instead
  would only reach an `ASK_ERASE` checkbox screen — not a fix this site can
  apply unilaterally, since `manifest-update.json`'s shape is ADR-0008's
  frozen, cross-mission contract (a breaking change needs its own ADR
  revision, not a silent edit here).
- This is exactly the trigger condition the owning mission's scope
  anticipated: "If the install-button auto-erases a device it doesn't
  recognize, fall back to a custom esptool-js flash flow that writes the app
  region only." The finding above proves that trigger fires for every
  MeshCadet device, deterministically — it is pure JS control flow with no
  hardware-timing dependency, which is why reading it settles the question
  without needing to physically execute it.

### D5 — The Upgrade path: a hand-rolled esptool-js flow

`site/flash.js` imports `esptool-js` directly —
`https://unpkg.com/esptool-js@0.5.7/bundle.js`, an ESM bundle with no
unresolved bare imports (verified by fetching and inspecting it), the same
"single pinned CDN import, no bundler" pattern `flash.html` already uses for
esp-web-tools and `provisioner.js` uses for `qrcode` (site/README.md's
no-build-step convention). The exact version (`0.5.7`, not the newer
`0.6.0` published to npm's `latest` tag) is pinned to match what
`esp-web-tools@10.2.1` (the CDN import already backing
`<esp-web-install-button>` on this same page) itself resolves as its
`esptool-js` dependency (`^0.5.7` in its own `package.json`, verified live
against the npm registry) — so both flash paths run the identical
underlying esptool-js write/erase code, eliminating any risk of behavioral
drift between an unpinned or independently-chosen version and the one
already proven to work for Fresh install.

The Upgrade flow (`runUpgradeFlash` in `flash.js`) is a direct trim of
esp-web-tools' own `src/flash.ts` — same `ESPLoader`/`Transport`
construction, same `main()` → `flashId()` → `writeFlash()` → `after()` →
`disconnect()` sequence — with two deliberate differences:

1. **No `<esp-web-install-button>`, no Improv/manifest-dashboard machinery.**
   The button requests a Web Serial port directly
   (`navigator.serial.requestPort()`), downloads exactly one file
   (`update-meta.json`'s `app_asset`, from the same-origin mirror), and
   writes it at exactly one address (`update-meta.json`'s `app_offset`) —
   no manifest-parsing, no chip-family auto-detection beyond a single
   equality check (below).
2. **`eraseAll: false`, always.** Per `esptool-js`'s own `writeFlash`
   implementation (read directly, both the `0.6.0` main-branch source and
   the pinned `0.5.7` tag's `FlashOptions`/`writeFlash` shape, which match):
   `eraseAll` gates a single, separate call to `eraseFlash()` (the full-chip
   erase primitive) *before* the per-file write loop begins; the write loop
   itself only ever calls `flashBegin`/`flashDeflBegin` for the address
   range each file covers, which — this being how esptool's underlying wire
   protocol works — erases only the flash sectors about to be overwritten.
   With `eraseAll: false` and a single `{data, address: 0x10000}` file, the
   only flash sectors ever touched are the ones under the `factory`
   partition; `nvs`@0x9000 and `mc_hist`@0x610000 are never in the write
   path at all, let alone erased.

A chip-family check (`esploader.chip.CHIP_NAME === "ESP32-S3"`, matching
both manifests' `chipFamily`) runs immediately after connecting and before
any bytes are written, refusing with a clear error rather than writing an
app image built for the wrong chip onto whatever's plugged in.

### D6 — Site mirror extended for the three new assets

`.github/workflows/pages-deploy.yml`'s "Mirror recent release firmware
assets" step gains a second, **best-effort** `gh release download` call per
tag (`manifest-update.json`, `meshcadet-*-app.bin`, `update-meta.json`) —
deliberately not folded into the existing required-assets download, and
deliberately not treated as a mirror failure if it matches nothing: a
release published before ADR-0008 landed (v0.1.0, v0.2.0) legitimately has
none of these three assets, and that is the expected, common case this
project will keep hitting for its two oldest releases, not a defect worth
an `::warning::`. `flash.js`'s D2 gating already treats "not mirrored" and
"mirrored but `upgrade_safe:false`" identically (Fresh install only), so no
second signal is needed here. The existing `MAX_VERSIONS`/`max_versions`
manual invariant (ADR-0006 §4) is unchanged — the same per-tag cap governs
both the required and the optional download call.

## Consequences

- **D4's verification is source-level, not hardware-in-the-loop.** This
  execution environment has no physical T-Deck Plus or interactive
  browser+Web-Serial rig available to click through a real flash. The
  finding above is decisive because the erase decision is deterministic
  control flow (device-Improv-support × manifest field, both known ahead of
  time for MeshCadet), not a timing- or hardware-dependent runtime
  behavior — but a human should still run one real Upgrade flash against
  actual hardware before leaning on this in the field, and confirm (e.g.
  via the provisioner's status panel) that `mc_id`/contacts/history survive
  it. Flagged explicitly rather than silently asserted as fully verified.
- Bypassing `<esp-web-install-button>` for Upgrade means the Upgrade path
  gets none of esp-web-tools' polish for free — no Improv Wi-Fi
  provisioning step (irrelevant here; MeshCadet doesn't support Improv
  regardless of path), no built-in retry/backoff UI beyond what `flash.js`
  implements itself. The trade is a page whose only claim about "what
  bytes get erased" is one this project's own code makes and controls,
  rather than depending on a third-party library's Improv-support branching
  to happen to land on the safe side.
- Because the Upgrade path never calls `esploader.eraseFlash()`, a device
  whose `factory` partition holds meaningfully different data than what a
  correctly-`upgrade_safe`-gated new app expects (e.g. a partially-written
  previous flash attempt) is not restored to a clean slate before write —
  `esptool`'s block-level erase-as-written behavior only guarantees the
  bytes actually covered by the new image end up correct, not that stale
  bytes beyond the new image's length (if the new image is ever shorter
  than a previous one at the same offset) are cleared. Not a new risk this
  ADR introduces — the `factory` partition is sized to the full 6 MB region
  regardless of app length (ADR-0008 D1), so this is a pre-existing
  property of the underlying esptool write primitive, not something specific
  to the two-path design.
- **A device disconnected or power-lost mid-Upgrade-write is not bricked.**
  Because the write path never touches the bootloader or partition-table
  regions (D5 — `eraseAll: false` plus a single `{data, address: 0x10000}`
  part), an interrupted Upgrade can leave the `factory` app partition
  corrupted/non-bootable, but the bootloader, partition table, `nvs`, and
  `mc_hist` all remain exactly as they were before the attempt — the device
  is always recoverable via another Upgrade attempt or Fresh install, never
  left in a state where nothing can be written to it again. This is a
  strictly safer failure mode than an interrupted Fresh install (which can
  leave the bootloader/partition-table region itself half-written), and is
  the reason `runUpgradeFlash`'s own error message tells the user it's
  "safe to retry" rather than only pointing at Fresh install.
- The two-path UI, the update-meta.json gate, the backup advisory, and the
  mirror step are all additive; `site/flash.js`'s existing Fresh-install
  code path (manifest.json → `<esp-web-install-button>`) is unmodified
  except for being relabeled/moved into its own panel — a visitor who
  never touches the new "Upgrade" radio sees byte-identical Fresh-install
  behavior to before this ADR.

## Alternatives Considered

### A. Tune `new_install_prompt_erase` to `true` and rely on esp-web-tools' `ASK_ERASE` checkbox

Rejected per D4: `manifest-update.json`'s shape is ADR-0008's frozen,
cross-mission contract — this mission cannot silently change it. Even
setting it aside, `ASK_ERASE` requires the user to actively uncheck an
"Erase device" checkbox that defaults unchecked-by-omission only if they
notice it (real risk of an inattentive user erasing anyway) — a hand-rolled
flow that simply never offers an erase option for Upgrade is strictly safer
by construction.

### B. Device-side layout probing before allowing Upgrade

Already rejected at the ADR-0008 layer (D2 Alternative C) and not reopened
here: reading raw flash bytes back over Web Serial and parsing the
partition-table format client-side is significant added complexity for a
gate release-time metadata (`upgrade_safe`) plus a user-facing caveat
already answers for the failure mode that matters (a user attempting to
Upgrade across a layout-incompatible jump).

### C. Skip the custom esptool-js flow; keep Upgrade Fresh-install-only pending a future esp-web-tools fix

Would satisfy "ship something" but defeats the campaign's entire premise
(non-destructive updates) and contradicts the owning mission's explicit
scope, which anticipated and pre-authorized exactly the fallback this ADR
implements. Not pursued.
