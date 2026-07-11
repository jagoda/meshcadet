// codec.js — pure-JS port of MeshCadet's USB-serial provisioning wire
// protocol (`protocol/src/provisioning.rs`, plus the `FRAME_RSP_HISTORY_ENTRY`
// payload codec in `protocol/src/history.rs`, and `find_magic_start` from
// `host/src/session.rs`).
//
// This is a deliberate hand port, NOT a WASM build of `protocol` (see
// docs/adr/0007-provisioner-codec.md — pure JS keeps the site's "no build
// step, on purpose" convention, site/README.md). A hand port has no compiler
// to catch wire-format drift, so `.github/workflows/pages-check.yml` runs
// `codec.conformance.test.mjs` against golden vectors generated straight from
// the real Rust codec (`xtask --bin gen-prov-golden-vectors`) on every PR
// that touches either side. If you change ANYTHING here, run that check
// locally (see the test file's header) before opening a PR.
//
// Field-naming convention: decoded objects use the SAME snake_case field
// names as the Rust payload structs (e.g. `gps_lat_e7`, `battery_raw_mv`)
// rather than idiomatic-JS camelCase, on purpose — it keeps this file a
// literal, line-searchable mirror of `protocol/src/provisioning.rs` for
// whoever has to eyeball a diff against it later.
//
// No build step: plain ES module, loaded directly by the browser or by
// `node` for the conformance test — no bundler, no TypeScript compile.

// ── Frame layout constants (mirrors provisioning.rs "Constants") ────────────

export const PROV_MAGIC = new Uint8Array([0x4d, 0x43]); // "MC"
export const FRAME_OVERHEAD = 7; // magic(2) + type(1) + len(2) + crc(2)
export const MAX_NAME_LEN = 32;
export const MAX_PIN_LEN = 16;
export const MAX_ERR_MSG_LEN = 64;
// From protocol::history — shared by FRAME_RSP_HISTORY_ENTRY's payload.
export const MAX_HISTORY_TEXT_LEN = 64;
export const MAX_RSP_HISTORY_ENTRY_PAYLOAD = 73;

// ── Frame-type constants (mirrors provisioning.rs "Frame type constants") ───

export const FRAME_QUERY_STATUS = 0x01;
export const FRAME_QUERY_CONTACTS = 0x02;
export const FRAME_QUERY_CHANNELS = 0x03;

export const FRAME_ADD_CONTACT = 0x10;
export const FRAME_DEL_CONTACT = 0x11;

export const FRAME_ADD_CHANNEL = 0x20;
export const FRAME_DEL_CHANNEL = 0x21;

export const FRAME_SET_NOTIF_DEFAULTS = 0x40;

export const FRAME_SET_PIN = 0x50;
export const FRAME_SET_DEVICE_NAME = 0x51;

export const FRAME_COMMIT_PROVISIONING = 0x70;
export const FRAME_EXPORT_HISTORY = 0x71;
export const FRAME_CLEAR_HISTORY = 0x72;

export const FRAME_RSP_OK = 0x80;
export const FRAME_RSP_ERROR = 0x81;
export const FRAME_RSP_STATUS = 0x82;
export const FRAME_RSP_IDENTITY = 0x83;
export const FRAME_RSP_HISTORY_ENTRY = 0x84;
export const FRAME_RSP_HISTORY_DONE = 0x85;
export const FRAME_RSP_CONTACT = 0x86;
export const FRAME_RSP_CONTACTS_DONE = 0x87;
export const FRAME_RSP_CHANNEL = 0x88;
export const FRAME_RSP_CHANNELS_DONE = 0x89;

// `protocol::history::HistoryMsgType`.
export const HISTORY_MSG_TYPE_DM = 0;
export const HISTORY_MSG_TYPE_GRP_TXT = 1;

// ── Errors ────────────────────────────────────────────────────────────────────

/**
 * Mirrors `protocol::provisioning::ProvError`'s variants (as `.kind`):
 * `TruncatedFrame`, `BadMagic`, `CrcMismatch`, `TruncatedPayload`,
 * `NameTooLong`, `PinTooLong`.
 */
export class ProvError extends Error {
  constructor(kind) {
    super(kind);
    this.name = "ProvError";
    this.kind = kind;
  }
}

// ── Hex helpers ───────────────────────────────────────────────────────────────
//
// Not present in the Rust codec (which takes/returns raw byte arrays) but
// needed on the JS side: pubkeys/secrets arrive from `<input>` fields or
// `crypto.getRandomValues` as hex, and get displayed back as hex.

