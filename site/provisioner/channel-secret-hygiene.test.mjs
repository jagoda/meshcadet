// channel-secret-hygiene.test.mjs — executable regression coverage mirroring
// admin-pin-hygiene.test.mjs/guest-password-hygiene.test.mjs's structure, for
// the THIRD browser secret this page carries: the channel secret
// (`session.js`'s `addChannel`/`delChannel`, `codec.js`'s
// `encodeAddChannel`/`encodeDelChannel`). Same shape, same rationale,
// different secret: the channel secret crosses the USB serial link in the
// clear BY DESIGN (it's sent straight to the device that will use it to
// decrypt/encrypt the channel), but in the BROWSER it must never be
// persisted or leaked — no `localStorage`, no `sessionStorage`, no
// URL/query param, no `console.log` — and must be cleared from memory on
// every exit path (success, failure, AND disconnect), for BOTH the
// add-channel and del-channel forms (unlike the PIN/guest-password, this
// secret is re-entered on two separate forms — see provisioner.html's
// `#add-channel-secret`/`#del-channel-secret`).
//
// WHY THIS IS TWO KINDS OF CHECK, NOT ONE — see guest-password-hygiene.test.mjs's
// header for the full rationale; the short version:
//
// The code that actually CARRIES the secret (`session.js`'s
// `addChannel`/`delChannel`) is DOM-free and loadable under plain `node` —
// so Part 1 below drives it for real (a mocked Web Serial port, a real
// `ProvisionerSession`) with hostile globals installed for
// `localStorage`/`sessionStorage` (any touch at all throws) and a
// `console.*` spy, and asserts neither is ever touched and the secret's hex
// substring never appears in anything logged, across success and
// device-error exchanges for both `addChannel` and `delChannel`.
//
// The code that actually HANDLES the secret at the DOM layer
// (`provisioner.js`'s `handleAddChannel`/`handleDelChannel`/
// `clearFormStatuses`, `provisioner.html`'s `add-channel-secret`/
// `del-channel-secret` fields) is NOT loadable under plain `node` at all
// (same `https://esm.sh/...` top-level import blocker as
// guest-password-hygiene.test.mjs documents). Part 2 below instead asserts
// directly against the shipped SOURCE TEXT of
// `provisioner.js`/`provisioner.html`:
//   - `add-channel-secret`/`del-channel-secret` are `type="password"` with
//     `autocomplete="off"` on both the input and its enclosing form (no
//     autofill/autocomplete).
//   - Neither `provisioner.js` nor `provisioner.html` ever touches
//     `localStorage`/`sessionStorage` as an actual API call.
//   - Neither file ever manipulates the URL/history at all.
//   - `handleAddChannel`/`handleDelChannel`'s function bodies never pass
//     any of the secret-carrying identifiers to a `console.*` call.
//   - `handleAddChannel` clears `addChannelSecret.value` and
//     `handleDelChannel` clears `delChannelSecret.value` on EVERY exit path
//     (success via `form.reset()` and the catch branch), and
//     `clearFormStatuses` (the disconnect path) clears both too.
//
// Plain `node`, zero dependencies (no package.json). Run directly:
//
//   node site/provisioner/channel-secret-hygiene.test.mjs

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

import {
  encodeFrame,
  encodeAddChannel,
  encodeDelChannel,
  bytesToHex,
  FRAME_ADD_CHANNEL,
  FRAME_DEL_CHANNEL,
  FRAME_RSP_OK,
  FRAME_RSP_ERROR,
} from "./codec.js";
import { ProvisionerSession, DeviceError } from "./session.js";

let checks = 0;
function ok(cond, label) {
  assert.ok(cond, label);
  checks++;
}

// 32 bytes (256-bit) — deliberately not all the same byte, so a naive
// "was any byte logged" check couldn't accidentally pass.
const CHANNEL_SECRET_BYTES = Uint8Array.from({ length: 32 }, (_, i) => (i * 7 + 11) & 0xff);
const CHANNEL_SECRET_HEX = bytesToHex(CHANNEL_SECRET_BYTES); // the exact substring every Part 1 check greps for

// ── Part 1: session.js's addChannel/delChannel, driven for real, against hostile globals ──

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
        // Already closed — fine.
      }
    },
  };
  return { port, push: (bytes) => controller.enqueue(bytes) };
}

