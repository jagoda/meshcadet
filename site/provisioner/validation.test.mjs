// validation.test.mjs — regression coverage for validation.js's
// `validatePubkeyHex`/`validateChannelSecretHex`/`validateDeviceName`.
//
// Plain `node`, zero dependencies (no package.json), matching
// contact-uri.test.mjs's build-step-free posture. Run directly:
//
//   node site/provisioner/validation.test.mjs

import assert from "node:assert/strict";
import { validatePubkeyHex, validateChannelSecretHex, validateDeviceName } from "./validation.js";

let checks = 0;
function ok(cond, label) {
  assert.ok(cond, label);
  checks++;
}
function eq(actual, expected, label) {
  assert.equal(actual, expected, label);
  checks++;
}

// ── validatePubkeyHex ────────────────────────────────────────────────────

{
  const hex = "ab".repeat(32); // 64 hex chars
  const result = validatePubkeyHex(hex);
  ok(result.ok, "64-hex-char pubkey is valid");
  eq(result.bytes.length, 32, "decodes to 32 bytes");
  eq(result.bytes[0], 0xab, "first byte decodes correctly");
}

{
  const result = validatePubkeyHex("  " + "ab".repeat(32) + "  ");
  ok(result.ok, "surrounding whitespace is trimmed");
}

{
  const result = validatePubkeyHex("ab".repeat(31)); // 62 chars
  ok(!result.ok, "too-short pubkey is rejected");
  ok(/64 hex characters/.test(result.error), `error names the expected length: ${result.error}`);
}

{
  const result = validatePubkeyHex("ab".repeat(33)); // 66 chars
  ok(!result.ok, "too-long pubkey is rejected");
}

{
  const result = validatePubkeyHex("zz".repeat(32));
  ok(!result.ok, "non-hex characters are rejected");
  ok(/hex characters only/.test(result.error), `error explains hex-only: ${result.error}`);
}

{
  const result = validatePubkeyHex("");
  ok(!result.ok, "empty pubkey is rejected");
}

// ── validateChannelSecretHex ─────────────────────────────────────────────

{
  const hex = "cd".repeat(16); // 32 hex chars -> 128-bit
  const result = validateChannelSecretHex(hex);
  ok(result.ok, "32-hex-char (128-bit) secret is valid");
  eq(result.keyLen, 16, "key_len is 16 for a 128-bit secret");
  eq(result.bytes.length, 32, "secret is zero-padded to 32 bytes on the wire");
  eq(result.bytes[15], 0xcd, "last real byte preserved");
  eq(result.bytes[16], 0, "byte 16 is zero-padded");
  eq(result.bytes[31], 0, "byte 31 is zero-padded");
}

{
  const hex = "ef".repeat(32); // 64 hex chars -> 256-bit
  const result = validateChannelSecretHex(hex);
  ok(result.ok, "64-hex-char (256-bit) secret is valid");
  eq(result.keyLen, 32, "key_len is 32 for a 256-bit secret");
  eq(result.bytes.length, 32, "secret is exactly 32 bytes");
  eq(result.bytes[31], 0xef, "last byte is significant (not padded)");
}

{
  const result = validateChannelSecretHex("ab".repeat(20)); // 40 hex chars: neither 32 nor 64
  ok(!result.ok, "an in-between length is rejected");
  ok(/32 hex characters.*64 hex characters/.test(result.error), `error names both accepted lengths: ${result.error}`);
}

{
  const result = validateChannelSecretHex("zz".repeat(16));
  ok(!result.ok, "non-hex secret is rejected");
}

{
  const result = validateChannelSecretHex("");
  ok(!result.ok, "empty secret is rejected");
}

// ── validateDeviceName ───────────────────────────────────────────────────

{
  const result = validateDeviceName("Alex's MeshCadet");
  ok(result.ok, "an ordinary name is valid");
  eq(result.name, "Alex's MeshCadet");
}

{
  const result = validateDeviceName("");
  ok(result.ok, "an empty name is valid (clears the stored name)");
}

{
  const result = validateDeviceName("a".repeat(32));
  ok(result.ok, "exactly 32 ASCII bytes is valid (the boundary)");
}

{
  const result = validateDeviceName("a".repeat(33));
  ok(!result.ok, "33 ASCII bytes exceeds MAX_NAME_LEN");
  ok(/32 bytes/.test(result.error), `error names the byte ceiling: ${result.error}`);
}

{
  // "é" is 2 UTF-8 bytes; 20 of them is 40 bytes, over the 32-byte ceiling
  // despite being only 20 *characters* — the check must count bytes.
  const result = validateDeviceName("é".repeat(20));
  ok(!result.ok, "multi-byte UTF-8 name is measured in bytes, not characters");
}

{
  // 16 "é" characters = 32 bytes exactly: still valid at the boundary.
  const result = validateDeviceName("é".repeat(16));
  ok(result.ok, "16 two-byte UTF-8 characters (32 bytes) is valid at the boundary");
}

console.log(`validation.test: OK — ${checks} check(s) passed.`);
