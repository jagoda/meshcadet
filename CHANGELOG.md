# Changelog

All notable changes to MeshCadet are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning is managed by [release-please](https://github.com/googleapis/release-please)
— see `release-please-config.json` and `docs/adr/0004-release-architecture.md`.
The entry below documents everything landed before release-please's first
`chore(release): vX.Y.Z` PR.

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
