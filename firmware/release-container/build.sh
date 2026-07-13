#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Runs INSIDE the pinned release-container (see ./Dockerfile) as its
# ENTRYPOINT. Builds PRODUCTION firmware (default features — no diagnostics,
# no hil), merges bootloader + custom partition table + app into one
# flashable image (the Fresh-install artifact), AND keeps the standalone app
# image plus a compatibility-gate metadata file as a second, app-only update
# artifact (see docs/adr/0008-nondestructive-update-artifacts.md — this is
# the CONTRACT the site flasher child consumes; ADR-0004 §7 covers the
# merged/Fresh-install artifact this augments). Same script for CI
# (.github/workflows/release.yml) and a third-party local reproduction — see
# docs/release-reproducibility.md.
#
# Usage: build.sh <version e.g. v0.1.0> <source-date-epoch>
#
# Expects the repo checked out at the release tag and bind-mounted at /build
# (this container's WORKDIR) — see docs/release-reproducibility.md for the
# exact `docker run` invocation.
set -euo pipefail

VERSION="${1:?usage: build.sh <version e.g. v0.1.0> <source-date-epoch>}"
SOURCE_DATE_EPOCH_ARG="${2:?usage: build.sh <version e.g. v0.1.0> <source-date-epoch>}"

# `espup install`'s export snippet (see Dockerfile) — puts rustup's `esp`
# toolchain + its bundled clang on PATH/LIBCLANG_PATH. Must happen before any
# `cargo` invocation against firmware/ (rust-toolchain.toml pins channel "esp").
# shellcheck source=/dev/null
source /opt/export-esp.sh

cd /build/firmware

# ── Determinism levers (docs/release-reproducibility.md §"How this is made
# reproducible") ──────────────────────────────────────────────────────────
#
# 1. MESHCADET_RELEASE_VERSION: the phase-2 seam (build.rs::emit_build_version)
#    — stamps the exact released version into the boot string instead of
#    `git rev-parse --short HEAD`, so the binary carries no build-machine- or
#    build-time-dependent git state.
# 2. SOURCE_DATE_EPOCH: the released tag's own commit date (passed in by the
#    caller — release.yml derives it via `git log -1 --format=%ct`), the
#    conventional signal several Rust/C toolchain components honor in place
#    of "now" for anything that would otherwise embed a build timestamp.
# 3. RUSTFLAGS --remap-path-prefix: rustc embeds the absolute source path of
#    every compiled file in panic messages / debug info (`file!()`). Every
#    build of this image — CI and a local reproduction alike — mounts the
#    checkout at the SAME container-internal path (/build) and uses the SAME
#    fixed CARGO_HOME (/opt/cargo, baked into the image, see Dockerfile), so
#    remapping both to fixed, build-machine-independent targets makes the
#    embedded paths identical regardless of where the reproducer's checkout
#    or cargo registry actually live on their own disk.
# 4. CONFIG_APP_REPRODUCIBLE_BUILD=y (firmware/sdkconfig.defaults): esp-idf's
#    own C-side equivalent of (2)+(3) — strips the esp_app_desc compile
#    date/time stamp and hides absolute paths in the C components' own debug
#    macros.
# 5. This container: pins libfreetype6-dev + fonts-dejavu-core (see
#    Dockerfile header comment) — the one hazard (1)-(4) cannot reach, since
#    it lives in build.rs's own host-side FreeType rasterization, not in
#    anything rustc or esp-idf's Kconfig controls.
export MESHCADET_RELEASE_VERSION="$VERSION"
export SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH_ARG"
export RUSTFLAGS="--remap-path-prefix=/build=. --remap-path-prefix=${CARGO_HOME}=/cargo-home ${RUSTFLAGS:-}"

echo "=== cargo build --release (target pinned by firmware/.cargo/config.toml) ==="
cargo build --release --locked

TARGET_DIR="target/xtensa-esp32s3-espidf/release"
ELF="${TARGET_DIR}/meshcadet-firmware"
BOOTLOADER_BIN="${TARGET_DIR}/bootloader.bin"
PARTITION_TABLE_BIN="${TARGET_DIR}/partition-table.bin"

for f in "$ELF" "$BOOTLOADER_BIN" "$PARTITION_TABLE_BIN"; do
  if [[ ! -f "$f" ]]; then
    echo "build.sh: expected build output $f not found — did cargo build --release succeed?" >&2
    exit 1
  fi
