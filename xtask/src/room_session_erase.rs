// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard for FINDING D of the room-lifecycle deep-review
//! pass 2 (`meshcadet-room-lifecycle-session-store`): `admin_server.rs`'s
//! `FRAME_ADD_ROOM` and `FRAME_DEL_ROOM` arms must each erase the room's
//! dedicated `mc_room` NVS session-store blob
//! (`room_session::delete_room_session`).
//!
//! # The defect this pins
//!
//! `firmware/src/main.rs`'s boot-time resume prefers a room's dedicated
//! session store over its `RoomExtra` seed
//! (`load_room_session(..).unwrap_or(seed)` — see `room_session.rs`'s module
//! doc). `room_admin::handle_add_room` documents a re-add as a full replace
//! of `RoomExtra` (fresh `sync_since`/`permissions`/`out_path`), and
//! `handle_del_room` removes `RoomExtra` outright — but neither touches the
//! SEPARATE dedicated store. Without an explicit erase at the admin-server
//! seam, a stale blob silently shadows both operations at the next boot: a
//! re-add resumes a stale watermark/route/permission instead of the
//! documented fresh one, and a delete followed later by an unrelated room
//! that happens to collide on the same 1-byte pubkey hash (1-in-256)
//! silently inherits the deleted room's learned state.
//!
//! # Why this lives in xtask and not admin_server.rs
//!
//! Same reason `ui_event_parity` does (see its module doc): `firmware`'s
//! single `[[bin]]` target sets `harness = false`, so `cargo test` only
//! type-checks its `#[cfg(test)]` blocks and never executes one. This is the
//! host-runnable equivalent, in the same "plain text scanning, no esp
//! toolchain" spirit.

use std::fs;
use std::path::Path;

use regex::Regex;

use crate::{brace_spans, innermost_span, slice_chars, tokenize};

/// Path, relative to the repo root, of the file this module scans.
pub const ADMIN_SERVER_REL_PATH: &str = "firmware/src/admin_server.rs";

/// The call every pinned arm must contain — `nvs_partition.clone()`'s exact
/// argument shape is deliberately NOT checked (a legitimate refactor of how
/// the partition handle is threaded through must not trip this), only that
/// the erase itself is invoked.
const REQUIRED_CALL: &str = "room_session::delete_room_session(";

/// The two match arms this guard pins — both sides of FINDING D's fix.
const PINNED_ARMS: &[&str] = &["FRAME_ADD_ROOM", "FRAME_DEL_ROOM"];

/// Extract the body of a `<ARM> => { … }` match arm from already-tokenized
/// (comment- and string-blanked) source. `ARM` here is a plain top-level
/// frame-type constant (not an enum variant with fields), so the match
/// pattern is `\bARM\s*=>\s*\{` rather than `ui_event_parity::arm_body`'s
/// `UiEvent::variant { .. } =>` shape — a bare identifier search for `ARM`
/// alone would also hit its `use` import, so the arrow is baked into the
/// search pattern itself instead of walked-to afterward.
///
/// Returns `Err` on any ambiguity (no hit, more than one hit, unbalanced
/// braces) rather than guessing — the same "parse gap = NO-GO" doctrine
/// `ui_event_parity` and the glyph harness already use.
fn arm_body(masked: &str, arm: &str) -> Result<String, String> {
    let re = Regex::new(&format!(r"\b{}\s*=>\s*\{{", regex::escape(arm))).unwrap();
    let mut hits = re.find_iter(masked);
    let (first, second) = (hits.next(), hits.next());
    let m = match (first, second) {
        (None, _) => {
            return Err(format!(
                "{ADMIN_SERVER_REL_PATH}: no `{arm} => {{` match arm found — the arm was \
                 renamed or deleted, or this scanner needs updating"
            ))
        }
        (Some(_), Some(_)) => {
            return Err(format!(
                "{ADMIN_SERVER_REL_PATH}: multiple `{arm} => {{` occurrences found — this \
                 scanner cannot tell which is the real match arm"
            ))
        }
        (Some(m), None) => m,
    };
    // `masked` is guaranteed pure ASCII (see `Tokenized::masked`'s doc), so
    // the regex's byte offsets equal the char offsets `brace_spans`/
    // `slice_chars` operate on.
    let open = m.end() - 1;
    let spans = brace_spans(masked);
    let (o, c) = innermost_span(&spans, open + 1).ok_or_else(|| {
        format!("{ADMIN_SERVER_REL_PATH}: unbalanced braces around the `{arm}` arm")
    })?;
    if o != open {
        return Err(format!(
            "{ADMIN_SERVER_REL_PATH}: could not delimit the `{arm}` arm body"
        ));
    }
    Ok(slice_chars(masked, o + 1, c))
}

