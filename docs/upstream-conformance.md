# Upstream wire conformance

MeshCadet's charter (ADR-0001) makes byte-exact MeshCore interop a hard
requirement. The codec's own unit tests pin the pieces — a header byte here,
an AES block there — but none of them compare a *whole frame* against the
upstream firmware that composes it. `protocol/tests/upstream_conformance.rs`
closes that gap with checked-in golden frames, and
`xtask/src/wire_vectors.rs` keeps those frames reproducible from a read-only
upstream checkout.

## What is pinned

Every vector records:

- the logical inputs (seeds, timestamps, text/password, LPP coordinates, …);
- the exact on-air bytes, decomposed as `header`, `path_len`, `path`, and
  `payload`, plus the whole `frame_hex`;
- provenance: the upstream tag and commit, and one citation per rule the
  frame depends on (`file`, `line`, the matched source snippet, and a note);
- a short derivation explaining how the frame was composed.

The frames use upstream's default framing: route `FLOOD` for outbound
requests/adverts/group text and `DIRECT` for the two decode-side replies, with
`path_len = 0x00` (upstream's default one-byte path hash, zero hops from
`Mesh::sendFlood`'s `path_hash_size = 1`). MeshCadet's own firmware presets
two-byte path hashes for its outbound frames; that is a routing preference,
not a payload-format difference, and is pinned separately by
`protocol/tests/tx_frame_wire.rs`.

## Vector inventory and provenance

All `companion-v1.18.0` vectors are derived from commit
`81dbae592ffad40b4212bc0b37e82b38da08f4bc`. The single `companion-v1.17.1`
vector is derived from commit
`d92964352441e53b93e8667b802e04f6e072b39e`.

| Vector | Kind | Upstream composition |
|--------|------|----------------------|
| `advert_v118` | ADVERT | `src/Mesh.cpp::createAdvert` (payload + signature) and `sendFlood` (header/path_len) |
| `dm_txt_v118` | TXT_MSG | `src/helpers/BaseChatMesh.cpp::sendMessage` plaintext + `src/Mesh.cpp::createDatagram` |
| `ack_v118` | ACK | `src/helpers/BaseChatMesh.cpp:245` (`sha256(ack_hash, 4, data, 5 + text_len, sender_pub_key, 32)`) |
| `grp_txt_v118` | GRP_TXT | `BaseChatMesh::sendGroupMessage` + `Mesh::createGroupDatagram` + `sha256(channel_secret)[0]` |
| `room_login_req_v118` | ANON_REQ | `BaseChatMesh::sendLogin` (room arm) + `Mesh::createAnonDatagram` |
| `room_login_resp_v118` | RESPONSE | `examples/simple_room_server/MyMesh.cpp` login reply |
| `telemetry_req_v118` | REQ | `BaseChatMesh::sendRequest(uint8_t req_type, …)` |
| `telemetry_resp_gps_v118` | RESPONSE | `examples/companion_radio/MyMesh.cpp::onContactRequest` + `EnvironmentSensorManager::querySensors`' `addGPS` |
| `advert_v1171` | ADVERT | same advert block in `companion-v1.17.1`; proves the frame is byte-identical across the version bump |

Every citation's line number is resolved by the generator from the checkout
it is run against. If upstream moves the cited statement, regeneration fails
loudly instead of silently keeping a stale line number.

### Deliberate compatibility notes

- **ACK size.** Upstream v1.16+ emits a 6-byte ACK: the 4-byte truncated hash
  plus an extended-attempt byte and one random perturbation byte. Every stock
  receiver gates on `payload_len >= 4` and copies exactly 4 bytes
  (`Mesh.cpp`'s `PAYLOAD_TYPE_ACK` arms). MeshCadet emits the compatible
  4-byte form, and `ack_v118` pins those 4 bytes. See
  `protocol/src/codec.rs::compute_ack_hash`.
- **Login request NUL.** Upstream's room-login plaintext length is
  `8 + password_len` and relies on AES zero padding for the C-string NUL.
  MeshCadet writes the NUL explicitly. For the 10-byte password in the
  vector both forms occupy the same AES block count and produce identical
  ciphertext. A password whose length makes upstream's plaintext an exact
  multiple of 16 (8 bytes) would differ in ciphertext length; the vector
  deliberately avoids that case and the test asserts the decrypted NUL.
- **Telemetry response subset.** MeshCadet's responder encodes GPS and battery
  LPP entries. The vector uses the GPS entry, which is exactly what upstream's
  `EnvironmentSensorManager` adds for location-permitted requests. Upstream's
  *base* telemetry reply starts with `LPP_VOLTAGE` (116) and optionally
  `LPP_TEMPERATURE` (103); MeshCadet's `decode_telemetry_response_lpp` stops at
  the first unrecognised entry type, so those upstream replies decode with the
  tag but no sensor fields. This is a known interop gap, reported rather than
  fixed by the conformance PR that introduced these vectors.
- **Advert route type.** `build_self_advert_card` emits `FLOOD` (0x11), not
  `TRANSPORT_FLOOD`; `src/Mesh.cpp`'s `createAdvert` leaves the route bits to
  `sendFlood`, which also selects `FLOOD`.

## Regeneration

The generator never touches the network. Clone the exact upstream tag
read-only into `/tmp` (or anywhere outside the repo), then run the xtask
subcommand:

```sh
git clone --depth 1 --branch companion-v1.18.0 \
  https://github.com/meshcore-dev/MeshCore /tmp/mc118
cargo run -p xtask --bin xtask -- regen-wire-vectors \
  --upstream /tmp/mc118 --tag companion-v1.18.0

git clone --depth 1 --branch companion-v1.17.1 \
  https://github.com/meshcore-dev/MeshCore /tmp/mc117
cargo run -p xtask --bin xtask -- regen-wire-vectors \
  --upstream /tmp/mc117 --tag companion-v1.17.1
```

The generator verifies the checkout's `HEAD` against the tag's pinned commit
before writing, records the upstream enum/constant inventory, and rewrites
`protocol/tests/fixtures/wire/companion_v1_18_0.json` or
`companion_v1_17_1.json`. Review the diff: a changed expected frame is a
protocol change, not formatting noise.

The command does **not** run in the ordinary guard battery; it is a
maintenance step. `cargo test --workspace --locked` (the existing CI `test`
job) runs both the per-vector tests and the exhaustive inventory test, so no
new CI job is required.

## Maintainer checklist: bumping the target firmware

When upstream tags a new companion release:

1. Create a worktree or branch and bump the version/provenance references
   (`protocol/src/lib.rs`'s target-firmware prose, `Cargo.toml` metadata, and
   any docs that name the target tag).
2. Re-read these upstream files, in this order:
   - `src/Packet.h` — every `PAYLOAD_TYPE_*` and `ROUTE_TYPE_*` define, and
     the header/`path_len` bit layout. This is the inventory the exhaustive
     test pins; a new type must be a deliberate `protocol::PayloadType`
     addition.
   - `src/MeshCore.h` — `PUB_KEY_SIZE`, `SIGNATURE_SIZE`,
     `MAX_ADVERT_DATA_SIZE`, the `CIPHER_*` sizes, `PATH_HASH_SIZE`,
     `MAX_PACKET_PAYLOAD`, `MAX_PATH_SIZE`, and any firmware version level
     the examples advertise.
   - `src/Utils.cpp` — encrypt-then-MAC layout, AES mode/padding, HMAC key
     length and truncation.
   - `src/Mesh.cpp` — `createAdvert`, `createDatagram`,
     `createAnonDatagram`, `createGroupDatagram`, `createAck`,
     `createPathReturn`, and the `sendFlood`/`sendDirect` framing rules.
   - `src/helpers/BaseChatMesh.cpp` — `sendMessage`, `sendGroupMessage`,
     `sendLogin`, `sendRequest`, ACK hash construction, and the text-type
     flags.
   - `examples/simple_room_server/MyMesh.cpp` — the 13-byte login reply and
     `FIRMWARE_VER_LEVEL`.
   - `examples/companion_radio/MyMesh.cpp` — telemetry/request handling and
     the response tag reflection.
   - `src/helpers/sensors/LPPDataHelpers.h` (and the pinned `CayenneLPP`
     library version in `platformio.ini`) — LPP type bytes and scaling.
3. Run the two regeneration commands above for the new tag. If the new tag is
   not yet in `xtask/src/wire_vectors.rs`'s `TARGETS` table, add it with its
   exact commit (and add a v1.17.1-style "byte-identical" case only if the
   relevant upstream block really is unchanged; verify with `diff`).
4. Diff the fixtures. For every changed frame, identify the upstream statement
   that changed and decide whether it is a compatible extension (parser
   accepts both), a MeshCadet code change, or a deliberate incompatibility
   that must be discussed in an issue first (ADR-0001).
5. Update `protocol/tests/upstream_conformance.rs` only when the expected
   inventory intentionally changes (new payload type, new constants). The
   test failing on a new upstream type is the signal to do this consciously.
6. Run the full lane:

   ```sh
   cargo test --workspace --locked
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo run -p xtask --bin xtask
   scripts/check-vocabulary-leak.sh
   scripts/check-commit-format.sh
   ```

7. In the PR body, list each vector and its provenance, and state explicitly
   whether any known compatibility note in this document changed.
