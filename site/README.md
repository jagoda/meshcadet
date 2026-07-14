# site/

Source for the MeshCadet GitHub Pages project site
(`https://jagoda.github.io/meshcadet/`), deployed by
`.github/workflows/pages-deploy.yml` on every push to `main` that touches
this directory. `.github/workflows/pages-check.yml` runs the relative-path
regression guard (below) on every PR that touches this directory, without
building or publishing anything.

## Structure

- `index.html` — the landing page. Sections are self-contained
  `<section id="...">` blocks (`#about`, `#screenshots`, `#getting-started`,
  `#design`, …), in that reading order after the hero — what it is, then
  proof (screenshots), then how to get started, then the deeper design/scope
  note — and linked from the top nav in the same order; add a new one the
  same way and it shows up in nav + on the page without touching anything
  else. `#about` ("What it does") is a `.card-grid` of the core feature/policy
  points. `#screenshots` is a responsive `.screenshot-grid` of four
  device-resolution (320x240) PNGs, each in a subtle rounded-corner +
  drop-shadow `.screenshot-card` frame (`styles.css`) — see the `assets/`
  bullet below for their provenance. `#getting-started` is the flash →
  provision → message flow: three numbered `.steps` cards, each with a CTA to
  `flash.html` / `provisioner.html` / the project docs, plus a subtle Web
  Serial browser-requirement note. `#design` closes the page with the
  project's scope/philosophy note and the no-warranty disclaimer, right
  before the footer. Two placeholder comments at the bottom of `<main>` mark
  likely next sections (hardware/build gallery, roadmap). The top nav is the
  same 7 items in the same order on `index.html`, `flash.html`, and
  `provisioner.html` — the four story anchors, then `flash.html` /
  `provisioner.html`, then GitHub; whichever tool page you're on marks its own
  nav item with `aria-current="page"` instead of linking to `flash.html` /
  `provisioner.html`.
- `flash.html` + `flash.js` — the two-path web flasher: **Fresh install**
  (erases the whole chip) vs **Upgrade** (writes only the app region,
  preserves device identity/config/history). Both paths drive a shared
  hand-rolled `esptool-js` flow (no `esp-web-tools`/`<esp-web-install-button>`
  dependency at all — see ADR-0011) plus a shared page-level status bar +
  expandable console. `flash.js` fetches
  `api.github.com/repos/jagoda/meshcadet/releases` client-side to populate
  the version dropdown (shared by both paths), then writes either
  `firmware/<tag>/manifest.json`'s merged-image parts (Fresh, shape-checked
  by `flash-manifest.js`'s `resolveFreshInstallParts`) or
  `firmware/<tag>/<app_asset>` from `firmware/<tag>/update-meta.json`
  (Upgrade, gated on `upgrade_safe`, shape-checked by `upgrade-gate.js`'s
  `isValidUpdateMeta`) — **not** directly at the GitHub Release asset, which
  is cross-origin-blocked (no `Access-Control-Allow-Origin` on the
  release-asset redirect chain, verified live). See ADR-0006
  (`docs/adr/0006-web-flasher.md`) for the version-selector/mirror design,
  ADR-0009 (`docs/adr/0009-two-path-flasher.md`) for the two-path split, and
  ADR-0011 (`docs/adr/0011-unified-esptool-js-flasher.md`) for why Fresh
  install also moved onto the hand-rolled flow;
  `.github/workflows/pages-deploy.yml`'s "Mirror recent release firmware
  assets" step populates `firmware/<tag>/` at deploy time (git-ignored, not
  checked in — see below).
