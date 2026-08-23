// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard for the screen-lock overlay's input-gate
//! invariant (`meshcadet-lock-firmware-ui`, screen-lock plan D3):
//! `UiRuntime::step()`'s keyboard and trackball blocks must check
//! `self.locked` BEFORE the branch that reads `self.active_screen`
//! directly, bypassing Slint's own window-component routing.
//!
//! # Why touch is not scanned here
//!
//! Touch's `self.window.dispatch_touch(ev)` reaches whatever component is
//! CURRENTLY attached to the one shared `MinimalSoftwareWindow`
//! (`platform.rs`'s module doc: "the window's currently-set component" is
//! singular) — while `locked` that is always the lock overlay
//! (`trip_lock`'s `screen.show()`), never `active_screen`, so touch already
//! cannot reach the underlying screen by construction, with no
//! `self.locked` branch needed. The keyboard and trackball blocks are
//! different: both branch on `self.active_screen` directly in Rust
//! (`ActiveScreen::MessageView`/`ActiveScreen::Compose` matches for
//! keyboard; `handle_trackball_event` for trackball), a path that bypasses
//! Slint's routing entirely and therefore needs an explicit gate — see
//! `ui/mod.rs`'s own comment at each of the three call sites for the full
//! reasoning, which this scanner pins mechanically for the two that need
//! it.
//!
//! # What this pins
//!
//! Within each of the keyboard/trackball sections of `step()` (sliced by
//! that section's own `// ── Poll ... ──` header comment, found in the RAW
//! source since `tokenize()` blanks comments to spaces and cannot itself
//! anchor a search), the block must contain `self.locked` (or `!self.locked`
//! — a plain substring match) BEFORE the modality's own risky call —
//! `ActiveScreen::MessageView` for keyboard, `self.handle_trackball_event(`
//! for trackball — in the MASKED (comment/string-blanked) text, so a mere
//! comment mentioning `self.locked` can never satisfy this check.
//!
//! This is a structural scan, not a full data-flow analysis — same
//! "textual precedes" proxy `room_session_erase.rs`'s epoch-ordering check
//! already uses for an analogous ordering invariant, and the same
//! "parse gap = NO-GO, never a silent pass" discipline every scanner in
//! this crate follows.

use std::fs;
use std::path::Path;

pub const UI_MOD_REL_PATH: &str = "firmware/src/ui/mod.rs";

/// Section-header anchors, found in RAW source (see module doc for why).
const KEYBOARD_ANCHOR: &str = "// ── Poll physical keyboard";
const TRACKBALL_ANCHOR: &str = "// ── Poll trackball";
/// End boundary for the trackball block — the next section after it.
const TRACKBALL_END_ANCHOR: &str = "// ── Screen-sleep inactivity check";

/// The call each block must not reach without `LOCK_GATE` preceding it.
const KEYBOARD_RISKY_CALL: &str = "ActiveScreen::MessageView";
const TRACKBALL_RISKY_CALL: &str = "self.handle_trackball_event(";

/// The gate every block must contain, textually BEFORE its risky call.
const LOCK_GATE: &str = "self.locked";

/// Find exactly one occurrence of `needle` in `haystack`. Returns `Err` on
/// zero or more-than-one hits — ambiguity is a scanner-needs-updating
/// condition, never a guess (same discipline every other xtask scanner
/// uses).
fn find_once(haystack: &str, needle: &str) -> Result<usize, String> {
    let mut it = haystack.match_indices(needle);
    match (it.next(), it.next()) {
        (None, _) => Err(format!(
            "{UI_MOD_REL_PATH}: anchor `{needle}` not found — the section was renamed/removed, \
             or this scanner needs updating"
        )),
        (Some(_), Some(_)) => Err(format!(
            "{UI_MOD_REL_PATH}: anchor `{needle}` occurs more than once — this scanner cannot \
             tell which is the real section header"
        )),
        (Some((i, _)), None) => Ok(i),
    }
}

/// Check one block (already sliced from RAW source) for the gate-precedes-
/// risky-call invariant. `label` names the modality for the violation text.
fn check_block(raw_block: &str, risky_call: &str, label: &str, violations: &mut Vec<String>) {
    let masked = crate::tokenize(raw_block).masked;
    let risky_idx = masked.find(risky_call);
    let gate_idx = masked.find(LOCK_GATE);
    match (gate_idx, risky_idx) {
        (_, None) => violations.push(format!(
            "{UI_MOD_REL_PATH}: the {label} block no longer contains `{risky_call}` — this \
             scanner needs updating"
        )),
        (None, Some(_)) => violations.push(format!(
            "{UI_MOD_REL_PATH}: the {label} block calls `{risky_call}` (reads \
             `self.active_screen` directly, bypassing Slint's window-component routing) with no \
             `{LOCK_GATE}` check anywhere in the block — an input event received while locked \
             could mutate or navigate the retained (hidden) screen, violating D3"
        )),
        (Some(g), Some(r)) if g > r => violations.push(format!(
            "{UI_MOD_REL_PATH}: the {label} block checks `{LOCK_GATE}` AFTER `{risky_call}` \
             instead of before — a gate checked once the active-screen branch already ran cannot \
             prevent it"
        )),
        _ => {}
    }
}

