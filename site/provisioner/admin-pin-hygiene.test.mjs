// admin-pin-hygiene.test.mjs — executable regression coverage mirroring
// guest-password-hygiene.test.mjs's structure, for the OTHER browser secret
// this page ever carries: the admin PIN (`session.js`'s `setPin`/
// `encodeSetPin`, ADR-0007). Same shape, same rationale, different secret:
// the PIN crosses the USB serial link in the clear BY DESIGN (ADR-0007 —
// physical USB possession is the auth factor for a PIN reset, no old PIN
// needed), but in the BROWSER it must never be persisted or leaked — no
// `localStorage`, no `sessionStorage`, no URL/query param, no
// `console.log` — and must be cleared from memory on every exit path
// (success, failure, AND disconnect).
//
// WHY THIS IS TWO KINDS OF CHECK, NOT ONE — see guest-password-hygiene.test.mjs's
// header for the full rationale; the short version:
//
// The code that actually CARRIES the PIN (`session.js`'s `setPin`) is
// DOM-free and loadable under plain `node` — so Part 1 below drives it for
// real (a mocked Web Serial port, a real `ProvisionerSession`) with hostile
// globals installed for `localStorage`/`sessionStorage` (any touch at all
// throws) and a `console.*` spy, and asserts neither is ever touched and the
// PIN substring never appears in anything logged, across both a success and
// a device-error exchange.
//
// The code that actually HANDLES the PIN at the DOM layer
// (`provisioner.js`'s `handleSetPin`/`clearFormStatuses`, `provisioner.html`'s
// `set-pin-input` field) is NOT loadable under plain `node` at all (same
// top-level `document.getElementById` DOM-wiring blocker as
// guest-password-hygiene.test.mjs documents). Part 2 below instead asserts
// directly against the shipped SOURCE TEXT of
// `provisioner.js`/`provisioner.html`:
//   - `set-pin-input` is `type="password"` with `autocomplete="off"` on both
//     the input and its enclosing form (no autofill/autocomplete).
//   - Neither `provisioner.js` nor `provisioner.html` ever touches
//     `localStorage`/`sessionStorage` as an actual API call.
//   - Neither file ever manipulates the URL/history at all.
//   - `handleSetPin`'s function body never passes any of the PIN-carrying
//     identifiers to a `console.*` call.
//   - `handleSetPin` clears `setPinInput.value` on EVERY exit path (success
//     and catch), and `clearFormStatuses` (the disconnect path) clears it
//     too.
//
// Plain `node`, zero dependencies (no package.json). Run directly:
//
//   node site/provisioner/admin-pin-hygiene.test.mjs

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { encodeFrame, encodeSetPin, FRAME_SET_PIN, FRAME_RSP_OK, FRAME_RSP_ERROR } from "./codec.js";
import { ProvisionerSession, DeviceError } from "./session.js";

let checks = 0;
function ok(cond, label) {
  assert.ok(cond, label);
  checks++;
}

const ADMIN_PIN = "sh1bboleth-admin-pin"; // the exact secret string every Part 1 check greps for (>16 bytes on purpose — see the truncation note below)

