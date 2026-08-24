// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard for `meshcadet-power-optimization` Phase 5's
//! render-skip invariant: `UiRuntime::step()`'s render section must not
//! reach `render_if_needed` while the screen is asleep. The ST7789 is
//! fully powered down (`SLPIN` + `DISPOFF`, see `TDeckDisplay::sleep`) for
//! the whole `screen_asleep` window, so flushing dirty regions over SPI to
//! it is pure waste, not merely redundant — see
//! `firmware_core::ui::idle_tick::render_gate`'s doc for the pure-logic
//! half of this invariant.
//!
//! Same "textual precedes"/"section-sliced" proxy discipline as
//! `xtask::lock_gate` (an almost identical shape: gate-before-risky-call,
//! within one named section of `step()`, over the masked/comment-blanked
//! text) — see that module's doc for the general pattern this mirrors.

use std::fs;
use std::path::Path;

pub const UI_MOD_REL_PATH: &str = "firmware/src/ui/mod.rs";

/// Section-header anchor, found in RAW source (see module doc for why —
/// `tokenize()` blanks comments to spaces and cannot itself anchor a
/// search).
const SECTION_ANCHOR: &str = "// ── Render dirty regions ──";
/// End boundary — the next function after `step()`.
const SECTION_END_ANCHOR: &str = "fn handle_event(&mut self, event: UiEvent, now_ms: u64) {";

/// The call the render section must not reach without `GATE` preceding it.
const RISKY_CALL: &str = "self.window.render_if_needed(&mut self.display)";
/// The gate the render section must contain, textually BEFORE its risky
/// call.
const GATE: &str = "render_gate(self.screen_asleep)";

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
             tell which is the real section"
        )),
        (Some((i, _)), None) => Ok(i),
    }
}

/// Check the render section (already sliced from RAW source) for the
/// gate-precedes-risky-call invariant, over the MASKED (comment/string-
/// blanked) text so a comment mentioning either string can't satisfy it.
pub fn check_source(ui_mod_src: &str) -> Vec<String> {
    let mut violations = Vec::new();

    let start = match find_once(ui_mod_src, SECTION_ANCHOR) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    let end = match find_once(ui_mod_src, SECTION_END_ANCHOR) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    if end <= start {
        violations.push(format!(
            "{UI_MOD_REL_PATH}: expected `{SECTION_END_ANCHOR}` to appear AFTER \
             `{SECTION_ANCHOR}` — step()'s section ordering changed; this scanner needs updating"
        ));
        return violations;
    }

    let section = &ui_mod_src[start..end];
    let masked = crate::tokenize(section).masked;
    let risky_idx = masked.find(RISKY_CALL);
    let gate_idx = masked.find(GATE);

    match (gate_idx, risky_idx) {
        (_, None) => violations.push(format!(
            "{UI_MOD_REL_PATH}: the render section no longer calls `{RISKY_CALL}` — this \
             scanner needs updating"
        )),
        (None, Some(_)) => violations.push(format!(
            "{UI_MOD_REL_PATH}: the render section calls `{RISKY_CALL}` with no `{GATE}` guard \
             anywhere in the section — `render_if_needed` would flush dirty regions over SPI to \
             a fully asleep (SLPIN+DISPOFF) panel, pure wasted bus/CPU time while asleep \
             (meshcadet-power-optimization Phase 5)"
        )),
        (Some(g), Some(r)) if g > r => violations.push(format!(
            "{UI_MOD_REL_PATH}: the render section checks `{GATE}` AFTER `{RISKY_CALL}` instead \
             of before — a gate checked once the render call already ran cannot prevent it"
        )),
        _ => {}
    }

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

    /// The actual guard: the shipped `firmware/src/ui/mod.rs` gates
    /// `render_if_needed` on `render_gate(self.screen_asleep)`.
    #[test]
    fn render_asleep_gate_contract_holds() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "render-while-asleep gate violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    /// Minimal synthetic stand-in for `step()`'s render section, close
    /// enough in shape to exercise this scanner without depending on the
    /// real (large, frequently-edited) file.
    fn synthetic_step(gated: bool) -> String {
        let render_block = if gated {
            r#"
            // ── Render dirty regions ──
            if render_gate(self.screen_asleep) {
                let render_due = true;
                if render_due {
                    self.window.render_if_needed(&mut self.display)?;
                }
            }
            "#
        } else {
            r#"
            // ── Render dirty regions ──
            let render_due = true;
            if render_due {
                self.window.render_if_needed(&mut self.display)?;
            }
            "#
        };
        format!(
            "{render_block}\n            Ok(())\n        }}\n\n        fn handle_event(&mut self, event: UiEvent, now_ms: u64) {{\n"
        )
    }

    #[test]
    fn synthetic_gated_is_clean() {
        assert_eq!(check_source(&synthetic_step(true)), Vec::<String>::new());
    }

    /// The mutation this guard exists for: the render section loses its
    /// `render_gate` check, so `render_if_needed` runs unconditionally even
    /// while the panel is fully asleep.
    #[test]
    fn synthetic_ungated_is_caught() {
        let violations = check_source(&synthetic_step(false));
        assert!(
            violations
                .iter()
                .any(|v| v.contains("with no `render_gate(self.screen_asleep)` guard")),
            "expected an ungated violation, got {violations:?}"
        );
    }

    /// Ordering matters, not mere presence: a gate that exists in the
    /// section but AFTER the render call already ran is exactly as
    /// ineffective as no gate at all.
    #[test]
    fn synthetic_gate_after_the_risky_call_is_caught() {
        let src = synthetic_step(true).replace(
            "if render_gate(self.screen_asleep) {\n                let render_due = true;\n                if render_due {\n                    self.window.render_if_needed(&mut self.display)?;\n                }\n            }",
            "self.window.render_if_needed(&mut self.display)?;\n            if render_gate(self.screen_asleep) {}",
        );
        let violations = check_source(&src);
        assert!(
            violations.iter().any(|v| v.contains("AFTER")),
            "expected an ordering violation, got {violations:?}"
        );
    }

    /// A comment merely MENTIONING the gate, with no real code check, must
    /// not satisfy this scanner — `tokenize()` blanks comment bodies before
    /// the keyword search runs.
    #[test]
    fn comment_only_mention_does_not_satisfy_the_gate() {
        let src = synthetic_step(false).replace(
            "let render_due = true;",
            "// render_gate(self.screen_asleep) is handled elsewhere, trust me\n                let render_due = true;",
        );
        let violations = check_source(&src);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("with no `render_gate(self.screen_asleep)` guard")),
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
