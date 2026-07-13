// upgrade-gate.test.mjs — regression coverage for upgrade-gate.js's
// isValidUpdateMeta, the gate deciding whether flash.html's Upgrade path is
// ever offered — or ever writes a byte — for a given release (ADR-0008
// D2/D4, ADR-0009 D2). This is the safety-critical decision this whole
// ADR is about, so it gets plain-node test coverage independent of a
// browser/DOM, same as site/provisioner/validation.js.
//
// Plain `node`, zero dependencies (no package.json), matching this site's
// other *.test.mjs files. Run directly:
//
//   node site/upgrade-gate.test.mjs

import assert from "node:assert/strict";
import { isValidUpdateMeta, EXPECTED_APP_OFFSET } from "./upgrade-gate.js";

let checks = 0;
function ok(actual, label) {
  assert.equal(actual, true, label);
  checks++;
}
function notOk(actual, label) {
  assert.equal(actual, false, label);
  checks++;
}

// A well-formed update-meta.json exactly as
// firmware/release-container/generate-update-meta.sh emits it.
const VALID = {
  version: "v0.3.0",
  layout_hash: "a".repeat(64),
  layout_baseline: "a".repeat(64),
  upgrade_safe: true,
  app_asset: "meshcadet-v0.3.0-app.bin",
  app_offset: EXPECTED_APP_OFFSET,
};

ok(isValidUpdateMeta(VALID), "accepts a well-formed, upgrade_safe metadata object");

notOk(isValidUpdateMeta(null), "rejects null (not mirrored / fetch failed)");

notOk(
  isValidUpdateMeta({ ...VALID, upgrade_safe: false }),
  "rejects upgrade_safe: false (a layout-changing release, ADR-0008 D2)"
);

notOk(
  isValidUpdateMeta({ ...VALID, upgrade_safe: "true" }),
  "rejects a truthy-but-non-boolean upgrade_safe"
);

notOk(
  isValidUpdateMeta({ ...VALID, app_offset: 0 }),
  "rejects an app_offset that doesn't match the frozen factory-partition offset"
);

notOk(
  isValidUpdateMeta({ ...VALID, app_offset: "65536" }),
  "rejects a stringly-typed app_offset even if numerically equal"
);

notOk(
  isValidUpdateMeta({ ...VALID, app_asset: "evil.bin" }),
  "rejects an app_asset that doesn't match release.yml's naming convention"
);

notOk(
  isValidUpdateMeta({ ...VALID, app_asset: "../../evil.bin" }),
  "rejects an app_asset attempting path traversal"
);

{
  const { app_asset, ...rest } = VALID;
  notOk(isValidUpdateMeta(rest), "rejects a missing app_asset");
}

{
  const { app_offset, ...rest } = VALID;
  notOk(isValidUpdateMeta(rest), "rejects a missing app_offset");
}

console.log(`upgrade-gate.test: OK — ${checks} check(s) passed.`);
