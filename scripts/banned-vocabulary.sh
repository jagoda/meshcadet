# SPDX-License-Identifier: GPL-3.0-only
# scripts/banned-vocabulary.sh — single source of truth for the
# internal-ops-vocabulary banned-term pattern.
#
# Sourced (not executed) by:
#   - .github/workflows/ci.yml's "no internal-ops vocabulary leaks in public
#     docs" job, which greps the whole tree for these terms.
#   - scripts/check-commit-format.sh, which rejects any commit *subject*
#     containing one of these terms before it can reach `main` — closing the
#     recurrence path the ci.yml job alone can't: that job only catches a
#     leak once it's already landed in a generated file like CHANGELOG.md
#     (sourced from commit subjects by release-please), by which point the
#     offending commit is already merged and un-rewritable.
#
# Both call sites `source` this file so the term list can never drift
# between "what fails a merged PR's docs scan" and "what fails a commit
# before it can merge" — see docs/adr/0004-release-architecture.md and
# 0008-nondestructive-update-artifacts.md for the history of leaks this
# guards against.
#
# This file necessarily spells out every banned term (to document what's
# banned), so both call sites exclude it from their own scans the same way
# they already exclude themselves.
BANNED_VOCAB_PATTERN='\b(dossier|commander|capcom|flight[- ]director|eecom|flight-manuals)\b'
