# Changelog

All notable changes to MeshCadet are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project does not yet follow a formal version-numbering scheme (all
workspace crates are currently `0.0.0`); the entry below will be retitled with
a version number and date at the first tagged release.

## [Unreleased] — Initial public release

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
