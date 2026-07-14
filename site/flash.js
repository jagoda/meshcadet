// flash.js — two-path web flasher for site/flash.html: Fresh install
// (merged image, erases) vs Upgrade (app-only image, non-erasing). Both
// paths (runFreshFlash / runUpgradeFlash below) drive the same hand-rolled
// esptool-js mechanics and the same shared status bar + expandable console
// — see docs/adr/0011-unified-esptool-js-flasher.md for why (both the
// flash-mechanism bug and the status-bar/console requirement made keeping
// esp-web-tools' <esp-web-install-button> for Fresh untenable). Extends
// docs/adr/0006-web-flasher.md (version
// selector over a same-origin mirror) and docs/adr/0009-two-path-flasher.md
// (the two-path split itself, and the reasons Upgrade specifically can never
// use install-button), and consumes docs/adr/0008-nondestructive-update-
// artifacts.md's frozen manifest-update.json / update-meta.json contract.
//
// Three data sources, deliberately different origins:
//
//   1. The release LIST comes from a live client-side fetch of the GitHub
//      REST API (api.github.com), which sends `Access-Control-Allow-Origin:
//      *` and so is fetchable cross-origin from a Pages page. This is what
//      keeps the dropdown always up to date with no redeploy needed.
//
//   2. The flashable MANIFESTS/BINARIES are never fetched from GitHub
//      Releases directly — verified live (curl against a real GitHub
//      release asset) that the release-download redirect chain carries no
//      Access-Control-Allow-Origin header at all, so browser fetch() of a
//      Release asset from this Pages origin is CORS-blocked (ADR-0006).
//      Instead, .github/workflows/pages-deploy.yml mirrors each recent
//      release's assets into site/firmware/<tag>/ at deploy time, and both
//      flash paths read from that same-origin copy. NOTE: that mirror step
//      only runs on a site/**-touching push OR a workflow_run completion of
//      release.yml (ADR-0006 §3a amendment) — a release can briefly exist on
//      GitHub before its assets are mirrored onto this Pages deployment; a
//      404 on firmware/<tag>/manifest.json or update-meta.json means "not
//      mirrored yet", not "doesn't exist" or "not offered by design".
//
//   3. update-meta.json (mirrored alongside the rest) carries the
//      layout-compatibility gate (ADR-0008 D2/D4): `upgrade_safe` decides
//      whether the Upgrade path is even offered for a given version, and
//      `app_asset`/`app_offset` tell this page which file to write and
//      where, without hardcoding either.
//
// No build step (site/README.md convention) — plain ES module JS loaded
// directly by the browser, no bundler.

// esptool-js: the ONLY flash-mechanism dependency this page has (ADR-0011 —
// esp-web-tools is no longer imported at all). Pinned to an ESM bundle with
// no unresolved bare imports (verified by fetching and inspecting it —
// esp-web-tools' own default CDN import path is NOT similarly self-
// contained; see ADR-0011's Context for the live-verified failure mode that
// finding closes off). Bundle exports `ESPLoader`/`Transport`, the same two
// classes esp-web-tools' own src/flash.ts drives internally.
import { ESPLoader, Transport } from "https://unpkg.com/esptool-js@0.5.7/bundle.js";
import { isValidUpdateMeta } from "./upgrade-gate.js";
import { resolveFreshInstallParts } from "./flash-manifest.js";
import { ui8ToBstr } from "./flash-image-encoding.js";

const REPO = "jagoda/meshcadet";
const RELEASES_API = `https://api.github.com/repos/${REPO}/releases`;

// Keep this equal to the mirror cap in .github/workflows/pages-deploy.yml's
// "Mirror recent release firmware assets" step — there is no shared build
// step to enforce that automatically, so a version beyond this count is
// guaranteed to exist on GitHub but NOT guaranteed to be mirrored onto
// Pages. If you change one, change the other.
const MAX_VERSIONS = 8;

