// csp.test.mjs — regression coverage for the meshcadet-channel-secret-leak-
// security-audit finding B3 fix: every site/*.html page must carry a
// Content-Security-Policy <meta> tag (GitHub Pages can't set response
// headers, so this is the only enforcement mechanism available). Also
// covers finding F6 / the meshcadet-flasher-cdn-import-unpinned-integrity
// fix: no page's script-src (nor default-src, when script-src is absent
// and it's the fallback) may allow a third-party origin, and no JS file
// reachable from a page's <script type=module> entry point may import an
// absolute http(s):// URL.
//
// The script-src/import checks below are a DOMAIN-ENUMERATION coverage
// test, not a per-instance one — see
// flight-manuals/library/coverage-must-enumerate-the-set.md. This file used
// to hand-enumerate one ok()/notOk() block per page/script, added
// reactively each time a CDN import was found and fixed (qrcode.js into
// provisioner.js, then esptool-js into flash.js): nothing asserted that the
// SET of checks spanned the SET of pages/scripts, so a third CDN import
// into a new page or script would have passed this file silently. Instead,
// this walks every site/*.html found on disk and every JS file reachable
// from each page's module entry point (following relative imports,
// stopping recursion at site/vendor/ — vendored bundles are expected to be
// fully self-contained) and asserts the invariant holds across that whole
// domain, so a future third instance fails CI automatically (mirrors
// site/provisioner/secret-hygiene-coverage.test.mjs's shape).
//
// This is deliberately a static source-text assertion (same pattern as
// site/provisioner/*-hygiene.test.mjs), not a driven-browser test: it
// exists to catch a future edit that silently drops/loosens a page's CSP or
// (re)introduces a CDN import, in CI — not to re-verify the CSP mechanism
// itself (browsers enforce that; verified live with headless Chromium when
// the B3/F6 fixes landed — zero securitypolicyviolation events, zero
// console errors, on all three pages).
//
// Plain `node`, zero dependencies (no package.json), matching this site's
// other *.test.mjs files. Run directly:
//
//   node site/csp.test.mjs

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
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

/** The raw space-separated value tokens `csp` grants `directive`, or null if the directive is absent. */
function directiveValues(csp, directive) {
  const re = new RegExp(`(?:^|;)\\s*${directive}\\s+([^;]+?)\\s*(?:;|$)`);
  const m = csp.match(re);
  return m ? m[1].trim().split(/\s+/) : null;
}

/** script-src's effective allowance: itself if present, else its CSP-spec fallback, default-src. */
function scriptSrcValues(csp) {
  return directiveValues(csp, "script-src") ?? directiveValues(csp, "default-src") ?? [];
}

/** Every top-level static import specifier in `source` — handles both single-line
 * (`import X from "spec";`) and multi-line (`import {\n  a,\n  b,\n} from "spec";`) forms,
 * since this site's real files use both. */
function importSpecifiers(source) {
  const re = /import\s+[^;]*?from\s+["']([^"']+)["']\s*;/g;
  const specs = [];
  let m;
  while ((m = re.exec(source)) !== null) specs.push(m[1]);
  return specs;
}

// ── index.html: no script, no fetch of its own — bare default-src 'self' ──

{
  const html = readSite("index.html");
  const csp = cspContent(html);
  ok(directiveGrants(csp, "default-src", ["'self'"]), "index.html: default-src is exactly 'self'");
  ok(!/connect-src/.test(csp), "index.html has no connect-src override (none needed — it issues no fetch/XHR)");
}

// ── flash.html: one named allowance left (the release-list fetch) ──

{
  const html = readSite("flash.html");
  const csp = cspContent(html);
  ok(directiveGrants(csp, "default-src", ["'self'"]), "flash.html: default-src is exactly 'self'");
  ok(
    directiveGrants(csp, "connect-src", ["'self'", "https://api.github.com"]),
    "flash.html: connect-src allows 'self' and https://api.github.com (the release-list fetch) and nothing else"
  );
}