export function bytesToHex(bytes) {
  let out = "";
  for (let i = 0; i < bytes.length; i++) {
    out += bytes[i].toString(16).padStart(2, "0");
  }
  return out;
}

export function hexToBytes(hexStr) {
  const clean = hexStr.trim();
  if (clean.length % 2 !== 0) {
    throw new Error(`hexToBytes: odd-length hex string (${clean.length} chars)`);
  }
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    const byteHex = clean.slice(i * 2, i * 2 + 2);
    const value = Number.parseInt(byteHex, 16);
    if (Number.isNaN(value)) {
      throw new Error(`hexToBytes: invalid hex byte "${byteHex}"`);
    }
    out[i] = value;
  }
  return out;
}

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder("utf-8", { fatal: false });

function encodeUtf8(str) {
  return textEncoder.encode(str ?? "");
}

function decodeUtf8(bytes) {
  return textDecoder.decode(bytes);
}

// ── CRC-16/ARC (mirrors provisioning.rs's private `crc16`) ──────────────────
//
// Polynomial 0x8005, reflected; init 0x0000; no final XOR. Known-answer:
// crc16(utf8("123456789")) === 0xBB3D (see docs/adr/0002-provisioning-wire-
// format.md and this file's own conformance test).

export function crc16(bytes) {
  let crc = 0;
  for (let i = 0; i < bytes.length; i++) {
    crc ^= bytes[i];
    for (let bit = 0; bit < 8; bit++) {
      if (crc & 1) {
        crc = (crc >>> 1) ^ 0xa001;
      } else {
        crc = crc >>> 1;
      }
    }
  }
  return crc & 0xffff;
}

// ── Frame-level encode/decode (mirrors provisioning.rs's "Frame-level
//    encode / decode") ───────────────────────────────────────────────────────

/**
 * Encode a provisioning frame. `payload` defaults to an empty buffer for the
 * many frame types with no payload (QUERY_STATUS, COMMIT_PROVISIONING, ...).
 * Returns the full frame as a fresh `Uint8Array`.
 */
export function encodeFrame(frameType, payload = new Uint8Array(0)) {
  const plen = payload.length;
  const out = new Uint8Array(FRAME_OVERHEAD + plen);
  out[0] = PROV_MAGIC[0];
  out[1] = PROV_MAGIC[1];
  out[2] = frameType;
  out[3] = plen & 0xff;
  out[4] = (plen >>> 8) & 0xff;
  out.set(payload, 5);
  const crc = crc16(out.subarray(0, 5 + plen));
  out[5 + plen] = crc & 0xff;
  out[5 + plen + 1] = (crc >>> 8) & 0xff;
  return out;
}

/**
 * Decode a provisioning frame from `buf` (a `Uint8Array`).
 *
 * Returns `{ frameType, payload }` where `payload` is a zero-copy subarray
 * of `buf`. Throws `ProvError` on `BadMagic` / `TruncatedFrame` /
 * `CrcMismatch`, exactly mirroring `decode_frame`'s error precedence.
 */
export function decodeFrame(buf) {
  if (buf.length < FRAME_OVERHEAD) {
    throw new ProvError("TruncatedFrame");
  }
  if (buf[0] !== PROV_MAGIC[0] || buf[1] !== PROV_MAGIC[1]) {
    throw new ProvError("BadMagic");
  }
  const frameType = buf[2];
  const plen = buf[3] | (buf[4] << 8);
  const total = FRAME_OVERHEAD + plen;
  if (buf.length < total) {
    throw new ProvError("TruncatedFrame");
  }
  const crcExpected = buf[5 + plen] | (buf[5 + plen + 1] << 8);
  const crcActual = crc16(buf.subarray(0, 5 + plen));
  if (crcActual !== crcExpected) {
    throw new ProvError("CrcMismatch");
  }
  return { frameType, payload: buf.subarray(5, 5 + plen) };
}

// ── Frame synchronisation (mirrors `find_magic_start` in
//    `host/src/session.rs` / `firmware/src/provisioning_server.rs`) ─────────

/**
 * Return the index of the first byte in `buf` that could be the start of a
 * `PROV_MAGIC` sequence, or `buf.length` if no candidate is found. Used to
 * resync past ESP-IDF log-noise bytes interleaved with binary frames on the
 * same USB-serial stream, exactly as the host CLI does.
 */
