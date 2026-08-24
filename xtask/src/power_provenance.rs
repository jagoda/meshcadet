// SPDX-License-Identifier: GPL-3.0-only
//! Mechanizes ADR-0014 D2's estimate-labelling rule: "Every power number
//! this campaign introduces — anywhere: this ADR, a PR body, a code
//! comment, a later write-up — is a labelled estimate with its reasoning
//! basis stated inline, never a bare number... A bare number without one of
//! the three tags above is a defect in whatever document contains it."
//!
//! D2 was prose-only when first written and broke inside the very document
//! that authored it (ADR-0014 D5 row 4 shipped two untagged power figures
//! at the same landing) — a rule that only lives in a reviewer's head is a
//! rule that lapses the next time someone drops a number into a table cell
//! under review pressure. This module is the host-runnable, mechanical half
//! of D2, in the same spirit as `xtask::check`'s glyph-coverage harness:
//! plain text scanning, no toolchain required, run by `cargo test` on every
//! change to `docs/`.
//!
//! # Scope
//!
//! Every `.md` file under `docs/`, scanned for a power-current figure
//! (`\d[\d.,–—-]*\s*(mA|µA|uA)`, e.g. `20–30 mA`, `4.6 mA`, `<1 mA`) is
//! required to carry one of D2's three tag markers — `[DATASHEET]`,
//! `[ESTIMATE`, `[MEASURED` — within [`TAG_WINDOW_CHARS`] characters of the
//! figure, searched in EITHER direction, without crossing a paragraph break
//! (`\n\n`), the start of the next markdown table row (`\n| `), or a
//! NEIGHBORING power figure (`tag_nearby`'s doc). That neighbor bound
//! matters as much as the paragraph/row one: a sentence or table cell can
//! pack two unrelated power figures close together ("the co-processor
//! draws roughly 7-14 mA continuously, whereas the radio draws 4.6 mA
//! `[DATASHEET]`"), and a tag that actually belongs to the SECOND figure
//! must never be credited backward to the first, untagged one — M1-gate R2
//! fold-in 4 (`meshcadet-power-optimization` Phase 4), which found the
//! original forward-only window silently passing exactly that shape.
//!
//! # Deliberately not checked
//!
//! D2 also says a tag "must state its reasoning basis inline... not just
//! carry the bracket" — whether an `[ESTIMATE]`'s bracket actually contains
//! a real duty-cycle/datasheet argument, versus reading as a guess wearing
//! an estimate's tag, is a judgment call this scanner does not attempt to
//! automate. Presence of a tag marker near the figure is the mechanizable
//! half of D2; reasoning-basis quality stays a review-time judgment, the
//! same posture D3.3 takes for "not every leg earns an xtask guard" — not
//! every clause of a rule is equally mechanizable, and pretending otherwise
//! would trade a real gap for a false sense of coverage.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

/// How far past a power figure's unit to search for a provenance tag before
/// giving up. Generous enough to span a markdown inline-code tag placed
/// immediately after the figure plus a short trailing clause; the
/// paragraph/table-row cutoff (not this constant alone) is what actually
/// stops a distant, unrelated tag from being credited.
const TAG_WINDOW_CHARS: usize = 400;

/// One power-figure-without-provenance-tag violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub file: String,
    pub line: usize,
    pub figure: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}: power figure {:?} has no [DATASHEET]/[ESTIMATE]/[MEASURED] \
             provenance tag within {TAG_WINDOW_CHARS} chars — ADR-0014 D2: \"a bare \
             number without one of the three tags is a defect in whatever document \
             contains it.\"",
            self.file, self.line, self.figure
        )
    }
}

/// Find every markdown file under `dir`, recursively.
fn find_markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            find_markdown_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// Find every `[DATASHEET|ESTIMATE|MEASURED ... ]` bracket's `(start, end)`
/// char-offset span (end is exclusive, past the closing `]`). A figure
/// cited INSIDE another figure's tag bracket — e.g. row 9's `[ESTIMATE]`
/// arithmetic cites row 4's `~20–30 mA` datasheet current as part of its own
/// reasoning basis — is already provenance-covered by that enclosing tag
/// and must not need a second, separate tag of its own.
fn tag_spans(text: &str, tag_re: &Regex) -> Vec<(usize, usize)> {
    tag_re
        .find_iter(text)
        .map(|m| {
            let end = text[m.start()..]
                .find(']')
                .map(|off| m.start() + off + 1)
                .unwrap_or(text.len());
            (m.start(), end)
        })
        .collect()
}

