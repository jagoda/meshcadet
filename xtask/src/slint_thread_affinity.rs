// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard for ADR-0012 R8's Slint thread-affinity barrier.
//!
//! # Why this exists (`meshcadet-slint-affinity-static-guard`)
//!
//! `firmware/Cargo.toml` builds `slint`/`i-slint-core` with
//! `unsafe-single-threaded`, which **removes** Slint's own thread-affinity
//! checks rather than satisfying them: every Slint interaction — platform
//! registration, bitmap-font registration, window-adapter creation, every
//! property write, every render — must happen on one and the same thread
//! (`ui_task`), and nothing in the build tells you if it does not. A stray
//! call from the dispatcher task is silent UB on a dual-core device, not a
//! build failure — and a UB-wedged UI task is a priority-1 (message
//! delivery) risk, not merely a UI one, since a wedged task's TWDT reboot
//! takes the radio down with it.
//!
//! `firmware/src/ui_task.rs` used to claim this was enforced by Rust
//! visibility: `mod ui;` is declared at the crate root
//! (`firmware/src/main.rs`) and `UiRuntime` is plain `pub`
//! (`firmware/src/ui/mod.rs`), so **any** module in the crate can already
//! write `crate::ui::UiRuntime` and it compiles — privacy in Rust attaches
//! to the item and is visible to the defining scope and all its
//! descendants, and the defining scope here is the crate root, whose
//! descendants are every module in the crate. There is no `pub(crate)`
//! boundary doing the claimed work. The barrier was, and is, a convention
//! documented in comments — this module is what turns it into something a
//! green `cargo test` actually certifies.
//!
//! # Why this lives in `xtask` and not in `firmware`
//!
//! Same reason the glyph-coverage and UI-event-parity harnesses do (see
//! this crate's module doc and [`crate::ui_event_parity`]'s): the
//! `firmware` crate's single `[[bin]]` target sets `harness = false`, so
//! `cargo test` only *type-checks* its `#[cfg(test)]` blocks and never
//! executes one. A `#[test]` asserting what the rest of the crate names
//! therefore cannot live there. This module is the host-runnable
//! equivalent — plain text scanning, no `esp` toolchain required.
//!
//! # What this checks
//!
//! Every `.rs` file under `firmware/src/`, **except** everything under
//! `firmware/src/ui/` (the UI implementation itself) and
//! `firmware/src/ui_task.rs` (the one file ADR-0012 D4.2 designates as the
//! boundary — it is allowed, indeed required, to name `UiRuntime` and the
//! Slint API in order to construct and drive it), must not name, in
//! non-comment/non-string source:
//!
//! - `UiRuntime` (the runtime type itself);
//! - `slint::` (any path into the `slint` crate);
//! - `i_slint*` (any path into an `i-slint-core`-family crate — the
//!   internals `firmware/src/ui/platform.rs` reaches into for bitmap-font
//!   registration).
//!
//! A hit means code outside the boundary is talking to Slint directly
//! rather than through `ui_task`'s `UiEvent`/`UiCommand` channel contract
//! (ADR-0012 D3) — exactly the pattern R8 exists to prevent.
//!
//! Comment and string-literal bodies are masked out first via
//! [`crate::tokenize`] (the same tokenizer the glyph-coverage and
//! UI-event-parity harnesses use) precisely because this codebase's own
//! doc comments mention `UiRuntime`/`slint::`/`i_slint_core` constantly when
//! *describing* the barrier (this module's own doc above is a case in
//! point) — those mentions must never be mistaken for a violation.
//!
//! # Honest limits
//!
//! This is a **name-scan**, not a type-checker: it cannot see through a
//! re-export, a macro expansion, or a renamed `use` (`use slint as s;`
//! followed by `s::Foo`). Widening it to catch those is legitimate future
//! work if one is ever observed; per this crate's "parse gap = NO-GO"
//! doctrine elsewhere, a scanner that cannot see a construct should fail
//! loud rather than silently pass it — but a rename import is not a
//! plausible accident to guard against today, only a deliberate evasion,
//! which review (not this scanner) is the backstop for. What matters is
//! that the *un-evaded* case — the actual failure mode observed and fixed
//! historically in this codebase (a stray `UiRuntime`/`slint::` mention
//! creeping into a file outside the boundary) — is now a `cargo test`
//! failure instead of a comment nobody re-reads.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::tokenize;