/// Scan already-read `firmware/src/ui/mod.rs` source and return every
/// violation of the lock-gate invariant. Empty vec == the contract holds.
pub fn check_source(ui_mod_src: &str) -> Vec<String> {
    let mut violations = Vec::new();

    let keyboard_off = match find_once(ui_mod_src, KEYBOARD_ANCHOR) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    let trackball_off = match find_once(ui_mod_src, TRACKBALL_ANCHOR) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    let trackball_end_off = match find_once(ui_mod_src, TRACKBALL_END_ANCHOR) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    if !(keyboard_off < trackball_off && trackball_off < trackball_end_off) {
        violations.push(format!(
            "{UI_MOD_REL_PATH}: expected anchor order keyboard < trackball < screen-sleep-check \
             (got byte offsets {keyboard_off}/{trackball_off}/{trackball_end_off}) — step()'s \
             section ordering changed; this scanner needs updating"
        ));
        return violations;
    }

    let keyboard_block = &ui_mod_src[keyboard_off..trackball_off];
    let trackball_block = &ui_mod_src[trackball_off..trackball_end_off];

    check_block(
        keyboard_block,
        KEYBOARD_RISKY_CALL,
        "keyboard",
        &mut violations,
    );
    check_block(
        trackball_block,
        TRACKBALL_RISKY_CALL,
        "trackball",
        &mut violations,
    );

    violations
}

/// Read [`UI_MOD_REL_PATH`] under `repo_root` and return every violation.
pub fn check(repo_root: &Path) -> Vec<String> {
    let path = repo_root.join(UI_MOD_REL_PATH);
    match fs::read_to_string(&path) {
        Ok(src) => check_source(&src),
        Err(e) => vec![format!("reading {}: {e}", path.display())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual guard: the shipped `firmware/src/ui/mod.rs` gates both
    /// the keyboard and trackball blocks on `self.locked` before their
    /// respective active-screen-dependent risky call.
    #[test]
    fn lock_gate_contract_holds() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "screen-lock input-gate contract violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    /// Minimal synthetic stand-in for `step()`'s three relevant sections,
    /// close enough in shape to exercise this scanner without depending on
    /// the real (large, frequently-edited) file.
    fn synthetic_step(keyboard_gated: bool, trackball_gated: bool) -> String {
        let keyboard_body = if keyboard_gated {
            r#"
            if self.locked {
                // no-op while locked
            } else {
                let compose_seed = if matches!(self.active_screen, ActiveScreen::MessageView(_)) {
                    message_view_compose_seed(byte)
                } else {
                    None
                };
            }
            "#
        } else {
            r#"
            let compose_seed = if matches!(self.active_screen, ActiveScreen::MessageView(_)) {
                message_view_compose_seed(byte)
            } else {
                None
            };
            "#
        };
        let trackball_body = if trackball_gated {
            "if !self.locked { self.handle_trackball_event(ev); }"
        } else {
            "self.handle_trackball_event(ev);"
        };
        format!(
            r#"
            // ── Poll physical keyboard ──────────────────────────────────
            {{
                {keyboard_body}
            }}

            // ── Poll trackball ──────────────────────────────────────────
            if let Some(ref mut tb) = self.trackball {{
                {trackball_body}
            }}

            // ── Screen-sleep inactivity check ───────────────────────────
            if !self.screen_asleep {{ }}
            "#
        )
    }

    #[test]
    fn synthetic_baseline_both_gated_is_clean() {
        assert_eq!(
            check_source(&synthetic_step(true, true)),
            Vec::<String>::new()
        );
    }

    /// The mutation this guard exists for: the keyboard block loses its
    /// `self.locked` gate, so a printable key typed while locked would
    /// reach the (retained, hidden) MessageView/Compose active-screen
    /// branches directly.
    #[test]
    fn ungated_keyboard_block_is_caught() {
        let violations = check_source(&synthetic_step(false, true));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("keyboard block calls")),
            "expected a keyboard violation, got {violations:?}"
        );
        assert!(
            !violations
                .iter()
                .any(|v| v.contains("trackball block calls")),
            "trackball is still gated and must not be flagged, got {violations:?}"
        );
    }

    /// The other half: the trackball block loses its gate.
    #[test]
    fn ungated_trackball_block_is_caught() {
        let violations = check_source(&synthetic_step(true, false));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("trackball block calls")),
            "expected a trackball violation, got {violations:?}"
        );
    }

    /// Ordering matters, not mere presence: a `self.locked` check that
    /// exists in the block but AFTER the risky call already ran is exactly
    /// as ineffective as no gate at all.
    #[test]
    fn gate_present_but_after_the_risky_call_is_caught() {
        let src = synthetic_step(true, true).replace(
            "if !self.locked { self.handle_trackball_event(ev); }",
            "self.handle_trackball_event(ev); if !self.locked { }",
        );
        let violations = check_source(&src);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("trackball") && v.contains("AFTER")),
            "expected an ordering violation, got {violations:?}"
        );
    }

    /// A comment merely MENTIONING `self.locked` inside the block, with no
    /// real code gate, must not satisfy the check — tokenize() blanks
    /// comment bodies before the keyword search runs.
    #[test]
    fn comment_only_mention_does_not_satisfy_the_gate() {
        let src = synthetic_step(false, true).replace(
            "let compose_seed",
            "// self.locked is handled elsewhere, trust me\n                let compose_seed",
        );
        let violations = check_source(&src);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("keyboard block calls")),
            "a comment-only mention must not satisfy the gate; got {violations:?}"
        );
    }

    /// Parse gaps fail loud rather than passing silently.
    #[test]
    fn missing_anchor_is_a_violation_not_a_silent_pass() {
        let violations = check_source("fn step(&mut self) {}");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("anchor") && v.contains("not found")),
            "expected a missing-anchor violation, got {violations:?}"
        );
    }
}
