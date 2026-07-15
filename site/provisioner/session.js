// session.js — async Web Serial transport + minimal provisioning session
// orchestration for provisioner.html.
//
// A fresh async reimplementation of the relevant `host/src/session.rs`
// orchestration (`send_recv_with_retry`, the `recv_frame` accumulation loop,
// `find_magic_start` resync) for the browser's async, single-threaded Web
// Serial model. `host/src/session.rs` itself is NOT modified by this or any
// other campaign mission — it is read here only as a reference for
// orchestration shape (see docs/adr/0007-provisioner-codec.md, Finding 2).
//
// M1 walking skeleton exposed only the read-only `queryStatus()` (the
// two-frame QUERY_STATUS -> RSP_STATUS + RSP_IDENTITY handshake). M2's config
// child layered the non-sensitive provisioning **write** commands on top of
// the same `#sendRecvWithRetry`/`#recvFrame` core: list-contacts/list-channels
// enumeration, add/del contact, add/del channel, set-notif-defaults,
// set-device-name, and commit. M2's sensitive child (this change) adds the
// last three — `setPin` (masked admin PIN), `exportHistory` (streamed
// oldest-first conversation transcript), and `clearHistory` (destructive
// erase) — upholding the ADR-0007 client-side security model: the PIN is
// scrubbed from this module's own buffer after send and never logged, and
// exported history (which surfaces private message text) is returned to the
// caller only — this module never persists, transmits, or logs it.
//
// No build step: plain ES module, loaded directly by the browser.

import {
  encodeFrame,
  decodeFrame,
  findMagicStart,
  decodeRspStatus,
  decodeRspIdentity,
  decodeRspError,
  decodeRspContact,
  decodeRspChannel,
  decodeRspHistoryEntry,
  decodeRspAdvert,
  encodeAddContact,
  encodeDelContact,
  encodeAddChannel,
  encodeDelChannel,
  encodeSetNotifDefaults,
  encodeSetDeviceName,
  encodeSetPin,
  encodeQueryAdvert,
  ProvError,
  FRAME_QUERY_STATUS,
  FRAME_QUERY_CONTACTS,
  FRAME_QUERY_CHANNELS,
  FRAME_QUERY_ADVERT,
  FRAME_ADD_CONTACT,
  FRAME_DEL_CONTACT,
  FRAME_ADD_CHANNEL,
  FRAME_DEL_CHANNEL,
  FRAME_SET_NOTIF_DEFAULTS,
  FRAME_SET_PIN,
  FRAME_SET_DEVICE_NAME,
  FRAME_COMMIT_PROVISIONING,
  FRAME_EXPORT_HISTORY,
  FRAME_CLEAR_HISTORY,
  FRAME_RSP_STATUS,
  FRAME_RSP_IDENTITY,
  FRAME_RSP_ERROR,
  FRAME_RSP_OK,
  FRAME_RSP_CONTACT,
  FRAME_RSP_CONTACTS_DONE,
  FRAME_RSP_CHANNEL,
  FRAME_RSP_CHANNELS_DONE,
  FRAME_RSP_HISTORY_ENTRY,
  FRAME_RSP_HISTORY_DONE,
  FRAME_RSP_ADVERT,
  MAX_RSP_HISTORY_ENTRY_PAYLOAD,
  MAX_ADVERT_CARD_LEN,
} from "./codec.js";

// Matches the host CLI's `--baud` default (`host/src/main.rs`).
const BAUD_RATE = 115200;

// Mirrors `Session::new`'s defaults (`host/src/session.rs`): 500 ms per retry
// attempt, 10 s overall retry budget, 5 s per-frame timeout once synced.
const RETRY_ATTEMPT_MS = 500;
const RETRY_TOTAL_MS = 10_000;
const FRAME_TIMEOUT_MS = 5_000;

/**
 * Upper bound on any legitimate provisioning frame payload — used only by
 * `#tryExtractFrame`'s false-`PROV_MAGIC`-in-log-noise guard to tell a
 * genuine frame header apart from ASCII log traffic that happens to contain
 * the two magic bytes. Must track the single largest payload any frame type
 * can carry. Mirrors `MAX_VALID_FRAME_PAYLOAD_LEN` (`host/src/session.rs`):
 * `FRAME_RSP_ADVERT`'s self-advert card (up to `MAX_ADVERT_CARD_LEN` = 134
 * bytes) is currently the largest, ahead of `FRAME_RSP_HISTORY_ENTRY`
 * (`MAX_RSP_HISTORY_ENTRY_PAYLOAD` = 73 bytes) — a guard hardcoded to the
 * smaller of the two would misclassify every genuine advert-card frame as
 * noise and byte-drain it into a timeout.
 */
