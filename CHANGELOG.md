# Changelog

All notable changes to MeshCadet are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning is managed by [release-please](https://github.com/googleapis/release-please)
— see `release-please-config.json` and `docs/adr/0004-release-architecture.md`.
The entry below documents everything landed before release-please's first
`chore(release): vX.Y.Z` PR.

## [0.3.2](https://github.com/jagoda/meshcadet/compare/v0.3.1...v0.3.2) (2026-07-14)


### Fixed

* **release:** let release-please complete its tag+label hand-off ([80b9cff](https://github.com/jagoda/meshcadet/commit/80b9cffeb79f2d713c2000daf5c81a88f5c54e6f))
* **release:** let release-please complete its tag+label hand-off ([7065f88](https://github.com/jagoda/meshcadet/commit/7065f88cdd77fde230433da664869278fa691c1a))

## [0.3.1](https://github.com/jagoda/meshcadet/compare/v0.3.0...v0.3.1) (2026-07-14)


### Fixed

* **release:** re-seed layout-baseline.txt to match generate-update-meta.sh's real hashing convention ([c104e9c](https://github.com/jagoda/meshcadet/commit/c104e9c1204e4228a31671d16264099275a9586f))
* **release:** re-seed layout-baseline.txt to match generate-update-meta.sh's real hashing convention ([69c95e9](https://github.com/jagoda/meshcadet/commit/69c95e96338eac46d50e5f05bd748340ef4973a2))
* **site:** convert flasher images to binary strings before esptool-js writeFlash ([d678abc](https://github.com/jagoda/meshcadet/commit/d678abc99b0a1653ea5f9a71143bb5bbb0697271))
* **site:** convert flasher images to binary strings before esptool-js writeFlash ([bfda9d4](https://github.com/jagoda/meshcadet/commit/bfda9d47f31a649eadefa459cd6f16ab322d101d))
* **site:** mirror release assets promptly + fix the web flasher's Fresh-install flow ([abbb8ea](https://github.com/jagoda/meshcadet/commit/abbb8eaf138b9e1d70c42a70274bd199e95019ca))
* **site:** mirror release assets promptly + fix the web flasher's Fresh-install flow ([53f917c](https://github.com/jagoda/meshcadet/commit/53f917cac4de831e65e302834092f45b8c997413))

## [0.3.0](https://github.com/jagoda/meshcadet/compare/v0.2.0...v0.3.0) (2026-07-14)


### Added

* **firmware-core:** add repeater signal-strength tracker ([474c4cf](https://github.com/jagoda/meshcadet/commit/474c4cfc3f16fda4e6b927f358d320382aa0d99e))
* **firmware-core:** repeater signal-strength tracker + ADR-0010 ([9e046df](https://github.com/jagoda/meshcadet/commit/9e046dff0e483e0c28fdcf1819a3e121c7b47cbf))
* **firmware:** wire the repeater signal meter into the rx path and UI ([4001c2c](https://github.com/jagoda/meshcadet/commit/4001c2c2e7ac95898551a24bd16e790fc9b806f9))
* **firmware:** wire the repeater signal meter into the rx path and UI ([35c39ec](https://github.com/jagoda/meshcadet/commit/35c39ecebe694c451097ec9519b9cef693429ab1))
* **release:** publish an app-only update artifact + layout compatibility gate ([75b45e2](https://github.com/jagoda/meshcadet/commit/75b45e2d6b6b3fcd35f9ced11eeb4aba6a2e2230))
* **release:** publish non-destructive app-only update artifacts ([2b585b3](https://github.com/jagoda/meshcadet/commit/2b585b31392a0f1ad582e0283503e163527eb4df))
* **site:** add a Getting Started section to the landing page ([62f6c57](https://github.com/jagoda/meshcadet/commit/62f6c57d9b217275e3dc2486b5908b9f1571b533))
* **site:** add a Getting Started section to the landing page ([b4fc37e](https://github.com/jagoda/meshcadet/commit/b4fc37ee19075dc63347a5435845492bd328762d))
* **site:** add promotional UI screenshots to the landing page ([b8f5bb9](https://github.com/jagoda/meshcadet/commit/b8f5bb95a4722a1c69c73ee094fc10ea311cc532))
* **site:** harden the Upgrade path per post-green + hardware-safety review ([ea3c972](https://github.com/jagoda/meshcadet/commit/ea3c97264564454d297e3dcdff52d84faa5939ff))
* **site:** two-path web flasher — Fresh install vs non-destructive Upgrade ([f74ba05](https://github.com/jagoda/meshcadet/commit/f74ba05db2240d4da109d6db52d2af9a9f4e6d19))
* **site:** two-path web flasher — Fresh install vs non-destructive Upgrade ([a6c1672](https://github.com/jagoda/meshcadet/commit/a6c16722463b9c90ba32aae8f1f1801beedbc5df))
* **site:** wire the four promo screenshots into the landing page gallery ([0cf3c0c](https://github.com/jagoda/meshcadet/commit/0cf3c0cba2485d68892e5f2469280cb28620aaee))
* **ui_sim:** add promo screenshot render rigs for four production screens ([91cbd16](https://github.com/jagoda/meshcadet/commit/91cbd16353a4b377fd72eca030e872281ac5860d))


### Fixed

* **firmware-core:** silence clippy on decay boundary test ([3876f6f](https://github.com/jagoda/meshcadet/commit/3876f6f3f78a748f0568ad0b670b56269acbf480))
* **release:** stop crashing the lockfile-sync commit step on every tag-only run ([1cc3c8c](https://github.com/jagoda/meshcadet/commit/1cc3c8c78011c1e13e6fbb49e5789a9946feae17))
* **release:** stop crashing the lockfile-sync commit step on every tag-only run ([cd63409](https://github.com/jagoda/meshcadet/commit/cd63409293f74b7c14c4e0aeb620ae1fd09174c1))
* **ui_sim:** sync promo screen markup with the merged signal-meter widget ([20c4493](https://github.com/jagoda/meshcadet/commit/20c44933e6d7aacdb32075f6eb31c5dab4058a0a))
* **ui:** move contact/channel-list signal meter to the right of the gear ([71d31c6](https://github.com/jagoda/meshcadet/commit/71d31c6e6b93e82d82a45c4d793b820a9dc171e8))
* **ui:** move contact/channel-list signal meter to the right of the gear ([7598a44](https://github.com/jagoda/meshcadet/commit/7598a445793fb6c9e5e7e3d20e2ae0eb40aa8013))


### Changed

* **release:** extract the layout-compatibility gate into a tested script ([e9d1053](https://github.com/jagoda/meshcadet/commit/e9d105384aebc83c3d0c569ed2cdb91f9b157d52))
* **site:** reorder landing page and unify navigation ([a2856fa](https://github.com/jagoda/meshcadet/commit/a2856faf5f0c00e81d1863e70fe22c43f79e685f))
* **site:** reorder landing page and unify navigation ([75df9b1](https://github.com/jagoda/meshcadet/commit/75df9b1a0f8a23d3c1abcfd58789d527de952449))


### Documentation

* **adr:** add ADR-0008 for non-destructive update artifacts ([5603611](https://github.com/jagoda/meshcadet/commit/5603611a9290463930b2dfbbe85b68f2818f3080))
* **adr:** add ADR-0010 for the repeater signal meter design ([399729a](https://github.com/jagoda/meshcadet/commit/399729aec113dcc278a0ceebad29e44e4e9c0715))
* **adr:** note the extracted gate script + verified site-mirror compatibility ([cec144a](https://github.com/jagoda/meshcadet/commit/cec144a6ee698a55bab9556b302d41195b17d68d))

## [0.2.0](https://github.com/jagoda/meshcadet/compare/v0.1.0...v0.2.0) (2026-07-13)


### Added

* **release:** add sync-cargo-lock-versions.sh + smoke test ([e386cb4](https://github.com/jagoda/meshcadet/commit/e386cb42a16ac2f0a67f0ba1f81fcea5d3869df7))


### Fixed

* **release:** post-green hardening for the Cargo.lock sync script ([df9f7e9](https://github.com/jagoda/meshcadet/commit/df9f7e9a184b317bc3fe703ffe9da00b7dcdd4c6))
* **release:** replace release-plz with release-please ([2cd1802](https://github.com/jagoda/meshcadet/commit/2cd1802bd8dc0a00998e22d49b981309be3750b1))
* **release:** replace release-plz with release-please ([9bdc928](https://github.com/jagoda/meshcadet/commit/9bdc92862bd59a5821e5fc0345b2d3c4c0d33872))
* **release:** sign the Cargo.lock sync commit via the GitHub API ([5302d79](https://github.com/jagoda/meshcadet/commit/5302d79739e5a400ebd4cb41c66c2039f8b3c73a))
* **release:** sign the Cargo.lock sync commit via the GitHub API ([c9a3f04](https://github.com/jagoda/meshcadet/commit/c9a3f04f0c582271cfd87fe6143e348ccbfa64c3))
* **release:** sync Cargo.lock/firmware/Cargo.lock on every release PR ([ecd1f18](https://github.com/jagoda/meshcadet/commit/ecd1f18410cbcad72d466dfbd4be0d8d9a49f77e))


### Documentation

* **release:** correct ADR-0004 §5's squash-merge premise ([35ab4ea](https://github.com/jagoda/meshcadet/commit/35ab4eafd0146ab54f98f1bf3d1f64cde08ad3f4))

## [0.1.0] - 2026-07-12

### MeshCadet

- Mesh-radio messaging firmware for the LilyGO T-Deck Plus


The first public release of MeshCadet: a deliberately-limited, MeshCore-interop
firmware for the LilyGo T-Deck Plus. Its limits are design choices for a
controlled, minimal comms device — MeshCadet is provided "as is" with no
warranty and no guarantee of safety or security; see the Disclaimer in
[`README.md`](README.md) and [`SECURITY.md`](SECURITY.md).

### Added

- **Protocol interop (`protocol/`)**: byte-exact Rust port of the MeshCore
  v1.15.0 wire protocol — packet framing, Ed25519/X25519 identity and ECDH,
  AES-128-ECB + HMAC-SHA256 DM/channel encryption, ACK codec, and routing.
- **Firmware (`firmware/`)**: ESP32-S3 device app for the T-Deck Plus —
  LoRa radio (SX1262) send/receive, GPS-backed pull-only telemetry, a
  touch-screen UI (Slint) for contacts/conversations/composing with a
  curated emoji set, on-device history storage, and a PIN-gated admin menu.
- **Allowlist policy layer**: allowlist-only contacts and channels, no
  device-initiated advertising, silent drop of all non-allowlisted traffic,
  pull-only (never push) location telemetry.
- **Admin host CLI (`host/`)**: USB-serial provisioning tool (`meshcadet`)
  for registering contacts/channels, setting notification defaults and a
  PIN, exporting history, and resetting a forgotten PIN.
- **Development tooling**: `xtask` (host-side glyph-coverage verification for
  the emoji/icon font pipeline), `ui_sim` (host-native Slint render rig for
  UI/asset verification without hardware), `ui_perf` (host-native UI
  performance measurement harness).
- Design record in `docs/adr/` (protocol/policy charter, provisioning
  wire format, UI toolkit choice) and a manual hardware verification
  checklist in `docs/hil-real-mesh-procedure.md`.
- GPLv3 licensing, upstream attribution (`NOTICE`), and a full third-party
  dependency license audit (`docs/licensing/`).

### Known limitations

See [`SECURITY.md`](SECURITY.md) and the README's
["Status and known limitations"](README.md#status-and-known-limitations)
section — notably: no at-rest encryption of provisioned data, no PIN
attempt lockout, and inherited AES-128-ECB from the MeshCore wire protocol.