// Only tags release.yml actually builds (v*.*.* triggers it) are real
// firmware releases; anything else on the releases list (there shouldn't
// be anything else, but defense in depth) is filtered out.
const VERSION_TAG_RE = /^v\d+\.\d+\.\d+$/;

// The chip family both manifest.json and manifest-update.json declare
// (.github/workflows/release.yml) — checked live against the connected
// device before either flash path ever writes a byte, so an image built for
// the wrong chip is refused rather than blindly written.
const EXPECTED_CHIP_FAMILY = "ESP32-S3";

const select = document.getElementById("version-select");
const status = document.getElementById("version-status");

const pathFreshRadio = document.getElementById("path-fresh");
const pathUpgradeRadio = document.getElementById("path-upgrade");
const upgradeSafetyNote = document.getElementById("upgrade-safety-note");
const freshPanel = document.getElementById("fresh-panel");
const upgradePanel = document.getElementById("upgrade-panel");
const freshUnsupported = document.getElementById("fresh-unsupported");
const freshConnectButton = document.getElementById("fresh-connect-button");
const upgradeUnsupported = document.getElementById("upgrade-unsupported");
const upgradeBackupAck = document.getElementById("upgrade-backup-ack");
const upgradeConnectButton = document.getElementById("upgrade-connect-button");

const flashStatusBar = document.getElementById("flash-status-bar");
const flashStatusPhase = document.getElementById("flash-status-phase");
const flashConsole = document.getElementById("flash-console");
const flashConsoleLog = document.getElementById("flash-console-log");

// Web Serial feature/context gate for both flash paths' custom flow (neither
// goes through <esp-web-install-button> — ADR-0011). Mirrors
// provisioner/session.js's ProvisionerSession static checks.
const WEB_SERIAL_SUPPORTED = "serial" in navigator && window.isSecureContext === true;

let currentUpdateMeta = null; // null = not upgrade_safe (missing, or fetch failed)
let releasesAvailable = false; // true once renderReleases populates real options — gates both connect buttons the same way select.value does for runUpgradeFlash
let flashBusy = false; // true while EITHER flash path is actively writing

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function manifestUrlFor(tag) {
  // Relative, no leading slash — this is a project Pages site
  // (/meshcadet/...), see site/README.md "Conventions".
  return `firmware/${tag}/manifest.json`;
}

function updateMetaUrlFor(tag) {
  return `firmware/${tag}/update-meta.json`;
}

function setStatus(message) {
  status.textContent = message;
}

// ── Shared flash status bar + expandable console ────────────────────────────
//
// One shared UI for both paths (only one can ever be running at a time —
// setFlashControlsLocked below enforces that), rather than a duplicated pair
// of per-panel status lines: a single "what's happening right now" surface
// that stays out of the way (collapsed console, hidden status bar) until a
// flash operation actually starts.

function logToConsole(line) {
  const timestamp = new Date().toISOString().slice(11, 19); // HH:MM:SS UTC
  const entry = `[${timestamp}] ${line}`;
  flashConsoleLog.textContent =
    flashConsoleLog.textContent && flashConsoleLog.textContent !== "No flash operation has run yet."
      ? `${flashConsoleLog.textContent}\n${entry}`
      : entry;
  flashConsoleLog.scrollTop = flashConsoleLog.scrollHeight;
}

// Updates only the status bar's headline text — used for high-frequency
// updates (write-progress percentage) that would otherwise flood the
// console with a line per chunk.
function setStatusText(text) {
  flashStatusBar.hidden = false;
  flashStatusPhase.textContent = text;
}

// A discrete phase transition: updates the status bar AND records a
// timestamped console line (e.g. "Connecting to device…", "Erasing and
// writing firmware…", "Done"). Use setStatusText instead for continuous
// progress-percentage churn within one phase.
function setFlashPhase(phase) {
  setStatusText(phase);
  logToConsole(phase);
}

