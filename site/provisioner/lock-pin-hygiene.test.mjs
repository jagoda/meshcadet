// lock-pin-hygiene.test.mjs — executable regression coverage mirroring
// admin-pin-hygiene.test.mjs's structure, for the FOURTH browser secret this
// page carries: the screen-lock PIN (`session.js`'s `setLockPin`,
// `codec.js`'s `encodeSetLockPin`, docs/adr/0013-screen-lock-policy-layer.md).
// Same shape, same rationale, different secret, one load-bearing
// difference: unlike the admin PIN (any UTF-8 content, silently truncated
// to MAX_PIN_LEN by `encodeSetPin`), the lock PIN must be EXACTLY 4 ASCII
// digits — `encodeSetLockPin` THROWS on anything else rather than
// truncating, so the constant below is a valid 4-digit string, not an
// over-length one. The PIN crosses the USB serial link in the clear BY
// DESIGN (same posture as the admin PIN — physical USB possession is the
// auth factor for a reset, no old PIN needed), but in the BROWSER it must
// never be persisted or leaked — no `localStorage`, no `sessionStorage`, no
// URL/query param, no `console.log` — and must be cleared from memory on
// every exit path (success, failure, AND disconnect).
//
// WHY THIS IS TWO KINDS OF CHECK, NOT ONE — see guest-password-hygiene.test.mjs's
// header for the full rationale; the short version:
//
// The code that actually CARRIES the PIN (`session.js`'s `setLockPin`) is
// DOM-free and loadable under plain `node` — so Part 1 below drives it for
// real (a mocked Web Serial port, a real `ProvisionerSession`) with hostile
// globals installed for `localStorage`/`sessionStorage` (any touch at all
// throws) and a `console.*` spy, and asserts neither is ever touched and
// the PIN substring never appears in anything logged, across both a
// success and a device-error exchange.
//
// The code that actually HANDLES the PIN at the DOM layer
// (`provisioner.js`'s `handleSetLockPin`/`clearFormStatuses`,
// `provisioner.html`'s `lock-pin-input` field) is NOT loadable under plain
// `node` at all (same top-level `document.getElementById` DOM-wiring
// blocker as guest-password-hygiene.test.mjs documents). Part 2 below
// instead asserts directly against the shipped SOURCE TEXT of
// `provisioner.js`/`provisioner.html`:
//   - `lock-pin-input` is `type="password"` with `autocomplete="off"` on
//     both the input and its enclosing form (no autofill/autocomplete).
//   - Neither `provisioner.js` nor `provisioner.html` ever touches
//     `localStorage`/`sessionStorage` as an actual API call.
//   - Neither file ever manipulates the URL/history at all.
//   - `handleSetLockPin`'s function body never passes any of the
//     PIN-carrying identifiers to a `console.*` call.
//   - `handleSetLockPin` clears `lockPinInput.value` on EVERY exit path
//     (success and catch), and `clearFormStatuses` (the disconnect path)
//     clears it too.
//
// Plain `node`, zero dependencies (no package.json). Run directly:
//
//   node site/provisioner/lock-pin-hygiene.test.mjs

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { encodeFrame, encodeSetLockPin, FRAME_SET_LOCK_PIN, FRAME_RSP_OK, FRAME_RSP_ERROR } from "./codec.js";
import { ProvisionerSession, DeviceError } from "./session.js";
import {
  makeFakePort,
  installHostileGlobals,
  spyOnConsole,
  assertSecretNeverCaptured,
  usesStorageApi,
  extractFunctionBody,
  countValueClears,
} from "./secret-hygiene-test-helpers.mjs";

let checks = 0;
function ok(cond, label) {
  assert.ok(cond, label);
  checks++;
}

// Exactly 4 ASCII digits — `encodeSetLockPin` throws on anything else, so
// (unlike the admin-PIN test's deliberately-over-16-bytes constant) this
// MUST already be a well-formed lock PIN.
const LOCK_PIN = "9137"; // the exact secret string every Part 1 check greps for

// ── Part 1: session.js's setLockPin, driven for real, against hostile globals ──
// (makeFakePort/installHostileGlobals/spyOnConsole/assertSecretNeverCaptured
// are shared with guest-password-hygiene.test.mjs/admin-pin-hygiene.test.mjs/
// channel-secret-hygiene.test.mjs — see secret-hygiene-test-helpers.mjs's
// own header.)

function assertLockPinNeverCaptured(captured, label) {
  assertSecretNeverCaptured(ok, captured, LOCK_PIN, "the lock PIN", label);
}

