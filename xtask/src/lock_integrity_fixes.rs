// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guards for two of the three
//! `meshcadet-lock-integrity-fixes` deep-review pass 1 findings that live in
//! the detached (esp-toolchain-only) `firmware` crate, so — like
//! [`crate::lock_gate`] and the `room_*` scanners — cannot carry a `#[test]`
//! that actually executes there (`firmware/`'s `[[bin]]` sets `harness =
//! false`; see `firmware/Cargo.toml`'s doc comment). F3
//! (`pin_menu::MenuAction::SetLockFlags` bit-masking) is pure `firmware-core`
//! logic and already has real, executing `#[test]`s in
//! `firmware-core/src/pin_menu.rs` — no scanner needed for it.
//!
//! # F2 — `trip_lock` must fail CLOSED
//!
//! [`trip_lock_fail_closed_violations`] pins that `UiRuntime::trip_lock`
//! sets `self.locked = true` UNCONDITIONALLY, textually BEFORE it calls
//! `self.construct_lock_screen(` (the fallible `LockScreen::new()` wrapper) —
//! not only after a successful construction, which is the exact ordering bug
//! this finding named: a `LockScreen::new()` failure used to leave
//! `self.locked == false` with the underlying screen already hidden by
//! `hide_active_screen()` — a blank window that still accepted input meant
//! for the (invisible) underlying screen, since every keyboard/trackball
//! input gate checks `self.locked`, not "is a lock overlay present" (see
//! [`crate::lock_gate`], which pins THAT half — the gate itself — and is
//! unaffected by this module).
//!
//! # F1 — `FRAME_SET_LOCK_PIN` must live-forward to the UI thread
//!
//! [`lock_pin_live_forward_violations`] pins that `admin_server.rs`'s
//! `FRAME_SET_LOCK_PIN` arm forwards a `UiEvent::LockPinChanged` over
//! `send_or_count`, mirroring `FRAME_SET_LOCK_CONFIG`'s existing live-forward
//! pattern — without it, a `set-lock-pin`/`reset-lock-pin` write only takes
//! effect at the next boot (`BootSeed::lock_pin`), so a same-USB-session
//! `set-lock-pin` followed by `lock-config --enable` locks the device out
//! until power-cycle, and `reset-lock-pin` against an already-locked device
//! is a silent no-op.
//!
//! Same "textual precedes"/"textual contains" proxy discipline every other
//! scanner in this crate uses — not a full data-flow analysis, and any parse
//! gap (anchor not found / found more than once) is a hard failure, never a
//! silent pass.

use std::fs;
use std::path::Path;

pub const UI_MOD_REL_PATH: &str = "firmware/src/ui/mod.rs";
pub const ADMIN_SERVER_REL_PATH: &str = "firmware/src/admin_server.rs";

/// Find exactly one occurrence of `needle` in `haystack`. Same discipline as
/// `lock_gate::find_once` (duplicated rather than shared — each scanner
/// module in this crate is deliberately self-contained; see e.g. the
/// `room_*` scanners).
fn find_once(haystack: &str, needle: &str, file: &str) -> Result<usize, String> {
    let mut it = haystack.match_indices(needle);
    match (it.next(), it.next()) {
        (None, _) => Err(format!(
            "{file}: anchor `{needle}` not found — the code was renamed/removed, or this \
             scanner needs updating"
        )),
        (Some(_), Some(_)) => Err(format!(
            "{file}: anchor `{needle}` occurs more than once — this scanner cannot tell which \
             is the real one"
        )),
        (Some((i, _)), None) => Ok(i),
    }
}

