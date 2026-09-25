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
// This mission (meshcadet-room-web-provisioner) mirrors the room-server
// provisioning verbs already proven end-to-end through the host CLI
// (`meshcadet-room-host-cli`) into this session layer: `listRooms`,
// `addRoom`, `delRoom`, against the room frames `meshcadet-room-provisioning-
// contract`/child 3 already added to `codec.js`. `addRoom` follows `setPin`'s
// scrub-after-send discipline for the guest password (ADR-0002 §4/ADR-0001
// §4: the password crosses USB in the clear by design, but must never
// linger in this module's own state, be logged, or be persisted).
//
// This mission (meshcadet-lock-web-provisioner) adds the screen-lock send
// path: `setLockPin` (masked, distinct from the admin PIN — same
// scrub-after-send discipline) and `setLockConfig` (enable flag + idle
// timeout, a plain `#sendAndExpectOk` wrapper like `setNotifDefaults`). See
// `docs/adr/0013-screen-lock-policy-layer.md`.
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
  decodeRspRoom,
  decodeRspHistoryEntry,
  decodeRspAdvert,
  encodeAddContact,
  encodeDelContact,
  encodeAddChannel,
  encodeDelChannel,
  encodeAddRoom,
  encodeDelRoom,
  encodeSetNotifDefaults,
  encodeSetDeviceName,
  encodeSetPin,
  encodeSetLockPin,
  encodeSetLockConfig,
  encodeQueryAdvert,
  ProvError,
  FRAME_QUERY_STATUS,
  FRAME_QUERY_CONTACTS,
  FRAME_QUERY_CHANNELS,
  FRAME_QUERY_ADVERT,
  FRAME_QUERY_ROOMS,
  FRAME_ADD_CONTACT,
  FRAME_DEL_CONTACT,
  FRAME_ADD_CHANNEL,
  FRAME_DEL_CHANNEL,
  FRAME_ADD_ROOM,
  FRAME_DEL_ROOM,
  FRAME_SET_NOTIF_DEFAULTS,
  FRAME_SET_PIN,
  FRAME_SET_LOCK_PIN,
  FRAME_SET_LOCK_CONFIG,
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
  FRAME_RSP_ROOM,
  FRAME_RSP_ROOMS_DONE,
  FRAME_RSP_HISTORY_ENTRY,
  FRAME_RSP_HISTORY_DONE,
  FRAME_RSP_ADVERT,
  FRAME_RSP_LOCK,
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
 *
 * `FRAME_RSP_LOCK` was missing here until
 * `meshcadet-web-provisioner-read-timeout-after-reset` (this module has no
 * `queryLock()`/`FRAME_QUERY_LOCK` caller yet, so it could never appear on
 * the wire through this client's own actions — found by inspection, not a
 * live symptom) — every OTHER `FRAME_RSP_*` codec.js defines was already
 * listed, so the omission was an oversight, not a deliberate exclusion.
 * Fixed for the day a `queryLock()`/lock-status UI lands and a leftover
 * `RSP_LOCK` reply needs the same stray-tolerance every other response type
 * already gets.
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
  FRAME_RSP_ROOM,
  FRAME_RSP_ROOMS_DONE,
  FRAME_RSP_ADVERT,
  FRAME_RSP_LOCK,
]);

/**
 * Bound on stray leftover-frame tolerance, shared by every read path that
 * tolerates them (`#recvUntilExpected`, `exportHistory`) — a genuinely
 * stuck device or corrupted stream must still surface as an error rather
 * than spin silently.
 */
const MAX_STRAY_FRAMES = 64;

/**
 * Cap on `#discardPreview`'s size — "the first ~512 discarded bytes",
 * enough for a human debugging a live wedge to actually read the content of
 * the "N bytes received, zero valid frames" traffic (an ESP-IDF boot banner
 * is typically a few hundred bytes) without dumping unbounded megabytes to
 * the console on a long-running, persistently noisy session.
 */
const DISCARD_PREVIEW_CAP = 512;

/**
 * ASCII prefix of the ESP32-S3 ROM's own boot banner
 * (`ESP-ROM:esp32s3-<hash>` — the exact text confirmed in kernel evidence,
 * 2026-09-22, `docs/provisioning-connect-verification-kit.md`). Printed
 * only by the ROM itself immediately after a USB-Serial-JTAG chip reset
 * (`rst:0x15 (USB_UART_CHIP_RESET)`) — a plain firmware-level reboot
 * (watchdog, panic, `esp_restart()`) never emits it. Its presence in the
 * discarded (non-frame) traffic is therefore an unambiguous signal that the
 * DEVICE reset, distinct from an ordinary slow response.
 * `#scanForRebootBanner` counts occurrences (not just detects one) so a
 * device that resets more than once during a single stuck command can be
 * reported accurately.
 *
 * This banner's presence does NOT mean the device "re-enumerated out from
 * under this session" — kernel-log evidence (`journalctl -k`) across a
 * reproducing connect showed no USB enumeration event at all in that
 * reproduction; the session survived the device's self-reset unchanged
 * from the host's point of view. One reproduction instead found the HOST's
 * own per-device USB/cdc_acm state broken afterward — see
 * `HOST_WEDGE_GUIDANCE` for what that is and is not established to mean.
 */
const REBOOT_BANNER = "ESP-ROM:esp32s3";

