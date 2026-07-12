#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# scripts/check-commit-format.test.sh — smoke test for
# scripts/check-commit-format.sh.
#
# Exercises the three behaviors that gate matters most for (a passing
# all-conventional range, a clearly-failing non-conventional commit, and the
# release-please exemption being opt-in rather than default-on) against an
# isolated throwaway git repo, so a future edit to the shared script can't
# silently regress either CI or the local pre-PR-publish step it backs. Run
# directly (`scripts/check-commit-format.test.sh`) or via
# `.github/workflows/commitlint.yml`.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
check="${script_dir}/check-commit-format.sh"

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

cd "${tmpdir}"
git init -q -b main
git config user.email "test@example.com"
git config user.name "Test User"

git commit -q --allow-empty -m "chore: seed"
base="$(git rev-parse HEAD)"

# Case 1: an all-conventional range passes with a zero exit.
git commit -q --allow-empty -m "feat(x): add a thing"
git commit -q --allow-empty -m "fix(y): correct a thing"
if ! "${check}" "${base}" HEAD >"${tmpdir}/out.log" 2>&1; then
  echo "FAIL: expected an all-conventional range to pass" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi

# Case 2: a non-conventional commit fails clearly (non-zero exit, names the
# offending commit in its output).
git commit -q --allow-empty -m "oops no type"
if "${check}" "${base}" HEAD >"${tmpdir}/out.log" 2>&1; then
  echo "FAIL: expected a non-conventional commit to fail the check" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi
if ! grep -q "oops no type" "${tmpdir}/out.log"; then
  echo "FAIL: expected failure output to name the offending commit" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi

# Case 3: the release-please exemption is opt-in, not default — the same
# non-conventional-looking release-please commit fails without
# EXEMPT_RELEASE_PLEASE/BRANCH_REF set (the local/manual mode this script
# defaults to) and passes once CI's opt-in env vars are supplied.
git checkout -q -b release-please--branches--main "${base}"
git -c user.name="github-actions[bot]" -c user.email="bot@example.com" \
  commit -q --allow-empty -m "release v9.9.9"
if "${check}" "${base}" HEAD >"${tmpdir}/out.log" 2>&1; then
  echo "FAIL: expected a release-please commit to fail WITHOUT the exemption enabled" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi
if ! EXEMPT_RELEASE_PLEASE=1 BRANCH_REF=release-please--branches--main "${check}" "${base}" HEAD >"${tmpdir}/out.log" 2>&1; then
  echo "FAIL: expected a release-please commit to pass WITH the exemption enabled" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi

echo "check-commit-format.test.sh: all cases passed"
