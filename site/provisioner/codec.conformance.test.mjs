#!/usr/bin/env node
// codec.conformance.test.mjs — asserts codec.js reproduces every golden
// vector emitted by the real Rust codec, byte-for-byte / field-for-field.
//
// This is the CI guard `docs/adr/0007-provisioner-codec.md` promises: codec.js
// is a hand port of `protocol::provisioning` (see that file's header), so
// nothing but this check catches wire-format drift between the two.
//
// Usage:
//   cargo run -q -p xtask --bin gen-prov-golden-vectors > /tmp/golden-vectors.json
//   node site/provisioner/codec.conformance.test.mjs /tmp/golden-vectors.json
//
// No dependencies beyond Node's own `node:assert`/`node:fs` — no build step,
// no package.json, matching site/README.md's "no build step, on purpose"
// convention. `.github/workflows/pages-check.yml` runs exactly the two
// commands above on every PR touching `site/provisioner/**` or
// `protocol/src/{provisioning,history}.rs`.

import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";

import * as codec from "./codec.js";

const vectorsPath = process.argv[2];
if (!vectorsPath) {
  console.error("usage: node codec.conformance.test.mjs <golden-vectors.json>");
  process.exit(2);
}

const vectors = JSON.parse(readFileSync(vectorsPath, "utf-8"));
assert.ok(Array.isArray(vectors) && vectors.length > 0, "golden-vectors.json must be a non-empty array");

let passed = 0;
const failures = [];

function check(name, fn) {
  try {
    fn();
    passed++;
  } catch (err) {
    failures.push({ name, err });
  }
}

// ── Local (non-golden-vector) sanity checks ──────────────────────────────────
//
// These don't need Rust-generated fixtures: CRC-16/ARC's known-answer value
// is a universal property of the algorithm (see docs/adr/0002-provisioning-
// wire-format.md), and find_magic_start's resync behavior is pure JS logic
// mirroring host/src/session.rs (not itself part of protocol::provisioning's
// public API, so it has no Rust golden vectors to derive from).

check("crc16 known-answer for \"123456789\" is 0xBB3D", () => {
  const crc = codec.crc16(new TextEncoder().encode("123456789"));
  assert.equal(crc, 0xbb3d);
});

check("findMagicStart skips ESP-IDF log-noise bytes", () => {
  const logNoise = new TextEncoder().encode("I (1234) boot: UNPROVISIONED\n");
  const frame = codec.encodeFrame(codec.FRAME_RSP_OK);
  const buf = new Uint8Array(logNoise.length + frame.length);
  buf.set(logNoise, 0);
  buf.set(frame, logNoise.length);
  assert.equal(codec.findMagicStart(buf), logNoise.length);
});

check("findMagicStart returns buf.length when no magic candidate exists", () => {
  const buf = new TextEncoder().encode("no magic bytes here at all");
  assert.equal(codec.findMagicStart(buf), buf.length);
});

check("findMagicStart skips a lone first-magic-byte false positive", () => {
  // 'M' (0x4D) followed by something other than 'C' (0x43) must NOT be
  // mistaken for PROV_MAGIC — mirrors host/src/session.rs's own comment on
  // this exact case.
  const buf = new Uint8Array([0x4d, 0x00, 0x4d, 0x43]);
  assert.equal(codec.findMagicStart(buf), 2);
});

check("decodeFrame rejects a truncated buffer", () => {
  assert.throws(() => codec.decodeFrame(new Uint8Array(4)), (err) => err instanceof codec.ProvError && err.kind === "TruncatedFrame");
});

check("decodeFrame rejects bad magic", () => {
  const frame = codec.encodeFrame(codec.FRAME_RSP_OK);
  frame[0] = 0xff;
  assert.throws(() => codec.decodeFrame(frame), (err) => err instanceof codec.ProvError && err.kind === "BadMagic");
});

check("decodeFrame rejects a CRC mismatch", () => {
  const frame = codec.encodeFrame(codec.FRAME_QUERY_STATUS);
  frame[frame.length - 1] ^= 0xff;
  assert.throws(() => codec.decodeFrame(frame), (err) => err instanceof codec.ProvError && err.kind === "CrcMismatch");
});