/// The one file, besides everything under [`UI_DIR_REL_PATH`], allowed to
/// name `UiRuntime` / Slint symbols — the boundary module itself (ADR-0012
/// D4.2).
pub const UI_TASK_REL_PATH: &str = "firmware/src/ui_task.rs";

/// Directory (relative to repo root) whose entire tree is exempt — the UI
/// implementation itself.
pub const UI_DIR_REL_PATH: &str = "firmware/src/ui";

/// One forbidden-symbol sighting: `file` named `symbol` outside the exempt
/// boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub file: String,
    pub symbol: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: names `{}` in non-comment source outside {UI_DIR_REL_PATH}/ and \
             {UI_TASK_REL_PATH} — ADR-0012 R8's Slint thread-affinity barrier is a convention, \
             not a compiler-enforced one (firmware/Cargo.toml's `unsafe-single-threaded` means \
             a call from the wrong task is silent UB, not a build error). Route through \
             ui_task's UiEvent/UiCommand channel contract instead of naming Slint directly.",
            self.file, self.symbol
        )
    }
}

/// The three forbidden-symbol patterns, run over masked (comment/string-
/// blanked) text — see module doc.
fn forbidden_patterns() -> [Regex; 3] {
    [
        Regex::new(r"\bUiRuntime\b").unwrap(),
        Regex::new(r"\bslint::").unwrap(),
        Regex::new(r"\bi_slint[A-Za-z0-9_]*\b").unwrap(),
    ]
}

/// Scan one already-read file's source for forbidden-symbol sightings, pure
/// text in/out — kept free of file I/O so it has its own fast, synthetic
/// unit tests independent of the live repo's current source (see [`check`]
/// for the real, file-reading entry point that feeds it every non-exempt
/// file under `firmware/src/`).
pub fn violations_in_source(file_label: &str, src: &str) -> Vec<Violation> {
    let masked = tokenize(src).masked;
    let mut symbols: Vec<String> = forbidden_patterns()
        .iter()
        .flat_map(|re| re.find_iter(&masked).map(|m| m.as_str().to_string()))
        .collect();
    symbols.sort();
    symbols.dedup();
    symbols
        .into_iter()
        .map(|symbol| Violation {
            file: file_label.to_string(),
            symbol,
        })
        .collect()
}

