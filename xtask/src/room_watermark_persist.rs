// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard pinning `meshcadet-room-ts-watermark-write-
//! behind`'s invariant across EVERY `record_sent_timestamp` call site in
//! `firmware/src/main.rs`, present and future.
//!
//! # Why this lives in `xtask` and not in `firmware`
//!
//! Same reason the other scanners in this crate do (see
//! `xtask::room_reflood_cadence`'s module doc): the `firmware` crate's
//! single `[[bin]]` target sets `harness = false`, so a `#[test]` inside
//! `firmware/src/main.rs` is type-checked but never EXECUTED by `cargo
//! test`. This module is the host-runnable equivalent, in the same
//! "plain text scanning, no esp toolchain" spirit.
//!
//! # The invariant being pinned
//!
//! `firmware_core::room_session::RoomSession::record_sent_timestamp`
//! advances the room's monotonic anti-replay TX watermark (`last_room_ts`)
//! in RAM only — it never touches flash. A caller that advances the
//! watermark and then never calls `room_session::save_room_session` for
//! that same room leaves the advance write-behind: a reboot before the
//! next persist resumes from the STALE on-flash value, which is already
//! `<=` a value this device has already handed the room server. The server
//! then silently drops every login/post/keep-alive from this device until
//! the client's +1-per-send climb overtakes the value it already gave the
//! server (worst case ~`ROOM_REFLOOD_BACKOFF_CEILING_MS`-scaled hours).
//!
//! `a56c7b7` (`meshcadet-room-ts-watermark-write-behind`) fixed this at the
//! boot-login, reflood-login, and keep-alive call sites by adding a
//! `save_room_session` immediately after each `record_sent_timestamp` —
//! but missed the room-POST send site (`meshcadet-room-post-watermark-
//! persist`'s own Objective), because nothing enumerated the call sites
//! structurally; the original fix was reviewed against a hand-maintained
//! list of known sites, and a list is exactly the kind of artifact that
//! silently goes stale the moment a NEW call site is added. This scanner
//! instead finds every `<var>.session.record_sent_timestamp(` occurrence
//! BY SHAPE (a regex over the call's own syntax, not a fixed count or a
//! set of expected line numbers) and asserts, for each one found, that a
//! `save_room_session(..., &<var>.session)` call for THE SAME room
//! variable appears somewhere after it before the next such site (or
//! end of file) — so a future fifth call site is covered automatically,
//! with no scanner update required, and dropping the persist at any site
//! (existing or new) fails loud rather than silently reintroducing this
//! defect class.
//!
//! # Scope and honest limits
//!
//! Structural, not behavioural: it checks that a persist call referencing
//! the same room's `.session` field textually follows each watermark
//! advance, not that `save_room_session` itself writes flash correctly —
//! that behaviour is `room_session`'s own concern. It fails loud (a
//! reported violation, never a silent skip) if no `record_sent_timestamp`
//! call site can be located at all, per this crate's "parse gap = NO-GO"
//! doctrine. A `save_room_session` call that precedes its
//! `record_sent_timestamp` (persisting the PRE-advance watermark rather
//! than the one just recorded) is deliberately NOT credited — only a
//! persist found AFTER the advance satisfies the invariant, so reordering
//! the two statements is caught exactly like dropping the persist
//! outright.

use std::fs;
use std::path::Path;

use regex::Regex;

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

/// One `<var>.session.record_sent_timestamp(` call site: `var` is the room
/// binding name, `end` is the char index right after the call's opening
/// `(` — the point from which this site's forward persist search begins.
struct Site {
    var: String,
    start: usize,
    end: usize,
}

