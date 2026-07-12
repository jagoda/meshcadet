// history-format.test.mjs — coverage for history-format.js, the DOM-free
// port of host/src/history_format.rs (the `export-history` transcript
// renderer). Asserts the same column-alignment invariant the Rust module's
// own #[cfg(test)] tests assert: header and data rows land their column
// starts at identical byte offsets, and each field renders in its column.
//
// Timezone is pinned to UTC (process.env.TZ, set before any Date is
// constructed) so `formatLocalTimestamp`'s local-offset output is
// deterministic across machines — the browser uses the viewer's real local
// zone, but the *format shape* is what this guards.
//
// Plain `node`, zero dependencies, matching the other provisioner tests'
// build-step-free posture. Run directly:
//
//   node site/provisioner/history-format.test.mjs

process.env.TZ = "UTC";

import assert from "node:assert/strict";
import {
  idxWidth,
  historyHeader,
  formatLocalTimestamp,
  formatHistoryLine,
  formatHistoryTranscript,
} from "./history-format.js";
import { HISTORY_MSG_TYPE_DM, HISTORY_MSG_TYPE_GRP_TXT } from "./codec.js";

// Column widths — kept in sync with history-format.js (and history_format.rs).
const TIMESTAMP_WIDTH = 26;
const TYPE_WIDTH = 4;
const DIR_WIDTH = 4;
const FROM_WIDTH = 4;
const COL_SEP_LEN = 2;

function makeEntry(senderHash, msgType, timestamp, text, isOurs = false) {
  return {
    index: 0,
    sender_hash: senderHash,
    msg_type: msgType,
    timestamp,
    text,
    text_len: text.length,
    is_ours: isOurs,
  };
}

/** Slice a fixed-width column starting at byte `start`, trimming pad spaces. */
function columnAt(line, start, width) {
  return line.slice(start, start + width).trimEnd();
}

/** Byte offsets of each column start given an `idx` width. */
function offsets(iw) {
  const ts = iw + COL_SEP_LEN;
  const ty = ts + TIMESTAMP_WIDTH + COL_SEP_LEN;
  const dr = ty + TYPE_WIDTH + COL_SEP_LEN;
  const fr = dr + DIR_WIDTH + COL_SEP_LEN;
  const tx = fr + FROM_WIDTH + COL_SEP_LEN;
  return { idx: 0, ts, ty, dr, fr, tx };
}

