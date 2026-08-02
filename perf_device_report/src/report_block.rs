// SPDX-License-Identifier: GPL-3.0-only
//! Parses `docs/perf/collection-kit.md` §9's report-back block: a header of
//! named fields followed by a raw serial-console capture, delimited by
//! literal marker lines. Exact shape (from that document):
//!
//! ```text
//! kit_version: 1
//! build_ref: <short SHA[-dirty]>
//! capture_date: <YYYY-MM-DD, UTC>
//! section: baseline | calibration | stack-hwm | felt-snappiness | two-device-delivery
//! payload_bytes: <10 | 40 | 100 | 255 | n/a>
//! ui_load: <idle | navigating | n/a>
//! peer_present: <yes | no>
//! notes: <optional free text>
//! --- raw-serial-log ---
//! <chronological serial console capture>
//! --- end-raw-serial-log ---
//! ```
//!
//! (the whole thing is normally wrapped in a ` ```meshcadet-perf-report `
//! / ` ``` ` markdown fence when pasted into a doc or tracking note — this
//! parser accepts the block with or without that fence, see
//! [`parse_report_blocks`]'s "works without markdown fences" test).
//!
//! A single document (a tracking note's `## Notes` section, a plain text
//! file) may carry more than one block back to back — §9 says up to 6 additional
//! blocks for Part G's payload/ui-load sweep on top of the single-device
//! baseline block. [`parse_report_blocks`] finds and parses every one it
//! sees, independently; one malformed block does not lose the others (its
//! error is attached to its own position, the well-formed ones still
//! parse).

use std::fmt;

/// Which of the kit's §9 sections a block reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Baseline,
    Calibration,
    StackHwm,
    FeltSnappiness,
    TwoDeviceDelivery,
}

impl Section {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "baseline" => Some(Section::Baseline),
            "calibration" => Some(Section::Calibration),
            "stack-hwm" => Some(Section::StackHwm),
            "felt-snappiness" => Some(Section::FeltSnappiness),
            "two-device-delivery" => Some(Section::TwoDeviceDelivery),
            _ => None,
        }
    }

    /// Lowercase, hyphenated form — the same text §9 uses, and what
    /// [`crate::archive`] uses for the archived filename.
    pub fn slug(&self) -> &'static str {
        match self {
            Section::Baseline => "baseline",
            Section::Calibration => "calibration",
            Section::StackHwm => "stack-hwm",
            Section::FeltSnappiness => "felt-snappiness",
            Section::TwoDeviceDelivery => "two-device-delivery",
        }
    }

    /// Which `docs/perf/ui-perf-baseline.md` §8 predicates a block of this
    /// section can close, per `docs/perf/collection-kit.md` §0's own
    /// "which part closes what" table. Informational only — closing a
    /// predicate is a human/doc-editing act, not something this crate does
    /// automatically.
    pub fn closes_predicates(&self) -> &'static [&'static str] {
        match self {
            Section::Baseline => &["D1", "D2", "D3 (partial)", "D7"],
            Section::Calibration => &["loop-model swept constants (params.rs)"],
            Section::StackHwm => &["D8"],
            Section::FeltSnappiness => &["D10"],
            Section::TwoDeviceDelivery => &["D4", "D5", "D6"],
        }
    }
}

impl fmt::Display for Section {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// The `ui_load` header field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiLoad {
    Idle,
    Navigating,
    NotApplicable,
}

impl UiLoad {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "idle" => Some(UiLoad::Idle),
            "navigating" => Some(UiLoad::Navigating),
            "n/a" => Some(UiLoad::NotApplicable),
            _ => None,
        }
    }

    pub fn slug(&self) -> &'static str {
        match self {
            UiLoad::Idle => "idle",
            UiLoad::Navigating => "navigating",
            UiLoad::NotApplicable => "na",
        }
    }
}

/// The `payload_bytes` header field. Stored as an option rather than a
/// closed enum: §9's own header comment (`<10 | 40 | 100 | 255 | n/a>`)
/// and Part G's procedure text (`10 B, 40 B and 255 B`) already disagree on
/// the exact enumerated set, so this parser accepts any non-negative
/// integer rather than silently rejecting a legitimate value the kit's own
/// prose allows.
pub type PayloadBytes = Option<u32>;

fn parse_payload_bytes(s: &str) -> Result<PayloadBytes, BlockParseError> {
    if s == "n/a" {
        return Ok(None);
    }
    s.parse::<u32>()
        .map(Some)
        .map_err(|_| BlockParseError::InvalidFieldValue {
            field: "payload_bytes",
            value: s.to_string(),
        })
}

fn payload_bytes_slug(p: PayloadBytes) -> String {
    match p {
        Some(n) => format!("{n}B"),
        None => "na".to_string(),
    }
}