// ── Part 1: session.js's setPin, driven for real, against hostile globals ──

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
 * trap — so if `session.js`'s `setPin` path (or anything it transitively
 * imports) ever so much as reads `localStorage.foo`, the test fails
 * immediately with a thrown error rather than relying on us remembering to
 * assert "it wasn't called".
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
          throw new Error(`admin-pin-hygiene: ${label} was touched (get) — must never be touched at all`);
        },
        set() {
          throw new Error(`admin-pin-hygiene: ${label} was touched (set) — must never be touched at all`);
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

function assertPinNeverCaptured(captured, label) {
  for (const args of captured) {
    for (const arg of args) {
      const rendered = typeof arg === "string" ? arg : safeStringify(arg);
      ok(!rendered.includes(ADMIN_PIN), `${label}: console output must never contain the admin PIN (got: ${rendered})`);
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

async function setPinNeverTouchesStorageOrLogsThePin() {
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

    await session.setPin(ADMIN_PIN);

    // The PIN DOES, correctly, cross the wire in the clear (ADR-0007 —
    // physical USB possession is the auth factor) — assert it's there on the
    // wire (the one place it's supposed to be), as a sanity check that this
    // test would actually catch a regression rather than trivially passing
    // because nothing ran. `encodeSetPin` truncates to `MAX_PIN_LEN` (16)
    // bytes, so the expected payload (not the raw 21-byte string) is what
    // must appear.
    ok(written.length === 1, "exactly one SET_PIN frame written");
    const expectedPayload = encodeSetPin(ADMIN_PIN);
    ok(
      Buffer.from(written[0]).includes(Buffer.from(expectedPayload)),
      "the (possibly truncated) PIN legitimately appears on the wire frame (the cable is the authentication)"
    );

    await session.disconnect();
  } finally {
    spy.restore();
  }

  assertPinNeverCaptured(spy.captured, "setPin success path");
  ok(true, "localStorage/sessionStorage were never touched (the hostile Proxy trap above would have thrown otherwise)");
}

async function setPinDeviceErrorNeverLeaksThePinEither() {
  const { port, push } = makeFakePort(() => {
    const msg = new TextEncoder().encode("pin rejected");
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
      () => session.setPin(ADMIN_PIN),
      (err) => err instanceof DeviceError
    );

    await session.disconnect();
  } finally {
    spy.restore();
  }

  assertPinNeverCaptured(spy.captured, "setPin device-error path");
  ok(true, "localStorage/sessionStorage were never touched on the error path either");
}

// ── Part 2: static source-text assertions over provisioner.js/provisioner.html ──
//
// See guest-password-hygiene.test.mjs's header (and this file's own header)
// for why these are source-text assertions rather than a driven DOM test:
// provisioner.js cannot be loaded under plain node (top-level
// `document.getElementById` DOM wiring).

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
// forecloses "no URL/query param" leakage for the admin PIN (or any other
// secret this page touches).
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

const handleSetPinBody = extractFunctionBody(provisionerJs, "handleSetPin");
const clearFormStatusesBody = extractFunctionBody(provisionerJs, "clearFormStatuses");

// No console.* call in handleSetPin may reference a PIN-carrying identifier
// — every console.* line in this function must only ever reference the
// caught error / status strings, never the PIN.
{
  const consoleCallLines = handleSetPinBody.split("\n").filter((line) => /console\.(log|warn|error)\(/.test(line));
  ok(consoleCallLines.length >= 0, "handleSetPin may or may not log (either is fine)"); // documents intent; real assertion below
  for (const line of consoleCallLines) {
    ok(!/\bpin\b/i.test(line), `handleSetPin's console call must not reference the PIN: ${line.trim()}`);
    ok(!/setPinInput/.test(line), `handleSetPin's console call must not reference the PIN field: ${line.trim()}`);
  }
}

// The PIN field is cleared on EVERY exit path from handleSetPin (success AND
// the catch branch) — count occurrences of the clear statement; there must
// be at least 2 (one per path), matching what handleAddRoom's equivalent
// clear-on-every-path discipline already does (see guest-password-hygiene.test.mjs).
{
  const clears = handleSetPinBody.match(/setPinInput\.value\s*=\s*("|')("|')/g) || [];
  ok(clears.length >= 2, `handleSetPin clears setPinInput.value on at least 2 exit paths (found ${clears.length})`);
}

// clearFormStatuses (the disconnect path) also scrubs it.
ok(/setPinInput\.value\s*=\s*("|')("|')/.test(clearFormStatusesBody), "clearFormStatuses clears setPinInput.value on disconnect");

// ── HTML: type=password + autocomplete=off (no form autofill/autocomplete) ──

{
  const inputMatch = provisionerHtml.match(/<input[^>]*id="set-pin-input"[^>]*>/);
  ok(inputMatch !== null, "found the set-pin-input <input> tag");
  const tag = inputMatch[0];
  ok(/type="password"/.test(tag), `set-pin-input input must be type="password": ${tag}`);
  ok(/autocomplete="off"/.test(tag), `set-pin-input input must have autocomplete="off": ${tag}`);
}

{
  const formMatch = provisionerHtml.match(/<form[^>]*id="set-pin-form"[^>]*>/);
  ok(formMatch !== null, "found the set-pin-form <form> tag");
  ok(/autocomplete="off"/.test(formMatch[0]), `set-pin-form must have autocomplete="off": ${formMatch[0]}`);
}

// ── Run the Part 1 async scenarios ───────────────────────────────────────

await setPinNeverTouchesStorageOrLogsThePin();
await setPinDeviceErrorNeverLeaksThePinEither();

console.log(`admin-pin-hygiene.test: OK — ${checks} check(s) passed.`);
