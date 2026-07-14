// flash-image-encoding.js — the Uint8Array -> Latin-1 binary-string
// conversion esptool-js's ESPLoader.writeFlash requires for each
// `fileArray[].data` entry.
//
// writeFlash does NOT accept a Uint8Array (verified live against the pinned
// esptool-js@0.5.7 bundle.js: `writeFlash` immediately does
// `this.bstrToUi8(fileArray[i].data)`, and `bstrToUi8` calls
// `data.charCodeAt(i)` — a method Uint8Array doesn't have). It wants a
// "binary string": one JS string character per byte, code unit === byte
// value, produced by exactly the loop below (the same one esptool-js's own
// `ui8ToBstr` instance method uses internally). Passing a Uint8Array
// straight through — which both flash.js flows did before this module
// existed — throws `TypeError: fileArray[i].data.charCodeAt is not a
// function` partway through the FIRST write call, i.e. after Fresh
// install's full-chip erase has already started (ESPLoader.writeFlash
// erases before it iterates fileArray) — see
// docs/adr/0011-unified-esptool-js-flasher.md and this fix's own commit for
// the incident.
//
// Deliberately NOT `new TextDecoder("latin1").decode(bytes)`: the WHATWG
// Encoding Standard's "latin1"/"iso-8859-1" label actually decodes via the
// windows-1252 table, which remaps bytes 0x80-0x9F to non-identical Unicode
// code points (e.g. 0x80 -> U+20AC). That would silently corrupt any
// firmware byte in that range instead of round-tripping it, which
// `String.fromCharCode` (operating on raw byte values, not a decoder table)
// does not.

// DOM-free, pure function — testable under plain `node` (flash-image-
// encoding.test.mjs), same pattern as flash-manifest.js/upgrade-gate.js.
export function ui8ToBstr(bytes) {
  let out = "";
  for (let i = 0; i < bytes.length; i++) {
    out += String.fromCharCode(bytes[i]);
  }
  return out;
}