const MAX_VALID_FRAME_PAYLOAD_LEN = Math.max(MAX_RSP_HISTORY_ENTRY_PAYLOAD, MAX_ADVERT_CARD_LEN);

/**
 * Every provisioning response frame type this protocol defines. Used by
 * `#recvUntilExpected` (and `exportHistory`'s own stream loop) to tell a
 * genuine-but-late reply to an EARLIER command (any OTHER type from this
 * set) apart from truly unrecognized/corrupted wire garbage — see
 * `#recvUntilExpected`'s doc comment for why that distinction matters.
 */
const ALL_RSP_FRAME_TYPES = new Set([
  FRAME_RSP_OK,
  FRAME_RSP_ERROR,
  FRAME_RSP_STATUS,
  FRAME_RSP_IDENTITY,
  FRAME_RSP_HISTORY_ENTRY,
  FRAME_RSP_HISTORY_DONE,
  FRAME_RSP_CONTACT,
  FRAME_RSP_CONTACTS_DONE,
  FRAME_RSP_CHANNEL,
  FRAME_RSP_CHANNELS_DONE,
  FRAME_RSP_ADVERT,
]);

/**
 * Bound on stray leftover-frame tolerance, shared by every read path that
 * tolerates them (`#recvUntilExpected`, `exportHistory`) — a genuinely
 * stuck device or corrupted stream must still surface as an error rather
 * than spin silently.
 */
const MAX_STRAY_FRAMES = 64;

/**
 * Thrown when the device answers a command with `RSP_ERROR`.
 * Mirrors the `anyhow::bail!("device error {}: {}", ...)` sites in
 * `host/src/session.rs`.
 */
export class DeviceError extends Error {
  constructor(errorCode, msg) {
    super(`device error ${errorCode}: ${msg}`);
    this.name = "DeviceError";
    this.errorCode = errorCode;
  }
}

function hex2(n) {
  return n.toString(16).toUpperCase().padStart(2, "0");
}

/**
 * A provisioning session over a single Web Serial port.
 *
 * Unlike `Session<T: Transport>` (synchronous, blocking `recv`), this class
 * runs one continuous background read loop (`#readLoop`) for the lifetime of
 * the connection and lets `#recvFrame` wait on that shared, ever-growing
 * accumulation buffer instead of racing concurrent `reader.read()` calls
 * against a timeout — issuing two `read()`s concurrently on the same
 * `ReadableStreamDefaultReader` would leave a stray pending read whose
 * eventual data could otherwise be silently dropped.
 */
export class ProvisionerSession {
  #port = null;
  #reader = null;
  #writer = null;
  #readLoopPromise = null;
  #accBuf = new Uint8Array(0);
  #waiters = [];
  /**
   * FIFO serialization queue for the command methods below (`queryStatus`,
   * `listContacts`/`listChannels`, `addContact`/`delContact`,
   * `addChannel`/`delChannel`, `setNotifDefaults`, `setDeviceName`,
   * `commit`). The physical link allows exactly one outstanding
   * request/response at a time — `#sendRecvWithRetry`/`#recvFrame` assume
   * it, matching `host/src/session.rs`'s `&mut self` methods, which the
   * borrow checker already serializes for free. Nothing enforces that on
   * this async, single-threaded-but-still-concurrent side: two command
   * calls issued close together (e.g. a background status refresh racing a
   * form submit) would otherwise interleave their writes and desync the
   * request/response protocol — one call could receive the frame meant for
   * the other. `#exclusive` queues command bodies so only one runs at a
   * time; `connect`/`disconnect` are connection-lifecycle, not commands,
   * and deliberately run outside this queue so a stuck request doesn't
   * block tearing down the connection.
   */
  #queue = Promise.resolve();

  /** Whether this browser exposes the Web Serial API at all. */
  static isSupported() {
    return "serial" in navigator;
  }

  /** Whether the current page is loaded in a context Web Serial permits (HTTPS or localhost). */
  static isSecureContext() {
    return window.isSecureContext === true;
  }

  get isConnected() {
    return this.#port !== null;
  }

  /** The underlying `SerialPort`, or `null` if not connected. Exposed so callers can match it against `navigator.serial`'s `"disconnect"` event's `event.target`. */
  get port() {
    return this.#port;
  }

  /**
   * Prompt the user (Web Serial's native "choose a device" picker — requires
   * a user gesture, e.g. a click handler calling this directly) to select a
   * port, then open it and start the background read loop.
   *
   * Throws `DOMException` with `name === "NotFoundError"` if the user
   * dismisses the picker without choosing a device — callers should treat
   * that as a silent cancel, not an error to surface.
   */
  async connect() {
    const port = await navigator.serial.requestPort();
    await port.open({ baudRate: BAUD_RATE });
    this.#port = port;
    this.#writer = port.writable.getWriter();
    this.#reader = port.readable.getReader();
    this.#accBuf = new Uint8Array(0);
    this.#readLoopPromise = this.#readLoop();
  }

