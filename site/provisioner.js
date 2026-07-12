// provisioner.js — UI glue for provisioner.html: Web Serial connect ->
// status/identity + MeshCore contact QR (M1), the M2 non-sensitive
// provisioning **writes** (contact/channel list+add/remove, notification
// defaults, device-name set, commit), and the M2 **sensitive-data** surface:
// admin-PIN set/reset, history export (local download), and history clear.
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
// static). Every secret this page touches — a generated channel secret, the
// admin PIN, and exported private-message history — is created/handled only
// in this page and either sent to the device over the already-open serial
// connection or offered to the user as an explicit local download; it is
// never logged, never placed in the URL, and never written to
// `localStorage`/`sessionStorage`. Specifically: the PIN field is cleared the
// instant it's sent (and `session.setPin` scrubs its own buffer), and history
// text is written to disk only on an explicit "Download transcript" click,
// never auto-downloaded and never console-logged.

import { ProvisionerSession, DeviceError } from "./provisioner/session.js";
import { bytesToHex } from "./provisioner/codec.js";
import { buildContactUri } from "./provisioner/contact-uri.js";
import { validatePubkeyHex, validateChannelSecretHex, validateDeviceName, validatePin } from "./provisioner/validation.js";
import { formatHistoryTranscript } from "./provisioner/history-format.js";
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

// ── M2 write-surface DOM refs ─────────────────────────────────────────────

const setNameForm = document.getElementById("set-name-form");
const setNameInput = document.getElementById("set-name-input");
const setNameButton = document.getElementById("set-name-button");
const setNameStatus = document.getElementById("set-name-status");

const contactsPanel = document.getElementById("contacts-panel");
const contactsTableBody = document.getElementById("contacts-table-body");
const addContactForm = document.getElementById("add-contact-form");
const addContactPubkey = document.getElementById("add-contact-pubkey");
const addContactName = document.getElementById("add-contact-name");
const addContactTelemetry = document.getElementById("add-contact-telemetry");
const addContactButton = document.getElementById("add-contact-button");
const addContactStatus = document.getElementById("add-contact-status");

const channelsPanel = document.getElementById("channels-panel");
const channelsTableBody = document.getElementById("channels-table-body");
const addChannelForm = document.getElementById("add-channel-form");
const addChannelSecret = document.getElementById("add-channel-secret");
const addChannelName = document.getElementById("add-channel-name");
const addChannelPrimary = document.getElementById("add-channel-primary");
const addChannelButton = document.getElementById("add-channel-button");
const addChannelStatus = document.getElementById("add-channel-status");
const genChannel128Button = document.getElementById("gen-channel-128-button");
const genChannel256Button = document.getElementById("gen-channel-256-button");
const delChannelForm = document.getElementById("del-channel-form");
const delChannelSecret = document.getElementById("del-channel-secret");
const delChannelButton = document.getElementById("del-channel-button");
const delChannelStatus = document.getElementById("del-channel-status");

const notifPanel = document.getElementById("notif-panel");
const notifForm = document.getElementById("notif-form");
const notifVisual = document.getElementById("notif-visual");
const notifAudible = document.getElementById("notif-audible");
const notifSaveButton = document.getElementById("notif-save-button");
const notifStatus = document.getElementById("notif-status");

const commitPanel = document.getElementById("commit-panel");
const commitButton = document.getElementById("commit-button");
const commitStatus = document.getElementById("commit-status");

// ── M2 sensitive-data DOM refs ────────────────────────────────────────────

const pinPanel = document.getElementById("pin-panel");
const setPinForm = document.getElementById("set-pin-form");
const setPinInput = document.getElementById("set-pin-input");
const setPinStatus = document.getElementById("set-pin-status");

const historyPanel = document.getElementById("history-panel");
const historyReadButton = document.getElementById("history-read-button");
const historyDownloadButton = document.getElementById("history-download-button");
const historyStatus = document.getElementById("history-status");

