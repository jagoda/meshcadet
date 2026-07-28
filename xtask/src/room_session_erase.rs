// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard for FINDING D and FINDING G of the room-lifecycle
//! deep-review (`meshcadet-room-lifecycle-session-store` pass 2,
//! `meshcadet-room-session-erase-durability` pass 3): `admin_server.rs`'s
//! `FRAME_ADD_ROOM` and `FRAME_DEL_ROOM` arms must each erase the room's
//! dedicated `mc_room` NVS session-store blob
//! (`room_session::delete_room_session`), AND that erase must actually
//! *survive* — not just be called.
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
//! silently inherits the deleted room's learned state. That's FINDING D, and
//! [`check_admin_server_source`] (the original half of this guard) pins it.
//!
//! # FINDING G: the erase CALL existing is not the erase's EFFECT surviving
//!
//! FINDING D's fix only proves `delete_room_session` is CALLED. It cannot see
//! that `main.rs`'s dispatcher loop built a `RoomRuntime` for this room ONCE
//! at boot and keeps calling `save_room_session` for it afterward — with no
//! cross-thread signal telling that loop an erase just happened on the
//! admin-server thread. Left alone, the very next one of those saves
//! resurrects the blob the erase just removed, silently undoing FINDING D's
//! fix without ever removing the pinned call site. [`check_room_session_source`]
//! closes that blind spot by pinning the actual durability mechanism
//! (`firmware_core::room_session`'s erase-epoch, see that module's doc) at
//! its two load-bearing wiring points in `room_session.rs`:
//! `delete_room_session` must bump the epoch, and `save_room_session` must
//! gate its write on the epoch still matching — in that order, since a gate
//! checked AFTER the write already happened has no effect. The actual PROOF
//! that this mechanism holds end-to-end (erase survives a live, un-rebooted
//! runtime) is a real behavioral test in `firmware_core::room_session`'s own
//! test module — this guard only proves the mechanism is still WIRED IN at
//! the two call sites that matter, the same "structural scan, not execution"
//! limit the rest of this module already works within.
//!
//! # Why this lives in xtask and not admin_server.rs / room_session.rs
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

/// Path, relative to the repo root, of the FINDING D half's target file.
pub const ADMIN_SERVER_REL_PATH: &str = "firmware/src/admin_server.rs";

/// Path, relative to the repo root, of the FINDING G half's target file.
pub const ROOM_SESSION_REL_PATH: &str = "firmware/src/room_session.rs";

/// The call every pinned arm must contain — `nvs_partition.clone()`'s exact
/// argument shape is deliberately NOT checked (a legitimate refactor of how
/// the partition handle is threaded through must not trip this), only that
/// the erase itself is invoked.
const REQUIRED_CALL: &str = "room_session::delete_room_session(";

/// The two match arms this guard pins — both sides of FINDING D's fix.
const PINNED_ARMS: &[&str] = &["FRAME_ADD_ROOM", "FRAME_DEL_ROOM"];

/// The call `delete_room_session` must make to bump this room's erase epoch
/// (FINDING G) — without it, a `RoomRuntime` built before the erase has no
/// signal that its next `save_room_session` must not resurrect the blob just
/// removed.
const EPOCH_BUMP_CALL: &str = "next_room_session_epoch(";

/// The call `save_room_session` must make, BEFORE its blob write, to check
/// this room's remembered epoch is still current (FINDING G) — without it
/// (or with it checked too late), a stale `RoomRuntime` can resurrect a blob
/// `delete_room_session` already erased.
const EPOCH_GATE_CALL: &str = "room_session_persist_is_current(";

/// The blob write `EPOCH_GATE_CALL` must run before, inside
/// `save_room_session` — the write that resurrects an erased blob if the
/// gate above didn't run first (or ran too late to matter).
const SESSION_BLOB_WRITE_CALL: &str = ".set_blob(";

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

