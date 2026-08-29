# SPDX-License-Identifier: GPL-3.0-only
# scripts/banned-vocabulary.sh — single source of truth for the
# internal-ops-vocabulary banned-term pattern.
#
# Sourced (not executed) by:
#   - scripts/check-vocabulary-leak.sh, which greps all tracked file CONTENT
#     tree-wide for these terms. Both ci.yml's "no internal-ops vocabulary
#     leaks in public docs" job and the local pre-PR-publish step
#     (CONTRIBUTING.md "Submitting changes") invoke that ONE script, so the
#     scan's scope can't drift between "what fails CI" and "what fails
#     locally before push" the way it did 2026-08-17/-08-24/-08-29 (three
#     content leaks that landed in already-open PRs before check-commit-
#     format.sh's commit-subject-only local check could catch them).
#   - scripts/check-commit-format.sh, which rejects any commit *subject*
#     containing one of these terms before it can reach `main` — closing the
#     recurrence path a content-only scan alone can't: a banned term in a
#     commit subject lands in a generated file like CHANGELOG.md (sourced
#     from commit subjects by release-please) the moment the commit merges,
#     by which point it's already un-rewritable history.
#
# All call sites source this file so the term list can never drift between
# "what fails a docs/content scan" and "what fails a commit before it can
# merge" — see docs/adr/0004-release-architecture.md and
# 0008-nondestructive-update-artifacts.md for the history of leaks this
# guards against.
#
# This file necessarily spells out every banned term (to document what's
# banned), so every call site excludes it from its own scan the same way it
# already excludes itself.
BANNED_VOCAB_PATTERN='\b(dossier|commander|capcom|flight[- ]director|eecom|flight-manuals)\b'
