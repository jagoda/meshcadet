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
#      can't hit the panic. It writes the bootloader + app image using espflash's OWN bundled
#      default single-app table, which — on every ESP-IDF target — places nvs/phy_init/factory
#      at the SAME fixed offsets (0x9000 / 0xf000 / 0x10000) our custom table also uses, so the
#      app bytes land exactly where our real table's `factory` partition expects them.
#   2. `espflash write-bin <addr> <file>` is a raw address-write with NO partition-table
#      parsing whatsoever, so it can safely overwrite just the partition-table sector
#      (0x8000, one 4 KB sector) with our real, esp-idf-sys-built `partition-table.bin`
#      (which DOES declare `mc_hist`) — landing the correct table without ever exercising
#      espflash's broken decoder.
#   3. Reset (already triggered by write-bin's default `--after hard-reset`) boots the app
#      with the corrected table in place; `espflash monitor` (no `--no-reset`) then does
#      its OWN before/after reset too to load its flash stub and attach — see that step's
#      own comment for why suppressing monitor's reset (as this script used to) hangs.
#
# `partition-table.bin` is esp-idf-sys's stable, always-fresh copy (copied out of the
# per-build-hash OUT_DIR into the crate's target/<triple>/<profile>/ dir on every build —
# see esp-idf-sys build/native/cargo_driver.rs `copy_binaries_to_target_folder`), so it
# always sits right next to the ELF cargo passes us.
set -euo pipefail

ELF="${1:?usage: flash-with-partition-table.sh <path-to-elf>}"
BIN_DIR="$(dirname "$ELF")"
PARTITION_TABLE_BIN="$BIN_DIR/partition-table.bin"

if [[ ! -f "$PARTITION_TABLE_BIN" ]]; then
  echo "flash-with-partition-table.sh: $PARTITION_TABLE_BIN not found (expected next to the" >&2
  echo "ELF — esp-idf-sys's build.rs copies it there on every build; did the build succeed?)" >&2
  exit 1
fi

# --no-skip: the stale-flash fix this runner already carried — always write the
#   freshly-linked app, never checksum-skip.
# --after no-reset: don't let the app boot yet — it would briefly run with espflash's
#   default (wrong, no mc_hist) table still in the partition-table sector.
espflash flash --no-skip --after no-reset "$ELF"

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