/**
 * Install `navigator`/`window` (same minimal shape session.smoke.test.mjs
 * uses), PLUS a `localStorage`/`sessionStorage` pair that throws on ANY
 * property access or assignment — not a spy that records calls, an actual
 * trap — so if `session.js`'s `addChannel`/`delChannel` path (or anything it
 * transitively imports) ever so much as reads `localStorage.foo`, the test
 * fails immediately with a thrown error rather than relying on us
 * remembering to assert "it wasn't called".
 */
function installHostileGlobals(port) {
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

  const storageTrap = (label) =>
    new Proxy(
      {},
      {
        get() {
          throw new Error(`channel-secret-hygiene: ${label} was touched (get) — must never be touched at all`);
        },
        set() {
          throw new Error(`channel-secret-hygiene: ${label} was touched (set) — must never be touched at all`);
        },
      }
    );
  Object.defineProperty(globalThis, "localStorage", {
    value: storageTrap("localStorage"),
    writable: true,
    configurable: true,
  });
  Object.defineProperty(globalThis, "sessionStorage", {
    value: storageTrap("sessionStorage"),
    writable: true,
    configurable: true,
  });
}

/** Wrap console.log/warn/error to capture every argument, restoring on manual restore(). */
function spyOnConsole() {
  const original = { log: console.log, warn: console.warn, error: console.error };
  const captured = [];
  for (const level of ["log", "warn", "error"]) {
    console[level] = (...args) => {
      captured.push(args);
    };
  }
  return {
    captured,
    restore() {
      console.log = original.log;
      console.warn = original.warn;
      console.error = original.error;
    },
  };
}

function assertSecretNeverCaptured(captured, label) {
  for (const args of captured) {
    for (const arg of args) {
      const rendered = typeof arg === "string" ? arg : safeStringify(arg);
      ok(!rendered.includes(CHANNEL_SECRET_HEX), `${label}: console output must never contain the channel secret (got: ${rendered})`);
    }
  }
}

function safeStringify(value) {
  try {
    return JSON.stringify(value, (_key, v) => (v instanceof Error ? { message: v.message, stack: v.stack } : v));
  } catch {
    return String(value);
  }
}

async function addChannelNeverTouchesStorageOrLogsTheSecret() {
  const written = [];
  const { port, push } = makeFakePort((chunk) => {
    written.push(chunk);
    setTimeout(() => push(encodeFrame(FRAME_RSP_OK)), 5);
  });
  installHostileGlobals(port);
  const spy = spyOnConsole();

  try {
    const session = new ProvisionerSession();
    await session.connect();

    await session.addChannel(CHANNEL_SECRET_BYTES, 32, true, "family");

    // The secret DOES, correctly, cross the wire in the clear (it's sent
    // straight to the device that will use it) — assert it's there on the
    // wire (the one place it's supposed to be), as a sanity check that this
    // test would actually catch a regression rather than trivially passing
    // because nothing ran.
    ok(written.length === 1, "exactly one ADD_CHANNEL frame written");
    const expectedPayload = encodeAddChannel(CHANNEL_SECRET_BYTES, 32, true, "family");
    ok(
      Buffer.from(written[0]).includes(Buffer.from(expectedPayload)),
      "the secret legitimately appears on the wire frame (sent directly to the device)"
    );

    await session.disconnect();
  } finally {
    spy.restore();
  }

  assertSecretNeverCaptured(spy.captured, "addChannel success path");
  ok(true, "localStorage/sessionStorage were never touched (the hostile Proxy trap above would have thrown otherwise)");
}

async function addChannelDeviceErrorNeverLeaksTheSecretEither() {
  const { port, push } = makeFakePort(() => {
    const msg = new TextEncoder().encode("channel slot full");
    const payload = new Uint8Array(2 + msg.length);
    payload[0] = 9;
    payload[1] = msg.length;
    payload.set(msg, 2);
    setTimeout(() => push(encodeFrame(FRAME_RSP_ERROR, payload)), 5);
  });
  installHostileGlobals(port);
  const spy = spyOnConsole();

  try {
    const session = new ProvisionerSession();
    await session.connect();

    await assert.rejects(
      () => session.addChannel(CHANNEL_SECRET_BYTES, 32, false, "annex"),
      (err) => err instanceof DeviceError
    );

    await session.disconnect();
  } finally {
    spy.restore();
  }

  assertSecretNeverCaptured(spy.captured, "addChannel device-error path");
  ok(true, "localStorage/sessionStorage were never touched on the error path either");
}