- `flash-manifest.js` — DOM-free validation for the Fresh-install path's
  `manifest.json` shape: `resolveFreshInstallParts` picks the
  chip-family-matched `{path, offset}` parts to write, or fails closed
  (`null`) on anything malformed/unexpected — same "fail closed" contract as
  `upgrade-gate.js`'s `isValidUpdateMeta` for `update-meta.json`. Tested
  under plain `node` via `flash-manifest.test.mjs`, run by
  `pages-check.yml`'s `check` job — see `docs/adr/0011-unified-esptool-js-
  flasher.md`.
- `upgrade-gate.js` — DOM-free validation for the Upgrade path's
  `update-meta.json` gate: `isValidUpdateMeta` checks both `upgrade_safe` and
  that `app_asset`/`app_offset` have the exact shape `flash.js`'s Upgrade
  flow expects, failing closed (Fresh install only) on anything else — a
  missing field, a wrong offset, or an `app_asset` that doesn't match
  `release.yml`'s naming convention (guards against path traversal via a
  malformed/corrupted metadata file). Tested under plain `node` via
  `upgrade-gate.test.mjs`, run by `pages-check.yml`'s `check` job — see
  `docs/adr/0009-two-path-flasher.md` D2.
- `provisioner/codec.js` — pure-JS port of the USB-serial provisioning wire
  protocol (`protocol/src/provisioning.rs`): frame encode/decode, CRC-16/ARC,
  `find_magic_start` log-noise resync, and the payload codecs a browser
  provisioner page needs. No build step — plain ES module. Guarded against
  drift from the Rust codec by `provisioner/codec.conformance.test.mjs` +
  `xtask --bin gen-prov-golden-vectors`, run by `pages-check.yml`'s
  `codec-conformance` job on every PR touching either side. See
  `docs/adr/0007-provisioner-codec.md` for the full design (why pure JS
  instead of WASM, and the client-side security model the rest of the
  provisioner page must uphold).
- `provisioner/session.js` — async Web Serial transport + session
  orchestration (`send_recv_with_retry`, the `recv_frame` accumulation loop,
  `find_magic_start` resync, the two-frame `QUERY_STATUS` ->
  `RSP_STATUS`+`RSP_IDENTITY` handshake) driving `codec.js`. A fresh async
  reimplementation of the relevant `host/src/session.rs` orchestration for
  the browser's single-threaded event loop — `host/src/session.rs` itself is
  read only as a reference and is never modified by this or any downstream
  provisioner mission (see `docs/adr/0007-provisioner-codec.md`, Finding 2).
  M1 (the walking-skeleton mission) exposed only the read-only
  `queryStatus()`; M2 (the config mission) layers the non-sensitive
  provisioning **write** commands on the same retry/resync core:
  `listContacts`/`listChannels` (streamed enumeration), `addContact`/
  `delContact`, `addChannel`/`delChannel`, `setNotifDefaults`,
  `setDeviceName`, and `commit` — each a thin `#sendAndExpectOk`/
  `#streamUntilDone` wrapper mirroring the correspondingly named
  `host/src/session.rs` method. M2's sensitive-data mission adds the last
  three: `setPin` (masked admin PIN — scrubs its own payload buffer after
  send), `exportHistory` (streamed `RSP_HISTORY_ENTRY` -> `_DONE`, oldest-
  first, with the same bounded stray-frame tolerance as
  `Session::export_history`), and `clearHistory`. Regression-guarded by
  `provisioner/session.smoke.test.mjs` (a mocked-Web-Serial orchestration
  test — no Rust counterpart to golden-vector against, unlike `codec.js`),
  run by `pages-check.yml`'s `check` job.
- `provisioner/contact-uri.js` — the MeshCore companion contact-add URI
  construction (`meshcore://contact/add?name=&public_key=&type=1`),
  byte-for-byte hand-ported from `host/src/main.rs`'s `url_encode` +
  URI-construction logic. Pulled out of `provisioner.js` into its own
  DOM-free module specifically so it's testable under plain `node` —
  `provisioner/contact-uri.test.mjs` checks it against the exact same
  fixtures as `host/src/main.rs`'s own `#[cfg(test)]` module, run by
  `pages-check.yml`'s `check` job.
