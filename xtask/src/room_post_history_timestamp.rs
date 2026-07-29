// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard pinning `meshcadet-room-clock-ux`'s fix for the
//! room-post LOCAL outbound-echo history timestamp in `firmware/src/main.rs`.
//!
//! # Why this lives in `xtask` and not in `firmware`
//!
//! Same reason the other scanners in this crate do (see
//! `xtask::room_reflood_cadence`'s module doc): the `firmware` crate's
//! single `[[bin]]` target sets `harness = false`, so a `#[test]` inside
//! `firmware/src/main.rs` is type-checked but never EXECUTED by `cargo
//! test`. This module is the host-runnable equivalent, in the same
//! "plain text scanning, no esp toolchain" spirit. The pure decision this
//! call site delegates to (`room_session::room_post_history_timestamp`)
//! already has direct `#[test]` coverage in `firmware-core` — this scanner
//! closes the OTHER half of the gap: proving the ESP-IDF-only call site
//! actually *uses* that decision rather than reverting to the wire nonce
//! inline, which no host-runnable test can otherwise observe.
//!
//! # The invariant being pinned
//!
//! The room-post send path's `append_history(room.hash, ..., <timestamp>,
//! ...)` call (`meshcadet-room-clock-ux`'s Objective, item 1 — "the real
//! bug") must pass `room_session::room_post_history_timestamp(
//! room_wall_clock_secs)`, NEVER `candidate_ts` (the room's monotonic
//! anti-replay wire nonce — see `room_session::room_tx_timestamp`'s "never a
//! clock reading" contract) directly. Before the fix this landed, a
//! GPS-denied device rendered its own room posts at a fabricated date in its
//! own thread — every other client in the room saw the correct,
//! server-re-stamped time fine, because only the LOCAL echo of OUR OWN send
//! ever read the nonce as if it were a clock.
//!
//! # Scope and honest limits
//!
//! Structural, not behavioural: it checks which identifiers appear inside
//! the `append_history` call's own argument list, not that
//! `room_post_history_timestamp` itself is implemented correctly — that
//! behaviour is what `firmware_core::room_session`'s own `#[test]`s for
//! `room_post_history_timestamp` pin. It fails loud (a reported violation,
//! never a silent skip) if the call site can't be located at all, per this
//! crate's "parse gap = NO-GO" doctrine.

use std::fs;
use std::path::Path;

use crate::tokenize;

/// Path, relative to the repo root, of the file this module scans.
pub const MAIN_RS_REL_PATH: &str = "firmware/src/main.rs";

/// Char-index positions of every occurrence of `needle` in `hay`.
fn find_all(hay: &[char], needle: &str) -> Vec<usize> {
    let pat: Vec<char> = needle.chars().collect();
    if pat.is_empty() || hay.len() < pat.len() {
        return Vec::new();
    }
    (0..=hay.len() - pat.len())
        .filter(|&i| hay[i..i + pat.len()] == pat[..])
        .collect()
}

/// Given the char-index of the `(` that opens a call's argument list, return
/// the char-index of its matching `)` — simple depth-counting over already
/// comment/string-masked text (no nested strings/comments to worry about).
/// `None` if the parens never balance before the text runs out.
fn matching_close_paren(chars: &[char], open_paren_pos: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, &c) in chars.iter().enumerate().skip(open_paren_pos) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Scan already-read source text and return every contract violation. Split
/// from [`check`] so the tests can drive it with synthetic sources.
pub fn check_source(src: &str) -> Vec<String> {
    let masked = tokenize(src).masked;
    let chars: Vec<char> = masked.chars().collect();

    // Disambiguate the room-post send path's `append_history` call from
    // every other call site (`SendDm`/`SendGroupMsg`/inbound receive
    // handlers) via `record_sent_timestamp(candidate_ts)` immediately
    // preceding it — unique in the whole file (it's the one and only place
    // a room session's TX watermark is advanced on a successful send), pure
    // CODE rather than a string-literal log message (this scanner runs over
    // comment/string-MASKED text, so an anchor living inside a `"..."` would
    // never match), and only ever reached right after a room post actually
    // reaches the wire (`Ok((n, ack)) =>` arm of `encode_room_post_checked`).
    let anchor = "record_sent_timestamp(candidate_ts)";
    let anchor_hits = find_all(&chars, anchor);
    let anchor_pos = match anchor_hits.len() {
        1 => anchor_hits[0],
        0 => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: no `{anchor}` call found — this scanner can no longer \
                 locate the room-post send-confirmation arm"
            )]
        }
        n => {
            return vec![format!(
                "{MAIN_RS_REL_PATH}: {n} occurrences of `{anchor}` (expected exactly one) — \
                 this scanner cannot disambiguate the send-confirmation arm"
            )]
        }
    };

    // The room-post history append is the first `append_history(` call
    // after that anchor — bounded to a generous window so an unrelated,
    // much-later call site in the file is never mistaken for it.
    const SEARCH_WINDOW: usize = 4000;
    let window_end = (anchor_pos + SEARCH_WINDOW).min(chars.len());
    let call_needle = "append_history(";
    let call_hits: Vec<usize> = find_all(&chars[anchor_pos..window_end], call_needle)
        .into_iter()
        .map(|rel| anchor_pos + rel)
        .collect();
    let Some(&call_pos) = call_hits.first() else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: no `{call_needle}` call found within {SEARCH_WINDOW} chars \
             after the room-post TX log line — this scanner can no longer locate the room-post \
             history append"
        )];
    };

    let open_paren = call_pos + call_needle.chars().count() - 1;
    let Some(close_paren) = matching_close_paren(&chars, open_paren) else {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: could not find the matching `)` for the room-post \
             `append_history(` call — parens don't balance within this scanner's search window"
        )];
    };

    let args: String = chars[open_paren + 1..close_paren].iter().collect();

    let mut violations = Vec::new();
    if !args.contains("room_post_history_timestamp") {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: the room-post `append_history` call no longer passes \
             `room_session::room_post_history_timestamp(..)` as its timestamp argument — a \
             history entry for our own outbound post must never be sourced from anything else, \
             see that function's doc"
        ));
    }
    if args.contains("candidate_ts") {
        violations.push(format!(
            "{MAIN_RS_REL_PATH}: the room-post `append_history` call references `candidate_ts` \
             (the room's monotonic anti-replay wire nonce) directly in its argument list — this \
             is `meshcadet-room-clock-ux`'s exact regression: our own posts would again render \
             at a fabricated date in our own thread on a GPS-denied device, see \
             `room_session::room_tx_timestamp`'s \"never a clock reading\" contract"
        ));
    }

    violations
}