// A failure: same as setFlashPhase, but also expands the console so the
// detailed log (including whatever line preceded the failure) is
// immediately visible rather than requiring the user to notice and open it.
function setFlashError(message) {
  setFlashPhase(message);
  flashConsole.open = true;
}

function resetFlashConsole() {
  flashConsoleLog.textContent = "";
}

// Builds a throttled esptool-js reportProgress callback: updates the status
// bar on every call (cheap), but only logs a console line every ~10
// percentage points (or the two ends) so a multi-megabyte write doesn't
// flood the console with one line per chunk.
function makeProgressReporter(prefix, totalParts) {
  let lastLoggedPct = -10;
  return (fileIndex, written, total) => {
    const pct = total ? Math.floor((written / total) * 100) : 0;
    const partSuffix = totalParts > 1 ? ` (part ${fileIndex + 1}/${totalParts})` : "";
    const text = `${prefix}: ${pct}%${partSuffix}`;
    setStatusText(text);
    if (pct - lastLoggedPct >= 10 || pct === 100) {
      lastLoggedPct = pct;
      logToConsole(text);
    }
  };
}

// ── Path choice (Fresh install vs Upgrade) ─────────────────────────────────

function applyPathChoice() {
  const upgrade = pathUpgradeRadio.checked;
  freshPanel.hidden = upgrade;
  upgradePanel.hidden = !upgrade;
  updateFreshConnectEnabled();
  updateUpgradeConnectEnabled();
}

function updateFreshConnectEnabled() {
  freshConnectButton.disabled = flashBusy || !WEB_SERIAL_SUPPORTED || !releasesAvailable;
}

function updateUpgradeConnectEnabled() {
  upgradeConnectButton.disabled =
    flashBusy ||
    !WEB_SERIAL_SUPPORTED ||
    !upgradeBackupAck.checked ||
    !isValidUpdateMeta(currentUpdateMeta);
}

// Locks the version selector and path choice while a flash write is in
// flight — switching versions or paths mid-write wouldn't affect the
// in-progress esptool-js session (its tag/meta are captured in local
// variables at the start of runFreshFlash/runUpgradeFlash, not re-read from
// shared state), but it would leave the visible UI (dropdown, radios, the
// OTHER path's connect button) in a confusing, actionable-looking state
// while a serial port is actively in use — e.g. clicking "Connect & Upgrade"
// while a Fresh install is still running would contend for the same
// physical port. Simpler to just disable the choice than to reason about two
// concurrent esptool-js sessions.
function setFlashControlsLocked(locked) {
  select.disabled = locked || !releasesAvailable;
  pathFreshRadio.disabled = locked;
  pathUpgradeRadio.disabled = locked || !isValidUpdateMeta(currentUpdateMeta);
}

pathFreshRadio.addEventListener("change", applyPathChoice);
pathUpgradeRadio.addEventListener("change", applyPathChoice);
upgradeBackupAck.addEventListener("change", updateUpgradeConnectEnabled);

// ── update-meta.json: the Upgrade-availability gate (ADR-0008 D2/D4) ───────

function applyUpdateMeta(meta) {
  currentUpdateMeta = meta;
  const safe = isValidUpdateMeta(meta);

  pathUpgradeRadio.disabled = !safe;
  if (safe) {
    upgradeSafetyNote.hidden = true;
  } else {
    upgradeSafetyNote.hidden = false;
    upgradeSafetyNote.textContent =
      meta === null
        ? "Upgrade metadata isn't available for this version (an older release published before Upgrade support, or the site mirror hasn't caught up yet) — Fresh install only."
        : "This version isn't offered for Upgrade (a layout-changing release, or its update metadata doesn't match what this page expects) — use Fresh install.";
  }

  // A version switch can invalidate a previously-safe Upgrade selection —
  // fall back to Fresh rather than leaving a disabled-but-still-selected
  // radio.
  if (!safe && pathUpgradeRadio.checked) {
    pathFreshRadio.checked = true;
  }
  applyPathChoice();

  // Reset the advisory gate on every version switch — an acknowledgment for
  // one version's backup reminder shouldn't silently carry over and skip
  // the reminder for a different version.
  upgradeBackupAck.checked = false;
  updateUpgradeConnectEnabled();
}

