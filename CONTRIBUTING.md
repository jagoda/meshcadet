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
`main`, as four separate jobs: `cargo test --workspace`,
`cargo fmt --all -- --check`, and
`cargo clippy --workspace --all-targets -- -D warnings` against the
host-native workspace, plus a dedicated `firmware` job. `firmware/` is a
DETACHED workspace (its own `[workspace]` table in `firmware/Cargo.toml`),
so none of the root-workspace jobs above ever touch it, and — since it
cross-compiles for `xtensa-esp32s3-espidf` under the Espressif `esp` Rust
fork — it's kept as its own job precisely so a transient Espressif-toolchain
hiccup can never block the fast host lane (see the workflow file's own
header comment for the full rationale). The `firmware` job:

- installs the `esp`/Xtensa cross-toolchain + ESP-IDF sysroot;
- runs `cargo run -p xtask --bin xtask` (no args) — the same host-native,
  no-esp-toolchain-needed static guard battery `cargo test --workspace`
  exercises as `#[test]`s (see "Building and testing" below), run again
  here because the `test`/`fmt`/`clippy` jobs above are skipped by their
  path-filter `if:` on a diff scoped entirely to `firmware/**`, and every
  one of those guards exists specifically to scan `firmware/src/**`;
- runs `cd firmware && bash check-all-features.sh` — the same command
  described below, now run by CI on every PR instead of only by a human
  before landing firmware changes;
- runs `cargo run -p xtask --bin xtask -- verify-partition-budget` (a fresh
  release build's app-image size, diffed against the committed flash-budget
  baseline; see "Flash-budget changes" below).

`firmware/` does **not** get a CI fmt/clippy pass yet — see "Known gaps"
below for why and what closes it.

### Known gaps

- **`firmware/` has no CI-enforced `cargo fmt`/`cargo clippy` pass.** Being a
  detached workspace, it's invisible to the root `fmt`/`clippy` jobs above,
  and adding `cd firmware && cargo fmt --all -- --check` /
  `cd firmware && cargo clippy --all-targets -- -D warnings` steps to the
  `firmware` job was tried alongside the fix in this section's git history
  and reverted: as of that check, `cargo fmt --all -- --check` reports 447
  diffs across 30 files, and `cargo clippy --all-targets -- -D warnings`
  reports 56 findings (mostly `clippy::doc_lazy_continuation`/
  `doc_markdown` on doc-comment formatting, plus real ones — two unused
  imports, two dead functions, a needlessly-boxed local, a
  `payload.get(0)` that should be `.first()`, a `CStr::from_bytes_with_nul`
  that should be a `c""` literal, an empty `loop {}`, a manual
  `Option::map`). Landing either gate unconditionally would turn CI red for
  every future firmware PR regardless of that PR's own content, which is
  worse than the status quo. Until a dedicated cleanup PR lands (fmt is a
  mechanical `cargo fmt --all` in `firmware/`; clippy needs each finding
  triaged, since some are real bugs worth fixing on their own merits, not
  just silenced), format and lint `firmware/` by hand before submitting
  (see "Code style" below) — CI cannot catch a regression there yet.

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

### Flash-budget changes: recompute, never re-read a comment

Any plan or PR that budgets against the firmware app image's flash headroom
(new UI assets, new glyph/font coverage, a dependency upgrade that touches
`firmware/`) **must** run the partition-budget guard before locking a
decision to a size figure:

```sh
cargo run -p xtask --bin xtask -- verify-partition-budget
```

This recomputes the actual app-image size from a fresh release build and
diffs it against the committed baseline
(`firmware/app-image-budget-baseline.txt`), failing loudly past a 5% drift.
It requires the `esp` cross-toolchain (same prerequisites as
`check-all-features.sh`) and runs as its own step in
`.github/workflows/ci.yml`'s `firmware` job on every push — but a
multi-phase campaign that budgets several decisions against the flash
headroom in advance of any single PR should still run it directly at
planning time, not wait for the next push's CI result. `firmware/partitions.csv`'s
`factory` partition comment used to carry this figure as hand-written
"measured" prose with no recompute trigger; it decayed 2.72 MB stale,
undetected, before an entire campaign had budgeted every phase-5/6 decision
against it — see `xtask/src/partition_budget.rs`'s module doc for the full
incident.

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
5. **Required, before `git push` / `gh pr create`:** run
   `scripts/check-commit-format.sh` from the repo root. It validates every
   commit your branch has added since it diverged from `main` against the
   same Conventional Commits rule `.github/workflows/commitlint.yml`'s
   "Lint commit messages" job enforces in CI (both invoke this one script —
   see the script's own header) — so a non-conventional commit (e.g. an
   un-squashed WIP/checkpoint commit) is caught locally instead of showing
   up as a red check after the PR is already open. Also give the PR title
   itself a Conventional Commits prefix (`feat: …`, `fix: …`, etc.) when you
   run `gh pr create` — CI lints that separately (`lint-pr-title`), and on
   this squash-merge-only repo the PR title becomes the commit subject that
   lands on `main`.
6. **Also required, before `git push` / `gh pr create`:** run
   `scripts/check-vocabulary-leak.sh` from the repo root. This repo is
   public; its docs and code comments have occasionally picked up
   internal ops-automation vocabulary (role names, per-task tracking-file
   jargon, etc.) that means nothing to an outside reader — see
   `scripts/banned-vocabulary.sh` for the exact term list. That script
   scans every tracked file's *content* — not just commit subjects — for
   those terms, the same way `.github/workflows/ci.yml`'s "no internal-ops
   vocabulary leaks in public docs" job checks tree-wide in CI (both invoke
   this one script — see its own header), so a leak in a source comment or
   a doc paragraph is caught locally instead of showing up as a red check
   after the PR is already open.

## Reporting security issues

Do not open a public issue for a security vulnerability. See
[`SECURITY.md`](SECURITY.md) for the responsible-disclosure process.
