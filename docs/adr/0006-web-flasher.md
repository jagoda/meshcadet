# ADR-0006 — Web Flasher: Version Selector Over a Same-Origin Mirror

- **Status:** Accepted (2026-07-11)
- **Deciders:** Maintainer design review
- **Supersedes:** ADR-0004 §9's original single-latest-pin sketch
- **Implements:** ADR-0004 §9 ("Web flasher — Not yet implemented")
- **Code:** `site/flash.html`, `site/flash.js`, `site/styles.css`
  (flasher-panel rules), `site/index.html` (nav link + hero CTA),
  `.github/workflows/pages-deploy.yml` ("Mirror recent release firmware
  assets" step + `release: published` trigger), `.gitignore`
  (`/site/firmware/`).

## Context

ADR-0004 §7 publishes a GitHub Release per `vX.Y.Z` tag carrying a merged,
flashable firmware image, an esp-web-tools `manifest.json`, and a
`SHA256SUMS`. ADR-0004 §9 sketched — but did not implement — a self-hosted
[esp-web-tools](https://github.com/esphome/esp-web-tools) flasher page on
this project's existing GitHub Pages site, pinned to whatever the *latest*
Release happened to be.

This piece adds that page, plus a version selector (list recent releases,
pick one, flash that one specifically) rather than a single latest-only pin
— both requested together, since the selector changes the manifest-sourcing
design enough that building the single-pin version first and reworking it
immediately after would have been wasted effort.

**Live CORS check (this is the load-bearing finding of this ADR):** a
GitHub Release asset's `browser_download_url` (e.g.
`github.com/OWNER/REPO/releases/download/vX.Y.Z/asset`) 302-redirects to
`release-assets.githubusercontent.com` with a short-lived signed URL.
Neither hop of that redirect chain sends an `Access-Control-Allow-Origin`
header — verified with `curl -H "Origin: https://jagoda.github.io" -D -`
against a real, public release asset (a different, larger public repo's
release, to get a live signed URL to inspect; the finding is a property of
GitHub's release-asset infrastructure, not of this specific repo). That
means browser `fetch()` of a Release's `manifest.json` or merged `.bin`
from the Pages origin is CORS-blocked in `cors` mode — esp-web-tools cannot
read either file directly from a GitHub Release. This is a live-network
finding, not a guess; ADR-0004 §9 and the mission that opened this ADR both
correctly anticipated it as a risk ("CORS fallback... needs live browser
check") but hadn't confirmed it either way.

By contrast, `api.github.com/repos/OWNER/REPO/releases` **does** send
`Access-Control-Allow-Origin: *` (also verified live) — the releases *list*
is fetchable directly from a browser with no proxy needed.

## Decision

### 1. Version list: live client-side fetch of the GitHub releases API

`site/flash.js` fetches `https://api.github.com/repos/jagoda/meshcadet/releases`
on page load, filters to non-draft, non-prerelease releases whose tag
matches `v\d+\.\d+\.\d+` (the same pattern `release.yml` triggers on), sorts
newest-first by `published_at`, and caps the dropdown at the 8 most recent
(`MAX_VERSIONS` in `flash.js`). This needs no server, no redeploy to reflect
a brand-new release in the dropdown's *option list*, and costs nothing to
maintain — it is exactly what the mission asked for.

### 2. Manifest/binary: same-origin mirror, populated by CI

Because Release assets aren't cross-origin fetchable (see Context),
`<esp-web-install-button>` is never pointed at a `github.com`/
`githubusercontent.com` URL. Instead, `.github/workflows/pages-deploy.yml`'s
`build` job gained a "Mirror recent release firmware assets" step that runs
`gh api`/`gh release download` for the same filtered, capped release set
(kept in sync with `flash.js`'s `MAX_VERSIONS` by comment cross-reference —
see §4) and writes each release's `manifest.json` + `meshcadet-<tag>-merged.bin`
+ `SHA256SUMS` into `site/firmware/<tag>/` before `site/` is uploaded as the
Pages artifact. `flash.js` points the install button at the relative path
`firmware/<tag>/manifest.json`, same-origin with the page, so esp-web-tools'
own `fetch()` calls succeed.

`site/firmware/` is git-ignored (`.gitignore`) and absent from a fresh
checkout / local preview — it exists only inside a given Pages deploy's
built artifact, regenerated every deploy from live GitHub Release data. This
keeps the mirrored binaries (which can be several hundred KB each, times up
to 8 releases) out of git history entirely.

### 3. Deploy trigger: push to main, or a published release

`pages-deploy.yml` already redeployed on every `site/**`-touching push to
`main`; this ADR adds `release: types: [published]` as a second trigger, so
a freshly tagged firmware release gets mirrored onto Pages promptly instead
of waiting for the next unrelated site change. Every deploy (regardless of
which trigger fired it) re-runs the mirror step against live GitHub data, so
a failed or delayed release-triggered deploy self-heals on the next site
push rather than leaving Pages permanently behind.

### 4. The MAX_VERSIONS/max_versions coupling is a manual invariant

`site/README.md`'s "no build step, on purpose" convention (plain HTML/CSS/JS,
no bundler) means there is no single source of truth `flash.js`'s JS cap and
`pages-deploy.yml`'s bash/jq cap can both read from. They are two literals
(`MAX_VERSIONS = 8` and `max_versions=8`) kept equal by comment
cross-reference in both files. **Consequence if they drift:** if the CI cap
is ever lowered below the JS cap, the dropdown can list a version whose
`firmware/<tag>/manifest.json` was never mirrored, and installing it 404s
inside esp-web-tools' own error UI (a real but non-silent failure — the user
sees an error, not a hang). If the CI cap is raised above the JS cap, extra
mirrored releases are simply unused disk in the Pages artifact. Neither
direction is silently wrong data reaching a user attempting to flash, which
is why a manual-invariant comment pair was accepted here instead of adding
a templating/build step this site deliberately doesn't have.

### 5. `meshcore.io/flasher` custom-firmware ingestion: out of scope, unresolved

The predecessor, cancelled mission
(`meshcadet-web-flasher-20260711-161609574`) flagged determining how
`meshcore.io/flasher` ingests custom firmware (manifest URL query param?
file upload?) as best-effort, non-blocking research, noting the domain is
Cloudflare-blocked to headless fetch. This piece does not attempt that
research; the self-hosted page (this ADR) is the PRIMARY deliverable and
stands on its own. Revisit only if there's a specific ask to integrate with
that third-party flasher.

## Consequences

- A visitor always sees a live, current release list (client-side GitHub API
  fetch), but can only successfully *flash* a version whose assets have been
  mirrored onto the current Pages deployment — normally seconds-to-minutes
  behind a release publish (§3), not instant. `flash.js`'s network-error and
  non-2xx handling covers GitHub API fetch failures (offline, rate-limited);
  a 404 on a specific mirrored manifest (asset not yet mirrored, or mirror
  step failed for that release) surfaces through esp-web-tools' own
  connect-time error handling instead of custom UI here — acceptable given
  how narrow the race window is in practice.
- Anonymous `api.github.com` requests are rate-limited (60/hour per IP,
  shared across every visitor behind the same NAT/proxy) — acceptable for a
  low-traffic firmware flasher page; revisit (e.g. server-side caching, a
  static `versions.json` generated at deploy time instead of a live client
  fetch) if this ever becomes a real complaint.
- `site/firmware/` existing only inside a built Pages deploy (never in git,
  never in a plain local checkout) means `flash.html` run via
  `python3 -m http.server` (site/README.md's local-preview recipe) will show
  a correctly populated dropdown (live GitHub API call still works) but
  every install attempt 404s, unless a release's assets are manually copied
  into `site/firmware/<tag>/` first. Documented in `site/README.md`.

## Alternatives Considered

### A. Pin to the single latest release only (ADR-0004 §9's original sketch)

Simpler, but explicitly superseded by this mission's ask for a version
selector — flashing an older release (e.g. to roll back, or to match a
known-good build for a bug report) would otherwise require the manual
"grab it from GitHub Releases and flash by hand" fallback unconditionally,
defeating the point of a web flasher.

### B. Generate a static `firmware/versions.json` at deploy time instead of a live `api.github.com` fetch

Would avoid the anonymous-rate-limit exposure (Consequences) and decouple
the dropdown from GitHub API availability entirely (dropdown would only
ever show what's actually mirrored, closing the §5-style race window). Not
done here because the mission's objective explicitly specified "client-side
fetch of /repos/jagoda/meshcadet/releases" driving the dropdown — worth
revisiting if rate-limiting or the mirror race window become real problems
in practice.

### C. Proxy Release-asset fetches through a Cloudflare Worker / other same-origin-appearing proxy

Would let the install button point at the *actual* latest Release asset
without a mirror step, at the cost of standing up and operating
non-GitHub-Pages infrastructure. Rejected: this project's whole Pages/
release setup (ADR-0004) is deliberately all first-party GitHub
infrastructure (Pages + Actions + Releases, no third-party hosting); a
CI-populated same-origin mirror achieves the same CORS-workaround with zero
new infrastructure.
