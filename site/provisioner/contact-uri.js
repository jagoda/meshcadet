// contact-uri.js — MeshCore companion contact-add URI construction.
//
// A byte-for-byte hand port of `url_encode` + the
// `meshcore://contact/add?...` construction in `host/src/main.rs`. Pulled
// out of provisioner.js (the DOM-touching UI glue) into its own
// DOM-free module so it's importable — and testable — in plain `node`
// without a browser (see contact-uri.test.mjs).
//
// No build step: plain ES module, loaded directly by the browser or by
// `node` for the test.

import { bytesToHex } from "./codec.js";

const NAME_ENCODER = new TextEncoder();

/**
 * Percent-encode a string for use as a URI query-component value (RFC 3986).
 * A byte-for-byte hand port of `url_encode` in `host/src/main.rs`: same
 * unreserved set (`A-Z a-z 0-9 - _ . ~`), same uppercase-hex escaping,
 * operating on the UTF-8 byte sequence (not JS UTF-16 code units).
 */
export function urlEncode(str) {
  let out = "";
  for (const b of NAME_ENCODER.encode(str)) {
    if (
      (b >= 0x41 && b <= 0x5a) || // A-Z
      (b >= 0x61 && b <= 0x7a) || // a-z
      (b >= 0x30 && b <= 0x39) || // 0-9
      b === 0x2d || // -
      b === 0x5f || // _
      b === 0x2e || // .
      b === 0x7e // ~
    ) {
      out += String.fromCharCode(b);
    } else {
      out += "%" + b.toString(16).toUpperCase().padStart(2, "0");
    }
  }
  return out;
}

/**
 * Build the `meshcore://contact/add?name=&public_key=&type=1` URI for a
 * decoded `RspIdentity` payload (`codec.js`'s `decodeRspIdentity`). Mirrors
 * `host/src/main.rs`'s contact-URI construction: falls back to
 * `MeshCadet-<hex pubkey[0]>` when the device has no persisted name, exactly
 * as the host CLI's `node_name` fallback does. `type=1` is hardcoded
 * (chat=1) — MeshCadet is always a chat node, matching main.rs.
 */
export function buildContactUri(identity) {
  const name =
    identity.device_name && identity.device_name.length > 0
      ? identity.device_name
      : `MeshCadet-${identity.pubkey[0].toString(16).toUpperCase().padStart(2, "0")}`;
  return `meshcore://contact/add?name=${urlEncode(name)}&public_key=${bytesToHex(identity.pubkey)}&type=1`;
}