async function loadUpdateMeta(tag) {
  let meta = null;
  try {
    const response = await fetch(updateMetaUrlFor(tag));
    if (response.ok) {
      meta = await response.json();
    }
  } catch (err) {
    // Network failure fetching the metadata — treated the same as "not
    // mirrored" (Fresh install only); logged for debugging.
    console.error("MeshCadet flasher: failed to fetch update-meta.json", err);
  }
  applyUpdateMeta(meta);
}

// ── Version selector ────────────────────────────────────────────────────────

function selectVersion(tag) {
  updateFreshConnectEnabled();
  loadUpdateMeta(tag);
}

function renderEmpty(message) {
  select.innerHTML = "";
  const option = document.createElement("option");
  option.textContent = message;
  select.append(option);
  select.disabled = true;
  releasesAvailable = false;
  updateFreshConnectEnabled();
  applyUpdateMeta(null);
}

function renderReleases(releases) {
  if (releases.length === 0) {
    renderEmpty("No published releases yet");
    setStatus(
      "MeshCadet hasn't cut a tagged release yet — check back soon, or " +
        "build from source (see the project README)."
    );
    return;
  }

  select.innerHTML = "";
  for (const release of releases) {
    const option = document.createElement("option");
    option.value = release.tag_name;
    option.textContent = release.name?.trim() || release.tag_name;
    select.append(option);
  }
  select.disabled = false;
  releasesAvailable = true;
  select.value = releases[0].tag_name;
  selectVersion(releases[0].tag_name);
  setStatus(`Showing the ${releases.length} most recent release${releases.length === 1 ? "" : "s"}.`);

  select.addEventListener("change", () => {
    selectVersion(select.value);
  });
}

async function loadReleases() {
  let response;
  try {
    response = await fetch(RELEASES_API, {
      headers: { Accept: "application/vnd.github+json" },
    });
  } catch (err) {
    // Logged for anyone debugging via devtools; the on-page status message
    // below is the actionable, non-technical version for everyone else.
    console.error("MeshCadet flasher: failed to reach the GitHub releases API", err);
    renderEmpty("Couldn't reach GitHub");
    setStatus(
      "Couldn't reach the GitHub API to list releases (offline, or a " +
        "network/CORS issue) — reload to retry, or get firmware directly " +
        "from the GitHub Releases page linked below."
    );
    return;
  }

  if (!response.ok) {
    console.error(
      `MeshCadet flasher: GitHub releases API returned ${response.status} ${response.statusText}`
    );
    renderEmpty("Couldn't load releases");
    setStatus(
      response.status === 403
        ? "GitHub API rate limit hit — reload in a bit, or get firmware " +
            "directly from the GitHub Releases page linked below."
        : `GitHub API returned ${response.status} — reload to retry, or ` +
            "get firmware directly from the GitHub Releases page linked below."
    );
    return;
  }

  const releases = (await response.json())
    .filter((r) => !r.draft && !r.prerelease && VERSION_TAG_RE.test(r.tag_name))
    .sort((a, b) => new Date(b.published_at) - new Date(a.published_at))
    .slice(0, MAX_VERSIONS);

  renderReleases(releases);
}

