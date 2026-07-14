// flash-image-encoding.test.mjs — regression coverage for
// flash-image-encoding.js's ui8ToBstr: the Uint8Array -> Latin-1
// binary-string conversion that stands between a downloaded firmware image
// and esptool-js's ESPLoader.writeFlash, which requires that shape (see the
// module's own header for the "A.charCodeAt is not a function" crash this
// closes). Plain `node`, zero dependencies, matching this site's other
// *.test.mjs files. Run directly:
//
//   node site/flash-image-encoding.test.mjs

import assert from "node:assert/strict";
import { ui8ToBstr } from "./flash-image-encoding.js";

let checks = 0;
function eq(actual, expected, label) {
  assert.equal(actual, expected, label);
  checks++;
}

eq(ui8ToBstr(new Uint8Array([])), "", "empty input produces an empty string");

eq(
  ui8ToBstr(new Uint8Array([0x41, 0x42, 0x43])),
  "ABC",
  "ASCII-range bytes map to the identical characters"
);

// The exact contract esptool-js's writeFlash depends on: every result
// character's code unit must equal the source byte value it came from — a
// TypeError-throwing `.charCodeAt()` call is impossible to distinguish from
// a byte-corrupting one from the caller's perspective, so round-tripping is
// the actual invariant under test, not just "doesn't throw".
{
  const bytes = new Uint8Array(256);
  for (let i = 0; i < 256; i++) {
    bytes[i] = i;
  }
  const bstr = ui8ToBstr(bytes);
  eq(bstr.length, 256, "one character per byte, including the full 0x00-0xFF range");
  let roundTripOk = true;
  for (let i = 0; i < 256; i++) {
    if (bstr.charCodeAt(i) !== bytes[i]) {
      roundTripOk = false;
      break;
    }
  }
  eq(roundTripOk, true, "every byte 0x00-0xFF round-trips through charCodeAt unchanged");
}

// The exact regression: a Uint8Array has no .charCodeAt, so passing one
// straight through to esptool-js's writeFlash (as flash.js did before this
// fix) throws `TypeError: fileArray[i].data.charCodeAt is not a function`.
// ui8ToBstr's output must actually support charCodeAt (i.e. be a string).
eq(
  typeof ui8ToBstr(new Uint8Array([1, 2, 3])).charCodeAt,
  "function",
  "output is a real string (supports .charCodeAt, unlike the Uint8Array esptool-js was handed before this fix)"
);

console.log(`flash-image-encoding.test: OK — ${checks} check(s) passed.`);