/// Whether a tag lies "nearby" `[fig_start, fig_end)` in EITHER direction —
/// M1-gate R2 fold-in 4 (`meshcadet-power-optimization` Phase 4): the
/// original forward-only window silently passed "the co-processor draws
/// roughly 7-14 mA continuously, whereas the radio draws 4.6 mA
/// `[DATASHEET]`." by crediting the SECOND figure's tag to the first,
/// unrelated one — a forward search from `7-14 mA` reached right past `4.6
/// mA` and found its tag with nothing to stop it. The fix bounds EACH
/// direction's window not just by the existing paragraph/table-row
/// boundary, but also by the nearest NEIGHBORING power figure
/// (`prev_end`/`next_start`) — a tag beyond that point belongs (or may
/// belong) to that neighbor, never to this figure, so it must never be
/// credited across it. Searching backward too (not just forward) is what
/// correctly credits `4.6 mA` itself (whose tag sits immediately after it,
/// found by ITS OWN forward search) while still flagging `7-14 mA`, which
/// has no tag in a window bounded on either side by its neighbors.
fn tag_nearby(
    text: &str,
    fig_start: usize,
    fig_end: usize,
    prev_end: usize,
    next_start: usize,
    tag_re: &Regex,
    boundary_re: &Regex,
) -> bool {
    // Forward: bounded by TAG_WINDOW_CHARS, text end, the next figure's
    // start, and a paragraph/table-row break — whichever comes first.
    let fwd_end = fig_end
        .saturating_add(TAG_WINDOW_CHARS)
        .min(text.len())
        .min(next_start);
    let mut fwd = &text[fig_end..fwd_end.max(fig_end)];
    if let Some(bm) = boundary_re.find(fwd) {
        fwd = &fwd[..bm.start()];
    }
    if tag_re.is_match(fwd) {
        return true;
    }
    // Backward: same bounds, mirrored — TAG_WINDOW_CHARS, text start, the
    // previous figure's end, and a paragraph/table-row break, whichever is
    // nearest to `fig_start`.
    let bwd_start = fig_start.saturating_sub(TAG_WINDOW_CHARS).max(prev_end);
    let mut bwd = &text[bwd_start.min(fig_start)..fig_start];
    if let Some(bm) = boundary_re.find_iter(bwd).last() {
        bwd = &bwd[bm.end()..];
    }
    tag_re.is_match(bwd)
}

/// Pure-logic check over one document's text: returns `(1-indexed line,
/// matched figure text)` for every power figure with no D2 tag nearby.
/// Independent of file I/O so it has its own fast synthetic-fixture tests —
/// see [`check`] for the file-reading entry point that walks `docs/`.
pub fn check_text(text: &str) -> Vec<(usize, String)> {
    let figure_re = Regex::new(r"[0-9][0-9.,–—-]*\s*(?:mA|µA|uA)\b").unwrap();
    let tag_re = Regex::new(r"\[(?:DATASHEET|ESTIMATE|MEASURED)").unwrap();
    // Whichever comes first bounds the search window: a blank line
    // (paragraph break) or the start of the next markdown table row.
    let boundary_re = Regex::new(r"\n\n|\n\|").unwrap();
    let spans = tag_spans(text, &tag_re);

    // Every power-figure match's `(start, end)`, INCLUDING ones inside a
    // tag's own bracket (excluded from violation-checking below, but still
    // real neighbor positions that must bound an adjacent figure's window)
    // — see `tag_nearby`'s doc.
    let figures: Vec<(usize, usize)> = figure_re
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect();

    let mut violations = Vec::new();
    for (i, &(fig_start, fig_end)) in figures.iter().enumerate() {
        // Already inside another tag's own bracket (cited as that tag's
        // reasoning basis) — covered, no separate tag required.
        if spans.iter().any(|(s, e)| *s <= fig_start && fig_start < *e) {
            continue;
        }
        let prev_end = figures
            .get(i.wrapping_sub(1))
            .filter(|_| i > 0)
            .map(|&(_, e)| e)
            .unwrap_or(0);
        let next_start = figures.get(i + 1).map(|&(s, _)| s).unwrap_or(text.len());
        if !tag_nearby(
            text,
            fig_start,
            fig_end,
            prev_end,
            next_start,
            &tag_re,
            &boundary_re,
        ) {
            let line = text[..fig_start].matches('\n').count() + 1;
            violations.push((line, text[fig_start..fig_end].trim().to_string()));
        }
    }
    violations
}

