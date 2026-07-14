// flash-manifest.test.mjs — regression coverage for flash-manifest.js's
// resolveFreshInstallParts, the gate deciding which bytes get written to
// which flash address for the Fresh-install path (see the module's own
// header). Plain `node`, zero dependencies, matching this site's other
// *.test.mjs files. Run directly:
//
//   node site/flash-manifest.test.mjs

import assert from "node:assert/strict";
import { resolveFreshInstallParts } from "./flash-manifest.js";

let checks = 0;
function eq(actual, expected, label) {
  assert.deepEqual(actual, expected, label);
  checks++;
}
function isNull(actual, label) {
  assert.equal(actual, null, label);
  checks++;
}

const CHIP = "ESP32-S3";

// A well-formed manifest.json exactly as release.yml's "Generate esp-web-tools
// manifest.json" step emits it.
const VALID = {
  name: "MeshCadet",
  version: "v0.3.0",
  builds: [
    {
      chipFamily: CHIP,
      parts: [{ path: "meshcadet-v0.3.0-merged.bin", offset: 0 }],
    },
  ],
};

eq(
  resolveFreshInstallParts(VALID, CHIP),
  [{ path: "meshcadet-v0.3.0-merged.bin", offset: 0 }],
  "accepts a well-formed manifest and returns its parts"
);

isNull(resolveFreshInstallParts(null, CHIP), "rejects null (not mirrored / fetch failed)");
isNull(resolveFreshInstallParts("not an object", CHIP), "rejects a non-object manifest");
isNull(resolveFreshInstallParts({ ...VALID, builds: undefined }, CHIP), "rejects a manifest with no builds array");
isNull(resolveFreshInstallParts({ ...VALID, builds: [] }, CHIP), "rejects a manifest with an empty builds array");

isNull(
  resolveFreshInstallParts(
    { ...VALID, builds: [{ chipFamily: "ESP32", parts: VALID.builds[0].parts }] },
    CHIP
  ),
  "rejects a manifest with no build matching the connected device's chip family"
);

isNull(
  resolveFreshInstallParts({ ...VALID, builds: [{ chipFamily: CHIP, parts: [] }] }, CHIP),
  "rejects a matching build with no parts"
);

isNull(
  resolveFreshInstallParts(
    { ...VALID, builds: [{ chipFamily: CHIP, parts: [{ path: "evil.bin", offset: 0 }] }] },
    CHIP
  ),
  "rejects a part path that doesn't match release.yml's merged-image naming convention"
);

isNull(
  resolveFreshInstallParts(
    {
      ...VALID,
      builds: [{ chipFamily: CHIP, parts: [{ path: "../../evil.bin", offset: 0 }] }],
    },
    CHIP
  ),
  "rejects a part path attempting path traversal"
);

isNull(
  resolveFreshInstallParts(
    {
      ...VALID,
      builds: [{ chipFamily: CHIP, parts: [{ path: "meshcadet-v0.3.0-merged.bin", offset: -1 }] }],
    },
    CHIP
  ),
  "rejects a negative offset"
);

isNull(
  resolveFreshInstallParts(
    {
      ...VALID,
      builds: [{ chipFamily: CHIP, parts: [{ path: "meshcadet-v0.3.0-merged.bin", offset: "0" }] }],
    },
    CHIP
  ),
  "rejects a stringly-typed offset even if numerically equal"
);

isNull(
  resolveFreshInstallParts(
    { ...VALID, builds: [{ chipFamily: CHIP, parts: [{ offset: 0 }] }] },
    CHIP
  ),
  "rejects a part missing its path"
);

{
  const multiPart = {
    ...VALID,
    builds: [
      {
        chipFamily: CHIP,
        parts: [
          { path: "meshcadet-v0.3.0-merged.bin", offset: 0 },
          { path: "meshcadet-v0.3.0-merged.bin", offset: 4096 },
        ],
      },
    ],
  };
  eq(
    resolveFreshInstallParts(multiPart, CHIP),
    [
      { path: "meshcadet-v0.3.0-merged.bin", offset: 0 },
      { path: "meshcadet-v0.3.0-merged.bin", offset: 4096 },
    ],
    "accepts (and preserves order of) multiple valid parts"
  );
}

console.log(`flash-manifest.test: OK — ${checks} check(s) passed.`);
