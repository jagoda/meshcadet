#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# scripts/sync-cargo-lock-versions.sh — keep Cargo.lock and
# firmware/Cargo.lock's OWN workspace-member `[[package]]` version entries in
# lockstep with the version release-please just wrote to
# Cargo.toml/firmware/Cargo.toml.
#
# Why this exists: release-please-config.json's `extra-files` bumps the
# version string in Cargo.toml ($.workspace.package.version) and
# firmware/Cargo.toml ($.package.version), but release-please has no
# built-in Cargo.lock awareness for `release-type: simple` (that's a
# `rust`/cargo-workspace release-type behavior this repo doesn't use — see
# docs/adr/0004-release-architecture.md §2 for why). Left alone, the release
# PR's Cargo.lock keeps pinning the OLD version for every workspace-member
# crate, and `cargo test --locked` / `cargo clippy --locked` (ci.yml) then
# hard-fail with "cannot update the lock file because --locked was passed" —
# `--locked` refuses to silently regenerate a stale lockfile.
#
# Why this doesn't invoke `cargo` at all (matching
# .github/workflows/release-please.yml's existing "never invokes cargo"
# property): a real lockfile-regenerating cargo command would introduce two
# problems this text-only substitution avoids —
#   1. firmware/Cargo.lock's workspace pins the `esp` (Xtensa) toolchain
#      (firmware/rust-toolchain.toml), which isn't installed on this
#      workflow's runner and is heavyweight to install just to refresh a
#      lockfile.
#   2. Any lockfile-regenerating cargo command (`cargo generate-lockfile`,
#      `cargo update`) re-resolves against the live registry and can pull in
#      unrelated newer-compatible dependency versions alongside the version
#      bump. Verified empirically during this fix's design: `cargo check
#      --workspace` from a stale Cargo.lock touches ONLY the local package
#      version fields, but `cargo generate-lockfile` rewrites dozens of
#      unrelated entries in the same run.
# Workspace-member crates are path/local `[[package]]` entries in Cargo.lock
# — they never carry a `source =`/`checksum =` line (those only appear on
# registry dependencies) — so rewriting just their `version = "..."` line is
# exactly what `--locked` needs and nothing else in the lockfile changes.
#
# Usage: scripts/sync-cargo-lock-versions.sh
# Run with the repo root as the current working directory (same convention
# as ci.yml's version-drift-guard job's `grep -m1 '^version = ' Cargo.toml`).
# Exits 0 whether or not anything needed changing (idempotent — safe to run
# on every release-please push, including ones where nothing changed). Exits
# non-zero and explains itself if a target `[[package]]` entry can't be
# found or looks like a registry package instead of a local one (fail loud
# rather than mis-patch).
set -euo pipefail

if [[ ! -f Cargo.toml || ! -f Cargo.lock ]]; then
  echo "error: run this from the repo root (Cargo.toml/Cargo.lock not found in $(pwd))" >&2
  exit 1
fi

new_version="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "(.*)"/\1/')"
if [[ -z "${new_version}" ]]; then
  echo "error: could not read [workspace.package].version from Cargo.toml" >&2
  exit 1
fi

# Root workspace member crate names, derived from Cargo.toml's `members`
# array (not hardcoded) so this script stays correct if crates are ever
# added/removed.
members_line="$(grep -m1 '^members = ' Cargo.toml)"
mapfile -t member_paths < <(sed -E 's/^members = \[(.*)\]$/\1/' <<<"${members_line}" | tr ',' '\n' | tr -d ' "')

declare -a root_names=()
for path in "${member_paths[@]}"; do
  [[ -z "${path}" ]] && continue
  root_names+=("$(grep -m1 '^name = ' "${path}/Cargo.toml" | sed -E 's/name = "(.*)"/\1/')")
done

# firmware/'s own crate name — its version is a hardcoded literal bumped
# separately by release-please's extra-files config, but its Cargo.lock
# entry needs the same lockstep treatment.
firmware_name="$(grep -m1 '^name = ' firmware/Cargo.toml | sed -E 's/name = "(.*)"/\1/')"

changed_any=0

# Rewrites the `version = "..."` line of the `[[package]] name = "<name>"`
# stanza in <lockfile> to ${new_version}, in place. Requires exactly one
# matching stanza and that it look like a local package (no `source =`
# line immediately below the version line) — errors out otherwise instead
# of guessing.
sync_one() {
  local lockfile="$1" name="$2"

  local name_lines
  name_lines="$(grep -n "^name = \"${name}\"$" "${lockfile}" || true)"
  local match_count
  match_count="$(grep -c "^name = \"${name}\"$" "${lockfile}" || true)"

  if [[ "${match_count}" -eq 0 ]]; then
    echo "error: ${lockfile}: no [[package]] entry named \"${name}\" found" >&2
    exit 1
  fi
  if [[ "${match_count}" -gt 1 ]]; then
    echo "error: ${lockfile}: ${match_count} [[package]] entries named \"${name}\" — expected exactly 1 (this script only handles unambiguous local crate names)" >&2
    exit 1
  fi

  local name_line version_line
  name_line="$(cut -d: -f1 <<<"${name_lines}")"
  version_line=$((name_line + 1))
  local version_text
  version_text="$(sed -n "${version_line}p" "${lockfile}")"
  if [[ ! "${version_text}" =~ ^version\ =\ \" ]]; then
    echo "error: ${lockfile}:${version_line}: expected a \"version = ...\" line directly after \"${name}\"'s name line, got: ${version_text}" >&2
    exit 1
  fi

  local source_line=$((version_line + 1))
  local source_text
  source_text="$(sed -n "${source_line}p" "${lockfile}")"
  if [[ "${source_text}" == source\ =\ * ]]; then
    echo "error: ${lockfile}:${name_line}: \"${name}\" has a \"source =\" line — that's a registry package, not a local workspace member; refusing to touch it" >&2
    exit 1
  fi

  local target_line="version = \"${new_version}\""
  if [[ "${version_text}" == "${target_line}" ]]; then
    return 0
  fi

  sed -i "${version_line}s/.*/${target_line}/" "${lockfile}"
  echo "  ${lockfile}: ${name} ${version_text#version = } -> \"${new_version}\""
  changed_any=1
}

echo "sync-cargo-lock-versions: target version ${new_version}"

for name in "${root_names[@]}"; do
  sync_one "Cargo.lock" "${name}"
done

# firmware/Cargo.lock: its own package, plus whichever root-workspace crates
# it pulls in via path dependency (only those actually present — not every
# root crate is a firmware dependency).
sync_one "firmware/Cargo.lock" "${firmware_name}"
for name in "${root_names[@]}"; do
  if grep -q "^name = \"${name}\"$" firmware/Cargo.lock; then
    sync_one "firmware/Cargo.lock" "${name}"
  fi
done

if [[ "${changed_any}" -eq 1 ]]; then
  echo "sync-cargo-lock-versions: lockfile(s) updated."
else
  echo "sync-cargo-lock-versions: already in sync, nothing to do."
fi
