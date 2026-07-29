// secret-hygiene-coverage.test.mjs — mechanized coverage guard: this page's
// secret-hygiene contract (guest-password-hygiene.test.mjs,
// admin-pin-hygiene.test.mjs, channel-secret-hygiene.test.mjs) was built up
// reactively, one `type="password"` field at a time, each only after an
// incident (a backfill mission, then a full security audit) noticed that
// field was uncovered. Nothing previously asserted that the SET of hygiene
// tests actually spans the SET of secret-carrying fields — a fourth secret
// field could be added to provisioner.html today with no hygiene test at
// all, and every existing test would keep passing, silently, until the next
// security audit happened to look.
//
// This test enumerates every `type="password"` <input> in provisioner.html
// by parsing the shipped SOURCE TEXT (same static-source-assertion pattern
// as the *-hygiene.test.mjs files it guards — see their own headers for why
// provisioner.html/.js can't be loaded under plain `node`), and asserts each
// one's `id` appears in at least one `*-hygiene.test.mjs` file's source text
// in this directory. It does NOT assert which file covers which field, or
// how — that's what the hygiene tests themselves check — only that SOME
// hygiene test exists that mentions the field. A future secret field added
// without a matching hygiene test (or added under an id no hygiene test
// mentions) fails this test immediately, in CI, without waiting for a
// security audit to notice.
//
// Plain `node`, zero dependencies (no package.json), matching this site's
// other *.test.mjs files. Run directly:
//
//   node site/provisioner/secret-hygiene-coverage.test.mjs

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

let checks = 0;
function ok(cond, label) {
  assert.ok(cond, label);
  checks++;
}

const here = path.dirname(fileURLToPath(import.meta.url));
const siteDir = path.resolve(here, "..");
const provisionerHtml = readFileSync(path.join(siteDir, "provisioner.html"), "utf-8");

/** Every `type="password"` <input>'s `id` in `html` — attribute order-independent, so this doesn't silently miss a field whose attributes are written in a different order than the existing ones. */
function passwordInputIds(html) {
  const inputTags = html.match(/<input\b[^>]*>/g) || [];
  const ids = [];
  for (const tag of inputTags) {
    if (!/\btype="password"/.test(tag)) continue;
    const idMatch = tag.match(/\bid="([^"]+)"/);
    ids.push({ tag, id: idMatch ? idMatch[1] : null });
  }
  return ids;
}

const passwordInputs = passwordInputIds(provisionerHtml);

// Sanity check that this test would actually catch a regression rather than
// trivially passing because the parse found nothing: provisioner.html has
// (at least) the channel secret (add/del), the room guest password, and the
// admin PIN today — four `type="password"` inputs across three secrets.
ok(passwordInputs.length > 0, "found at least one type=\"password\" <input> in provisioner.html (if this is 0, the parse regex above is broken — fix it, don't skip this test)");

for (const { tag, id } of passwordInputs) {
  ok(id !== null, `every type="password" <input> must carry an id so a hygiene test can reference it: ${tag}`);
}

// Every *-hygiene.test.mjs file in this directory, concatenated — this test
// deliberately does NOT hardcode "there are exactly 3 hygiene test files" or
// "field X maps to file Y"; it only requires that the DOMAIN (every
// password field) is a SUBSET of what the hygiene suite as a whole
// mentions, so adding a fourth hygiene test file (rather than editing an
// existing one) satisfies this just as well.
const hygieneTestFiles = readdirSync(here).filter((name) => name.endsWith("-hygiene.test.mjs"));
ok(hygieneTestFiles.length > 0, "at least one *-hygiene.test.mjs file exists in site/provisioner/ (if this is 0, the hygiene suite itself was deleted)");

const hygieneSource = hygieneTestFiles.map((name) => readFileSync(path.join(here, name), "utf-8")).join("\n");

for (const { tag, id } of passwordInputs) {
  if (id === null) continue; // already flagged above
  ok(
    hygieneSource.includes(id),
    `provisioner.html's "${id}" type="password" field (${tag}) must be covered by a *-hygiene.test.mjs — ` +
      `add one (see guest-password-hygiene.test.mjs/admin-pin-hygiene.test.mjs/channel-secret-hygiene.test.mjs ` +
      `as worked examples, and site/provisioner/secret-hygiene-test-helpers.mjs for the shared assertion logic), ` +
      `then add a step for it to .github/workflows/pages-check.yml (this repo's CI wires each hygiene test file ` +
      `by an explicit \`run:\` step, not a glob)`
  );
}

console.log(`secret-hygiene-coverage.test: OK — ${checks} check(s) passed (${passwordInputs.length} type="password" field(s), ${hygieneTestFiles.length} hygiene test file(s)).`);