  /** Close the port and release all resources. Safe to call when not connected. */
  async disconnect() {
    if (!this.#port) {
      return;
    }
    try {
      await this.#reader.cancel();
    } catch {
      // Port may already be gone (device unplugged) — fall through to cleanup.
    }
    try {
      await this.#readLoopPromise;
    } catch {
      // #readLoop's own read() rejects when cancel()/disconnect races it; the
      // loop's catch already swallows and returns, so this is defensive only.
    }
    this.#reader.releaseLock();
    this.#writer.releaseLock();
    try {
      await this.#port.close();
    } catch {
      // Already closed (e.g. device physically unplugged) — nothing to do.
    }
    this.#port = null;
    this.#reader = null;
    this.#writer = null;
    this.#accBuf = new Uint8Array(0);
    this.#rejectAllWaiters(new Error("session disconnected"));
  }

  /**
   * Query the device's provisioning status and identity: sends
   * `FRAME_QUERY_STATUS` and consumes the two response frames the firmware
   * always sends for it (`RSP_STATUS` then `RSP_IDENTITY`), mirroring
   * `Session::query_status` (`host/src/session.rs`).
   *
   * Returns `{ status, identity }`, the decoded payload objects from
   * `codec.js`'s `decodeRspStatus`/`decodeRspIdentity`.
   */
  async queryStatus() {
    return this.#exclusive(async () => {
      const first = await this.#sendRecvWithRetry(
        FRAME_QUERY_STATUS,
        new Uint8Array(0),
        (ft) => ft === FRAME_RSP_STATUS,
        "QUERY_STATUS's RSP_STATUS"
      );
      let status;
      if (first.frameType === FRAME_RSP_STATUS) {
        status = decodeRspStatus(first.payload);
      } else if (first.frameType === FRAME_RSP_ERROR) {
        throw deviceErrorFrom(first.payload);
      } else {
        throw new Error(`unexpected response 0x${hex2(first.frameType)} to QUERY_STATUS`);
      }

