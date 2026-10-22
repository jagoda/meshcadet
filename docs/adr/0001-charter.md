# ADR-0001 — MeshCadet Charter

- **Status:** Accepted (locked 2026-06-07); amended 2026-07-25 — §1 protocol
  target bumped from a v1.15.0-only lock to **MeshCore v1.16, with backward
  compatibility to v1.15 where possible**, and §2 extended to admit
  `role=room` provisioned contacts (MeshCore Room Server support). See
  "Amendment (2026-07-25)" below for the full record. **Qualified
  2026-08-01:** §2's room-amendment claim that the no-advert/no-discovery
  posture is "unchanged" for room contacts is corrected — the no-*discovery*
  half does not hold once a room contact is provisioned (identity pubkey
  leaks in the clear via room login); see the "Qualified 2026-08-01" note
  under §2 below. **Qualified 2026-08-22 (v1.17 compat recon):** §1's
  protocol target is current for MeshCore **v1.16/v1.17** — v1.17.1 was
  verified wire-unchanged from v1.16.0 (`FIRMWARE_VER_CODE` still 13,
  `MAX_PACKET_PAYLOAD` 184, `MAX_PATH_SIZE` 64, `MAX_ADVERT_DATA_SIZE` 32,
  `CIPHER_*` all identical) by `meshcadet-meshcore-117-compat-recon-20260822-145129066`;
  no codec/wire change was needed. See "Amendment (2026-07-25)" §a for the
  v1.17 note. **Amended 2026-10-22 (v1.18.0 currency review):** §1's protocol
  target is now MeshCore **v1.18.0** (released 2026-10-15). Its on-air
  framing, header, wire constants, payload/route-type values, advert format,
  and ACK hash contract are byte-identical to v1.17.1, so no codec or wire
  change was needed; the review also withdrew the 2026-08-22 note's
  `FIRMWARE_VER_CODE` evidence (that constant versions the companion
  host-link frames, not the on-air protocol). See "Amendment (2026-10-22)"
  below.
- **Deciders:** Project maintainer design conversation
- **Supersedes:** —
- **Canonical source:** This ADR is the authoritative in-repo record of the
  project's founding design decision. Where any other note or draft ever
  diverges from it, the design intent recorded here is authoritative for the
  codebase.
