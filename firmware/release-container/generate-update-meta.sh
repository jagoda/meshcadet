#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# firmware/release-container/generate-update-meta.sh — computes the
# layout-compatibility gate (docs/adr/0008-nondestructive-update-artifacts.md
# D2) and writes update-meta.json. Split out of build.sh (which invokes this
# as its own step, after producing bootloader.bin/partition-table.bin) so
# this specific logic — comparing THIS build's layout_hash against the
# committed baseline and emitting the update-meta.json contract that decides
# whether an app-only flash is offered as a non-destructive Upgrade — can be
# exercised by generate-update-meta.test.sh against small synthetic
# fixtures, without needing a full ESP-IDF/docker build just to test the one
# part of the release pipeline that gates device safety.
#
# Usage: generate-update-meta.sh <version> <bootloader.bin> <partition-table.bin> <baseline-file> <app-asset-basename> <output-json>
set -euo pipefail

VERSION="${1:?usage: generate-update-meta.sh <version> <bootloader.bin> <partition-table.bin> <baseline-file> <app-asset-basename> <output-json>}"
BOOTLOADER_BIN="${2:?usage: generate-update-meta.sh <version> <bootloader.bin> <partition-table.bin> <baseline-file> <app-asset-basename> <output-json>}"
PARTITION_TABLE_BIN="${3:?usage: generate-update-meta.sh <version> <bootloader.bin> <partition-table.bin> <baseline-file> <app-asset-basename> <output-json>}"
BASELINE_FILE="${4:?usage: generate-update-meta.sh <version> <bootloader.bin> <partition-table.bin> <baseline-file> <app-asset-basename> <output-json>}"
APP_ASSET="${5:?usage: generate-update-meta.sh <version> <bootloader.bin> <partition-table.bin> <baseline-file> <app-asset-basename> <output-json>}"
OUTPUT_JSON="${6:?usage: generate-update-meta.sh <version> <bootloader.bin> <partition-table.bin> <baseline-file> <app-asset-basename> <output-json>}"

for f in "$BOOTLOADER_BIN" "$PARTITION_TABLE_BIN"; do
  if [[ ! -f "$f" ]]; then
    echo "generate-update-meta.sh: missing input $f" >&2
    exit 1
  fi
done

if [[ ! -f "$BASELINE_FILE" ]]; then
  echo "generate-update-meta.sh: missing layout-hash baseline $BASELINE_FILE — the committed baseline (see docs/adr/0008-nondestructive-update-artifacts.md); a missing baseline is a build defect, not a soft-fail case" >&2
  exit 1
fi

# Baseline file format: '#'-comment and blank lines ignored, exactly one
# remaining hex-hash line expected (firmware/layout-baseline.txt's own
# header comment documents this and how the seeded value was derived).
LAYOUT_BASELINE="$(grep -v '^[[:space:]]*#' "$BASELINE_FILE" | grep -v '^[[:space:]]*$' | tr -d '[:space:]')"
if [[ -z "$LAYOUT_BASELINE" ]]; then
  echo "generate-update-meta.sh: $BASELINE_FILE has no non-comment hash line" >&2
  exit 1
fi

# layout_hash = sha256(bootloader.bin || partition-table.bin) — the RAW file
# bytes concatenated, not a slice of a padded merged image (see ADR-0008 D2
# for why the distinction matters).
LAYOUT_HASH="$(cat "$BOOTLOADER_BIN" "$PARTITION_TABLE_BIN" | sha256sum | cut -d' ' -f1)"
if [[ "$LAYOUT_HASH" == "$LAYOUT_BASELINE" ]]; then
  UPGRADE_SAFE=true
else
  UPGRADE_SAFE=false
fi

cat > "$OUTPUT_JSON" <<EOF
{
  "version": "${VERSION}",
  "layout_hash": "${LAYOUT_HASH}",
  "layout_baseline": "${LAYOUT_BASELINE}",
  "upgrade_safe": ${UPGRADE_SAFE},
  "app_asset": "${APP_ASSET}",
  "app_offset": 65536
}
EOF
echo "generate-update-meta.sh: wrote ${OUTPUT_JSON} (layout_hash=${LAYOUT_HASH} upgrade_safe=${UPGRADE_SAFE})"
