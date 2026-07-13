#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# firmware/release-container/generate-update-meta.test.sh — smoke test for
# generate-update-meta.sh (docs/adr/0008-nondestructive-update-artifacts.md
# D2's compatibility gate). Exercises the happy path (matching baseline ->
# upgrade_safe:true), the layout-changed path (mismatched baseline ->
# upgrade_safe:false), and the three fail-loud guards (missing bootloader/
# partition-table input, missing baseline file, and a baseline file with no
# non-comment hash line) — all against small synthetic fixture bytes in a
# throwaway tmpdir, never against the real firmware/layout-baseline.txt or
# real build outputs.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
generate="${script_dir}/generate-update-meta.sh"

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

fail() {
  echo "FAIL: $1" >&2
  exit 1
}

json_field() {
  # Minimal dependency-free JSON scalar-field extractor — good enough for
  # this fixed, known-shape schema (no nested objects/arrays to worry about).
  local file="$1" field="$2"
  grep "\"${field}\"" "$file" | sed -E "s/.*\"${field}\"[[:space:]]*:[[:space:]]*\"?([^\",}]*)\"?,?/\1/"
}

# ── Fixture bytes: deliberately small, arbitrary content — this script only
# cares about byte-for-byte concatenation + sha256, not real ESP32 image
# structure. ────────────────────────────────────────────────────────────────
printf 'fake-bootloader-bytes' > "${tmpdir}/bootloader.bin"
printf 'fake-partition-table-bytes' > "${tmpdir}/partition-table.bin"
expected_hash="$(cat "${tmpdir}/bootloader.bin" "${tmpdir}/partition-table.bin" | sha256sum | cut -d' ' -f1)"

# ── 1. Happy path: baseline matches -> upgrade_safe:true ───────────────────
cat > "${tmpdir}/baseline-match.txt" <<EOF
# a comment line, and a blank line below, both must be ignored

${expected_hash}
EOF
out="${tmpdir}/out-match.json"
"$generate" v9.9.9 "${tmpdir}/bootloader.bin" "${tmpdir}/partition-table.bin" \
  "${tmpdir}/baseline-match.txt" meshcadet-v9.9.9-app.bin "$out"
[[ "$(json_field "$out" upgrade_safe)" == "true" ]] || fail "expected upgrade_safe:true on matching baseline"
[[ "$(json_field "$out" layout_hash)" == "$expected_hash" ]] || fail "layout_hash mismatch in output"
[[ "$(json_field "$out" version)" == "v9.9.9" ]] || fail "version not passed through"
[[ "$(json_field "$out" app_asset)" == "meshcadet-v9.9.9-app.bin" ]] || fail "app_asset not passed through"
echo "PASS: matching baseline -> upgrade_safe:true"

# ── 2. Layout changed: baseline does NOT match -> upgrade_safe:false ───────
echo "0000000000000000000000000000000000000000000000000000000000000000" > "${tmpdir}/baseline-mismatch.txt"
out="${tmpdir}/out-mismatch.json"
"$generate" v9.9.9 "${tmpdir}/bootloader.bin" "${tmpdir}/partition-table.bin" \
  "${tmpdir}/baseline-mismatch.txt" meshcadet-v9.9.9-app.bin "$out"
[[ "$(json_field "$out" upgrade_safe)" == "false" ]] || fail "expected upgrade_safe:false on mismatched baseline"
echo "PASS: mismatched baseline -> upgrade_safe:false"

# ── 3. Fail-loud guards ─────────────────────────────────────────────────────
if "$generate" v1 "${tmpdir}/does-not-exist.bin" "${tmpdir}/partition-table.bin" \
  "${tmpdir}/baseline-match.txt" app.bin "${tmpdir}/out-x.json" 2>/dev/null; then
  fail "expected failure on missing bootloader.bin input"
fi
echo "PASS: missing bootloader input fails loudly"

if "$generate" v1 "${tmpdir}/bootloader.bin" "${tmpdir}/partition-table.bin" \
  "${tmpdir}/does-not-exist-baseline.txt" app.bin "${tmpdir}/out-x.json" 2>/dev/null; then
  fail "expected failure on missing baseline file"
fi
echo "PASS: missing baseline file fails loudly"

printf '# only a comment, no hash line\n' > "${tmpdir}/baseline-empty.txt"
if "$generate" v1 "${tmpdir}/bootloader.bin" "${tmpdir}/partition-table.bin" \
  "${tmpdir}/baseline-empty.txt" app.bin "${tmpdir}/out-x.json" 2>/dev/null; then
  fail "expected failure on a baseline file with no non-comment hash line"
fi
echo "PASS: comment-only baseline file fails loudly"

echo "generate-update-meta.test.sh: all checks passed"