done

# ── Locate esptool (esp-idf-sys/embuild's own ESP-IDF python env) ──────────
#
# esp-idf-sys never links the final app itself (that's Cargo's job — it only
# builds the bootloader/partition-table as their own cmake sub-projects,
# which IS why those two show up as ready-made .bin files above but the app
# doesn't — see esp-idf-sys's build/native/cargo_driver.rs
# copy_binaries_to_target_folder, which copies exactly those two). Turning
# our ELF into a flashable app image is therefore our own job, via the SAME
# esptool ESP-IDF already bootstrapped for the bootloader/partition-table
# build — no separate esptool install/version needed. embuild scopes its
# ESP-IDF tools install under firmware/.embuild for this project (same dir
# ci.yml caches as `firmware/.embuild`); fall back to the global `~/.espressif`
# location (embuild's other, non-project-scoped cache dir, also cached by
# ci.yml) if the project-local one isn't where the venv landed.
IDF_PYTHON="$(find .embuild "$HOME/.espressif" -maxdepth 4 -type d -name 'idf*_env' -print -quit 2>/dev/null)/bin/python"
if [[ ! -x "$IDF_PYTHON" ]]; then
  echo "build.sh: could not locate the ESP-IDF python env (esptool) under firmware/.embuild or ~/.espressif" >&2
  echo "  — did the cargo build above actually invoke esp-idf-sys's ESP-IDF bootstrap?" >&2
  exit 1
fi
echo "=== esptool: ${IDF_PYTHON} -m esptool ==="

# ── Flash timing params: read from THIS build's own resolved sdkconfig, not
# hardcoded — the ground truth for what the bootloader/app headers must
# agree on (esp-idf-sys writes the fully-resolved config back out at
# OUT_DIR/sdkconfig (plain KConfig .config format — OUT_DIR/build/config/
# only holds the CMake-generated sdkconfig.cmake/.h/.json siblings, not this
# file); OUT_DIR itself is a per-build content hash, hence the find). Fail
# loudly rather than guess if a future sdkconfig change picks an option this
# script doesn't recognize.
SDKCONFIG_RESOLVED="$(find "${TARGET_DIR}/build" -path '*/out/sdkconfig' -print -quit 2>/dev/null)"
if [[ -z "$SDKCONFIG_RESOLVED" ]]; then
  echo "build.sh: could not locate the resolved sdkconfig under ${TARGET_DIR}/build/esp-idf-sys-*/out/" >&2
  exit 1
fi

flash_mode=""
for m in qio qout dio dout; do
  if grep -qx "CONFIG_ESPTOOLPY_FLASHMODE_${m^^}=y" "$SDKCONFIG_RESOLVED"; then
    flash_mode="$m"
    break
  fi
done
[[ -n "$flash_mode" ]] || { echo "build.sh: could not determine CONFIG_ESPTOOLPY_FLASHMODE_* from ${SDKCONFIG_RESOLVED}" >&2; exit 1; }

flash_freq=""
for f in 80m 40m 26m 20m; do
  if grep -qx "CONFIG_ESPTOOLPY_FLASHFREQ_${f^^}=y" "$SDKCONFIG_RESOLVED"; then
    flash_freq="$f"
    break
  fi
done
[[ -n "$flash_freq" ]] || { echo "build.sh: could not determine CONFIG_ESPTOOLPY_FLASHFREQ_* from ${SDKCONFIG_RESOLVED}" >&2; exit 1; }

flash_size=""
for s in 1MB 2MB 4MB 8MB 16MB 32MB; do
  if grep -qx "CONFIG_ESPTOOLPY_FLASHSIZE_${s^^}=y" "$SDKCONFIG_RESOLVED"; then
    flash_size="$s"
    break
  fi
done
[[ -n "$flash_size" ]] || { echo "build.sh: could not determine CONFIG_ESPTOOLPY_FLASHSIZE_* from ${SDKCONFIG_RESOLVED}" >&2; exit 1; }

echo "    flash_mode=${flash_mode} flash_freq=${flash_freq} flash_size=${flash_size} (from ${SDKCONFIG_RESOLVED})"