/// One parsed `meshcadet-perf-report` block.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportBlock {
    pub kit_version: u32,
    pub build_ref: String,
    /// `YYYY-MM-DD`, validated for shape (not calendar validity — this
    /// crate has no calendar library dependency and the kit only asks for
    /// UTC date, not full parsing).
    pub capture_date: String,
    pub section: Section,
    pub payload_bytes: PayloadBytes,
    pub ui_load: UiLoad,
    pub peer_present: bool,
    pub notes: Option<String>,
    pub raw_serial_log: String,
}

impl ReportBlock {
    /// The archive filename stem this block would be written under — see
    /// `crate::archive`.
    pub fn archive_stem(&self) -> String {
        format!(
            "{}--{}--{}--{}--{}",
            self.build_ref,
            self.section.slug(),
            payload_bytes_slug(self.payload_bytes),
            self.ui_load.slug(),
            self.capture_date,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockParseError {
    MissingField {
        field: &'static str,
        near_line: usize,
    },
    UnknownField {
        field: String,
        line: usize,
    },
    InvalidFieldValue {
        field: &'static str,
        value: String,
    },
    InvalidCaptureDate {
        value: String,
    },
    MissingRawLogStartMarker {
        near_line: usize,
    },
    MissingRawLogEndMarker {
        near_line: usize,
    },
}

impl fmt::Display for BlockParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockParseError::MissingField { field, near_line } => {
                write!(
                    f,
                    "missing required field `{field}` (block near line {near_line})"
                )
            }
            BlockParseError::UnknownField { field, line } => {
                write!(f, "unrecognized header field `{field}` at line {line}")
            }
            BlockParseError::InvalidFieldValue { field, value } => {
                write!(f, "invalid value for `{field}`: {value:?}")
            }
            BlockParseError::InvalidCaptureDate { value } => {
                write!(f, "capture_date {value:?} is not YYYY-MM-DD")
            }
            BlockParseError::MissingRawLogStartMarker { near_line } => write!(
                f,
                "header never reached `--- raw-serial-log ---` (started near line {near_line})"
            ),
            BlockParseError::MissingRawLogEndMarker { near_line } => write!(
                f,
                "raw log never reached `--- end-raw-serial-log ---` (started near line {near_line})"
            ),
        }
    }
}

impl std::error::Error for BlockParseError {}

fn is_fence_line(line: &str) -> bool {
    line.trim().starts_with("```")
}

fn looks_like_date(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

const RAW_LOG_START: &str = "--- raw-serial-log ---";
const RAW_LOG_END: &str = "--- end-raw-serial-log ---";

#[derive(Default)]
struct PartialHeader {
    kit_version: Option<u32>,
    build_ref: Option<String>,
    capture_date: Option<String>,
    section: Option<Section>,
    payload_bytes: Option<PayloadBytes>,
    ui_load: Option<UiLoad>,
    peer_present: Option<bool>,
    notes: Option<String>,
}

/// Parse every `meshcadet-perf-report` block found in `text`, in document
/// order. Returns one `Result` per block found — a malformed block reports
/// its own [`BlockParseError`] without preventing the rest of `text` from
/// being scanned.
pub fn parse_report_blocks(text: &str) -> Vec<Result<ReportBlock, BlockParseError>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        if !lines[i].trim_start().starts_with("kit_version:") {
            i += 1;
            continue;
        }
        let (result, next_i) = parse_one_block(&lines, i);
        out.push(result);
        // `next_i` always points strictly past whatever this block
        // consumed (header line + raw log, when both were found) — the
        // `.max` is only a defensive floor against an implausible 0-length
        // advance turning this into an infinite loop.
        i = next_i.max(i + 1);
    }

    out
}