/// Run the full check over every `.md` file under `docs/`.
///
/// `repo_root`: path to the MeshCadet repository root (containing `docs/`).
pub fn check(repo_root: &Path) -> Vec<Violation> {
    let docs_dir = repo_root.join("docs");
    let mut files = Vec::new();
    find_markdown_files(&docs_dir, &mut files);
    files.sort();

    let mut violations = Vec::new();
    for path in files {
        let text =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let rel = path
            .strip_prefix(repo_root)
            .unwrap_or(path.as_path())
            .to_string_lossy()
            .to_string();
        for (line, figure) in check_text(&text) {
            violations.push(Violation {
                file: rel.clone(),
                line,
                figure,
            });
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard for the exact defect this module exists to catch:
    /// ADR-0014 D5 row 4 shipped two bare power figures with no provenance
    /// tag anywhere nearby. This is a pure-logic reconstruction of that
    /// case's shape, independent of the live repo's current (now fixed)
    /// text, so a future edit that re-introduces the same shape of defect
    /// is still caught even after the live-repo instance is gone.
    #[test]
    fn seeded_violation_bare_power_figures_are_detected() {
        const SEEDED: &str = "the exclusion is cheap: genuinely small next to the GPS \
             receiver's ~20–30 mA and the backlight's 40–100 mA. Confirmed correct as \
             built.";
        let violations = check_text(SEEDED);
        assert_eq!(
            violations.len(),
            2,
            "expected both bare figures caught, got: {violations:?}"
        );
        assert!(violations.iter().any(|(_, f)| f.contains("20–30 mA")));
        assert!(violations.iter().any(|(_, f)| f.contains("40–100 mA")));
    }

    /// Pass case: a figure immediately followed by a tagged, inline-code
    /// bracket (the real shape every figure in ADR-0014 carries after this
    /// mission's fix) is not flagged.
    #[test]
    fn tagged_figure_with_basis_is_not_flagged() {
        const CLEAN: &str = "SX1262 continuous RX is order 4.6–5.5 mA `[DATASHEET]` \
             (continuous-RX current) — genuinely small.";
        assert!(check_text(CLEAN).is_empty());
    }

    /// Pass case: `[ESTIMATE]` and `[MEASURED]` are each accepted as a tag
    /// marker, not just `[DATASHEET]`.
    #[test]
    fn estimate_and_measured_tags_are_both_accepted() {
        assert!(check_text("order ~20 mA `[ESTIMATE — duty-cycle basis]` foregone").is_empty());
        assert!(check_text("read as 12 mA `[MEASURED, 2026-08-24, bench multimeter]`").is_empty());
    }

    /// A figure cited INSIDE another tag's own bracket, as part of that
    /// tag's stated reasoning basis (row 9's real shape: its `[ESTIMATE]`
    /// arithmetic cites row 4's `~20–30 mA` datasheet current by name), is
    /// already provenance-covered by the enclosing tag and must not be
    /// flagged as a second, separately-untagged figure.
    #[test]
    fn figure_cited_inside_another_tags_bracket_is_not_flagged() {
        const NESTED: &str = "order ~20 mA `[ESTIMATE — 80% duty fraction × the ~20–30 mA \
             datasheet current above, nets order ~20 mA of average draw removed]` foregone";
        assert!(check_text(NESTED).is_empty());
    }

    /// A tag belonging to a DIFFERENT, later markdown table row must not be
    /// credited to a bare figure in the row above — the whole reason the
    /// window is bounded at a table-row boundary, not left open-ended.
    #[test]
    fn tag_in_next_table_row_is_not_credited_to_previous_row() {
        const TWO_ROWS: &str = "| 4 | Radio duty-cycling | genuinely small next to ~20–30 mA \
             and the backlight's 40–100 mA |\n\
             | 5 | Something else | 12 mA `[DATASHEET]` unrelated figure |\n";
        let violations = check_text(TWO_ROWS);
        assert_eq!(
            violations.len(),
            2,
            "row 4's two bare figures must still be caught even though row 5 has a tag; \
             got: {violations:?}"
        );
    }

    /// A tag on the far side of a paragraph break must not be credited to a
    /// bare figure in the paragraph above.
    #[test]
    fn tag_across_paragraph_break_is_not_credited() {
        const TWO_PARAS: &str = "The GPS receiver draws ~20–30 mA continuously.\n\n\
             A wholly different number, 5 mA `[ESTIMATE]`, appears in the next paragraph.";
        let violations = check_text(TWO_PARAS);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].1.contains("20–30 mA"));
    }

    /// A tag split across a single word-wrap newline (not a paragraph break
    /// or table-row boundary) IS still credited — this is the real shape
    /// ADR-0014's own hard-wrapped prose uses (a figure at the end of one
    /// line, its `[ESTIMATE]` tag opening on the very next line).
    #[test]
    fn tag_across_single_wrapped_newline_is_still_credited() {
        const WRAPPED: &str = "order ~20 mA\n`[ESTIMATE]` — not landed for either variant";
        assert!(check_text(WRAPPED).is_empty());
    }

    // ── Bidirectional window (M1-gate R2 fold-in 4) ─────────────────────

    /// Regression guard for the exact reversed-order shape fold-in 4 found
    /// silently passing: a BARE figure that PRECEDES a tagged one in the
    /// same sentence. Probed live against `4f8c4d2` — this is verbatim the
    /// text that reported OK/exit 0 under the old forward-only window.
    #[test]
    fn seeded_bare_figure_preceding_a_tagged_one_is_detected() {
        const REVERSED: &str = "The co-processor draws roughly 7-14 mA continuously, \
             whereas the radio draws 4.6 mA `[DATASHEET]`.";
        let violations = check_text(REVERSED);
        assert_eq!(
            violations.len(),
            1,
            "only the untagged first figure should be flagged; got: {violations:?}"
        );
        assert!(
            violations[0].1.contains("7-14 mA"),
            "the tag belongs to the SECOND figure (4.6 mA) and must not be credited \
             backward to the first; got: {violations:?}"
        );
    }

    /// A tag that PRECEDES its own figure (the backward-search direction
    /// itself, independent of the neighbor-bounding fix above) must still
    /// credit it — `tag_nearby`'s search is genuinely bidirectional, not
    /// just forward-with-a-tighter-bound.
    #[test]
    fn tag_preceding_its_own_figure_is_credited() {
        assert!(check_text("`[ESTIMATE]` gives roughly 20 mA of savings.").is_empty());
    }

    /// Two adjacent, EACH separately tagged figures in the same sentence
    /// must both pass — the neighbor bound must not accidentally strip a
    /// figure's OWN adjacent tag just because another figure sits nearby.
    #[test]
    fn two_adjacent_figures_each_with_their_own_tag_both_pass() {
        const BOTH_TAGGED: &str = "The co-processor draws 7-14 mA `[DATASHEET]` \
             continuously, whereas the radio draws 4.6 mA `[DATASHEET]`.";
        assert!(check_text(BOTH_TAGGED).is_empty());
    }

    /// Non-power numbers (no `mA`/`µA`/`uA` unit) must never be flagged —
    /// this guard is scoped to power-current figures only, e.g. `240 MHz`
    /// clock speeds are out of scope entirely.
    #[test]
    fn non_power_numbers_are_ignored() {
        assert!(check_text("both cores fixed at 240 MHz").is_empty());
    }

    /// Integration pass case: the live repo's `docs/` tree, after this
    /// mission's fix, must be clean — the actual guard wired into `xtask
    /// verify-power-provenance` / `cargo test`, not just the pure logic in
    /// isolation.
    #[test]
    fn power_provenance_check_passes_on_live_repo() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "\npower-provenance check found {} violation(s):\n{}\n",
            violations.len(),
            violations
                .iter()
                .map(|v| format!("  - {v}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