/// Read `firmware/src/main.rs` under `repo_root` and return every contract
/// violation. Empty vec == the contract holds.
pub fn check(repo_root: &Path) -> Vec<String> {
    let path = repo_root.join(MAIN_RS_REL_PATH);
    match fs::read_to_string(&path) {
        Ok(src) => check_source(&src),
        Err(e) => vec![format!("reading {}: {e}", path.display())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual guard: the shipped `firmware/src/main.rs` sources the
    /// room-post history timestamp from `room_post_history_timestamp`, never
    /// `candidate_ts`.
    #[test]
    fn room_post_history_timestamp_contract_holds() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "room-post history timestamp contract violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    const CLEAN_BASELINE: &str = r#"
        Ok((n, ack)) => {
            log_tx_queue_eviction(txq.enqueue(&frame_buf[..n]), "room post");
            room.pending_post_ack = Some(ack);
            room.session.record_sent_timestamp(candidate_ts);
            log::info!(
                "TX room post to 0x{:02x}: {:?} ({} bytes)",
                room.hash, text, n,
            );
            append_history(
                room.hash,
                protocol::history::HistoryMsgType::Dm,
                room_session::room_post_history_timestamp(
                    room_wall_clock_secs,
                ),
                text.as_bytes(),
                true,
                false,
            );
        }
    "#;

    #[test]
    fn synthetic_clean_baseline_is_clean() {
        assert!(
            check_source(CLEAN_BASELINE).is_empty(),
            "{:?}",
            check_source(CLEAN_BASELINE)
        );
    }

    /// REGRESSION: reverting to `candidate_ts` (the wire nonce) directly —
    /// the exact pre-fix defect — must be caught.
    #[test]
    fn synthetic_regression_to_the_wire_nonce_is_caught() {
        let synthetic = r#"
            Ok((n, ack)) => {
                room.session.record_sent_timestamp(candidate_ts);
                log::info!(
                    "TX room post to 0x{:02x}: {:?} ({} bytes)",
                    room.hash, text, n,
                );
                append_history(
                    room.hash,
                    protocol::history::HistoryMsgType::Dm,
                    candidate_ts,
                    text.as_bytes(),
                    true,
                    false,
                );
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("references `candidate_ts`")),
            "a room-post history append sourced directly from `candidate_ts` must be caught: \
             {violations:?}"
        );
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no longer passes `room_session::room_post_history_timestamp")),
            "the same regression must also be flagged as missing the required helper call: \
             {violations:?}"
        );
    }

    /// A DIFFERENT regression shape: the call is dropped entirely in favor
    /// of some other ad-hoc expression (not `candidate_ts` by name, but
    /// still not the required helper) — must also be caught, not just the
    /// literal `candidate_ts` shape.
    #[test]
    fn synthetic_missing_helper_call_is_caught() {
        let synthetic = r#"
            Ok((n, ack)) => {
                room.session.record_sent_timestamp(candidate_ts);
                log::info!(
                    "TX room post to 0x{:02x}: {:?} ({} bytes)",
                    room.hash, text, n,
                );
                append_history(
                    room.hash,
                    protocol::history::HistoryMsgType::Dm,
                    room_wall_clock_secs.unwrap_or(0),
                    text.as_bytes(),
                    true,
                    false,
                );
            }
        "#;
        let violations = check_source(synthetic);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no longer passes `room_session::room_post_history_timestamp")),
            "{violations:?}"
        );
    }

    #[test]
    fn a_missing_anchor_is_a_violation_not_a_silent_pass() {
        let violations = check_source("fn main() {}");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no `record_sent_timestamp(candidate_ts)` call found")),
            "{violations:?}"
        );
    }

    #[test]
    fn comment_mention_of_candidate_ts_inside_the_call_is_not_counted() {
        // A doc comment mentioning `candidate_ts` inside the append_history
        // call's argument list must not itself trip the guard — only CODE
        // usage should (masked comments/strings are blanked before
        // scanning).
        let synthetic = r#"
            Ok((n, ack)) => {
                room.session.record_sent_timestamp(candidate_ts);
                log::info!(
                    "TX room post to 0x{:02x}: {:?} ({} bytes)",
                    room.hash, text, n,
                );
                append_history(
                    room.hash,
                    protocol::history::HistoryMsgType::Dm,
                    // NEVER candidate_ts here — see room_post_history_timestamp's doc.
                    room_session::room_post_history_timestamp(
                        room_wall_clock_secs,
                    ),
                    text.as_bytes(),
                    true,
                    false,
                );
            }
        "#;
        assert!(
            check_source(synthetic).is_empty(),
            "{:?}",
            check_source(synthetic)
        );
    }
}
