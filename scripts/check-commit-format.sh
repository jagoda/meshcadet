#!/usr/bin/env bash
# scripts/check-commit-format.sh — Conventional Commits per-commit format
# check.
#
# Single source of truth for MeshCadet's Conventional Commits subject-line
# rule: `.github/workflows/commitlint.yml`'s "Lint commit messages" CI job
# and the local pre-PR-publish step (see CONTRIBUTING.md "Submitting
# changes") both invoke THIS script, so a local run and CI can never
# disagree about what passes. See docs/adr/0004-release-architecture.md §4
# for the design this implements.
#
# Usage:
#   scripts/check-commit-format.sh [<base-rev> [<head-rev>]]
#
# Validates every commit in `<base-rev>..<head-rev>` against the
# Conventional Commits subject grammar, printing a clear message per
# offending commit. Exits 0 iff every commit in range is conventional.
#
#   <base-rev>  defaults to `git merge-base HEAD origin/main` — i.e.
#               everything the current branch has added since it diverged
#               from main. CI passes the PR's actual base/head SHAs instead
#               (github.event.pull_request.base.sha / .head.sha), which is
#               the same range GitHub itself is about to merge.
#   <head-rev>  defaults to HEAD.
#
# Run with no arguments from a mission/feature branch before `git push` /
# `gh pr create` — that is the "pre-PR-publish" step this script exists for.
#
# release-plz exemption (CI-only; leave unset for local/manual use):
# release-plz's own release-PR commit subject can legitimately lag its PR
# title by one release-plz run and is exempted from this check on its own
# branch — see docs/adr/0004-release-architecture.md §4 for the verified
# root cause. Set EXEMPT_RELEASE_PLZ=1 and BRANCH_REF=<branch under test> to
# enable it (commitlint.yml does this). The interactive PR-prep flow this
# script's default/local mode covers never carries release-plz's own
# commits, so local runs leave this exemption off and check every commit
# unconditionally.
set -euo pipefail

# type(scope)!: subject — scope optional, "!" (breaking) optional. Kept
# byte-identical to the regex `.github/workflows/commitlint.yml` used to
# inline directly; the workflow now sources this script instead of
# duplicating the pattern, so local and CI verdicts cannot drift apart.
pattern='^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\([a-zA-Z0-9_./-]+\))?!?: .+'

release_plz_branch_prefix='release-plz-'
release_plz_bot_author='github-actions[bot]'

base="${1:-$(git merge-base HEAD origin/main)}"
head="${2:-HEAD}"

fail=0
checked=0
for sha in $(git rev-list "${base}..${head}"); do
  subject="$(git log -1 --format=%s "${sha}")"
  author="$(git log -1 --format=%an "${sha}")"

  if [[ "${EXEMPT_RELEASE_PLZ:-0}" == "1" \
        && "${BRANCH_REF:-}" == "${release_plz_branch_prefix}"* \
        && "${author}" == "${release_plz_bot_author}" ]]; then
    echo "Skipping release-plz automated commit ${sha}: \"${subject}\""
    continue
  fi

  checked=$((checked + 1))
  if ! [[ "${subject}" =~ ${pattern} ]]; then
    # Under GitHub Actions, `::error::` renders as an annotation on the PR's
    # Checks tab instead of a plain log line — preserve that UX now that CI
    # calls this script instead of inlining its own `::error::` output.
    if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
      echo "::error::Commit ${sha} is not a Conventional Commit: \"${subject}\"" >&2
    else
      echo "error: commit ${sha} is not a Conventional Commit: \"${subject}\"" >&2
    fi
    echo "  expected form: type(scope)!: subject" >&2
    echo "  type one of: feat fix docs style refactor perf test build ci chore revert" >&2
    fail=1
  fi
done

if [[ "${fail}" -eq 0 ]]; then
  echo "check-commit-format: ${checked} commit(s) in ${base}..${head} are all Conventional Commits."
fi

exit "${fail}"
