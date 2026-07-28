// guest-password-hygiene.test.mjs — executable regression coverage for this
// mission's binding scope requirement (meshcadet-room-web-provisioner,
// scope item 4): the room-server guest password crosses the USB serial link
// in the clear BY DESIGN (ADR-0002 §4 / ADR-0001 §4 — the cable is the
// authentication), but in the BROWSER it must never be persisted or leaked —
// no `localStorage`, no `sessionStorage`, no URL/query param, no
// `console.log`, no form autofill/autocomplete — and must be cleared from
// memory on disconnect.
//
// WHY THIS IS TWO KINDS OF CHECK, NOT ONE:
//
// The code that actually CARRIES the password (`session.js`'s `addRoom`) is
// DOM-free and loadable under plain `node` exactly like
// `session.smoke.test.mjs` — so Part 1 below drives it for real (a mocked
// Web Serial port, a real `ProvisionerSession`) with hostile globals
// installed for `localStorage`/`sessionStorage` (any touch at all throws)
// and a `console.*` spy, and asserts neither is ever touched and the
// password substring never appears in anything logged, across both a
// success and a device-error exchange.
//
// The code that actually HANDLES the password at the DOM layer
// (`provisioner.js`'s `handleAddRoom`/`clearFormStatuses`, `provisioner.html`'s
// `add-room-password` field) is NOT loadable under plain `node` at all:
// `provisioner.js` top-level-calls `document.getElementById(...)` to wire up
// its DOM refs (its QR library import is now a plain relative import of the
// vendored `./vendor/qrcode.js` — see that file's header — so it is no
// longer network-import-blocked, but the DOM dependency alone still rules
// out plain `node`), and contrast with why `contact-uri.js`/`validation.js`/
// `session.js` were pulled out of `provisioner.js` into DOM-free modules in
// the first place (site/README.md). Building a full fake-DOM harness just to
// load one file was judged not worth the maintenance cost for one mission's
// worth of coverage. Part 2
// below instead asserts directly against the shipped SOURCE TEXT of
// `provisioner.js`/`provisioner.html` — not a comment, an executable
// assertion that fails loudly if any of these invariants regress:
//   - `add-room-password` is `type="password"` with `autocomplete="off"` on
//     both the input and its enclosing form (no autofill/autocomplete).
//   - Neither `provisioner.js` nor `provisioner.html` ever touches
//     `localStorage`/`sessionStorage` as an actual API call (member access,
//     not merely a comment mentioning the words — see `usesStorageApi`).
//   - Neither file ever manipulates the URL/history at all
//     (`location.`/`URLSearchParams`/`history.push`/`history.replace`) — the
//     page has no such code path today, so the password (or anything else)
//     categorically cannot leak into one.
//   - `handleAddRoom`'s function body never passes any of the
//     password-carrying identifiers to a `console.*` call.
//   - `handleAddRoom` clears `addRoomPassword.value` on EVERY exit path
//     (success and catch), and `clearFormStatuses` (the disconnect path)
//     clears it too.
//
// Plain `node`, zero dependencies (no package.json), matching
// session.smoke.test.mjs's build-step-free posture. Run directly:
//
//   node site/provisioner/guest-password-hygiene.test.mjs

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { encodeFrame, encodeAddRoom, FRAME_ADD_ROOM, FRAME_RSP_OK, FRAME_RSP_ERROR } from "./codec.js";
import { ProvisionerSession, DeviceError } from "./session.js";

let checks = 0;
function ok(cond, label) {
  assert.ok(cond, label);
  checks++;
}

const GUEST_PASSWORD = "sh1bboleth-guest-pw"; // the exact secret string every Part 1 check greps for

