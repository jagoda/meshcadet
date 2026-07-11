#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# firmware/check-all-features.sh
#
# Cross-compile the firmware for all feature configurations to verify that every
# feature combination compiles cleanly for xtensa-esp32s3-espidf.
#
# Prerequisites: run `espup install` and `. ~/export-esp.sh` before executing.
#
# Usage (from repo root or firmware/ directory):
#   cd firmware && bash check-all-features.sh
#
# This script is the firmware build gate: a non-compiling production firmware
# previously passed CI because the workspace `cargo test` (which runs
# on the host toolchain for `protocol` + `host` crates only) never exercises
# the cross-compiled firmware crate. This script closes that gap.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

TARGET=xtensa-esp32s3-espidf

echo "=== firmware cross-compile check: target=$TARGET ==="
echo ""

# Default (production) — no features active.
echo "--- [1/4] default (production): cargo build --target $TARGET"
cargo build --target "$TARGET"
echo "    PASS"
echo ""

# diagnostics — on-device instrumentation (byte counters, hex dumps, touch/GPS
# debug overlays); compiled out of production, composes with any build role.
echo "--- [2/4] diagnostics: cargo build --target $TARGET --features diagnostics"
cargo build --target "$TARGET" --features diagnostics
echo "    PASS"
echo ""

# hil — fixed HIL keys from local file; requires src/hil_keys.rs to exist.
# If hil_keys.rs is absent, this step is skipped with a warning rather than
# failing the gate (the file is gitignored and only present on a real HIL rig).
if [ -f "src/hil_keys.rs" ]; then
    echo "--- [3/4] hil: cargo build --target $TARGET --features hil"
    cargo build --target "$TARGET" --features hil
    echo "    PASS"
    echo ""
    echo "--- [4/4] hil+diagnostics: cargo build --target $TARGET --features hil,diagnostics"
    cargo build --target "$TARGET" --features hil,diagnostics
    echo "    PASS"
else
    echo "--- [3/4] hil: SKIPPED (src/hil_keys.rs not present — copy from hil_keys.example.rs)"
    echo "--- [4/4] hil+diagnostics: SKIPPED (same reason)"
fi

echo ""
echo "=== firmware cross-compile check: ALL PASS ==="
