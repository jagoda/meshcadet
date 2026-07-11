# ADR-0004 — Release Architecture

- **Status:** Accepted (2026-07-11)
- **Deciders:** Maintainer design review
- **Supersedes:** —
- **Implements:** —
- **Code:** `release-plz.toml`, `cliff.toml`, `.github/workflows/release-plz.yml`,
  `.github/workflows/commitlint.yml`, `.github/workflows/ci.yml`
  (`version-drift-guard` job), `Cargo.toml` (`[workspace.package].version`),
  `firmware/Cargo.toml` (`version`), `firmware/build.rs`
  (`emit_build_version` / `MESHCADET_RELEASE_VERSION`),
  `.github/workflows/release.yml`, `firmware/release-container/` (Dockerfile
  + `build.sh`), `firmware/sdkconfig.defaults`
  (`CONFIG_APP_REPRODUCIBLE_BUILD`), `docs/release-reproducibility.md`.
  GitHub ruleset `18807600` (`main` branch protection).

## Context

MeshCadet had no tags, no releases, and every crate pinned at `0.0.0`. This
ADR records the design for the release machinery end to end — version/tag
scheme, changelog automation, the commit-message discipline that feeds it,
how a signed commit lands on `main` through a squash-merge, the tag-fired
production firmware build, and reproducibility/provenance. The machinery
described here is built incrementally; this ADR is written up front (during
the first implementation piece) so later pieces have one place recording the
whole shape and don't have to re-derive it. Each section below is marked
with its implementation status.

Constraints:
- `firmware/` is a **detached** Cargo workspace (its own `[workspace]` table
  in `firmware/Cargo.toml`) — see `Cargo.toml`'s own doc comment for why
  (keeps `cargo test`/`fmt`/`clippy` at the repo root fast and
  toolchain-independent, with no `espup`/ESP-IDF install required). Any
  tool that operates on "the Cargo workspace" (release-plz included) only
  sees the root workspace's five crates (`protocol`, `host`, `xtask`,
  `ui_sim`, `ui_perf`); `firmware/` is invisible to it.
- All crates are `publish = false` (Cargo.toml `[workspace.package]`) —
  MeshCadet is an end-user firmware project, not a crates.io library
  ecosystem.
- Merges to `main` happen exclusively via signed, PR-only squash-merge
  (ruleset `18807600`) — no direct pushes, no fast-forwards, no deletions.

## Decision

### 1. Single project-wide version (Implemented)

One version number for the whole project, not one per crate. The root
workspace declares it once — `Cargo.toml`'s `[workspace.package].version`
— and `protocol`, `host`, `xtask`, `ui_sim`, and `ui_perf` all inherit it via
`version.workspace = true`. release-plz manages this one field; bumping it
bumps every root-workspace crate identically, and it opens exactly one PR
for the whole project rather than five independent per-crate PRs.

`firmware/Cargo.toml` cannot use `version.workspace = true` — it is its own
detached workspace root (see Context) with nothing to inherit from. Its
`version` field is a hardcoded literal, kept in lockstep by hand: any PR
that bumps the root version must bump `firmware/Cargo.toml`'s version
identically. **CI enforces this** — `.github/workflows/ci.yml`'s
`version-drift-guard` job fails any PR where the two values disagree. This
is the practical answer to "release-plz cannot cross the workspace
boundary": rather than teach release-plz about a workspace it structurally
cannot see, a small, fast, dependency-free CI check makes drift impossible
to land unnoticed.

All five root-workspace crates were bumped `0.0.0` → `0.1.0` in the same
change that wired this up, and `firmware/Cargo.toml` was bumped to match.

### 2. release-plz: version bump + changelog PR (Implemented)