const clearHistoryPanel = document.getElementById("clear-history-panel");
const clearHistoryButton = document.getElementById("clear-history-button");
const clearHistoryConfirm = document.getElementById("clear-history-confirm");
const clearHistoryAck = document.getElementById("clear-history-ack");
const clearHistoryConfirmButton = document.getElementById("clear-history-confirm-button");
const clearHistoryCancelButton = document.getElementById("clear-history-cancel-button");
const clearHistoryStatus = document.getElementById("clear-history-status");

const writePanels = [contactsPanel, channelsPanel, notifPanel, commitPanel, pinPanel, historyPanel, clearHistoryPanel];

// Transient hold for a just-read history transcript, awaiting an explicit
// user "Download transcript" click. Contains private message text, so it is
// kept only in this module variable (never logged, stored, or transmitted)
// and scrubbed on disconnect / re-read. See `handleReadHistory`.
let pendingTranscript = null;

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
  // Wrapped in a closure (not passed directly) — addEventListener would
  // otherwise call `handleDisconnect(clickEvent)`, silently overriding its
  // `message = "Disconnected."` default parameter with the click Event
  // object (coerced to the string "[object PointerEvent]" by
  // `setStatus`/`textContent`) instead of leaving the default in place.
  disconnectButton.addEventListener("click", () => handleDisconnect());
  refreshButton.addEventListener("click", handleRefresh);

  setNameForm.addEventListener("submit", (event) => {
    event.preventDefault();
    handleSetName();
  });
  addContactForm.addEventListener("submit", (event) => {
    event.preventDefault();
    handleAddContact();
  });
  addChannelForm.addEventListener("submit", (event) => {
    event.preventDefault();
    handleAddChannel();
  });
  delChannelForm.addEventListener("submit", (event) => {
    event.preventDefault();
    handleDelChannel();
  });
  genChannel128Button.addEventListener("click", () => handleGenerateChannelSecret(16));
  genChannel256Button.addEventListener("click", () => handleGenerateChannelSecret(32));
  notifForm.addEventListener("submit", (event) => {
    event.preventDefault();
    handleSetNotifDefaults();
  });
  commitButton.addEventListener("click", handleCommit);

  setPinForm.addEventListener("submit", (event) => {
    event.preventDefault();
    handleSetPin();
  });
  historyReadButton.addEventListener("click", handleReadHistory);
  historyDownloadButton.addEventListener("click", handleDownloadTranscript);
  clearHistoryButton.addEventListener("click", handleClearHistoryArm);
  clearHistoryCancelButton.addEventListener("click", resetClearHistoryConfirm);
  clearHistoryAck.addEventListener("change", () => {
    // Second gate: the final erase button only enables once the user ticks
    // the acknowledgement (the arm-reveal above was the first gate; the
    // native confirm() in handleClearHistory is the last).
    clearHistoryConfirmButton.disabled = !clearHistoryAck.checked;
  });
  clearHistoryConfirmButton.addEventListener("click", handleClearHistory);

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
  for (const panel of writePanels) {
    panel.hidden = false;
  }
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
  for (const panel of writePanels) {
    panel.hidden = true;
  }
  clearFormStatuses();
  setStatus(message);
}

/**
 * Disable `button` for the duration of `fn()`, always re-enabling it
 * afterward (success or failure). `session.js` already serializes
 * concurrent command calls (`ProvisionerSession#exclusive`) so a double
 * click can't desync the wire protocol, but leaving a submit button live
 * mid-request invites confusing duplicate submissions and overlapping
 * status messages — this is the UX half of that guard, applied the same
 * way `commitButton`/`refreshButton` already disable themselves.
 */
async function withButtonDisabled(button, fn) {
  button.disabled = true;
  try {
    return await fn();
  } finally {
    button.disabled = false;
  }
}

function clearFormStatuses() {
  setNameStatus.textContent = "";
  addContactStatus.textContent = "";
  addChannelStatus.textContent = "";
  delChannelStatus.textContent = "";
  notifStatus.textContent = "";
  commitStatus.textContent = "";
  setPinStatus.textContent = "";
  historyStatus.textContent = "";
  clearHistoryStatus.textContent = "";
  // Scrub sensitive UI state on teardown: drop any typed PIN, forget any
  // read-but-not-downloaded transcript, and re-arm the clear-history gate.
  setPinInput.value = "";
  pendingTranscript = null;
  historyDownloadButton.hidden = true;
  resetClearHistoryConfirm();
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
    await refreshLists();
    setStatus(`Last read at ${new Date().toLocaleTimeString()}.`);
  } catch (err) {
    console.error("MeshCadet provisioner: query_status failed", err);
    setStatus(`Couldn't read device status: ${err.message || err}`);
  } finally {
    refreshButton.disabled = false;
  }
}

