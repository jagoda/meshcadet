# ADR-0015 — Region Scoping: Outbound-Only Transport-Code Scope, Provisioner/CLI-Only Config

- **Status:** Accepted (2026-10-22)
- **Deciders:** Maintainer design review
- **Supersedes:** —
- **Implements:** ADR-0001 §4 (physical USB possession = admin authority),
  ADR-0002 (provisioning wire format), ADR-0007 (web-provisioner codec +
  golden-vector CI guard)
- **Code:** `protocol/src/region.rs` (name validation, key derivation,
  transport codes, frame writers), `protocol/src/provisioning.rs`
  (`FRAME_QUERY_REGION`/`FRAME_SET_REGION`/`FRAME_RSP_REGION`),
  `protocol/src/crypto.rs` (`hmac_sha256_2_parts`), `host/src/session.rs` +
  `host/src/main.rs` (`set-region`, `status`), `site/provisioner/codec.js`,
  `site/provisioner/session.js`, `site/provisioner/validation.js`,
  `site/provisioner.js`, `site/provisioner.html`, `xtask/src/golden.rs`,
  `site/provisioner/codec.conformance.test.mjs`

## Context

MeshCore v1.18.0 (`companion-v1.18.0`, commit `81dbae5`) added region
scoping: a sender can suffix a flood frame with a 16-bit transport code
derived from a publicly-known region name, and repeaters that belong to a
different region drop it. This is the mechanism that keeps a busy regional
mesh from carrying every other region's flood traffic.

MeshCadet must remain byte-exact-compatible with MeshCore while adding no
new on-air behaviour of its own (ADR-0001). That makes region scoping a
dual-surface feature: an on-air wire format that must match upstream
exactly, and a local configuration surface that is entirely MeshCadet's
choice. This ADR records the configuration and scope decisions; the on-air
details below are pinned to upstream source, not invented here.

The verified upstream semantics (re-checked against
`src/helpers/RegionMap.cpp` and `src/helpers/TransportKeyStore.cpp` at
`companion-v1.18.0`) are:

- A scoped frame is
  `[header=(ptype<<2)|route][code0 lo,hi][code1 lo,hi][path_len][path][payload]`.
  The four code bytes exist only for route types `TRANSPORT_FLOOD (0x00)`
  and `TRANSPORT_DIRECT (0x03)`; every other route type is unsuffixed.
  `protocol/src/frame.rs` already parses this branch on receive.
- `code0 = HMAC-SHA256(region_key, payload_type_byte || payload)[0:2]`,
  interpreted little-endian and remapped `0x0000 -> 0x0001`,
  `0xFFFF -> 0xFFFE` (`TransportKeyStore::calcTransportCode`). It is keyed
  on the payload, so it is computed once per distinct frame — never
  recomputed per retry or per hop.
- `code1` is `0` at every upstream send site (`MyMesh.cpp` companion and
  room `sendFloodScoped`). It is a reserved home/return-region slot, not a
  hop index; v1 emits `0`.
- A public region key is `SHA256("#" + bare_name)[0:16]`. A name supplied
  with a leading `#` keeps it; a bare name gets one prepended
  (`RegionMap::getTransportKeysFor`, `TransportKeyStore::getAutoKeyFor`).
- Names are at most 31 bytes. Upstream `RegionMap::is_name_char` accepts
  `-`, `$`, `#`, ASCII digits, and every byte `>= 'A'` (which also admits
  lowercase and UTF-8 bytes `>= 0x80`). A **leading** `$` marks a private
  region; upstream's private hardware keystore is unimplemented
  (`loadKeysFor` returns no keys).

## Decision

### D1 — Outbound-only scope in v1

MeshCadet only decides what it **emits**. Receiving scoped frames already
works (`protocol/src/frame.rs` skips the four code bytes), but v1 does not
assign this device to a region for inbound filtering, does not participate
in a region hierarchy, and does not answer scoped frames with a return
region (`code1` stays `0`). Those are receive-policy features with their own
interop questions; bundling them into the first release would make a
configuration mistake change *what this device hears* as well as what it
sends. Outbound-only is the smaller, independently verifiable half, and it
is the half that solves the actual complaint: a MeshCadet flooding traffic
into regions where nobody wants it.

Consequence stated plainly: this device can send into a region without
filtering inbound traffic by that region. That is the deliberate v1
boundary, not an oversight.

### D2 — Configuration only via the web provisioner and host CLI

