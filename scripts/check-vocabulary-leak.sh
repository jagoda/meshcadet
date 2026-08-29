#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# scripts/check-vocabulary-leak.sh — tree-wide internal-ops-vocabulary leak
# scan over tracked file CONTENT, run identically by CI and by the local
# pre-PR-publish step.
#
# Why this exists: ci.yml's "no internal-ops vocabulary leaks in public
# docs" job greps every tracked file's content, tree-wide, for
# scripts/banned-vocabulary.sh's terms — but until this script existed, the
# only WRITE-TIME (pre-merge) companion check was
# scripts/check-commit-format.sh, which only ever looks at commit
# *subjects*. That gap let five leaks land in already-open PRs before CI
# caught them post-hoc: two ADR paragraphs (2026-07-29), a commit subject
# baked into CHANGELOG.md (2026-08-04, closed by check-commit-format.sh),
# two source comments (2026-08-17), an ADR-body paragraph (2026-08-24), and
# two more source comments (2026-08-29, PR #190). Each of the last three
# went through the exact same scope gap: file content, not commit subjects.
#
# This script closes that gap by being the single source of truth for the
# scan itself (not just the term pattern) — both ci.yml and the local
# pre-PR-publish step call THIS script, so their scope can never drift
# apart the way a hand-duplicated exclude list could.
#
# Usage: scripts/check-vocabulary-leak.sh
#   Scans every git-tracked file's content (via `git grep`, so untracked
#   build output and .gitignore'd paths are never scanned) for
#   scripts/banned-vocabulary.sh's BANNED_VOCAB_PATTERN. Exits 0 if clean,
#   1 (and prints every hit) otherwise.
#
# Run from a mission/feature branch before `git push` / `gh pr create` —
# CONTRIBUTING.md's "Submitting changes" step 5 runs this alongside
# scripts/check-commit-format.sh. CI runs this identical script in
# .github/workflows/ci.yml's "no internal-ops vocabulary leaks in public
# docs" job, so a local pass and a CI pass can never disagree.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/banned-vocabulary.sh
source "${script_dir}/banned-vocabulary.sh"

# Scan the repo that the CALLER is standing in (not necessarily this
# script's own checkout) — `git rev-parse --show-toplevel` resolves off the
# current working directory's repo, so a test harness that invokes this
# script by absolute path from inside a throwaway repo scans THAT repo, not
# the one this script happens to live in.
repo_root="$(git rev-parse --show-toplevel)"
cd "${repo_root}"

# Exclude banned-vocabulary.sh and ci.yml's own workflow file: both
# necessarily spell out every banned term (to document what's banned / why
# the job exists), which would otherwise make this scan flag itself on
# every run — the same exclusion ci.yml's inline grep step already made.
if hits=$(git grep -nIiE "${BANNED_VOCAB_PATTERN}" -- \
    ':(exclude)scripts/banned-vocabulary.sh' \
    ':(exclude).github/workflows/ci.yml'); then
  echo "error: internal ops-automation vocabulary found in tracked file content (see below)." >&2
  echo "Rewrite these to plain language a repo outsider can follow — see" >&2
  echo "docs/adr/0004-release-architecture.md and 0008-nondestructive-update-artifacts.md" >&2
  echo "history for prior fixes of this exact class." >&2
  echo "${hits}"
  exit 1
fi

echo "check-vocabulary-leak: no internal-ops vocabulary leaks found in tracked file content."
