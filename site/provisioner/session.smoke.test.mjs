// session.smoke.test.mjs — mocked-Web-Serial regression coverage for
// session.js's async orchestration (send_recv_with_retry / recv_frame /
// find_magic_start resync / the two-frame QUERY_STATUS handshake).
//
// NOT a golden-vector conformance test (that's codec.conformance.test.mjs,
// which guards codec.js's wire-format byte-shuffling against drift from the
// Rust codec). This file has no Rust counterpart to conform to — session.js
// is a fresh async reimplementation with no Rust twin (host/src/session.rs
// is synchronous/blocking; see docs/adr/0007-provisioner-codec.md, Finding
// 2) — so this exercises session.js's own orchestration logic in isolation,
// against a fake `navigator.serial` port built from Node's built-in
// WHATWG Streams globals (`ReadableStream`/`WritableStream`, no real
// hardware, no real browser).
//
// Plain `node`, zero dependencies (no package.json), matching
// codec.conformance.test.mjs's build-step-free posture. Run directly:
//
//   node site/provisioner/session.smoke.test.mjs

import assert from "node:assert/strict";
import {
  encodeFrame,
  decodeFrame,
  encodeAddContact,
  encodeAddRoom,
  encodeSetPin,
  encodeSetLockPin,
  encodeSetLockConfig,
  LOCK_SCREEN_ENABLE,
  FRAME_QUERY_STATUS,
  FRAME_QUERY_CONTACTS,
  FRAME_QUERY_CHANNELS,
  FRAME_QUERY_ADVERT,
  FRAME_QUERY_ROOMS,
  FRAME_ADD_CONTACT,
  FRAME_ADD_ROOM,
  FRAME_DEL_ROOM,
  FRAME_SET_PIN,
  FRAME_SET_LOCK_PIN,
  FRAME_SET_LOCK_CONFIG,
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
  HISTORY_MSG_TYPE_DM,
  HISTORY_MSG_TYPE_GRP_TXT,
} from "./codec.js";
import { ProvisionerSession, DeviceError } from "./session.js";

// ── Fake Web Serial port ─────────────────────────────────────────────────

/**
 * Build a fake `SerialPort`-shaped object backed by real `ReadableStream`/
 * `WritableStream` instances, so `session.js`'s `getReader()`/`getWriter()`
 * calls behave exactly as they would against a real port. `onWrite(chunk)`
 * is called synchronously for every frame the session writes (used to drive
 * scripted device responses and to assert what was sent); the returned
 * `push(bytes)` enqueues bytes as if the device sent them; `signalsCalls`
 * records every `setSignals()` invocation in order (used to assert
 * `connect()` never calls it at all — see session.js's `connect()` doc
 * comment for why); `error(err)` makes the readable stream error out, as if
 * the device vanished mid-read (e.g. a USB re-enumeration from a hard
 * reset).
 */
function makeFakePort(onWrite) {
  let controller;
  const readable = new ReadableStream({
    start(c) {
      controller = c;
    },
  });
  const writable = new WritableStream({
    write(chunk) {
      onWrite(chunk);
    },
  });
  const signalsCalls = [];
  const port = {
    readable,
    writable,
    signalsCalls,
    async open() {},
    async setSignals(signals) {
      signalsCalls.push(signals);
    },
    async close() {
      try {
        controller.close();
      } catch {
        // Already closed by a prior push/close — fine.
      }
    },
  };
  return {
    port,
    push: (bytes) => controller.enqueue(bytes),
    error: (err) => controller.error(err),
  };
}

function installFakeGlobals(port) {
  // Node (21+) ships its own read-only `navigator` global (a
  // `userAgent`-only stub) — a plain `globalThis.navigator = ...` assignment
  // throws "Cannot set property navigator of #<Object> which has only a
  // getter". `defineProperty` overrides it outright, which is fine here
  // since this file only ever runs standalone under `node`, never alongside
  // other code that might depend on Node's real `navigator` stub.
  Object.defineProperty(globalThis, "navigator", {
    value: {
      serial: {
        async requestPort() {
          return port;
        },
        addEventListener() {},
      },
    },
    writable: true,
    configurable: true,
  });
  globalThis.window = { isSecureContext: true };
}

