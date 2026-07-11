# ADR-0002 — Provisioning Wire Format

- **Status:** Accepted (2026-06-13); amended 2026-06-15 (ADD_CHANNEL key_len + blob v0x02);
  amended 2026-07-03 (retired `SET_RADIO_PRESET`/`SET_LOCKS` — see §3 note)
- **Deciders:** Maintainer design review
- **Supersedes:** —
- **Implements:** ADR-0001 §4 (admin configuration interface via USB-serial)
- **Code:** `protocol/src/provisioning.rs` (shared codec), `firmware/src/config_store.rs`,
  `firmware/src/provisioning_server.rs`

## Context

MeshCadet requires a USB-serial provisioning channel so an admin can configure
a device before it joins the mesh (ADR-0001 §4: "Physical possession =
admin authority").  The provisioning data includes:

- **Identity readout** — device's Ed25519 pubkey (read-only; generated on-device)
- **Contacts** — peer pubkey, per-contact telemetry-enable flag, display name
- **Channels** — symmetric 32-byte channel secret, primary flag, channel name
- **Radio preset** — frequency, bandwidth, spreading factor, coding rate, TX power
- **Notification defaults** — visual, audible
- **PIN** — admin lock code for on-device settings
- **Locks** — which on-device settings are locked from routine on-device use

The shared codec must be `no_std`-compatible (for firmware) and `std`-compatible
(for the host CLI); all message types must roundtrip without heap allocation.

## Decision

### 1. Binary framing: length-prefixed with CRC-16

```
byte 0-1  MAGIC        = 0x4D 0x43  ("MC")
byte 2    frame_type   = u8 constant
byte 3-4  payload_len  = u16 little-endian
byte 5..  payload      = payload_len bytes
last 2    crc16        = CRC-16/ARC over bytes [0 .. 5 + payload_len]
```

**Why binary, not JSON / text?**  The firmware is `no_std` (at the codec layer),
and constructing or parsing JSON without heap allocation requires either a
heavyweight library or hand-rolled unsafe code.  A compact binary format with
fixed-size structs is simpler and faster to decode on an embedded target.

**Why length-prefixed, not COBS?**  COBS requires either heap allocation or a
known-maximum-frame size.  Length-prefixed framing with a 2-byte length field
is self-delimiting, simple to implement, and handles any payload size up to
65 535 bytes (in practice all provisioning messages are < 100 bytes).

**Why CRC-16/ARC?**  Simple to implement in `no_std` (no external crate, 12
lines), adequate for a reliable USB-serial link, and its known-answer value
(`0xBB3D` for `"123456789"`) makes it easy to verify in tests.  The CRC is an
integrity check against accidental corruption; it is NOT a MAC — the cable is
the authentication (physical possession model).

**Why `"MC"` as magic bytes?** Two distinct, non-ASCII bytes that do not appear
in the ESP-IDF log output as a pair, providing a reliable sync marker when
ASCII log messages and binary frames share the same USB-JTAG byte stream.
`0x4D 0x43` = "MC" for "MeshCadet".

### 2. Frame-type allocation

| Range | Direction | Meaning |
|-------|-----------|---------|
| `0x01–0x7F` | Host → device | Commands |
| `0x80–0xFF` | Device → host | Responses |

The split gives 127 command codes and 128 response codes with room to extend
in future missions (history export, firmware OTA intent, etc.).

### 3. Payload encoding: fixed-width fields, no TLV

Each frame type has a fixed, documented wire layout:

| Frame type | Key payload fields |
|------------|--------------------|
| `ADD_CONTACT (0x10)` | `pubkey(32) \| telemetry(1) \| name_len(1) \| name(N)` |
| `ADD_CHANNEL (0x20)` | `secret(32) \| key_len(1) \| primary(1) \| name_len(1) \| name(N)` |
| `SET_NOTIF_DEFAULTS (0x40)` | `visual(1) \| audible(1)` |
| `SET_PIN (0x50)` | `pin_len(1) \| pin(N)` |
| `SET_DEVICE_NAME (0x51)` | `name_len(1) \| name(N)` |
| `COMMIT_PROVISIONING (0x70)` | (empty) |
| `CLEAR_HISTORY (0x72)` | (empty) |
| `RSP_STATUS (0x82)` | `provisioned(1) \| pubkey(32) \| contacts(1) \| channels(1) \| gps_has_fix(1) \| gps_lat_e7(4 LE) \| gps_lon_e7(4 LE) \| gps_fix_age_secs(4 LE) \| gps_clock_synced(1) \| gps_clock_sync_age_secs(4 LE) \| battery_percent(1) \| battery_charging(1) \| battery_raw_mv(2 LE) \| battery_held_raw_mv(2 LE)` |
| `RSP_IDENTITY (0x83)` | `pubkey(32) \| pub_hash(1) \| name_len(1) \| name(N)` |

No TLV wrapping is applied because every field is necessary in every call —
there are no optional sub-fields.  The `name_len` / `pin_len` prefix handles
the only variably-sized fields (names and PIN).

**2026-07-03 amendment — `SET_RADIO_PRESET (0x30)` and `SET_LOCKS (0x60)`
retired.** A host-command audit found both had
zero firmware consumer: `SET_RADIO_PRESET` persisted a value `Radio::init()`
never read (the radio preset is hardcoded to the §1 locked ADR-0001 value —
letting a host pick arbitrary RF parameters also contradicts that hard
interop requirement), and `SET_LOCKS`' `lock_flags` was stored in two places
but nothing anywhere branched on `LOCK_CONTACTS`/`LOCK_NOTIF_SETTINGS`/
`LOCK_RADIO_PRESET` to gate any behavior. Both frame types are retired (byte
values `0x30`/`0x60` reserved, not reused) rather than reassigned. `0x40`
(`SET_NOTIF_DEFAULTS`) was audited alongside them and kept: its destination
(`RuntimeSettings.notif_visual/audible` → the live notification dispatcher)
is real, only the first-boot seed from `ProvisionedConfig.notif_defaults` was
missing — fixed in the same pass rather than removed.

**2026-07-03 amendment — `SET_DEVICE_NAME (0x51)` added, `RSP_IDENTITY (0x83)`
gains a name field.** The `meshcadet identity --set-name` host command
persists a device display name. Unlike `SET_PIN`/`SET_NOTIF_DEFAULTS`/
`ADD_CONTACT`/etc., the name is NOT part of `ProvisionedConfig` (§5) — it is
stored in the identity NVS namespace (`mc_id`/`name`, alongside the Ed25519
seed) via `firmware/src/identity_store.rs`, because it is a property of the
node's identity rather than of the mesh contact/channel provisioning an admin
does once per device. Both `provisioning_server` (first-boot, unprovisioned)
and `admin_server` (post-commit, runtime) handle `SET_DEVICE_NAME`
identically: write-through to NVS immediately (no staging, no
`CommitProvisioning` gate) and reply `RSP_OK`/`RSP_ERROR`. `RSP_IDENTITY`
(previously `pubkey(32) | pub_hash(1)`, 33 bytes fixed) gained a
`name_len(1) | name(N)` suffix so `QUERY_STATUS` round-trips the persisted
name back to the host for readout/confirmation.

**2026-07-05 amendment — `RSP_STATUS (0x82)` gains `battery_raw_mv(2 LE)`.**
Diagnostic-only field added for a battery-ADC-calibration investigation: the
live, unfrozen post-divider ADC millivolt reading, distinct from
`battery_percent`'s charging-latch-frozen basis (see firmware `battery`
module docs). Payload grows 55→57 bytes; `decode_rsp_status` still accepts
the legacy 55-byte payload (defaults `battery_raw_mv` to `0`) so an
old-firmware/new-host or new-firmware/old-host pairing during a staged
rollout does not hard-fail — old `decode_rsp_status` builds simply never read
the trailing 2 bytes, and new builds handle their absence explicitly. Not
read by the on-device admin-menu screen or the over-the-air telemetry
RESPONSE — both stay scoped to `battery_percent`/`battery_charging` only,
per the 2026-07-03 "no raw voltage" scoping decision, which this field is a
deliberate, narrow (host-CLI-only), temporary exception to for diagnosis.

