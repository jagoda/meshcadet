#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# scripts/check-vocabulary-leak.test.sh — smoke test for
# scripts/check-vocabulary-leak.sh.
#
# Regression guard for the "local guard narrower than CI guard" defect
# class (see this repo's five internal-ops-vocabulary leaks, 2026-07-29
# through 2026-08-29): scripts/check-commit-format.sh only ever scanned
# commit *subjects*, so a banned term landing in a source comment or an ADR
# paragraph — file *content*, never a commit subject — was caught nowhere
# until CI ran against an already-open PR. This test proves
# check-vocabulary-leak.sh actually catches a content-only leak (case 2)
# and stays quiet on a clean tree (case 1), so a future edit to the shared
# script can't silently regress either CI or the local pre-PR-publish step
# it backs. Run directly (`scripts/check-vocabulary-leak.test.sh`) or via
# `.github/workflows/ci.yml`.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
check="${script_dir}/check-vocabulary-leak.sh"

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

cd "${tmpdir}"
git init -q -b main
git config user.email "test@example.com"
git config user.name "Test User"

# check-vocabulary-leak.sh resolves banned-vocabulary.sh relative to its own
# script_dir, so it finds the real term list regardless of where this test
# repo lives — no need to copy it into the throwaway repo.

# Case 1: a clean tree (no banned terms anywhere in tracked content) passes
# with a zero exit.
cat >clean.txt <<'EOF'
Nothing to see here — plain language a repo outsider can follow.
EOF
git add clean.txt
git commit -q -m "chore: seed"
if ! "${check}" >"${tmpdir}/out.log" 2>&1; then
  echo "FAIL: expected a clean tree to pass" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi

# Case 2: a banned term landing in tracked file CONTENT — not a commit
# subject — fails clearly (non-zero exit, names the offending file:line).
# This is exactly the scope check-commit-format.sh's commit-subject-only
# scan can never cover (source comments, ADR paragraphs, CHANGELOG bodies).
# Built from two halves at runtime, never written contiguously in this
# file's source: a literal banned term here would trip ci.yml's own "no
# internal-ops vocabulary leaks in public docs" job when it scans this test
# file.
banned_term="flight-manu"
banned_term+="als"
printf '// see %s/checklists/example.md for context\n' "${banned_term}" >leaky.rs
git add leaky.rs
git commit -q -m "fix(x): add a comment"
if "${check}" >"${tmpdir}/out.log" 2>&1; then
  echo "FAIL: expected a file-content banned-vocabulary leak to fail the check" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi
if ! grep -q "leaky.rs" "${tmpdir}/out.log"; then
  echo "FAIL: expected failure output to name the offending file" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi

# Case 3: a banned term staged but not yet committed is still caught — the
# whole point is to fire before `git push`, on working-tree content, not
# just on already-committed history.
git rm -q leaky.rs
git commit -q -m "chore: drop leaky file"
printf '// %s reference, uncommitted\n' "${banned_term}" >staged-leak.rs
git add staged-leak.rs
if "${check}" >"${tmpdir}/out.log" 2>&1; then
  echo "FAIL: expected a staged-but-uncommitted leak to fail the check" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi
git rm -q --cached staged-leak.rs
rm -f staged-leak.rs

# Case 4: the two hardcoded self-exclusions (banned-vocabulary.sh's own path
# and ci.yml's own path) actually suppress a hit at those exact paths — not
# just "no leak found" on a repo that happens not to touch them. Without
# this, the real repo's banned-vocabulary.sh (which necessarily spells out
# every term) and ci.yml (whose job description names them too) would fail
# every run.
mkdir -p scripts .github/workflows
printf '# %s\n' "${banned_term}" >scripts/banned-vocabulary.sh
printf '# %s\n' "${banned_term}" >.github/workflows/ci.yml
git add scripts/banned-vocabulary.sh .github/workflows/ci.yml
git commit -q -m "chore: add self-excluded paths with banned content"
if ! "${check}" >"${tmpdir}/out.log" 2>&1; then
  echo "FAIL: expected banned-vocabulary.sh/ci.yml's own paths to stay excluded from the scan" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi

echo "check-vocabulary-leak.test.sh: all cases passed"