/**
 * Recovery guidance appended to a timeout/write-stall message once a device
 * reset has been observed this command (`#rebootCount > 0`).
 *
 * One reproduction, with hardware/kernel-log evidence (2026-09-22), found
 * the broken state living in the HOST's per-device USB/cdc_acm state and
 * surviving a full device-side chip reset — so the device rebooting is not
 * itself a fix and does not mean the wedge is about to clear on its own. A
 * browser-side `connect()` — closing and reopening the SAME Web Serial
 * port — does not rebuild the host kernel's endpoint state, exactly as
 * reopening the port in the host CLI does not (see
 * `host/src/transport.rs`'s `SerialTransport` doc comment); Web Serial
 * exposes no primitive to force host-side re-enumeration (no equivalent of
 * unplug/replug or `/sys/bus/usb/devices/<dev>/authorized`).
 *
 * This is kept as a real, recoverable-in-principle *consequence* of a
 * device reset, not as the explanation for the connect wedge overall — see
 * `docs/provisioning-connect-verification-kit.md`'s "host-side USB/cdc_acm
 * wedge" section. Clearing it is also not a guarantee the rest of a
 * provisioning session will succeed: a device-confirmed `pthread` stack
 * overflow in the device's `admin_server` (a separate, fixed defect — see
 * the kit's "Real defects found and fixed" section) can still crash the
 * device later in the SAME session, during `queryAdvert`, well after any
 * host-side wedge is cleared and `queryStatus` has already succeeded.
 *
 * The message text below does not tell the user to unplug and replug the
 * cable as a remedy: that specific action has since been tried, more than
 * once, against a live instance of this wedge and did not reliably clear
 * it — this states only what is actually known about the failure, not an
 * action known not to work.
 */