function concat(...parts) {
  const total = parts.reduce((n, p) => n + p.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}

// ── Test fixtures: a QUERY_STATUS response pair ──────────────────────────

const PUBKEY = new Uint8Array(32).map((_, i) => i);

function buildStatusPayload() {
  const buf = new Uint8Array(55); // legacy (pre-battery_raw_mv) length, on purpose
  const view = new DataView(buf.buffer);
  buf[0] = 1; // provisioned
  buf.set(PUBKEY, 1);
  buf[33] = 3; // contact_count
  buf[34] = 2; // channel_count
  buf[35] = 1; // gps_has_fix
  view.setInt32(36, 123456789, true); // gps_lat_e7
  view.setInt32(40, -987654321, true); // gps_lon_e7
  view.setUint32(44, 42, true); // gps_fix_age_secs
  buf[48] = 1; // gps_clock_synced
  view.setUint32(49, 7, true); // gps_clock_sync_age_secs
  buf[53] = 88; // battery_percent
  buf[54] = 0; // battery_charging
  return buf;
}

function buildIdentityPayload() {
  const name = new TextEncoder().encode("Cadet-Test");
  const buf = new Uint8Array(34 + name.length);
  buf.set(PUBKEY, 0);
  buf[32] = PUBKEY[0]; // pub_hash
  buf[33] = name.length;
  buf.set(name, 34);
  return buf;
}

const statusFrame = encodeFrame(FRAME_RSP_STATUS, buildStatusPayload());
const identityFrame = encodeFrame(FRAME_RSP_IDENTITY, buildIdentityPayload());

/**
 * Build a wire-shaped (but unsigned — `decodeRspAdvert` never checks the
 * signature, only structure) self-advert card payload:
 * `header(1)=0x11 | path_len(1)=0x00 | pubkey(32) | timestamp(4 LE) |
 * signature(64) | flags(1)=0x81 | name`. Mirrors `protocol::advert`'s wire
 * format. Deliberately sized well past `MAX_RSP_HISTORY_ENTRY_PAYLOAD` (73
 * B) for any non-trivial name — every real card is — so tests using it
 * exercise `#tryExtractFrame`'s plen guard at the size that matters.
 */
function buildAdvertCardPayload(name) {
  const nameBytes = new TextEncoder().encode(name);
  const buf = new Uint8Array(102 + 1 + nameBytes.length);
  buf[0] = 0x11; // header: VER0 | ADVERT<<2 | FLOOD
  buf[1] = 0x00; // path_len
  buf.fill(0xaa, 2, 34); // pubkey
  new DataView(buf.buffer).setUint32(34, 1_700_000_000, true); // timestamp
  buf.fill(0xbb, 38, 102); // signature (unverified by decodeRspAdvert)
  buf[102] = 0x81; // appdata flags: ADV_TYPE_CHAT | ADV_NAME_MASK
  buf.set(nameBytes, 103);
  return buf;
}

function assertStatusAndIdentity(status, identity) {
  assert.equal(status.provisioned, true);
  assert.equal(status.contact_count, 3);
  assert.equal(status.channel_count, 2);
  assert.equal(status.gps_has_fix, true);
  assert.equal(status.gps_lat_e7, 123456789);
  assert.equal(status.gps_lon_e7, -987654321);
  assert.equal(status.gps_fix_age_secs, 42);
  assert.equal(status.gps_clock_synced, true);
  assert.equal(status.gps_clock_sync_age_secs, 7);
  assert.equal(status.battery_percent, 88);
  assert.equal(status.battery_charging, false);
  assert.equal(status.battery_raw_mv, 0); // legacy 55-byte payload default
  assert.deepEqual(Array.from(identity.pubkey), Array.from(PUBKEY));
  assert.equal(identity.device_name, "Cadet-Test");
}

// ── Scenario 1: straight two-frame handshake + magic-resync past log noise ─

async function happyPathWithLogNoiseResync() {
  const written = [];
  const { port, push } = makeFakePort((chunk) => written.push(chunk));
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  assert.equal(ProvisionerSession.isSupported(), true);
  assert.equal(ProvisionerSession.isSecureContext(), true);

  await session.connect();
  assert.equal(session.isConnected, true);

  // ESP-IDF-style log noise, PLUS a false "MC" magic candidate with a bad
  // CRC (exercises decodeFrame's CrcMismatch -> 1-byte-advance resync path
  // in #tryExtractFrame), all interleaved before the real response frames —
  // mirrors what a real T-Deck sends on the shared USB-serial stream.
  const logNoise = new TextEncoder().encode("I (1234) prov_server: ready\n");
  const falseMagic = new Uint8Array([0x4d, 0x43, 0x01, 0x00, 0x00, 0x00, 0x00]); // "MC" + bogus header, wrong CRC

  const queryPromise = session.queryStatus();

  // Deliver in two chunks (across separate #readLoop iterations) to also
  // exercise the accumulation-buffer merge path, not just a single read().
  push(concat(logNoise, falseMagic));
  await new Promise((r) => setTimeout(r, 5));
  push(concat(statusFrame, identityFrame));

  const { status, identity } = await queryPromise;
  assertStatusAndIdentity(status, identity);

  // Exactly one QUERY_STATUS frame was sent (no retry needed on the happy path).
  assert.equal(written.length, 1);
  const sent = decodeFrame(written[0]);
  assert.equal(sent.frameType, FRAME_QUERY_STATUS);
  assert.equal(sent.payload.length, 0);

  await session.disconnect();
  assert.equal(session.isConnected, false);
}

// ── Scenario 1b: connect() never touches setSignals() at all ─────────────
//
// Regression guard for TWO successive defects on the same line, in order:
// (a) PR #197 fixed the host CLI (`host/src/transport.rs`) to leave DTR/RTS
// untouched; the browser client never got the equivalent treatment, so PR
// #200 added a post-open `setSignals({dataTerminalReady: false,
// requestToSend: false})` de-assert, reasoning it might "beat" the reset
// pulse `open()`'s own forced assert triggers on this board's CH343
// auto-program wiring (EN/IO0). (b) `meshcadet-provisioner-connect-
// unbounded-awaits-hang` (this mission) got the first real hardware
// evidence against that de-assert and found it left the device unreachable
// by the host CLI until a PHYSICAL RESET — `setSignals()`'s two line
// changes are not guaranteed atomic below the JS call, and a staggered
// transition on this wiring is indistinguishable from `site/flash.js`'s
// vendored `esptool-js`'s deliberate bootloader-entry dance. The fix:
// `connect()` must not call `setSignals()` at all — see its doc comment for
// the full history and why relying on `open()`'s own (already
// field-proven-survivable, pre-#200) forced assert is the smallest correct
// fix.

async function connectNeverCallsSetSignals() {
  const { port } = makeFakePort(() => {});
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  assert.deepEqual(port.signalsCalls, []);

  await session.disconnect();
}

// ── Scenario 1e: writer.write() that HANGS surfaces a distinct
//    "write stalled" error within bounded time — the other unbounded-await
//    defect this mission fixes ──────────────────────────────────────────
//
// `#sendFrame`'s `await this.#writer.write(...)` was, pre-fix, a bare await
// with no bound at all — reachable on every attempt of every command, not
// just connect(). A wedged writable stream hung the whole call forever,
// with the 10s retry budget never even consulted (it only ever wrapped the
// receive side). This must surface its OWN cause ("write stalled"),
// distinct from a plain receive timeout ("no response"), within bounded
// time — not hang, and not report a generic timeout that buries which of
// the two actually happened.

function makeFakePortWithStallingWrite() {
  const readable = new ReadableStream({ start() {} });
  const writable = new WritableStream({
    write() {
      return new Promise(() => {}); // never settles
    },
  });
  return {
    readable,
    writable,
    async open() {},
    async setSignals() {},
    async close() {},
  };
}

async function writeStallSurfacesDistinctErrorWithinBoundedTime() {
  const port = makeFakePortWithStallingWrite();
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const start = Date.now();
  await assert.rejects(() => session.queryStatus(), /write stalled/);
  const elapsed = Date.now() - start;
  assert.ok(elapsed < 4000, `write stall must surface in bounded time (took ${elapsed}ms)`);
  assert.ok(
    elapsed > 1000,
    `write stall should genuinely wait out the bounded write timeout, not resolve suspiciously early (took ${elapsed}ms)`
  );

  // A stalled write is treated as fatal (mirrors #readLoop's own
  // #fatalError latch for a dead read side) — a SECOND command must fail
  // fast with the same cause rather than re-attempting a doomed write.
  await assert.rejects(() => session.listContacts(), /write stalled/);

  await session.disconnect();
}

// ── Scenario 1f: disconnect() does not hang when reader.cancel() stalls —
//    audited per this mission's Task 4 (teardown is a candidate too) ─────
//
// A hang here would strand the user with no recovery but a page reload —
// exactly the failure mode a "disconnect and reconnect" path exists to
// prevent.

function makeFakePortWithStallingCancel() {
  const readable = new ReadableStream({
    start() {},
    cancel() {
      return new Promise(() => {}); // never settles
    },
  });
  const writable = new WritableStream({ write() {} });
  return {
    readable,
    writable,
    async open() {},
    async setSignals() {},
    async close() {},
  };
}

async function disconnectDoesNotHangWhenReaderCancelNeverSettles() {
  const port = makeFakePortWithStallingCancel();
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const start = Date.now();
  await session.disconnect(); // must not hang
  const elapsed = Date.now() - start;
  assert.equal(session.isConnected, false);
  assert.ok(elapsed < 4000, `disconnect() must not hang when reader.cancel() stalls (took ${elapsed}ms)`);
}

// ── Scenario 1c: connect survives a device reset it cannot prevent — a
//    full ESP-IDF boot banner across the first (dropped) attempt, resolved
//    on retry ────────────────────────────────────────────────────────────
//
// Even a "successful" de-assert (Scenario 1b) may not beat an
// already-in-flight reset pulse (see session.js's `connect()` doc comment):
// the reset-tolerant path is what actually has to hold. This drives
// `queryStatus()` — the very first command `provisioner.js` issues right
// after `connect()` resolves — against a device that answers with a plain
// ESP-IDF reboot banner (no valid frame anywhere in it) for the whole first
// 500ms attempt, then goes silent long enough for that attempt to time out,
// then answers for real once `#sendRecvWithRetry` retries. Proves the
// "settle, drain the banner, retry" sequence is already load-bearing in the
// existing retry/resync machinery — no separate boot-wait step needed in
// `connect()` itself.

async function connectSurvivesResetBootBannerBeforeFirstQueryStatus() {
  let writeCount = 0;
  const { port, push } = makeFakePort(() => {
    writeCount++;
    if (writeCount === 1) {
      // Mid-reboot: a burst of ESP-IDF boot log lines, no valid frame
      // anywhere in them, delivered immediately. No further data follows —
      // the 500ms per-attempt timeout fires and #sendRecvWithRetry retries.
      const banner = new TextEncoder().encode(
        "ets Jul 29 2019 12:21:46\n" +
          "rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\n" +
          "I (412) boot: ESP-IDF v5.2 2nd stage bootloader\n" +
          "I (890) prov_server: starting provisioning server\n"
      );
      push(banner);
    } else if (writeCount === 2) {
      // The retry: the device has finished booting and answers for real.
      setTimeout(() => {
        push(statusFrame);
        push(identityFrame);
      }, 5);
    }
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();
  assert.deepEqual(port.signalsCalls, []); // connect() no longer touches setSignals() — see its doc comment

  const { status, identity } = await session.queryStatus();
  assertStatusAndIdentity(status, identity);
  assert.equal(writeCount, 2, "expected exactly one retry to survive the boot banner");

  await session.disconnect();
}

// ── Scenario 1d: a read-loop failure (e.g. the device vanishing mid-wait,
//    as a hard EN reset re-enumerating the USB device might cause) surfaces
//    as a real rejection to whoever is waiting — never a silent stall ────

async function readLoopErrorSurfacesAsRealRejectionNotSilentStall() {
  const { port, error } = makeFakePort(() => {
    // The device disappears mid-attempt instead of ever answering — the
    // underlying stream errors out rather than just going quiet.
    setTimeout(() => error(new Error("device disappeared")), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await assert.rejects(() => session.queryStatus(), /device disappeared/);

  await session.disconnect();
}

// ── Scenario 1e: a device that keeps re-emitting the ESP32-S3 ROM boot
//    banner on every retry attempt (a real repeated USB-Serial-JTAG chip
//    reset, not just a slow boot) times out with a message naming the
//    reboot count and the accurate recovery action — not a generic "timeout
//    waiting for response frame" that leaves the real cause to be
//    reverse-engineered from a byte count, the way this exact scenario cost
//    the campaign a full round (see
//    `docs/provisioning-connect-verification-kit.md`). Round 7 (`meshcadet-
//    connect-wedge-round7-stale-handle-reenumeration`) first added the
//    reboot-count reporting but told the user to "reconnect to continue" —
//    round 8 (`meshcadet-connect-wedge-round8-host-usb-endpoint-state`,
//    hardware evidence) RETRACTS that: a browser-side reconnect cannot force
//    host-side re-enumeration, so this test now asserts the accurate
//    guidance instead ─────────────────────────────────────────────────────

async function repeatedRebootBannerReportsCountAndActionableTimeoutMessage() {
  const { port, push } = makeFakePort(() => {
    // Every attempt: the device is still resetting and answers with a
    // fresh ROM boot banner, never a valid frame — #sendRecvWithRetry keeps
    // retrying until its (deliberately shortened, for this test) overall
    // budget elapses and the whole command times out.
    push(
      new TextEncoder().encode(
        "ESP-ROM:esp32s3-20210327\n" +
          "Build:Mar 27 2021\n" +
          "rst:0x15 (USB_UART_CHIP_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n"
      )
    );
  });
  installFakeGlobals(port);

  // Shortened retry budget so this test proves a real end-to-end timeout
  // (not just the message-building logic in isolation) without an actual
  // multi-second wait — mirrors `Session::with_retry_params`'s reason for
  // existing (`host/src/session.rs`).
  const session = new ProvisionerSession({ retryAttemptMs: 20, retryTotalMs: 70 });
  await session.connect();

  await assert.rejects(
    () => session.queryStatus(),
    (err) => {
      assert.match(
        err.message,
        /device rebooted \d+ times? during this command/,
        `expected the reboot count in the timeout message, got: ${err.message}`
      );
      assert.match(
        err.message,
        /Web Serial exposes no way for this page to force host-side re-enumeration/,
        `expected the accurate (round 8) recovery guidance, got: ${err.message}`
      );
      assert.doesNotMatch(
        err.message,
        /reconnect to continue/,
        `must not repeat round 7's retracted claim that reconnecting alone clears this, got: ${err.message}`
      );
      return true;
    }
  );

  await session.disconnect();
}

// ── Scenario 1f: the reboot-banner scan must not miss an occurrence split
//    across two separate reads — exactly the shape a real USB transfer can
//    deliver (the banner arriving in more than one `reader.read()` chunk) ──

async function rebootBannerSplitAcrossTwoReadsIsStillCounted() {
  const fullBanner = "ESP-ROM:esp32s3-20210327\nBuild:Mar 27 2021\n";
  const splitPoint = 8; // "ESP-ROM:" — splits inside the banner text itself
  const firstHalf = fullBanner.slice(0, splitPoint);
  const secondHalf = fullBanner.slice(splitPoint);

  let writeCount = 0;
  const { port, push } = makeFakePort(() => {
    writeCount++;
    if (writeCount !== 1) {
      // Only the FIRST attempt delivers the (split) banner — later retries
      // go silent, so the total reboot count this test asserts on stays
      // exactly 1 regardless of how many retries the shortened budget below
      // allows, isolating this test to the split-boundary question alone.
      return;
    }
    // Deliver the SAME banner occurrence as two genuinely separate
    // `#readLoop` reads (the `setTimeout` gap forces the first chunk all the
    // way through `#tryExtractFrame`/`#recordDiscarded` — populating
    // `#rebootScanCarry` — before the second chunk arrives), rather than two
    // synchronous pushes that risk landing in `#accBuf` together before
    // either is ever scanned, which would trivially "pass" without
    // exercising the carry-over boundary this test targets.
    push(new TextEncoder().encode(firstHalf));
    setTimeout(() => push(new TextEncoder().encode(secondHalf)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession({ retryAttemptMs: 20, retryTotalMs: 40 });
  await session.connect();

  await assert.rejects(
    () => session.queryStatus(),
    (err) => {
      assert.match(
        err.message,
        /device rebooted 1 time during this command/,
        `expected the split banner to be counted exactly once, got: ${err.message}`
      );
      return true;
    }
  );

  await session.disconnect();
}

// ── Scenario 1g (round 8, `meshcadet-connect-wedge-round8-host-usb-endpoint-
//    state`): a write stall on a RETRY attempt, after the device was already
//    observed rebooting earlier in the same command, must report the reboot
//    count and the accurate recovery guidance too — not a bare "write
//    stalled" with no context. This is the regression guard for the
//    `#sendFrame`-outside-`try` defect: before the fix, a write stall always
//    skipped `#withRebootContext` no matter what `#rebootCount` already was,
//    because `#sendRecvWithRetry`'s catch block never saw it ─────────────

function makeFakePortRebootThenStallingWrite() {
  let controller;
  const readable = new ReadableStream({
    start(c) {
      controller = c;
    },
  });
  let writeCount = 0;
  const writable = new WritableStream({
    write() {
      writeCount += 1;
      if (writeCount === 1) {
        // First attempt: the device answers with a reboot banner and no
        // valid frame — this attempt times out and #sendRecvWithRetry
        // retries.
        controller.enqueue(
          new TextEncoder().encode(
            "ESP-ROM:esp32s3-20210327\n" +
              "Build:Mar 27 2021\n" +
              "rst:0x15 (USB_UART_CHIP_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n"
          )
        );
        return Promise.resolve();
      }
      // The retry's write itself stalls forever.
      return new Promise(() => {});
    },
  });
  return {
    port: {
      readable,
      writable,
      async open() {},
      async setSignals() {},
      async close() {
        try {
          controller.close();
        } catch {
          // Already closed — fine.
        }
      },
    },
  };
}

async function writeStallAfterObservedRebootReportsRebootContext() {
  const { port } = makeFakePortRebootThenStallingWrite();
  installFakeGlobals(port);

  // Short per-attempt/frame timeouts so the first (reboot-banner) attempt
  // times out quickly and retries into the stalling write; the fixed
  // UNBOUNDED_CALL_TIMEOUT_MS (2s, not test-overridable) still bounds the
  // write stall itself, so this test genuinely waits that out.
  const session = new ProvisionerSession({ retryAttemptMs: 20, retryTotalMs: 60_000, frameTimeoutMs: 20 });
  await session.connect();

  await assert.rejects(
    () => session.queryStatus(),
    (err) => {
      assert.match(
        err.message,
        /write stalled/,
        `expected the write-stall cause to still be named, got: ${err.message}`
      );
      assert.match(
        err.message,
        /device rebooted 1 time during this command/,
        `expected the reboot observed on the earlier attempt to be reported, got: ${err.message}`
      );
      assert.match(
        err.message,
        /Web Serial exposes no way for this page to force host-side re-enumeration/,
        `expected the accurate recovery guidance, got: ${err.message}`
      );
      return true;
    }
  );

  await session.disconnect();
}

// ── Scenario 2: send_recv_with_retry actually retries a dropped first frame ─

async function retryOnDroppedFirstResponse() {
  let writeCount = 0;
  const { port, push } = makeFakePort(() => {
    writeCount++;
    if (writeCount === 2) {
      // Device "wakes up" only on the second (retried) command frame. The
      // first attempt is deliberately left unanswered so the 500ms
      // per-attempt timeout inside #sendRecvWithRetry fires and retries.
      setTimeout(() => {
        push(statusFrame);
        push(identityFrame);
      }, 5);
    }
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const { status, identity } = await session.queryStatus();
  assertStatusAndIdentity(status, identity);
  assert.equal(writeCount, 2, "expected exactly one retry (two sends total)");

  await session.disconnect();
}

// ── Scenario 2b: a retry's stale duplicate reply must not desync a LATER,
//    unrelated command ─────────────────────────────────────────────────────
//
// Reproduces the reported field defect: `queryStatus()`'s first attempt
// times out (per-attempt 500ms) and `#sendRecvWithRetry` retries, but the
// device actually answers BOTH the original attempt (late) and the retry.
// `queryStatus()` only ever consumes the retry's two frames and resolves —
// the original's duplicate RSP_STATUS/RSP_IDENTITY pair arrives afterward,
// once nothing is waiting on it, and must be discarded rather than left to
// desync whatever command runs next. Without the cross-command residue
// guard in `#sendRecvWithRetry`, `listContacts()` (called after the
// duplicate has landed, mirroring `provisioner.js`'s real
// queryStatus -> render -> listContacts -> listChannels sequence) reads the
// leftover RSP_STATUS (0x82) as its own response and throws "unexpected
// frame 0x82 during contact enumeration"; `listChannels()` then inherits the
// leftover RSP_IDENTITY (0x83) the same way — exactly the two "unexpected
// frame" defects reported against a real device.
async function staleRetryDuplicateDoesNotDesyncNextCommand() {
  let writeCount = 0;
  const { port, push } = makeFakePort((chunk) => {
    writeCount++;
    const { frameType } = decodeFrame(chunk);
    if (writeCount === 1) {
      // The original QUERY_STATUS attempt: the device answers, but only
      // after the 500ms per-attempt timeout has already elapsed and
      // #sendRecvWithRetry has moved on to a retry.
      setTimeout(() => {
        push(statusFrame);
        push(identityFrame);
      }, 600);
    } else if (writeCount === 2) {
      // The retry: device answers promptly, and #sendRecvWithRetry/
      // queryStatus() resolve from this pair alone.
      setTimeout(() => {
        push(statusFrame);
        push(identityFrame);
      }, 5);
    } else if (frameType === FRAME_QUERY_CONTACTS) {
      setTimeout(() => push(encodeFrame(FRAME_RSP_CONTACTS_DONE)), 5);
    } else if (frameType === FRAME_QUERY_CHANNELS) {
      setTimeout(() => push(encodeFrame(FRAME_RSP_CHANNELS_DONE)), 5);
    }
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const { status, identity } = await session.queryStatus();
  assertStatusAndIdentity(status, identity);
  assert.equal(writeCount, 2, "expected exactly one retry (two sends total)");

  // Let the original attempt's late, duplicate reply land — mirrors the real
  // gap between a status refresh resolving/rendering and the next user- or
  // poll-driven command (listContacts) actually being issued.
  await new Promise((r) => setTimeout(r, 200));

  const contacts = await session.listContacts();
  assert.deepEqual(contacts, []);

  const channels = await session.listChannels();
  assert.deepEqual(channels, []);

  await session.disconnect();
}

// ── Scenario 3: disconnect() before connect() is a harmless no-op ────────

async function disconnectWithoutConnect() {
  const { port } = makeFakePort(() => {});
  installFakeGlobals(port);
  const session = new ProvisionerSession();
  await session.disconnect(); // must not throw
  assert.equal(session.isConnected, false);
}

// ── Scenario 4: device answers QUERY_STATUS with RSP_ERROR ───────────────

async function deviceErrorOnQueryStatus() {
  const { port, push } = makeFakePort(() => {
    // error_code(1) | msg_len(1) | msg(msg_len) — wire layout for RSP_ERROR.
    const msg = new TextEncoder().encode("not ready");
    const payload = new Uint8Array(2 + msg.length);
    payload[0] = 7; // error_code
    payload[1] = msg.length;
    payload.set(msg, 2);
    setTimeout(() => push(encodeFrame(FRAME_RSP_ERROR, payload)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await assert.rejects(
    () => session.queryStatus(),
    (err) => {
      assert.ok(err instanceof DeviceError, `expected DeviceError, got ${err}`);
      assert.equal(err.errorCode, 7);
      assert.match(err.message, /not ready/);
      return true;
    }
  );

  await session.disconnect();
}

// ── Scenario 5: an unexpected frame follows RSP_STATUS instead of RSP_IDENTITY ─

async function unexpectedFrameAfterStatus() {
  const { port, push } = makeFakePort(() => {
    setTimeout(() => {
      push(statusFrame);
      // RSP_OK instead of the expected RSP_IDENTITY — a genuine protocol
      // desync the caller must surface, not silently misinterpret.
      push(encodeFrame(FRAME_RSP_OK));
    }, 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await assert.rejects(() => session.queryStatus(), /expected RSP_IDENTITY.*got 0x80/);

  await session.disconnect();
}

// ── Scenario 6: listContacts streams RSP_CONTACT*N -> RSP_CONTACTS_DONE ──

function buildContactPayload(index, pubkeyByte, telemetry, name) {
  const nameBytes = new TextEncoder().encode(name);
  const buf = new Uint8Array(35 + nameBytes.length);
  buf[0] = index;
  buf.fill(pubkeyByte, 1, 33);
  buf[33] = telemetry ? 1 : 0;
  buf[34] = nameBytes.length;
  buf.set(nameBytes, 35);
  return buf;
}

async function listContactsStreamsToDone() {
  const { port, push } = makeFakePort(() => {
    setTimeout(() => {
      push(encodeFrame(FRAME_RSP_CONTACT, buildContactPayload(0, 0xaa, true, "Alice")));
      push(encodeFrame(FRAME_RSP_CONTACT, buildContactPayload(1, 0xbb, false, "Bob")));
      push(encodeFrame(FRAME_RSP_CONTACTS_DONE, new Uint8Array(0)));
    }, 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const contacts = await session.listContacts();
  assert.equal(contacts.length, 2);
  assert.equal(contacts[0].index, 0);
  assert.equal(contacts[0].telemetry_enable, true);
  assert.equal(contacts[0].display_name, "Alice");
  assert.equal(contacts[1].index, 1);
  assert.equal(contacts[1].telemetry_enable, false);
  assert.equal(contacts[1].display_name, "Bob");

  await session.disconnect();
}

// ── Scenario 7: listChannels streams RSP_CHANNEL*N -> RSP_CHANNELS_DONE ──

function buildChannelPayload(index, hash, keyLen, primary, name) {
  const nameBytes = new TextEncoder().encode(name);
  const buf = new Uint8Array(5 + nameBytes.length);
  buf[0] = index;
  buf[1] = hash;
  buf[2] = keyLen;
  buf[3] = primary ? 1 : 0;
  buf[4] = nameBytes.length;
  buf.set(nameBytes, 5);
  return buf;
}

async function listChannelsStreamsToDone() {
  const { port, push } = makeFakePort(() => {
    setTimeout(() => {
      push(encodeFrame(FRAME_RSP_CHANNEL, buildChannelPayload(0, 0x12, 32, true, "family")));
      push(encodeFrame(FRAME_RSP_CHANNELS_DONE, new Uint8Array(0)));
    }, 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const channels = await session.listChannels();
  assert.equal(channels.length, 1);
  assert.equal(channels[0].channel_hash, 0x12);
  assert.equal(channels[0].key_len, 32);
  assert.equal(channels[0].primary, true);
  assert.equal(channels[0].name, "family");

  await session.disconnect();
}

// ── Scenario 8: addContact happy path sends the right frame + payload ───

async function addContactSendsCorrectFrame() {
  const written = [];
  const { port, push } = makeFakePort((chunk) => {
    written.push(chunk);
    setTimeout(() => push(encodeFrame(FRAME_RSP_OK)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const pubkey = new Uint8Array(32).fill(0xcc);
  await session.addContact(pubkey, true, "Carol");

  assert.equal(written.length, 1);
  const sent = decodeFrame(written[0]);
  assert.equal(sent.frameType, FRAME_ADD_CONTACT);
  assert.deepEqual(Array.from(sent.payload), Array.from(encodeAddContact(pubkey, true, "Carol")));

  await session.disconnect();
}

// ── Scenario 9: a write command answered with RSP_ERROR surfaces as DeviceError ─

async function writeCommandDeviceError() {
  const { port, push } = makeFakePort(() => {
    const msg = new TextEncoder().encode("contact list full");
    const payload = new Uint8Array(2 + msg.length);
    payload[0] = 3;
    payload[1] = msg.length;
    payload.set(msg, 2);
    setTimeout(() => push(encodeFrame(FRAME_RSP_ERROR, payload)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await assert.rejects(
    () => session.addContact(new Uint8Array(32), false, "Dave"),
    (err) => {
      assert.ok(err instanceof DeviceError);
      assert.equal(err.errorCode, 3);
      assert.match(err.message, /contact list full/);
      return true;
    }
  );

  await session.disconnect();
}

// ── Scenario 10: commit() resolves on RSP_OK — a first-boot-style port
//    teardown immediately afterward is not observed by commit() itself ────
//
// Mirrors the firmware's real sequence (provisioning_server.rs's "USB-DRAIN
// GUARD": RSP_OK, THEN a 250ms dwell, THEN esp_restart()): by the time the
// fake port closes here, commit()'s `await` has already resolved with the
// decoded RSP_OK frame, so the ensuing teardown cannot retroactively turn
// that resolved promise into a rejection — exercising exactly the ordering
// invariant `provisioner.js`'s commit handler relies on.

async function commitResolvesBeforeSimulatedReboot() {
  const { port, push } = makeFakePort(() => {
    setTimeout(() => {
      push(encodeFrame(FRAME_RSP_OK));
      // Simulate the device closing the port shortly after RSP_OK, as a
      // first-boot commit's esp_restart() would from the OS's perspective.
      setTimeout(() => port.close(), 5);
    }, 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await session.commit(); // must resolve, not throw

  // Give the simulated post-commit port-close a moment to actually land —
  // asserts it doesn't retroactively surface as a rejection anywhere (e.g.
  // an unhandled rejection from #readLoop) by simply letting the test
  // process continue cleanly past this point.
  await new Promise((r) => setTimeout(r, 20));
}

// ── Scenario 11: concurrent command calls are serialized, not interleaved ─
//
// `queryStatus`/`listContacts`/`listChannels`/`addContact`/`delContact`/
// `addChannel`/`delChannel`/`setNotifDefaults`/`setDeviceName`/`commit` all
// route through `#exclusive` precisely so that two calls issued close
// together (e.g. a background refresh racing a form submit) never write
// their frames out of order — the single physical link allows exactly one
// outstanding request/response at a time. This fires two command calls back
// to back, without awaiting the first, and asserts the second's frame is
// only written after the first's full exchange completes.

async function concurrentCommandsAreSerialized() {
  const written = [];
  const { port, push } = makeFakePort((chunk) => {
    written.push(decodeFrame(chunk).frameType);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const statusPromise = session.queryStatus();
  const addPromise = session.addContact(new Uint8Array(32).fill(0xee), false, "Erin");

  // Let both promises' synchronous-ish startup run. If the two command
  // calls were NOT serialized, ADD_CONTACT's frame would already be written
  // here, racing QUERY_STATUS's still-unanswered exchange.
  await new Promise((r) => setTimeout(r, 5));
  assert.equal(written.length, 1, "addContact must not write until queryStatus's exchange completes");
  assert.equal(written[0], FRAME_QUERY_STATUS);

  push(statusFrame);
  push(identityFrame);
  const { status, identity } = await statusPromise;
  assertStatusAndIdentity(status, identity);

  // Only now should addContact's write follow.
  await new Promise((r) => setTimeout(r, 5));
  assert.equal(written.length, 2, "addContact's write should follow queryStatus's completion");
  assert.equal(written[1], FRAME_ADD_CONTACT);

  push(encodeFrame(FRAME_RSP_OK));
  await addPromise;

  await session.disconnect();
}

// ── Scenario 12: setPin sends FRAME_SET_PIN with the encoded PIN payload ──

async function setPinSendsCorrectFrame() {
  const written = [];
  const { port, push } = makeFakePort((chunk) => {
    written.push(chunk);
    setTimeout(() => push(encodeFrame(FRAME_RSP_OK)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await session.setPin("1234");

  assert.equal(written.length, 1);
  const sent = decodeFrame(written[0]);
  assert.equal(sent.frameType, FRAME_SET_PIN);
  // Payload is pin_len(1) | pin bytes — matches encodeSetPin exactly. (The
  // session scrubs its own copy after send; the bytes already handed to the
  // fake writer here are the frame the device would have received.)
  assert.deepEqual(Array.from(sent.payload), Array.from(encodeSetPin("1234")));

  await session.disconnect();
}

// ── Scenario 12b: setLockPin sends FRAME_SET_LOCK_PIN with the encoded lock-PIN payload ──

async function setLockPinSendsCorrectFrame() {
  const written = [];
  const { port, push } = makeFakePort((chunk) => {
    written.push(chunk);
    setTimeout(() => push(encodeFrame(FRAME_RSP_OK)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await session.setLockPin("4321");

  assert.equal(written.length, 1);
  const sent = decodeFrame(written[0]);
  assert.equal(sent.frameType, FRAME_SET_LOCK_PIN);
  // Payload is exactly LOCK_PIN_LEN bytes, no length prefix — matches
  // encodeSetLockPin exactly (unlike encodeSetPin, no truncation possible).
  assert.deepEqual(Array.from(sent.payload), Array.from(encodeSetLockPin("4321")));

  await session.disconnect();
}

// ── Scenario 12c: setLockConfig sends FRAME_SET_LOCK_CONFIG with lock_flags + lock_timeout_s ──

async function setLockConfigSendsCorrectFrame() {
  const written = [];
  const { port, push } = makeFakePort((chunk) => {
    written.push(chunk);
    setTimeout(() => push(encodeFrame(FRAME_RSP_OK)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await session.setLockConfig(LOCK_SCREEN_ENABLE, 300);

  assert.equal(written.length, 1);
  const sent = decodeFrame(written[0]);
  assert.equal(sent.frameType, FRAME_SET_LOCK_CONFIG);
  assert.deepEqual(Array.from(sent.payload), Array.from(encodeSetLockConfig(LOCK_SCREEN_ENABLE, 300)));

  await session.disconnect();
}

// ── Scenario 13: exportHistory streams RSP_HISTORY_ENTRY*N -> RSP_HISTORY_DONE ─

/** Wire layout: index(1) | sender_hash(1) | msg_type(1) | timestamp(4 LE) | text_len(1) | is_ours(1) | text(text_len). */
function buildHistoryPayload(index, senderHash, msgType, timestamp, isOurs, text) {
  const textBytes = new TextEncoder().encode(text);
  const buf = new Uint8Array(9 + textBytes.length);
  const view = new DataView(buf.buffer);
  buf[0] = index;
  buf[1] = senderHash;
  buf[2] = msgType;
  view.setUint32(3, timestamp, true);
  buf[7] = textBytes.length;
  buf[8] = isOurs ? 1 : 0;
  buf.set(textBytes, 9);
  return buf;
}

async function exportHistoryStreamsToDone() {
  const { port, push } = makeFakePort(() => {
    setTimeout(() => {
      push(encodeFrame(FRAME_RSP_HISTORY_ENTRY, buildHistoryPayload(0, 0x11, HISTORY_MSG_TYPE_DM, 1_700_000_000, false, "hi there")));
      push(encodeFrame(FRAME_RSP_HISTORY_ENTRY, buildHistoryPayload(1, 0x22, HISTORY_MSG_TYPE_GRP_TXT, 1_700_000_010, true, "sent one")));
      push(encodeFrame(FRAME_RSP_HISTORY_DONE, new Uint8Array(0)));
    }, 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const entries = await session.exportHistory();
  assert.equal(entries.length, 2);
  // Oldest-first, as the device streams them.
  assert.equal(entries[0].sender_hash, 0x11);
  assert.equal(entries[0].msg_type, HISTORY_MSG_TYPE_DM);
  assert.equal(entries[0].is_ours, false);
  assert.equal(entries[0].text, "hi there");
  assert.equal(entries[1].sender_hash, 0x22);
  assert.equal(entries[1].msg_type, HISTORY_MSG_TYPE_GRP_TXT);
  assert.equal(entries[1].is_ours, true);
  assert.equal(entries[1].text, "sent one");

  await session.disconnect();
}

// ── Scenario 14: exportHistory tolerates a bounded stray reply, then completes ─

async function exportHistoryToleratesStrayFrame() {
  const { port, push } = makeFakePort(() => {
    setTimeout(() => {
      // A leftover RSP_OK (e.g. an earlier command's reply still draining)
      // arrives before the history stream — must be tolerated, not fatal.
      push(encodeFrame(FRAME_RSP_OK));
      push(encodeFrame(FRAME_RSP_HISTORY_ENTRY, buildHistoryPayload(0, 0x33, HISTORY_MSG_TYPE_DM, 1_700_000_000, false, "after stray")));
      push(encodeFrame(FRAME_RSP_HISTORY_DONE, new Uint8Array(0)));
    }, 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const entries = await session.exportHistory();
  assert.equal(entries.length, 1);
  assert.equal(entries[0].text, "after stray");

  await session.disconnect();
}

// ── Scenario 15: exportHistory surfaces a device RSP_ERROR ────────────────

async function exportHistoryDeviceError() {
  const { port, push } = makeFakePort(() => {
    const msg = new TextEncoder().encode("history unavailable");
    const payload = new Uint8Array(2 + msg.length);
    payload[0] = 5;
    payload[1] = msg.length;
    payload.set(msg, 2);
    setTimeout(() => push(encodeFrame(FRAME_RSP_ERROR, payload)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await assert.rejects(
    () => session.exportHistory(),
    (err) => {
      assert.ok(err instanceof DeviceError);
      assert.equal(err.errorCode, 5);
      assert.match(err.message, /history unavailable/);
      return true;
    }
  );

  await session.disconnect();
}

// ── Scenario 16: clearHistory sends FRAME_CLEAR_HISTORY, expects RSP_OK ───

async function clearHistorySendsCorrectFrame() {
  const written = [];
  const { port, push } = makeFakePort((chunk) => {
    written.push(chunk);
    setTimeout(() => push(encodeFrame(FRAME_RSP_OK)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await session.clearHistory();

  assert.equal(written.length, 1);
  const sent = decodeFrame(written[0]);
  assert.equal(sent.frameType, FRAME_CLEAR_HISTORY);
  assert.equal(sent.payload.length, 0);

  await session.disconnect();
}

// ── Scenario 17: queryAdvert() sends FRAME_QUERY_ADVERT carrying the
//    browser's real wall-clock unix time, and decodes the returned card ───

async function queryAdvertSendsCorrectFrameAndDecodesTheCard() {
  const cardPayload = buildAdvertCardPayload("Cadet");
  assert.ok(
    cardPayload.length > MAX_RSP_HISTORY_ENTRY_PAYLOAD,
    "fixture card must exceed the legacy (pre-fix) plen guard to exercise it"
  );
  const written = [];
  const { port, push } = makeFakePort((chunk) => {
    written.push(chunk);
    setTimeout(() => push(encodeFrame(FRAME_RSP_ADVERT, cardPayload)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const before = Math.floor(Date.now() / 1000);
  const card = await session.queryAdvert();
  assert.deepEqual(Array.from(card), Array.from(cardPayload));

  assert.equal(written.length, 1);
  const sent = decodeFrame(written[0]);
  assert.equal(sent.frameType, FRAME_QUERY_ADVERT);
  assert.equal(sent.payload.length, 4);
  const sentUnixTime = new DataView(sent.payload.buffer, sent.payload.byteOffset, 4).getUint32(0, true);
  const after = Math.floor(Date.now() / 1000);
  assert.ok(
    sentUnixTime >= before && sentUnixTime <= after,
    `QUERY_ADVERT payload must carry the browser's real wall-clock unix time (sent ${sentUnixTime}, expected [${before}, ${after}])`
  );

  await session.disconnect();
}

// ── Scenario 18: queryAdvert() surfaces a device RSP_ERROR as DeviceError ─

async function queryAdvertDeviceError() {
  const { port, push } = makeFakePort(() => {
    const msg = new TextEncoder().encode("no identity yet");
    const payload = new Uint8Array(2 + msg.length);
    payload[0] = 4;
    payload[1] = msg.length;
    payload.set(msg, 2);
    setTimeout(() => push(encodeFrame(FRAME_RSP_ERROR, payload)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await assert.rejects(
    () => session.queryAdvert(),
    (err) => {
      assert.ok(err instanceof DeviceError, `expected DeviceError, got ${err}`);
      assert.equal(err.errorCode, 4);
      assert.match(err.message, /no identity yet/);
      return true;
    }
  );

  await session.disconnect();
}

// ── Scenario 19: regression guard — an oversized (>73 B) RSP_ADVERT frame
//    must survive log-noise resync, not be misclassified as false PROV_MAGIC
//    noise ─────────────────────────────────────────────────────────────────
//
// Before this mission, `#tryExtractFrame`'s plen guard was hardcoded to
// `MAX_RSP_HISTORY_ENTRY_PAYLOAD` (73 B) — the largest payload that existed
// prior to FRAME_RSP_ADVERT. A genuine card frame (up to 134 B) would trip
// "plen too large -> treat as false magic -> advance 1 byte", shredding the
// real frame one byte at a time until the read timed out. This uses a
// deliberately long name so the fixture card lands well past 73 B, and
// interleaves ESP-IDF-style log noise ahead of it exactly like a real T-Deck
// would, mirroring `happyPathWithLogNoiseResync` above but for RSP_ADVERT.

async function oversizedAdvertFrameSurvivesLogNoiseResync() {
  const cardPayload = buildAdvertCardPayload("Field Cadet Twelve");
  assert.ok(cardPayload.length > MAX_RSP_HISTORY_ENTRY_PAYLOAD, "fixture card must exceed the legacy plen guard");
  const advertFrame = encodeFrame(FRAME_RSP_ADVERT, cardPayload);

  const { port, push } = makeFakePort(() => {});
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const logNoise = new TextEncoder().encode("I (5678) prov_server: building self-advert card\n");
  const queryPromise = session.queryAdvert();
  push(concat(logNoise, advertFrame));

  const card = await queryPromise;
  assert.deepEqual(Array.from(card), Array.from(cardPayload));

  await session.disconnect();
}

// ── Scenario 20: regression guard — a stray leftover frame ahead of
//    QUERY_ADVERT's real answer must not cascade one-command-behind through
//    contact and channel enumeration ─────────────────────────────────────
//
// Reproduces the exact field defect (meshcadet-provisioner-advert-frame-
// desync-regression, filed against a real device post PR #54): a leftover
// RSP_STATUS arrives ahead of QUERY_ADVERT's real RSP_ADVERT reply (e.g. a
// duplicate reply to an earlier retried QUERY_STATUS, still trickling in
// over USB — see `#sendRecvWithRetry`'s CROSS-COMMAND RESIDUE GUARD doc
// comment for why this can happen on real hardware but never in the
// existing `staleRetryDuplicateDoesNotDesyncNextCommand` scenario, which
// only covers residue that has FULLY arrived before the next command
// starts).
//
// Before the fix, `queryAdvert()` read that leftover RSP_STATUS as if it
// were its own answer and threw `unexpected response 0x82 to QUERY_ADVERT
// (expected RSP_ADVERT 0x8A)` — verbatim the field HIL report — while
// the device's real (correct, just slow — advert-card signing + an NVS
// write) RSP_ADVERT reply was left orphaned in the buffer. That orphaned
// reply then became `listContacts()`'s own stray ("unexpected frame 0x8A
// during contact enumeration"), whose own real RSP_CONTACT reply in turn
// became `listChannels()`'s stray ("unexpected frame 0x86 during channel
// enumeration") — the reported one-command-behind cascade, precisely
// because nothing IN QUERY_ADVERT's read waited past the first wrong frame
// for its own real answer.
//
// After the fix, `#recvUntilExpected` discards the stray in place — inside
// the read for the command it arrived on — so the real answer to each
// command is consumed by that SAME command, and the whole chain
// (status -> advert -> contacts -> channels) succeeds cleanly, exactly as
// it must on real hardware.

async function advertResidueDoesNotCascadeThroughContactsAndChannels() {
  const cardPayload = buildAdvertCardPayload("Cadet");
  const advertFrame = encodeFrame(FRAME_RSP_ADVERT, cardPayload);
  const contactFrame = encodeFrame(FRAME_RSP_CONTACT, buildContactPayload(0, 0xaa, true, "Alice"));
  const contactsDoneFrame = encodeFrame(FRAME_RSP_CONTACTS_DONE, new Uint8Array(0));
  const channelsDoneFrame = encodeFrame(FRAME_RSP_CHANNELS_DONE, new Uint8Array(0));

  const { port, push } = makeFakePort((chunk) => {
    const { frameType } = decodeFrame(chunk);
    if (frameType === FRAME_QUERY_STATUS) {
      setTimeout(() => {
        push(statusFrame);
        push(identityFrame);
      }, 5);
    } else if (frameType === FRAME_QUERY_ADVERT) {
      // A leftover RSP_STATUS lands almost immediately (residue already in
      // flight); the REAL RSP_ADVERT — slower, mirroring the device's
      // signing + NVS write — follows well after.
      setTimeout(() => push(statusFrame), 5);
      setTimeout(() => push(advertFrame), 40);
    } else if (frameType === FRAME_QUERY_CONTACTS) {
      setTimeout(() => push(contactFrame), 5);
      setTimeout(() => push(contactsDoneFrame), 40);
    } else if (frameType === FRAME_QUERY_CHANNELS) {
      setTimeout(() => push(channelsDoneFrame), 5);
    }
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const { status, identity } = await session.queryStatus();
  assertStatusAndIdentity(status, identity);

  // Pre-fix, this rejects with "unexpected response 0x82 to QUERY_ADVERT
  // (expected RSP_ADVERT 0x8A)" — the leftover RSP_STATUS above.
  const card = await session.queryAdvert();
  assert.deepEqual(Array.from(card), Array.from(cardPayload));

  // Pre-fix, this rejects with "unexpected frame 0x8A during contact
  // enumeration" — the RSP_ADVERT that queryAdvert() never waited for.
  const contacts = await session.listContacts();
  assert.equal(contacts.length, 1);
  assert.equal(contacts[0].display_name, "Alice");

  // Pre-fix, this rejects with "unexpected frame 0x86 during channel
  // enumeration" — the RSP_CONTACT that listContacts() never waited for.
  const channels = await session.listChannels();
  assert.deepEqual(channels, []);

  await session.disconnect();
}

// ── Scenario 21: a genuinely unrecognized frame type is NOT tolerated as
//    stray residue — it still surfaces immediately ───────────────────────
//
// `#recvUntilExpected`'s stray tolerance (Scenario 20 above) only applies to
// *recognized* provisioning response types (`ALL_RSP_FRAME_TYPES`) — this
// guards the other half of that invariant: a byte that decodes to a valid
// frame (correct magic/CRC) but an unknown frame-type value is genuine
// protocol corruption or a version mismatch, not late residue, and must
// still fail fast rather than being silently absorbed.

async function unrecognizedFrameTypeIsNotToleratedAsStray() {
  const UNKNOWN_FRAME_TYPE = 0x99; // not in ALL_RSP_FRAME_TYPES
  const { port, push } = makeFakePort((chunk) => {
    const { frameType } = decodeFrame(chunk);
    if (frameType === FRAME_QUERY_ADVERT) {
      setTimeout(() => push(encodeFrame(UNKNOWN_FRAME_TYPE, new Uint8Array(0))), 5);
    }
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await assert.rejects(() => session.queryAdvert(), /unexpected frame 0x99/);

  await session.disconnect();
}

// ── Scenario 21b: a stray RSP_LOCK is tolerated like every other recognized
//    response type (ALL_RSP_FRAME_TYPES completeness regression) ─────────
//
// `FRAME_RSP_LOCK` (0x8D) was missing from `ALL_RSP_FRAME_TYPES` — found by
// inspection while diagnosing `meshcadet-web-provisioner-read-timeout-
// after-reset`, not from a live symptom (this module has no `queryLock()`
// caller yet, so a real session can't produce this today). Guards the
// invariant `ALL_RSP_FRAME_TYPES` exists for: every `FRAME_RSP_*` codec.js
// defines must be tolerated as late residue here, not just the ones this
// module currently happens to send queries for — the same gap, for a
// different frame type, that `unrecognizedFrameTypeIsNotToleratedAsStray`
// (Scenario 21) guards the *other* half of.

async function strayRspLockIsToleratedLikeAnyOtherRecognizedType() {
  const { port, push } = makeFakePort((chunk) => {
    const { frameType } = decodeFrame(chunk);
    if (frameType === FRAME_QUERY_ADVERT) {
      // A leftover RSP_LOCK (e.g. a future queryLock() caller's late reply)
      // arrives ahead of this command's real answer.
      setTimeout(() => push(encodeFrame(FRAME_RSP_LOCK, new Uint8Array([0, 0, 0, 0]))), 5);
      setTimeout(() => push(encodeFrame(FRAME_RSP_ADVERT, buildAdvertCardPayload("Cadet"))), 10);
    }
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const card = await session.queryAdvert();
  assert.deepEqual(Array.from(card), Array.from(buildAdvertCardPayload("Cadet")));

  await session.disconnect();
}

// ── Scenario 22: stray-frame tolerance is bounded, not infinite ──────────
//
// A device wedged into replaying the same well-formed-but-wrong response
// forever (or a genuinely corrupted stream that happens to keep producing
// recognized frame types) must still surface as an error within bounded
// time — `MAX_STRAY_FRAMES` exists precisely so `#recvUntilExpected` cannot
// spin silently until the outer retry budget (10s) is exhausted one frame
// at a time. Uses a shortened per-attempt timeout so the bound (not the
// timeout) is what trips, keeping the test fast.

async function strayFrameToleranceIsBounded() {
  const { port, push } = makeFakePort((chunk) => {
    const { frameType } = decodeFrame(chunk);
    if (frameType === FRAME_QUERY_ADVERT) {
      // Flood well past MAX_STRAY_FRAMES (64) with a recognized-but-wrong
      // response type; the real RSP_ADVERT never comes.
      for (let i = 0; i < 70; i++) {
        push(statusFrame);
      }
    }
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  await assert.rejects(() => session.queryAdvert(), /too many stray frames/);

  await session.disconnect();
}

// ── Scenario 23: listRooms streams RSP_ROOM*N -> RSP_ROOMS_DONE ───────────

/** Wire layout: index(1) | pubkey(32) | sync_since(4 LE) | permissions(1) | out_path_len(1) | out_path(P) | name_len(1) | name(N). */
function buildRoomPayload(index, pubkeyByte, syncSince, permissions, outPath, name) {
  const nameBytes = new TextEncoder().encode(name);
  const buf = new Uint8Array(40 + outPath.length + nameBytes.length);
  const view = new DataView(buf.buffer);
  buf[0] = index;
  buf.fill(pubkeyByte, 1, 33);
  view.setUint32(33, syncSince, true);
  buf[37] = permissions;
  buf[38] = outPath.length;
  buf.set(outPath, 39);
  buf[39 + outPath.length] = nameBytes.length;
  buf.set(nameBytes, 40 + outPath.length);
  return buf;
}

async function listRoomsStreamsToDone() {
  const { port, push } = makeFakePort(() => {
    setTimeout(() => {
      push(encodeFrame(FRAME_RSP_ROOM, buildRoomPayload(0, 0xaa, 1_700_000_000, 2, new Uint8Array(0), "Lobby")));
      push(encodeFrame(FRAME_RSP_ROOMS_DONE, new Uint8Array(0)));
    }, 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const rooms = await session.listRooms();
  assert.equal(rooms.length, 1);
  assert.equal(rooms[0].index, 0);
  assert.equal(rooms[0].sync_since, 1_700_000_000);
  assert.equal(rooms[0].permissions, 2);
  assert.equal(rooms[0].name, "Lobby");
  // The guest password is never part of RSP_ROOM's payload (ADR-0002 §7) —
  // nothing to assert absent here beyond the shape decodeRspRoom already
  // enforces (codec.conformance.test.mjs pins that byte layout).

  await session.disconnect();
}

// ── Scenario 24: addRoom sends FRAME_ADD_ROOM with the correct payload ───

async function addRoomSendsCorrectFrame() {
  const written = [];
  const { port, push } = makeFakePort((chunk) => {
    written.push(chunk);
    setTimeout(() => push(encodeFrame(FRAME_RSP_OK)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const pubkey = new Uint8Array(32).fill(0xdd);
  await session.addRoom(pubkey, "hunter2", "Lobby");

  assert.equal(written.length, 1);
  const sent = decodeFrame(written[0]);
  assert.equal(sent.frameType, FRAME_ADD_ROOM);
  assert.deepEqual(Array.from(sent.payload), Array.from(encodeAddRoom(pubkey, "hunter2", "Lobby")));

  await session.disconnect();
}

// ── Scenario 25: addRoom's RSP_ERROR surfaces as DeviceError (and never
//    logs/embeds the guest password anywhere in that error) ──────────────

async function addRoomDeviceError() {
  const { port, push } = makeFakePort(() => {
    const msg = new TextEncoder().encode("room slot full");
    const payload = new Uint8Array(2 + msg.length);
    payload[0] = 9;
    payload[1] = msg.length;
    payload.set(msg, 2);
    setTimeout(() => push(encodeFrame(FRAME_RSP_ERROR, payload)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const guestPassword = "top-secret-guest-pw";
  await assert.rejects(
    () => session.addRoom(new Uint8Array(32).fill(0xee), guestPassword, "Annex"),
    (err) => {
      assert.ok(err instanceof DeviceError);
      assert.equal(err.errorCode, 9);
      assert.match(err.message, /room slot full/);
      assert.ok(!err.message.includes(guestPassword), "DeviceError message must never embed the guest password");
      return true;
    }
  );

  await session.disconnect();
}

// ── Scenario 26: delRoom sends FRAME_DEL_ROOM with the pubkey-only payload ─

async function delRoomSendsCorrectFrame() {
  const written = [];
  const { port, push } = makeFakePort((chunk) => {
    written.push(chunk);
    setTimeout(() => push(encodeFrame(FRAME_RSP_OK)), 5);
  });
  installFakeGlobals(port);

  const session = new ProvisionerSession();
  await session.connect();

  const pubkey = new Uint8Array(32).fill(0xff);
  await session.delRoom(pubkey);

  assert.equal(written.length, 1);
  const sent = decodeFrame(written[0]);
  assert.equal(sent.frameType, FRAME_DEL_ROOM);
  assert.deepEqual(Array.from(sent.payload), Array.from(pubkey));

  await session.disconnect();
}

// ── Run ────────────────────────────────────────────────────────────────────

const scenarios = [
  ["happy path: two-frame handshake + magic-resync past log noise", happyPathWithLogNoiseResync],
  [
    "connect() never calls setSignals() (device-wedge regression — see connect()'s doc comment)",
    connectNeverCallsSetSignals,
  ],
  ["a stalled writer.write() surfaces a distinct 'write stalled' error within bounded time (unbounded-await regression)", writeStallSurfacesDistinctErrorWithinBoundedTime],
  ["disconnect() does not hang when reader.cancel() stalls (unbounded-await regression)", disconnectDoesNotHangWhenReaderCancelNeverSettles],
  ["connect survives a device reset at open (boot banner across a retry)", connectSurvivesResetBootBannerBeforeFirstQueryStatus],
  ["a read-loop failure surfaces as a real rejection, never a silent stall", readLoopErrorSurfacesAsRealRejectionNotSilentStall],
  [
    "a repeating ESP32-S3 ROM boot banner reports a reboot count and accurate (host-side) recovery guidance on timeout",
    repeatedRebootBannerReportsCountAndActionableTimeoutMessage,
  ],
  [
    "a reboot banner split across two separate reads is still counted once (carry-over boundary regression)",
    rebootBannerSplitAcrossTwoReadsIsStillCounted,
  ],
  [
    "a write stall after an observed reboot reports the reboot count too, not just 'write stalled' (sendFrame-outside-try regression)",
    writeStallAfterObservedRebootReportsRebootContext,
  ],
  ["send_recv_with_retry retries a dropped first response", retryOnDroppedFirstResponse],
  ["a retry's stale duplicate reply does not desync a later command", staleRetryDuplicateDoesNotDesyncNextCommand],
  ["disconnect() before connect() is a no-op", disconnectWithoutConnect],
  ["device RSP_ERROR on QUERY_STATUS surfaces as DeviceError", deviceErrorOnQueryStatus],
  ["unexpected frame after RSP_STATUS surfaces a desync error", unexpectedFrameAfterStatus],
  ["listContacts streams RSP_CONTACT*N -> RSP_CONTACTS_DONE", listContactsStreamsToDone],
  ["listChannels streams RSP_CHANNEL*N -> RSP_CHANNELS_DONE", listChannelsStreamsToDone],
  ["addContact sends FRAME_ADD_CONTACT with the correct payload", addContactSendsCorrectFrame],
  ["a write command's RSP_ERROR surfaces as DeviceError", writeCommandDeviceError],
  ["commit() resolves on RSP_OK ahead of a simulated first-boot port close", commitResolvesBeforeSimulatedReboot],
  ["concurrent command calls are serialized, not interleaved", concurrentCommandsAreSerialized],
  ["setPin sends FRAME_SET_PIN with the encoded PIN payload", setPinSendsCorrectFrame],
  ["setLockPin sends FRAME_SET_LOCK_PIN with the encoded lock-PIN payload", setLockPinSendsCorrectFrame],
  ["setLockConfig sends FRAME_SET_LOCK_CONFIG with lock_flags + lock_timeout_s", setLockConfigSendsCorrectFrame],
  ["exportHistory streams RSP_HISTORY_ENTRY*N -> RSP_HISTORY_DONE (oldest-first)", exportHistoryStreamsToDone],
  ["exportHistory tolerates a bounded stray reply before the stream", exportHistoryToleratesStrayFrame],
  ["exportHistory surfaces a device RSP_ERROR as DeviceError", exportHistoryDeviceError],
  ["clearHistory sends FRAME_CLEAR_HISTORY and expects RSP_OK", clearHistorySendsCorrectFrame],
  ["queryAdvert sends FRAME_QUERY_ADVERT with the browser's real unix time and decodes the card", queryAdvertSendsCorrectFrameAndDecodesTheCard],
  ["queryAdvert surfaces a device RSP_ERROR as DeviceError", queryAdvertDeviceError],
  ["an oversized RSP_ADVERT frame survives log-noise resync (plen-guard regression)", oversizedAdvertFrameSurvivesLogNoiseResync],
  ["a stray leftover frame ahead of QUERY_ADVERT's reply does not cascade through contacts/channels (one-behind desync regression)", advertResidueDoesNotCascadeThroughContactsAndChannels],
  ["a genuinely unrecognized frame type is not tolerated as stray residue", unrecognizedFrameTypeIsNotToleratedAsStray],
  ["a stray RSP_LOCK is tolerated like any other recognized response type (ALL_RSP_FRAME_TYPES completeness regression)", strayRspLockIsToleratedLikeAnyOtherRecognizedType],
  ["stray-frame tolerance is bounded, not infinite", strayFrameToleranceIsBounded],
  ["listRooms streams RSP_ROOM*N -> RSP_ROOMS_DONE", listRoomsStreamsToDone],
  ["addRoom sends FRAME_ADD_ROOM with the correct payload", addRoomSendsCorrectFrame],
  ["addRoom's RSP_ERROR surfaces as DeviceError without ever embedding the guest password", addRoomDeviceError],
  ["delRoom sends FRAME_DEL_ROOM with the pubkey-only payload", delRoomSendsCorrectFrame],
];

for (const [name, fn] of scenarios) {
  await fn();
  console.log(`  ok — ${name}`);
}

console.log(`session.smoke: OK — ${scenarios.length} scenario(s) passed.`);
