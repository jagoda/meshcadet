// region.test.mjs — region-name validation + codec round-trip coverage.
//
// The wire bytes themselves are pinned against the real Rust codec by
// codec.conformance.test.mjs's golden vectors; this file covers the
// provisioner-side validation contract (accept/reject matrix, including the
// v1 private-region rejection and the clear path) under plain `node`.

import test from "node:test";
import assert from "node:assert/strict";

import {
  REGION_NAME_MAX,
  encodeSetRegion,
  decodeRspRegion,
  ProvError,
} from "./codec.js";
import { validateRegionName } from "./validation.js";

test("validateRegionName accepts public names, bare and '#'-prefixed", () => {
  assert.deepEqual(validateRegionName("seattle"), { ok: true, name: "seattle" });
  assert.deepEqual(validateRegionName("#seattle"), { ok: true, name: "#seattle" });
  assert.deepEqual(validateRegionName("  bay-area  "), { ok: true, name: "bay-area" });
  assert.deepEqual(validateRegionName("abc_DEF"), { ok: true, name: "abc_DEF" });
});

test("validateRegionName rejects an empty name (clear is explicit)", () => {
  const result = validateRegionName("");
  assert.equal(result.ok, false);
  assert.match(result.error, /required/);
});

test("validateRegionName rejects a leading '$' private region with a clear reason", () => {
  const result = validateRegionName("$private");
  assert.equal(result.ok, false);
  assert.match(result.error, /private/);
  assert.match(result.error, /not supported/);
});

test("validateRegionName rejects disallowed punctuation and whitespace", () => {
  for (const name of ["bad name", "zone.1", "a:b", "@home", "x/y"]) {
    const result = validateRegionName(name);
    assert.equal(result.ok, false, `${name} must be rejected`);
    assert.match(result.error, /invalid character/);
  }
});

test("validateRegionName enforces the 31-byte ceiling, not a character count", () => {
  assert.equal(validateRegionName("A".repeat(REGION_NAME_MAX)).ok, true);
  assert.equal(validateRegionName("A".repeat(REGION_NAME_MAX + 1)).ok, false);
  // Multi-byte UTF-8 bytes are >= 'A' upstream, but the byte ceiling still
  // applies: 11 x 3-byte characters = 33 bytes > 31.
  const result = validateRegionName("€".repeat(11));
  assert.equal(result.ok, false);
  assert.match(result.error, /31 bytes/);
});

test("encodeSetRegion emits name_len then the name verbatim", () => {
  assert.deepEqual(Array.from(encodeSetRegion("seattle")), [7, ...Buffer.from("seattle")]);
  assert.deepEqual(Array.from(encodeSetRegion("#bay-area")), [9, ...Buffer.from("#bay-area")]);
});

test("encodeSetRegion empty is the clear command", () => {
  assert.deepEqual(Array.from(encodeSetRegion("")), [0]);
});

test("decodeRspRegion round-trips a name and the empty/no-region state", () => {
  assert.deepEqual(decodeRspRegion(encodeSetRegion("seattle")), { name: "seattle", name_len: 7 });
  assert.deepEqual(decodeRspRegion(encodeSetRegion("")), { name: "", name_len: 0 });
});

test("decodeRspRegion rejects truncated, over-length, private, and malformed names", () => {
  assert.throws(
    () => decodeRspRegion(new Uint8Array([5, 0x61])),
    (err) => err instanceof ProvError && err.kind === "TruncatedPayload"
  );
  assert.throws(
    () => decodeRspRegion(new Uint8Array([REGION_NAME_MAX + 1])),
    (err) => err instanceof ProvError && err.kind === "RegionNameInvalid"
  );
  for (const bad of ["$private", "bad name"]) {
    const encoded = new Uint8Array(1 + bad.length);
    encoded[0] = bad.length;
    encoded.set(Buffer.from(bad), 1);
    assert.throws(
      () => decodeRspRegion(encoded),
      (err) => err instanceof ProvError && err.kind === "RegionNameInvalid"
    );
  }
});
