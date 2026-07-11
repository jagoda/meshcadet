// contact-uri.test.mjs — regression coverage for contact-uri.js's
// `urlEncode`/`buildContactUri`, checked against the SAME fixtures as
// host/src/main.rs's own `#[cfg(test)]` module (`url_encode_passes_unreserved`,
// `url_encode_escapes_space_and_reserved`, `url_encode_escapes_utf8_multibyte`,
// `identity_uri_builds_and_encodes_as_qr`) — this is what makes the "byte-for-
// byte hand port" claim in contact-uri.js's header a checked fact rather than
// just an assertion in a comment.
//
// Plain `node`, zero dependencies (no package.json), matching
// codec.conformance.test.mjs's build-step-free posture. Run directly:
//
//   node site/provisioner/contact-uri.test.mjs

import assert from "node:assert/strict";
import { urlEncode, buildContactUri } from "./contact-uri.js";

let checks = 0;
function eq(actual, expected, label) {
  assert.equal(actual, expected, label);
  checks++;
}
function ok(cond, label) {
  assert.ok(cond, label);
  checks++;
}

// Mirrors url_encode_passes_unreserved (host/src/main.rs).
eq(urlEncode("MeshCadet-AB_1.2~3"), "MeshCadet-AB_1.2~3", "unreserved set passes through unchanged");

// Mirrors url_encode_escapes_space_and_reserved (host/src/main.rs).
eq(urlEncode("Mom & Dad"), "Mom%20%26%20Dad", "space and & escaped");
eq(urlEncode("a=b#c"), "a%3Db%23c", "= and # escaped");

// Mirrors url_encode_escapes_utf8_multibyte (host/src/main.rs): "é" is
// U+00E9 -> UTF-8 0xC3 0xA9, encoded byte-by-byte (not code-point-by-code-
// point) to match Rust's `s.as_bytes()` iteration.
eq(urlEncode("é"), "%C3%A9", "UTF-8 multibyte escaped byte-for-byte");

// Mirrors identity_uri_builds_and_encodes_as_qr (host/src/main.rs).
{
  const pubkey = new Uint8Array(32).fill(0xab);
  const identity = { pubkey, device_name: "Mom & Dad's T-Deck" };
  const uri = buildContactUri(identity);
  ok(uri.startsWith("meshcore://contact/add?name="), "URI starts with the expected prefix");
  ok(
    uri.includes("&public_key=abababababababababababababababababababababababababababababababab"),
    "URI contains the full 64-hex-char pubkey"
  );
  ok(uri.endsWith("&type=1"), "URI ends with the chat-node type");
  ok(!uri.includes(" "), "URI must not contain raw spaces");
}

// device_name fallback: an empty/absent persisted name falls back to
// "MeshCadet-<hex pubkey[0]>", mirroring main.rs's `node_name` fallback.
{
  const pubkey = new Uint8Array(32);
  pubkey[0] = 0x0a;
  const uri = buildContactUri({ pubkey, device_name: "" });
  ok(uri.includes("name=MeshCadet-0A"), `expected fallback name in ${uri}`);
}

console.log(`contact-uri.test: OK — ${checks} check(s) passed.`);
