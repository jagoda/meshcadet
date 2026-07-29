// secret-hygiene-test-helpers.mjs — shared assertion/fixture logic for
// site/provisioner/*-hygiene.test.mjs.
//
// WHY THIS EXISTS: guest-password-hygiene.test.mjs, admin-pin-hygiene.test.mjs,
// and channel-secret-hygiene.test.mjs each independently hand-implemented the
// SAME fixtures and assertion logic — a fake Web Serial port, hostile
// localStorage/sessionStorage traps, a console spy, a JSON-safe stringifier,
// a "never captured in console output" check, a brace-balance function-body
// extractor, a storage-API-touch detector, and an exit-path clear-count
// counter — three times, by hand, with no shared module asserting parity
// between the copies. Two hand-duplicated copies (guest-password, admin-pin)
// was a coincidence; channel-secret-hygiene.test.mjs made it three, which is
// this project's own N=3 dedup-or-extract threshold for hand-duplicated
// logic. Extracted here so the NEXT secret-carrying handler's hygiene test
// is a call site, not a
// fourth from-scratch transcription.
//
// `extractFunctionBody` and `countValueClears` (and, transitively, anything
// that calls them) take the caller's own `ok(cond, label)` counter/asserter
// as their first argument so each test file's own "N checks passed" count
// keeps counting every assertion this module performs on the caller's
// behalf, exactly as it did when the logic was inlined.
//
// Plain ES module, zero dependencies, matching this site's "no build step"
// convention (site/README.md).

/** A fake Web Serial `SerialPort`-shaped object over Node's built-in WHATWG Streams, calling `onWrite` with every chunk written to it. */
export function makeFakePort(onWrite) {
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
 * Install `navigator`/`window` (the minimal shape session.smoke.test.mjs
 * uses), PLUS a `localStorage`/`sessionStorage` pair that throws on ANY
 * property access or assignment — not a spy that records calls, an actual
 * trap — so if the code under test ever so much as reads `localStorage.foo`,
 * the test fails immediately with a thrown error rather than relying on
 * remembering to assert "it wasn't called". `label` (e.g.
 * "channel-secret-hygiene") identifies the caller in the thrown message.
 */
export function installHostileGlobals(port, label) {
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

  const storageTrap = (storageName) =>
    new Proxy(
      {},
      {
        get() {
          throw new Error(`${label}: ${storageName} was touched (get) — must never be touched at all`);
        },
        set() {
          throw new Error(`${label}: ${storageName} was touched (set) — must never be touched at all`);
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

/** Wrap console.log/warn/error to capture every argument, restoring on manual `.restore()`. */
export function spyOnConsole() {
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

/** JSON-stringify that never throws — Error objects render as {message, stack} instead of `{}`. */
export function safeStringify(value) {
  try {
    return JSON.stringify(value, (_key, v) => (v instanceof Error ? { message: v.message, stack: v.stack } : v));
  } catch {
    return String(value);
  }
}

/** Assert no argument captured by `spyOnConsole()` ever contains `secretValue`. `secretName` (e.g. "the guest password") is used in the failure message; `phaseLabel` identifies which scenario was running (e.g. "addRoom success path"). */
export function assertSecretNeverCaptured(ok, captured, secretValue, secretName, phaseLabel) {
  for (const args of captured) {
    for (const arg of args) {
      const rendered = typeof arg === "string" ? arg : safeStringify(arg);
      ok(!rendered.includes(secretValue), `${phaseLabel}: console output must never contain ${secretName} (got: ${rendered})`);
    }
  }
}

/** True if `source` contains an actual `identifier.`/`identifier[` API touch — not merely the word appearing inside prose/backticks. */
export function usesStorageApi(source, identifier) {
  return new RegExp(`\\b${identifier}\\s*[.[]`).test(source);
}

/** Extract a top-level `async function <name>(...) { ... }` body by brace-balance scanning — good enough for provisioner.js's own straight-line functions (no template-literal braces inside the extracted range). Asserts (via the caller's `ok`) that the declaration was found. */
export function extractFunctionBody(ok, source, name) {
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

/** Count `<fieldIdent>.value = "…"` clear statements inside `body` — the exit-path clear-count check every hygiene test performs (at least one per exit path a secret-carrying field must be scrubbed on). */
export function countValueClears(body, fieldIdent) {
  return (body.match(new RegExp(`${fieldIdent}\\.value\\s*=\\s*("|')("|')`, "g")) || []).length;
}