/// Extract the body of a top-level `fn NAME(...) { … }` definition from
/// already-tokenized source — the FINDING G half's equivalent of
/// [`arm_body`]. Unlike a match arm's fixed `=> {` shape, a function's
/// opening brace can trail an arbitrarily long (and, for these two
/// functions, multi-line) parameter list and return type, so this walks
/// forward from the `fn NAME` keyword pair to the first `{` rather than
/// anchoring a single regex to it.
///
/// Returns `Err` on any ambiguity (no hit, more than one hit, no brace found,
/// unbalanced braces) rather than guessing — same "parse gap = NO-GO"
/// doctrine [`arm_body`] uses.
fn fn_body(masked: &str, name: &str) -> Result<String, String> {
    let re = Regex::new(&format!(r"\bfn\s+{}\b", regex::escape(name))).unwrap();
    let mut hits = re.find_iter(masked);
    let (first, second) = (hits.next(), hits.next());
    let m = match (first, second) {
        (None, _) => {
            return Err(format!(
                "{ROOM_SESSION_REL_PATH}: no `fn {name}` found — the function was renamed or \
                 removed, or this scanner needs updating"
            ))
        }
        (Some(_), Some(_)) => {
            return Err(format!(
                "{ROOM_SESSION_REL_PATH}: multiple `fn {name}` occurrences found — this \
                 scanner cannot tell which is the real definition"
            ))
        }
        (Some(m), None) => m,
    };
    // `masked` is guaranteed pure ASCII (see `Tokenized::masked`'s doc), so
    // byte offsets equal char offsets throughout, same as `arm_body`.
    let brace_offset = masked[m.end()..].find('{').ok_or_else(|| {
        format!("{ROOM_SESSION_REL_PATH}: no `{{` found after `fn {name}`'s signature")
    })?;
    let open = m.end() + brace_offset;
    let spans = brace_spans(masked);
    let (o, c) = innermost_span(&spans, open + 1)
        .ok_or_else(|| format!("{ROOM_SESSION_REL_PATH}: unbalanced braces around `fn {name}`"))?;
    if o != open {
        return Err(format!(
            "{ROOM_SESSION_REL_PATH}: could not delimit `fn {name}`'s body"
        ));
    }
    Ok(slice_chars(masked, o + 1, c))
}