There is deliberately **no on-device/T-Deck control** for the region
(owner's explicit decision). Region scoping is a provisioning-time,
fleet-level decision: a wrong value silences the device on a mesh that is
not region-managed, and the person holding the T-Deck in the field is
usually not the person who knows the regional plan. Putting it behind the
admin menu would add a high-consequence, rarely-used setting to the
smallest screen in the system, with no way to show the full name or the
scope consequences. The two existing provisioning surfaces already require
physical USB possession (ADR-0001 §4), have room for the warning copy, and
are where every other mesh-wide setting is applied. The host CLI
(`set-region --name` / `set-region --clear`) and the web provisioner
(region field, Save, read-back, Clear) are therefore the only writers.

`status` and the provisioner's status panel read the value back through a
dedicated `QUERY_REGION` / `RSP_REGION` pair rather than adding a trailing
field to `RSP_STATUS`: the status payload has a long amendment history and
a frozen shape, and region state can evolve (receive policy, hierarchy)
without dragging that payload along.

### D3 — `{0, 0}` is forbidden; "no region" means plain `FLOOD`

Upstream treats `transport_codes[0] == 0` as the share/"send to nowhere"
marker: `RegionMap::findMatch` short-circuits it straight to the wildcard
region and **ignores that region's `DENY` flags**. Encoding "no region" as
`TRANSPORT_FLOOD{0,0}` would therefore look scoped while actually
broadcasting everywhere — a silent wrong-scope emission that no status line
would reveal. The transport-code remap already refuses to mint `0x0000`
from a payload; the second half of the rule is that an unconfigured device
must keep emitting plain `ROUTE_TYPE_FLOOD` exactly as before, with no code
bytes at all. `write_unscoped_flood_frame` and
`write_scoped_flood_frame` in `protocol/src/region.rs` pin both shapes, and
a regression test asserts the unscoped header route bits.

### D4 — Fail closed on anything that cannot derive a key

A configured region whose key cannot be derived must surface as an error,
never fall back to unscoped emission: a healthy-looking device emitting
traffic that every region drops is the exact defect this feature fixes.
Accordingly:

- Name validation is shared by `protocol::region`, the provisioning codec,
  the host CLI, and the browser form. The browser refuses to send an invalid
  name; the host CLI refuses before touching the port; the Rust decoder
  rejects it again at the wire boundary.
- A leading `$` (private region) is **accepted as a valid upstream name
  shape but rejected with a clear error**, because upstream has no private
  keystore implementation for v1 to key it with. Silently provisioning it
  as a public region would scope traffic to a key the sender did not choose.
- Over-length names are rejected, never truncated. A truncated region name
  derives a different key and therefore scopes to a different region.

### D5 — Provisioning frames are additive

`FRAME_QUERY_REGION (0x07)`, `FRAME_SET_REGION (0x54)`, and
`FRAME_RSP_REGION (0x8E)` use unused command/response values; nothing is
renumbered, and the retired `0x30`/`0x60` values stay retired (ADR-0002
§3). `SET_REGION`'s payload is `name_len(1) | name(name_len)`, with an empty
name meaning "clear" — the same length-prefixed shape as
`SET_DEVICE_NAME`, deliberately without truncation. Old decoders that do
not know these frame types are unaffected; they reject them as unknown,
exactly as they already do for every later-added frame.

### D6 — No behaviour change for unconfigured devices

The default remains a plain unscoped `ROUTE_TYPE_FLOOD` frame, byte-for-byte
identical to the pre-region path. Region scoping is opt-in; nothing in this
ADR changes emission for a device with no configured name.

## Consequences

- A device with a region configured, deployed onto a mesh that is not
  region-managed, goes silent (frames dropped at the first hop). The
  provisioner and CLI warn about this explicitly; clearing the region and
  committing restores unscoped emission.
- `code1` is always `0` in v1. Upstream reserves it for a future
  home/return region; matching upstream's current send sites is the
  compatible choice, and it leaves room to implement returns later without
  changing this release's frames.
- Private (`$`) regions are unavailable until upstream implements a
  keystore. The rejection is intentional and user-visible, not a silent
  fallback.
- **Firmware wiring is a deliberate follow-up PR.** This change lands the
  protocol codec, host CLI, web provisioner, golden vectors, and tests. The
  detached `firmware/` workspace cannot be compiled in the host-only
  environment used for this change (it needs the Xtensa ESP toolchain), so
  wiring `protocol::region` into the firmware's send path and provisioning
  handlers is left to a separate, hardware-verifiable change rather than
  landing compile-unverified edits here.
