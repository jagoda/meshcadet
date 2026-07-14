# ADR-0004 — Release Architecture

- **Status:** Accepted (2026-07-11); §2/§3/§4 revised (2026-07-12) —
  version/changelog/tag automation moved from release-plz to
  release-please. See §2's "Why release-please, not release-plz" for why.
  §10 added (2026-07-12) — Cargo.lock/firmware/Cargo.lock kept in lockstep
  with the version bump via a text-only sync script. §10 revised
  (2026-07-13) — the sync commit is now created via the GitHub API
  (`createCommitOnBranch`) instead of a plain in-runner `git commit`, so it
  carries a verified signature; see §10 and the corrected §5/Consequences
  note below on why an unsigned commit on the PR branch is NOT safe under
  `required_signatures`, contrary to this ADR's original assumption. §2/§7
  and Alternatives Considered §C corrected, §11 added (2026-07-14) —
  `skip-github-release: true` turned out to structurally deadlock every
  release (v0.2.0 and v0.3.0 both needed manual recovery); §11 records the
  root cause and the fix (release-please now owns the initial Release
  object, `release.yml` enriches it rather than creating a competing one).
- **Deciders:** Maintainer design review
- **Supersedes:** —
- **Implements:** —
- **Code:** `release-please-config.json`, `.release-please-manifest.json`,
  `CHANGELOG.md`, `.github/workflows/release-please.yml`,
  `.github/workflows/commitlint.yml`, `scripts/check-commit-format.sh`,
  `scripts/sync-cargo-lock-versions.sh`,
  `CONTRIBUTING.md` ("Submitting changes"), `.github/workflows/ci.yml`
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
  tool that operates on "the Cargo workspace" (release-please included, when
  it edits `Cargo.toml`) only sees the root workspace's crates; `firmware/`
  is invisible to it — which is exactly why release-please's `extra-files`
  config (§1) targets `firmware/Cargo.toml` explicitly, by path, rather than
  relying on workspace discovery to find it.
- All crates are `publish = false` (Cargo.toml `[workspace.package]`) —
  MeshCadet is an end-user firmware project, not a crates.io library
  ecosystem.
- Merges to `main` happen exclusively via signed, PR-only squash-merge
  (ruleset `18807600`) — no direct pushes, no fast-forwards, no deletions.

## Decision

### 1. Single project-wide version (Implemented)

One version number for the whole project, not one per crate. The root
workspace declares it once — `Cargo.toml`'s `[workspace.package].version`
— and every root-workspace crate inherits it via `version.workspace = true`.
release-please manages this one field (as of the 2026-07-12 revision — see
below §4); bumping it bumps every root-workspace crate identically, and it
opens exactly one PR for the whole project rather than one per crate.

`firmware/Cargo.toml` cannot use `version.workspace = true` — it is its own
detached workspace root (see Context) with nothing to inherit from. Its
`version` field is a hardcoded literal. release-please's `extra-files`
config (`release-please-config.json`) targets it directly by TOML path
(`$.package.version`) alongside the root workspace's own
`$.workspace.package.version`, so both are bumped in the same release PR
without release-please needing to cross the workspace boundary at all —
neither file relies on the other's workspace-inheritance mechanism.
**CI still enforces the invariant independently** —
`.github/workflows/ci.yml`'s `version-drift-guard` job fails any PR where
the two values disagree — as a belt-and-suspenders check against a human
hand-editing one file without the other, or `extra-files` ever
misconfigured, not as the primary bump mechanism.

All root-workspace crates were bumped `0.0.0` → `0.1.0` in the same change
that wired this up, and `firmware/Cargo.toml` was bumped to match.

### 2. release-please: version bump + changelog PR + tag (Implemented; revised 2026-07-12)

