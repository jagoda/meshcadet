# Contributing to MeshCadet

Thanks for your interest in MeshCadet. This document covers how to build,
test, and submit changes.

## Before you start

- Read [`docs/adr/0001-charter.md`](docs/adr/0001-charter.md) first. It's the
  project's design contract: protocol interop is a hard requirement (the
  device must remain byte-exact-compatible with MeshCore), and all policy
  behavior lives in a policy/UI layer *on top of* that compliant protocol —
  it must never fork or weaken the wire protocol itself. Changes that would
  break MeshCore interop need to be discussed in an issue first.
- For anything touching the wire protocol (`protocol/`), cross-check against
  the upstream [MeshCore](https://github.com/meshcore-dev/MeshCore) source —
  this project ports it byte-exact, it does not vendor it.

## Development setup

See the [README](README.md#building-from-a-fresh-clone) for full toolchain
setup. Summary:

- Host-native crates (`protocol`, `host`, `xtask`, `ui_sim`, `ui_perf`) build
  and test on stable Rust — no extra tooling needed.
- The `firmware` crate needs the `esp` toolchain (`espup`), `ldproxy`, and
  `espflash`, and a physical T-Deck Plus to flash and test on real hardware.

## Continuous integration

`.github/workflows/ci.yml` runs on every pull request and every push to
`main`: `cargo test --workspace`, `cargo fmt --all -- --check`, and
`cargo clippy --workspace --all-targets -- -D warnings` — all against the
host-native workspace only. It deliberately does **not** build `firmware/`
(the `esp`/Xtensa cross-toolchain and ESP-IDF sysroot are too heavy for a
per-PR job); see the workflow file's own header comment for the full
rationale. `cd firmware && bash check-all-features.sh` (below) remains the
required manual gate for firmware changes.

## Building and testing

```sh
# Host-native workspace: protocol, host, xtask, ui_sim, ui_perf
cargo test --workspace

# Firmware: type-checks + cross-compiles for the device target.
# NOTE: firmware's own #[cfg(test)] blocks are compiled but CANNOT execute on
# host (the target is xtensa-esp32s3-espidf, not this machine's architecture)
# — they only run on real hardware. `cargo build`/`cargo check` here verifies
# the crate compiles; it does not run its tests.
cd firmware && cargo check --target xtensa-esp32s3-espidf

# Firmware: verify every feature combination still compiles (production +
# diagnostics + hil + hil+diagnostics).
cd firmware && bash check-all-features.sh
```

Firmware logic that can be tested on the host is usually a good candidate for
porting into `ui_perf` or `ui_sim` as pure functions (see those crates'
`README.md` for the pattern) rather than trusting an on-device-only test.

### Testing UI changes without hardware

`ui_sim/` is a host-native render rig that exercises the real Slint markup
(`firmware/src/ui/motifs.slint`, screen layouts) through the same software
renderer the firmware uses, without needing a T-Deck Plus. Use it to prove out
image-asset and layout changes before a hardware flash. See
[`ui_sim/README.md`](ui_sim/README.md).

### Testing on real hardware

Some changes (radio timing, display flush cost, touch input, battery/GPS
reads) can only be verified on a real T-Deck Plus. See
[`docs/hil-real-mesh-procedure.md`](docs/hil-real-mesh-procedure.md) for the
manual verification checklist used before landing changes in these areas.

## Code style

- Format with `cargo fmt` and lint with `cargo clippy` before submitting
  (`rust-toolchain.toml` installs both components).
- Match the existing module-level doc-comment style: explain *why*, not just
  *what*, especially for anything non-obvious (hardware quirks, upstream
  protocol discrepancies, workarounds for third-party bugs).
- Every source file carries an `SPDX-License-Identifier: GPL-3.0-only`
  header; new files should too.

## Submitting changes

1. Open an issue first for anything that changes wire-protocol behavior,
   the allowlist policy layer, or the license/dependency set — these need
   discussion before code.
2. Keep changes focused; a PR that mixes an unrelated refactor with a
   behavioral fix is harder to review and to revert if something's wrong.
3. Include the evidence for your change: test output for host-testable code,
   or the relevant excerpt of a hardware verification run
   (`docs/hil-real-mesh-procedure.md`) for anything hardware-only.
4. No real cryptographic material (identity seeds, channel secrets, peer
   public keys) in commits, issues, or PR descriptions — ever. Use obvious
   dummy values in examples and test fixtures.

## Reporting security issues

Do not open a public issue for a security vulnerability. See
[`SECURITY.md`](SECURITY.md) for the responsible-disclosure process.
