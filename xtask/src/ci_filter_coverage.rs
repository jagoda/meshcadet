// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard: every root Cargo workspace member is
//! explicitly wired into `.github/workflows/ci.yml`'s `changes` job path
//! filter.
//!
//! # Why this exists
//!
//! `perf_device_report` was added to root `Cargo.toml`'s `[workspace]
//! members` array without a matching `perf_device_report/**` entry in
//! ci.yml's `dorny/paths-filter` `host:` list. Since `full`/`host`/
//! `firmware` used to have no catch-all, a PR scoped entirely to that crate
//! set all three outputs false and skipped `test`/`fmt`/`clippy` outright —
//! silently zeroing out `perf_device_report`'s ~34 unit tests plus
//! `tests/parse_report.rs` for every such PR. ci.yml's `host` filter now
//! carries a fail-safe catch-all (`'**'` / `'!firmware/**'`, see that file's
//! header comment) that makes this specific failure mode unreachable going
//! forward — any path not under `firmware/` runs the host lane regardless
//! of whether it's individually enumerated.
//!
//! That catch-all is a safety net, not a substitute for correct
//! categorization: a NEW root-workspace member that `firmware/` also
//! path-deps on (the way `protocol`/`firmware-core` do — see Cargo.toml's
//! own doc comment) needs to land in `full`, not just fall through to
//! `host`'s catch-all, or the `firmware` job never re-runs when that
//! member changes. Silently relying on the catch-all would hide exactly
//! that miscategorization. This guard makes the omission itself loud and
//! immediate — at the PR that adds the member, not discovered later — by
//! failing whenever a `members` entry has no explicit `<name>/**` pattern
//! in ci.yml's `full:` or `host:` filter lists, regardless of whether the
//! catch-all would have covered it anyway.
//!
//! # Scope
//!
//! Plain text scanning of two files, no toolchain required, in the same
//! spirit as this crate's other static guards (see `xtask::check`'s
//! module doc):
//!
//! - Root `Cargo.toml`'s `[workspace] members = [...]` array (single- or
//!   multi-line — same shape `scripts/sync-cargo-lock-versions.sh` already
//!   has to parse).
//! - `.github/workflows/ci.yml`'s `filters: |` block scalar (the
//!   `dorny/paths-filter` YAML embedded in the `changes` job), bucketed by
//!   its `full:`/`host:`/`firmware:` list headers.
//!
//! A member is "covered" if either bucket contains a literal, non-negated
//! `"<member>/**"` pattern. The `firmware` bucket and any `!`-prefixed
//! negation pattern are deliberately never treated as coverage — `firmware`
//! itself is a DETACHED workspace, never a root member, and a negation
//! pattern subtracts from a match rather than granting one.

use std::fs;
use std::path::Path;

use regex::Regex;

/// One root-workspace member with no explicit `<name>/**` entry in
/// ci.yml's `full:`/`host:` filter lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub member: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Cargo.toml's [workspace] members includes \"{}\", but \
             .github/workflows/ci.yml's `changes` job path filter has no \
             \"{}/**\" entry under `full:` or `host:` — add one (to `full` \
             if firmware/ path-deps on it, otherwise `host`) rather than \
             relying solely on the `host` catch-all.",
            self.member, self.member
        )
    }
}