// ── Golden-vector conformance ─────────────────────────────────────────────────

const ENCODE_OPS = {
  query_status: () => new Uint8Array(0),
  query_contacts: () => new Uint8Array(0),
  query_channels: () => new Uint8Array(0),
  commit_provisioning: () => new Uint8Array(0),
  export_history: () => new Uint8Array(0),
  clear_history: () => new Uint8Array(0),
  add_contact: (p) => codec.encodeAddContact(codec.hexToBytes(p.pubkey), p.telemetry_enable, p.name),
  del_contact: (p) => codec.encodeDelContact(codec.hexToBytes(p.pubkey)),
  add_channel: (p) => codec.encodeAddChannel(codec.hexToBytes(p.secret), p.key_len, p.primary, p.name),
  del_channel: (p) => codec.encodeDelChannel(codec.hexToBytes(p.secret)),
  set_notif_defaults: (p) => codec.encodeSetNotifDefaults(p.visual, p.audible),
  set_pin: (p) => codec.encodeSetPin(p.pin),
  set_device_name: (p) => codec.encodeSetDeviceName(p.name),
};

const DECODE_OPS = {
  rsp_error: codec.decodeRspError,
  rsp_status: codec.decodeRspStatus,
  rsp_identity: codec.decodeRspIdentity,
  rsp_contact: codec.decodeRspContact,
  rsp_channel: codec.decodeRspChannel,
  rsp_history_entry: codec.decodeRspHistoryEntry,
};

// Fields that are raw byte arrays (`Uint8Array`) in codec.js's decoded
// objects but hex strings in the golden vector's `expect` — hex-encode
// before comparing.
function normalizeForCompare(decoded) {
  const out = {};
  for (const [key, value] of Object.entries(decoded)) {
    out[key] = value instanceof Uint8Array ? codec.bytesToHex(value) : value;
  }
  return out;
}

for (const vector of vectors) {
  if (vector.direction === "encode") {
    check(`encode: ${vector.name}`, () => {
      const buildPayload = ENCODE_OPS[vector.op];
      assert.ok(buildPayload, `no ENCODE_OPS entry for op "${vector.op}"`);
      const payload = buildPayload(vector.params);
      assert.equal(codec.bytesToHex(payload), vector.payload_hex, "payload_hex mismatch");
      const frame = codec.encodeFrame(vector.frame_type, payload);
      assert.equal(codec.bytesToHex(frame), vector.frame_hex, "frame_hex mismatch");
    });
    continue;
  }

  // direction === "decode"
  check(`decode: ${vector.name}`, () => {
    const frameBytes = codec.hexToBytes(vector.frame_hex);
    const { frameType, payload } = codec.decodeFrame(frameBytes);
    assert.equal(frameType, vector.frame_type, "frame_type mismatch");
    assert.equal(codec.bytesToHex(payload), vector.payload_hex, "payload_hex mismatch");

    if (vector.op === "frame_only") {
      assert.equal(payload.length, 0, "frame_only vector expected an empty payload");
      return;
    }

    const decodeFn = DECODE_OPS[vector.op];
    assert.ok(decodeFn, `no DECODE_OPS entry for op "${vector.op}"`);
    const decoded = decodeFn(payload);
    assert.ok(decoded !== null, `${vector.op} decode returned null for a well-formed golden payload`);
    assert.deepEqual(normalizeForCompare(decoded), vector.expect, "decoded fields mismatch");
  });
}

// ── Report ────────────────────────────────────────────────────────────────────

if (failures.length > 0) {
  console.error(`codec.conformance: ${failures.length}/${passed + failures.length} check(s) FAILED\n`);
  for (const { name, err } of failures) {
    console.error(`✗ ${name}`);
    console.error(`  ${err.message}\n`);
  }
  process.exit(1);
}

console.log(`codec.conformance: OK — ${passed} check(s) passed (${vectors.length} golden vectors).`);