/// F2: check `UiRuntime::trip_lock`'s body (sliced from its own `fn`
/// signature to the next `fn` at the same indentation) for the
/// gate-precedes-fallible-call invariant.
pub fn trip_lock_fail_closed_violations(ui_mod_src: &str) -> Vec<String> {
    const FN_ANCHOR: &str = "fn trip_lock(&mut self) {";
    const NEXT_FN_ANCHOR: &str = "fn construct_lock_screen(&mut self) {";
    const SET_LOCKED: &str = "self.locked = true";
    const RISKY_CALL: &str = "self.construct_lock_screen(";

    let mut violations = Vec::new();

    let start = match find_once(ui_mod_src, FN_ANCHOR, UI_MOD_REL_PATH) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    let end = match find_once(ui_mod_src, NEXT_FN_ANCHOR, UI_MOD_REL_PATH) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    if end <= start {
        violations.push(format!(
            "{UI_MOD_REL_PATH}: expected `{NEXT_FN_ANCHOR}` to appear AFTER `{FN_ANCHOR}` — \
             function order changed; this scanner needs updating"
        ));
        return violations;
    }

    let body = &ui_mod_src[start..end];
    let masked = crate::tokenize(body).masked;
    let set_locked_idx = masked.find(SET_LOCKED);
    let risky_idx = masked.find(RISKY_CALL);

    match (set_locked_idx, risky_idx) {
        (_, None) => violations.push(format!(
            "{UI_MOD_REL_PATH}: `trip_lock` no longer calls `{RISKY_CALL}` — this scanner needs \
             updating"
        )),
        (None, Some(_)) => violations.push(format!(
            "{UI_MOD_REL_PATH}: `trip_lock` calls `{RISKY_CALL}` (the fallible \
             `LockScreen::new()` wrapper) with no `{SET_LOCKED}` anywhere in the function — a \
             construction failure leaves `locked == false` with the underlying screen already \
             hidden: fails OPEN, not closed (F2)"
        )),
        (Some(g), Some(r)) if g > r => violations.push(format!(
            "{UI_MOD_REL_PATH}: `trip_lock` sets `{SET_LOCKED}` AFTER `{RISKY_CALL}` instead of \
             before — gating on a successful construction (not unconditionally) is exactly the \
             fail-OPEN ordering F2 fixed"
        )),
        _ => {}
    }

    violations
}

/// F1: check `admin_server.rs`'s `FRAME_SET_LOCK_PIN` arm (sliced to the
/// next `FRAME_SET_LOCK_CONFIG` arm, which already immediately follows it in
/// the match — see that file's own frame-type table) for the live-forward
/// call.
pub fn lock_pin_live_forward_violations(admin_server_src: &str) -> Vec<String> {
    const ARM_ANCHOR: &str = "FRAME_SET_LOCK_PIN => {";
    const NEXT_ARM_ANCHOR: &str = "FRAME_SET_LOCK_CONFIG => {";
    const FORWARD_CALL: &str = "send_or_count(";
    const EVENT_VARIANT: &str = "UiEvent::LockPinChanged";

    let mut violations = Vec::new();

    let start = match find_once(admin_server_src, ARM_ANCHOR, ADMIN_SERVER_REL_PATH) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    let end = match find_once(admin_server_src, NEXT_ARM_ANCHOR, ADMIN_SERVER_REL_PATH) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    if end <= start {
        violations.push(format!(
            "{ADMIN_SERVER_REL_PATH}: expected `{NEXT_ARM_ANCHOR}` to appear AFTER \
             `{ARM_ANCHOR}` — match-arm order changed; this scanner needs updating"
        ));
        return violations;
    }

    let arm = &admin_server_src[start..end];
    let masked = crate::tokenize(arm).masked;

    if !masked.contains(FORWARD_CALL) || !masked.contains(EVENT_VARIANT) {
        violations.push(format!(
            "{ADMIN_SERVER_REL_PATH}: the FRAME_SET_LOCK_PIN arm no longer forwards \
             `{EVENT_VARIANT}` via `{FORWARD_CALL}` — a host-written lock PIN would only take \
             effect at the next boot again (BootSeed), re-opening the same-session lockout / \
             reset-lock-pin-does-not-unlock defect F1 fixed"
        ));
    }

    violations
}

