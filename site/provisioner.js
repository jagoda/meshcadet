// provisioner.js — UI glue for provisioner.html (M1 walking skeleton):
// Web Serial connect -> read-only status/identity + MeshCore contact QR.
//
// No build step (site/README.md convention): plain ES module, loaded
// directly by the browser. The QR library is a major-version-pinned CDN
// import via esm.sh (which serves the "qrcode" npm package — soldair/
// node-qrcode, pure JS, no native deps — pre-bundled as a browser-ready ES
// module), the same "single pinned CDN import, no bundler" pattern flash.html
// uses for esp-web-tools.
//
// Client-side security model (docs/adr/0007-provisioner-codec.md): no
// analytics/telemetry, nothing sent to a server (GitHub Pages is fully
// static), no secrets in this M1 skeleton (pubkey/device name/status counters
// are not secret — PIN/channel-secret handling is a later campaign
// milestone that must uphold the same model then).

import { ProvisionerSession } from "./provisioner/session.js";
import { bytesToHex } from "./provisioner/codec.js";
import QRCode from "https://esm.sh/qrcode@1.5.3";

const unsupportedPanel = document.getElementById("unsupported-panel");
const connectPanel = document.getElementById("connect-panel");
const connectButton = document.getElementById("connect-button");
const disconnectButton = document.getElementById("disconnect-button");
const refreshButton = document.getElementById("refresh-button");
const connectStatus = document.getElementById("connect-status");
const statusPanel = document.getElementById("status-panel");
const qrPanel = document.getElementById("qr-panel");
const qrCanvas = document.getElementById("qr-canvas");
const qrUri = document.getElementById("qr-uri");

const fields = {
  provisioned: document.getElementById("stat-provisioned"),
  name: document.getElementById("stat-name"),
  pubkey: document.getElementById("stat-pubkey"),
  contacts: document.getElementById("stat-contacts"),
  channels: document.getElementById("stat-channels"),
  battery: document.getElementById("stat-battery"),
  gpsFix: document.getElementById("stat-gps-fix"),
  gpsCoords: document.getElementById("stat-gps-coords"),
  gpsClock: document.getElementById("stat-gps-clock"),
};

const session = new ProvisionerSession();

function setStatus(message) {
  connectStatus.textContent = message;
}

// ── Feature gate: Chrome/Edge + HTTPS guidance, mirroring flash.html's
//    unsupported-browser fallback for esp-web-tools ──────────────────────────

if (!ProvisionerSession.isSupported() || !ProvisionerSession.isSecureContext()) {
  unsupportedPanel.hidden = false;
  connectPanel.hidden = true;
} else {
  connectButton.addEventListener("click", handleConnect);
  disconnectButton.addEventListener("click", handleDisconnect);
  refreshButton.addEventListener("click", handleRefresh);

  // If the OS reports the connected device physically unplugged, tear down
  // the session instead of leaving the UI showing stale "connected" state.
  navigator.serial.addEventListener("disconnect", (event) => {
    if (session.isConnected && event.target === session.port) {
      handleDisconnect("Device disconnected.");
    }
  });

  // Best-effort port release on navigation away — not strictly required
  // (this M1 skeleton holds no secrets to scrub, see header), but leaves the
  // OS-level serial port cleanly closed rather than orphaned.
  window.addEventListener("pagehide", () => {
    if (session.isConnected) {
      session.disconnect();
    }
  });
}

// ── Connect / disconnect / refresh ───────────────────────────────────────────

async function handleConnect() {
  connectButton.disabled = true;
  setStatus("Requesting device…");
  try {
    await session.connect();
  } catch (err) {
    connectButton.disabled = false;
    if (err && err.name === "NotFoundError") {
      // User dismissed the browser's device picker — a silent cancel, not an
      // error worth alarming anyone over.
      setStatus("");
      return;
    }
    console.error("MeshCadet provisioner: connect failed", err);
    setStatus(`Couldn't open the device: ${err.message || err}`);
    return;
  }
  connectButton.hidden = true;
  disconnectButton.hidden = false;
  refreshButton.hidden = false;
  await queryAndRender();
}

async function handleDisconnect(message = "Disconnected.") {
  await session.disconnect();
  connectButton.hidden = false;
  connectButton.disabled = false;
  disconnectButton.hidden = true;
  refreshButton.hidden = true;
  statusPanel.hidden = true;
  qrPanel.hidden = true;
  setStatus(message);
}

