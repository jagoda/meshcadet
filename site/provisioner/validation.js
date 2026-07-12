// validation.js — input validation for the provisioner page's write forms
// (add-contact / del-contact pubkey, add-channel / del-channel secret,
// identity --set-name device name).
//
// DOM-free (like contact-uri.js) so it's testable under plain `node` via
// validation.test.mjs — no `document`/`navigator` top-level side effects.
//
// Byte-length mirrors host/src/main.rs's `parse_32bytes_hex` /
// `parse_channel_secret_hex` / the Cmd::Identity `set_name` length check,
// adapted into `{ ok, ... }` result objects instead of `anyhow::bail!` (this
// is a form-validation surface, not a CLI arg parser — callers render
// `.error` as inline copy next to the offending field rather than exiting
// the process).
//
// No build step: plain ES module, loaded directly by the browser or by
// `node` for the test.

import { hexToBytes, MAX_NAME_LEN } from "./codec.js";

const HEX_ONLY_RE = /^[0-9a-fA-F]*$/;

/**
 * Validate a contact Ed25519 public key entered as hex.
 * Mirrors `parse_32bytes_hex(s, "pubkey")` (host/src/main.rs): exactly 64
 * hex characters (32 bytes).
 *
 * Returns `{ ok: true, bytes: Uint8Array(32) }` or `{ ok: false, error }`.
 */
export function validatePubkeyHex(input) {
  const clean = (input ?? "").trim();
  if (clean.length === 0) {
    return { ok: false, error: "pubkey is required" };
  }
  if (!HEX_ONLY_RE.test(clean)) {
    return { ok: false, error: "pubkey must be hex characters only (0-9, a-f)" };
  }
  if (clean.length !== 64) {
    return { ok: false, error: `pubkey must be exactly 64 hex characters (32 bytes); got ${clean.length}` };
  }
  return { ok: true, bytes: hexToBytes(clean) };
}

/**
 * Validate a channel secret entered as hex.
 * Mirrors `parse_channel_secret_hex` (host/src/main.rs): accepts 32 hex
 * characters (16 bytes, 128-bit) or 64 hex characters (32 bytes, 256-bit).
 * A 128-bit secret is zero-padded into a 32-byte buffer — bytes `[16..32]`
 * are zero — exactly as the Rust parser does, so `encodeAddChannel`'s
 * `secret.subarray(0, 32)` sees the same bytes on both sides.
 *
 * Returns `{ ok: true, bytes: Uint8Array(32), keyLen: 16 | 32 }` or
 * `{ ok: false, error }`.
 */
export function validateChannelSecretHex(input) {
  const clean = (input ?? "").trim();
  if (clean.length === 0) {
    return { ok: false, error: "secret is required" };
  }
  if (!HEX_ONLY_RE.test(clean)) {
    return { ok: false, error: "secret must be hex characters only (0-9, a-f)" };
  }
  if (clean.length !== 32 && clean.length !== 64) {
    return {
      ok: false,
      error: `secret must be 32 hex characters (128-bit) or 64 hex characters (256-bit); got ${clean.length}`,
    };
  }
  const raw = hexToBytes(clean);
  if (raw.length === 16) {
    const padded = new Uint8Array(32);
    padded.set(raw, 0);
    return { ok: true, bytes: padded, keyLen: 16 };
  }
  return { ok: true, bytes: raw, keyLen: 32 };
}

/**
 * Validate a device display name.
 * Mirrors the `Cmd::Identity { set_name }` length check (host/src/main.rs):
 * at most `MAX_NAME_LEN` (32) bytes UTF-8 — note this is a BYTE length, not
 * a character count, so multi-byte UTF-8 names have a lower character
 * ceiling. An empty string is valid (clears the stored name).
 *
 * Returns `{ ok: true, name }` or `{ ok: false, error }`.
 */
export function validateDeviceName(input) {
  const name = input ?? "";
  const byteLen = new TextEncoder().encode(name).length;
  if (byteLen > MAX_NAME_LEN) {
    return { ok: false, error: `device name must be at most ${MAX_NAME_LEN} bytes (UTF-8); got ${byteLen} bytes` };
  }
  return { ok: true, name };
}