/// Parse root `Cargo.toml`'s `[workspace] members = [...]` array —
/// tolerant of both the single-line and the one-per-line multi-line shape
/// (see `scripts/sync-cargo-lock-versions.sh`, which parses the same
/// array).
fn parse_members(cargo_toml_text: &str) -> Vec<String> {
    // `^members` (not just `members`) so this can never match a
    // `default-members = [...]` array that happens to precede the real
    // `[workspace] members` one — `members` would match as a bare substring
    // of `default-members` with no anchor.
    let members_re = Regex::new(r"(?ms)^members\s*=\s*\[(.*?)\]").unwrap();
    let Some(caps) = members_re.captures(cargo_toml_text) else {
        return Vec::new();
    };
    let body = &caps[1];
    let entry_re = Regex::new(r#""([^"]+)""#).unwrap();
    entry_re
        .captures_iter(body)
        .map(|c| c[1].to_string())
        .collect()
}

/// Bucket ci.yml's `filters: |` block scalar by its top-level list keys
/// (`full:`, `host:`, `firmware:`), returning each bucket's raw list-item
/// text (quotes stripped, negation `!` prefix left intact so callers can
/// distinguish a positive pattern from an exclusion).
fn parse_filter_buckets(ci_yml_text: &str) -> Vec<(String, Vec<String>)> {
    let lines: Vec<&str> = ci_yml_text.lines().collect();
    let Some(filters_idx) = lines.iter().position(|l| l.trim_start() == "filters: |") else {
        return Vec::new();
    };
    let filters_indent = lines[filters_idx].len() - lines[filters_idx].trim_start().len();

    let mut buckets: Vec<(String, Vec<String>)> = Vec::new();
    let mut current: Option<usize> = None;

    for line in &lines[filters_idx + 1..] {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent <= filters_indent {
            break;
        }
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(key) = trimmed.strip_suffix(':') {
            if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                buckets.push((key.to_string(), Vec::new()));
                current = Some(buckets.len() - 1);
                continue;
            }
        }
        if let Some(rest) = trimmed.strip_prefix("- ") {
            let pattern = rest.trim().trim_matches('\'').trim_matches('"').to_string();
            if let Some(i) = current {
                buckets[i].1.push(pattern);
            }
        }
    }
    buckets
}

/// Pure-logic check over the two files' already-read text — see [`check`]
/// for the file-reading entry point.
pub fn check_texts(cargo_toml_text: &str, ci_yml_text: &str) -> Vec<Violation> {
    let members = parse_members(cargo_toml_text);
    let buckets = parse_filter_buckets(ci_yml_text);

    let covered = |member: &str| -> bool {
        let wanted = format!("{member}/**");
        buckets.iter().any(|(name, patterns)| {
            (name == "full" || name == "host") && patterns.iter().any(|p| p == &wanted)
        })
    };

    members
        .into_iter()
        .filter(|m| !covered(m))
        .map(|member| Violation { member })
        .collect()
}

