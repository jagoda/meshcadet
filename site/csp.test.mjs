// csp.test.mjs — regression coverage for the meshcadet-channel-secret-leak-
// security-audit finding B3 fix: every site/*.html page must carry a
// Content-Security-Policy <meta> tag (GitHub Pages can't set response
// headers, so this is the only enforcement mechanism available), and
// provisioner.js — the one page handling secrets (channel secret, admin
// PIN, room guest password) — must never regress back to loading a
// third-party script origin. Also covers the
// meshcadet-flasher-cdn-import-unpinned-integrity fix (outsider-boundary
// finding F6): flash.js — the page that writes firmware to an attached
// ESP32 over WebSerial — must never regress back to CDN-importing
// esptool-js either.
//
// This is deliberately a static source-text assertion (same pattern as
// site/provisioner/*-hygiene.test.mjs), not a driven-browser test: it
// exists to catch a future edit that silently drops or loosens a page's
// CSP, or reintroduces a CDN import into provisioner.js, in CI — not to
// re-verify the CSP mechanism itself (browsers enforce that; verified live
// with headless Chromium when this fix landed — zero
// securitypolicyviolation events, zero console errors, on all three pages).
//
// Plain `node`, zero dependencies (no package.json), matching this site's
// other *.test.mjs files. Run directly:
//
//   node site/csp.test.mjs

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

let checks = 0;
function ok(actual, label) {
  assert.equal(actual, true, label);
  checks++;
}
function notOk(actual, label) {
  assert.equal(actual, false, label);
  checks++;
}

const siteDir = path.dirname(fileURLToPath(import.meta.url));

function readSite(relPath) {
  return readFileSync(path.join(siteDir, relPath), "utf-8");
}

/** Extracts the `content="..."` value of the page's CSP <meta> tag, or null if absent. */
function cspContent(html) {
  const m = html.match(/<meta\s+http-equiv="Content-Security-Policy"\s+content="([^"]*)"\s*\/?>/);
  return m ? m[1] : null;
}

/** True if `csp` (a `directive value1 value2; directive2 …` string) grants `directive` exactly `values` (as a set, order-independent). */
function directiveGrants(csp, directive, values) {
  const re = new RegExp(`(?:^|;)\\s*${directive}\\s+([^;]+?)\\s*(?:;|$)`);
  const m = csp.match(re);
  if (!m) return false;
  const got = m[1].trim().split(/\s+/).sort();
  const want = [...values].sort();
  return got.length === want.length && got.every((v, i) => v === want[i]);
}

// ── index.html: no script, no fetch of its own — bare default-src 'self' ──

{
  const html = readSite("index.html");
  const csp = cspContent(html);
  ok(csp !== null, "index.html carries a Content-Security-Policy meta tag");
  ok(directiveGrants(csp, "default-src", ["'self'"]), "index.html: default-src is exactly 'self'");
  ok(!/connect-src|script-src/.test(csp), "index.html has no script-src/connect-src override (none needed — it loads no script)");
}

// ── flash.html: one named allowance left (the release-list fetch); esptool-js
//    is vendored, so script-src needs no third-party origin (meshcadet-
//    flasher-cdn-import-unpinned-integrity) ──

{
  const html = readSite("flash.html");
  const csp = cspContent(html);
  ok(csp !== null, "flash.html carries a Content-Security-Policy meta tag");
  ok(directiveGrants(csp, "default-src", ["'self'"]), "flash.html: default-src is exactly 'self'");
  ok(
    directiveGrants(csp, "connect-src", ["'self'", "https://api.github.com"]),
    "flash.html: connect-src allows 'self' and https://api.github.com (the release-list fetch) and nothing else"
  );
  ok(
    !/script-src/.test(csp),
    "flash.html has no script-src override — default-src 'self' alone must cover it (esptool-js is vendored, no third-party script origin)"
  );
}

// ── provisioner.html: the secret-bearing page gets the tightest policy ──

{
  const html = readSite("provisioner.html");
  const csp = cspContent(html);
  ok(csp !== null, "provisioner.html carries a Content-Security-Policy meta tag");
  ok(directiveGrants(csp, "default-src", ["'self'"]), "provisioner.html: default-src is exactly 'self'");
  ok(
    directiveGrants(csp, "connect-src", ["'none'"]),
    "provisioner.html: connect-src is 'none' — this page issues no fetch/XHR of any kind"
  );
  ok(!/script-src/.test(csp), "provisioner.html has no script-src override — default-src 'self' alone must cover it (no third-party script origin)");
}

// ── provisioner.js: the QR import must stay a local, relative import ──

{
  const js = readSite("provisioner.js");
  const importLine = js.match(/^import QRCode from "([^"]+)";$/m);
  ok(importLine !== null, "provisioner.js imports QRCode from a single, greppable import statement");
  const specifier = importLine[1];
  ok(specifier.startsWith("./"), "provisioner.js's QRCode import is a local relative path, not a CDN URL");
  notOk(/^https?:\/\//.test(specifier), "provisioner.js's QRCode import is not an absolute http(s) URL");
}

// ── site/vendor/qrcode.js: the vendored file itself must have no unresolved imports ──

{
  const vendored = readSite("vendor/qrcode.js");
  const bareImports = vendored.match(/^\s*import\b/gm) || [];
  ok(bareImports.length === 0, "site/vendor/qrcode.js has no import statements of its own (fully self-contained)");
}

// ── flash.js: the esptool-js import must stay a local, relative import
//    (meshcadet-flasher-cdn-import-unpinned-integrity) ──

{
  const js = readSite("flash.js");
  const importLine = js.match(/^import \{ ESPLoader, Transport \} from "([^"]+)";$/m);
  ok(importLine !== null, "flash.js imports ESPLoader/Transport from a single, greppable import statement");
  const specifier = importLine[1];
  ok(specifier.startsWith("./"), "flash.js's esptool-js import is a local relative path, not a CDN URL");
  notOk(/^https?:\/\//.test(specifier), "flash.js's esptool-js import is not an absolute http(s) URL");
}

// ── site/vendor/esptool-js.js: the vendored file itself must have no unresolved imports ──

{
  const vendored = readSite("vendor/esptool-js.js");
  const bareImports = vendored.match(/^\s*import\b/gm) || [];
  ok(bareImports.length === 0, "site/vendor/esptool-js.js has no import statements of its own (fully self-contained)");
}

console.log(`csp.test: OK — ${checks} check(s) passed.`);