/// Parse one block starting at `lines[start]` (a `kit_version:` line).
/// Returns the parsed block (or the error that stopped it) and the line
/// index the outer scan should resume from.
fn parse_one_block(lines: &[&str], start: usize) -> (Result<ReportBlock, BlockParseError>, usize) {
    let header_start_line = start + 1; // 1-based, for error messages
    let mut header = PartialHeader::default();
    let mut j = start;
    let mut found_start_marker = false;

    while j < lines.len() {
        let t = lines[j].trim();
        if t == RAW_LOG_START {
            found_start_marker = true;
            j += 1;
            break;
        }
        if t.is_empty() || is_fence_line(t) {
            j += 1;
            continue;
        }
        let field_result = parse_header_line(t, j + 1)
            .and_then(|(key, value)| apply_header_field(&mut header, key, value, j + 1));
        if let Err(e) = field_result {
            // Resume the outer scan after this block's raw-log region (if
            // one is even findable) so a bad header field in THIS block
            // doesn't also corrupt parsing of the next one.
            let resume_at = skip_to_after_raw_log_end(lines, j);
            return (Err(e), resume_at);
        }
        j += 1;
    }

    if !found_start_marker {
        return (
            Err(BlockParseError::MissingRawLogStartMarker {
                near_line: header_start_line,
            }),
            j,
        );
    }

    let log_start = j;
    while j < lines.len() && lines[j].trim() != RAW_LOG_END {
        j += 1;
    }
    if j >= lines.len() {
        return (
            Err(BlockParseError::MissingRawLogEndMarker {
                near_line: header_start_line,
            }),
            lines.len(),
        );
    }

    let raw_serial_log = lines[log_start..j].join("\n");
    let resume_at = j + 1; // past the end marker
    (
        finish_block(header, header_start_line, raw_serial_log),
        resume_at,
    )
}

/// After a header-field error, skip forward past this block's raw-log
/// region (if the start/end markers are even present) so the outer scan
/// resumes cleanly at the next block rather than re-parsing log lines as
/// headers.
fn skip_to_after_raw_log_end(lines: &[&str], mut j: usize) -> usize {
    while j < lines.len() && lines[j].trim() != RAW_LOG_START {
        j += 1;
    }
    if j < lines.len() {
        j += 1; // past the start marker
        while j < lines.len() && lines[j].trim() != RAW_LOG_END {
            j += 1;
        }
        if j < lines.len() {
            j += 1; // past the end marker
        }
    }
    j
}

fn parse_header_line(line: &str, line_no: usize) -> Result<(&str, &str), BlockParseError> {
    match line.split_once(':') {
        Some((k, v)) => Ok((k.trim(), v.trim())),
        None => Err(BlockParseError::UnknownField {
            field: line.to_string(),
            line: line_no,
        }),
    }
}

fn apply_header_field(
    header: &mut PartialHeader,
    key: &str,
    value: &str,
    line_no: usize,
) -> Result<(), BlockParseError> {
    match key {
        "kit_version" => {
            header.kit_version =
                Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| BlockParseError::InvalidFieldValue {
                            field: "kit_version",
                            value: value.to_string(),
                        })?,
                );
        }
        "build_ref" => header.build_ref = Some(value.to_string()),
        "capture_date" => {
            if !looks_like_date(value) {
                return Err(BlockParseError::InvalidCaptureDate {
                    value: value.to_string(),
                });
            }
            header.capture_date = Some(value.to_string());
        }
        "section" => {
            header.section =
                Some(
                    Section::parse(value).ok_or_else(|| BlockParseError::InvalidFieldValue {
                        field: "section",
                        value: value.to_string(),
                    })?,
                );
        }
        "payload_bytes" => header.payload_bytes = Some(parse_payload_bytes(value)?),
        "ui_load" => {
            header.ui_load =
                Some(
                    UiLoad::parse(value).ok_or_else(|| BlockParseError::InvalidFieldValue {
                        field: "ui_load",
                        value: value.to_string(),
                    })?,
                );
        }
        "peer_present" => {
            header.peer_present = Some(match value {
                "yes" => true,
                "no" => false,
                _ => {
                    return Err(BlockParseError::InvalidFieldValue {
                        field: "peer_present",
                        value: value.to_string(),
                    })
                }
            });
        }
        "notes" => {
            header.notes = if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            };
        }
        other => {
            return Err(BlockParseError::UnknownField {
                field: other.to_string(),
                line: line_no,
            })
        }
    }
    Ok(())
}