const tests = [
  ["formatLocalTimestamp is deterministic", () => {
    assert.equal(formatLocalTimestamp(1_700_000_000), formatLocalTimestamp(1_700_000_000));
  }],

  ["formatLocalTimestamp distinguishes different epochs", () => {
    assert.notEqual(formatLocalTimestamp(0), formatLocalTimestamp(1_700_000_000));
  }],

  ["formatLocalTimestamp has the expected 26-char shape", () => {
    const s = formatLocalTimestamp(1_700_000_000);
    assert.equal(s.length, 26, `unexpected length: ${JSON.stringify(s)}`);
    assert.equal(s[10], " ");
    assert.equal(s[19], " ");
    const sign = s[20];
    assert.ok(sign === "+" || sign === "-", `missing offset sign: ${JSON.stringify(s)}`);
    assert.equal(s[23], ":");
  }],

  ["formatLocalTimestamp renders the known UTC epoch", () => {
    // TZ=UTC (pinned above): 1_700_000_000 => 2023-11-14 22:13:20 +00:00.
    assert.equal(formatLocalTimestamp(1_700_000_000), "2023-11-14 22:13:20 +00:00");
  }],

  ["formatHistoryLine renders all columns", () => {
    const entry = makeEntry(0xab, HISTORY_MSG_TYPE_DM, 1_700_000_000, "hello", false);
    const iw = idxWidth(4); // simulate a 4-entry export (indices 0..=3)
    const line = formatHistoryLine(3, entry, iw);
    const o = offsets(iw);
    assert.equal(columnAt(line, o.idx, iw), "3");
    assert.equal(columnAt(line, o.ts, TIMESTAMP_WIDTH), formatLocalTimestamp(1_700_000_000));
    assert.equal(columnAt(line, o.ty, TYPE_WIDTH), "DM");
    assert.equal(columnAt(line, o.dr, DIR_WIDTH), "RECV");
    assert.equal(columnAt(line, o.fr, FROM_WIDTH), "0xAB");
    assert.equal(line.slice(o.tx), "hello");
  }],

  ["formatHistoryLine renders GRP_TXT type", () => {
    const entry = makeEntry(0x01, HISTORY_MSG_TYPE_GRP_TXT, 0, "g");
    const iw = idxWidth(1);
    const line = formatHistoryLine(0, entry, iw);
    const o = offsets(iw);
    assert.equal(columnAt(line, o.ty, TYPE_WIDTH), "GRP");
  }],

  ["formatHistoryLine renders SENT direction for is_ours", () => {
    const entry = makeEntry(0x67, HISTORY_MSG_TYPE_DM, 0, "hi", true);
    const iw = idxWidth(1);
    const line = formatHistoryLine(0, entry, iw);
    const o = offsets(iw);
    assert.equal(columnAt(line, o.dr, DIR_WIDTH), "SENT");
  }],

  ["header and data rows align at identical byte offsets", () => {
    const entry = makeEntry(0xab, HISTORY_MSG_TYPE_DM, 1_700_000_000, "hi", false);
    const iw = idxWidth(11); // two-digit max index (11 exported entries)
    const header = historyHeader(iw);
    const line = formatHistoryLine(10, entry, iw);
    const o = offsets(iw);
    assert.equal(columnAt(header, o.idx, iw), "idx");
    assert.equal(columnAt(line, o.idx, iw), "10");
    assert.equal(columnAt(header, o.ts, TIMESTAMP_WIDTH), "timestamp");
    assert.equal(columnAt(line, o.ts, TIMESTAMP_WIDTH), formatLocalTimestamp(1_700_000_000));
    assert.equal(columnAt(header, o.ty, TYPE_WIDTH), "type");
    assert.equal(columnAt(line, o.ty, TYPE_WIDTH), "DM");
    assert.equal(columnAt(header, o.dr, DIR_WIDTH), "dir");
    assert.equal(columnAt(line, o.dr, DIR_WIDTH), "RECV");
    assert.equal(columnAt(header, o.fr, FROM_WIDTH), "from");
    assert.equal(columnAt(line, o.fr, FROM_WIDTH), "0xAB");
    assert.equal(header.slice(o.tx), "text");
    assert.equal(line.slice(o.tx), "hi");
  }],

  ["idxWidth floors at the header width for small exports", () => {
    assert.equal(idxWidth(0), 3);
    assert.equal(idxWidth(1), 3); // max index 0
    assert.equal(idxWidth(9), 3); // max index 8, still 1 digit
  }],

  ["idxWidth grows for wide indices", () => {
    assert.equal(idxWidth(1000), 3); // max index 999, 3 digits == header width
    assert.equal(idxWidth(1001), 4); // max index 1000, 4 digits > header width
  }],

  ["columns are separated by two spaces", () => {
    const iw = idxWidth(1);
    assert.ok(historyHeader(iw).includes("  "), "expected two-space separator in header");
    const line = formatHistoryLine(0, makeEntry(0x01, HISTORY_MSG_TYPE_DM, 0, "x"), iw);
    assert.ok(line.includes("  "), "expected two-space separator in data row");
  }],

  ["formatHistoryTranscript renders the empty-history case", () => {
    assert.equal(formatHistoryTranscript([]), "no history entries");
  }],

  ["formatHistoryTranscript renders header + oldest-first rows", () => {
    const entries = [
      makeEntry(0x11, HISTORY_MSG_TYPE_DM, 1_700_000_000, "first", false),
      makeEntry(0x22, HISTORY_MSG_TYPE_GRP_TXT, 1_700_000_001, "second", true),
    ];
    const transcript = formatHistoryTranscript(entries);
    const lines = transcript.split("\n");
    assert.equal(lines.length, 3, "expected header + 2 rows");
    assert.equal(lines[0], historyHeader(idxWidth(2)));
    assert.equal(lines[1], formatHistoryLine(0, entries[0], idxWidth(2)));
    assert.equal(lines[2], formatHistoryLine(1, entries[1], idxWidth(2)));
    // Oldest-first: index 0 row precedes index 1 row.
    assert.ok(lines[1].includes("first"));
    assert.ok(lines[2].includes("second"));
  }],
];

for (const [name, fn] of tests) {
  fn();
  console.log(`  ok — ${name}`);
}

console.log(`history-format: OK — ${tests.length} test(s) passed.`);