// ── Fresh-install path: hand-rolled esptool-js flow (ADR-0011) ─────────────
//
// Previously <esp-web-install-button>, pointed at manifest.json. Two
// independent reasons moved it onto the same custom esptool-js flow the
// Upgrade path already used (ADR-0009 D5):
//
//   1. THE BUG: flash.html's esp-web-tools CDN import
//      (unpkg.com/esp-web-tools@10/dist/install-button.js, no `?module`) was
//      the wrong URL — upstream's documented tag is
//      dist/web/install-button.js?module (see
//      https://esphome.github.io/esp-web-tools/). The path this page used
//      resolves (./connect → ./install-dialog.js) to unresolved bare
//      specifiers ("lit", "tslib", "improv-wifi-serial-sdk/dist/serial")
//      that a plain browser module loader can't load without an import map
//      — confirmed live by fetching and inspecting both dist trees.
//      connect()'s dynamic import("./install-dialog.js") is never
//      awaited/error-handled, so the failure was silent:
//      navigator.serial.requestPort() + port.open() still succeeded
//      ("connects"), but the ewt-install-dialog custom element was never
//      defined, so nothing ever rendered or flashed — exactly the reported
//      "connects but never flashes" symptom.
//   2. Even with the URL fixed, <esp-web-install-button> exposes no public
//      event hooks for phase/progress state (its own progress UI lives
//      inside a black-box shadow-DOM dialog) — no way to drive this page's
//      own persistent status bar / expandable console from it without
//      forking vendor internals.
//
// See docs/adr/0011-unified-esptool-js-flasher.md for the full decision
// record. The manifest.json file itself is unchanged (still generated by
// release.yml, still mirrored by pages-deploy.yml) — this page just reads
// it directly instead of handing it to a vendor component.

async function runFreshFlash() {
  if (flashBusy) {
    return;
  }
  if (!releasesAvailable) {
    // Defense in depth — the button is disabled in this state already
    // (updateFreshConnectEnabled), so this should be unreachable. Guards
    // against select.value falling back to the placeholder option's text
    // content ("Loading releases…"/"No published releases yet…", not a
    // real tag) when no release is actually selectable.
    setFlashError("No release selected.");
    return;
  }
  const tag = select.value;

  flashBusy = true;
  updateFreshConnectEnabled();
  updateUpgradeConnectEnabled();
  setFlashControlsLocked(true);
  resetFlashConsole();

  // Tracks whether writeFlash actually started — a failure before this point
  // (port access, download, connect, chip-family check) never wrote a byte
  // and is unconditionally safe to retry; a failure at or after this point
  // means the erase already started and the device's bootloader/partition
  // table may now be incomplete (see the catch block below).
  let writeStarted = false;
  let transport;
  try {
    let port;
    try {
      setFlashPhase("Requesting device access…");
      port = await navigator.serial.requestPort();
    } catch (err) {
      setFlashPhase("No device selected.");
      return;
    }

    setFlashPhase(`Downloading manifest for ${tag}…`);
    const manifestResponse = await fetch(manifestUrlFor(tag));
    if (!manifestResponse.ok) {
      throw new Error(`Downloading manifest.json failed: ${manifestResponse.status}`);
    }
    const manifest = await manifestResponse.json();
    const parts = resolveFreshInstallParts(manifest, EXPECTED_CHIP_FAMILY);
    if (!parts) {
      throw new Error(
        `This version's manifest.json doesn't have the expected shape for a ${EXPECTED_CHIP_FAMILY} Fresh install — refusing to guess a flash address.`
      );
    }

    const fileArray = [];
    for (const part of parts) {
      setFlashPhase(`Downloading ${part.path}…`);
      const assetResponse = await fetch(`firmware/${tag}/${part.path}`);
      if (!assetResponse.ok) {
        throw new Error(`Downloading ${part.path} failed: ${assetResponse.status}`);
      }
      // esptool-js's writeFlash wants each part's `data` as a Latin-1
      // binary string, not a Uint8Array — see flash-image-encoding.js's
      // header for why (and for the crash this conversion fixes).
      fileArray.push({
        data: ui8ToBstr(new Uint8Array(await assetResponse.arrayBuffer())),
        address: part.offset,
      });
    }

    setFlashPhase("Connecting to device…");
    transport = new Transport(port);
    const esploader = new ESPLoader({ transport, baudrate: 115200, enableTracing: false });
    await esploader.main();
    await esploader.flashId();

    const chipFamily = esploader.chip.CHIP_NAME;
    if (chipFamily !== EXPECTED_CHIP_FAMILY) {
      throw new Error(
        `Connected device reports "${chipFamily}", expected "${EXPECTED_CHIP_FAMILY}" — refusing to write a firmware image built for a different chip.`
      );
    }

    setFlashPhase("Erasing and writing firmware — do not disconnect…");
    writeStarted = true;
    await esploader.writeFlash({
      fileArray,
      flashSize: "keep",
      flashMode: "keep",
      flashFreq: "keep",
      // Fresh install intentionally erases the whole chip — the opposite of
      // the Upgrade path's eraseAll: false (see runUpgradeFlash below).
      eraseAll: true,
      compress: true,
      reportProgress: makeProgressReporter("Writing firmware", fileArray.length),
    });

    setFlashPhase("Resetting device…");
    await transport.setRTS(true);
    await sleep(100);
    await esploader.after();
    await transport.disconnect();

    setFlashPhase(`Done — ${tag} installed.`);
  } catch (err) {
    console.error("MeshCadet flasher: fresh install failed", err);
    // Unlike Upgrade (which never erases beyond the single app part it
    // writes — ADR-0009 D5 — and so is always provably safe to retry), a
    // Fresh install that fails DURING the write leaves the bootloader/
    // partition table in an unknown, possibly incomplete state: esptool's
    // erase-as-written behavior only guarantees bytes the new image
    // actually covers end up correct, not that a full erase begun and then
    // interrupted leaves a bootable chip. Say so plainly rather than
    // implying the same blanket "safe to retry" Upgrade's message does.
    setFlashError(
      writeStarted
        ? `Install failed mid-write: ${err.message || err}. The device's bootloader/partition table may now be incomplete — try Fresh install again before disconnecting. If the device no longer enumerates over USB afterward, it likely needs manual bootloader/download-mode recovery (consult LilyGo's T-Deck Plus documentation).`
        : `Install failed before any bytes were written: ${err.message || err}. Nothing was flashed — safe to retry.`
    );
    if (transport) {
      try {
        await transport.disconnect();
      } catch (disconnectErr) {
        console.error("MeshCadet flasher: failed to disconnect after a failed install", disconnectErr);
      }
    }
  } finally {
    flashBusy = false;
    setFlashControlsLocked(false);
    updateFreshConnectEnabled();
    updateUpgradeConnectEnabled();
  }
}