/// Read both source files under `repo_root` and return every violation from
/// both checks combined.
pub fn check(repo_root: &Path) -> Vec<String> {
    let ui_mod_path = repo_root.join(UI_MOD_REL_PATH);
    let admin_server_path = repo_root.join(ADMIN_SERVER_REL_PATH);

    let mut violations = Vec::new();
    match fs::read_to_string(&ui_mod_path) {
        Ok(src) => violations.extend(trip_lock_fail_closed_violations(&src)),
        Err(e) => violations.push(format!("reading {}: {e}", ui_mod_path.display())),
    }
    match fs::read_to_string(&admin_server_path) {
        Ok(src) => violations.extend(lock_pin_live_forward_violations(&src)),
        Err(e) => violations.push(format!("reading {}: {e}", admin_server_path.display())),
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual guard: the shipped tree fails closed and live-forwards.
    #[test]
    fn lock_integrity_fixes_contract_holds() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "screen-lock integrity contract violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    // ── F2: trip_lock fail-closed ────────────────────────────────────────

    fn synthetic_trip_lock(set_locked_before_risky: bool) -> String {
        let body = if set_locked_before_risky {
            r#"
            fn trip_lock(&mut self) {
                if self.locked {
                    return;
                }
                self.hide_active_screen();
                self.locked = true;
                self.construct_lock_screen();
                self.window.request_redraw();
            }

            fn construct_lock_screen(&mut self) {
            "#
        } else {
            // The original bug shape: `locked` only set inside a (now-inlined)
            // success path, textually after the risky call.
            r#"
            fn trip_lock(&mut self) {
                if self.locked {
                    return;
                }
                self.hide_active_screen();
                self.construct_lock_screen();
                self.locked = true;
                self.window.request_redraw();
            }

            fn construct_lock_screen(&mut self) {
            "#
        };
        body.to_string()
    }

    #[test]
    fn synthetic_fail_closed_ordering_is_clean() {
        assert_eq!(
            trip_lock_fail_closed_violations(&synthetic_trip_lock(true)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn synthetic_fail_open_ordering_is_caught() {
        let violations = trip_lock_fail_closed_violations(&synthetic_trip_lock(false));
        assert!(
            violations.iter().any(|v| v.contains("AFTER")),
            "expected an ordering violation, got {violations:?}"
        );
    }

    #[test]
    fn synthetic_missing_set_locked_is_caught() {
        let src = r#"
            fn trip_lock(&mut self) {
                self.hide_active_screen();
                self.construct_lock_screen();
            }

            fn construct_lock_screen(&mut self) {
            "#;
        let violations = trip_lock_fail_closed_violations(src);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("fails OPEN, not closed")),
            "expected a fail-open violation, got {violations:?}"
        );
    }

    #[test]
    fn synthetic_comment_only_mention_does_not_satisfy_the_gate() {
        let src = r#"
            fn trip_lock(&mut self) {
                // self.locked = true is set elsewhere, trust me
                self.hide_active_screen();
                self.construct_lock_screen();
            }

            fn construct_lock_screen(&mut self) {
            "#;
        let violations = trip_lock_fail_closed_violations(src);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("fails OPEN, not closed")),
            "a comment-only mention must not satisfy the gate; got {violations:?}"
        );
    }

    #[test]
    fn synthetic_missing_anchor_is_a_violation_not_a_silent_pass() {
        let violations = trip_lock_fail_closed_violations("fn other() {}");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("anchor") && v.contains("not found")),
            "expected a missing-anchor violation, got {violations:?}"
        );
    }

    // ── F1: SET_LOCK_PIN live forward ────────────────────────────────────

    fn synthetic_admin_server(forwards: bool) -> String {
        let arm_body = if forwards {
            r#"
            FRAME_SET_LOCK_PIN => {
                match decode_set_lock_pin(payload) {
                    Ok(p) => {
                        match crate::lock_store::save(nvs_partition.clone(), &p.pin, LOCK_PIN_LEN as u8) {
                            Ok(()) => {
                                send_or_count(
                                    evt_tx,
                                    UiEvent::LockPinChanged { lock_pin: p.pin, lock_pin_len: LOCK_PIN_LEN as u8 },
                                    evt_dropped,
                                );
                                send_ok(out)?;
                            }
                            Err(e) => {}
                        }
                    }
                    Err(e) => {}
                }
            }
            "#
        } else {
            // Pre-fix shape: NVS write only, no live forward at all.
            r#"
            FRAME_SET_LOCK_PIN => {
                match decode_set_lock_pin(payload) {
                    Ok(p) => {
                        match crate::lock_store::save(nvs_partition.clone(), &p.pin, LOCK_PIN_LEN as u8) {
                            Ok(()) => {
                                send_ok(out)?;
                            }
                            Err(e) => {}
                        }
                    }
                    Err(e) => {}
                }
            }
            "#
        };
        format!("{arm_body}\n            FRAME_SET_LOCK_CONFIG => {{\n")
    }

    #[test]
    fn synthetic_live_forward_present_is_clean() {
        assert_eq!(
            lock_pin_live_forward_violations(&synthetic_admin_server(true)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn synthetic_missing_live_forward_is_caught() {
        let violations = lock_pin_live_forward_violations(&synthetic_admin_server(false));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("no longer forwards") || v.contains("next boot again")),
            "expected a missing-forward violation, got {violations:?}"
        );
    }

    #[test]
    fn synthetic_admin_server_missing_anchor_is_a_violation_not_a_silent_pass() {
        let violations = lock_pin_live_forward_violations("fn handle_frame() {}");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("anchor") && v.contains("not found")),
            "expected a missing-anchor violation, got {violations:?}"
        );
    }
}
