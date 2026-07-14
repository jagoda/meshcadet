# ADR-0011 — Unified esptool-js Flasher: Fresh Install Drops `<esp-web-install-button>`

- **Status:** Accepted (2026-07-14)
- **Deciders:** Maintainer design review (`meshcadet-flasher-metadata-and-flash-status` mission)
- **Supersedes:** ADR-0006's decision that Fresh install uses
  `<esp-web-install-button>`; amends ADR-0009 D6's "Fresh-install code path
  ... unmodified" consequence.
- **Extends:** ADR-0009 (Two-Path Web Flasher), whose D5 (a hand-rolled
  esptool-js flow for Upgrade) this ADR generalizes to both paths.
- **Code:** `site/flash.html`, `site/flash.js`, `site/flash-manifest.js`
  (+ `site/flash-manifest.test.mjs`), `site/styles.css` (`.flash-status-bar`/
  `.flash-console`/`.flash-console-log` rules), `.github/workflows/
  pages-check.yml` (new test step).

## Context

Two defects were reported against the live site (`meshcadet-flasher-
metadata-and-flash-status` mission):

1. Selecting the newest release (v0.3.0) showed "Upgrade metadata isn't
   available" — traced to a *different* file entirely
   (`.github/workflows/pages-deploy.yml`'s broken `release: published`
   trigger, see ADR-0006 §3a's amendment) and fixed there, not in the
   flasher. Included here only because it's the sibling defect in the same
   mission.
2. **"Connect & Install" (the Fresh-install button) connects to the device
   but never flashes.** Root cause, verified live by fetching and inspecting
   both the URL flash.html actually used and upstream's documented CDN tag:

   - `flash.html` imported
     `https://unpkg.com/esp-web-tools@10/dist/install-button.js` (no
     `?module`). Upstream's own documented CDN usage
     (https://esphome.github.io/esp-web-tools/) is
     `dist/web/install-button.js?module` — a *different* file, in a
     `dist/web/` subdirectory that doesn't exist at the path flash.html
     used, requesting unpkg's `?module` bare-specifier-rewriting mode.
   - `dist/install-button.js` (the wrong path) is the bundler-target build:
     its `connect()` function does `import { connect } from "./connect"` →
     `./connect.js` does `import("./install-dialog.js")` (unawaited, no
     `.catch`) → `install-dialog.js` itself has *unresolved bare
     specifiers* (`import ... from "lit"`, `"tslib"`,
     `"improv-wifi-serial-sdk/dist/serial"`, `"improv-wifi-serial-sdk/dist/const"`)
     that a plain `<script type="module">` with no import map cannot
     resolve. `dist/web/install-button.js?module` is the OTHER build
     target — pre-bundled with code-split, purely-relative (or unpkg-
     `?module`-rewritten) chunk imports, meant for exactly this drop-in
     `<script>`-tag use case.
   - Because the failing `import()` is fire-and-forget, the failure is
     silent end to end: `connect()` still calls
     `navigator.serial.requestPort()` and `port.open()` — both succeed, so
     the browser visibly "connects" (port picker, OS-level pairing) — but
     the dynamically-imported module that defines the `ewt-install-dialog`
     custom element never finishes evaluating. `document.createElement
     ("ewt-install-dialog")` then creates an inert, undefined element,
     appended to `document.body`, that does nothing. No dialog, no error
     surfaced to the user, no flash. This matches the reported symptom
     exactly.

   Fixing the CDN URL alone (swap to `dist/web/install-button.js?module`)
   would resolve the immediate "never flashes" bug. It was NOT the option
   taken here — see Decision below.

Separately, the mission's acceptance criteria required "a default status bar
... showing current phase" and "an expandable console ... on demand" for
flash operations. `<esp-web-install-button>`'s own dialog (once its CDN
import is fixed) DOES have an internal phase-labeled progress view and a
"Logs & Console" screen — but both live inside its own black-box shadow-DOM
modal, with no public event API this page can hook to drive a page-level
status bar / console of its own. ADR-0009 D4 already established the
precedent for exactly this class of problem (needing more control than
`<esp-web-install-button>` exposes) by building a hand-rolled esptool-js flow
for the Upgrade path.

## Decision

**Fresh install moves onto the same hand-rolled esptool-js flow the Upgrade
path already used (ADR-0009 D5), rather than fixing the CDN URL and keeping
`<esp-web-install-button>`.** `site/flash.js` no longer imports esp-web-tools
at all; `flash.html`'s esp-web-tools `<script>` tag and the
`<esp-web-install-button>` element are both removed.

Reasons, in order of weight:

1. **A real page-level status bar + console requires it.** There is no
   supported way to get granular phase/progress events out of
   `<esp-web-install-button>` short of forking its internals or polling its
   shadow DOM — both worse than owning the ~80 lines of esptool-js
   orchestration directly, which is exactly what the Upgrade path already
   proved out.
2. **It eliminates the entire bug class, not just this instance.** A CDN-URL
   fix pins correctness to unpkg continuing to serve `dist/web/` at the
   documented path/query indefinitely; owning the flash sequence directly
   means this page depends on nothing but the same pinned `esptool-js`
   bundle the Upgrade path already verified has no unresolved imports.
3. **Consistency.** Two flash paths, one shared mechanism (`ESPLoader`/
   `Transport`, same construction, same status/console plumbing) is less
   code to reason about than one path on a vendor web component and one on a
   hand-rolled flow, and makes the new `site/flash-manifest.test.mjs` /
   existing `site/upgrade-gate.test.mjs` regression coverage exercise the
   same kind of "which bytes go to which address" decision for both paths.

### Fresh vs. Upgrade: the one real difference

`runFreshFlash` mirrors `runUpgradeFlash` (ADR-0009 D5's structure:
`requestPort` → download asset(s) → `ESPLoader`/`Transport` → chip-family
check → `writeFlash` → reset/disconnect) with exactly one behavioral
difference, matching each path's stated contract:

- Fresh: `eraseAll: true` (full-chip erase — "erases and reinstalls the
  whole chip", unchanged from what `<esp-web-install-button>` did once its
  Install click reached `_startInstall(true)`), parts resolved from
  `manifest.json`'s `builds[].parts` (chip-family-matched, shape-validated
  by `flash-manifest.js`'s `resolveFreshInstallParts`, mirroring
  `upgrade-gate.js`'s fail-closed contract for `update-meta.json`).
- Upgrade: `eraseAll: false`, single part from `update-meta.json`'s
  `app_asset`/`app_offset` (unchanged from ADR-0009 D5).

### Shared status bar + expandable console

One page-level UI (`#flash-status-bar` / `#flash-console`), driven by both
paths (only one can run at a time — `setFlashControlsLocked` already
enforced mutual exclusion for Upgrade; extended to lock the Fresh button
too), rather than per-panel duplicates:

- `setFlashPhase(phase)` updates the status bar's headline text AND appends
  a timestamped line to the console — used for discrete transitions
  ("Requesting device access…", "Connecting to device…", "Erasing and
  writing firmware…", "Resetting device…", "Done").
- `setStatusText(text)` updates only the status bar — used for the
  high-frequency write-percentage ticks `reportProgress` fires, so a
  multi-megabyte write doesn't flood the console with one line per chunk;
  `makeProgressReporter` throttles console logging to every ~10 percentage
  points.
- `setFlashError(message)` is `setFlashPhase` plus auto-expanding the
  console `<details>` — a failure is exactly the moment a user most needs
  the detailed log visible without an extra click.
- The console `<details>` is always present but collapsed by default
  ("available on demand, without cluttering the default view" — the
  mission's own acceptance-criteria wording); the status bar is hidden until
  the first phase of a flash attempt.

## Consequences

- **No more Improv Wi-Fi provisioning follow-up screen for Fresh install.**
  `<esp-web-install-button>`'s dashboard offered a "Connect to Wi-Fi" /
  Improv-provisioning step after a successful install. Irrelevant in
  practice: ADR-0009 D4 already established MeshCadet's firmware doesn't
  speak Improv Serial at all (it takes the `_renderDashboardNoImprov`
  branch unconditionally), so this was already a dead code path for every
  real MeshCadet device.
- **No more `<esp-web-install-button>` "Fund Development"/`home_assistant_domain`
  affordances.** Neither `manifest.json` nor `manifest-update.json` sets
  those fields, so both were already inert.
- **`site/README.md`'s "Fresh install... unchanged mechanism" line is now
  stale** and is updated alongside this ADR to describe the unified flow.
- Same hardware-verification caveat as ADR-0009 Consequences: this is a
  **static-but-deterministic, source-level verification** (reading unpkg's
  actual served bytes for both CDN paths, reading esptool-js's `writeFlash`
  semantics) in an environment with no physical T-Deck Plus / interactive
  Web-Serial rig — a human should run one real Fresh install and one real
  Upgrade against actual hardware before leaning on this in the field.
- The mirrored `manifest.json` file's own shape is unchanged (still
  generated by `release.yml`, still esp-web-tools-shaped) — a user who
  prefers to grab it by hand from GitHub Releases and feed it to a
  standalone esp-web-tools install page still can.

## Alternatives Considered

### A. Fix only the CDN URL (`dist/install-button.js` → `dist/web/install-button.js?module`), keep `<esp-web-install-button>` for Fresh

Would have fixed the reported "never flashes" symptom with a one-line
change. Rejected as the sole fix because it leaves the status-bar/console
acceptance criteria unaddressable for the Fresh path (no event hooks — see
Context) and keeps this page's most-used path dependent on a third-party
CDN path/query shape that already proved fragile once.

### B. Fix the CDN URL, and layer a coarse status bar on top by observing DOM mutations on the vendor dialog's shadow root

Technically possible (`MutationObserver` against `ewt-install-dialog`'s
`state` attribute — `install-dialog.js` does set `this.setAttribute("state",
this._state)`) but explicitly rejected: fragile (depends on an
undocumented, version-unpinned internal implementation detail that could
change in any `esp-web-tools@10.x` patch release), and still can't reach
percentage-level write progress (that lives in a `@state` Lit property, not
a reflected attribute). ADR-0009 D4 already rejected reasoning about
esp-web-tools' undocumented internals for a materially similar problem; this
is the same call.