/// Locate every `record_sent_timestamp` call site BY SHAPE — a regex over
/// `<identifier>.session.record_sent_timestamp(`, run against
/// comment/string-masked text so a doc-comment MENTION of the method name
/// (there are several in this file) is never mistaken for a real call —
/// rather than any fixed count or hand-maintained line-number list. Sites
/// are returned in source order.
fn find_sites(masked: &str) -> Vec<Site> {
    let re = Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\.session\.record_sent_timestamp\(").unwrap();
    re.captures_iter(masked)
        .map(|cap| {
            let whole = cap.get(0).unwrap();
            Site {
                var: cap.get(1).unwrap().as_str().to_string(),
                start: whole.start(),
                end: whole.end(),
            }
        })
        .collect()
}

/// Scan already-read source text and return every contract violation. Split
/// from [`check`] so the tests can drive it with synthetic sources.
pub fn check_source(src: &str) -> Vec<String> {
    let masked = tokenize(src).masked;
    let chars: Vec<char> = masked.chars().collect();

    let sites = find_sites(&masked);
    if sites.is_empty() {
        return vec![format!(
            "{MAIN_RS_REL_PATH}: no `<var>.session.record_sent_timestamp(` call site found — \
             this scanner can no longer locate the room TX-watermark advance it's meant to \
             guard"
        )];
    }

    let mut violations = Vec::new();
    let call_needle = "save_room_session(";

    for (idx, site) in sites.iter().enumerate() {
        // Bound the forward search by the NEXT site's start (so a persist
        // that actually belongs to a later call site can never be credited
        // to this one) or end of file for the last site.
        let window_end = sites.get(idx + 1).map(|s| s.start).unwrap_or(chars.len());

        let call_hits: Vec<usize> = find_all(&chars[site.end..window_end], call_needle)
            .into_iter()
            .map(|rel| site.end + rel)
            .collect();

        let needed_arg = format!("&{}.session", site.var);
        let persisted = call_hits.iter().any(|&call_pos| {
            let open_paren = call_pos + call_needle.chars().count() - 1;
            let Some(close_paren) = matching_close_paren(&chars, open_paren) else {
                return false;
            };
            let args: String = chars[open_paren + 1..close_paren].iter().collect();
            args.contains(&needed_arg)
        });

        if !persisted {
            violations.push(format!(
                "{MAIN_RS_REL_PATH}: `{}.session.record_sent_timestamp(..)` at char {} is not \
                 followed (before the next site, or EOF) by a `save_room_session(..., \
                 &{}.session)` call — the advanced watermark is RAM-only here; a reboot before \
                 the next persist can resume below a value already given to the room server \
                 (`meshcadet-room-ts-watermark-write-behind`'s exact defect class). If a \
                 `save_room_session` call for this room DOES appear nearby, check it isn't \
                 BEFORE the `record_sent_timestamp` call — persisting the pre-advance watermark \
                 doesn't satisfy this invariant either.",
                site.var, site.start, site.var,
            ));
        }
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

    /// The actual guard: every room TX-watermark advance in the shipped
    /// `firmware/src/main.rs` is write-through persisted.
    #[test]
    fn every_watermark_advance_is_persisted() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "room watermark write-behind contract violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    /// Sanity check on the LIVE file: this scanner must actually find all
    /// four known call sites (boot login, reflood login, keep-alive, room
    /// post) — if this count ever drops, the regex shape no longer matches
    /// this file's real call syntax and the guard above would be passing
    /// vacuously.
    #[test]
    fn finds_all_four_known_live_sites() {
        let path = crate::repo_root_from_manifest_dir().join(MAIN_RS_REL_PATH);
        let src = fs::read_to_string(&path).unwrap();
        let masked = tokenize(&src).masked;
        let sites = find_sites(&masked);
        assert_eq!(
            sites.len(),
            4,
            "expected exactly 4 record_sent_timestamp call sites (boot login, reflood login, \
             keep-alive, room post) — found {}: this scanner's shape-match may be stale, or a \
             call site was genuinely added/removed and this count needs updating alongside it",
            sites.len()
        );
    }

    const CLEAN_BASELINE: &str = r#"
        for room in room_runtime.iter_mut() {
            room.session.record_sent_timestamp(boot_ts);
            room_session::save_room_session(
                nvs_partition.clone(),
                room.hash,
                room.session_epoch,
                &room.session,
            );
        }
        for room in room_runtime.iter_mut() {
            room.session.record_sent_timestamp(ts);
            room_session::save_room_session(
                nvs_partition.clone(),
                room.hash,
                room.session_epoch,
                &room.session,
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

    /// MUTATION 1: drop the persist entirely at one of two sites — must be
    /// caught, and only for the site actually missing it.
    #[test]
    fn synthetic_dropped_persist_is_caught() {
        let synthetic = r#"
            for room in room_runtime.iter_mut() {
                room.session.record_sent_timestamp(boot_ts);
                room_session::save_room_session(
                    nvs_partition.clone(),
                    room.hash,
                    room.session_epoch,
                    &room.session,
                );
            }
            for room in room_runtime.iter_mut() {
                room.session.record_sent_timestamp(ts);
                // no save_room_session here — the exact FINDING A defect shape
            }
        "#;
        let violations = check_source(synthetic);
        assert_eq!(
            violations.len(),
            1,
            "exactly one site is missing its persist: {violations:?}"
        );
        assert!(violations[0].contains("is not followed"));
    }

    /// MUTATION 2: the persist call exists but is moved BEFORE the
    /// `record_sent_timestamp` advance — must still be caught, since it
    /// persists the pre-advance watermark, not the one just recorded.
    #[test]
    fn synthetic_persist_moved_before_the_record_is_caught() {
        let synthetic = r#"
            for room in room_runtime.iter_mut() {
                room_session::save_room_session(
                    nvs_partition.clone(),
                    room.hash,
                    room.session_epoch,
                    &room.session,
                );
                room.session.record_sent_timestamp(ts);
            }
        "#;
        let violations = check_source(synthetic);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("is not followed"));
    }

    /// A persist call for a DIFFERENT room variable must not be credited —
    /// disambiguates "a save_room_session appears somewhere nearby" from
    /// "a save_room_session appears for THIS room".
    #[test]
    fn synthetic_persist_for_a_different_room_variable_is_not_credited() {
        let synthetic = r#"
            for room in room_runtime.iter_mut() {
                room.session.record_sent_timestamp(ts);
                room_session::save_room_session(
                    nvs_partition.clone(),
                    other.hash,
                    other.session_epoch,
                    &other.session,
                );
            }
        "#;
        let violations = check_source(synthetic);
        assert_eq!(violations.len(), 1, "{violations:?}");
    }

    /// A later, unrelated site's persist must never be credited to an
    /// earlier site that dropped its own — the forward search is bounded by
    /// the NEXT site's position.
    #[test]
    fn synthetic_next_sites_persist_is_not_stolen_by_an_earlier_dropped_one() {
        let synthetic = r#"
            for room in room_runtime.iter_mut() {
                room.session.record_sent_timestamp(boot_ts);
                // dropped — no save_room_session before the next site below
            }
            for room in room_runtime.iter_mut() {
                room.session.record_sent_timestamp(ts);
                room_session::save_room_session(
                    nvs_partition.clone(),
                    room.hash,
                    room.session_epoch,
                    &room.session,
                );
            }
        "#;
        let violations = check_source(synthetic);
        assert_eq!(
            violations.len(),
            1,
            "the first site's dropped persist must still be reported even though a persist \
             exists later in the file: {violations:?}"
        );
    }

    #[test]
    fn a_missing_anchor_is_a_violation_not_a_silent_pass() {
        let violations = check_source("fn main() {}");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no `<var>.session.record_sent_timestamp(` call site found")),
            "{violations:?}"
        );
    }

    #[test]
    fn comment_mentions_are_not_counted_as_call_sites() {
        // A doc comment mentioning `record_sent_timestamp` (there are
        // several in the real file) must not itself be mistaken for a call
        // site — masked comments/strings are blanked before scanning.
        let synthetic = r#"
            // mirrors `record_sent_timestamp`'s guard on monotonicity
            for room in room_runtime.iter_mut() {
                room.session.record_sent_timestamp(ts);
                room_session::save_room_session(
                    nvs_partition.clone(),
                    room.hash,
                    room.session_epoch,
                    &room.session,
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
