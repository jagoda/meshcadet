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
// M1 walking skeleton: read-only. Only `queryStatus()` (the two-frame
// QUERY_STATUS -> RSP_STATUS + RSP_IDENTITY handshake) is exposed; the
// command-frame methods (add/del contact, set pin, commit, …) are later
// campaign milestones layered on top of the same `#sendRecvWithRetry`.
//
// No build step: plain ES module, loaded directly by the browser.

import {
  encodeFrame,
  decodeFrame,
  findMagicStart,
  decodeRspStatus,
  decodeRspIdentity,
  decodeRspError,
  ProvError,
  FRAME_QUERY_STATUS,
  FRAME_RSP_STATUS,
  FRAME_RSP_IDENTITY,
  FRAME_RSP_ERROR,
  MAX_RSP_HISTORY_ENTRY_PAYLOAD,
} from "./codec.js";

// Matches the host CLI's `--baud` default (`host/src/main.rs`).
const BAUD_RATE = 115200;

// Mirrors `Session::new`'s defaults (`host/src/session.rs`): 500 ms per retry
// attempt, 10 s overall retry budget, 5 s per-frame timeout once synced.
const RETRY_ATTEMPT_MS = 500;
const RETRY_TOTAL_MS = 10_000;
const FRAME_TIMEOUT_MS = 5_000;

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
    const first = await this.#sendRecvWithRetry(FRAME_QUERY_STATUS, new Uint8Array(0));
    let status;
    if (first.frameType === FRAME_RSP_STATUS) {
      status = decodeRspStatus(first.payload);
    } else if (first.frameType === FRAME_RSP_ERROR) {
      throw deviceErrorFrom(first.payload);
    } else {
      throw new Error(`unexpected response 0x${hex2(first.frameType)} to QUERY_STATUS`);
    }

    // Consume the trailing RSP_IDENTITY frame the firmware always sends after
    // RSP_STATUS — leaving it unread would desync the next command, exactly
    // as documented on `Session::query_status`.
    const second = await this.#recvFrame(FRAME_TIMEOUT_MS);
    if (second.frameType !== FRAME_RSP_IDENTITY) {
      throw new Error(
        `expected RSP_IDENTITY (0x${hex2(FRAME_RSP_IDENTITY)}) after RSP_STATUS; got 0x${hex2(second.frameType)}`
      );
    }
    const identity = decodeRspIdentity(second.payload);
    return { status, identity };
  }

  // ── Low-level frame I/O ───────────────────────────────────────────────────

  async #sendFrame(frameType, payload) {
    await this.#writer.write(encodeFrame(frameType, payload));
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
   */
  async #sendRecvWithRetry(frameType, payload) {
    const overallDeadline = Date.now() + RETRY_TOTAL_MS;
    while (true) {
      await this.#sendFrame(frameType, payload);
      try {
        return await this.#recvFrame(RETRY_ATTEMPT_MS);
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
      // fits within MAX_RSP_HISTORY_ENTRY_PAYLOAD, so a larger plen means the
      // "MC" bytes were ASCII log noise, not a real frame header. Advance 1
      // byte and re-scan.
      if (plen > MAX_RSP_HISTORY_ENTRY_PAYLOAD) {
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
