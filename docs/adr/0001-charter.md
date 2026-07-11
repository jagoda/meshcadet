# ADR-0001 — MeshCadet Charter

- **Status:** Accepted (locked 2026-06-07)
- **Deciders:** Project maintainer design conversation
- **Supersedes:** —
- **Canonical source:** This ADR is the authoritative in-repo record of the
  project's founding design decision. Where any other note or draft ever
  diverges from it, the design intent recorded here is authoritative for the
  codebase.
- **Protocol reference:** Reverse-engineered from the MeshCore firmware
  source, byte-exact against MeshCore `dee3e26a` / v1.15.0.

## Context

MeshCadet is a polished, deliberately-limited **MeshCore-interop firmware** for
the **LilyGo T-Deck Plus**, written in **Rust** (esp-idf / std,
ESP32-S3 / Xtensa), cross-compiled for the device. Interop with the admin's
existing MeshCore network is a **hard requirement** — the firmware is a fully
compliant MeshCore citizen on the air; **all limiting behaviour lives in a
policy + UI layer on top of a byte-exact-compliant protocol.**

This ADR is the anti-amnesia anchor: every decision below was made deliberately
in the design conversation and must survive across future work sessions.

## Decision

### 1. Interop (hard requirement — must match the deployed mesh byte-exact)

- **Protocol target:** MeshCore **v1.15.0-dee3e26**.
- **Radio preset:** freq **910.525 MHz**, bandwidth **62.5 kHz**, spreading
  factor **7**, coding rate **5 (4/5)**, **2-byte path hashes** (confirmed
  against a live device; `path_len` bits[7:6] = `0b01`).
- **TX power:** **+22 dBm** (SX1262 max), matching the mesh.
- **Crypto/identity (confirmed by recon against v1.15 source):**
  - Identity = **Ed25519** keypair (32 B public = node identity, 64 B private).
  - On-device keypair generation; **the private key never leaves the device.**
  - DM encryption: **ECDH** via Ed25519→X25519 transposition →
    32-byte shared secret; **AES-128-ECB** keyed on the first 16 bytes.
  - Integrity: **Encrypt-then-MAC**, 2-byte truncated **HMAC-SHA256** over the
    ciphertext keyed on the full 32-byte shared secret.
  - **ACK = 4-byte** truncated SHA256 (v1.15 behaviour — note v1.16 widened it
    to 6 bytes; build against `dee3e26`).
  - Channels: symmetric AES-128-ECB + 2-byte HMAC keyed on the shared channel
    secret; channel hash = `SHA256(channel_secret)[0]`.
  - **Discrepancy on record:** an early prose guide called this "AES-128 CBC";
    the v1.15 source (`Utils::encrypt()`) is **ECB** (no IV, no chaining). The
    codebase MUST implement ECB (confirmed against source during protocol
    analysis).

### 2. Policy / allowlist layer (firmware-side, on top of compliant protocol)

- **No public channels** — none supported at all.
- **No advertising** — the device never sends an explicit advert. (Return paths
  are still learned automatically from flooded data packets; recon §7.2.)
- **No auto-discovery / no auto-add** of contacts. Contacts and channels exist
  *only* if an admin provisioned them via the USB CLI.
- **Allowlist-only comms:** DMs accepted only from registered known contacts;
  everything else (DMs, telemetry requests) is **silently dropped** — no ACK,
  no presence leak.
- **DMs always ACK** — for known contacts only.
- **Telemetry (location) is pull-only**, answered only for contacts an admin
  explicitly enabled. Responses include a **fix age/timestamp** so staleness is
  visible.
- **Primary channel** is the default mode of communication; DMs supported for
  known contacts. Render provided contact names when available. Assume only
  family ever holds the channel keys.

### 3. Device behaviour

- **Standalone** in normal use — no phone/companion required to operate.
- **GPS always provides an available location** for telemetry: cached last-known
  fix, refreshed periodically (~2 min), power-conserving duty-cycle (instant-fix
  not required).
- **Emoji:** Slack-style `:shortcode:` entry/render over a **curated
  set** (not the full Unicode table). Text travels as UTF-8 on the
  wire (free); cost is rendering.
- **UI:** engaging, intuitive, simple; **touch-first** (T-Deck touch panel),
  icon/image-rich. Toolkit choice (Slint vs LVGL vs embedded-graphics) —
  **leaning Slint** — was decided during touch-UI evaluation (see
  [ADR-0003](0003-ui-toolkit.md)), not pre-locked here.
- **Notifications:** visual + audible, per-event configurable. Admin sets
  initial defaults at provisioning; the user may freely adjust their own.
- **History:** conversation history **persisted to internal flash**, **rotating**
  (oldest ages out; no huge retention), **exportable via the admin CLI**.
- **At-rest security:** **none** — a lost device is treated as low value; if
  compromised, rotate channels/keys. (No flash encryption / secure boot.)
- **First boot (unprovisioned):** "connect me to an admin over USB" screen; no
  comms until provisioned.

### 4. Admin configuration interface

- **USB-serial provisioning** via a **host CLI** (the `host/` crate). Physical
  possession = admin authority. Provisions identity readout, contacts
  (+telemetry flag), channels (+primary), radio preset, notification defaults,
  PIN, locks; exports history.
- **PIN-gated on-device admin menu** for lightweight runtime toggles (no laptop
  needed).
- **PIN recovery** via the USB host tool (physical possession resets it).
- **Firmware update:** USB flash (esptool/DFU) — documented in the project.

### 5. Workspace shape (mirrors the gimbal split)

- **`protocol/`** — shared MeshCore v1.15 port (framing, routing, crypto, ACK,
  codec); used by *both* firmware and host so the CLI can encode config and
  decode exported history. Host-native + `no_std`-capable; testable on stable.
- **`firmware/`** — esp-idf (std) device app: radio, GPS, touch UI, storage,
  admin menu. Cross-compiles for `xtensa-esp32s3-espidf` under the `esp`
  toolchain; a **detached** Cargo workspace so root `cargo test` stays native.
- **`host/`** — admin CLI: provisioning, history export, PIN reset over USB
  serial.

## Consequences

- The protocol layer carries **zero** allowlist policy — it must be able to do
  anything MeshCore can, byte-for-byte, or interop breaks. The allowlist policy
  is enforced one layer up. This separation is load-bearing and must not be
  eroded. Note that this policy layer is a **best-effort risk-reduction design,
  not a guarantee of any kind** — see the Disclaimer in
  `README.md` and `SECURITY.md`; nothing in this charter promises the policy is
  effective or cannot fail.
- Building against `dee3e26` (not v1.16 HEAD) is deliberate: ACK is 4 bytes. A
  toolchain or source bump to v1.16 is a breaking wire change and needs its own
  ADR.
- No at-rest crypto means a lost device leaks any provisioned channel keys; the
  mitigation is operational (rotate), accepted as low value.
- The detached-firmware-workspace shape means `cargo test` at the repo root
  covers `protocol` + `host` only; firmware is built/flashed from `firmware/`.

## Toolchain

| Concern | Choice |
|---|---|
| Host crates (`protocol`, `host`) | stable rustc (`rust-toolchain.toml` at root) |
| Firmware crate | `esp` channel via `espup` (`firmware/rust-toolchain.toml`) |
| Firmware target | `xtensa-esp32s3-espidf` (`firmware/.cargo/config.toml`) |
| ESP-IDF | std bindings (esp-idf-svc / -hal / -sys); `sdkconfig.defaults` at root |
| Flash/monitor | `espflash flash --monitor` (configured runner) |
