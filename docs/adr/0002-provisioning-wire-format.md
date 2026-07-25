# ADR-0002 — Provisioning Wire Format

- **Status:** Accepted (2026-06-13); amended 2026-06-15 (ADD_CHANNEL key_len + blob v0x02);
  amended 2026-07-03 (retired `SET_RADIO_PRESET`/`SET_LOCKS` — see §3 note);
  amended 2026-07-25 (room-server contacts: `ADD_ROOM`/`DEL_ROOM`/`QUERY_ROOMS`,
  blob v0x03, room URI — see §7)
- **Deciders:** Maintainer design review
- **Supersedes:** —
- **Implements:** ADR-0001 §4 (admin configuration interface via USB-serial)
- **Code:** `protocol/src/provisioning.rs` (shared codec), `firmware-core/src/config_store.rs`
  (blob codec + structs — see ADR-0005), `firmware/src/config_store.rs` (NVS I/O),
  `firmware/src/provisioning_server.rs`, `host/src/main.rs` (room URI),
  `site/provisioner/codec.js` (JS mirror)

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

Binary blob layout: version byte (current: `0x03`), counts, flags,
per-contact entries (67 bytes each — the `role` byte was added in v0x03,
§7), per-channel entries (67 bytes each — the `key_len` byte was added in
v0x02 to support 128-bit channel secrets), and (v0x03+) per-room-extras
entries (119 bytes each, §7). A blob with version `0x01` triggers
reprovisioning (safe: device is treated as unprovisioned); a `v0x02` blob
instead migrates losslessly forward to `v0x03` (§7 — the one exception to
"unrecognized version ⇒ reprovision"). Max blob size: 3 544 bytes (well
within the 24 KB NVS partition).

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

### 7. Room-server contacts (2026-07-25 amendment)

A room server is provisioned like a chat contact plus one extra secret (the
guest password) and some runtime-learned state (`sync_since`, `permissions`,
`out_path`). This amendment covers the storage layout, the new frame types,
and the room URI/QR decision. It does **not** cover the actual room-login
session logic, host-CLI verbs, or browser UI — those are later work; this
amendment lands the contract they build against.

**Storage: `role` byte + parallel room-extras list, not a separate contact
type.** `ProvisionedConfig::contacts` gained a `role: u8` field (`ROLE_CHAT =
0`, `ROLE_ROOM = 3` — the latter reused directly from MeshCore's own advert
node-type nibble, `0=none, 1=chat, 2=repeater, 3=room server, 4=sensor`, so
it can be passed straight through as a URI `type=` value with no
translation; `ROLE_CHAT` is `0`, NOT upstream's `1`, specifically so a
pre-this-amendment blob — which has no `role` byte at all — migrates every
existing contact to "chat" for free by zero-fill). A room's password,
`sync_since`, `permissions`, and `out_path` live in a separate
`room_extras` list (`firmware-core/src/config_store.rs::RoomExtra`), keyed by
pubkey rather than positional index, so the existing `DEL_CONTACT`
compaction-shift can never desync it from its owning contact. Keeping rooms
in the one contacts store (rather than a wholly separate table) makes a
Contacts-tab filter and a Groups-tab union each a one-field predicate on
`role`.

**Blob version `0x02` → `0x03`.** New header byte `room_count`; contact
entries grow from 66 to 67 bytes (`+role`); a new room-extras section
(119 bytes/entry) follows the channel section. `MAX_BLOB_LEN` is rebudgeted
to `32 + 16×67 + 8×67 + 16×119` = **3544 bytes** (room count is
capacity-bounded by `MAX_CONTACTS`, since every room is also a contact —
there is no separate `MAX_ROOMS`), still well within the 24 KB NVS
partition. **The migration is mandatory and lossless**: a `v0x02` blob loads
under `v0x03` with every contact and channel intact, every contact's `role`
defaulted to `ROLE_CHAT`, and zero rooms — a device in the field is never
bricked, wiped, or silently reset to unprovisioned by this firmware upgrade.
The very next config save persists it forward as a full `v0x03` blob. See
`firmware-core/src/config_store.rs`'s module doc for the byte-level layout
and `v2_blob_migrates_losslessly_*`/`v2_blob_then_resave_upgrades_to_v3_*`
for the executable proof.

**New frame types**, numbered to continue the existing per-entity add/del/query
sequences rather than reusing `ADD_CONTACT`/`RSP_CONTACT` (a room's
provisioning-time payload — a guest password — has no chat-contact
equivalent):

