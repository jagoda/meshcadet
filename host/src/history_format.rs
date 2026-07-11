// SPDX-License-Identifier: GPL-3.0-only
//! Human-facing rendering for `export-history` output.
//!
//! Not a documented machine format — columns are fixed-width, left-aligned,
//! and space-padded for terminal readability, not for scripted parsing.
//! (Tab stops don't work here: the ~26-char timestamp column overruns an
//! 8-char tab stop, so a tab after the short header word "timestamp" lands
//! at a different stop than a tab after a full timestamp value.)

use chrono::{Local, TimeZone};
use protocol::history::{HistoryEntry, HistoryMsgType};

/// Number of characters in the rendered timestamp column
/// (see [`format_local_timestamp`]: `"YYYY-MM-DD HH:MM:SS +HH:MM"`, always 26
/// chars — the `%:z` offset is always sign + 2 digits + `:` + 2 digits).
const TIMESTAMP_WIDTH: usize = 26;

/// Width of the `type` column — sized to the header label itself, which is
/// wider than either data value (`"DM"`, `"GRP"`).
const TYPE_WIDTH: usize = 4;

/// Width of the `from` column — sized to the header label itself, which
/// matches the data width exactly (`0x` + 2 hex digits = 4 chars).
const FROM_WIDTH: usize = 4;

/// Width of the `dir` column — sized to the data values (`"SENT"` / `"RECV"`,
/// both 4 chars), which are wider than the header label itself.
const DIR_WIDTH: usize = 4;

/// Minimum width of the `idx` column — the header label itself.
const IDX_HEADER_WIDTH: usize = 3;

/// Two spaces between columns reads more clearly than one in a terminal.
const COL_SEP: &str = "  ";

/// Width of the `idx` column for a given entry count: sized to the largest
/// index that will actually be printed (`entry_count - 1`), floored at the
/// header label's own width so short exports don't waste space.
pub fn idx_width(entry_count: usize) -> usize {
    let max_index = entry_count.saturating_sub(1);
    max_index.to_string().len().max(IDX_HEADER_WIDTH)
}

/// Header row matching the columns emitted by [`format_history_line`].
///
/// `idx_width` must be the same value passed to [`format_history_line`] for
/// every row in the same export, e.g. from [`idx_width`], so the header
/// lines up with the data.
pub fn history_header(idx_width: usize) -> String {
    format!(
        "{:<iw$}{sep}{:<tw$}{sep}{:<yw$}{sep}{:<dw$}{sep}{:<fw$}{sep}text",
        "idx",
        "timestamp",
        "type",
        "dir",
        "from",
        iw = idx_width,
        tw = TIMESTAMP_WIDTH,
        yw = TYPE_WIDTH,
        dw = DIR_WIDTH,
        fw = FROM_WIDTH,
        sep = COL_SEP,
    )
}

/// Render a stored `u32` unix-epoch-seconds value as a local, unambiguous,
/// human-readable timestamp (ISO-8601-ish with a UTC offset), e.g.
/// `2026-07-01 18:34:25 -04:00`.
///
/// Device timestamps are trusted as genuine epochs here — device-clock
/// discipline (making that assumption actually hold) is a separate, deferred
/// concern.
pub fn format_local_timestamp(epoch_seconds: u32) -> String {
    // Converting *from* an epoch instant is always unambiguous (unlike
    // constructing a datetime from local calendar fields, which can land in
    // a DST gap/overlap) — `Local` always resolves this to `Single`.
    let dt = Local
        .timestamp_opt(epoch_seconds as i64, 0)
        .single()
        .expect("epoch-to-instant conversion is always unambiguous");
    dt.format("%Y-%m-%d %H:%M:%S %:z").to_string()
}