async function delChannelNeverTouchesStorageOrLogsTheSecret() {
  const written = [];
  const { port, push } = makeFakePort((chunk) => {
    written.push(chunk);
    setTimeout(() => push(encodeFrame(FRAME_RSP_OK)), 5);
  });
  installHostileGlobals(port);
  const spy = spyOnConsole();

  try {
    const session = new ProvisionerSession();
    await session.connect();

    await session.delChannel(CHANNEL_SECRET_BYTES);

    ok(written.length === 1, "exactly one DEL_CHANNEL frame written");
    const expectedPayload = encodeDelChannel(CHANNEL_SECRET_BYTES);
    ok(
      Buffer.from(written[0]).includes(Buffer.from(expectedPayload)),
      "the secret legitimately appears on the wire frame (sent directly to the device)"
    );

    await session.disconnect();
  } finally {
    spy.restore();
  }

  assertSecretNeverCaptured(spy.captured, "delChannel success path");
  ok(true, "localStorage/sessionStorage were never touched (the hostile Proxy trap above would have thrown otherwise)");
}

async function delChannelDeviceErrorNeverLeaksTheSecretEither() {
  const { port, push } = makeFakePort(() => {
    const msg = new TextEncoder().encode("secret not found");
    const payload = new Uint8Array(2 + msg.length);
    payload[0] = 9;
    payload[1] = msg.length;
    payload.set(msg, 2);
    setTimeout(() => push(encodeFrame(FRAME_RSP_ERROR, payload)), 5);
  });
  installHostileGlobals(port);
  const spy = spyOnConsole();

  try {
    const session = new ProvisionerSession();
    await session.connect();

    await assert.rejects(
      () => session.delChannel(CHANNEL_SECRET_BYTES),
      (err) => err instanceof DeviceError
    );

    await session.disconnect();
  } finally {
    spy.restore();
  }

  assertSecretNeverCaptured(spy.captured, "delChannel device-error path");
  ok(true, "localStorage/sessionStorage were never touched on the error path either");
}

// ── Part 2: static source-text assertions over provisioner.js/provisioner.html ──
//
// See guest-password-hygiene.test.mjs's header (and this file's own header)
// for why these are source-text assertions rather than a driven DOM test:
// provisioner.js cannot be loaded under plain node (top-level
// `https://esm.sh/...` import).

const here = path.dirname(fileURLToPath(import.meta.url));
const siteDir = path.resolve(here, "..");
const provisionerJs = readFileSync(path.join(siteDir, "provisioner.js"), "utf-8");
const provisionerHtml = readFileSync(path.join(siteDir, "provisioner.html"), "utf-8");

/** True if `source` contains an actual `identifier.`/`identifier[` API touch — not merely the word appearing inside prose/backticks. */
function usesStorageApi(source, identifier) {
  return new RegExp(`\\b${identifier}\\s*[.[]`).test(source);
}

ok(!usesStorageApi(provisionerJs, "localStorage"), "provisioner.js never calls the localStorage API");
ok(!usesStorageApi(provisionerJs, "sessionStorage"), "provisioner.js never calls the sessionStorage API");
ok(!usesStorageApi(provisionerHtml, "localStorage"), "provisioner.html never calls the localStorage API");
ok(!usesStorageApi(provisionerHtml, "sessionStorage"), "provisioner.html never calls the sessionStorage API");

// No URL/history manipulation anywhere on the page at all — categorically
// forecloses "no URL/query param" leakage for the channel secret (or any
// other secret this page touches).
for (const [label, source] of [
  ["provisioner.js", provisionerJs],
  ["provisioner.html", provisionerHtml],
]) {
  ok(!/location\s*\./.test(source), `${label} never assigns/reads window.location`);
  ok(!/URLSearchParams/.test(source), `${label} never builds a URLSearchParams`);
  ok(!/history\s*\.\s*(push|replace)State/.test(source), `${label} never manipulates history state`);
}