**2026-07-05 amendment — `RSP_STATUS (0x82)` gains `battery_held_raw_mv(2 LE)`.**
Follow-on to the `battery_raw_mv` amendment above:
because USB carries both the host CLI UART and charge power on this board,
any live read of `battery_raw_mv` is necessarily taken while the charger's
contaminated rail (~4.2-4.9 V, above the 4200 mV Li-ion ceiling) is on the
pin — the CLI can never show a clean battery voltage while a cable is
attached to read it. `battery_held_raw_mv` is the last known
non-charge-inflated ("resting") millivolt reading — the same frozen basis
`battery_percent` is derived from (see firmware `battery` module docs' fix
section), exposed as raw millivolts instead of a lossy-rounded percentage.
Reading it after a brief unplug/replug cycle (to re-attach the CLI) surfaces
the true pre-charge pack voltage. Payload grows 57→59 bytes;
`decode_rsp_status` accepts both the legacy 55-byte (pre-`battery_raw_mv`)
and 57-byte (pre-`battery_held_raw_mv`) payloads, defaulting each missing
trailing field to `0` for the same staged-rollout reason as the prior
amendment. Same scoping as `battery_raw_mv`: host-CLI-only, not read by the
on-device admin-menu screen or the telemetry RESPONSE.

### 4. Security model

Physical USB possession is the authentication factor; no transport encryption
is applied.  The CRC is solely for corruption detection.

This is consistent with ADR-0001 §4: "Physical possession = admin authority."
A future change may add an optional session key for remote provisioning over an
end-to-end secure channel, but that is out of scope here.

