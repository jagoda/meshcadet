// flash.js — two-path web flasher for site/flash.html: Fresh install
// (merged image, erases) vs Upgrade (app-only image, non-erasing). See
// docs/adr/0009-two-path-flasher.md for the design record; extends
// docs/adr/0006-web-flasher.md (version selector over a same-origin mirror)
// and consumes docs/adr/0008-nondestructive-update-artifacts.md's frozen
// manifest-update.json / update-meta.json contract.
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
//      paths below read from that same-origin copy.
//
//   3. update-meta.json (mirrored alongside the rest) carries the
//      layout-compatibility gate (ADR-0008 D2/D4): `upgrade_safe` decides
//      whether the Upgrade path is even offered for a given version, and
//      `app_asset`/`app_offset` tell this page which file to write and
//      where, without hardcoding either.
//
// No build step (site/README.md convention) — plain ES module JS loaded
// directly by the browser, no bundler.

// esptool-js: pinned to the EXACT version esp-web-tools@10 (the CDN import
// in flash.html, backing <esp-web-install-button> below) itself depends on
// (esp-web-tools@10.2.1's package.json pins "esptool-js": "^0.5.7", which
// resolves to exactly 0.5.7 — verified live against the published npm
// registry). Pinning our own import to the same exact version means the
// Upgrade path's flash mechanics (this file) and the Fresh-install path's
// flash mechanics (esp-web-install-button, internally) run the identical
// underlying esptool-js write/erase code — no behavioral drift between the
// two paths from an unpinned or mismatched version. Bundle is a
// self-contained ES module (no unresolved bare imports — verified by
// fetching and inspecting it) exporting `ESPLoader`/`Transport`, the same
// two classes esp-web-tools' own src/flash.ts drives.
import { ESPLoader, Transport } from "https://unpkg.com/esptool-js@0.5.7/bundle.js";
import { isValidUpdateMeta } from "./upgrade-gate.js";

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
// device before the Upgrade path ever writes a byte, so an app image built
// for the wrong chip is refused rather than blindly written.
const EXPECTED_CHIP_FAMILY = "ESP32-S3";

const select = document.getElementById("version-select");
const status = document.getElementById("version-status");
const installButton = document.getElementById("install-button");

const pathFreshRadio = document.getElementById("path-fresh");
const pathUpgradeRadio = document.getElementById("path-upgrade");
const upgradeSafetyNote = document.getElementById("upgrade-safety-note");
const freshPanel = document.getElementById("fresh-panel");
const upgradePanel = document.getElementById("upgrade-panel");
const upgradeUnsupported = document.getElementById("upgrade-unsupported");
const upgradeBackupAck = document.getElementById("upgrade-backup-ack");
const upgradeConnectButton = document.getElementById("upgrade-connect-button");
const upgradeStatus = document.getElementById("upgrade-status");

// Web Serial feature/context gate for the Upgrade path's own custom flash
// flow (it does not go through <esp-web-install-button>, which handles this
// itself via its "unsupported"/"not-allowed" slots for the Fresh path — see
// flash.html). Mirrors provisioner/session.js's ProvisionerSession static
// checks.
const WEB_SERIAL_SUPPORTED = "serial" in navigator && window.isSecureContext === true;

let currentUpdateMeta = null; // null = not upgrade_safe (missing, or fetch failed)
let upgradeBusy = false;

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

function setUpgradeStatus(message) {
  upgradeStatus.textContent = message;
}

// ── Path choice (Fresh install vs Upgrade) ─────────────────────────────────

function applyPathChoice() {
  const upgrade = pathUpgradeRadio.checked;
  freshPanel.hidden = upgrade;
  upgradePanel.hidden = !upgrade;
  if (upgrade) {
    upgradeUnsupported.hidden = WEB_SERIAL_SUPPORTED;
    updateUpgradeConnectEnabled();
  }
}

function updateUpgradeConnectEnabled() {
  upgradeConnectButton.disabled =
    upgradeBusy ||
    !WEB_SERIAL_SUPPORTED ||
    !upgradeBackupAck.checked ||
    !isValidUpdateMeta(currentUpdateMeta);
}