/// Run the full check against the live repo's root `Cargo.toml` and
/// `.github/workflows/ci.yml`.
///
/// `repo_root`: path to the MeshCadet repository root.
pub fn check(repo_root: &Path) -> Vec<Violation> {
    let cargo_toml = fs::read_to_string(repo_root.join("Cargo.toml"))
        .unwrap_or_else(|e| panic!("reading Cargo.toml: {e}"));
    let ci_yml = fs::read_to_string(repo_root.join(".github/workflows/ci.yml"))
        .unwrap_or_else(|e| panic!("reading .github/workflows/ci.yml: {e}"));
    // Fail LOUD, not vacuously green, if `parse_members` can't find the
    // `[workspace] members = [...]` array at all (e.g. a future reformat
    // this regex no longer recognizes) — an empty member list makes every
    // downstream `covered()` check trivially pass with zero violations,
    // which would read identically to "every real member is correctly
    // wired." The live repo always has real members; zero is never a
    // legitimate result here.
    assert!(
        !parse_members(&cargo_toml).is_empty(),
        "parsed zero [workspace] members from Cargo.toml — the parsing regex in \
         xtask::ci_filter_coverage::parse_members likely needs updating for a \
         reformatted members array; left as-is, this guard would silently report \
         zero coverage violations regardless of actual state"
    );
    check_texts(&cargo_toml, &ci_yml)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CI_YML_FIXTURE: &str = r#"
jobs:
  changes:
    steps:
      - uses: dorny/paths-filter@v4
        id: filter
        with:
          predicate-quantifier: 'some-with-excludes'
          filters: |
            # comment
            full:
              - 'Cargo.toml'
              - 'protocol/**'
            host:
              - 'host/**'
              - 'xtask/**'
              - '**'
              - '!firmware/**'
            firmware:
              - 'firmware/**'

  other-job:
    steps:
      - run: echo hi
"#;

    /// Regression guard for the exact defect this module exists to catch:
    /// a root-workspace member (`perf_device_report`) present in
    /// Cargo.toml's `members` array with no matching `<name>/**` entry in
    /// either ci.yml filter bucket.
    #[test]
    fn seeded_missing_member_is_flagged() {
        let cargo_toml = r#"
[workspace]
members = [
    "protocol",
    "host",
    "xtask",
    "perf_device_report",
]
"#;
        let violations = check_texts(cargo_toml, CI_YML_FIXTURE);
        assert_eq!(
            violations,
            vec![Violation {
                member: "perf_device_report".to_string()
            }]
        );
    }

    /// A member covered under `full:` (not `host:`) passes — coverage is
    /// checked across both buckets, not just `host`.
    #[test]
    fn member_covered_under_full_is_not_flagged() {
        let cargo_toml = r#"members = ["protocol"]"#;
        assert!(check_texts(cargo_toml, CI_YML_FIXTURE).is_empty());
    }

    /// A member covered under `host:` passes.
    #[test]
    fn member_covered_under_host_is_not_flagged() {
        let cargo_toml = r#"members = ["xtask"]"#;
        assert!(check_texts(cargo_toml, CI_YML_FIXTURE).is_empty());
    }

    /// The `host` bucket's fail-safe `'**'` catch-all must NOT, by itself,
    /// satisfy this guard — the guard exists precisely to force an
    /// explicit `<name>/**` entry even though the catch-all would already
    /// run the host lane for an uncategorized member. Silently accepting
    /// catch-all coverage here would defeat the guard's whole purpose
    /// (surfacing a `full`-vs-`host` miscategorization loudly instead of
    /// letting it hide behind the fallback).
    #[test]
    fn catch_all_alone_does_not_satisfy_the_guard() {
        let cargo_toml = r#"members = ["some_future_crate"]"#;
        let violations = check_texts(cargo_toml, CI_YML_FIXTURE);
        assert_eq!(
            violations,
            vec![Violation {
                member: "some_future_crate".to_string()
            }]
        );
    }

    /// A member matched only by the `firmware:` bucket (which is never a
    /// root-workspace member's home — `firmware/` is a detached workspace)
    /// must still be flagged if it also happens to appear as a
    /// `Cargo.toml` member — the `firmware` bucket never counts as
    /// coverage.
    #[test]
    fn firmware_bucket_does_not_count_as_coverage() {
        let cargo_toml = r#"members = ["firmware"]"#;
        let violations = check_texts(cargo_toml, CI_YML_FIXTURE);
        assert_eq!(
            violations,
            vec![Violation {
                member: "firmware".to_string()
            }]
        );
    }

    /// Multi-line, one-per-line `members = [...]` (the real shape root
    /// Cargo.toml uses) parses identically to the single-line form.
    #[test]
    fn multiline_members_array_parses() {
        let cargo_toml = "members = [\n  \"protocol\",\n  \"xtask\",\n]\n";
        assert!(check_texts(cargo_toml, CI_YML_FIXTURE).is_empty());
    }

    /// A `default-members = [...]` array preceding the real `[workspace]
    /// members` one must not be mistaken for it — `members` is a bare
    /// substring of `default-members`, so an unanchored regex would parse
    /// the wrong array (and, worse, could silently parse zero real
    /// members if `default-members` came first and had none of the real
    /// crates in it).
    #[test]
    fn default_members_array_is_not_mistaken_for_workspace_members() {
        let cargo_toml = "default-members = [\"xtask\"]\nmembers = [\"protocol\"]\n";
        assert!(check_texts(cargo_toml, CI_YML_FIXTURE).is_empty());
        // If the regex had matched `default-members` first, "protocol"
        // would never have been parsed as a member and this would also
        // pass vacuously — confirm the real array's content is what
        // actually got parsed by seeding an uncovered member into it.
        let cargo_toml_uncovered =
            "default-members = [\"xtask\"]\nmembers = [\"some_future_crate\"]\n";
        let violations = check_texts(cargo_toml_uncovered, CI_YML_FIXTURE);
        assert_eq!(
            violations,
            vec![Violation {
                member: "some_future_crate".to_string()
            }]
        );
    }

    /// Integration pass case: the live repo's root Cargo.toml and ci.yml,
    /// after this mission's fix, must be clean.
    #[test]
    fn ci_filter_coverage_check_passes_on_live_repo() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "\nci-filter-coverage check found {} violation(s):\n{}\n",
            violations.len(),
            violations
                .iter()
                .map(|v| format!("  - {v}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