### 5. Flash persistence (firmware side)

The config is stored in the ESP-IDF NVS default partition under namespace
`mc_cfg`, as a single binary blob (`cfg_blob`) plus a provisioned flag (`prov`).
The flag is written last (after the blob) so a power-failure during commit
leaves the device unprovisioned rather than half-configured.

Binary blob layout: version byte (current: `0x02`), counts, flags,
per-contact entries (66 bytes each), per-channel entries (67 bytes each —
the `key_len` byte was added in v0x02 to support 128-bit channel secrets).
A blob with version `0x01` triggers reprovisioning (safe: device is treated as
unprovisioned).  Max blob size: 1 623 bytes (well within the 24 KB NVS
partition).

The device display name (§3 `SET_DEVICE_NAME` amendment) does NOT live in this
blob — it is a separate key (`name`) in the identity namespace (`mc_id`),
alongside the Ed25519 seed (`seed`), managed by `firmware/src/identity_store.rs`.
It applies and persists independently of `prov`/`cfg_blob` and survives a
reboot the same way the identity seed does.

### 6. First-boot gate

The provisioning check sits between identity load and radio initialisation in
`firmware/src/main.rs`.  An unprovisioned device:

1. Logs a prominent "UNPROVISIONED — connect to an admin over USB" banner.
2. Calls `provisioning_server::run()` which blocks reading USB-serial frames.
3. On `CommitProvisioning`: saves config to NVS, returns, triggers `esp_restart()`.
4. The radio is NEVER initialised during the unprovisioned state.

HIL builds (feature flag `hil`) bypass the gate and always proceed to radio
init (they are for hardware testing, not production deployment).

**2026-07-05 amendment — `CLEAR_HISTORY (0x72)` added.** The `meshcadet
clear-history` host command wipes ALL persisted conversation history (every
DM contact and channel, both inbound and outbound entries) from the
flash-backed `mc_hist` per-conversation store (`firmware/src/history_store.rs`).
Empty payload, single `RSP_OK`/`RSP_ERROR` ack — same shape as
`COMMIT_PROVISIONING`, not the streamed `EXPORT_HISTORY` pattern, since there
is nothing to enumerate back. Handled only by the runtime `admin_server`
(the only server holding the `HISTORY` mutex); the first-boot
`provisioning_server` has no history store yet to clear and falls through to
its existing unknown-frame-type error reply, same as every other
`admin_server`-only command (`EXPORT_HISTORY` included).

DESIGN DECISION — reboot-required MVP, not live in-memory clear: the flash
erase takes effect immediately, but `ui::UiRuntime`'s in-memory
`messages`/`unread` maps (owned by the main/UI thread) are left untouched by
this frame — `admin_server` runs on its own thread with no channel back into
UI state. A reboot re-hydrates the UI from the now-empty store via the
existing boot-hydrate path (`main.rs`, `HistoryStore::load_all_conversations`
→ `UiRuntime::seed_conversation`). This mirrors the pre-existing behavior of
every other runtime provisioning edit (`ADD_CONTACT`/`ADD_CHANNEL`/etc. also
only reach the live radio/UI state after a reboot); the host CLI's
`clear-history` output states the reboot requirement explicitly rather than
implying an instant on-screen clear.

## Alternatives Considered

### A. USB CDC/HID custom class
A dedicated USB device class (CDC-ACM for virtual serial, or custom HID) would
cleanly separate the provisioning channel from the ESP-IDF console.  Rejected:
ESP-IDF's USB-JTAG driver does not expose a separate CDC-ACM endpoint; adding
TinyUSB would require significant build system changes.  The shared
UART0/USB-JTAG stream is adequate given the binary framing.

### B. Bluetooth provisioning
BLE provisioning (like ESP-Provisioning / Unified Provisioning) would be wireless
and more user-friendly on mobile.  Rejected: it requires a BLE companion app,
adds complexity, and contradicts the "physical possession = admin authority"
model.  USB cable is intentional.

### C. JSON over serial (text protocol)
Human-readable, easy to debug with a terminal.  Rejected: too large for a
firmware `no_std` decoder without alloc; prone to encoding edge cases (escaping,
encoding names containing special characters).

## Consequences

- The protocol is **not human-readable** — a host-side CLI or Python script is
  required to send provisioning frames.  This is intentional: the host CLI
  provides the user-facing tool.
- The format is **versioned** (current: blob `v0x02`; was `v0x01` before
  `key_len` was added to channels).  A version bump triggers a reprovisioning
  request rather than a silent mismatch — safe failure mode.
- The CRC algorithm is **not authenticated** — eavesdroppers on the USB bus can
  inject frames.  Accepted risk under the physical-possession security model.
- The **primary-channel flag** is enforced to be unique: `ADD_CHANNEL` with
  `primary=true` clears the flag on any existing primary channel.