// ── provisioner.html: the secret-bearing page gets the tightest policy ──

{
  const html = readSite("provisioner.html");
  const csp = cspContent(html);
  ok(directiveGrants(csp, "default-src", ["'self'"]), "provisioner.html: default-src is exactly 'self'");
  ok(
    directiveGrants(csp, "connect-src", ["'none'"]),
    "provisioner.html: connect-src is 'none' — this page issues no fetch/XHR of any kind"
  );
}

// ── domain enumeration: every site/*.html's script-src (falling back to
//    default-src) must allow no third-party origin, and every JS file
//    reachable from a page's <script type="module"> entry point (stopping
//    recursion at site/vendor/) must carry no absolute http(s):// import
//    specifier. This replaces the old per-page/per-file hand-written
//    blocks (one added per CDN-import incident) — a future page or script
//    with a fresh CDN import now fails here automatically, without a new
//    assertion block having to be hand-added for it. ──

{
  const htmlFiles = readdirSync(siteDir)
    .filter((name) => name.endsWith(".html"))
    .sort();
  ok(htmlFiles.length > 0, "found at least one site/*.html page (if 0, the enumeration below is broken — fix it, don't skip this test)");

  const reachable = new Map(); // relative JS path -> its import specifiers
  let entryPointCount = 0;

  for (const name of htmlFiles) {
    const html = readSite(name);
    const csp = cspContent(html);
    ok(csp !== null, `${name} carries a Content-Security-Policy meta tag`);

    for (const value of scriptSrcValues(csp)) {
      notOk(
        /^https?:\/\//.test(value),
        `${name}'s script-src (or default-src fallback, if script-src is absent) allows no third-party origin — found "${value}"`
      );
    }

    // Attribute-order-independent (same discipline as passwordInputIds() in
    // secret-hygiene-coverage.test.mjs): a `<script src="…" type="module">`
    // with attributes in the other order must still be found as an entry
    // point, or this whole domain walk silently skips that page.
    const scriptTags = html.match(/<script\b[^>]*>/g) || [];
    const entrySrcs = scriptTags
      .filter((tag) => /\btype="module"/.test(tag))
      .map((tag) => tag.match(/\bsrc="([^"]+)"/))
      .filter((m) => m !== null)
      .map((m) => m[1]);
    if (entrySrcs.length === 0) continue; // this page loads no module script (e.g. index.html) — nothing to walk
    entryPointCount++;

    const queue = [...entrySrcs];
    while (queue.length > 0) {
      const rel = queue.shift();
      if (reachable.has(rel)) continue;

      const specs = importSpecifiers(readSite(rel));
      reachable.set(rel, specs);

      const isVendored = rel === "vendor" || rel.startsWith("vendor/") || rel.includes("/vendor/");
      if (isVendored) continue; // stop recursion at the vendor boundary — vendored bundles are expected to be fully self-contained

      for (const spec of specs) {
        if (!spec.startsWith(".")) continue; // an absolute http(s):// URL or a bare specifier — not a local file to recurse into, but still recorded below for the http(s) check
        queue.push(path.posix.normalize(path.posix.join(path.posix.dirname(rel), spec)));
      }
    }
  }

  ok(
    entryPointCount > 0,
    'found at least one <script type="module" src="..."> page entry point (if 0, the enumeration above is broken — fix it, don\'t skip this test)'
  );
  ok(
    reachable.size > 0,
    "walked at least one reachable local JS file from a page entry point (if 0, the enumeration above is broken — fix it, don't skip this test)"
  );

  for (const [rel, specs] of reachable) {
    for (const spec of specs) {
      notOk(/^https?:\/\//.test(spec), `${rel}'s import "${spec}" is not an absolute http(s) URL`);
    }
    if (rel.startsWith("vendor/")) {
      ok(specs.length === 0, `${rel} has no import statements of its own (fully self-contained, per the vendoring boundary)`);
    }
  }
}

console.log(`csp.test: OK — ${checks} check(s) passed.`);