      // Consume the trailing RSP_IDENTITY frame the firmware always sends
      // after RSP_STATUS — leaving it unread would desync the next command,
      // exactly as documented on `Session::query_status`.
      const second = await this.#recvFrame(FRAME_TIMEOUT_MS);
      if (second.frameType !== FRAME_RSP_IDENTITY) {
        throw new Error(
          `expected RSP_IDENTITY (0x${hex2(FRAME_RSP_IDENTITY)}) after RSP_STATUS; got 0x${hex2(second.frameType)}`
        );
      }
      const identity = decodeRspIdentity(second.payload);
      return { status, identity };
    });
  }

  /**
   * Ask the device to build and return its signed self-advert "biz card"
   * (`protocol::advert::build_self_advert_card`). Mirrors `Session::query_advert`
   * (`host/src/session.rs`): sends `FRAME_QUERY_ADVERT` carrying the
   * BROWSER's current wall-clock unix time — the device has no RTC of its
   * own and stamps the card with this value — and returns the raw card
   * bytes from `FRAME_RSP_ADVERT`, validated by `codec.js`'s
   * `decodeRspAdvert`.
   *
   * This is "Format B" — the string `meshcore-cli import-contact <URI>`
   * expects, rendered via `contact-uri.js`'s `cardToUri`. Campaign guard:
   * the browser cannot synthesize this card itself (the signature needs the
   * device's Ed25519 private key, which never leaves it) — this call is the
   * only legitimate source of one; never build a Format B card client-side.
   */
  async queryAdvert() {
    return this.#exclusive(async () => {
      const hostUnixTime = Math.floor(Date.now() / 1000);
      const { frameType, payload } = await this.#sendRecvWithRetry(
        FRAME_QUERY_ADVERT,
        encodeQueryAdvert(hostUnixTime),
        (ft) => ft === FRAME_RSP_ADVERT,
        "QUERY_ADVERT's RSP_ADVERT"
      );
      if (frameType === FRAME_RSP_ADVERT) {
        return decodeRspAdvert(payload);
      }
      if (frameType === FRAME_RSP_ERROR) {
        throw deviceErrorFrom(payload);
      }
      throw new Error(
        `unexpected response 0x${hex2(frameType)} to QUERY_ADVERT (expected RSP_ADVERT 0x${hex2(FRAME_RSP_ADVERT)})`
      );
    });
  }

  /**
   * Enumerate the device's configured contacts. Sends `FRAME_QUERY_CONTACTS`,
   * then consumes the streamed `FRAME_RSP_CONTACT` frames terminated by
   * `FRAME_RSP_CONTACTS_DONE`. Mirrors `Session::list_contacts`
   * (`host/src/session.rs`). Returns entries in device-index order.
   *
   * Served against the firmware's in-progress (pre-commit) staging config —
   * pair with `addContact`/`delContact`/`queryStatus` to verify the
   * configured set before `commit()`.
   */
  async listContacts() {
    return this.#exclusive(() =>
      this.#streamUntilDone(FRAME_QUERY_CONTACTS, FRAME_RSP_CONTACT, FRAME_RSP_CONTACTS_DONE, decodeRspContact, "contact")
    );
  }

  /**
   * Enumerate the device's configured channels. Sends `FRAME_QUERY_CHANNELS`,
   * then consumes the streamed `FRAME_RSP_CHANNEL` frames terminated by
   * `FRAME_RSP_CHANNELS_DONE`. Mirrors `Session::list_channels`
   * (`host/src/session.rs`). Returns entries in device-index order.
   */
  async listChannels() {
    return this.#exclusive(() =>
      this.#streamUntilDone(FRAME_QUERY_CHANNELS, FRAME_RSP_CHANNEL, FRAME_RSP_CHANNELS_DONE, decodeRspChannel, "channel")
    );
  }

  /**
   * Add a contact. `pubkey` is a 32-byte `Uint8Array` (Ed25519 public key);
   * `name` is a UTF-8 display name (empty = device falls back to the routing
   * hash as a label). Mirrors `Session::add_contact`.
   */
  async addContact(pubkey, telemetryEnable, name) {
    await this.#exclusive(() => this.#sendAndExpectOk(FRAME_ADD_CONTACT, encodeAddContact(pubkey, telemetryEnable, name)));
  }

  /** Delete a contact by its 32-byte Ed25519 public key. Mirrors `Session::del_contact`. */
  async delContact(pubkey) {
    await this.#exclusive(() => this.#sendAndExpectOk(FRAME_DEL_CONTACT, encodeDelContact(pubkey)));
  }

  /**
   * Add (or replace) a channel. `secret` is a 32-byte `Uint8Array` (128-bit
   * secrets zero-padded to 32 bytes by the caller — see
   * `provisioner/validation.js`'s `validateChannelSecretHex`); `keyLen` is 16
   * or 32. Mirrors `Session::add_channel`. The channel hash is computed by
   * the firmware, never by this page.
   */
  async addChannel(secret, keyLen, primary, name) {
    await this.#exclusive(() => this.#sendAndExpectOk(FRAME_ADD_CHANNEL, encodeAddChannel(secret, keyLen, primary, name)));
  }

  /**
   * Delete a channel by its 32-byte secret — must match exactly what was
   * passed to `addChannel` (the device has no other way to identify a
   * channel for removal; the 1-byte hash alone is not enough). Mirrors
   * `Session::del_channel`.
   */
  async delChannel(secret) {
    await this.#exclusive(() => this.#sendAndExpectOk(FRAME_DEL_CHANNEL, encodeDelChannel(secret)));
  }

  /** Set notification defaults (visual/audible). Mirrors `Session::set_notif_defaults`. */
  async setNotifDefaults(visual, audible) {
    await this.#exclusive(() => this.#sendAndExpectOk(FRAME_SET_NOTIF_DEFAULTS, encodeSetNotifDefaults(visual, audible)));
  }

  /**
   * Set (or clear, with an empty string) the device display name. Persists
   * to the device's identity store (NVS) immediately, independent of
   * first-boot provisioning state. Mirrors `Session::set_device_name`.
   * `name` must be ≤ `MAX_NAME_LEN` (32) bytes UTF-8 — validate with
   * `provisioner/validation.js`'s `validateDeviceName` before calling.
   */
  async setDeviceName(name) {
    await this.#exclusive(() => this.#sendAndExpectOk(FRAME_SET_DEVICE_NAME, encodeSetDeviceName(name)));
  }

  /**
   * Commit provisioning: persist the staged config to flash. Mirrors
   * `Session::commit` (`host/src/session.rs`) — a plain "send, expect
   * RSP_OK" call with no special-cased branching here.
   *
   * On a first-boot device, the firmware (`firmware/src/provisioning_server.rs`)
   * sends RSP_OK, dwells 250 ms (specifically to outrun the ensuing
   * `esp_restart()`'s USB re-enumeration — see that file's "USB-DRAIN GUARD"
   * comment), then reboots into the mesh, closing the serial connection. An
   * already-provisioned device's runtime handler
   * (`firmware/src/admin_server.rs`) replies RSP_OK WITHOUT rebooting. Either
   * way, by the time this method resolves the RSP_OK has already been
   * received — the deliberate dwell is what makes that dependable. Any
   * *subsequent* port teardown (the reboot case) is observed later via
   * `navigator.serial`'s `"disconnect"` event, which `provisioner.js` already
   * treats as a benign disconnect, not a failure — this method itself never
   * needs to distinguish the two cases.
   */
  async commit() {
    await this.#exclusive(() => this.#sendAndExpectOk(FRAME_COMMIT_PROVISIONING, new Uint8Array(0)));
  }

  /**
   * Set (or reset) the device admin PIN. `pin` is a UTF-8 string, silently
   * truncated to `MAX_PIN_LEN` (16) bytes by `encodeSetPin` — validate/trim
   * upstream if you need to reject rather than truncate. Mirrors
   * `Session::set_pin` (`host/src/session.rs`).
   *
   * ADR-0007 security model: the PIN is a secret. This method holds it only in
   * the transient `payload` buffer for the duration of the send, then scrubs
   * that buffer (`.fill(0)`) once the retry loop can no longer reference it —
   * it is never stored on the instance, logged, placed in the URL, or written
   * to `localStorage`/`sessionStorage`. (The caller owns the `pin` string
   * itself; JS strings are immutable and cannot be scrubbed, so the caller
   * should also drop its reference and clear any input field after this
   * resolves — see `provisioner.js`'s `handleSetPin`.)
   */
  async setPin(pin) {
    const payload = encodeSetPin(pin);
    try {
      await this.#sendAndExpectOk(FRAME_SET_PIN, payload);
    } finally {
      // Scrub the PIN bytes from our buffer now that no retry can re-send it.
      payload.fill(0);
    }
  }

  /**
   * Clear ALL persisted conversation history on the device — every sent and
   * received message across every DM contact and channel. Destructive and
   * irreversible; gate behind an explicit user confirmation (see
   * `provisioner.js`'s `handleClearHistory`). Mirrors `Session::clear_history`
   * (`host/src/session.rs`). The erase hits flash immediately, but the
   * device's on-screen conversation views only refresh after a reboot (they
   * hold an in-memory copy hydrated at boot).
   */
  async clearHistory() {
    await this.#sendAndExpectOk(FRAME_CLEAR_HISTORY, new Uint8Array(0));
  }

  /**
   * Export conversation history from the device, oldest-first. Sends
   * `FRAME_EXPORT_HISTORY`, then consumes the streamed
   * `FRAME_RSP_HISTORY_ENTRY` frames terminated by `FRAME_RSP_HISTORY_DONE`.
   * Mirrors `Session::export_history` (`host/src/session.rs`), including its
   * bounded tolerance of stray well-formed replies to an *earlier* command
   * that can still be in flight when the stream begins.
   *
   * Returns an array of decoded history-entry objects (see
   * `codec.js`'s `decodeRspHistoryEntry` — each carries `is_ours`, which
   * distinguishes a sent message from a received one since `sender_hash` is
   * always the conversation hash regardless of direction).
   *
   * ADR-0007 security model: the returned entries contain **private message
   * text**. This method returns them to the caller and does nothing else —
   * it never logs, persists, or transmits them. The caller must keep them
   * client-side (see `provisioner.js`'s explicit user-initiated download).
   */
  async exportHistory() {
    // Retry only the initial command (a timing race is healed before the
    // stream begins); the first frame is HISTORY_ENTRY, HISTORY_DONE, or
    // RSP_ERROR — all handled in the loop below, mirroring the Rust version.
    let { frameType, payload } = await this.#sendRecvWithRetry(FRAME_EXPORT_HISTORY, new Uint8Array(0));

    const entries = [];
    let strayFrames = 0;

    while (true) {
      if (frameType === FRAME_RSP_HISTORY_ENTRY) {
        const entry = decodeRspHistoryEntry(payload);
        if (entry === null) {
          throw new Error("malformed RSP_HISTORY_ENTRY payload");
        }
        entries.push(entry);
      } else if (frameType === FRAME_RSP_HISTORY_DONE) {
        break;
      } else if (frameType === FRAME_RSP_ERROR) {
        throw deviceErrorFrom(payload);
      } else if (ALL_RSP_FRAME_TYPES.has(frameType)) {
        // A leftover well-formed reply to an earlier command still draining
        // over USB — tolerate a bounded number rather than mistaking it for a
        // corrupted stream. Checked against the FULL recognized-response set
        // (not a hand-maintained list) so a newly added frame type — like
        // FRAME_RSP_ADVERT, added after this tolerance list was first
        // written — is covered automatically instead of silently falling
        // through to the hard-fail branch below.
        strayFrames += 1;
        if (strayFrames > MAX_STRAY_FRAMES) {
          throw new Error(
            `too many stray non-history frames (last: 0x${hex2(frameType)}) during history export`
          );
        }
      } else {
        throw new Error(`unexpected frame 0x${hex2(frameType)} during history export`);
      }
      // Next streaming frame — no retry: the device is awake and streaming, so
      // a timeout here is a genuine protocol error.
      ({ frameType, payload } = await this.#recvFrame(FRAME_TIMEOUT_MS));
    }
    return entries;
  }

  // ── Low-level frame I/O ───────────────────────────────────────────────────

  /**
   * Run `fn` once every earlier-queued `#exclusive` call has settled, and
   * queue anyone who calls `#exclusive` while `fn` is running behind it —
   * a plain FIFO async mutex. `fn`'s rejection propagates to its own caller
   * without breaking the chain for whoever is queued next.
   */
  async #exclusive(fn) {
    const previous = this.#queue;
    let release;
    this.#queue = new Promise((resolve) => {
      release = resolve;
    });
    await previous;
    try {
      return await fn();
    } finally {
      release();
    }
  }

  async #sendFrame(frameType, payload) {
    await this.#writer.write(encodeFrame(frameType, payload));
  }

  /**
   * Send a command frame and assert the response is `RSP_OK`. Mirrors
   * `Session::send_and_expect_ok` (`host/src/session.rs`). Throws
   * `DeviceError` on `RSP_ERROR`, or a plain `Error` on any other
   * unexpected response frame.
   */
  async #sendAndExpectOk(frameType, payload) {
    const { frameType: ft, payload: rspPayload } = await this.#sendRecvWithRetry(
      frameType,
      payload,
      (t) => t === FRAME_RSP_OK,
      "RSP_OK"
    );
    if (ft === FRAME_RSP_OK) {
      return;
    }
    if (ft === FRAME_RSP_ERROR) {
      throw deviceErrorFrom(rspPayload);
    }
    throw new Error(`unexpected response 0x${hex2(ft)} (expected RSP_OK 0x${hex2(FRAME_RSP_OK)})`);
  }

  /**
   * Send `queryFrameType`, then consume the streamed response — repeated
   * `entryFrameType` frames (decoded with `decodeEntry`) terminated by a
   * single `doneFrameType` frame. Shared by `listContacts`/`listChannels`,
   * mirroring `Session::list_contacts`/`Session::list_channels`
   * (`host/src/session.rs`).
   */
  async #streamUntilDone(queryFrameType, entryFrameType, doneFrameType, decodeEntry, label) {
    let { frameType, payload } = await this.#sendRecvWithRetry(
      queryFrameType,
      new Uint8Array(0),
      (ft) => ft === entryFrameType || ft === doneFrameType,
      `${label} enumeration`
    );
    const entries = [];
    while (true) {
      if (frameType === entryFrameType) {
        entries.push(decodeEntry(payload));
      } else if (frameType === doneFrameType) {
        break;
      } else if (frameType === FRAME_RSP_ERROR) {
        throw deviceErrorFrom(payload);
      } else {
        throw new Error(`unexpected frame 0x${hex2(frameType)} during ${label} enumeration`);
      }
      ({ frameType, payload } = await this.#recvFrame(FRAME_TIMEOUT_MS));
    }
    return entries;
  }

  /**
   * Send a command frame and wait for the response, retrying the send every
   * `RETRY_ATTEMPT_MS` until a valid response frame arrives or
   * `RETRY_TOTAL_MS` has elapsed. Mirrors `Session::send_recv_with_retry`.
   *
   * Unlike the Rust version, there is no `flush_input()` step between
   * retries: Web Serial exposes no OS-buffer-clear primitive, but since
   * `#readLoop` continuously drains the port into `#accBuf` there is no
   * separate kernel buffer accumulating behind the scenes to flush — clearing
   * `#accBuf` itself (below) is the JS-side equivalent.
   *
   * CROSS-COMMAND RESIDUE GUARD: every top-level command (`queryStatus`,
   * `listContacts`, ...) enters here exactly once as its first frame I/O.
   * `#exclusive` guarantees no other command can be mid-exchange when that
   * happens, so any bytes already sitting in `#accBuf` at this point cannot
   * belong to the command about to be sent — they can only be leftovers from
   * an *earlier* command's exchange (most commonly: a retry whose original,
   * pre-timeout attempt the device answered anyway, arriving after that
   * exchange had already resolved on the retry's reply — mirrors the "stray
   * reply to an earlier command" scenario `Session::export_history` documents
   * on the Rust side, except here it can poison a *different, later* command
   * instead of just the same one). Rust doesn't need this guard: each CLI
   * invocation opens a fresh port/process, so there is no cross-command
   * lifetime for residue to leak across. The browser session is long-lived
   * across many commands, so it must discard that residue itself before
   * starting a new exchange, or the next command reads someone else's
   * response and misreports it as "unexpected frame".
   *
   * THIS GUARD ALONE IS NOT SUFFICIENT, and its gap is exactly what let the
   * cross-command desync regress (`meshcadet-provisioner-advert-frame-
   * desync-regression` mission): it only discards residue that has already
   * fully arrived by the time THIS call starts. It does nothing about a
   * stray reply that arrives *during* this call's own wait — which is
   * precisely what happens when the command about to run (e.g.
   * `queryAdvert`, whose device-side signing + NVS write takes noticeably
   * longer than a plain status/contact/channel query) is slower than the
   * leftover reply still in flight from an earlier command. Passing
   * `isExpected`/`label` through to `#recvUntilExpected` below closes that
   * remaining gap: a frame that doesn't match what THIS command asked for,
   * but is still a recognized provisioning response type, is treated as
   * that leftover and discarded so the real answer is not orphaned to
   * become the *next* command's residue in turn (the one-command-behind
   * cascade: QUERY_ADVERT reads a trailing RSP_STATUS, then QUERY_CONTACTS
   * reads the RSP_ADVERT that QUERY_ADVERT never waited for, then
   * QUERY_CHANNELS reads the first RSP_CONTACT that QUERY_CONTACTS never
   * waited for).
   *
   * `isExpected`/`label` default to accepting whatever arrives (matching
   * the old, non-tolerant behavior) for callers — `exportHistory` — that
   * already implement their own bounded stray-frame tolerance downstream.
   */
  async #sendRecvWithRetry(frameType, payload, isExpected = () => true, label = "response") {
    this.#accBuf = new Uint8Array(0);
    const overallDeadline = Date.now() + RETRY_TOTAL_MS;
    while (true) {
      await this.#sendFrame(frameType, payload);
      try {
        return await this.#recvUntilExpected(RETRY_ATTEMPT_MS, isExpected, label);
      } catch (err) {
        if (Date.now() >= overallDeadline) {
          throw err;
        }
        // This attempt timed out but we still have overall budget — clear
        // the accumulated bytes so stale log noise from the device-side
        // processing delay doesn't confuse the next recvFrame call, then
        // loop → send again.
        this.#accBuf = new Uint8Array(0);
      }
    }
  }

  /**
   * Wait for a frame whose type satisfies `isExpected`, silently discarding
   * any OTHER *recognized* provisioning response frame (`ALL_RSP_FRAME_TYPES`)
   * along the way instead of returning it to the caller as a desync.
   *
   * WHY THIS EXISTS: `#exclusive` guarantees only one command is ever
   * mid-exchange, so a frame that doesn't match what the CURRENT command
   * asked for cannot legitimately belong to it — it can only be the
   * genuine (correctly formed, just late) reply to an EARLIER command
   * whose own read already gave up (timed out, or itself hit a stray frame
   * and threw) before the device's real answer made it back over USB.
   * Discarding those strays HERE — rather than only at the top of
   * `#sendRecvWithRetry`, which catches just the residue that has already
   * fully arrived before the next command starts — is what stops that
   * residue from cascading onto whatever command runs after this one.
   *
   * An UNRECOGNIZED byte (not in `ALL_RSP_FRAME_TYPES`) is never tolerated:
   * that is genuine corruption or a protocol mismatch, not residue, and
   * must still surface immediately. `RSP_ERROR` is always accepted too, so
   * callers keep handling device errors themselves. Bounded by
   * `MAX_STRAY_FRAMES` so a truly stuck device or corrupted stream still
   * surfaces as an error instead of spinning silently until timeout.
   */
  async #recvUntilExpected(timeoutMs, isExpected, label) {
    const deadline = Date.now() + timeoutMs;
    let strayFrames = 0;
    while (true) {
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        throw new Error(`timeout waiting for response frame (accumulated ${this.#accBuf.length} bytes)`);
      }
      const frame = await this.#recvFrame(remaining);
      if (frame.frameType === FRAME_RSP_ERROR || isExpected(frame.frameType)) {
        return frame;
      }
      if (ALL_RSP_FRAME_TYPES.has(frame.frameType)) {
        strayFrames += 1;
        if (strayFrames > MAX_STRAY_FRAMES) {
          throw new Error(
            `too many stray frames (last: 0x${hex2(frame.frameType)}) waiting for ${label}`
          );
        }
        // Leftover reply to an earlier command, still draining — discard
        // and keep waiting for the real answer.
        continue;
      }
      throw new Error(`unexpected frame 0x${hex2(frame.frameType)} (waiting for ${label})`);
    }
  }

  /**
   * Wait until a complete provisioning frame is available in `#accBuf` (fed
   * by `#readLoop`), then decode and return `{ frameType, payload }`.
   * Mirrors `Session::recv_frame`'s accumulation-and-resync loop, including
   * the same `find_magic_start` resync and CRC/magic recovery.
   */
  async #recvFrame(timeoutMs) {
    const deadline = Date.now() + timeoutMs;
    while (true) {
      const frame = this.#tryExtractFrame();
      if (frame) {
        return frame;
      }
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        throw new Error(`timeout waiting for response frame (accumulated ${this.#accBuf.length} bytes)`);
      }
      await this.#waitForDataOrTimeout(remaining);
    }
  }

  /**
   * Try to pull one complete frame out of `#accBuf` without blocking.
   * Returns `null` if there isn't a complete frame yet.
   */
  #tryExtractFrame() {
    // Looped (not recursive) so a long run of non-frame garbage — e.g. a
    // verbose ESP-IDF log burst — resyncs one byte at a time without risking
    // a stack overflow on a large accumulation buffer.
    while (true) {
      // Discard bytes preceding a PROV_MAGIC candidate — the device writes
      // ESP-IDF log lines on the same USB-serial stream interleaved with
      // binary frames (find_magic_start resync, mirroring session.rs).
      const sync = findMagicStart(this.#accBuf);
      if (sync > 0) {
        this.#accBuf = this.#accBuf.slice(sync);
      }

      if (this.#accBuf.length < 5) {
        return null;
      }
      const plen = this.#accBuf[3] | (this.#accBuf[4] << 8);
      // Guard against a false PROV_MAGIC in log traffic: every real payload
      // fits within MAX_VALID_FRAME_PAYLOAD_LEN, so a larger plen means the
      // "MC" bytes were ASCII log noise, not a real frame header. Advance 1
      // byte and re-scan.
      if (plen > MAX_VALID_FRAME_PAYLOAD_LEN) {
        this.#accBuf = this.#accBuf.slice(1);
        continue;
      }
      const total = 7 + plen;
      if (this.#accBuf.length < total) {
        return null;
      }
      try {
        const { frameType, payload } = decodeFrame(this.#accBuf.subarray(0, total));
        const result = { frameType, payload: payload.slice() };
        this.#accBuf = this.#accBuf.slice(total);
        return result;
      } catch (err) {
        if (err instanceof ProvError && (err.kind === "CrcMismatch" || err.kind === "BadMagic")) {
          // False PROV_MAGIC sequence in log traffic: advance 1 byte past the
          // fake magic and re-scan.
          this.#accBuf = this.#accBuf.slice(1);
          continue;
        }
        throw err;
      }
    }
  }

  // ── Background read loop + waiter notification ───────────────────────────

  async #readLoop() {
    try {
      while (true) {
        const { value, done } = await this.#reader.read();
        if (done) {
          return;
        }
        if (value && value.length) {
          const merged = new Uint8Array(this.#accBuf.length + value.length);
          merged.set(this.#accBuf, 0);
          merged.set(value, this.#accBuf.length);
          this.#accBuf = merged;
          this.#notifyWaiters();
        }
      }
    } catch (err) {
      // Port error (e.g. device physically unplugged mid-read) — surface it
      // to anyone currently waiting on new data instead of hanging them.
      this.#rejectAllWaiters(err instanceof Error ? err : new Error(String(err)));
    }
  }

  /** Resolve when either new data arrives via `#readLoop`, or `ms` elapses — whichever first. */
  #waitForDataOrTimeout(ms) {
    return new Promise((resolve, reject) => {
      const waiter = {
        resolve: () => {
          clearTimeout(timer);
          this.#removeWaiter(waiter);
          resolve();
        },
        reject: (err) => {
          clearTimeout(timer);
          this.#removeWaiter(waiter);
          reject(err);
        },
      };
      const timer = setTimeout(() => waiter.resolve(), ms);
      this.#waiters.push(waiter);
    });
  }

  #removeWaiter(waiter) {
    const idx = this.#waiters.indexOf(waiter);
    if (idx !== -1) {
      this.#waiters.splice(idx, 1);
    }
  }

  #notifyWaiters() {
    const waiters = this.#waiters;
    this.#waiters = [];
    for (const w of waiters) {
      w.resolve();
    }
  }

  #rejectAllWaiters(err) {
    const waiters = this.#waiters;
    this.#waiters = [];
    for (const w of waiters) {
      w.reject(err);
    }
  }
}

/** Build a `DeviceError` from a decoded `RSP_ERROR` payload's raw bytes. */
function deviceErrorFrom(payload) {
  const e = decodeRspError(payload);
  return new DeviceError(e.error_code, e.msg);
}