freshConnectButton.addEventListener("click", runFreshFlash);

// ── Upgrade path: hand-rolled esptool-js flow ──────────────────────────────
//
// Deliberately NOT <esp-web-install-button> (ADR-0009 D4, unchanged by
// ADR-0011). Reading esp-web-tools' own source (src/install-dialog.ts, the
// code driving that element) shows that for a device which doesn't speak
// Improv Serial — which MeshCadet's firmware doesn't, it uses its own custom
// provisioning protocol, never Improv — a manifest with
// `new_install_prompt_erase: false` (exactly what ADR-0008's frozen
// manifest-update.json sets) takes the `_renderDashboardNoImprov` branch's
// Install click handler straight to `_startInstall(true)`: a full chip
// erase, unconditionally, with no prompt and no way for the user to opt
// out. `new_install_prompt_erase: true` would only reach an ASK_ERASE
// checkbox screen instead of skipping it — not something this site can
// change unilaterally, since manifest-update.json's shape is ADR-0008's
// frozen, cross-mission contract. Either way, the install-button element
// cannot be trusted to leave a MeshCadet device's nvs/mc_hist partitions
// untouched. This is exactly the case docs/adr/0009-two-path-flasher.md's D4
// calls out as the fallback trigger: a hand-rolled flow using the same
// esptool-js primitives directly, calling `writeFlash` with
// `eraseAll: false` and a single {data, address} part — which (per
// esptool-js's own writeFlash implementation) only erases the flash sectors
// the part being written actually covers, never the whole chip, and never
// calls the separate `eraseFlash()` (full-chip erase) primitive at all.