fn finish_block(
    header: PartialHeader,
    header_start_line: usize,
    raw_serial_log: String,
) -> Result<ReportBlock, BlockParseError> {
    macro_rules! require {
        ($field:ident, $name:literal) => {
            header.$field.ok_or(BlockParseError::MissingField {
                field: $name,
                near_line: header_start_line,
            })?
        };
    }
    Ok(ReportBlock {
        kit_version: require!(kit_version, "kit_version"),
        build_ref: require!(build_ref, "build_ref"),
        capture_date: require!(capture_date, "capture_date"),
        section: require!(section, "section"),
        payload_bytes: require!(payload_bytes, "payload_bytes"),
        ui_load: require!(ui_load, "ui_load"),
        peer_present: require!(peer_present, "peer_present"),
        notes: header.notes,
        raw_serial_log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_BLOCK: &str = "\
```meshcadet-perf-report
kit_version: 1
build_ref: a1b2c3d
capture_date: 2026-08-02
section: baseline
payload_bytes: n/a
ui_load: n/a
peer_present: no
notes: idle + one nav window
--- raw-serial-log ---
PERF phase=gps: n=1 min=10 mean=10 max=10 p95=10
PERF phase=battery: n=1 min=5 mean=5 max=5 p95=5
--- end-raw-serial-log ---
```
";

    #[test]
    fn parses_one_well_formed_block() {
        let results = parse_report_blocks(ONE_BLOCK);
        assert_eq!(results.len(), 1);
        let block = results[0].clone().expect("should parse");
        assert_eq!(block.kit_version, 1);
        assert_eq!(block.build_ref, "a1b2c3d");
        assert_eq!(block.capture_date, "2026-08-02");
        assert_eq!(block.section, Section::Baseline);
        assert_eq!(block.payload_bytes, None);
        assert_eq!(block.ui_load, UiLoad::NotApplicable);
        assert!(!block.peer_present);
        assert_eq!(block.notes.as_deref(), Some("idle + one nav window"));
        assert!(block.raw_serial_log.contains("PERF phase=gps"));
        assert!(block.raw_serial_log.contains("PERF phase=battery"));
    }

    #[test]
    fn archive_stem_is_stable_and_descriptive() {
        let block = parse_report_blocks(ONE_BLOCK)[0].clone().unwrap();
        assert_eq!(
            block.archive_stem(),
            "a1b2c3d--baseline--na--na--2026-08-02"
        );
    }

    #[test]
    fn parses_multiple_blocks_in_one_document() {
        let doc = format!("{ONE_BLOCK}\nsome commentary in between\n\n{ONE_BLOCK}");
        let results = parse_report_blocks(&doc);
        assert_eq!(results.len(), 2);
        for r in results {
            assert!(r.is_ok());
        }
    }

    #[test]
    fn works_without_markdown_fences() {
        let unfenced = ONE_BLOCK
            .replace("```meshcadet-perf-report\n", "")
            .replace("```\n", "");
        let results = parse_report_blocks(&unfenced);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
    }

    #[test]
    fn missing_raw_log_start_marker_is_reported() {
        let broken = "\
kit_version: 1
build_ref: deadbee
capture_date: 2026-08-02
section: baseline
payload_bytes: n/a
ui_load: n/a
peer_present: no
";
        let results = parse_report_blocks(broken);
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0],
            Err(BlockParseError::MissingRawLogStartMarker { .. })
        ));
    }

    #[test]
    fn missing_raw_log_end_marker_is_reported() {
        let broken = "\
kit_version: 1
build_ref: deadbee
capture_date: 2026-08-02
section: baseline
payload_bytes: n/a
ui_load: n/a
peer_present: no
--- raw-serial-log ---
PERF phase=gps: n=1 min=10 mean=10 max=10 p95=10
";
        let results = parse_report_blocks(broken);
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0],
            Err(BlockParseError::MissingRawLogEndMarker { .. })
        ));
    }

    #[test]
    fn unknown_header_field_is_reported_and_does_not_lose_the_next_block() {
        let doc = format!(
            "kit_version: 1\nbuild_ref: bad\ncapture_date: 2026-08-02\nsection: baseline\npayload_bytes: n/a\nui_load: n/a\npeer_present: no\nbogus_field: oops\n--- raw-serial-log ---\nlog\n--- end-raw-serial-log ---\n\n{ONE_BLOCK}"
        );
        let results = parse_report_blocks(&doc);
        assert_eq!(results.len(), 2);
        assert!(matches!(
            results[0],
            Err(BlockParseError::UnknownField { .. })
        ));
        assert!(results[1].is_ok());
    }

    #[test]
    fn invalid_capture_date_is_reported() {
        let broken = ONE_BLOCK.replace("capture_date: 2026-08-02", "capture_date: not-a-date");
        let results = parse_report_blocks(&broken);
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0],
            Err(BlockParseError::InvalidCaptureDate { .. })
        ));
    }

    #[test]
    fn invalid_section_value_is_reported() {
        let broken = ONE_BLOCK.replace("section: baseline", "section: made-up");
        let results = parse_report_blocks(&broken);
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0],
            Err(BlockParseError::InvalidFieldValue {
                field: "section",
                ..
            })
        ));
    }

    #[test]
    fn payload_bytes_accepts_any_non_negative_integer_not_just_the_headline_four() {
        let block = ONE_BLOCK
            .replace("payload_bytes: n/a", "payload_bytes: 128")
            .replace("ui_load: n/a", "ui_load: idle");
        let results = parse_report_blocks(&block);
        assert_eq!(results[0].as_ref().unwrap().payload_bytes, Some(128));
    }

    #[test]
    fn no_input_produces_no_blocks() {
        assert!(parse_report_blocks("just some prose, no report block here").is_empty());
    }
}