`release-plz.toml` configures [release-plz](https://release-plz.dev) to:
- Open (and keep up to date) a PR titled `release v{{ version }}` whenever
  Conventional Commits land on `main` that warrant a release
  (`release_always = false` — no PR opens for a no-op push).
- Regenerate `CHANGELOG.md` via `cliff.toml` (git-cliff, Keep a Changelog
  shape — see §3).
- **Never publish to crates.io** (`publish = false`, on top of every
  crate's own `publish = false`) and **never open its own GitHub Release**
  object (`git_release_enable = false`) — the tag it creates
  (`git_tag_enable`, default on) is consumed by the production firmware
  release workflow (§5, not yet implemented), which publishes the
  user-facing Release itself. Two separate "release" objects on the same
  tag — an empty crate release and the real firmware release — would be
  confusing; there is exactly one.
- Skip semver-checking (`semver_check = false`): with every crate
  unpublished, there is no crates.io baseline to diff against.

`.github/workflows/release-plz.yml` runs two jobs on every push to `main`:
`release-plz-pr` (open/update the version+changelog PR) and
`release-plz-release` (tag `main` `vX.Y.Z` once such a PR has been merged).
Both are no-ops when there's nothing new to do, so running them
unconditionally on every push to `main` is safe.

### 3. Changelog: git-cliff from Conventional Commits (Implemented)

`cliff.toml` groups commits by Conventional Commit type into Keep a
Changelog sections (`feat` → Added, `fix` → Fixed, `perf` → Performance,
`refactor` → Changed, `revert` → Reverted, `docs` → Documentation).
Internal bookkeeping commit types (`chore`, `ci`, `build`, `style`, `test`,
and release-plz's own `chore(release)` commits) are filtered out of the
user-facing changelog entirely.

`CHANGELOG.md`'s existing hand-written header and its
`## [Unreleased] — Initial public release` section (covering the whole
pre-tag history, written before this project had Conventional Commits
discipline) are preserved as-is: release-plz only ever rewrites the file
below the first `## [...]` heading it finds, never the header above it.
**On the first release-plz PR**, that `[Unreleased]` heading is retitled to
`[0.1.0] - <date>` (standard Keep a Changelog / release-plz behavior for
the first tagged release), and a fresh, empty `[Unreleased]` section is
opened above it for whatever lands next.

### 4. Conventional-commit CI gate (Implemented)

`.github/workflows/commitlint.yml` runs on every PR:
- **`lint-pr-title`** — the PR title must itself be a Conventional Commit
  (`amannn/action-semantic-pull-request`). This is the check that matters
  most in practice: this repo squash-merges exclusively, and GitHub's
  squash-merge commit subject defaults to the PR title, so the PR title
  *is* what git-cliff parses into `CHANGELOG.md`.
- **`lint-commit-messages`** — every individual commit in the PR must also
  be a Conventional Commit (small dependency-free bash regex check against
  `git rev-list base..head`). This catches non-conventional commits before
  a title edit can paper over them, and remains correct if the merge method
  is ever changed away from squash.

`ci.yml`'s existing three jobs (`test`, `fmt`, `clippy`) are untouched and
stay required; `commitlint.yml` and the new `version-drift-guard` job are
additive gates, not replacements.

### 5. Squash-merge satisfies `required_signatures` (Implemented — ruleset re-verified/re-added)

This repo requires every commit on `main` to carry `required_signatures`,
but also merges exclusively via **squash**-merge through the GitHub UI, and
release-plz's own bot commits on its PR branch are **not** signed. Those two
facts are compatible for a specific reason, worth spelling out because it
is not obvious from either policy alone:

> When a PR is squash-merged through the GitHub web UI (or `gh pr merge
> --squash`), GitHub synthesizes a **new, single commit** on the target
> branch. That commit is authored and signed by GitHub's own web-flow
> identity (`GitHub <noreply@github.com>`, GitHub's own GPG/SSH signing
> key) — it is not a fast-forward of any commit that existed on the PR
> branch. The individual commits on the PR branch (including release-plz's
> unsigned bot commits) never themselves land on `main`; only the
> synthesized, GitHub-signed squash commit does.

So a `required_signatures` rule on `main` is satisfied by every squash
merge regardless of whether the source branch's commits were signed,
**as long as the merge itself goes through GitHub** (not a local `git
merge --squash` + push, which would carry the pusher's own — possibly
absent — signature instead).

**Live-state discrepancy found and corrected by this ADR's implementation
PR:** planning-phase verification (`gh api repos/jagoda/meshcadet/rulesets/
18807600`) found the ruleset held only `deletion`, `non_fast_forward`, and
`pull_request` — **`required_signatures` was absent**, and
`branches/main/protection` (classic protection) returned 404. The design's
premise that squash-merges land signed was sound, but nothing was actually
enforcing it: an unsigned squash-merge (or any other merge-method landing
an unsigned commit, since the ruleset's `pull_request` rule allows `merge`,
`squash`, and `rebase`) could have reached `main` undetected. This PR
re-added `required_signatures` to ruleset `18807600` to close that gap —
see the ruleset's `updated_at` timestamp and rule list for the corrected
live state.

### 6. Boot-version injection seam (Implemented)

`firmware/build.rs::emit_build_version()` reads `MESHCADET_RELEASE_VERSION`
and, if set/non-empty, emits it verbatim as `MESHCADET_BUILD_VERSION` instead
of deriving a value from git. An ordinary `cargo run` (the env var unset)
keeps emitting a bare `git rev-parse --short HEAD` (+ `-dirty`) — deliberately
NOT `git describe --tags`, which would switch to `vX.Y.Z-N-gSHA` the moment
this repo's first tag exists. This is the seam the tag-fired release workflow
(§7) writes to.

### 7. Tag-fired production build + artifacts (Implemented)

`.github/workflows/release.yml` fires on `v*.*.*` tag pushes (created by
release-plz per §2). It first re-verifies the tag's version against the
checked-out `Cargo.toml`/`firmware/Cargo.toml` (belt-and-suspenders on top of
§1's drift guard, in case a tag is ever pushed by hand rather than by
release-plz), then builds default-feature (no `diagnostics`, no `hil`)
production firmware inside the pinned release container (§8), merges
bootloader (`0x0`) + the custom `partition-table.bin` carrying `mc_hist`
(`0x8000`) + the app image (`0x10000`, `factory`) into one flashable image via
`esptool merge_bin` (`firmware/release-container/build.sh` — offsets per
`firmware/partitions.csv`; a bare app `.bin` will not boot against this
project's custom partition table), and publishes a GitHub Release carrying
`meshcadet-vX.Y.Z-merged.bin`, an esp-web-tools `manifest.json` (chip family
`ESP32-S3`, single part at offset `0`), and a `SHA256SUMS`. UF2 and the raw
ELF are deliberately not published (serial flashing only, out of scope for
now). Release notes are extracted from the matching `CHANGELOG.md` section
(§3) rather than hand-written per release.

### 8. Reproducibility + provenance (Implemented)

The release build sets `CONFIG_APP_REPRODUCIBLE_BUILD=y`
(`firmware/sdkconfig.defaults`, applied to every build config, not just
release), injects `SOURCE_DATE_EPOCH` from the tag commit's own date, and
passes `RUSTFLAGS=--remap-path-prefix=...` remapping both the checkout path
and `CARGO_HOME` to fixed, machine-independent targets. All of this runs
inside a pinned container
(`firmware/release-container/Dockerfile`) with `libfreetype6-dev` and
`fonts-dejavu-core` pinned to exact package versions — the one
byte-reproducibility hazard the other three levers cannot reach, since
`firmware/build.rs`'s `build_emoji_font()` shells out to the *host's own*
FreeType/DejaVu Sans to rasterize the bundled emoji font at compile time, and
a different FreeType/DejaVu build silently changes those baked-in bytes.
`docs/release-reproducibility.md` is the full third-party rebuild recipe
(`docker build` + `docker run` + `sha256sum` compare against the published
`SHA256SUMS`).

`actions/attest-build-provenance@v1` (needing `id-token: write` +
`attestations: write`, granted at the workflow level) attests SLSA build
provenance over the three published assets. `docs/release-reproducibility.md`
documents the `gh attestation verify <asset> --repo jagoda/meshcadet`
third-party re-check, and is explicit that attestation (provenance: who
built this, from what source) and the rebuild-and-compare recipe
(reproducibility: does anyone else get the same bytes) are complementary,
not substitutes for each other.

### 9. Web flasher (Not yet implemented)

A self-hosted GitHub Pages `esp-web-tools` flasher page, pointed at the
latest Release's `manifest.json` from §7.

## Consequences

- Contributors must write Conventional Commit messages and PR titles going
  forward (`commitlint.yml` enforces this) — a real but small workflow
  change, and the mechanism by which `CHANGELOG.md` stays truthful without
  manual editing.
- `firmware/Cargo.toml`'s version is no longer purely "whatever the
  maintainer typed" — it is now a CI-enforced invariant tied to the root
  workspace version. Bumping one without the other is a build failure, not
  a silent drift.
- release-plz's bot commits are deliberately left unsigned; the security
  property this project relies on lives entirely in the squash-merge +
  `required_signatures` mechanism (§5), not in the bot's own signing setup.
  If the merge method policy on ruleset `18807600` is ever loosened to
  allow non-squash merges of unreviewed branches, this property needs
  re-examination.
- `firmware/` remains fully outside release-plz's reach by design (see
  Context) — every future piece of this architecture that touches firmware
  versioning must go through the drift-guard pattern established in §1,
  not attempt to fold `firmware/` into the root workspace.

## Alternatives Considered

### A. Merge `firmware/` into the root Cargo workspace

Would let release-plz manage `firmware/Cargo.toml`'s version directly with
no drift guard needed. Rejected: this is the exact coupling
`Cargo.toml`'s workspace-split doc comment and `ci.yml`'s "why no firmware
job" doc already reject — pulling `firmware/` into the root workspace would
require the `esp` toolchain + ESP-IDF sysroot for `cargo test`/`clippy` at
the repo root, reintroducing the slow, network-dependent, flaky dependency
this project deliberately scoped out of every-PR CI.

### B. Classic branch protection instead of a signature-checking ruleset

GitHub's newer repository rulesets (used here, ruleset `18807600`) and
classic branch protection both support "require signed commits". Rulesets
were already in place for this repo's other rules (`deletion`,
`non_fast_forward`, `pull_request`) before this ADR; adding
`required_signatures` to the existing ruleset keeps all `main`-branch policy
in one place instead of splitting it across two mechanisms.

### C. Let release-plz open a GitHub Release itself

Rejected per §2: the production firmware workflow (§7) needs to own the
Release object for a given `vX.Y.Z` tag (it's where the flashable
image/manifest/checksums live) — a second, empty release-plz-created
Release on the same tag would be redundant and confusing to a downloading
user.