| Frame type | Direction | Key payload fields |
|------------|-----------|--------------------|
| `QUERY_ROOMS (0x05)` | host→device | (empty) |
| `ADD_ROOM (0x22)` | host→device | `pubkey(32) \| guest_password_len(1) \| guest_password(N) \| name_len(1) \| name(M)` |
| `DEL_ROOM (0x23)` | host→device | `pubkey(32)` |
| `RSP_ROOM (0x8B)` | device→host | `index(1) \| pubkey(32) \| sync_since(4 LE) \| permissions(1) \| out_path_len(1) \| out_path(P) \| name_len(1) \| name(M)` |
| `RSP_ROOMS_DONE (0x8C)` | device→host | (empty) |

`RSP_ROOM` deliberately does **not** echo the guest password back — same
precedent as `RSP_CHANNEL` not echoing the channel secret. This keeps "the
password appears in no log, echo, or error line" a mechanical property of
the wire format rather than a call-site discipline the firmware has to get
right every time. The guest password crosses the USB link in the clear —
that is correct and intentional per §4/ADR-0001 §4: the cable is the
authentication. No transport crypto is added for this frame family either.

**Room URI/QR decision: reuse `type=3` as-is, carry the guest password
out-of-band.** A room server is upstream MeshCore node type 3
(`meshcore-dev/MeshCore` docs/faq.md §7.5 documents
`meshcore://contact/add?name=<n>&public_key=<hex>&type=<type>` with
`type`: chat=1, repeater=2, room=3, sensor=4) — but that scheme has **no
slot for a password at any `type` value**. Two options were weighed:

1. Extend the URI with a non-standard `password=` parameter.
2. Reuse `type=3` unchanged and communicate the guest password out-of-band.

**Compatibility investigation.** The FAQ text itself is silent on how the
companion app's query-string parser handles an unrecognized parameter
(ignored vs. hard-fail). A search for a public source repository for the
companion app's URI/deep-link parser (to inspect its behavior directly,
per this amendment's own requirement not to assert it) turned up nothing
under the `meshcore-dev` GitHub org — the mobile companion app does not
appear to have public source to read. Absent that, option 1's actual
compatibility cost is **unknown and unverifiable from here**: a
`password=` param might be silently ignored by every companion-app version
(fine) or hard-fail the parse on some version, present or future (breaks
the QR scan entirely, an unacceptable failure mode for a physical-QR
handoff with no error channel back to the admin).

**Decision: option 2.** The room URI is byte-identical in shape to the
`type=1` contact URI, only `type=3` differs:

```
meshcore://contact/add?name=<name>&public_key=<hex>&type=3
```

This has **zero compatibility risk** — it is not a deviation from the
upstream scheme at all, just the value upstream already reserves for this
exact case. The cost is UX, not compatibility: the guest password must be
communicated by some channel other than the QR itself (spoken, written
alongside, printed separately by the host CLI) and entered by whichever
later UI implements the actual room-login flow. Given the room-login
session itself is already a separate step from the identity/contact-add
scan (a `RESPONSE`/`ANON_REQ` round trip over the mesh, not something a
QR scan alone can complete), this costs no additional step in practice — it
only means the password isn't riding along in the same QR payload.

`host/src/main.rs::build_room_add_uri` builds this URI;
`host/src/main.rs::parse_contact_uri` is the round-trip parser proving it
(and the existing `type=1` contact URI, unchanged byte-for-byte) decode
back to their exact inputs — see that file's `room_uri_golden_string`,
`contact_uri_byte_output_unchanged_by_room_uri_addition`, and
`*_round_trips_through_the_host_cli_parser` tests.

**JS codec mirror.** `site/provisioner/codec.js` gained
`encodeAddRoom`/`encodeDelRoom`/`decodeRspRoom` and the new frame-type
constants, extended via the same golden-vector conformance mechanism as
every other frame (`xtask`'s `gen-prov-golden-vectors` +
`codec.conformance.test.mjs`) — see `docs/adr/0007-provisioner-codec.md`.

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
- The format is **versioned** (current: blob `v0x03`, adding room-server
  contacts; was `v0x02` before that, and `v0x01` before `key_len` was added
  to channels).  A version bump triggers a reprovisioning request rather
  than a silent mismatch, EXCEPT `v0x02` → `v0x03`, which is a mandatory
  lossless migration (§7) rather than a reprovisioning request — the first
  version bump in this ADR's history that a device in the field must not be
  reprovisioned over.
- The CRC algorithm is **not authenticated** — eavesdroppers on the USB bus can
  inject frames.  Accepted risk under the physical-possession security model.
- The **primary-channel flag** is enforced to be unique: `ADD_CHANNEL` with
  `primary=true` clears the flag on any existing primary channel.
