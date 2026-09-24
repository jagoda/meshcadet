#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# firmware/scripts/flash-with-partition-table.test.sh — smoke test for
# firmware/scripts/flash-with-partition-table.sh.
#
# This container has no Xtensa toolchain and no device, so nothing here
# touches real hardware or a real `espflash`. Instead it stubs `espflash` on
# PATH with a fake that just logs its argv, and exercises the script against
# a throwaway fixture directory standing in for target/<triple>/<profile>/.
# That's enough to pin down the one thing round 10 changed without a device:
# the script's own CALL SEQUENCE and its fail-loud preconditions. It cannot
# and does not assert anything about what espflash actually does on the
# wire — that's a maintainer's device-side job, done on real hardware.
#
# Run directly (`firmware/scripts/flash-with-partition-table.test.sh`).
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
flash_script="${script_dir}/flash-with-partition-table.sh"

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

fixture_dir="${tmpdir}/target/xtensa-esp32s3-espidf/debug"
mkdir -p "${fixture_dir}"
elf="${fixture_dir}/meshcadet-firmware"
: >"${elf}"
: >"${fixture_dir}/partition-table.bin"
: >"${fixture_dir}/bootloader.bin"

fake_bin_dir="${tmpdir}/fake-bin"
mkdir -p "${fake_bin_dir}"
espflash_log="${tmpdir}/espflash.log"
cat >"${fake_bin_dir}/espflash" <<'EOF'
#!/usr/bin/env bash
echo "$@" >>"${ESPFLASH_LOG}"
exit 0
EOF
chmod +x "${fake_bin_dir}/espflash"

run_flash_script() {
  : >"${espflash_log}"
  PATH="${fake_bin_dir}:${PATH}" ESPFLASH_LOG="${espflash_log}" "${flash_script}" "${elf}"
}

# Case 1: happy path — both artifacts present, three espflash invocations in
# the expected order: the ELF flash (with --after no-reset, since the
# bootloader/table repair below still needs to happen before the device
# boots), the bootloader repair at 0x0, the partition-table repair at 0x8000,
# then a non-interactive monitor attach. This is the exact sequence round 10
# added the second line to (the bootloader write did not exist before).
if ! run_flash_script >"${tmpdir}/out.log" 2>&1; then
  echo "FAIL: expected the happy path (both artifacts present) to succeed" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi
mapfile -t calls <"${espflash_log}"
if [[ ${#calls[@]} -ne 4 ]]; then
  echo "FAIL: expected exactly 4 espflash invocations, got ${#calls[@]}:" >&2
  printf '%s\n' "${calls[@]}" >&2
  exit 1
fi
if [[ "${calls[0]}" != "flash --no-skip --after no-reset ${elf}" ]]; then
  echo "FAIL: call 1 should be the ELF flash with --no-skip --after no-reset, got: ${calls[0]}" >&2
  exit 1
fi
if [[ "${calls[1]}" != "write-bin 0x0 ${fixture_dir}/bootloader.bin" ]]; then
  echo "FAIL: call 2 should repair the bootloader sector at 0x0 (project's own bootloader.bin), got: ${calls[1]}" >&2
  exit 1
fi
if [[ "${calls[2]}" != "write-bin 0x8000 ${fixture_dir}/partition-table.bin" ]]; then
  echo "FAIL: call 3 should repair the partition-table sector at 0x8000, got: ${calls[2]}" >&2
  exit 1
fi
if [[ "${calls[3]}" != "monitor --non-interactive" ]]; then
  echo "FAIL: call 4 should attach the monitor, got: ${calls[3]}" >&2
  exit 1
fi
# Neither write-bin call should carry an explicit --after: both scripts'
# comments record that forcing --after no-reset on a write-bin call fails on
# real hardware ("Communication error while flashing device"), so both must
# stay at espflash's own default (hard-reset).
if [[ "${calls[1]}" == *"--after"* || "${calls[2]}" == *"--after"* ]]; then
  echo "FAIL: write-bin calls must not override --after (real-hardware regression, see script header)" >&2
  printf '%s\n' "${calls[@]}" >&2
  exit 1
fi

# Case 2: missing bootloader.bin fails loudly, names the missing file, and
# never invokes espflash at all (no partial/wrong flash).
rm "${fixture_dir}/bootloader.bin"
: >"${fixture_dir}/partition-table.bin"
if run_flash_script >"${tmpdir}/out.log" 2>&1; then
  echo "FAIL: expected a missing bootloader.bin to fail the script" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi
if ! grep -q "bootloader.bin not found" "${tmpdir}/out.log"; then
  echo "FAIL: expected failure output to name the missing bootloader.bin" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi
if [[ -s "${espflash_log}" ]]; then
  echo "FAIL: expected no espflash invocation at all when bootloader.bin is missing" >&2
  cat "${espflash_log}" >&2
  exit 1
fi
: >"${fixture_dir}/bootloader.bin"

# Case 3: missing partition-table.bin still fails loudly too (pre-existing
# behavior — regression guard so round 10's bootloader check doesn't
# accidentally shadow it).
rm "${fixture_dir}/partition-table.bin"
if run_flash_script >"${tmpdir}/out.log" 2>&1; then
  echo "FAIL: expected a missing partition-table.bin to fail the script" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi
if ! grep -q "partition-table.bin not found" "${tmpdir}/out.log"; then
  echo "FAIL: expected failure output to name the missing partition-table.bin" >&2
  cat "${tmpdir}/out.log" >&2
  exit 1
fi

echo "OK: flash-with-partition-table.sh smoke tests passed"
