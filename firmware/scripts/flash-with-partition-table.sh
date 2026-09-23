#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# cargo runner wrapper for the firmware crate's custom (16 MB, `mc_hist`-bearing) partition table.
#
# WHY THIS EXISTS (do not simplify back to a plain `espflash flash --partition-table ...`):
#
# espflash 4.4.0's CSV/binary partition-table parser (esp-idf-part v0.6.0) unconditionally
# assumes every `data`-type partition's SubType is one of its small built-in `DataType` enum
# variants and unwraps a lookup that returns `None` for any custom, ESP-IDF-documented
# "user-defined" SubType (the 0x40-0xFE range) — see
# esp-idf-part-0.6.0/src/partition/mod.rs:157 (`DataType::from_repr(value).unwrap()`, no
# `SubType::Custom` fallback). This means `espflash flash --partition-table <file>` and even
# `espflash partition-table <file>` PANIC on our real `mc_hist` partition (SubType 0x40),
# whether given as CSV or as the correctly pre-built binary. Verified upstream bug, not a
# config mistake on our side — filed against esp-idf-part/espflash, not fixed as of 4.4.0.
#
# WORKAROUND (verified end-to-end on real T-Deck Plus hardware, on-device boot-log
# transcript on file):
#   1. `espflash flash` WITHOUT `--partition-table` never touches our CSV/bin at all, so it
#      can't hit the panic. It writes espflash's OWN bundled default bootloader + our app
#      image using espflash's OWN bundled default single-app table, which — on every
#      ESP-IDF target — places nvs/phy_init/factory at the SAME fixed offsets (0x9000 /
#      0xf000 / 0x10000) our custom table also uses, so the app bytes land exactly where our
#      real table's `factory` partition expects them.
#   2. `espflash write-bin <addr> <file>` is a raw address-write with NO partition-table
#      parsing whatsoever, so it can safely overwrite just the partition-table sector
#      (0x8000, one 4 KB sector) with our real, esp-idf-sys-built `partition-table.bin`
#      (which DOES declare `mc_hist`) — landing the correct table without ever exercising
#      espflash's broken decoder.
#   3. The same `write-bin` mechanism also repairs the bootloader sector (0x0 on ESP32-S3 —
#      NOT 0x1000, that's original ESP32; see firmware/release-container/build.sh, which
#      flashes the same offset for the release image) with esp-idf-sys's own bootloader.bin,
#      the one actually built against this project's pinned ESP_IDF_VERSION
#      (firmware/.cargo/config.toml). Step 1's bootloader is espflash's bundled default —
#      it tracks espflash's OWN release cadence, not this project's ESP-IDF pin, so leaving
#      it in place is a real skew hazard, not a theoretical one: a device flashed by step 1
#      alone was confirmed on hardware to boot an ESP-IDF v5.5.1 bootloader (ten months
#      newer than the app, and produced by no meshcadet build) paired with a v5.2.2 app, and
#      flashing the project's own bootloader by hand materially changed device behaviour —
#      see docs/provisioning-connect-verification-kit.md ("round 10") for the evidence and
#      for why this is a partial fix, not a confirmed resolution of the connect wedge.
#   4. Reset (already triggered by write-bin's default `--after hard-reset`) boots the app
#      with the corrected bootloader + table in place; `espflash monitor` (no `--no-reset`)
#      then does its OWN before/after reset too to load its flash stub and attach — see that
#      step's own comment for why suppressing monitor's reset (as this script used to) hangs.
#
# `partition-table.bin` and `bootloader.bin` are esp-idf-sys's stable, always-fresh copies
# (copied out of the per-build-hash OUT_DIR into the crate's target/<triple>/<profile>/ dir on
# every build — see esp-idf-sys build/native/cargo_driver.rs `copy_binaries_to_target_folder`),
# so they always sit right next to the ELF cargo passes us.
set -euo pipefail

ELF="${1:?usage: flash-with-partition-table.sh <path-to-elf>}"
BIN_DIR="$(dirname "$ELF")"
PARTITION_TABLE_BIN="$BIN_DIR/partition-table.bin"
BOOTLOADER_BIN="$BIN_DIR/bootloader.bin"

# Both artifacts are esp-idf-sys build outputs copied next to the ELF on every
# build (see the header comment above) — a missing one means the build didn't
# succeed (or didn't run), and flashing anyway would silently write a stale or
# absent bootloader/partition table. Fail loud instead.
require_bin() {
  local bin_path="$1"
  if [[ ! -f "$bin_path" ]]; then
    echo "flash-with-partition-table.sh: $bin_path not found (expected next to the" >&2
    echo "ELF — esp-idf-sys's build.rs copies it there on every build; did the build succeed?)" >&2
    exit 1
  fi
}
require_bin "$PARTITION_TABLE_BIN"
require_bin "$BOOTLOADER_BIN"

# --no-skip: the stale-flash fix this runner already carried — always write the
#   freshly-linked app, never checksum-skip.
# --after no-reset: don't let the app boot yet — it would briefly run with espflash's
#   own bundled bootloader and (wrong, no mc_hist) table still on the device.
espflash flash --no-skip --after no-reset "$ELF"

# Overwrite the bootloader sector (0x0) with the project's own, ESP_IDF_VERSION-matched
# bootloader.bin — repairs the skew left by step 1's espflash-bundled bootloader (see the
# header comment above for the confirmed-on-hardware hazard this closes). Default --after
# is hard-reset; left at default for the same reason the partition-table write below is:
# see that step's comment for the "Communication error" failure an explicit
# `--after no-reset` produced on real hardware.
espflash write-bin 0x0 "$BOOTLOADER_BIN"

# Overwrite just the partition-table sector with our real table. Default --after is
# hard-reset, so this is also what boots the device into its corrected state. (An
# `--after no-reset` variant here, to let the monitor stage below own the sole reset,
# was tried and rejected: on real hardware it made write-bin itself fail with
# "Communication error while flashing device" — this raw address-write doesn't tolerate
# skipping its own post-write reset the way `espflash flash` does. Leave it default.)
espflash write-bin 0x8000 "$PARTITION_TABLE_BIN"

# Attach and stream the boot log.
#
# WHY NO --no-reset HERE:
# this used to be `espflash monitor --non-interactive --no-reset`, reasoning that the
# write-bin step above already reset the device so monitor shouldn't reset it again.
# That reasoning was wrong: `--no-reset` suppresses monitor's *before-connect* reset
# too, and monitor needs that reset to drive the chip into the ROM bootloader and load
# its flash stub (the framing it uses for the serial link) — every espflash connection
# goes through this handshake, monitor included, per its own "Using flash stub" log
# line. Without permission to reset, monitor's stub-load handshake has nothing on the
# other end of the wire to answer it reliably, so it deadlocks forever at
# "Connecting..." / "Using flash stub" — confirmed on real T-Deck Plus hardware: this
# was misdiagnosed as a USB-CDC re-enumeration race (it isn't one — polling
# /dev/ttyACM0 at 50ms resolution across a reset never shows it disappear), and a
# manual, flag-less `espflash monitor` a few seconds later "fixing" it was actually
# just that manual invocation using its own default (non-suppressed) reset, not the
# port having settled. Letting monitor do its default before/after reset here restores
# that working handshake; verified on hardware to stream the full boot log (mc_hist +
# history hydrate) without a hang.
espflash monitor --non-interactive