- `provisioner/validation.js` — input validation for the M2 write forms:
  contact-pubkey and channel-secret hex-length checks (mirroring
  `host/src/main.rs`'s `parse_32bytes_hex`/`parse_channel_secret_hex`,
  including the 128-bit secret's zero-pad-to-32-bytes behavior), the
  device-name byte-length check (mirroring the `Cmd::Identity` `--set-name`
  check), and `validatePin` (a ≤`MAX_PIN_LEN` byte check that deliberately
  never returns the PIN itself). DOM-free like `contact-uri.js`, so it's
  testable under plain `node` via `provisioner/validation.test.mjs`, run by
  `pages-check.yml`'s `check` job.
- `provisioner/history-format.js` — the DOM-free port of
  `host/src/history_format.rs`: renders exported history entries into the
  same fixed-width `idx  timestamp  type  dir  from  text` transcript the CLI
  prints, so a browser-downloaded transcript reads identically. It only
  *formats* entries (which carry private message text) — it never logs,
  persists, or transmits them. Tested via `provisioner/history-format.test.mjs`
  (column-alignment invariant, TZ pinned to UTC for deterministic
  timestamps), run by `pages-check.yml`'s `check` job.
- `provisioner.html` + `provisioner.js` — the provisioner page itself:
  connect over Web Serial (mirrors `flash.html`'s Chrome/Edge + HTTPS
  guidance and unsupported-browser fallback), then render status/identity
  (via `session.js`) and a MeshCore contact QR (via `contact-uri.js`). The QR
  itself is rendered by a major-version-pinned CDN import of the `qrcode`
  npm package via esm.sh (pure JS, no WASM) — the same single-pinned-CDN-
  import, no-bundler pattern `flash.html` uses for esp-web-tools. M2 adds the
  non-sensitive provisioning **writes**: contact/channel list + add/remove
  (with a client-side `crypto.getRandomValues` channel-secret generator —
  generated secrets never leave the browser), notification defaults,
  device-name set, and commit (mirroring the CLI's operator notes —
  reboot-to-apply-to-the-live-mesh for contacts, first-boot-commit-reboots
  for commit). Channel deletion needs the exact secret (not just the list
  view's 1-byte hash), so it has its own form rather than a per-row button.
  M2's sensitive-data mission adds three more sections, each upholding the
  ADR-0007 client-side security model: a **masked admin-PIN** field (sent to
  the device then cleared from the DOM, never in URL/storage/console); a
  two-step **history export** (read into memory, then written to disk only on
  an explicit "Download transcript" click — the transcript holds private
  message text and is never auto-downloaded, transmitted, or logged); and a
  triple-gated **clear-history** (reveal → acknowledgement checkbox → native
  `confirm()` dialog) that surfaces the CLI's reboot-to-refresh note.
- `styles.css` — one stylesheet, no build step. Color tokens at the top
  mirror `firmware/src/ui/theme.slint`'s `Theme` global 1:1, so the site and
  the on-device UI read as the same product. Keep them in sync if the
  firmware palette changes.
- `assets/` — pixel-art motifs copied from `firmware/assets/space/`
  (project-owned, GPLv3, reproducibly generated by
  `firmware/generate_assets.py`). Reuse assets from there rather than adding
  new art when possible, to keep the visual identity consistent.
  `assets/screenshot-{splash,contacts,messages,compose}.png` are a different
  kind of generated asset: 320x240 host-sim renders of the REAL production
  screens (`firmware/src/ui/screens/{splash,contact_list,message_view,
  compose}.rs`'s markup, mounted verbatim), seeded with tasteful sample data,
  produced by `ui_sim`'s `{contact_list,message_view,compose,splash}_promo`
  render rigs — same "checked-in, build-reproducible" convention as
  `docs/renders/` (see `ui_sim/README.md`). Regenerate any one with
  `cargo run -p ui_sim --bin <name>_promo_render` after a markup or seed-data
  change.
- `firmware/` — **not checked in.** Populated at deploy time by
  `.github/workflows/pages-deploy.yml`, which mirrors each recent GitHub
  Release's `manifest.json` + merged firmware image into
  `firmware/<tag>/` so `flash.html` can fetch them same-origin. Empty (and
  absent) in a fresh checkout and in local preview — `flash.html`'s version
  dropdown will list releases (that part is a live GitHub API call) but
  installing will 404 locally unless you mirror a release's assets in by
  hand first.

## Conventions

- **No leading-slash paths.** This is a *project* Pages site
  (`/meshcadet/...`), not a root site — every asset/stylesheet reference must
  be relative (`assets/foo.png`, not `/assets/foo.png`) or it 404s in
  production while still working locally from `file://`.
- **No build step, on purpose.** Plain HTML/CSS keeps this deployable by a
  two-step Actions job (`upload-pages-artifact` + `deploy-pages`) with zero
  toolchain. If this grows enough to need templating, revisit — but a static
  landing page doesn't.

## Local preview

```sh
cd site
python3 -m http.server 8000
# open http://localhost:8000/
```