/// Recursively collect every `.rs` file under `dir`, sorted, for
/// deterministic output.
///
/// Panics (rather than silently skipping) on a directory it cannot read —
/// per this crate's "parse gap = NO-GO" doctrine (see the glyph-coverage
/// harness's module doc): a swallowed `read_dir` error here would make
/// [`check`] silently scan fewer files than it should and report a false
/// "barrier holds" on an incomplete scan, exactly the failure mode a guard
/// like this exists to rule out.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("reading directory {}: {e}", dir.display()));
    let mut entries: Vec<PathBuf> = entries
        .map(|e| e.unwrap_or_else(|e| panic!("reading entry under {}: {e}", dir.display())))
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Run the full guard: every `.rs` file under `firmware/src/`, except
/// [`UI_DIR_REL_PATH`]'s tree and [`UI_TASK_REL_PATH`] itself, must not name
/// a forbidden symbol in non-comment source. Empty vec == the barrier
/// holds.
///
/// `repo_root`: path to the MeshCadet repository root (containing
/// `firmware/`).
pub fn check(repo_root: &Path) -> Vec<Violation> {
    let src_dir = repo_root.join("firmware/src");
    let ui_dir = repo_root.join(UI_DIR_REL_PATH);
    let ui_task_path = repo_root.join(UI_TASK_REL_PATH);

    let mut files = Vec::new();
    collect_rs_files(&src_dir, &mut files);

    let mut violations = Vec::new();
    for path in files {
        if path.starts_with(&ui_dir) || path == ui_task_path {
            continue;
        }
        let src =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let rel = path
            .strip_prefix(repo_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        violations.extend(violations_in_source(&rel, &src));
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual guard: today's shipped `firmware/src/` (outside the
    /// exempt boundary) does not name `UiRuntime`, `slint::`, or any
    /// `i_slint*` symbol. This is the M1 task-split checkpoint's own grep
    /// finding ("ZERO Slint symbols exist outside firmware/src/ui/ today"),
    /// now certified by `cargo test` instead of asserted by hand. It also
    /// doubles as the regression guard for this scanner's own exemption
    /// logic: `firmware/src/ui_task.rs` and every file under
    /// `firmware/src/ui/` are packed with exactly these symbols in real
    /// code (not comments) — if the exclusion filter above were ever
    /// broken, this assertion would fail immediately and loudly with a
    /// large violation list, not silently pass.
    #[test]
    fn slint_affinity_barrier_holds() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "\nADR-0012 R8 Slint thread-affinity guard found {} violation(s):\n  - {}\n",
            violations.len(),
            violations
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join("\n  - ")
        );
    }

    #[test]
    fn flags_bare_uiruntime_use_in_code() {
        let violations = violations_in_source("main.rs", "use crate::ui::UiRuntime;\nfn f() {}\n");
        assert_eq!(
            violations,
            vec![Violation {
                file: "main.rs".to_string(),
                symbol: "UiRuntime".to_string(),
            }]
        );
    }

    #[test]
    fn flags_slint_path_symbol_in_code() {
        let violations = violations_in_source("dispatcher.rs", "let w: slint::Weak<X> = w;\n");
        assert_eq!(
            violations,
            vec![Violation {
                file: "dispatcher.rs".to_string(),
                symbol: "slint::".to_string(),
            }]
        );
    }

    #[test]
    fn flags_i_slint_core_symbol_in_code() {
        let violations =
            violations_in_source("radio.rs", "use i_slint_core::graphics::BitmapFont;\n");
        assert_eq!(
            violations,
            vec![Violation {
                file: "radio.rs".to_string(),
                symbol: "i_slint_core".to_string(),
            }]
        );
    }

    #[test]
    fn line_comment_mentions_are_not_flagged() {
        let violations = violations_in_source(
            "gps.rs",
            "// starving ui::UiRuntime::step() / slint::platform / i_slint_core internals\n",
        );
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn doc_comment_and_string_literal_mentions_are_not_flagged() {
        let violations = violations_in_source(
            "admin_server.rs",
            "/// no reach into ui::UiRuntime's slint::Weak or i_slint_core state\nfn f() { let s = \"UiRuntime slint:: i_slint_core\"; }\n",
        );
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn block_comment_mentions_are_not_flagged() {
        let violations = violations_in_source(
            "history_store.rs",
            "/* seeds UiRuntime via slint:: and i_slint_core, see ui_task */\nfn f() {}\n",
        );
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn does_not_confuse_unrelated_ui_module_symbols() {
        // `ui::UiEvent`/`ui::UiCommand` are the deliberate, shared channel
        // contract (ADR-0012 D3) — naming those from outside the boundary
        // is the whole point, not a violation.
        let violations = violations_in_source(
            "main.rs",
            "fn send(tx: &SyncSender<ui::UiEvent>, cmd: ui::UiCommand) {}\n",
        );
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn multiple_violations_in_one_file_are_all_reported_and_deduped() {
        let violations = violations_in_source(
            "main.rs",
            "use crate::ui::UiRuntime;\nfn f(u: UiRuntime) { let _ = slint::Weak::<u32>::new(); let _ = slint::Image::default(); }\n",
        );
        let mut symbols: Vec<String> = violations.into_iter().map(|v| v.symbol).collect();
        symbols.sort();
        assert_eq!(
            symbols,
            vec!["UiRuntime".to_string(), "slint::".to_string()]
        );
    }
}