`release-please-config.json` + `.release-please-manifest.json` configure
[release-please](https://github.com/googleapis/release-please)
(`.github/workflows/release-please.yml`, `googleapis/release-please-action`)
to open (and keep up to date) a PR titled `chore(release): vX.Y.Z` whenever
Conventional Commits land on `main` that warrant a release, and — in the
same workflow run that detects such a PR was just merged — create the
`vX.Y.Z` git tag consumed by the production firmware release workflow (§7),
plus a GitHub Release object for that tag (changelog notes only, no
assets). It never publishes to crates.io. **This Release object is no
longer skipped (`skip-github-release` was removed 2026-07-14 — see §11):**
release-please's own tag+label hand-off is only reachable through that
object, so skipping it structurally deadlocked every release (§11). The
production firmware release workflow (§7) still owns the *user-facing*
shape of the Release for a given tag — it finds the one release-please
already created and edits/uploads onto it, rather than creating a second,
competing Release.

**Why release-please, not release-plz (this section's original tool):**
release-plz's `git_only` mode — needed because every crate here is
`publish = false` end to end, so there is no crates.io baseline to version
against — unconditionally runs `cargo package --workspace` to materialize
each package's manifest, and that step requires every `path` dependency,
even to a workspace-internal sibling that will never be published, to
resolve against the *real* crates.io index. Verified two ways: reading
`release_plz_core`'s `next_ver.rs` source, and reproducing the exact
failure with plain `cargo package --workspace` outside release-plz
entirely. Two independent failure modes followed from this: (a) a
never-published crate name with no collision on crates.io fails outright
("no matching package found"), and (b) a never-published crate name that
*does* collide with an unrelated real crate of the same name (this
project's `protocol`) silently resolves against that wrong crate and fails
to compile — a live bug, not a hypothetical one, confirmed by reproducing
it directly. Separately, release-plz's tag-creation command
(`release_plz_core::project::Project::publishable_packages()`) filters
candidate packages by each crate's own Cargo.toml `publish` field, which is
`false` everywhere in this project (`cargo metadata` confirms
`"publish": []` on every crate) — meaning release-plz could never create
the release tag automatically, regardless of `git_only`, regardless of
merge-vs-squash. (This is almost certainly the real cause behind v0.1.0's
tag needing manual bootstrapping, a fact this ADR's §4 investigation had
attributed to squash-merge instead.) release-please has neither failure
mode: it is pure git/GitHub-API/text-manifest automation and never invokes
`cargo` at all.

`release-please-config.json` uses the `simple` release-type (not the
built-in `rust`/cargo-workspace strategy — that strategy requires the root
`Cargo.toml` to have its own `[package]` table and writes literal
`[package].version` into every member, neither of which fits this repo's
pure-`[workspace]` root + `version.workspace = true` inheritance shape) with
an `extra-files` entry per §1 targeting `Cargo.toml`'s
`$.workspace.package.version` and `firmware/Cargo.toml`'s `$.package.version`
directly by TOML path.

### 3. Changelog: release-please from Conventional Commits (Implemented; revised 2026-07-12)

`release-please-config.json`'s `changelog-sections` groups commits by
Conventional Commit type into the same Keep a Changelog sections the
original git-cliff-based setup used (`feat` → Added, `fix` → Fixed, `perf` →
Performance, `refactor` → Changed, `revert` → Reverted, `docs` →
Documentation). Internal bookkeeping commit types (`chore`, `ci`, `build`,
`style`, `test`) are marked `hidden: true` and filtered out of the
user-facing changelog entirely. `cliff.toml`/git-cliff is no longer part of
this pipeline — release-please has its own built-in changelog generator and
never reads it — so it was removed rather than left as dead config.

`CHANGELOG.md`'s hand-written header (the `# Changelog` title + Keep a
Changelog blurb) is preserved; release-please inserts each new release's
entry immediately above the most recent existing `## [x.y.z]`-style heading
it finds, never touching content above that. One format difference from
the removed git-cliff setup: release-please has no concept of a
perpetually-empty `## [Unreleased]` placeholder heading — it accumulates
pending commits in its own PR/manifest state instead, so the standalone
`## [Unreleased]` heading was removed from `CHANGELOG.md` as part of this
revision (its presence would otherwise have been misdetected as a version
heading by release-please's changelog updater, since its heading-match regex
treats a bare `[` the same as a leading digit).

### 4. Conventional-commit CI gate (Implemented)

`.github/workflows/commitlint.yml` runs on every PR:
- **`lint-pr-title`** — the PR title must itself be a Conventional Commit
  (`amannn/action-semantic-pull-request`). release-please parses the
  individual commits reachable from `main` since the last release tag
  directly (not the PR title) into `CHANGELOG.md` entries/version bumps, so
  this check's job is to keep the PR title itself — the thing a human
  actually reads on the Pull Requests list and in `git log --merges` —
  honest and consistent with the commits it summarizes.
- **`lint-commit-messages`** — every individual commit in the PR must also
  be a Conventional Commit: it invokes `scripts/check-commit-format.sh` (a
  small dependency-free bash regex check against `git rev-list base..head`),
  the same script contributors are required to run locally before
  publishing a branch (`CONTRIBUTING.md` "Submitting changes") — this is
  the left-shift piece (Implemented, 2026-07-12): non-conventional commits
  (e.g. an un-squashed worker `checkpoint:` commit) are now caught before
  `git push`/`gh pr create` instead of surfacing only as a CI red check
  after the PR is already open, and because both call sites run the exact
  same script, local and CI verdicts cannot drift apart. This catches
  non-conventional commits before a title edit can paper over them, and
  remains correct if the merge method is ever changed away from squash.
  Coverage boundary: this covers commits published through the interactive
  PR-prep flow (mission/feature branches); release-please's own
  auto-generated release-branch commits are exempted here (see below) and
  handled separately.

`ci.yml`'s existing three jobs (`test`, `fmt`, `clippy`) are untouched and
stay required; `commitlint.yml` and the new `version-drift-guard` job are
additive gates, not replacements.

**release-please's own release-commit subject is exempt, defensively:**
the prior release-plz-based setup had a verified, specific bug here —
release-plz's `update_pr` code path committed the release PR's *previous*
title on every push after the first, so its commit subject could legitimately
lag its own (correct, visibly-updated) PR title by one automation run. That
investigation is what motivated exempting the tool's own bot commits from
this check in the first place, rather than relying on the PR title alone.
release-please's own branch/commit state machine has not been read/verified
to the same depth, so the exemption is kept rather than assumed unnecessary:
`scripts/check-commit-format.sh` exempts commits that are BOTH on
release-please's own branch (`release-please--branches--<target-branch>`)
AND authored by its bot identity (`github-actions[bot]`) from the
Conventional Commits format check — but ONLY when invoked with
`EXEMPT_RELEASE_PLEASE=1` (`commitlint.yml`'s `lint-commit-messages` job
sets this; local/manual runs of the script for the pre-PR-publish step in
`CONTRIBUTING.md` leave it unset, which is correct — release-please's own
commits never pass through that interactive flow, see §4's coverage-boundary
note above) — leaving human-commit enforcement, on that branch and
everywhere else, unchanged.

### 5. `required_signatures` on `main` (Implemented — ruleset re-verified/re-added; original squash-merge premise corrected below)

This repo requires every commit on `main` to carry `required_signatures`,
but also merges exclusively via **squash**-merge through the GitHub UI, and
release-please's own bot commits on its PR branch are **not** signed. Those two
facts are compatible for a specific reason, worth spelling out because it
is not obvious from either policy alone:

> When a PR is squash-merged through the GitHub web UI (or `gh pr merge
> --squash`), GitHub synthesizes a **new, single commit** on the target
> branch. That commit is authored and signed by GitHub's own web-flow
> identity (`GitHub <noreply@github.com>`, GitHub's own GPG/SSH signing
> key) — it is not a fast-forward of any commit that existed on the PR
> branch. The individual commits on the PR branch (including release-please's
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

**Correction (2026-07-13) — the squash-merge premise above does not match
actual practice.** Despite this section's title and the "merges exclusively
via squash-merge" claim, every PR merge in this repo's real history is a
**regular (non-squash) merge commit** — `git show --no-patch --format="%P"`
on any merged PR's merge SHA shows two parents (the pre-merge `main` tip and
the PR branch's own head), e.g. `2cd1802` (PR #24), `25ead45` (PR #22),
`5fbada2` (PR #19); `commitlint.yml`'s own header comment states this
explicitly ("every PR ... via a signed merge commit ... never squash").
Ruleset `18807600`'s `pull_request` rule permits `merge`/`squash`/`rebase`
alike, but the merge button has consistently been used in `merge` mode.
Regular merge does not synthesize a new commit for the PR's own contents —
it preserves every commit from the PR branch verbatim as an ancestor of the
new merge commit on `main`. So the mechanism was never "GitHub signs one
synthesized commit and the rest ride along unsigned" — it is much more
directly: **every individual commit on a PR branch lands on `main` byte-for-
byte, so `required_signatures` must be (and is) satisfied by every one of
them individually**, with no squash step in the picture at all. An unsigned
commit anywhere in the PR — release-please's own bump commit or the
Cargo.lock sync commit — is therefore an unsigned commit landing directly on
`main`, exactly what `required_signatures` exists to reject. This was found
by a prior mission attempting to exercise this section against a live run
(the Cargo.lock sync commit's plain `git commit` failed this way), and is
why §10's sync commit is now created through the GitHub API
(`createCommitOnBranch`) rather than a raw `git commit`: it needs to be
signed on its own merits, not by leaning on a squash boundary that this repo
doesn't actually use. **Second live-state discrepancy found by this
mission:** as of 2026-07-13, `required_signatures` was again absent from
ruleset `18807600` (`gh api .../rulesets/18807600/history` shows it present
as of version `42771384` and gone by `42874592`, dated the same evening as
the abandoned attempt referenced above) — restored as a separate,
explicitly-logged action once this fix's own PR no longer depended on it
being off (see this mission's dossier for the exact sequencing). The
squash-merge framing in this section's title/opening paragraphs is left
in place below as the historical record of the (mistaken) original design
premise; treat this correction as superseding it.

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
release-please per §2). It first re-verifies the tag's version against the
checked-out `Cargo.toml`/`firmware/Cargo.toml` (belt-and-suspenders on top of
§1's drift guard, in case a tag is ever pushed by hand rather than by
release-please), then builds default-feature (no `diagnostics`, no `hil`)
production firmware inside the pinned release container (§8), merges
bootloader (`0x0`) + the custom `partition-table.bin` carrying `mc_hist`
(`0x8000`) + the app image (`0x10000`, `factory`) into one flashable image via
`esptool merge_bin` (`firmware/release-container/build.sh` — offsets per
`firmware/partitions.csv`; a bare app `.bin` will not boot against this
project's custom partition table), and publishes onto the Release
release-please already created for this tag (§2/§11) the merged image, an
esp-web-tools `manifest.json` (chip family `ESP32-S3`, single part at offset
`0`), the app-only update image, its manifest/metadata counterparts, and a
`SHA256SUMS` — `gh release edit` + `gh release upload --clobber`, not `gh
release create` (§11). UF2 and the raw ELF are deliberately not published
(serial flashing only, out of scope for now). Release notes are extracted
from the matching `CHANGELOG.md` section (§3) rather than hand-written per
release.

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

### 9. Web flasher (Implemented — see ADR-0006)

A self-hosted GitHub Pages `esp-web-tools` flasher page
(`site/flash.html`), with a version selector over recent Releases from §7
rather than a single latest-only pin. **ADR-0006 is the record of how it's
actually built** — in particular, a live check found GitHub Release assets
carry no CORS headers, so the flasher can't fetch a Release's
`manifest.json`/merged image directly; it points at a same-origin mirror
that `.github/workflows/pages-deploy.yml` populates from live Release data
at deploy time instead. This section is left brief on purpose so the two
ADRs don't drift out of sync with each other — see ADR-0006 for the full
design and its Alternatives Considered.

### 10. Cargo.lock kept in lockstep via a text-only sync script, committed via the GitHub API (Implemented)

§1/§2's `extra-files` config bumps the version *string* in Cargo.toml and
firmware/Cargo.toml, but release-please has no built-in Cargo.lock awareness
for `release-type: simple` (that's a `rust`/cargo-workspace release-type
behavior — see §2's "Why release-please, not release-plz" for why this repo
doesn't use it). Left alone, a release PR's Cargo.lock and
firmware/Cargo.lock keep pinning the OLD version for every workspace-member
crate, and `ci.yml`'s `cargo test --locked`/`cargo clippy --locked` hard-fail
on the release PR: "cannot update the lock file because --locked was
passed" — `--locked` deliberately refuses to silently regenerate a stale
lockfile. (Live case: PR #25, `chore(release): v0.1.1`, failed exactly this
way.)

`.github/workflows/release-please.yml` runs
`scripts/sync-cargo-lock-versions.sh` immediately after release-please opens
or updates the release PR (gated on the action's `prs_created` output), and
commits a `chore(release): sync Cargo.lock to the version bump above` change
back onto the PR branch if either lockfile changed — via the GraphQL
`createCommitOnBranch` mutation (`actions/github-script`), not a plain
in-runner `git commit` + `git push` (see §5's correction: this repo merges
via regular, non-squash merge commits, so every commit on the release PR
lands on `main` verbatim and must individually satisfy
`required_signatures` — an unsigned one anywhere on the branch blocks the
merge outright). Commits created through GitHub's API are
signed server-side by GitHub itself and always show `verified: true` — the
same mechanism `googleapis/release-please-action`'s own bump commit already
uses. That script is a
**text-only substitution**, not a `cargo` invocation — it rewrites just the
`version = "..."` line of each local workspace-member `[[package]]` stanza
(these never carry a `source =`/`checksum =` line, unlike registry
dependencies, which is what makes the substitution unambiguous) and leaves
every other line of both lockfiles byte-identical. Two things this
deliberately avoids by not shelling out to `cargo`:

- **The `esp`/Xtensa toolchain problem.** firmware/Cargo.lock's workspace is
  pinned to the `esp` channel (firmware/rust-toolchain.toml) — any real
  `cargo` command touching it would need that toolchain installed on the
  release-please runner, which doesn't have it and shouldn't need to (§2's
  Alternatives-Considered-style reasoning: this workflow stays
  toolchain-independent, same motivation as `firmware/`'s workspace split in
  Context).
- **Unrelated dependency drift.** `cargo generate-lockfile` (or `cargo
  update` with no package filter) re-resolves the entire dependency graph
  against the live registry and can silently pull in unrelated
  semver-compatible upgrades alongside the version bump — verified
  empirically while designing this fix: `cargo check --workspace` (which
  uses the existing lockfile as its resolution baseline) touches ONLY the
  local package version fields, but `cargo generate-lockfile` rewrote dozens
  of unrelated registry-dependency entries in the same run. A release PR's
  diff should be the version bump, not a surprise dependency upgrade.

The commit is authored as `github-actions[bot]
<41898282+github-actions[bot]@users.noreply.github.com>` — the default
identity GitHub attributes to `GITHUB_TOKEN`-authenticated API commits, the
same one release-please's own commit uses — so
`scripts/check-commit-format.sh`'s existing release-please exemption (§4)
covers it on the same `release-please--branches--*` prefix without any
changes to that script.

**Live defect this revision fixes:** the original (2026-07-12) version of
this step used a plain in-runner `git commit` + `git push` under the
workflow's local, unconfigured git identity, which has no GPG/SSH key at
all — its commit is unverified (`reason: unknown_key`). Live evidence:
`4ef0737`, the sync commit on the since-closed PR #25, is exactly this
(`gh api repos/jagoda/meshcadet/commits/4ef0737` →
`verification.verified: false`), sitting alongside that same run's
release-please bump commit `3a7125d`, which IS verified (API-created). A
prior mission attempted to enforce `required_signatures` against this and
found the resulting PR unmergeable (§5's correction) — this revision closes
that gap by committing the sync through the same API path release-please
itself already uses, rather than through raw git.

**Known caveat, not treated as a blocker:** a ref update to an existing PR
branch made under the workflow's default `GITHUB_TOKEN` does not always
cause GitHub to fire a fresh `pull_request: synchronize` check run (GitHub's
recursive-workflow-run prevention) — true whether the update comes from a
plain `git push` or the GraphQL mutation used here, since both go through
the same `GITHUB_TOKEN` identity. Empirically, this repo's
`pull_request`-triggered checks (`ci.yml`, `commitlint.yml`) DID run
automatically off release-please's own `GITHUB_TOKEN`-authored commit
opening PR #25, so this is expected to work the same way for this sync
commit in the common case; if a future release PR's checks ever look stale
after this step runs, a maintainer re-running the checks (or pushing any
follow-up commit) picks up the synced lockfile — the lockfile fix itself
lands correctly regardless.

### 11. Closing the tag/label deadlock: release-please owns the initial Release object (Implemented)

**The defect.** `skip-github-release: true` (§2's original text) was meant
to avoid two competing Release objects on the same tag, but it silently
disabled release-please's own tag-on-merge completion. Reading
release-please's source (`src/strategies/base.js`, `src/manifest.js`,
version `17.10.3`, the version `googleapis/release-please-action@v5`
itself pins): `Strategy.buildRelease()` returns immediately, before ever
computing a tag, whenever `this.skipGitHubRelease` is set — so
`Manifest.buildReleases()` produces zero candidate releases for a merged
release PR, `Manifest.createReleases()` never calls
`createReleasesForPullRequest()` for that PR, and the unconditional
`removeIssueLabels(pending)` + `addIssueLabels(tagged)` swap inside
`createReleasesForPullRequest()` — the ONLY place in release-please that
flips the label — never runs. The PR's `autorelease: pending` label
therefore never becomes `autorelease: tagged`, and every subsequent
release-please run aborts with "There are untagged, merged release PRs
outstanding" (`Manifest.createPullRequests()`'s own check on that same
label). This is not a transient bug: it recurs on **every** release, and
bit both v0.2.0 and v0.3.0, each requiring a hand-created tag + a
hand-applied label swap to unblock (missions
`meshcadet-release-v0-2-0-stranded-tag-20260713-012022784` and
`meshcadet-release-v0-3-0-deadlock-clear-20260714-023732442`).

**The fix.** Remove `skip-github-release` from `release-please-config.json`
entirely (defaults to `false`), so `buildRelease()` runs its full path:
computes the tag, calls `createRelease()` (creates the real tag + a Release
object — changelog notes only, no firmware assets), and
`createReleasesForPullRequest()` then unconditionally performs the label
swap on every path through it (the plain-success branch and the
duplicate-release branch both call it) — the deadlock cannot recur, because
the label flip is no longer gated on anything `release.yml` controls.
`.github/workflows/release.yml`'s "Publish or update GitHub Release" step
changed from a bare `gh release create` (which would now fail outright —
the tag already has a Release) to: check for an existing Release on the tag
(`gh release view`), `gh release edit` its title/notes if found (the normal
case — release-please already made it), `gh release create` as a fallback
only if the tag was pushed by hand with no release-please-created Release
at all, then `gh release upload --clobber` the six production assets onto
it either way. `--clobber` also makes a re-run after a partial upload
idempotent, closing a secondary, non-structural failure mode the old
`gh release create`-only step had (§7's prior "KNOWN FAILURE MODE" comment).

**A second defect caught during this fix's own review, before landing:** a
release-please-created Release publishes immediately (not draft by
default), which fires GitHub's `release: published` event right away — the
same event `.github/workflows/pages-deploy.yml` listens for to mirror a new
release's firmware assets onto the site flasher (ADR-0006). Published
immediately, that Release has no assets yet (the firmware build hasn't even
started), so the mirror step would run against an empty release; and
because `gh release edit` (used later to attach the real assets) fires
`release: edited`, not `published`, the site would never get a second
chance to pick up this tag's real assets. Fixed by also setting
`release-please-config.json`'s `draft: true` (the Release release-please
creates stays hidden from the public until explicitly published) and
`force-tag-creation: true` (GitHub otherwise defers creating the underlying
git tag until a draft release is published, which would silently break
`release.yml`'s own `push: tags` trigger). `release.yml` un-drafts the
Release (`gh release edit "${TAG}" --draft=false`) as its very last step,
after the real notes and every asset are already attached — so
`release: published` fires exactly once, against a complete Release. The
manual-tag fallback branch (`gh release create` with no release-please
Release to inherit) is unaffected: it still publishes immediately, same as
this step always did before this change, since there's no draft state to
manage in that path.

**Alternative considered and rejected: a bespoke label-flip mechanism with
`skip-github-release` left in place** (option (b) from this fix's own
mission brief) — e.g. a second workflow, triggered on `release-please.yml`
completing, that inspects the merged PR and manually swaps the label the
way the two stranded-tag recovery missions did by hand. Rejected: this
reimplements, as bespoke bookkeeping this repo would have to maintain
forever, exactly the state machine release-please already ships and tests
upstream — every future release-please upgrade would need re-verifying
against a parallel, hand-rolled label-flip path. Enabling the tool's native
Release-based completion is strictly less code and less to maintain, and is
the supported way the upstream project expects this hand-off to work.

**Preserved:** GitHub still authors/signs the tag exactly as before (no
change to how `push: tags` fires `release.yml`, and no local/YubiKey
signing enters this path either way); the published Release still ends up
in the v0.1.0/v0.2.0/v0.3.0 shape (same title format, same six assets, same
SLSA provenance), and — thanks to the draft/un-draft handling above — a
downloading user or the site flasher never observes an incomplete Release:
what release-please creates stays a draft, invisible outside the repo,
until `release.yml` un-drafts it with everything already attached. The tag
itself still appears immediately (`force-tag-creation: true`), so
`release.yml`'s own build starts without waiting on anything.

**Verification.** A live release could not be cut in-mission from this
sandboxed context (GPG/SSH commit signing requires a physical YubiKey touch
that the automated worker cannot supply — confirmed live, `git commit`
timed out waiting on the signing agent). Verification instead traced the
actual mechanism end to end against the pinned library version: read
`buildRelease()`'s `skipGitHubRelease` early-return and
`createReleasesForPullRequest()`'s unconditional label swap directly in the
installed `release-please@17.10.3` source (the same major version
`release-please-action@v5` depends on, confirmed via that action's own
`package.json`), confirmed `draft`/`force-tag-creation` are real,
supported top-level config keys against that same package's own JSON
schema (including the schema's own note that `force-tag-creation` is
"particularly useful when draft is enabled" for exactly the lazy-tag reason
above), traced `github-api.js`'s `createRelease()` to confirm `forceTag`
issues a standalone `git.createRef` for the tag *before* the (draft)
`repos.createRelease` call — so the tag ref, and this repo's
`push: tags`-triggered `release.yml`, fire immediately regardless of the
Release's draft state — and exercised the corrected `release.yml` step's
existence-check
logic live and read-only against the real repository (`gh release view
v0.3.0` correctly selects the "edit" branch; `gh release view` against a
nonexistent tag correctly selects the "create" fallback branch). Read
`.github/workflows/pages-deploy.yml`'s own trigger (`release: types:
[published]`) directly to ground the second defect above, rather than
assuming GitHub's draft/edit/publish event semantics. Both workflow YAMLs
and the config JSON parse cleanly. The next real release cycle is the
end-to-end confirmation that no manual tag/label recovery is needed.

## Consequences

- Contributors must write Conventional Commit messages and PR titles going
  forward (`commitlint.yml` enforces this) — a real but small workflow
  change, and the mechanism by which `CHANGELOG.md` stays truthful without
  manual editing.
- `firmware/Cargo.toml`'s version is no longer purely "whatever the
  maintainer typed" — it is now a CI-enforced invariant tied to the root
  workspace version. Bumping one without the other is a build failure, not
  a silent drift.
- **Correction (2026-07-13):** the line originally here claimed
  release-please's bot commits are "deliberately left unsigned" and that
  the security property lives entirely in squash-merge + `required_signatures`
  (§5). That was wrong on both counts: this repo merges via regular
  (non-squash) merge commits (§5's correction), so every PR commit lands on
  `main` verbatim and must itself satisfy `required_signatures` — there is
  no squash-synthesized commit to lean on. And
  `googleapis/release-please-action` already creates its version-bump commit
  via the GitHub API, which GitHub signs server-side (`verified: true`) —
  it was never unsigned to begin with. §10's lock-sync commit now uses that
  same API-commit mechanism for the same reason: every commit on a release
  PR must be independently verified.
- `firmware/` remains fully outside release-please's reach by design (see
  Context) — every future piece of this architecture that touches firmware
  versioning must go through the drift-guard pattern established in §1,
  not attempt to fold `firmware/` into the root workspace.

## Alternatives Considered

### A. Merge `firmware/` into the root Cargo workspace

Would let release-please manage `firmware/Cargo.toml`'s version directly with
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

### C. Let release-please open a GitHub Release itself

Originally rejected per §2's first text: the concern was that a second,
empty release-please-created Release alongside the production firmware
workflow's (§7) own would be redundant and confusing to a downloading user.
**Corrected 2026-07-14 (§11):** that concern conflated "release-please
creates *a* Release object" with "release-please creates a *second, visible*
Release" — the two are not the same thing. release-please's tag-on-merge
completion (and the `autorelease: pending` -> `autorelease: tagged` label
flip that depends on it) is only reachable through its own
`createReleasesForPullRequest()`, which requires a Release object to exist;
skipping it (`skip-github-release: true`) didn't avoid a redundant Release,
it disabled release-please's own hand-off entirely, deadlocking every
release. §11's fix keeps this alternative's original goal — one coherent,
asset-bearing Release per tag, not two — by having `release.yml` **edit**
release-please's Release object in place rather than creating a competing
one, so the end state a downloading user sees is unchanged.