/// Scan already-read source text and return every contract violation. Split
/// from [`check`] so the tests can drive it with synthetic sources.
pub fn check_source(src: &str) -> Vec<String> {
    let masked = tokenize(src).masked;
    let mut violations = Vec::new();
    for arm in PINNED_ARMS {
        match arm_body(&masked, arm) {
            Err(e) => violations.push(e),
            Ok(body) => {
                if !body.contains(REQUIRED_CALL) {
                    violations.push(format!(
                        "{ADMIN_SERVER_REL_PATH}: `{arm}` arm does not call \
                         `{REQUIRED_CALL}` — a stale `mc_room` session-store blob would \
                         shadow this arm's RoomExtra reset/removal at the next boot's \
                         `load_room_session(..).unwrap_or(seed)` resume, reintroducing \
                         FINDING D"
                    ));
                }
            }
        }
    }
    violations
}

/// Read [`ADMIN_SERVER_REL_PATH`] under `repo_root` and return every contract
/// violation. Empty vec == the contract holds.
pub fn check(repo_root: &Path) -> Vec<String> {
    let path = repo_root.join(ADMIN_SERVER_REL_PATH);
    match fs::read_to_string(&path) {
        Ok(src) => check_source(&src),
        Err(e) => vec![format!("reading {}: {e}", path.display())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual guard: the shipped `firmware/src/admin_server.rs` erases
    /// the dedicated session store from both the `ADD_ROOM` and `DEL_ROOM`
    /// arms.
    #[test]
    fn room_lifecycle_session_erase_contract_holds() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "room-lifecycle session-erase contract violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    /// A minimal stand-in for `handle_frame`'s room arms, mirroring the real
    /// file's shape closely enough to exercise this scanner (an `use`-list
    /// mention of the frame constant plus the actual match arm).
    fn synthetic(add_room_erases: bool, del_room_erases: bool) -> String {
        let add_erase = if add_room_erases {
            "room_session::delete_room_session(nvs_partition.clone(), hash);"
        } else {
            ""
        };
        let del_erase = if del_room_erases {
            "room_session::delete_room_session(nvs_partition.clone(), hash);"
        } else {
            ""
        };
        format!(
            r#"
            use protocol::provisioning::{{FRAME_ADD_ROOM, FRAME_DEL_ROOM}};

            // A doc mention of `FRAME_ADD_ROOM` must not count as the arm.
            fn handle_frame(frame_type: u8) {{
                match frame_type {{
                    FRAME_ADD_ROOM => {{
                        let hash = payload.first().copied().unwrap_or(0);
                        {add_erase}
                        persist_or_rollback(config, nvs_partition, out, ConfigKind::Room)?;
                    }}
                    FRAME_DEL_ROOM => {{
                        let hash = payload.first().copied().unwrap_or(0);
                        {del_erase}
                        persist_or_rollback(config, nvs_partition, out, ConfigKind::Room)?;
                    }}
                    _ => {{}}
                }}
            }}
            "#
        )
    }

    #[test]
    fn synthetic_baseline_is_clean() {
        assert_eq!(check_source(&synthetic(true, true)), Vec::<String>::new());
    }

    /// The mutation this guard exists for: `ADD_ROOM` stops erasing the
    /// dedicated store, reintroducing "a re-add resumes a stale watermark".
    #[test]
    fn dropping_the_add_room_erase_is_caught() {
        let violations = check_source(&synthetic(false, true));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("`FRAME_ADD_ROOM` arm does not call")),
            "expected an ADD_ROOM violation, got {violations:?}"
        );
        assert!(
            !violations
                .iter()
                .any(|v| v.contains("`FRAME_DEL_ROOM` arm does not call")),
            "DEL_ROOM still erases and must not be flagged, got {violations:?}"
        );
    }

    /// The other arm: `DEL_ROOM` stops erasing, reintroducing the
    /// hash-collision inheritance defect.
    #[test]
    fn dropping_the_del_room_erase_is_caught() {
        let violations = check_source(&synthetic(true, false));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("`FRAME_DEL_ROOM` arm does not call")),
            "expected a DEL_ROOM violation, got {violations:?}"
        );
    }

    /// Parse gaps fail loud rather than passing silently.
    #[test]
    fn a_missing_arm_is_a_violation_not_a_silent_pass() {
        let violations =
            check_source("fn handle_frame(frame_type: u8) { match frame_type { _ => {} } }");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no `FRAME_ADD_ROOM => {` match arm found")),
            "expected a missing-arm violation, got {violations:?}"
        );
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no `FRAME_DEL_ROOM => {` match arm found")),
            "expected a missing-arm violation, got {violations:?}"
        );
    }

    /// A `use`-list mention of the frame constant (no `=>` following it)
    /// must not be mistaken for the match arm.
    #[test]
    fn use_list_mention_is_not_counted_as_the_arm() {
        let src = synthetic(true, true);
        assert!(
            src.contains("use protocol::provisioning::{FRAME_ADD_ROOM, FRAME_DEL_ROOM};"),
            "fixture must actually contain a use-list mention"
        );
        assert_eq!(check_source(&src), Vec::<String>::new());
    }
}
