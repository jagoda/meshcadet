// history-format.js — human-facing rendering for the provisioner page's
// `export-history` transcript, a DOM-free port of `host/src/history_format.rs`.
//
// Produces the SAME fixed-width, left-aligned, space-padded columns the CLI
// prints (`idx  timestamp  type  dir  from  text`), so a transcript downloaded
// from the browser reads identically to one piped from `meshcadet
// export-history`. Not a documented machine format — the columns are for
// human readability, not scripted parsing (same caveat as the Rust module).
//
// DOM-free (like contact-uri.js / validation.js) so it's testable under plain
// `node` via history-format.test.mjs — no `document`/`navigator` top-level
// side effects.
//
// ADR-0007 security model: this module only *formats* history entries into a
// string; it never logs, persists, or transmits them. The private message
// text it renders stays wherever the caller puts the returned string — for
// the provisioner page, an explicit user-initiated local download only.
//
// No build step: plain ES module, loaded directly by the browser or by `node`
// for the test.

import { HISTORY_MSG_TYPE_DM } from "./codec.js";

// ── Column widths (mirror host/src/history_format.rs's consts) ──────────────

// "YYYY-MM-DD HH:MM:SS +HH:MM" — always 26 chars (see formatLocalTimestamp).
const TIMESTAMP_WIDTH = 26;
// `type` column — sized to the header label, wider than "DM"/"GRP".
const TYPE_WIDTH = 4;
// `from` column — sized to the header label, matches "0xNN" (4 chars) exactly.
const FROM_WIDTH = 4;
// `dir` column — sized to "SENT"/"RECV" (4 chars), wider than the header label.
const DIR_WIDTH = 4;
// Minimum `idx` column width — the header label itself.
const IDX_HEADER_WIDTH = 3;
// Two spaces between columns reads more clearly than one.
const COL_SEP = "  ";

function padEnd(str, width) {
  return str.length >= width ? str : str + " ".repeat(width - str.length);
}

function pad2(n) {
  return String(n).padStart(2, "0");
}

/**
 * Width of the `idx` column for a given entry count: sized to the largest
 * index that will actually be printed (`entryCount - 1`), floored at the
 * header label's own width so short exports don't waste space. Mirrors
 * `history_format::idx_width`.
 */
export function idxWidth(entryCount) {
  const maxIndex = Math.max(entryCount - 1, 0);
  return Math.max(String(maxIndex).length, IDX_HEADER_WIDTH);
}

/**
 * Header row matching the columns emitted by `formatHistoryLine`. `iw` must
 * be the same value passed to `formatHistoryLine` for every row in the same
 * export (e.g. from `idxWidth`) so the header lines up with the data.
 * Mirrors `history_format::history_header`.
 */
export function historyHeader(iw) {
  return (
    padEnd("idx", iw) + COL_SEP +
    padEnd("timestamp", TIMESTAMP_WIDTH) + COL_SEP +
    padEnd("type", TYPE_WIDTH) + COL_SEP +
    padEnd("dir", DIR_WIDTH) + COL_SEP +
    padEnd("from", FROM_WIDTH) + COL_SEP +
    "text"
  );
}

/**
 * Render a stored `u32` unix-epoch-seconds value as a local, unambiguous,
 * human-readable timestamp, e.g. `2026-07-01 18:34:25 -04:00`. Mirrors
 * `history_format::format_local_timestamp` (`%Y-%m-%d %H:%M:%S %:z`), using
 * the browser's local timezone via `Date` — always 26 chars.
 *
 * Device timestamps are trusted as genuine epochs here (same assumption the
 * Rust module documents); device-clock discipline is a separate concern.
 */
export function formatLocalTimestamp(epochSeconds) {
  const d = new Date(epochSeconds * 1000);
  const date = `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
  const time = `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
  // getTimezoneOffset returns minutes *behind* UTC (positive when local is
  // behind UTC), so negate to get the conventional signed offset.
  const offsetMin = -d.getTimezoneOffset();
  const sign = offsetMin >= 0 ? "+" : "-";
  const abs = Math.abs(offsetMin);
  return `${date} ${time} ${sign}${pad2(Math.floor(abs / 60))}:${pad2(abs % 60)}`;
}

/**
 * Render one export row: index, local timestamp, message type, direction,
 * sender hash, and text. `entry` is a decoded `RSP_HISTORY_ENTRY` object (see
 * `codec.js`'s `decodeRspHistoryEntry`) — `entry.is_ours` selects `SENT` vs
 * `RECV` (necessary because `from` is always the conversation hash regardless
 * of direction). `iw` must match the width used for `historyHeader` in the
 * same export. `text` is the final column, left unpadded (it may contain wide
 * characters, and no columns follow it). Mirrors
 * `history_format::format_history_line`.
 */
export function formatHistoryLine(index, entry, iw) {
  // Only DM/GRP_TXT reach here — decodeRspHistoryEntry rejects other msg_type
  // bytes (returns null) before an entry is ever handed to this formatter.
  const typeStr = entry.msg_type === HISTORY_MSG_TYPE_DM ? "DM" : "GRP";
  const dirStr = entry.is_ours ? "SENT" : "RECV";
  const from = `0x${entry.sender_hash.toString(16).toUpperCase().padStart(2, "0")}`;
  return (
    padEnd(String(index), iw) + COL_SEP +
    padEnd(formatLocalTimestamp(entry.timestamp), TIMESTAMP_WIDTH) + COL_SEP +
    padEnd(typeStr, TYPE_WIDTH) + COL_SEP +
    padEnd(dirStr, DIR_WIDTH) + COL_SEP +
    padEnd(from, FROM_WIDTH) + COL_SEP +
    entry.text
  );
}

/**
 * Build the full downloadable transcript from decoded history entries
 * (oldest-first, as `session.exportHistory` returns them): a header row
 * followed by one `formatHistoryLine` per entry. Mirrors the CLI's
 * `Cmd::ExportHistory` output, including the empty-history case.
 */
export function formatHistoryTranscript(entries) {
  if (entries.length === 0) {
    return "no history entries";
  }
  const iw = idxWidth(entries.length);
  const lines = [historyHeader(iw)];
  entries.forEach((entry, i) => {
    lines.push(formatHistoryLine(i, entry, iw));
  });
  return lines.join("\n");
}
