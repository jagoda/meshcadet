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
  FRAME_QUERY_STATUS,
  FRAME_RSP_STATUS,
  FRAME_RSP_IDENTITY,
} from "./codec.js";
import { ProvisionerSession } from "./session.js";

// ── Fake Web Serial port ─────────────────────────────────────────────────

/**
 * Build a fake `SerialPort`-shaped object backed by real `ReadableStream`/
 * `WritableStream` instances, so `session.js`'s `getReader()`/`getWriter()`
 * calls behave exactly as they would against a real port. `onWrite(chunk)`
 * is called synchronously for every frame the session writes (used to drive
 * scripted device responses and to assert what was sent); the returned
 * `push(bytes)` enqueues bytes as if the device sent them.
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
  const port = {
    readable,
    writable,
    async open() {},
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

// ── Scenario 3: disconnect() before connect() is a harmless no-op ────────

async function disconnectWithoutConnect() {
  const { port } = makeFakePort(() => {});
  installFakeGlobals(port);
  const session = new ProvisionerSession();
  await session.disconnect(); // must not throw
  assert.equal(session.isConnected, false);
}

// ── Run ────────────────────────────────────────────────────────────────────

const scenarios = [
  ["happy path: two-frame handshake + magic-resync past log noise", happyPathWithLogNoiseResync],
  ["send_recv_with_retry retries a dropped first response", retryOnDroppedFirstResponse],
  ["disconnect() before connect() is a no-op", disconnectWithoutConnect],
];

for (const [name, fn] of scenarios) {
  await fn();
  console.log(`  ok — ${name}`);
}

console.log(`session.smoke: OK — ${scenarios.length} scenario(s) passed.`);