/// FINDING D half: scan already-tokenized `admin_server.rs` source and return
/// every violation of "the pinned arm calls the erase".
fn check_admin_server_source(masked: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for arm in PINNED_ARMS {
        match arm_body(masked, arm) {
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

/// FINDING G half: scan already-tokenized `room_session.rs` source and
/// return every violation of "the erase actually survives a live,
/// un-rebooted `RoomRuntime`" — NOT just that `delete_room_session` is
/// called (that's [`check_admin_server_source`]'s job), but that the epoch
/// mechanism `delete_room_session`/`save_room_session` must both wire in for
/// the erase's EFFECT to hold is still present, and in the right order.
fn check_room_session_source(masked: &str) -> Vec<String> {
    let mut violations = Vec::new();

    match fn_body(masked, "delete_room_session") {
        Err(e) => violations.push(e),
        Ok(body) => {
            if !body.contains(EPOCH_BUMP_CALL) {
                violations.push(format!(
                    "{ROOM_SESSION_REL_PATH}: `delete_room_session` does not call \
                     `{EPOCH_BUMP_CALL}` — a `RoomRuntime` built before this erase has no \
                     signal that its next `save_room_session` must not resurrect the blob \
                     just removed, reintroducing FINDING G"
                ));
            }
        }
    }

    match fn_body(masked, "save_room_session") {
        Err(e) => violations.push(e),
        Ok(body) => match (
            body.find(EPOCH_GATE_CALL),
            body.find(SESSION_BLOB_WRITE_CALL),
        ) {
            (None, _) => violations.push(format!(
                "{ROOM_SESSION_REL_PATH}: `save_room_session` does not call \
                 `{EPOCH_GATE_CALL}` before writing — a stale `RoomRuntime` can resurrect a \
                 blob `delete_room_session` already erased; the erase CALL existing in \
                 admin_server.rs is not enough, its EFFECT must survive (FINDING G)"
            )),
            (Some(_), None) => violations.push(format!(
                "{ROOM_SESSION_REL_PATH}: `save_room_session` no longer writes the session \
                 blob (`{SESSION_BLOB_WRITE_CALL}`) at all — this scanner needs updating"
            )),
            (Some(gate_idx), Some(write_idx)) if gate_idx > write_idx => violations.push(format!(
                "{ROOM_SESSION_REL_PATH}: `save_room_session` calls `{EPOCH_GATE_CALL}` \
                     AFTER the blob write instead of before — a gate checked once the write \
                     already happened cannot prevent it, reintroducing FINDING G"
            )),
            _ => {}
        },
    }

    violations
}

/// Scan already-read source text from both target files and return every
/// contract violation (FINDING D + FINDING G, combined). Split from [`check`]
/// so the tests can drive it with synthetic sources.
pub fn check_source(admin_server_src: &str, room_session_src: &str) -> Vec<String> {
    let mut violations = check_admin_server_source(&tokenize(admin_server_src).masked);
    violations.extend(check_room_session_source(
        &tokenize(room_session_src).masked,
    ));
    violations
}

/// Read [`ADMIN_SERVER_REL_PATH`] and [`ROOM_SESSION_REL_PATH`] under
/// `repo_root` and return every contract violation. Empty vec == the
/// contract holds.
pub fn check(repo_root: &Path) -> Vec<String> {
    let admin_server_path = repo_root.join(ADMIN_SERVER_REL_PATH);
    let admin_server_src = match fs::read_to_string(&admin_server_path) {
        Ok(src) => src,
        Err(e) => return vec![format!("reading {}: {e}", admin_server_path.display())],
    };
    let room_session_path = repo_root.join(ROOM_SESSION_REL_PATH);
    let room_session_src = match fs::read_to_string(&room_session_path) {
        Ok(src) => src,
        Err(e) => return vec![format!("reading {}: {e}", room_session_path.display())],
    };
    check_source(&admin_server_src, &room_session_src)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual guard: the shipped `firmware/src/admin_server.rs` erases
    /// the dedicated session store from both the `ADD_ROOM` and `DEL_ROOM`
    /// arms, AND `firmware/src/room_session.rs` wires the erase-epoch
    /// mechanism that makes that erase survive a live, un-rebooted
    /// `RoomRuntime`.
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
    fn synthetic_admin_server(add_room_erases: bool, del_room_erases: bool) -> String {
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

    /// A valid baseline `synthetic_admin_server` fixture, for tests that
    /// only care about the FINDING G (`room_session.rs`) half.
    fn valid_admin_server() -> String {
        synthetic_admin_server(true, true)
    }

    /// A minimal stand-in for `room_session.rs`'s two erase-durability
    /// functions. `bumps_epoch` gates whether `delete_room_session` calls
    /// `next_room_session_epoch`; `gate` selects whether/where
    /// `save_room_session` calls `room_session_persist_is_current` relative
    /// to its blob write.
    #[derive(Clone, Copy)]
    enum Gate {
        /// The gate call is present and runs before the write — correct.
        BeforeWrite,
        /// The gate call is present but runs after the write — too late.
        AfterWrite,
        /// The gate call is missing entirely.
        Missing,
    }

    fn synthetic_room_session(bumps_epoch: bool, gate: Gate) -> String {
        let bump = if bumps_epoch {
            "let next = next_room_session_epoch(current);"
        } else {
            "let next = current;"
        };
        let gate_stmt =
            "if !room_session_persist_is_current(remembered_epoch, current_epoch) { return; }";
        let write_stmt = "nvs.set_blob(key, &blob[..n]).ok();";
        let (first, second) = match gate {
            Gate::BeforeWrite => (gate_stmt, write_stmt),
            Gate::AfterWrite => (write_stmt, gate_stmt),
            Gate::Missing => ("", write_stmt),
        };
        format!(
            r#"
            fn delete_room_session(nvs_partition: EspNvsPartition<NvsDefault>, hash: u8) {{
                let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true).unwrap();
                nvs.remove(key).ok();
                let current = nvs.get_u8(epoch_key).unwrap().unwrap_or(0);
                {bump}
                nvs.set_u8(epoch_key, next).ok();
            }}

            fn save_room_session(
                nvs_partition: EspNvsPartition<NvsDefault>,
                hash: u8,
                remembered_epoch: u8,
                state: &PersistedRoomSession,
            ) {{
                let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true).unwrap();
                let current_epoch = nvs.get_u8(epoch_key).unwrap().unwrap_or(0);
                {first}
                {second}
            }}
            "#
        )
    }

    /// A valid baseline `synthetic_room_session` fixture, for tests that
    /// only care about the FINDING D (`admin_server.rs`) half.
    fn valid_room_session() -> String {
        synthetic_room_session(true, Gate::BeforeWrite)
    }

    #[test]
    fn synthetic_baseline_is_clean() {
        assert_eq!(
            check_source(&valid_admin_server(), &valid_room_session()),
            Vec::<String>::new()
        );
    }

    /// The mutation this guard exists for: `ADD_ROOM` stops erasing the
    /// dedicated store, reintroducing "a re-add resumes a stale watermark".
    #[test]
    fn dropping_the_add_room_erase_is_caught() {
        let violations = check_source(&synthetic_admin_server(false, true), &valid_room_session());
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
        let violations = check_source(&synthetic_admin_server(true, false), &valid_room_session());
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
        let violations = check_source(
            "fn handle_frame(frame_type: u8) { match frame_type { _ => {} } }",
            &valid_room_session(),
        );
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
        let src = valid_admin_server();
        assert!(
            src.contains("use protocol::provisioning::{FRAME_ADD_ROOM, FRAME_DEL_ROOM};"),
            "fixture must actually contain a use-list mention"
        );
        assert_eq!(
            check_source(&src, &valid_room_session()),
            Vec::<String>::new()
        );
    }

    // ── FINDING G: the erase's EFFECT, not just the call site ──────────────

    /// REGRESSION (FINDING G): `delete_room_session` stops bumping the
    /// erase epoch — a `RoomRuntime` built before this erase would have no
    /// signal to stop resurrecting the blob just removed. This is exactly
    /// the gap `check_admin_server_source` alone cannot see: the erase CALL
    /// is still present and would pass FINDING D's half of this guard.
    #[test]
    fn dropping_the_epoch_bump_is_caught() {
        let violations = check_source(
            &valid_admin_server(),
            &synthetic_room_session(false, Gate::BeforeWrite),
        );
        assert!(
            violations
                .iter()
                .any(|v| v.contains("`delete_room_session` does not call")),
            "expected an epoch-bump violation, got {violations:?}"
        );
    }

    /// REGRESSION (FINDING G): `save_room_session` stops gating its write on
    /// the epoch at all — a stale `RoomRuntime` can resurrect an erased blob
    /// unconditionally.
    #[test]
    fn dropping_the_epoch_gate_is_caught() {
        let violations = check_source(
            &valid_admin_server(),
            &synthetic_room_session(true, Gate::Missing),
        );
        assert!(
            violations
                .iter()
                .any(|v| v.contains("`save_room_session` does not call")),
            "expected an epoch-gate violation, got {violations:?}"
        );
    }

    /// REGRESSION (FINDING G): `save_room_session` checks the epoch AFTER
    /// already writing the blob — a gate that runs too late to prevent the
    /// write it's supposed to gate is exactly as ineffective as no gate at
    /// all, and must be caught just the same.
    #[test]
    fn gating_after_the_write_is_caught() {
        let violations = check_source(
            &valid_admin_server(),
            &synthetic_room_session(true, Gate::AfterWrite),
        );
        assert!(
            violations
                .iter()
                .any(|v| v.contains("AFTER the blob write")),
            "expected an ordering violation, got {violations:?}"
        );
    }

    /// The baseline mechanism (bump + gate-before-write) must not be flagged
    /// — sanity check that the two FINDING G checks aren't just always
    /// failing.
    #[test]
    fn valid_room_session_fixture_is_clean() {
        assert_eq!(
            check_room_session_source(&tokenize(&valid_room_session()).masked),
            Vec::<String>::new()
        );
    }
}