/** Extract a top-level `async function <name>(...) { ... }` body by brace-balance scanning — good enough for this file's own straight-line functions (no template-literal braces inside the extracted range). */
function extractFunctionBody(source, name) {
  const sigMatch = source.match(new RegExp(`(?:async\\s+)?function\\s+${name}\\s*\\([^)]*\\)\\s*\\{`));
  ok(sigMatch !== null, `found a "function ${name}(...)" declaration to extract`);
  const start = sigMatch.index + sigMatch[0].length;
  let depth = 1;
  let i = start;
  for (; i < source.length && depth > 0; i++) {
    if (source[i] === "{") depth++;
    else if (source[i] === "}") depth--;
  }
  return source.slice(start, i - 1);
}

const handleAddChannelBody = extractFunctionBody(provisionerJs, "handleAddChannel");
const handleDelChannelBody = extractFunctionBody(provisionerJs, "handleDelChannel");
const clearFormStatusesBody = extractFunctionBody(provisionerJs, "clearFormStatuses");

// No console.* call in handleAddChannel/handleDelChannel may reference a
// secret-carrying identifier — every console.* line in these functions must
// only ever reference the caught error / status strings, never the secret.
for (const [label, body, fieldIdent] of [
  ["handleAddChannel", handleAddChannelBody, "addChannelSecret"],
  ["handleDelChannel", handleDelChannelBody, "delChannelSecret"],
]) {
  const consoleCallLines = body.split("\n").filter((line) => /console\.(log|warn|error)\(/.test(line));
  for (const line of consoleCallLines) {
    ok(!/\bsecret\b/i.test(line), `${label}'s console call must not reference the secret: ${line.trim()}`);
    ok(!new RegExp(fieldIdent).test(line), `${label}'s console call must not reference the secret field: ${line.trim()}`);
  }
}

// The secret field is cleared on EVERY exit path from handleAddChannel/
// handleDelChannel (the success path via `form.reset()`, and the catch
// branch via an explicit `.value = ""`) — assert both.
{
  ok(/addChannelForm\.reset\(\)/.test(handleAddChannelBody), "handleAddChannel clears the form (and thus addChannelSecret.value) on success via form.reset()");
  ok(
    /addChannelSecret\.value\s*=\s*("|')("|')/.test(handleAddChannelBody),
    "handleAddChannel clears addChannelSecret.value on the catch (failure) path"
  );
}
{
  ok(/delChannelForm\.reset\(\)/.test(handleDelChannelBody), "handleDelChannel clears the form (and thus delChannelSecret.value) on success via form.reset()");
  ok(
    /delChannelSecret\.value\s*=\s*("|')("|')/.test(handleDelChannelBody),
    "handleDelChannel clears delChannelSecret.value on the catch (failure) path"
  );
}

// clearFormStatuses (the disconnect path) also scrubs both fields.
ok(/addChannelSecret\.value\s*=\s*("|')("|')/.test(clearFormStatusesBody), "clearFormStatuses clears addChannelSecret.value on disconnect");
ok(/delChannelSecret\.value\s*=\s*("|')("|')/.test(clearFormStatusesBody), "clearFormStatuses clears delChannelSecret.value on disconnect");

// ── HTML: type=password + autocomplete=off (no form autofill/autocomplete) ──

for (const [inputId, formId] of [
  ["add-channel-secret", "add-channel-form"],
  ["del-channel-secret", "del-channel-form"],
]) {
  const inputMatch = provisionerHtml.match(new RegExp(`<input[^>]*id="${inputId}"[^>]*>`));
  ok(inputMatch !== null, `found the ${inputId} <input> tag`);
  const tag = inputMatch[0];
  ok(/type="password"/.test(tag), `${inputId} input must be type="password": ${tag}`);
  ok(/autocomplete="off"/.test(tag), `${inputId} input must have autocomplete="off": ${tag}`);

  const formMatch = provisionerHtml.match(new RegExp(`<form[^>]*id="${formId}"[^>]*>`));
  ok(formMatch !== null, `found the ${formId} <form> tag`);
  ok(/autocomplete="off"/.test(formMatch[0]), `${formId} must have autocomplete="off": ${formMatch[0]}`);
}

// ── Run the Part 1 async scenarios ───────────────────────────────────────

await addChannelNeverTouchesStorageOrLogsTheSecret();
await addChannelDeviceErrorNeverLeaksTheSecretEither();
await delChannelNeverTouchesStorageOrLogsTheSecret();
await delChannelDeviceErrorNeverLeaksTheSecretEither();

console.log(`channel-secret-hygiene.test: OK — ${checks} check(s) passed.`);