/** Re-read the contact/channel staging lists and re-render both tables. */
async function refreshLists() {
  try {
    const contacts = await session.listContacts();
    renderContacts(contacts);
  } catch (err) {
    console.error("MeshCadet provisioner: list_contacts failed", err);
    setTableMessage(contactsTableBody, 5, `Couldn't read contacts: ${err.message || err}`);
  }
  try {
    const channels = await session.listChannels();
    renderChannels(channels);
  } catch (err) {
    console.error("MeshCadet provisioner: list_channels failed", err);
    setTableMessage(channelsTableBody, 5, `Couldn't read channels: ${err.message || err}`);
  }
}

/**
 * Replace `tbody`'s contents with a single full-width message row.
 * Built via `textContent` (not `innerHTML`) so a device-supplied string
 * embedded in `message` (e.g. a `DeviceError`'s error text) can never be
 * interpreted as markup.
 */
function setTableMessage(tbody, colspan, message) {
  tbody.replaceChildren();
  const tr = document.createElement("tr");
  const td = document.createElement("td");
  td.colSpan = colspan;
  td.className = "muted";
  td.textContent = message;
  tr.appendChild(td);
  tbody.appendChild(tr);
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
// URI construction itself (`buildContactUri`/`urlEncode`, a byte-for-byte
// hand port of host/src/main.rs's contact-URI construction and `url_encode`)
// lives in ./provisioner/contact-uri.js — a DOM-free module so it's testable
// under plain `node` (contact-uri.test.mjs) without dragging in this file's
// `document`/`navigator` top-level side effects.

async function renderQr(identity) {
  const uri = buildContactUri(identity);
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

// ── Set device name (identity --set-name) ────────────────────────────────

async function handleSetName() {
  const validated = validateDeviceName(setNameInput.value);
  if (!validated.ok) {
    setNameStatus.textContent = validated.error;
    return;
  }
  await withButtonDisabled(setNameButton, async () => {
    setNameStatus.textContent = "Setting name…";
    try {
      await session.setDeviceName(validated.name);
      setNameStatus.textContent = validated.name ? `Device name set: "${validated.name}"` : "Device name cleared.";
      await queryAndRender(); // re-read identity/QR so the new name shows immediately
    } catch (err) {
      reportWriteError(setNameStatus, "set device name", err);
    }
  });
}

// ── Contacts (list-contacts / add-contact / del-contact) ─────────────────
//
// Field selection mirrors the host CLI's `list-contacts`/`add-contact`
// output (host/src/main.rs) — idx, pubkey, telemetry flag, name. Rows are
// built with `createElement`/`textContent` (never `innerHTML`) since
// `display_name` is a device-supplied UTF-8 string this page must not treat
// as markup.

function renderContacts(contacts) {
  contactsTableBody.replaceChildren();
  if (contacts.length === 0) {
    setTableMessage(contactsTableBody, 5, "No contacts configured.");
    return;
  }
  for (const c of contacts) {
    const tr = document.createElement("tr");
    tr.appendChild(textCell(String(c.index)));
    tr.appendChild(textCell(bytesToHex(c.pubkey), "mono"));
    tr.appendChild(textCell(c.telemetry_enable ? "yes" : "no"));
    tr.appendChild(textCell(c.display_name || "(unnamed)"));
    const actionsTd = document.createElement("td");
    const removeButton = document.createElement("button");
    removeButton.type = "button";
    removeButton.className = "btn btn-ghost";
    removeButton.textContent = "Remove";
    removeButton.addEventListener("click", () => handleDelContact(c.pubkey, removeButton));
    actionsTd.appendChild(removeButton);
    tr.appendChild(actionsTd);
    contactsTableBody.appendChild(tr);
  }
}

function textCell(text, className) {
  const td = document.createElement("td");
  if (className) {
    td.className = className;
  }
  td.textContent = text;
  return td;
}

async function handleAddContact() {
  const pubkeyResult = validatePubkeyHex(addContactPubkey.value);
  if (!pubkeyResult.ok) {
    addContactStatus.textContent = pubkeyResult.error;
    return;
  }
  await withButtonDisabled(addContactButton, async () => {
    addContactStatus.textContent = "Adding contact…";
    try {
      await session.addContact(pubkeyResult.bytes, addContactTelemetry.checked, addContactName.value);
      addContactStatus.textContent =
        "Contact added. Note: reboot the device to apply this to the live mesh (allowlist + telemetry gate are loaded at boot).";
      addContactForm.reset();
      await refreshLists();
    } catch (err) {
      reportWriteError(addContactStatus, "add contact", err);
    }
  });
}

async function handleDelContact(pubkey, removeButton) {
  await withButtonDisabled(removeButton, async () => {
    try {
      await session.delContact(pubkey);
      addContactStatus.textContent = `Contact removed: ${bytesToHex(pubkey).slice(0, 8)}…`;
      await refreshLists();
    } catch (err) {
      reportWriteError(addContactStatus, "remove contact", err);
    }
  });
}

// ── Channels (list-channels / add-channel / del-channel) ─────────────────
//
// Channel removal needs the exact secret (not the 1-byte hash the list
// shows) so there is no per-row "Remove" button here — see del-channel-form
// in provisioner.html.

function renderChannels(channels) {
  channelsTableBody.replaceChildren();
  if (channels.length === 0) {
    setTableMessage(channelsTableBody, 5, "No channels configured.");
    return;
  }
  for (const ch of channels) {
    const tr = document.createElement("tr");
    tr.appendChild(textCell(String(ch.index)));
    tr.appendChild(textCell(`0x${ch.channel_hash.toString(16).toUpperCase().padStart(2, "0")}`, "mono"));
    tr.appendChild(textCell(String(ch.key_len * 8)));
    tr.appendChild(textCell(ch.primary ? "yes" : "no"));
    tr.appendChild(textCell(ch.name || "(unnamed)"));
    channelsTableBody.appendChild(tr);
  }
}

/**
 * Fill `add-channel-secret` with a fresh, locally generated secret.
 * `bytesLen` is 16 (128-bit) or 32 (256-bit). Uses `crypto.getRandomValues`
 * (never `Math.random`) — the generated secret is placed only into this
 * page's own form field and, on submit, sent directly to the device over
 * the already-open serial connection; it is never logged, placed in the
 * URL, or written to `localStorage`/`sessionStorage`.
 */
function handleGenerateChannelSecret(bytesLen) {
  const raw = new Uint8Array(bytesLen);
  crypto.getRandomValues(raw);
  addChannelSecret.value = bytesToHex(raw);
  addChannelStatus.textContent =
    "Generated locally — copy this secret somewhere safe now; it's the only way to remove this channel later.";
}

async function handleAddChannel() {
  const secretResult = validateChannelSecretHex(addChannelSecret.value);
  if (!secretResult.ok) {
    addChannelStatus.textContent = secretResult.error;
    return;
  }
  await withButtonDisabled(addChannelButton, async () => {
    addChannelStatus.textContent = "Adding channel…";
    try {
      await session.addChannel(secretResult.bytes, secretResult.keyLen, addChannelPrimary.checked, addChannelName.value);
      addChannelStatus.textContent = "Channel added.";
      addChannelForm.reset();
      await refreshLists();
    } catch (err) {
      reportWriteError(addChannelStatus, "add channel", err);
    }
  });
}

async function handleDelChannel() {
  const secretResult = validateChannelSecretHex(delChannelSecret.value);
  if (!secretResult.ok) {
    delChannelStatus.textContent = secretResult.error;
    return;
  }
  await withButtonDisabled(delChannelButton, async () => {
    delChannelStatus.textContent = "Removing channel…";
    try {
      await session.delChannel(secretResult.bytes);
      delChannelStatus.textContent = "Channel removed.";
      delChannelForm.reset();
      await refreshLists();
    } catch (err) {
      reportWriteError(delChannelStatus, "remove channel", err);
    }
  });
}

// ── Notification defaults (set-notif-defaults) ────────────────────────────
//
// Write-only, mirroring the CLI: there is no QUERY_STATUS field for the
// current defaults, so this form always starts unchecked rather than
// reflecting device state.

async function handleSetNotifDefaults() {
  await withButtonDisabled(notifSaveButton, async () => {
    notifStatus.textContent = "Saving…";
    try {
      await session.setNotifDefaults(notifVisual.checked, notifAudible.checked);
      notifStatus.textContent = `Notification defaults set: visual=${notifVisual.checked}, audible=${notifAudible.checked}`;
    } catch (err) {
      reportWriteError(notifStatus, "set notification defaults", err);
    }
  });
}

// ── Commit provisioning ───────────────────────────────────────────────────

async function handleCommit() {
  await withButtonDisabled(commitButton, async () => {
    commitStatus.textContent = "Committing…";
    try {
      await session.commit();
      commitStatus.textContent =
        "Provisioning committed — config persisted to flash. If this was the device's first commit, it will now reboot into the mesh; reconnect once it's back up.";
    } catch (err) {
      if (err instanceof DeviceError) {
        // A genuine firmware-reported failure (e.g. NVS save failed) — the
        // firmware never reboots in this path, so this is a real failure to
        // surface, not the benign reboot-and-USB-re-enumerate race below.
        console.error("MeshCadet provisioner: commit failed", err);
        commitStatus.textContent = `Couldn't commit: ${err.message}`;
      } else {
        // A transport-level failure here (read/write error, port already
        // tearing down) is indistinguishable from the expected first-boot
        // reboot-and-USB-re-enumerate race (firmware/src/provisioning_server.rs
        // sends RSP_OK, dwells 250ms specifically to outrun it, THEN calls
        // esp_restart()) — treat it as the benign case rather than surfacing a
        // failure banner for what is very likely a successful commit. The
        // `navigator.serial` "disconnect" listener above will report the
        // resulting disconnect on its own, same as any other unplug.
        console.warn("MeshCadet provisioner: commit connection closed (likely a first-boot reboot)", err);
        commitStatus.textContent =
          "Provisioning committed — the connection closed right after, which is expected on a device's first commit (it reboots into the mesh). Reconnect once it's back up.";
      }
    }
  });
}

// ── Shared write-error reporting ──────────────────────────────────────────

/**
 * Render a caught write-command error into `statusEl`. `DeviceError` (the
 * firmware's own RSP_ERROR) gets its message as-is; anything else (timeout,
 * transport failure) gets a generic prefix naming the attempted action.
 */
function reportWriteError(statusEl, action, err) {
  console.error(`MeshCadet provisioner: ${action} failed`, err);
  if (err instanceof DeviceError) {
    statusEl.textContent = `Couldn't ${action}: ${err.message}`;
  } else {
    statusEl.textContent = `Couldn't ${action}: ${(err && err.message) || err}`;
  }
}

// ── Admin PIN (set-pin / reset-pin) ───────────────────────────────────────
//
// The PIN is a secret (ADR-0007): it is validated for byte-length only,
// passed straight to `session.setPin` (which sends it over serial and scrubs
// its own buffer), and the input field is cleared the instant it's sent. It
// is never logged, echoed into a status message, placed in the URL, or
// written to storage. `validatePin` deliberately never returns the PIN, so
// there is only ever the one `setPinInput.value` copy to drop.

async function handleSetPin() {
  const pin = setPinInput.value;
  const validated = validatePin(pin);
  if (!validated.ok) {
    setPinStatus.textContent = validated.error;
    return;
  }
  setPinStatus.textContent = "Setting PIN…";
  try {
    await session.setPin(pin);
    // Clear the field immediately on success so the secret doesn't linger in
    // the DOM (it's already been scrubbed from the session buffer).
    setPinInput.value = "";
    setPinStatus.textContent = "PIN set. Physical USB possession remains the auth factor for resets.";
  } catch (err) {
    // Note: never surface the PIN itself — reportWriteError logs only the
    // error (a device RSP_ERROR message or transport failure), not the PIN.
    reportWriteError(setPinStatus, "set PIN", err);
  }
}

// ── Export history (export-history) ───────────────────────────────────────
//
// Two explicit, user-initiated steps: (1) "Read history from device" pulls
// the stream and formats a transcript held only in `pendingTranscript`;
// (2) "Download transcript" writes it to a local file. The transcript
// contains PRIVATE MESSAGE TEXT — it is never auto-downloaded, transmitted,
// or console-logged; nothing leaves the browser except the file the user
// explicitly saves.

async function handleReadHistory() {
  historyReadButton.disabled = true;
  historyDownloadButton.hidden = true;
  pendingTranscript = null;
  historyStatus.textContent = "Reading history…";
  try {
    const entries = await session.exportHistory();
    // Build the downloadable transcript in memory only. Do NOT log it.
    pendingTranscript = formatHistoryTranscript(entries);
    if (entries.length === 0) {
      historyStatus.textContent = "No conversation history on the device.";
      historyDownloadButton.hidden = true;
    } else {
      const noun = entries.length === 1 ? "message" : "messages";
      historyStatus.textContent =
        `Read ${entries.length} ${noun}. Click "Download transcript" to save them locally — nothing has left this browser.`;
      historyDownloadButton.hidden = false;
    }
  } catch (err) {
    reportWriteError(historyStatus, "export history", err);
  } finally {
    historyReadButton.disabled = false;
  }
}

function handleDownloadTranscript() {
  if (pendingTranscript === null) {
    historyStatus.textContent = "Nothing to download — read history from the device first.";
    return;
  }
  // Build a Blob and trigger a same-document download via a transient object
  // URL. This is the only path by which history text is written anywhere, and
  // it fires only from this explicit user click.
  const blob = new Blob([pendingTranscript], { type: "text/plain;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const now = new Date();
  const stamp = `${now.getFullYear()}${pad2(now.getMonth() + 1)}${pad2(now.getDate())}-${pad2(now.getHours())}${pad2(now.getMinutes())}${pad2(now.getSeconds())}`;
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `meshcadet-history-${stamp}.txt`;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  // Revoke on the next macrotask, not synchronously: some browsers abort the
  // download if the object URL is revoked before they've begun reading the
  // blob after the click. A short deferral lets the download start first.
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  historyStatus.textContent = "Transcript saved locally.";
}

function pad2(n) {
  return String(n).padStart(2, "0");
}

// ── Clear history (clear-history) — destructive, triple-gated ─────────────
//
// Gate 1: "Clear all history…" only reveals the confirmation box. Gate 2: the
// acknowledgement checkbox must be ticked to enable the erase button. Gate 3:
// a native confirm() dialog. Only after all three does the erase run.

function handleClearHistoryArm() {
  clearHistoryConfirm.hidden = false;
  clearHistoryStatus.textContent = "";
}

function resetClearHistoryConfirm() {
  clearHistoryConfirm.hidden = true;
  clearHistoryAck.checked = false;
  clearHistoryConfirmButton.disabled = true;
}

async function handleClearHistory() {
  if (!clearHistoryAck.checked) {
    return; // guard: shouldn't be reachable (button disabled), but be safe
  }
  // Final gate: an unmistakable native dialog before the irreversible erase.
  const confirmed = window.confirm(
    "Permanently erase ALL conversation history on this device? This cannot be undone."
  );
  if (!confirmed) {
    clearHistoryStatus.textContent = "Cancelled — nothing was erased.";
    return;
  }
  clearHistoryConfirmButton.disabled = true;
  clearHistoryStatus.textContent = "Erasing…";
  try {
    await session.clearHistory();
    resetClearHistoryConfirm();
    // Any transcript read before the wipe is now stale — drop it.
    pendingTranscript = null;
    historyDownloadButton.hidden = true;
    clearHistoryStatus.textContent =
      "History cleared — all conversations (DMs and channels) wiped on flash. Reboot the device to refresh its on-screen conversation views.";
  } catch (err) {
    reportWriteError(clearHistoryStatus, "clear history", err);
    // Re-enable so the user can retry without re-arming from scratch.
    clearHistoryConfirmButton.disabled = !clearHistoryAck.checked;
  }
}