// ── Part 1: session.js's addRoom, driven for real, against hostile globals ──

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
 * trap — so if `session.js`'s `addRoom` path (or anything it transitively
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
          throw new Error(`guest-password-hygiene: ${label} was touched (get) — must never be touched at all`);
        },
        set() {
          throw new Error(`guest-password-hygiene: ${label} was touched (set) — must never be touched at all`);
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

/** Wrap console.log/warn/error to capture every argument, restoring on `[Symbol.dispose]`-style manual restore(). */
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

function assertPasswordNeverCaptured(captured, label) {
  for (const args of captured) {
    for (const arg of args) {
      const rendered = typeof arg === "string" ? arg : safeStringify(arg);
      ok(!rendered.includes(GUEST_PASSWORD), `${label}: console output must never contain the guest password (got: ${rendered})`);
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

async function addRoomNeverTouchesStorageOrLogsThePassword() {
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

    const pubkey = new Uint8Array(32).fill(0x42);
    await session.addRoom(pubkey, GUEST_PASSWORD, "Lobby");

    // The password DOES, correctly, cross the wire in the clear (ADR-0002
    // §4/ADR-0001 §4) — assert it's there on the wire (the one place it's
    // supposed to be), as a sanity check that this test would actually catch
    // a regression rather than trivially passing because nothing ran.
    ok(written.length === 1, "exactly one ADD_ROOM frame written");
    const expectedPayload = encodeAddRoom(pubkey, GUEST_PASSWORD, "Lobby");
    ok(
      Buffer.from(written[0]).includes(Buffer.from(expectedPayload)),
      "the password legitimately appears on the wire frame (the cable is the authentication)"
    );

    await session.disconnect();
  } finally {
    spy.restore();
  }

  assertPasswordNeverCaptured(spy.captured, "addRoom success path");
  ok(true, "localStorage/sessionStorage were never touched (the hostile Proxy trap above would have thrown otherwise)");
}

async function addRoomDeviceErrorNeverLeaksThePasswordEither() {
  const { port, push } = makeFakePort(() => {
    const msg = new TextEncoder().encode("room slot full");
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
      () => session.addRoom(new Uint8Array(32).fill(0x99), GUEST_PASSWORD, "Annex"),
      (err) => err instanceof DeviceError
    );

    await session.disconnect();
  } finally {
    spy.restore();
  }

  assertPasswordNeverCaptured(spy.captured, "addRoom device-error path");
  ok(true, "localStorage/sessionStorage were never touched on the error path either");
}

// ── Part 2: static source-text assertions over provisioner.js/provisioner.html ──
//
// See this file's header for why these are source-text assertions rather
// than a driven DOM test: provisioner.js cannot be loaded under plain node
// (top-level `document.getElementById` DOM wiring).

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
// forecloses "no URL/query param" leakage for the guest password (or any
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

const handleAddRoomBody = extractFunctionBody(provisionerJs, "handleAddRoom");
const clearFormStatusesBody = extractFunctionBody(provisionerJs, "clearFormStatuses");

// No console.* call in handleAddRoom may reference a password-carrying
// identifier — every console.* line in this function must only ever
// reference the caught error / status strings, never the password.
{
  const consoleCallLines = handleAddRoomBody.split("\n").filter((line) => /console\.(log|warn|error)\(/.test(line));
  ok(consoleCallLines.length >= 0, "handleAddRoom may or may not log (either is fine)"); // documents intent; real assertion below
  for (const line of consoleCallLines) {
    ok(!/\bpassword\b/i.test(line), `handleAddRoom's console call must not reference the password: ${line.trim()}`);
    ok(!/addRoomPassword/.test(line), `handleAddRoom's console call must not reference the password field: ${line.trim()}`);
  }
}

// The password field is cleared on EVERY exit path from handleAddRoom
// (success AND the catch branch) — count occurrences of the clear
// statement; there must be at least 2 (one per path), matching what was
// actually written (see provisioner.js's handleAddRoom).
{
  const clears = handleAddRoomBody.match(/addRoomPassword\.value\s*=\s*("|')("|')/g) || [];
  ok(clears.length >= 2, `handleAddRoom clears addRoomPassword.value on at least 2 exit paths (found ${clears.length})`);
}

// clearFormStatuses (the disconnect path — see provisioner.js's
// window "pagehide"/handleDisconnect wiring) also scrubs it.
ok(/addRoomPassword\.value\s*=\s*("|')("|')/.test(clearFormStatusesBody), "clearFormStatuses clears addRoomPassword.value on disconnect");

// The room QR/URI builder used by this page (contact-uri.js's buildRoomUri)
// is never handed anything that looks like the password field/variable —
// static guard that a future edit doesn't thread the secret into the one
// public-facing string this page renders and displays as a QR code.
ok(
  !/buildRoomUri\([^)]*password[^)]*\)/i.test(provisionerJs),
  "buildRoomUri is never called with a password-named argument"
);

// ── HTML: type=password + autocomplete=off (no form autofill/autocomplete) ──

{
  const inputMatch = provisionerHtml.match(/<input[^>]*id="add-room-password"[^>]*>/);
  ok(inputMatch !== null, "found the add-room-password <input> tag");
  const tag = inputMatch[0];
  ok(/type="password"/.test(tag), `add-room-password input must be type="password": ${tag}`);
  ok(/autocomplete="off"/.test(tag), `add-room-password input must have autocomplete="off": ${tag}`);
}

{
  const formMatch = provisionerHtml.match(/<form[^>]*id="add-room-form"[^>]*>/);
  ok(formMatch !== null, "found the add-room-form <form> tag");
  ok(/autocomplete="off"/.test(formMatch[0]), `add-room-form must have autocomplete="off": ${formMatch[0]}`);
}

// ── Run the Part 1 async scenarios ───────────────────────────────────────

await addRoomNeverTouchesStorageOrLogsThePassword();
await addRoomDeviceErrorNeverLeaksThePasswordEither();

console.log(`guest-password-hygiene.test: OK — ${checks} check(s) passed.`);