export function findMagicStart(buf) {
  const m0 = PROV_MAGIC[0];
  const m1 = PROV_MAGIC[1];
  for (let i = 0; i < buf.length; i++) {
    if (buf[i] === m0) {
      if (i + 1 < buf.length) {
        if (buf[i + 1] === m1) {
          return i;
        }
        // m0 not followed by m1 — keep scanning.
      } else {
        // m0 at the end of the buffer — can't confirm or deny yet; preserve
        // it for the next recv iteration.
        return i;
      }
    }
  }
  return buf.length; // No magic candidate found — discard the entire buffer.
}

// ── Payload encode functions (host -> device commands) ──────────────────────
//
// Mirrors provisioning.rs's "Payload encode functions": every encoder
// silently truncates an over-length name/pin to its MAX_*_LEN (matching
// Rust's `.min(MAX_..._LEN)`) rather than throwing — truncation is a
// decode-time-detectable data-loss condition, not an encode-time error, in
// the Rust codec, so this mirrors that exactly.

/** Wire layout: `pubkey(32) | telemetry_enable(1) | name_len(1) | name(name_len)` */
export function encodeAddContact(pubkey, telemetryEnable, name) {
  const nameBytes = encodeUtf8(name).subarray(0, MAX_NAME_LEN);
  const out = new Uint8Array(34 + nameBytes.length);
  out.set(pubkey.subarray(0, 32), 0);
  out[32] = telemetryEnable ? 1 : 0;
  out[33] = nameBytes.length;
  out.set(nameBytes, 34);
  return out;
}

/** Wire layout: `pubkey(32)` */
export function encodeDelContact(pubkey) {
  return pubkey.slice(0, 32);
}

/** Wire layout: `secret(32) | key_len(1) | primary(1) | name_len(1) | name(name_len)` */
export function encodeAddChannel(secret, keyLen, primary, name) {
  const nameBytes = encodeUtf8(name).subarray(0, MAX_NAME_LEN);
  const out = new Uint8Array(35 + nameBytes.length);
  out.set(secret.subarray(0, 32), 0);
  out[32] = keyLen;
  out[33] = primary ? 1 : 0;
  out[34] = nameBytes.length;
  out.set(nameBytes, 35);
  return out;
}

/** Wire layout: `secret(32)` */
export function encodeDelChannel(secret) {
  return secret.slice(0, 32);
}

/** Wire layout: `visual(1) | audible(1)` */
export function encodeSetNotifDefaults(visual, audible) {
  return new Uint8Array([visual ? 1 : 0, audible ? 1 : 0]);
}

/** Wire layout: `pin_len(1) | pin(pin_len)` */
export function encodeSetPin(pin) {
  const pinBytes = encodeUtf8(pin).subarray(0, MAX_PIN_LEN);
  const out = new Uint8Array(1 + pinBytes.length);
  out[0] = pinBytes.length;
  out.set(pinBytes, 1);
  return out;
}

/** Wire layout: `name_len(1) | name(name_len)` */
export function encodeSetDeviceName(name) {
  const nameBytes = encodeUtf8(name).subarray(0, MAX_NAME_LEN);
  const out = new Uint8Array(1 + nameBytes.length);
  out[0] = nameBytes.length;
  out.set(nameBytes, 1);
  return out;
}

// ── Payload decode functions (device -> host responses) ─────────────────────
//
// Mirrors provisioning.rs's "Payload decode functions" byte-for-byte,
// including field order and error precedence.

function requireLen(payload, min) {
  if (payload.length < min) {
    throw new ProvError("TruncatedPayload");
  }
}

/** Decode an `RspError` payload. Wire layout: `error_code(1) | msg_len(1) | msg(msg_len)` */
export function decodeRspError(payload) {
  requireLen(payload, 2);
  const errorCode = payload[0];
  const msgLen = payload[1];
  if (msgLen > MAX_ERR_MSG_LEN || payload.length < 2 + msgLen) {
    throw new ProvError("TruncatedPayload");
  }
  return {
    error_code: errorCode,
    msg: decodeUtf8(payload.subarray(2, 2 + msgLen)),
    msg_len: msgLen,
  };
}

/**
 * Decode an `RspStatus` payload.
 *
 * Accepts a legacy 55-byte payload (pre-`battery_raw_mv`) and a 57-byte
 * payload (pre-`battery_held_raw_mv`): each trailing field defaults to `0`
 * when absent, mirroring `decode_rsp_status`'s staged-rollout compatibility.
 */