const HOST_WEDGE_GUIDANCE =
  "A device-side reset does not clear this -- one reproduction traced it to the HOST's " +
  "USB/cdc_acm state, and Web Serial exposes no way for this page to force host-side " +
  "re-enumeration. No recovery action is currently known to reliably clear it; the " +
  "underlying cause of the connect-time reset itself is not established.";

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
 * Upper bound on every Web Serial call this module makes that the browser
 * gives no timeout of its own for: `writer.write()` (`#sendFrame`), plus
 * teardown's `reader.cancel()` and `port.close()` (`disconnect()`). Every
 * one of these is normally a single USB control transfer or buffer hand-off
 * — sub-100ms on a healthy link — so 2s is generous headroom, while still
 * bounded well under `RETRY_TOTAL_MS` (10s) so a stalled call surfaces its
 * own distinct cause quickly instead of hanging forever with no diagnosis
 * at all (part of the defect an earlier mission fixed: `writer.write()` was
 * never guarded by `RETRY_TOTAL_MS`/`RETRY_ATTEMPT_MS`/`FRAME_TIMEOUT_MS` —
 * those three only ever wrapped the RECEIVE side). `connect()` makes no
 * `setSignals()` calls at all (see that method's doc comment) — there is no
 * such call left for this helper to route or exclude.
 */
const UNBOUNDED_CALL_TIMEOUT_MS = 2_000;

/** Thrown by `withTimeout` when `promise` does not settle within `ms`. */
class TimeoutError extends Error {
  constructor(label, ms) {
    super(`${label} timed out after ${ms}ms`);
    this.name = "TimeoutError";
  }
}

/**
 * Race `promise` against a `ms` timer, rejecting with a `TimeoutError`
 * naming `label` if the timer wins first. `promise` itself is left to
 * settle in the background if it never does — Web Serial gives this module
 * no way to actually cancel a `write()`/`cancel()`/`close()` call out from
 * under the browser, so this only bounds how long THIS module waits on it,
 * not the underlying call. Every call site below that awaits an otherwise-
 * unbounded Web Serial promise goes through this.
 */
function withTimeout(promise, ms, label) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new TimeoutError(label, ms)), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (err) => {
        clearTimeout(timer);
        reject(err);
      }
    );
  });
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
  /**
   * Total bytes received from the device since the start of the CURRENT
   * top-level command (`#sendRecvWithRetry`'s entry) — unlike
   * `#bytesArrivedThisAttempt`, this is never reset between retry attempts
   * within that command, only once per command. Exists so a "timeout
   * waiting for response frame" error can report a genuine whole-command
   * total alongside the per-attempt count: `#bytesArrivedThisAttempt` alone
   * made a timeout's "N bytes this attempt" read as "the device has been
   * silent all along" when N=0, even though it may have answered fully on
   * an earlier, already-timed-out attempt whose bytes this field still
   * remembers — an "accumulated 0 bytes" report against a real device was
   * misread exactly this way before this field existed.
   */
  #cumulativeBytesThisCommand = 0;
  /**
   * Raw bytes ARRIVED from the device during the CURRENT retry attempt only
   * — reset once per attempt by `#sendRecvWithRetry`'s entry and its
   * per-retry `catch` block (see that method's RETRY-BOUNDARY BYTE
   * RETENTION doc comment: unlike this field, `#accBuf` itself is NOT
   * cleared on a retry boundary, so the two no longer track the same
   * lifecycle — this field answers "how much arrived in JUST this
   * attempt's window", `#accBuf.length` answers "how much is retained right
   * now, across however many attempts"), incremented by `#readLoop`
   * alongside `#cumulativeBytesThisCommand`, and — unlike `#accBuf.length`
   * — never reduced by `#tryExtractFrame`'s magic-resync discard.
   *
   * WHY THIS EXISTS (round 6, `meshcadet-connect-wedge-round6-transmit-
   * side`): `#timeoutMessage` used to report `#accBuf.length` as "bytes this
   * attempt", but `#accBuf` is what `#tryExtractFrame` DISCARDS from on
   * every resync — `findMagicStart` returns `buf.length` (i.e. "discard
   * everything") whenever no `PROV_MAGIC` candidate is found, and
   * `#tryExtractFrame` immediately slices that whole span out of `#accBuf`.
   * So `#accBuf.length` at timeout time reports what's RETAINED (a trailing
   * partial-frame remnant from this attempt, PLUS — since a retry no longer
   * clears it, see `#sendRecvWithRetry`'s RETRY-BOUNDARY BYTE RETENTION doc
   * comment — anything still-unconsumed from an earlier attempt of this same
   * command; often 0 either way), not what ARRIVED — "0 bytes this attempt"
   * was indistinguishable between "the device said nothing at all" and "the
   * device said plenty, all of it non-frame noise that got discarded."
   * `#bytesArrivedThisAttempt` is the ARRIVED count `#timeoutMessage` needed
   * instead; `#accBuf.length` is still reported alongside it (labeled
   * "retained") since the two together are what actually distinguish the two
   * scenarios above.
   */
  #bytesArrivedThisAttempt = 0;
  /**
   * Preview buffer: the first up to `DISCARD_PREVIEW_CAP` bytes discarded as
   * non-frame noise (log traffic, false `PROV_MAGIC`) during the CURRENT
   * top-level command — reset alongside `#cumulativeBytesThisCommand`, never
   * cleared per-attempt (so a retry doesn't lose the earliest, most useful
   * sample). `#logDiscardedPreview` hex-dumps it to the console at the
   * moment a timeout is about to be thrown.
   *
   * Six rounds of this connect-wedge investigation have theorized about the
   * ~10.5KB of "N bytes received, zero valid frames" traffic
   * (`#cumulativeBytesThisCommand`'s doc comment) without anyone having
   * actually looked at its content — this exists so the next timeout report
   * carries real bytes, not just a count.
   */
  #discardPreview = new Uint8Array(0);
  /**
   * Number of times `REBOOT_BANNER` has been seen in the discarded
   * (non-frame) traffic during the CURRENT top-level command — reset
   * alongside `#cumulativeBytesThisCommand`/`#discardPreview`, never
   * cleared per-attempt (a device that resets on every retry must still be
   * reported as resetting more than once). `#timeoutMessage` surfaces this
   * as "device rebooted N times during this command" plus `HOST_WEDGE_GUIDANCE`
   * once it is greater than zero, so a connect-triggered reset reads as what
   * it is instead of a generic frame timeout — see `HOST_WEDGE_GUIDANCE`'s
   * doc comment for why this is no longer "reconnect to continue" (round 7's
   * retracted claim).
   */
  #rebootCount = 0;
  /**
   * Trailing `REBOOT_BANNER.length - 1` characters carried over between
   * `#scanForRebootBanner` calls so an occurrence of the banner split across
   * two separate discards (e.g. two separate USB reads, or
   * `#tryExtractFrame`'s one-byte-at-a-time false-magic resync) is not
   * missed at the boundary. Reset alongside `#rebootCount`.
   */
  #rebootScanCarry = "";
  #waiters = [];
  /**
   * Set by `#readLoop`'s catch when the underlying stream itself errors
   * (e.g. the device physically vanishing mid-read — plausible after a
   * DTR/RTS-triggered EN reset severe enough to re-enumerate the USB
   * device rather than just reboot the firmware under an unchanged USB
   * session; see `connect()`'s doc comment). Once set, no more bytes will
   * ever arrive on this session — `#recvFrame`/`#sendRecvWithRetry` check
   * it so a dead link fails fast with the real cause instead of retrying
   * blind for the full 10s budget and then reporting a generic timeout
   * that buries what actually happened.
   */
  #fatalError = null;
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
  /**
   * Per-attempt / overall / per-frame timeouts actually used by this
   * instance — default to the module constants (`RETRY_ATTEMPT_MS`/
   * `RETRY_TOTAL_MS`/`FRAME_TIMEOUT_MS`) but overridable via the
   * constructor. Mirrors `Session::with_retry_params` (`host/src/
   * session.rs`), which exists for exactly the same reason: a test that
   * needs to actually reach a real timeout (e.g. proving the reboot-count
   * message below) without a genuine multi-second wait.
   */
  #retryAttemptMs;
  #retryTotalMs;
  #frameTimeoutMs;

  /**
   * @param {{retryAttemptMs?: number, retryTotalMs?: number, frameTimeoutMs?: number}} [opts]
   *   Overrides for this session's retry/timeout budget. Tests only —
   *   production code (`provisioner.js`) always constructs
   *   `new ProvisionerSession()` with no arguments and gets the real
   *   `RETRY_ATTEMPT_MS`/`RETRY_TOTAL_MS`/`FRAME_TIMEOUT_MS` defaults.
   */
  constructor({
    retryAttemptMs = RETRY_ATTEMPT_MS,
    retryTotalMs = RETRY_TOTAL_MS,
    frameTimeoutMs = FRAME_TIMEOUT_MS,
  } = {}) {
    this.#retryAttemptMs = retryAttemptMs;
    this.#retryTotalMs = retryTotalMs;
    this.#frameTimeoutMs = frameTimeoutMs;
  }

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
   *
   * ── `open()` and nothing else — reverted, round 13 (this mission) ──
   *
   * `connect()` is exactly `await port.open({ baudRate: BAUD_RATE })`, with
   * NO signal manipulation of any kind afterward. This matches every Web
   * Serial client independently confirmed to work against this hardware
   * class: `meshcore-dev/config.meshcore.io`'s `lib/serial-cli.js`
   * (`requestPort(); open({baudRate}); getReader(); getWriter();
   * startReading()`, no `setSignals` anywhere), `meshtastic/js`'s
   * `packages/transport-web-serial/src/transport.ts` (`port.open({ baudRate:
   * baudRate || 115200 })` then pipes streams, no `setSignals`, no
   * preamble), and this repo's own host CLI (`host/src/transport.rs`, which
   * deliberately never writes DTR/RTS at all). The only client examined that
   * touches `setSignals` is `esptool-js`, which *wants* the reset to enter
   * its own bootloader for flashing — not a model for this method.
   *
   * ── History: three rounds on this exact line ──
   *
   * PR #200 called `setSignals({ dataTerminalReady: false, requestToSend:
   * false })` as a single combined call immediately after `open()`,
   * reasoning that de-asserting fast might "beat" `open()`'s own forced
   * DTR/RTS assert — explicitly flagged in that PR's own commit message as
   * unverified on real hardware. Tested, it made things worse: the device
   * was left unreachable by the host CLI until a physical reset. PR #201
   * removed signal handling from `connect()` entirely. A later round
   * (86746ca, "drop RTS before DTR to stop ESP32-S3 DTR=0/RTS=1 core-reset
   * on connect") re-added it, split into two separate awaited calls — RTS
   * first, then DTR — on the theory that a single combined call let
   * Chromium choose the internal line order and could land on the
   * ESP32-S3's confirmed DTR=0/RTS=1 core-reset trigger, while clearing RTS
   * first would never visit that state. **That fix was tested on hardware
   * and did not clear the connect wedge.** It is reverted here, all the way
   * back to `open()` and nothing else, because a systematic comparison
   * against every Web Serial client independently known to work against
   * this hardware class found that none of them touch these lines at all —
   * the working pattern is `open()` alone, not any particular ordering of
   * post-open signal calls.
   *
   * This is not a re-endorsement of PR #200's or #201's own reasoning,
   * just a return to the same code #201 already landed. Also worth noting:
   * de-asserting DTR post-open is independently suspect on this chip
   * family regardless of ordering — `meshcore_py` issue #105 documents that
   * native-USB CDC stacks (which is what the ESP32-S3's USB-Serial-JTAG
   * peripheral is) can treat DTR low as "no host connected" and stop
   * replying, the opposite of what a bridge-chip board needs a DTR toggle
   * for.
   *
   * `port.open()` itself still asserts DTR and RTS unconditionally before
   * any application code runs — Chromium gives no "open without touching
   * the lines" call, so that one transition is not something this method
   * can avoid regardless of what it does afterward. Field evidence already
   * shows the resulting reset (`rst:0x15 USB_UART_CHIP_RESET`) is
   * known-survivable (tolerated by the retry/resync machinery below) even
   * though its CAUSE is not established — no theory about it is asserted
   * here; see `HOST_WEDGE_GUIDANCE`'s doc comment for what is and is not
   * known about what follows it.
   *
   * `#readLoop`'s magic-header resync (`#tryExtractFrame`, gotcha #9)
   * already tolerates an ESP-IDF boot banner landing ahead of any real
   * frame, and `#sendRecvWithRetry` already retries the first command
   * (`queryStatus`, called by `provisioner.js` immediately after
   * `connect()` resolves) for up to `RETRY_TOTAL_MS` — long enough to ride
   * out a reboot if one happens. No separate "wait for boot" step is added
   * here: `#sendRecvWithRetry` clears `#accBuf` before its first send, so
   * stale bytes accumulated during `port.open()` are already discarded
   * before the first attempt — that IS the "settle, drain the banner,
   * retry" sequence, already in place for any caller, not something
   * `connect()` needs to duplicate.
   *
   * What this does NOT handle: if a reset is severe enough to make the
   * ESP32-S3's native USB peripheral fully re-enumerate (as opposed to a
   * soft reboot that keeps the same USB session alive), the already-open
   * `port.readable`/`port.writable` streams may error out from under
   * `#readLoop` rather than just going quiet for a while. That surfaces as
   * a real rejection (`#readLoop`'s catch rejects every waiter — never a
   * silent hang), but recovering from it means the user reconnecting, not
   * something this method can paper over. If a device reset ever DOES leave
   * the host CLI wedged afterward, that is a real, separate, HOST-side
   * USB/cdc_acm consequence — see `HOST_WEDGE_GUIDANCE`'s doc comment — not
   * something this method can prevent. And a session that survives connect
   * entirely can still hit the unrelated, separate `admin_server`
   * stack-overflow defect on `queryAdvert` — see `HOST_WEDGE_GUIDANCE`'s doc
   * comment.
   *
   * ── Case closed, 2026-09-24: this connect path is not the defect ──
   *
   * The identical deployed page, against the identical device and firmware,
   * connects and reads status correctly from a different host (a Chromebook
   * running native Chrome). The connect failures this campaign chased are
   * specific to one host (Pop!_OS running Flatpak Chromium, where the
   * native host CLI works fine against the same device but the
   * Flatpak-sandboxed browser does not) — root cause on that host is not
   * identified, and this method is not changed further to chase it. See
   * `docs/provisioning-connect-verification-kit.md` for the full account,
   * what this campaign fixed, what it eliminated, and the one open next
   * step (native Chromium `.deb` vs. Flatpak Chromium on the same host).
   */
  async connect() {
    const port = await navigator.serial.requestPort();
    await port.open({ baudRate: BAUD_RATE });
    this.#port = port;
    this.#writer = port.writable.getWriter();
    this.#reader = port.readable.getReader();
    this.#accBuf = new Uint8Array(0);
    this.#cumulativeBytesThisCommand = 0;
    this.#bytesArrivedThisAttempt = 0;
    this.#discardPreview = new Uint8Array(0);
    this.#rebootCount = 0;
    this.#rebootScanCarry = "";
    this.#fatalError = null;
    this.#readLoopPromise = this.#readLoop();
  }

  /**
   * Close the port and release all resources. Safe to call when not
   * connected.
   *
   * Every `await` below is bounded by `UNBOUNDED_CALL_TIMEOUT_MS`
   * (`withTimeout`) — `reader.cancel()`/`port.close()` are Web Serial calls
   * the browser gives no timeout of its own for, same class of gap as
   * `#sendFrame`'s `writer.write()` (this mission's audit — Task 4). Without
   * this, a wedged stream at teardown time would hang `disconnect()` itself,
   * stranding the user with no recovery but a page reload — the exact
   * failure mode a "disconnect and reconnect" recovery path exists to
   * prevent. A timeout here still lets teardown proceed:
   * `releaseLock()` on a reader with a still-pending read forces that read
   * to reject (unblocking a stuck `#readLoop`), so cleanup keeps moving even
   * when the underlying call never actually settles.
   */
  async disconnect() {
    if (!this.#port) {
      return;
    }
    try {
      await withTimeout(this.#reader.cancel(), UNBOUNDED_CALL_TIMEOUT_MS, "reader.cancel()");
    } catch (err) {
      // Port may already be gone (device unplugged), or cancel() itself
      // stalled — either way, disconnect() must still finish tearing down
      // rather than hanging indefinitely.
      console.warn(`MeshCadet provisioner: ${err.message} — continuing teardown anyway`, err);
    }
    try {
      await withTimeout(this.#readLoopPromise, UNBOUNDED_CALL_TIMEOUT_MS, "read loop shutdown");
    } catch {
      // #readLoop's own read() rejects when cancel()/disconnect races it (its
      // catch already swallows and returns) — this is defensive only. A
      // timeout here just means the loop's pending read() never unblocked;
      // continue tearing down regardless, per reader.cancel() above.
    }
    try {
      this.#reader.releaseLock();
    } catch {
      // Throws if a read is still genuinely pending (cancel()/the read loop
      // never actually settled) — nothing more this method can do about an
      // already-wedged stream; continue tearing down the rest of the state.
    }
    try {
      this.#writer.releaseLock();
    } catch {
      // Same as reader.releaseLock() above, write side.
    }
    try {
      await withTimeout(this.#port.close(), UNBOUNDED_CALL_TIMEOUT_MS, "port.close()");
    } catch (err) {
      // Already closed (e.g. device physically unplugged), or close()
      // itself stalled — either way, fall through and clear our own state
      // so the user can still attempt a fresh connect().
      console.warn(`MeshCadet provisioner: ${err.message} — continuing teardown anyway`, err);
    }
    this.#port = null;
    this.#reader = null;
    this.#writer = null;
    this.#accBuf = new Uint8Array(0);
    this.#cumulativeBytesThisCommand = 0;
    this.#bytesArrivedThisAttempt = 0;
    this.#discardPreview = new Uint8Array(0);
    this.#rebootCount = 0;
    this.#rebootScanCarry = "";
    this.#fatalError = null;
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
      const second = await this.#recvFrame(this.#frameTimeoutMs);
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

  /**
   * Enumerate the device's configured room-server contacts. Sends
   * `FRAME_QUERY_ROOMS`, then consumes the streamed `FRAME_RSP_ROOM` frames
   * terminated by `FRAME_RSP_ROOMS_DONE`. Mirrors `Session::list_rooms`
   * (`host/src/session.rs`). Returns entries in device-index order.
   *
   * The guest password is never part of a room's `RspRoom` payload (see
   * `codec.js`'s `decodeRspRoom` doc comment) — there is nothing to scrub
   * here, unlike `addRoom` below.
   */
  async listRooms() {
    return this.#exclusive(() =>
      this.#streamUntilDone(FRAME_QUERY_ROOMS, FRAME_RSP_ROOM, FRAME_RSP_ROOMS_DONE, decodeRspRoom, "room")
    );
  }

  /**
   * Add (or replace) a room-server contact: a contact entry plus its guest
   * password. `pubkey` is a 32-byte `Uint8Array` (Ed25519 public key);
   * `guestPassword` is a UTF-8 string, silently truncated to
   * `MAX_ROOM_PASSWORD_LEN` (16) bytes by `encodeAddRoom` — validate/warn
   * upstream with `provisioner/validation.js`'s `validateRoomPassword` if you
   * need to flag that rather than silently truncate. `name` is a UTF-8
   * display name (empty = device falls back to a routing-hash-derived
   * label, same as `addContact`). Mirrors `Session::add_room`
   * (`host/src/session.rs`).
   *
   * ADR-0002 §4/ADR-0001 §4 security model: the guest password crosses the
   * USB serial link in the clear BY DESIGN (the cable is the authentication)
   * — that is correct here and is not this method's concern. What IS this
   * method's concern, mirroring `setPin`'s discipline exactly: the password
   * is held only in this transient `payload` buffer for the duration of the
   * send, then scrubbed (`.fill(0)`) once the retry loop can no longer
   * reference it — never stored on the instance, logged, placed in the URL,
   * or written to `localStorage`/`sessionStorage`. (The caller owns the
   * `guestPassword` string itself; JS strings are immutable and cannot be
   * scrubbed, so the caller — see `provisioner.js`'s `handleAddRoom` — must
   * also drop its reference and clear the password input field once this
   * resolves or rejects.)
   *
   * Unlike `setPin`/`clearHistory` (which bypass `#exclusive` — see that
   * method's doc comment), this routes through `#exclusive` like every other
   * non-sensitive write command (`addContact`, `addChannel`, ...): a room is
   * just another provisioned entity, not a per-session secret update, so it
   * gets the same serialization guarantee against a concurrent command
   * racing its write.
   */
  async addRoom(pubkey, guestPassword, name) {
    const payload = encodeAddRoom(pubkey, guestPassword, name);
    try {
      await this.#exclusive(() => this.#sendAndExpectOk(FRAME_ADD_ROOM, payload));
    } finally {
      // Scrub the guest-password bytes from our buffer now that no retry can
      // re-send it.
      payload.fill(0);
    }
  }

  /**
   * Delete a room-server contact (its contact entry AND room extras) by its
   * 32-byte Ed25519 public key. Mirrors `Session::del_room`
   * (`host/src/session.rs`).
   */
  async delRoom(pubkey) {
    await this.#exclusive(() => this.#sendAndExpectOk(FRAME_DEL_ROOM, encodeDelRoom(pubkey)));
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
   * Set the screen-lock PIN — distinct from the admin PIN (`setPin` above):
   * unlocking the screen must never open the admin menu, and vice versa
   * (docs/adr/0013-screen-lock-policy-layer.md, D6). `pin` must already be
   * exactly `LOCK_PIN_LEN` (4) ASCII digits — validate with
   * `provisioner/validation.js`'s `validateLockPin` before calling;
   * `encodeSetLockPin` throws on anything else rather than silently
   * truncating (unlike `encodeSetPin`).
   *
   * Routed through `#exclusive` like every other command added since M2's
   * config child (unlike `setPin`/`clearHistory` above, which predate that
   * discipline — see their own doc comments).
   *
   * ADR-0007 security model, same discipline as `setPin`: this method holds
   * the PIN only in the transient `payload` buffer for the duration of the
   * send, then scrubs it (`.fill(0)`) once the retry loop can no longer
   * reference it. It is never stored on the instance, logged, placed in the
   * URL, or written to `localStorage`/`sessionStorage`. The caller owns the
   * `pin` string itself and should drop its reference and clear any input
   * field after this resolves — see `provisioner.js`'s `handleSetLockPin`.
   */
  async setLockPin(pin) {
    const payload = encodeSetLockPin(pin);
    try {
      await this.#exclusive(() => this.#sendAndExpectOk(FRAME_SET_LOCK_PIN, payload));
    } finally {
      // Scrub the PIN bytes from our buffer now that no retry can re-send it.
      payload.fill(0);
    }
  }

  /**
   * Set the screen-lock enable flag(s) and idle timeout. `lockFlags` is the
   * raw `lock_flags` byte (see `codec.js`'s `LOCK_SCREEN_ENABLE`);
   * `lockTimeoutS` must already be within `LOCK_TIMEOUT_MIN_S..=LOCK_TIMEOUT_MAX_S`
   * — validate with `provisioner/validation.js`'s `validateLockTimeout`
   * before calling. `RSP_OK` here means "accepted and forwarded to the UI
   * thread", not "persisted" — persistence completes asynchronously on the
   * device's UI thread (docs/adr/0013-screen-lock-policy-layer.md, D2).
   */
  async setLockConfig(lockFlags, lockTimeoutS) {
    await this.#exclusive(() =>
      this.#sendAndExpectOk(FRAME_SET_LOCK_CONFIG, encodeSetLockConfig(lockFlags, lockTimeoutS))
    );
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
      ({ frameType, payload } = await this.#recvFrame(this.#frameTimeoutMs));
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

  /**
   * Write one frame to the port. Unlike the RECEIVE side (bounded by
   * `RETRY_ATTEMPT_MS`/`RETRY_TOTAL_MS`/`FRAME_TIMEOUT_MS` throughout this
   * class), `writer.write()` itself was, until an earlier mission, a bare
   * `await` with no bound at all — a wedged writable stream (a dead link
   * for any reason, e.g. a reset in flight — see `connect()`'s doc comment)
   * hung here forever, with `#sendRecvWithRetry`'s loop never even reached
   * its own `#recvUntilExpected` call, let alone its retry/deadline logic.
   *
   * A `writer.write()` that doesn't settle within `UNBOUNDED_CALL_TIMEOUT_MS`,
   * or that rejects outright (the stream has errored — e.g. the device
   * vanishing mid-write), is treated as fatal immediately: same
   * `#fatalError` latch `#readLoop`'s catch already uses for a dead read
   * side (see that field's doc comment), just discovered from the write
   * side instead. Every current and future waiter is rejected with a
   * "write stalled" cause distinct from a plain receive timeout ("no
   * response" — see `#recvUntilExpected`/`#recvFrame`), so whichever of the
   * two actually happened is what the caller — and the user — sees.
   *
   * `#sendRecvWithRetry` calls this INSIDE its own `try`/`catch` (round 8,
   * `meshcadet-connect-wedge-round8-host-usb-endpoint-state` — an earlier
   * round called it OUTSIDE the `try`, on the reasoning that a caught write
   * stall would just be retried into the same doomed write; retrying was
   * never actually implemented for this case, since `#fatalError` always
   * short-circuits the retry branch either way, so the only effect of being
   * outside `try` was that a write-stall error skipped the same
   * reboot-count-aware enrichment (`HOST_WEDGE_GUIDANCE`) a receive timeout
   * already gets from `#timeoutMessage()` — moving it inside closes that
   * gap without changing the "no retry on a fatal error" behavior at all).
   */
  async #sendFrame(frameType, payload) {
    try {
      await withTimeout(this.#writer.write(encodeFrame(frameType, payload)), UNBOUNDED_CALL_TIMEOUT_MS, "writer.write()");
    } catch (err) {
      const cause =
        err instanceof TimeoutError
          ? new Error(`write stalled — ${err.message} (writable stream is not draining; the link is likely dead)`)
          : err;
      this.#fatalError = cause;
      this.#rejectAllWaiters(cause);
      throw cause;
    }
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
      ({ frameType, payload } = await this.#recvFrame(this.#frameTimeoutMs));
    }
    return entries;
  }

  /**
   * Send a command frame and wait for the response, retrying the send every
   * `RETRY_ATTEMPT_MS` until a valid response frame arrives or
   * `RETRY_TOTAL_MS` has elapsed. Mirrors `Session::send_recv_with_retry`.
   *
   * Unlike the Rust version, there is no `flush_input()` step between
   * retries: Web Serial exposes no OS-buffer-clear primitive, and — unlike
   * `Session::flush_input`'s deliberate discard of whatever is sitting in
   * the OS buffer — this method does NOT clear `#accBuf` between retry
   * attempts either (see RETRY-BOUNDARY BYTE RETENTION below). `#readLoop`
   * continuously drains the port into `#accBuf` regardless of which attempt
   * is "current", so there is no separate kernel buffer accumulating behind
   * the scenes that would need a JS-side equivalent of a flush in the first
   * place.
   *
   * RETRY-BOUNDARY BYTE RETENTION: an earlier version of this method cleared
   * `#accBuf` at the top of the `catch` block below, on the theory that
   * stale bytes left over from a timed-out attempt would otherwise confuse
   * the next attempt's `#recvFrame` call. No other client this protocol was
   * compared against (the host CLI included) discards received bytes
   * mid-command, and the theory doesn't hold up: `#tryExtractFrame`'s own
   * `find_magic_start`/`plen` resync already exists to recover from stale or
   * malformed bytes at the front of `#accBuf`, so clearing it here was pure
   * loss with no corresponding gain. The real cost showed up whenever the
   * device's one and only reply to an attempt straddled the retry boundary
   * — some of its bytes arriving just before the per-attempt deadline, the
   * rest just after: the pre-deadline partial frame was thrown away by the
   * clear, and the post-deadline remainder then had no header to resync
   * against and was shredded as noise, silently losing a reply the device
   * never sends twice. `#accBuf` is now left untouched across a retry
   * boundary — only the whole-command entry reset above and the
   * cross-command residue guard below ever clear it — so a reply that
   * completes a moment after `#sendRecvWithRetry` has already moved on to
   * the next attempt is still sitting there, still resyncable, the next
   * time `#recvFrame` looks.
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
   *
   * FATAL-LINK GUARD: a per-attempt timeout means "no answer yet, worth
   * retrying" — the device may still be rebooting (see `connect()`'s doc
   * comment). A `#fatalError` (the read stream itself has died — the
   * device is not coming back without a fresh `connect()`) means retrying
   * is pointless: nothing will ever arrive again. Checked both before
   * sending (skip a write to a dead link) and after a failed attempt
   * (stop immediately instead of burning the rest of the 10s budget one
   * doomed attempt at a time) so the error the caller sees names the real
   * cause instead of a generic "timeout waiting for response frame".
   *
   * `#sendFrame` is called INSIDE the `try` below (round 8,
   * `meshcadet-connect-wedge-round8-host-usb-endpoint-state` — see that
   * method's own doc comment for why an earlier round had it outside): a
   * fatal write-stall error still propagates on the FIRST catch (no retry —
   * `this.#fatalError` is truthy the moment `#sendFrame` sets it, so the
   * `throw err` below fires immediately, same as before), but now goes
   * through the same reboot-count enrichment (`#withRebootContext`) a
   * receive timeout already gets, instead of surfacing a bare "write
   * stalled" with no context on a device that had already been observed
   * resetting mid-command.
   */
  async #sendRecvWithRetry(frameType, payload, isExpected = () => true, label = "response") {
    this.#accBuf = new Uint8Array(0);
    this.#cumulativeBytesThisCommand = 0;
    this.#bytesArrivedThisAttempt = 0;
    this.#discardPreview = new Uint8Array(0);
    this.#rebootCount = 0;
    this.#rebootScanCarry = "";
    const overallDeadline = Date.now() + this.#retryTotalMs;
    while (true) {
      if (this.#fatalError) {
        throw this.#fatalError;
      }
      try {
        await this.#sendFrame(frameType, payload);
        return await this.#recvUntilExpected(this.#retryAttemptMs, isExpected, label);
      } catch (err) {
        if (this.#fatalError || Date.now() >= overallDeadline) {
          throw this.#withRebootContext(err);
        }
        // This attempt timed out but we still have overall budget — loop
        // and send again. `#accBuf` is deliberately NOT cleared here (see
        // RETRY-BOUNDARY BYTE RETENTION above): whatever this attempt
        // accumulated, complete or partial, stays in place for the next
        // attempt's `#recvFrame` to keep resyncing against.
        // `#bytesArrivedThisAttempt` IS reset here — unlike `#accBuf`, it is
        // purely a per-attempt "how many bytes arrived during JUST this
        // attempt's window" counter (see its own doc comment), independent
        // of what `#accBuf` currently retains, so the NEXT attempt's
        // "arrived" figure must start back at zero regardless.
        this.#bytesArrivedThisAttempt = 0;
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
        throw new Error(this.#timeoutMessage());
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
   *
   * A frame already fully buffered before a fatal read-loop error is still
   * returned (checked first, below) — only once there is genuinely nothing
   * left to extract does a `#fatalError` end the wait immediately, instead
   * of idling out the full `timeoutMs` only to report a generic timeout
   * that hides the real cause (see `#sendRecvWithRetry`'s FATAL-LINK GUARD).
   */
  async #recvFrame(timeoutMs) {
    const deadline = Date.now() + timeoutMs;
    while (true) {
      const frame = this.#tryExtractFrame();
      if (frame) {
        return frame;
      }
      if (this.#fatalError) {
        throw this.#fatalError;
      }
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        throw new Error(this.#timeoutMessage());
      }
      await this.#waitForDataOrTimeout(remaining);
    }
  }

  /**
   * Build the "timeout waiting for response frame" message shared by
   * `#recvUntilExpected`/`#recvFrame`, and hex-dump `#discardPreview` to the
   * console as a side effect (see that field's doc comment) — every timeout
   * report goes through this one method, so it is the single place that
   * needs to trigger the dump.
   *
   * Reports THREE figures, not one:
   * - `#bytesArrivedThisAttempt`: raw bytes ARRIVED from the device during
   *   just this attempt, regardless of whether `#tryExtractFrame` went on to
   *   discard them as non-frame noise.
   * - `#accBuf.length`: bytes RETAINED right now (i.e. still sitting
   *   un-discarded in `#accBuf`, usually a trailing partial-frame remnant).
   * - `#cumulativeBytesThisCommand`: bytes ARRIVED across the WHOLE command
   *   (every retry attempt, never cleared until the next command starts).
   *
   * Round 6 (`meshcadet-connect-wedge-round6-transmit-side`) found that
   * reporting only `#accBuf.length` as "bytes this attempt" conflated
   * RETAINED with ARRIVED: `findMagicStart` returning `buf.length` (no magic
   * candidate found) makes `#tryExtractFrame` discard the entire attempt's
   * traffic, so "0 bytes this attempt" read as "the device said nothing"
   * when it could equally mean "the device said plenty, all of it discarded
   * as noise" — see `#bytesArrivedThisAttempt`'s own doc comment.
   *
   * Round 7 found that if `REBOOT_BANNER` was seen anywhere in the
   * discarded traffic this command (`#rebootCount > 0`), this is no longer
   * a generic frame timeout — the device actually reset mid-command. Round
   * 8 (`meshcadet-connect-wedge-round8-host-usb-endpoint-state`, hardware
   * evidence) RETRACTS round 7's "reconnect to continue" — see
   * `HOST_WEDGE_GUIDANCE`'s doc comment for why a browser-side reconnect
   * cannot clear this. `#withRebootContext` applies the identical
   * enrichment to a write-stall error, so the two share one message shape.
   */
  #timeoutMessage() {
    this.#logDiscardedPreview();
    const base = `timeout waiting for response frame (${this.#bytesArrivedThisAttempt} bytes arrived this attempt, ${this.#accBuf.length} retained, ${this.#cumulativeBytesThisCommand} bytes arrived total this command)`;
    return this.#withRebootContext(new Error(base)).message;
  }

  /**
   * Append `HOST_WEDGE_GUIDANCE` plus the observed reboot count to `err`'s
   * message, returning a NEW `Error` (never mutating `err` in place — `err`
   * may be `#fatalError` itself, which `#rejectAllWaiters` has already
   * handed to, or will hand to, every OTHER waiter with the SAME object;
   * mutating it here would leak this one call site's enrichment onto every
   * other caller's error too).
   *
   * A no-op (returns `err` unchanged) when no reboot has been observed this
   * command (`#rebootCount === 0` — nothing to report) or when `err`'s
   * message already carries `HOST_WEDGE_GUIDANCE` (it came from
   * `#timeoutMessage()`, which just applied this same enrichment itself —
   * avoids double-appending when `#sendRecvWithRetry`'s catch wraps an
   * error that already went through it).
   */
  #withRebootContext(err) {
    if (this.#rebootCount === 0 || err.message.includes(HOST_WEDGE_GUIDANCE)) {
      return err;
    }
    const times = this.#rebootCount === 1 ? "1 time" : `${this.#rebootCount} times`;
    return new Error(`${err.message} — device rebooted ${times} during this command. ${HOST_WEDGE_GUIDANCE}`);
  }

  /**
   * Hex/ASCII-dump `#discardPreview` to the console — the first (up to
   * `DISCARD_PREVIEW_CAP`) bytes this command discarded as non-frame noise.
   * Diagnostic only: a no-op if nothing has been discarded yet. Called from
   * `#timeoutMessage` so a "timeout waiting for response frame" report is
   * always accompanied by an actual look at what the device was sending,
   * instead of just a count.
   *
   * `console.warn`, not `console.debug`: six rounds of this connect-wedge
   * campaign carried this exact diagnostic and nobody read it, because
   * Chrome's console filter hides the "Verbose"/"debug" level by default —
   * a human debugging a live wedge would have to know to go flip that
   * filter on before it was ever visible. A diagnostic written to be read
   * during a failure must actually show up at the console's default level.
   */
  #logDiscardedPreview() {
    if (this.#discardPreview.length === 0) {
      return;
    }
    const hex = Array.from(this.#discardPreview, (b) => b.toString(16).padStart(2, "0")).join(" ");
    const ascii = Array.from(this.#discardPreview, (b) =>
      b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : "."
    ).join("");
    console.warn(
      `MeshCadet provisioner: first ${this.#discardPreview.length} discarded (non-frame) bytes this command —\nhex: ${hex}\nascii: ${ascii}`
    );
  }

  /**
   * Append `bytes` to `#discardPreview`, capped at `DISCARD_PREVIEW_CAP`
   * total — called by `#tryExtractFrame` at every point it discards bytes as
   * non-frame noise. The `DISCARD_PREVIEW_CAP` truncation only bounds the
   * human-readable dump; `#scanForRebootBanner` below always sees the full
   * `bytes` span regardless of how much of `#discardPreview` room is left,
   * so a reboot banner arriving after the preview cap is still counted.
   */
  #recordDiscarded(bytes) {
    if (bytes.length === 0) {
      return;
    }
    this.#scanForRebootBanner(bytes);
    if (this.#discardPreview.length >= DISCARD_PREVIEW_CAP) {
      return;
    }
    const room = DISCARD_PREVIEW_CAP - this.#discardPreview.length;
    const take = bytes.length > room ? bytes.subarray(0, room) : bytes;
    const merged = new Uint8Array(this.#discardPreview.length + take.length);
    merged.set(this.#discardPreview, 0);
    merged.set(take, this.#discardPreview.length);
    this.#discardPreview = merged;
  }

  /**
   * Scan newly-discarded (non-frame) `bytes` for `REBOOT_BANNER`,
   * incrementing `#rebootCount` once per occurrence found. Carries the
   * trailing `REBOOT_BANNER.length - 1` characters across calls
   * (`#rebootScanCarry`) so an occurrence split across two `#recordDiscarded`
   * calls is not missed at the boundary.
   *
   * Latin-1 decode (`String.fromCharCode` per byte, not `TextDecoder`'s
   * UTF-8): every byte maps to exactly one code point, so a match against
   * the ASCII banner text is exact and lossless regardless of any non-ASCII
   * bytes elsewhere in the noise (binary log content, partial frame
   * remnants) that would otherwise make a UTF-8 decode throw or substitute
   * a replacement character and shift subsequent byte offsets.
   */
  #scanForRebootBanner(bytes) {
    const text = this.#rebootScanCarry + Array.from(bytes, (b) => String.fromCharCode(b)).join("");
    let from = 0;
    let idx;
    while ((idx = text.indexOf(REBOOT_BANNER, from)) !== -1) {
      this.#rebootCount += 1;
      from = idx + REBOOT_BANNER.length;
    }
    this.#rebootScanCarry = text.slice(Math.max(0, text.length - (REBOOT_BANNER.length - 1)));
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
        this.#recordDiscarded(this.#accBuf.subarray(0, sync));
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
        this.#recordDiscarded(this.#accBuf.subarray(0, 1));
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
          this.#recordDiscarded(this.#accBuf.subarray(0, 1));
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
          this.#cumulativeBytesThisCommand += value.length;
          this.#bytesArrivedThisAttempt += value.length;
          this.#notifyWaiters();
        }
      }
    } catch (err) {
      // Port error (e.g. device physically unplugged mid-read, or a
      // DTR/RTS-triggered EN reset severe enough to re-enumerate the USB
      // device rather than just reboot the firmware under an unchanged USB
      // session — see `connect()`'s doc comment). Nothing will ever arrive
      // on this session again: record it as `#fatalError` so every current
      // AND future wait fails fast with this real cause (`#recvFrame`,
      // `#sendRecvWithRetry`), not just the waiters that happened to be
      // pending at this exact instant.
      this.#fatalError = err instanceof Error ? err : new Error(String(err));
      this.#rejectAllWaiters(this.#fatalError);
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