async function setLockPinNeverTouchesStorageOrLogsThePin() {
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

    await session.setLockPin(LOCK_PIN);

    // The PIN DOES, correctly, cross the wire in the clear (same posture as
    // the admin PIN) — assert it's there on the wire (the one place it's
    // supposed to be), as a sanity check that this test would actually
    // catch a regression rather than trivially passing because nothing ran.
    ok(written.length === 1, "exactly one SET_LOCK_PIN frame written");
    const expectedPayload = encodeSetLockPin(LOCK_PIN);
    ok(
      Buffer.from(written[0]).includes(Buffer.from(expectedPayload)),
      "the PIN legitimately appears on the wire frame (the cable is the authentication)"
    );

    await session.disconnect();
  } finally {
    spy.restore();
  }

  assertLockPinNeverCaptured(spy.captured, "setLockPin success path");
  ok(true, "localStorage/sessionStorage were never touched (the hostile Proxy trap above would have thrown otherwise)");
}

async function setLockPinDeviceErrorNeverLeaksThePinEither() {
  const { port, push } = makeFakePort(() => {
    const msg = new TextEncoder().encode("lock pin rejected");
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
      () => session.setLockPin(LOCK_PIN),
      (err) => err instanceof DeviceError
    );

    await session.disconnect();
  } finally {
    spy.restore();
  }

  assertLockPinNeverCaptured(spy.captured, "setLockPin device-error path");
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

ok(!usesStorageApi(provisionerJs, "localStorage"), "provisioner.js never calls the localStorage API");
ok(!usesStorageApi(provisionerJs, "sessionStorage"), "provisioner.js never calls the sessionStorage API");
ok(!usesStorageApi(provisionerHtml, "localStorage"), "provisioner.html never calls the localStorage API");
ok(!usesStorageApi(provisionerHtml, "sessionStorage"), "provisioner.html never calls the sessionStorage API");

// No URL/history manipulation anywhere on the page at all — categorically
// forecloses "no URL/query param" leakage for the lock PIN (or any other
// secret this page touches).
for (const [label, source] of [
  ["provisioner.js", provisionerJs],
  ["provisioner.html", provisionerHtml],
]) {
  ok(!/location\s*\./.test(source), `${label} never assigns/reads window.location`);
  ok(!/URLSearchParams/.test(source), `${label} never builds a URLSearchParams`);
  ok(!/history\s*\.\s*(push|replace)State/.test(source), `${label} never manipulates history state`);
}

const handleSetLockPinBody = extractFunctionBody(ok, provisionerJs, "handleSetLockPin");
const clearFormStatusesBody = extractFunctionBody(ok, provisionerJs, "clearFormStatuses");

// No console.* call in handleSetLockPin may reference a PIN-carrying
// identifier — every console.* line in this function must only ever
// reference the caught error / status strings, never the PIN.
{
  const consoleCallLines = handleSetLockPinBody.split("\n").filter((line) => /console\.(log|warn|error)\(/.test(line));
  ok(consoleCallLines.length >= 0, "handleSetLockPin may or may not log (either is fine)"); // documents intent; real assertion below
  for (const line of consoleCallLines) {
    ok(!/\bpin\b/i.test(line), `handleSetLockPin's console call must not reference the PIN: ${line.trim()}`);
    ok(!/lockPinInput/.test(line), `handleSetLockPin's console call must not reference the PIN field: ${line.trim()}`);
  }
}

// The lock-PIN field is cleared on EVERY exit path from handleSetLockPin
// (success AND the catch branch) — count occurrences of the clear
// statement; there must be at least 2 (one per path), matching what
// handleSetPin's equivalent clear-on-every-path discipline already does.
{
  const clears = countValueClears(handleSetLockPinBody, "lockPinInput");
  ok(clears >= 2, `handleSetLockPin clears lockPinInput.value on at least 2 exit paths (found ${clears})`);
}

// clearFormStatuses (the disconnect path) also scrubs it.
ok(countValueClears(clearFormStatusesBody, "lockPinInput") >= 1, "clearFormStatuses clears lockPinInput.value on disconnect");

// ── HTML: type=password + autocomplete=off (no form autofill/autocomplete) ──

{
  const inputMatch = provisionerHtml.match(/<input[^>]*id="lock-pin-input"[^>]*>/);
  ok(inputMatch !== null, "found the lock-pin-input <input> tag");
  const tag = inputMatch[0];
  ok(/type="password"/.test(tag), `lock-pin-input input must be type="password": ${tag}`);
  ok(/autocomplete="off"/.test(tag), `lock-pin-input input must have autocomplete="off": ${tag}`);
  ok(/inputmode="numeric"/.test(tag), `lock-pin-input input must have inputmode="numeric": ${tag}`);
}

{
  const formMatch = provisionerHtml.match(/<form[^>]*id="lock-pin-form"[^>]*>/);
  ok(formMatch !== null, "found the lock-pin-form <form> tag");
  ok(/autocomplete="off"/.test(formMatch[0]), `lock-pin-form must have autocomplete="off": ${formMatch[0]}`);
}

// ── Run the Part 1 async scenarios ───────────────────────────────────────

await setLockPinNeverTouchesStorageOrLogsThePin();
await setLockPinDeviceErrorNeverLeaksThePinEither();

console.log(`lock-pin-hygiene.test: OK — ${checks} check(s) passed.`);