// Locks the version selector and path choice while an Upgrade write is in
// flight — switching versions or paths mid-write wouldn't affect the
// in-progress esptool-js session (its tag/meta are captured in local
// variables at the start of runUpgradeFlash, not re-read from shared
// state), but it would leave the visible UI (dropdown, radios, the
// Fresh-install button) in a confusing, actionable-looking state while a
// serial port is actively in use — e.g. clicking "Connect & Install" for
// Fresh install while an Upgrade write is still running would contend for
// the same physical port. Simpler to just disable the choice than to reason
// about two concurrent esptool-js sessions.
function setUpgradeControlsLocked(locked) {
  select.disabled = locked || select.options.length === 0;
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

  // Reset the advisory gate + status on every version switch — an
  // acknowledgment for one version's backup reminder shouldn't silently
  // carry over and skip the reminder for a different version.
  upgradeBackupAck.checked = false;
  setUpgradeStatus("");
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

// ── Fresh-install path (esp-web-install-button, unchanged mechanism) ───────

function selectVersion(tag) {
  const url = manifestUrlFor(tag);
  installButton.manifest = url;
  installButton.setAttribute("manifest", url);
  installButton.hidden = false;
  loadUpdateMeta(tag);
}

function renderEmpty(message) {
  select.innerHTML = "";
  const option = document.createElement("option");
  option.textContent = message;
  select.append(option);
  select.disabled = true;
  installButton.hidden = true;
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

// ── Upgrade path: custom esptool-js flash flow ──────────────────────────────
//
// Deliberately NOT <esp-web-install-button>. Reading esp-web-tools' own
// source (src/install-dialog.ts, the code driving that element) shows that
// for a device which doesn't speak Improv Serial — which MeshCadet's
// firmware doesn't, it uses its own custom provisioning protocol, never
// Improv — a manifest with `new_install_prompt_erase: false` (exactly what
// ADR-0008's frozen manifest-update.json sets) takes the
// `_renderDashboardNoImprov` branch straight to `_startInstall(true)`: a
// full chip erase, unconditionally, with no prompt and no way for the user
// to opt out. `new_install_prompt_erase: true` would only reach an
// ASK_ERASE checkbox screen instead of skipping it — not something this
// site can change unilaterally, since manifest-update.json's shape is
// ADR-0008's frozen, cross-mission contract. Either way, the install-button
// element cannot be trusted to leave a MeshCadet device's nvs/mc_hist
// partitions untouched. This is exactly the case
// docs/adr/0009-two-path-flasher.md's D4 calls out as the fallback trigger:
// a hand-rolled flow using the same esptool-js primitives directly, calling
// `writeFlash` with `eraseAll: false` and a single {data, address} part —
// which (per esptool-js's own writeFlash implementation) only erases the
// flash sectors the part being written actually covers, never the whole
// chip, and never calls the separate `eraseFlash()` (full-chip erase)
// primitive at all.

async function runUpgradeFlash() {
  if (upgradeBusy) {
    return;
  }
  if (!isValidUpdateMeta(currentUpdateMeta)) {
    // Defense in depth — the button is disabled in this state already
    // (updateUpgradeConnectEnabled), so this should be unreachable.
    setUpgradeStatus("Upgrade isn't available for the selected version.");
    return;
  }

  const tag = select.value;
  const meta = currentUpdateMeta;

  upgradeBusy = true;
  updateUpgradeConnectEnabled();
  setUpgradeControlsLocked(true);

  let transport;
  try {
    let port;
    try {
      setUpgradeStatus("Requesting device access…");
      port = await navigator.serial.requestPort();
    } catch (err) {
      // User dismissed the browser's port picker — not an error worth
      // logging.
      setUpgradeStatus("No device selected.");
      return;
    }

    setUpgradeStatus(`Downloading ${meta.app_asset}…`);
    const appUrl = `firmware/${tag}/${meta.app_asset}`;
    const appResponse = await fetch(appUrl);
    if (!appResponse.ok) {
      throw new Error(`Downloading ${meta.app_asset} failed: ${appResponse.status}`);
    }
    const appBytes = new Uint8Array(await appResponse.arrayBuffer());

    setUpgradeStatus("Connecting to device…");
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

    setUpgradeStatus("Writing app image — do not disconnect…");
    await esploader.writeFlash({
      fileArray: [{ data: appBytes, address: meta.app_offset }],
      flashSize: "keep",
      flashMode: "keep",
      flashFreq: "keep",
      // The load-bearing flag (see the module-level comment above): only
      // the sectors this one part covers are erased-as-written. nvs@0x9000
      // and mc_hist@0x610000 are never touched.
      eraseAll: false,
      compress: true,
      reportProgress: (_fileIndex, written, total) => {
        const pct = total ? Math.floor((written / total) * 100) : 0;
        setUpgradeStatus(`Writing app image: ${pct}%`);
      },
    });

    setUpgradeStatus("Resetting device…");
    await transport.setRTS(true);
    await sleep(100);
    await esploader.after();
    await transport.disconnect();

    setUpgradeStatus(
      `Done — ${tag} installed. Your device's identity, config, and history were preserved.`
    );
  } catch (err) {
    console.error("MeshCadet flasher: upgrade flash failed", err);
    setUpgradeStatus(
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
    upgradeBusy = false;
    setUpgradeControlsLocked(false);
    updateUpgradeConnectEnabled();
  }
}

upgradeConnectButton.addEventListener("click", runUpgradeFlash);

if (!WEB_SERIAL_SUPPORTED) {
  upgradeUnsupported.hidden = false;
}

loadReleases();