mkdir -p /build/dist
# Tag-named, same convention as MERGED_BIN below — this is the app-only
# update-artifact asset (ADR-0008): the bare `factory` app image, meant to be
# flashed ALONE at 0x10000 to upgrade a device without touching `nvs`@0x9000
# or `mc_hist`@0x610000. Unlike the old behavior, this file is a KEPT build
# output, not a scratch intermediate — do not `rm` it after merge_bin below.
APP_BIN="/build/dist/meshcadet-${VERSION}-app.bin"
MERGED_BIN="/build/dist/meshcadet-${VERSION}-merged.bin"

"$IDF_PYTHON" -m esptool --chip esp32s3 elf2image \
  --flash_mode "$flash_mode" --flash_freq "$flash_freq" --flash_size "$flash_size" \
  -o "$APP_BIN" "$ELF"

# Offsets are firmware/partitions.csv's fixed, documented layout (see that
# file + firmware/scripts/flash-with-partition-table.sh, which flashes the
# same three components separately for local `cargo run` dev flashing): the
# bootloader at 0x0 (ESP32-S3's bootloader offset — NOT 0x1000, that's
# original ESP32), our custom partition-table.bin (carrying `mc_hist`) at
# 0x8000, and the `factory` app partition at 0x10000.
"$IDF_PYTHON" -m esptool --chip esp32s3 merge_bin \
  --flash_mode "$flash_mode" --flash_freq "$flash_freq" --flash_size "$flash_size" \
  -o "$MERGED_BIN" \
  0x0     "$BOOTLOADER_BIN" \
  0x8000  "$PARTITION_TABLE_BIN" \
  0x10000 "$APP_BIN"

echo "=== wrote ${MERGED_BIN} (Fresh-install artifact — unchanged) ==="
echo "=== wrote ${APP_BIN} (kept — app-only update artifact, see ADR-0008) ==="

# ── Compatibility-gate metadata (ADR-0008, plan D2 revised by R1) ──────────
#
# An app-only flash at 0x10000 is only safe over a device whose installed
# bootloader + partition table are byte-identical to THIS release's — a
# resized/moved partition means the app would either not fit or misalign
# against the device's actual layout. `layout_hash` is that fingerprint;
# `upgrade_safe` records whether it matches the COMMITTED baseline
# (firmware/layout-baseline.txt — the source of truth, seeded from the
# shipped v0.1.0/v0.2.0 layout; NOT the previous release's own metadata, since
# no release before this one ever emitted a layout_hash to compare against).
# This mission (ADR-0008) owns bumping that baseline whenever it
# intentionally changes firmware/partitions.csv or the bootloader layout.
LAYOUT_BASELINE_FILE="layout-baseline.txt"
if [[ ! -f "$LAYOUT_BASELINE_FILE" ]]; then
  echo "build.sh: missing firmware/${LAYOUT_BASELINE_FILE} — the committed layout-hash baseline (see docs/adr/0008-nondestructive-update-artifacts.md)" >&2
  exit 1
fi
LAYOUT_BASELINE="$(grep -v '^[[:space:]]*#' "$LAYOUT_BASELINE_FILE" | grep -v '^[[:space:]]*$' | tr -d '[:space:]')"
if [[ -z "$LAYOUT_BASELINE" ]]; then
  echo "build.sh: firmware/${LAYOUT_BASELINE_FILE} has no non-comment hash line" >&2
  exit 1
fi
LAYOUT_HASH="$(cat "$BOOTLOADER_BIN" "$PARTITION_TABLE_BIN" | sha256sum | cut -d' ' -f1)"
if [[ "$LAYOUT_HASH" == "$LAYOUT_BASELINE" ]]; then
  UPGRADE_SAFE=true
else
  UPGRADE_SAFE=false
fi

cat > /build/dist/update-meta.json <<EOF
{
  "version": "${VERSION}",
  "layout_hash": "${LAYOUT_HASH}",
  "layout_baseline": "${LAYOUT_BASELINE}",
  "upgrade_safe": ${UPGRADE_SAFE},
  "app_asset": "$(basename "$APP_BIN")",
  "app_offset": 65536
}
EOF
echo "=== wrote /build/dist/update-meta.json (layout_hash=${LAYOUT_HASH} upgrade_safe=${UPGRADE_SAFE}) ==="