async function handleRefresh() {
  await queryAndRender();
}

async function queryAndRender() {
  refreshButton.disabled = true;
  setStatus("Reading status…");
  try {
    const { status, identity } = await session.queryStatus();
    renderStatus(status, identity);
    await renderQr(identity);
    setStatus(`Last read at ${new Date().toLocaleTimeString()}.`);
  } catch (err) {
    console.error("MeshCadet provisioner: query_status failed", err);
    setStatus(`Couldn't read device status: ${err.message || err}`);
  } finally {
    refreshButton.disabled = false;
  }
}

// ── Status/identity rendering ─────────────────────────────────────────────
//
// Field selection + formatting mirrors the host CLI's `status` command
// (host/src/main.rs's format_gps_fix/format_gps_coords/format_gps_clock/
// format_battery) — consumer-facing copy only. Deliberately OMITS
// format_battery_raw_mv/format_battery_held_raw_mv: those are diagnostic-only
// internal vernacular (see their doc comments in host/src/main.rs), not for a
// public page.

function renderStatus(status, identity) {
  statusPanel.hidden = false;
  fields.provisioned.textContent = status.provisioned ? "yes" : "no";
  fields.name.textContent = identity.device_name || "(unnamed)";
  fields.pubkey.textContent = bytesToHex(identity.pubkey);
  fields.contacts.textContent = String(status.contact_count);
  fields.channels.textContent = String(status.channel_count);
  fields.battery.textContent = status.battery_charging
    ? `${status.battery_percent}% (charging)`
    : `${status.battery_percent}%`;
  fields.gpsFix.textContent = status.gps_has_fix ? "yes" : "no";
  fields.gpsCoords.textContent = status.gps_has_fix
    ? `${(status.gps_lat_e7 / 10_000_000).toFixed(6)}, ${(status.gps_lon_e7 / 10_000_000).toFixed(6)} (age ${status.gps_fix_age_secs}s)`
    : "—";
  fields.gpsClock.textContent = status.gps_clock_synced
    ? `synced (age ${status.gps_clock_sync_age_secs}s)`
    : "not synced";
}

// ── MeshCore contact QR ──────────────────────────────────────────────────
//
// Mirrors host/src/main.rs's contact-URI construction and `url_encode`
// byte-for-byte (see docs/adr/0002-provisioning-wire-format.md /
// meshcore-dev/MeshCore docs/faq.md §7.5 for the URI grammar). `type=1` is
// hardcoded (chat=1) — MeshCadet is always a chat node, matching main.rs.

const NAME_ENCODER = new TextEncoder();

/**
 * Percent-encode a string for use as a URI query-component value (RFC 3986).
 * A byte-for-byte hand port of `url_encode` in `host/src/main.rs`: same
 * unreserved set (`A-Z a-z 0-9 - _ . ~`), same uppercase-hex escaping,
 * operating on the UTF-8 byte sequence (not JS UTF-16 code units).
 */
function urlEncode(str) {
  let out = "";
  for (const b of NAME_ENCODER.encode(str)) {
    if (
      (b >= 0x41 && b <= 0x5a) || // A-Z
      (b >= 0x61 && b <= 0x7a) || // a-z
      (b >= 0x30 && b <= 0x39) || // 0-9
      b === 0x2d || // -
      b === 0x5f || // _
      b === 0x2e || // .
      b === 0x7e // ~
    ) {
      out += String.fromCharCode(b);
    } else {
      out += "%" + b.toString(16).toUpperCase().padStart(2, "0");
    }
  }
  return out;
}

/** Build the `meshcore://contact/add?...` URI for the connected device's identity. */
function contactUri(identity) {
  const name =
    identity.device_name && identity.device_name.length > 0
      ? identity.device_name
      : `MeshCadet-${identity.pubkey[0].toString(16).toUpperCase().padStart(2, "0")}`;
  return `meshcore://contact/add?name=${urlEncode(name)}&public_key=${bytesToHex(identity.pubkey)}&type=1`;
}

async function renderQr(identity) {
  const uri = contactUri(identity);
  qrPanel.hidden = false;
  qrUri.textContent = uri;
  try {
    await QRCode.toCanvas(qrCanvas, uri, {
      width: 220,
      margin: 2,
      color: { dark: "#0d1117", light: "#ffffff" },
    });
    qrCanvas.hidden = false;
  } catch (err) {
    console.error("MeshCadet provisioner: QR render failed", err);
    qrCanvas.hidden = true;
  }
}