export function decodeRspStatus(payload) {
  requireLen(payload, 55);
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  return {
    provisioned: payload[0] !== 0,
    pubkey: payload.slice(1, 33),
    contact_count: payload[33],
    channel_count: payload[34],
    gps_has_fix: payload[35] !== 0,
    gps_lat_e7: view.getInt32(36, true),
    gps_lon_e7: view.getInt32(40, true),
    gps_fix_age_secs: view.getUint32(44, true),
    gps_clock_synced: payload[48] !== 0,
    gps_clock_sync_age_secs: view.getUint32(49, true),
    battery_percent: payload[53],
    battery_charging: payload[54] !== 0,
    battery_raw_mv: payload.length >= 57 ? view.getUint16(55, true) : 0,
    battery_held_raw_mv: payload.length >= 59 ? view.getUint16(57, true) : 0,
  };
}

/** Decode an `RspIdentity` payload. Wire layout: `pubkey(32) | pub_hash(1) | name_len(1) | name(name_len)` */
export function decodeRspIdentity(payload) {
  requireLen(payload, 34);
  const nameLen = payload[33];
  if (nameLen > MAX_NAME_LEN) {
    throw new ProvError("NameTooLong");
  }
  requireLen(payload, 34 + nameLen);
  return {
    pubkey: payload.slice(0, 32),
    pub_hash: payload[32],
    device_name: decodeUtf8(payload.subarray(34, 34 + nameLen)),
    device_name_len: nameLen,
  };
}

/** Decode an `RspContact` payload. Wire layout: `index(1) | pubkey(32) | telemetry(1) | name_len(1) | name(name_len)` */
export function decodeRspContact(payload) {
  requireLen(payload, 35);
  const nameLen = payload[34];
  if (nameLen > MAX_NAME_LEN) {
    throw new ProvError("NameTooLong");
  }
  requireLen(payload, 35 + nameLen);
  return {
    index: payload[0],
    pubkey: payload.slice(1, 33),
    telemetry_enable: payload[33] !== 0,
    display_name: decodeUtf8(payload.subarray(35, 35 + nameLen)),
    display_name_len: nameLen,
  };
}

/** Decode an `RspChannel` payload. Wire layout: `index(1) | channel_hash(1) | key_len(1) | primary(1) | name_len(1) | name(name_len)` */
export function decodeRspChannel(payload) {
  requireLen(payload, 5);
  const nameLen = payload[4];
  if (nameLen > MAX_NAME_LEN) {
    throw new ProvError("NameTooLong");
  }
  requireLen(payload, 5 + nameLen);
  return {
    index: payload[0],
    channel_hash: payload[1],
    key_len: payload[2],
    primary: payload[3] !== 0,
    name: decodeUtf8(payload.subarray(5, 5 + nameLen)),
    name_len: nameLen,
  };
}

/**
 * Decode a `FRAME_RSP_HISTORY_ENTRY` payload (`protocol::history::
 * decode_rsp_history_entry`). Wire layout: `index(1) | sender_hash(1) |
 * msg_type(1) | timestamp(4 LE) | text_len(1) | is_ours(1) | text(text_len)`.
 *
 * Mirrors the Rust function's `Option` return (not `Result`/throw): returns
 * `null` on a truncated payload or an unrecognised `msg_type` byte instead
 * of raising — there is no `ProvError` variant for this codec, since it
 * lives in `protocol::history`, not `protocol::provisioning`.
 */
export function decodeRspHistoryEntry(payload) {
  if (payload.length < 9) {
    return null;
  }
  const index = payload[0];
  const senderHash = payload[1];
  const msgType = payload[2];
  if (msgType !== HISTORY_MSG_TYPE_DM && msgType !== HISTORY_MSG_TYPE_GRP_TXT) {
    return null;
  }
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const timestamp = view.getUint32(3, true);
  const textLen = payload[7];
  const isOurs = payload[8] !== 0;
  if (payload.length < 9 + textLen || textLen > MAX_HISTORY_TEXT_LEN) {
    return null;
  }
  return {
    index,
    sender_hash: senderHash,
    msg_type: msgType,
    timestamp,
    text: decodeUtf8(payload.subarray(9, 9 + textLen)),
    text_len: textLen,
    is_ours: isOurs,
  };
}
