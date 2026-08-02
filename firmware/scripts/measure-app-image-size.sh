#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# firmware/scripts/measure-app-image-size.sh
#
# Builds PRODUCTION firmware (release profile, default features) and prints
# the resulting flashable app-image size in BYTES to stdout — and ONLY that;
# every other message this script emits goes to stderr, so a caller can
# capture `app_bytes="$(measure-app-image-size.sh)"` as a bare integer.
#
# This is the measurement half of the partition-budget drift guard
# (xtask/src/partition_budget.rs — `cargo run -p xtask --bin xtask --
# verify-partition-budget`, wired into .github/workflows/ci.yml's `firmware`
# job). firmware/partitions.csv's `factory` partition used to carry a
# hand-written "measured" app-image figure in a comment that had no
# recompute trigger and silently decayed 2.72 MB stale as the tree grew
# undetected across an entire campaign's worth of budget decisions — see the
# checkpoint that caught it
# (meshcadet-emoji-font-upgrade-checkpoint-20260802-140922469). This script
# exists so the figure is RECOMPUTED from an actual build every time it's
# needed, never re-read from prose.
#
# Reuses the same cargo-build + esptool elf2image steps as
# firmware/release-container/build.sh (see that script for the full
# rationale behind each flag) but does NOT merge a full flashable image or
# emit release metadata — it only needs the standalone app image's byte
# count. Deliberately skips MESHCADET_RELEASE_VERSION / SOURCE_DATE_EPOCH /
# --remap-path-prefix: those exist for release-build BYTE reproducibility,
# not for a size measurement with a 5%-drift tolerance, and skipping them
# means this script runs against a plain working-tree checkout (no tag, no
# fixed epoch required) instead of only against a tagged release commit.
#
# Prerequisites: the `esp` rustup toolchain + ESP-IDF sysroot must already be
# bootstrapped (`espup install`; see .github/workflows/ci.yml's `firmware`
# job for the CI equivalent) — same precondition check-all-features.sh
# already has.
#
# Usage (from repo root or firmware/ directory):
#   firmware/scripts/measure-app-image-size.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=== cargo build --release (target pinned by firmware/.cargo/config.toml) ===" >&2
cargo build --release --locked >&2

TARGET_DIR="target/xtensa-esp32s3-espidf/release"
ELF="${TARGET_DIR}/meshcadet-firmware"
if [[ ! -f "$ELF" ]]; then
  echo "measure-app-image-size.sh: expected build output $ELF not found — did cargo build --release succeed?" >&2
  exit 1
fi

# Same discovery as build.sh: esp-idf-sys/embuild's own bootstrapped ESP-IDF
# python env is where esptool lives — no separate install needed.
IDF_PYTHON="$(find .embuild "$HOME/.espressif" -maxdepth 4 -type d -name 'idf*_env' -print -quit 2>/dev/null)/bin/python"
if [[ ! -x "$IDF_PYTHON" ]]; then
  echo "measure-app-image-size.sh: could not locate the ESP-IDF python env (esptool) under firmware/.embuild or ~/.espressif" >&2
  echo "  — did the cargo build above actually invoke esp-idf-sys's ESP-IDF bootstrap?" >&2
  exit 1
fi

# Same flash-timing-param discovery as build.sh: read from THIS build's own
# resolved sdkconfig rather than hardcoded, so a future sdkconfig.defaults
# change can't silently desync the elf2image header from the real device
# config.
SDKCONFIG_RESOLVED="$(find "${TARGET_DIR}/build" -path '*/out/sdkconfig' -print -quit 2>/dev/null)"
if [[ -z "$SDKCONFIG_RESOLVED" ]]; then
  echo "measure-app-image-size.sh: could not locate the resolved sdkconfig under ${TARGET_DIR}/build/esp-idf-sys-*/out/" >&2
  exit 1
fi

flash_mode=""
for m in qio qout dio dout; do
  if grep -qx "CONFIG_ESPTOOLPY_FLASHMODE_${m^^}=y" "$SDKCONFIG_RESOLVED"; then
    flash_mode="$m"
    break
  fi
done
[[ -n "$flash_mode" ]] || { echo "measure-app-image-size.sh: could not determine CONFIG_ESPTOOLPY_FLASHMODE_* from ${SDKCONFIG_RESOLVED}" >&2; exit 1; }

flash_freq=""
for f in 80m 40m 26m 20m; do
  if grep -qx "CONFIG_ESPTOOLPY_FLASHFREQ_${f^^}=y" "$SDKCONFIG_RESOLVED"; then
    flash_freq="$f"
    break
  fi
done
[[ -n "$flash_freq" ]] || { echo "measure-app-image-size.sh: could not determine CONFIG_ESPTOOLPY_FLASHFREQ_* from ${SDKCONFIG_RESOLVED}" >&2; exit 1; }

flash_size=""
for s in 1MB 2MB 4MB 8MB 16MB 32MB; do
  if grep -qx "CONFIG_ESPTOOLPY_FLASHSIZE_${s^^}=y" "$SDKCONFIG_RESOLVED"; then
    flash_size="$s"
    break
  fi
done
[[ -n "$flash_size" ]] || { echo "measure-app-image-size.sh: could not determine CONFIG_ESPTOOLPY_FLASHSIZE_* from ${SDKCONFIG_RESOLVED}" >&2; exit 1; }

echo "    flash_mode=${flash_mode} flash_freq=${flash_freq} flash_size=${flash_size} (from ${SDKCONFIG_RESOLVED})" >&2

APP_BIN="$(mktemp -u /tmp/meshcadet-app-image-XXXXXX.bin)"
trap 'rm -f "$APP_BIN"' EXIT

"$IDF_PYTHON" -m esptool --chip esp32s3 elf2image \
  --flash_mode "$flash_mode" --flash_freq "$flash_freq" --flash_size "$flash_size" \
  -o "$APP_BIN" "$ELF" >&2

# `wc -c`, not `stat -c%s` / `stat -f%z`: portable across the GNU (Linux CI)
# and BSD (macOS dev machine, per README's flashing instructions) stat
# implementations this script may run under.
wc -c < "$APP_BIN" | tr -d '[:space:]'
echo