async function runUpgradeFlash() {
  if (flashBusy) {
    return;
  }
  if (!isValidUpdateMeta(currentUpdateMeta)) {
    // Defense in depth — the button is disabled in this state already
    // (updateUpgradeConnectEnabled), so this should be unreachable.
    setFlashError("Upgrade isn't available for the selected version.");
    return;
  }

  const tag = select.value;
  const meta = currentUpdateMeta;

  flashBusy = true;
  updateFreshConnectEnabled();
  updateUpgradeConnectEnabled();
  setFlashControlsLocked(true);
  resetFlashConsole();

  let transport;
  try {
    let port;
    try {
      setFlashPhase("Requesting device access…");
      port = await navigator.serial.requestPort();
    } catch (err) {
      // User dismissed the browser's port picker — not an error worth
      // logging.
      setFlashPhase("No device selected.");
      return;
    }

    setFlashPhase(`Downloading ${meta.app_asset}…`);
    const appUrl = `firmware/${tag}/${meta.app_asset}`;
    const appResponse = await fetch(appUrl);
    if (!appResponse.ok) {
      throw new Error(`Downloading ${meta.app_asset} failed: ${appResponse.status}`);
    }
    // esptool-js's writeFlash wants `data` as a Latin-1 binary string, not
    // a Uint8Array — see flash-image-encoding.js's header. (The name keeps
    // "app" — it's still the app image — but holds a binary string now,
    // not raw bytes.)
    const appImageData = ui8ToBstr(new Uint8Array(await appResponse.arrayBuffer()));

    setFlashPhase("Connecting to device…");
    transport = new Transport(port);
    const esploader = new ESPLoader({ transport, baudrate: 115200, enableTracing: false });
    await esploader.main();
    await esploader.flashId();

    const chipFamily = esploader.chip.CHIP_NAME;
    if (chipFamily !== EXPECTED_CHIP_FAMILY) {
      throw new Error(
        `Connected device reports "${chipFamily}", expected "${EXPECTED_CHIP_FAMILY}" — refusing to write an app image built for a different chip.`
      );
    }

    setFlashPhase("Writing app image — do not disconnect…");
    await esploader.writeFlash({
      fileArray: [{ data: appImageData, address: meta.app_offset }],
      flashSize: "keep",
      flashMode: "keep",
      flashFreq: "keep",
      // The load-bearing flag (see the module-level comment above): only
      // the sectors this one part covers are erased-as-written. nvs@0x9000
      // and mc_hist@0x610000 are never touched.
      eraseAll: false,
      compress: true,
      reportProgress: makeProgressReporter("Writing app image", 1),
    });

    setFlashPhase("Resetting device…");
    await transport.setRTS(true);
    await sleep(100);
    await esploader.after();
    await transport.disconnect();

    setFlashPhase(
      `Done — ${tag} installed. Your device's identity, config, and history were preserved.`
    );
  } catch (err) {
    console.error("MeshCadet flasher: upgrade flash failed", err);
    setFlashError(
      `Upgrade failed: ${err.message || err}. Only the app region is ever written by this ` +
        "path, so your device's identity, config, and history were not touched — safe to " +
        "retry, or use Fresh install instead."
    );
    if (transport) {
      try {
        await transport.disconnect();
      } catch (disconnectErr) {
        console.error("MeshCadet flasher: failed to disconnect after a failed upgrade", disconnectErr);
      }
    }
  } finally {
    flashBusy = false;
    setFlashControlsLocked(false);
    updateFreshConnectEnabled();
    updateUpgradeConnectEnabled();
  }
}

upgradeConnectButton.addEventListener("click", runUpgradeFlash);

// Static for the lifetime of the page (WEB_SERIAL_SUPPORTED never changes),
// so this is set once rather than recomputed on every path-choice change.
freshUnsupported.hidden = WEB_SERIAL_SUPPORTED;
upgradeUnsupported.hidden = WEB_SERIAL_SUPPORTED;

loadReleases();