- **Protocol reference:** Reverse-engineered from the MeshCore firmware
  source, byte-exact against MeshCore `dee3e26a` / v1.15.0; target bumped
  2026-07-25 to v1.16 with v1.15 back-compat (see §1 and "Amendment
  (2026-07-25)"); v1.17.1 verified wire-unchanged 2026-08-22, and v1.18.0
  (released 2026-10-15) verified on-air-unchanged 2026-10-22 — the current
  target (see "Amendment (2026-10-22)").

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

- **Protocol target:** MeshCore **v1.18.0** (released 2026-10-15), with
  backward compatibility to v1.15 where possible. v1.18.0 is byte-identical
  to v1.17.1 in framing, header, `MAX_PACKET_PAYLOAD` 184,
  `MAX_ADVERT_DATA_SIZE` 32, `MAX_PATH_SIZE` 64, `PATH_HASH_SIZE`, the
  `CIPHER_*` sizes, every `PayloadType`/`RouteType` value, the advert format
  and signature scope, and the ACK hash contract; it adds only the
  `TXT_TYPE_CLI_COMMAND` = 3 text subtype (with `TXT_TYPE_CLI_DATA` = 1
  re-documented as "CLI command or reply"), which reuses the existing TXT_MSG
  frame encoding (amended 2026-10-22; see "Amendment (2026-10-22)"). The
  target was last bumped 2026-07-25 (v1.16), superseding the original
  v1.15.0-dee3e26-only lock; v1.17.1 and v1.18.0 are currency reviews, not
  breaking wire changes.
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
  - **ACK = 4-byte** truncated SHA256, emitted identically for v1.15 and
    v1.16 peers, and this stays permanently valid rather than being a
    version-specific shim to revisit (amended 2026-07-25 — see "Amendment
    (2026-07-25)" for the mechanism: v1.16's wire ACK widened to 6 bytes, but
    `ack_hash[0..4]` is bit-identical to v1.15's, and no receiver on either
    version reads past byte 4).
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
- **Provisioned-contact `role` (amended 2026-07-25 — see "Amendment
  (2026-07-25)" below):** a provisioned contact carries an optional `role`,
  with `role=room` designating a MeshCore **Room Server**. This does not
  weaken the allowlist model above — a room contact is provisioned the same
  out-of-band way as any other contact — but adds distinct invariants:
  - **Out-of-band acquisition only.** A room contact's pubkey, name, and
    guest password are admin-provisioned over the USB provisioning link,
    exactly like any other contact. **Never advert-acquired** — MeshCadet
    still never sends a MeshCore `ADVERT` packet, room or no room.
    **Qualified 2026-08-01 (outsider-boundary security review):**
    the no-*discovery* half of this claim does NOT hold once a room contact
    is provisioned. `encode_anon_req_login`
    (`protocol/src/room.rs:174`) must carry the caller's full 32-byte
    Ed25519 identity pubkey in the clear, outside the ciphertext — a room
    login has no prior session for the server to address the client by
    hash, so the protocol has no other way to identify the sender. That
    login frame is flood-routed mesh-wide at boot and on every reflood
    tick, so any node sniffing the mesh can read the device's identity
    pubkey without holding the room's guest password or being on the
    device's own allowlist. This is protocol-mandated (MeshCore's
    `ANON_REQ` framing) and not fixable without breaking room-login
    interop. The **addable** half of the invariant is unaffected — leaking
    the identity pubkey does not let a stranger get added as a contact; the
    allowlist gate (§2 above) is untouched. See `SECURITY.md`, "Known
    limitations", for the user-facing writeup.
  - **Client-only.** MeshCadet joins, reads, and posts to a room at
    guest-password level. It never *is* a room server: it exposes no ACL or
    admin surface, and sends no `TXT_TYPE_CLI_DATA`.
  - **USB-only password transport.** The guest password reaches the device
    over USB only, under the same physical-possession authentication as
    every other provisioning value (§4) — no transport crypto is added to
    the provisioning link on account of this.
  - **Zero allowlist policy in the protocol layer, unchanged.** Room login,
    push, post, and keep-alive codecs live in `protocol/`, exactly like every
    other wire codec; the *decision* to talk to a given room remains
    firmware-side allowlist policy, same as any other contact. This ADR's
    existing "protocol carries zero allowlist policy" invariant
    (Consequences, below) is not weakened by this addition.

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
  (+telemetry flag, +optional `role` — e.g. `role=room` with its guest
  password, §2), channels (+primary), radio preset, notification defaults,
  PIN, locks; exports history.
- **PIN-gated on-device admin menu** for lightweight runtime toggles (no laptop
  needed).
- **PIN recovery** via the USB host tool (physical possession resets it).
- **Firmware update:** USB flash (esptool/DFU) — documented in the project.

### 5. Workspace shape (mirrors the gimbal split)

- **`protocol/`** — shared MeshCore wire port (framing, routing, crypto, ACK,
  codec), byte-exact against v1.15.0 and wire-compatible through v1.18.0 (§1);
  used by *both* firmware and host so the CLI can encode config and decode
  exported history. Host-native + `no_std`-capable; testable on stable.
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
- **Superseded 2026-07-25:** targeting v1.16 does not need its own ADR after
  all — the ACK-size widening is confirmed non-breaking (§1; "Amendment
  (2026-07-25)" below), so the v1.16 bump is recorded as an amendment to this
  charter instead. The one open item the bump does NOT resolve is the radio
  preamble: v1.16's widened default preamble length (32 symbols at SF7,
  vs. v1.15's inherited default of 8) against MeshCadet's still-8-symbol
  `PREAMBLE_LEN` (`firmware/src/radio.rs`) has no on-air bench measurement
  yet — treat v1.16 interop as protocol-verified, not radio-PHY-verified.
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

## Amendment (2026-07-25) — protocol-target bump and Room Server support

Two changes, both settled by maintainer ruling on this date.

### a. Protocol target: v1.15.0-dee3e26 → v1.16, with v1.15 back-compat

§1's original lock and this ADR's original Consequences rationale ("Building
against `dee3e26` (not v1.16 HEAD) is deliberate: ACK is 4 bytes. A toolchain
or source bump to v1.16 is a breaking wire change and needs its own ADR.")
are **superseded**, not merely contradicted: both have been rewritten in
place above. The reason the bump is not breaking, landed and tested in
`meshcadet-meshcore-1-16-interop` (do not re-derive it):

> v1.16's ACK is 6 bytes but `ack_hash[0..4]` is the **identical** SHA-256 as
> v1.15. Bytes 4–5 are an extended-attempt byte and a random byte, existing
> only to perturb the packet hash after `SimpleMeshTables.h` moved ACKs onto
> packet-hash dedup. No receiver on either version reads past byte 4; both
> gate on `payload_len >= 4` (a minimum, not an equality). 4-byte emission is
> therefore permanently valid, and **no version detection or negotiation is
> required.**

MeshCadet emits 4-byte ACKs (unchanged) and now accepts + prefix-matches
inbound 6-byte ACKs on both parse paths (bare `Ack` frame and the PATH-return
bundle) — no build flag, no runtime negotiation.

**What this amendment does NOT claim:** the preamble question
(`firmware/src/radio.rs`'s `PREAMBLE_LEN`) remains open. v1.16 changed its
own default LoRa preamble length (`preambleLengthForSF(sf)`, 32 symbols at
MeshCadet's locked SF7) versus v1.15's inherited RadioLib default of 8;
whether 8 and 32 are mutually receivable on air is a physical-layer question
no amount of source inspection answers. `PREAMBLE_LEN` stays at 8 pending an
on-air bench measurement against stock v1.15 *and* v1.16 nodes — this
amendment upgrades the protocol *target*, not the bench-verification status
of the radio PHY.

**Qualified 2026-08-22 (v1.17 compat recon,
`meshcadet-meshcore-117-compat-recon-20260822-145129066`):** protocol target
extended to v1.16/v1.17 — v1.17.1 is byte-exact wire-compatible with v1.16.0
(every `src/MeshCore.h` constant identical, `FIRMWARE_VER_CODE` still 13; the
851 host tests in `protocol`/`firmware-core` are green and already pin the
on-air DM/ACK/GRP_TXT/advert frames), so this is a currency note, not a new
amendment to §1's decision. The preamble question above is unaffected and
does **not** narrow: v1.17 does not touch `preambleLengthForSF(sf)` either —
still `sf <= 8 ? 32 : 16` — so the open 8-vs-32 on-air bench question carries
forward unchanged against v1.17 nodes too. This note does **not** claim any
bench confirmation; the predicate remains maintainer-run (Houston mission
`meshcadet-hil-preamble-measurement` cancelled by maintainer ruling
2026-07-15).

**Corrected 2026-10-22 (v1.18.0 currency review):** the `FIRMWARE_VER_CODE`
figure in the 2026-08-22 note above is withdrawn as evidence. That constant is
the companion host-link frame version a node sends to an attached app over
USB/BLE (13 in v1.17.x, 14 in v1.18.0); MeshCadet implements the on-air
protocol and no companion protocol, so the constant says nothing about its
conformance. The review appended below re-bases the currency claim on the
on-air wire itself.

### b. Room-server-as-provisioned-contact

§2 (Policy / allowlist layer) now admits a `role` on provisioned contacts,
with `role=room` designating a MeshCore Room Server, per the invariants
written into §2 above:

- Room servers are acquired **out-of-band only** — admin-provisioned over
  the USB provisioning link, carrying pubkey + name + guest password. Never
  advert-acquired; the no-advert posture in §2 is unchanged. The
  no-*discovery* posture, however, does NOT survive room provisioning
  unqualified — see the "Qualified 2026-08-01" note under §2 above:
  `encode_anon_req_login` leaks the device's own identity pubkey in the
  clear, mesh-wide, on every room login/reflood.
- MeshCadet is a room **client** only: join, read, post at guest-password
  level. It never *is* a room server, exposes no ACL or admin surface, and
  sends no `TXT_TYPE_CLI_DATA`.
- The guest password reaches the device over USB only. Physical cable
  possession remains the authentication (§4) — no transport crypto is added
  to the provisioning link.
- The protocol layer still carries **zero** allowlist policy (Consequences,
  above). Room login, push, post, and keep-alive codecs live in `protocol/`;
  the *decision* to talk to a given room is firmware-side allowlist policy,
  same as every other contact. This is not weakened by admitting `role`.

The room URI/QR provisioning format itself is out of scope for this
amendment — that decision belongs to its own ADR.

## Amendment (2026-10-22) — MeshCore v1.18.0 currency review

MeshCore v1.18.0 was released 2026-10-15 (`companion-v1.18.0`,
`81dbae59`). §1's protocol target is now v1.18.0. This is a currency review,
not a change to the charter's interop decision: no codec or wire change was
needed.

Checked against the `companion-v1.17.1` (`d9296435`) and `companion-v1.18.0`
(`81dbae59`) source trees:

- **Byte-identical:** packet framing and the 1-byte header (`src/Packet.h`
  is unchanged in full, including every `PAYLOAD_TYPE_*` and `ROUTE_TYPE_*`
  value and the `PH_*` masks), `MAX_PACKET_PAYLOAD` 184,
  `MAX_ADVERT_DATA_SIZE` 32, `MAX_PATH_SIZE` 64, `PATH_HASH_SIZE`,
  `CIPHER_KEY_SIZE`/`CIPHER_BLOCK_SIZE`/`CIPHER_MAC_SIZE` (16/16/2), the
  advert format and signature scope (no change in the `createAdvert` path),
  and the ACK hash contract (the same SHA-256 over timestamp, text-type byte,
  text and sender pubkey, truncated to 4 bytes; only a local variable rename
  differs).
- **New on-air value:** `TXT_TYPE_CLI_COMMAND` = 3
  (`src/helpers/TxtDataHelpers.h`), and `TXT_TYPE_CLI_DATA` = 1 is
  re-documented as "a CLI command -or- reply". Both reuse the existing
  `TXT_MSG` frame encoding — the text-type byte already existed in the
  plaintext — so this is an added subtype value, not a frame-format change.
  v1.18.0 also moves the CLI-command reply path onto that encoding and
  removes the PATH return that inbound `TXT_TYPE_CLI_DATA` used to trigger;
  MeshCadet consumes neither.
- **Hardening, already complied with:** `Utils::MACThenDecrypt` now rejects
  ciphertext whose length is not a multiple of `CIPHER_BLOCK_SIZE` before
  MAC-checking. MeshCadet's encrypt path always emits whole AES blocks, so
  no change is needed.
- **Not on-air, not evidence of conformance:** `FIRMWARE_VER_CODE` moved 13 →
  14; it is the companion host-link frame version, only sent from firmware to
  an attached app over USB/BLE, and MeshCadet implements no companion
  protocol. The room/repeater/sensor on-air login-response byte 12
  (`FIRMWARE_VER_LEVEL`) is 1/2/1 in both releases, unchanged. Direct
  multi-ACK spacing changed from cumulative to base + 300 ms × n; MeshCadet
  emits flood ACKs only. The `preambleLengthForSF(sf) = sf <= 8 ? 32 : 16`
  rule is identical in both trees, so the open 8-vs-32 on-air preamble
  question in the 2026-07-25 amendment is neither resolved nor widened by
  this release.

Outcome: the port stays valid against v1.18.0 on the air; no wire or codec
change was required, and the open radio-PHY preamble question carries forward
unchanged.