/// Render one `export-history` row: index, local timestamp, message type,
/// direction, sender hash, and text.
///
/// `is_ours` distinguishes a message this device sent (`"SENT"`) from one it
/// received (`"RECV"`). This is necessary because the `from` column is always the
/// *conversation* hash (contact/channel) regardless of direction, so it alone
/// cannot tell an outbound entry from an inbound one.
///
/// `idx_width` must match the width used for [`history_header`] in the same
/// export (see [`idx_width`]) so the row lines up with the header. `text` is
/// the final column and is left unpadded — it may contain wide characters
/// (e.g. emoji) that would otherwise throw off later columns, and there are
/// no columns after it to misalign.
pub fn format_history_line(
    index: usize,
    entry: &HistoryEntry,
    is_ours: bool,
    idx_width: usize,
) -> String {
    let type_str = match entry.msg_type {
        HistoryMsgType::Dm => "DM",
        HistoryMsgType::GrpTxt => "GRP",
    };
    let dir_str = if is_ours { "SENT" } else { "RECV" };
    let text =
        std::str::from_utf8(&entry.text[..entry.text_len as usize]).unwrap_or("<invalid utf-8>");
    let from = format!("0x{:02X}", entry.sender_hash);
    format!(
        "{:<iw$}{sep}{:<tw$}{sep}{:<yw$}{sep}{:<dw$}{sep}{:<fw$}{sep}{}",
        index,
        format_local_timestamp(entry.timestamp),
        type_str,
        dir_str,
        from,
        text,
        iw = idx_width,
        tw = TIMESTAMP_WIDTH,
        yw = TYPE_WIDTH,
        dw = DIR_WIDTH,
        fw = FROM_WIDTH,
        sep = COL_SEP,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_local_timestamp_is_deterministic() {
        assert_eq!(
            format_local_timestamp(1_700_000_000),
            format_local_timestamp(1_700_000_000)
        );
    }

    #[test]
    fn format_local_timestamp_distinguishes_different_epochs() {
        assert_ne!(
            format_local_timestamp(0),
            format_local_timestamp(1_700_000_000)
        );
    }

    #[test]
    fn format_local_timestamp_has_expected_shape() {
        // "YYYY-MM-DD HH:MM:SS +HH:MM" (or "-HH:MM") — 26 chars, space-
        // separated date/time/offset, offset always signed with a colon.
        let s = format_local_timestamp(1_700_000_000);
        assert_eq!(s.len(), 26, "unexpected length: {s:?}");
        assert_eq!(s.as_bytes()[10], b' ');
        assert_eq!(s.as_bytes()[19], b' ');
        let sign = s.as_bytes()[20];
        assert!(sign == b'+' || sign == b'-', "missing offset sign: {s:?}");
        assert_eq!(s.as_bytes()[23], b':');
    }

    fn make_entry(
        sender_hash: u8,
        msg_type: HistoryMsgType,
        timestamp: u32,
        text: &[u8],
    ) -> HistoryEntry {
        let text_len = text.len() as u8;
        let mut text_buf = [0u8; protocol::history::MAX_HISTORY_TEXT_LEN];
        text_buf[..text.len()].copy_from_slice(text);
        HistoryEntry {
            sender_hash,
            msg_type,
            timestamp,
            text: text_buf,
            text_len,
        }
    }

    /// Slice out the fixed-width column starting at byte `start`, trimming
    /// the trailing pad spaces. Mirrors the layout `format_history_line` and
    /// `history_header` both write, so it doubles as the "these two line up"
    /// check: if either producer drifted from the column-offset scheme,
    /// slicing at these offsets would return garbage instead of the field.
    fn column_at(line: &str, start: usize, width: usize) -> &str {
        line[start..start + width].trim_end()
    }

    /// Byte offsets of each column's start, given an `idx` column width.
    /// `(idx, timestamp, type, dir, from, text)`.
    fn offsets(iw: usize) -> (usize, usize, usize, usize, usize, usize) {
        let ts = iw + COL_SEP.len();
        let ty = ts + TIMESTAMP_WIDTH + COL_SEP.len();
        let dr = ty + TYPE_WIDTH + COL_SEP.len();
        let fr = dr + DIR_WIDTH + COL_SEP.len();
        let tx = fr + FROM_WIDTH + COL_SEP.len();
        (0, ts, ty, dr, fr, tx)
    }

    #[test]
    fn format_history_line_renders_all_columns() {
        let entry = make_entry(0xAB, HistoryMsgType::Dm, 1_700_000_000, b"hello");
        let iw = idx_width(4); // simulate a 4-entry export (indices 0..=3)
        let line = format_history_line(3, &entry, false, iw);
        let (idx_off, ts_off, ty_off, dr_off, fr_off, tx_off) = offsets(iw);
        assert_eq!(column_at(&line, idx_off, iw), "3");
        assert_eq!(
            column_at(&line, ts_off, TIMESTAMP_WIDTH),
            format_local_timestamp(1_700_000_000)
        );
        assert_eq!(column_at(&line, ty_off, TYPE_WIDTH), "DM");
        assert_eq!(column_at(&line, dr_off, DIR_WIDTH), "RECV");
        assert_eq!(column_at(&line, fr_off, FROM_WIDTH), "0xAB");
        assert_eq!(&line[tx_off..], "hello");
    }

    #[test]
    fn format_history_line_renders_grp_txt_type() {
        let entry = make_entry(0x01, HistoryMsgType::GrpTxt, 0, b"g");
        let iw = idx_width(1);
        let line = format_history_line(0, &entry, false, iw);
        let (_, _, ty_off, _, _, _) = offsets(iw);
        assert_eq!(column_at(&line, ty_off, TYPE_WIDTH), "GRP");
    }

    /// Acceptance: `is_ours=true` renders as `SENT` in the `dir` column, distinguishing
    /// an outbound entry from an inbound one even though `from` shows the
    /// same conversation hash either way.
    #[test]
    fn format_history_line_renders_sent_direction() {
        let entry = make_entry(0x67, HistoryMsgType::Dm, 0, b"hi");
        let iw = idx_width(1);
        let line = format_history_line(0, &entry, true, iw);
        let (_, _, _, dr_off, _, _) = offsets(iw);
        assert_eq!(column_at(&line, dr_off, DIR_WIDTH), "SENT");
    }

    #[test]
    fn header_and_data_row_columns_align_at_the_same_byte_offsets() {
        // The defect this module fixes: header and data rows must land their
        // column starts at identical byte offsets regardless of terminal tab
        // stops. Assert it directly rather than just eyeballing output.
        let entry = make_entry(0xAB, HistoryMsgType::Dm, 1_700_000_000, b"hi");
        let iw = idx_width(11); // two-digit max index, e.g. 11 exported entries
        let header = history_header(iw);
        let line = format_history_line(10, &entry, false, iw);
        let (idx_off, ts_off, ty_off, dr_off, fr_off, tx_off) = offsets(iw);
        assert_eq!(column_at(&header, idx_off, iw), "idx");
        assert_eq!(column_at(&line, idx_off, iw), "10");
        assert_eq!(column_at(&header, ts_off, TIMESTAMP_WIDTH), "timestamp");
        assert_eq!(
            column_at(&line, ts_off, TIMESTAMP_WIDTH),
            format_local_timestamp(1_700_000_000)
        );
        assert_eq!(column_at(&header, ty_off, TYPE_WIDTH), "type");
        assert_eq!(column_at(&line, ty_off, TYPE_WIDTH), "DM");
        assert_eq!(column_at(&header, dr_off, DIR_WIDTH), "dir");
        assert_eq!(column_at(&line, dr_off, DIR_WIDTH), "RECV");
        assert_eq!(column_at(&header, fr_off, FROM_WIDTH), "from");
        assert_eq!(column_at(&line, fr_off, FROM_WIDTH), "0xAB");
        assert_eq!(&header[tx_off..], "text");
        assert_eq!(&line[tx_off..], "hi");
    }

    #[test]
    fn idx_width_floors_at_header_width_for_small_exports() {
        assert_eq!(idx_width(0), IDX_HEADER_WIDTH);
        assert_eq!(idx_width(1), IDX_HEADER_WIDTH); // max index 0
        assert_eq!(idx_width(9), IDX_HEADER_WIDTH); // max index 8, still 1 digit
    }

    #[test]
    fn idx_width_grows_for_wide_indices() {
        assert_eq!(idx_width(1000), 3); // max index 999, 3 digits == header width
        assert_eq!(idx_width(1001), 4); // max index 1000, 4 digits > header width
    }

    #[test]
    fn columns_are_separated_by_two_spaces() {
        let entry = make_entry(0x01, HistoryMsgType::Dm, 0, b"x");
        let iw = idx_width(1);
        let header = history_header(iw);
        assert!(
            header.contains("  "),
            "expected two-space column separator in header"
        );
        let line = format_history_line(0, &entry, false, iw);
        assert!(
            line.contains("  "),
            "expected two-space column separator in data row"
        );
    }
}
