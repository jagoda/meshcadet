// SPDX-License-Identifier: GPL-3.0-only
//! Upstream wire-conformance vectors (issue #214).
//!
//! MeshCadet's charter is byte-exact MeshCore interop, but the codec's own
//! unit tests only pin the pieces they construct: a header byte here, an AES
//! block there. None of them compare a *whole frame* against the upstream
//! firmware that composes it. That leaves two silent-drift holes this file
//! closes:
//!
//! 1. **Composition drift.** A refactor can keep encode/decode self-consistent
//!    (round-trip tests still pass) while putting different bytes on the air.
//!    Each checked-in vector below is an expected frame derived from upstream
//!    MeshCore source, not captured from MeshCadet, so a composition change
//!    fails a named test.
//! 2. **Enum drift.** Upstream can add a payload type in `src/Packet.h` and
//!    MeshCadet would keep accepting its old subset while silently reporting
//!    `None` for the new one. [`payload_type_and_route_inventory_is_exhaustive`]
//!    iterates every payload-type nibble and route-bit pair and compares the
//!    accepted set against the inventory recorded from the pinned upstream
//!    checkout.
//!
//! Fixtures live in `tests/fixtures/wire/` and are generated (never hand
//! edited) by `cargo run -p xtask --bin xtask -- regen-wire-vectors` from a
//! read-only upstream checkout. See `docs/upstream-conformance.md` for
//! provenance, the firmware-bump checklist, and the regeneration command.
//!
//! # Vector provenance summary
//!
//! | Vector | Upstream source |
//! |--------|-----------------|
//! | `advert_v118` | `src/Mesh.cpp::createAdvert` + `sendFlood` |
//! | `dm_txt_v118` | `BaseChatMesh::sendMessage` + `Mesh::createDatagram` |
//! | `ack_v118` | `BaseChatMesh.cpp:245` ACK hash truncation |
//! | `grp_txt_v118` | `BaseChatMesh::sendGroupMessage` + `Mesh::createGroupDatagram` |
//! | `room_login_req_v118` | `BaseChatMesh::sendLogin` (room arm) + `createAnonDatagram` |
//! | `room_login_resp_v118` | `simple_room_server/MyMesh.cpp` login reply |
//! | `telemetry_req_v118` | `BaseChatMesh::sendRequest(req_type)` |
//! | `telemetry_resp_gps_v118` | `onContactRequest` + `EnvironmentSensorManager::addGPS` |
//! | `advert_v1171` | same advert block, v1.17.1 (byte-identical proof) |
//!
//! Every frame uses upstream's default one-byte path hash and zero hops
//! (`path_len = 0x00`), matching `Mesh::sendFlood`'s `path_hash_size = 1`
//! default. MeshCadet's own firmware presets two-byte hashes for its
//! outbound frames; that choice is orthogonal to payload composition and is
//! pinned separately by `tests/tx_frame_wire.rs`.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use protocol::{
    build_self_advert_card, channel_hash_var, compute_ack_hash, decode_dm_payload,
    decode_grp_txt_var, decode_login_response, decode_telemetry_response_lpp,
    encode_anon_req_login, encode_dm_payload, encode_grp_txt_var, encode_telemetry_response_lpp,
    encode_txt_msg_plaintext, format_channel_text, mac_then_decrypt, parse_channel_text,
    parse_rx_frame_header, parse_telemetry_req, Header, Identity, PathLen, RoomPermission,
    RouteType, MAX_ADVERT_CARD_LEN,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

const V118_JSON: &str = include_str!("fixtures/wire/companion_v1_18_0.json");
const V1171_JSON: &str = include_str!("fixtures/wire/companion_v1_17_1.json");

const V118_COMMIT: &str = "81dbae592ffad40b4212bc0b37e82b38da08f4bc";
const V1171_COMMIT: &str = "d92964352441e53b93e8667b802e04f6e072b39e";

#[derive(Debug, Deserialize)]
struct Fixture {
    schema: u32,
    upstream_tag: String,
    upstream_commit: String,
    inventory: Inventory,
    vectors: Vec<Vector>,
}

#[derive(Debug, Deserialize)]
struct Inventory {
    payload_types: Vec<NamedValue>,
    route_types: Vec<NamedValue>,
    constants: std::collections::BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
struct NamedValue {
    name: String,
    value: u8,
}

#[derive(Debug, Deserialize)]
struct Vector {
    id: String,
    upstream: UpstreamRef,
    kind: String,
    direction: String,
    inputs: Value,
    frame: FrameJson,
    provenance: Vec<Provenance>,
    derivation: String,
    #[serde(default)]
    byte_identical_to: Option<String>,
    #[serde(default, rename = "compatibility_note")]
    compat: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpstreamRef {
    tag: String,
    commit: String,
}

#[derive(Debug, Deserialize)]
struct FrameJson {
    header: u8,
    path_len: u8,
    path_hex: String,
    payload_hex: String,
    frame_hex: String,
}

#[derive(Debug, Deserialize)]
struct Provenance {
    file: String,
    line: u64,
    snippet: String,
    note: String,
}

fn v118() -> Fixture {
    serde_json::from_str(V118_JSON).expect("companion_v1_18_0.json must parse")
}

fn v1171() -> Fixture {
    serde_json::from_str(V1171_JSON).expect("companion_v1_17_1.json must parse")
}

fn vector<'a>(fixture: &'a Fixture, id: &str) -> &'a Vector {
    fixture
        .vectors
        .iter()
        .find(|v| v.id == id)
        .unwrap_or_else(|| panic!("fixture has no vector {id:?}"))
}

// ── Input helpers ───────────────────────────────────────────────────────────

fn hex_decode(text: &str) -> Vec<u8> {
    assert!(
        text.len().is_multiple_of(2),
        "odd-length hex string {text:?}"
    );
    (0..text.len() / 2)
        .map(|i| {
            u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
                .unwrap_or_else(|_| panic!("invalid hex in {text:?}"))
        })
        .collect()
}

fn hex_seed(text: &str) -> [u8; 32] {
    let bytes = hex_decode(text);
    let len = bytes.len();
    bytes
        .try_into()
        .unwrap_or_else(|_| panic!("expected a 32-byte seed, got {len} bytes"))
}

fn str_input<'a>(v: &'a Vector, key: &str) -> &'a str {
    v.inputs[key]
        .as_str()
        .unwrap_or_else(|| panic!("{}: inputs.{key} must be a string", v.id))
}

fn u32_input(v: &Vector, key: &str) -> u32 {
    u32::try_from(
        v.inputs[key]
            .as_u64()
            .unwrap_or_else(|| panic!("{}: inputs.{key} must be an integer", v.id)),
    )
    .unwrap_or_else(|_| panic!("{}: inputs.{key} out of u32 range", v.id))
}

fn i32_input(v: &Vector, key: &str) -> i32 {
    i32::try_from(
        v.inputs[key]
            .as_i64()
            .unwrap_or_else(|| panic!("{}: inputs.{key} must be an integer", v.id)),
    )
    .unwrap_or_else(|_| panic!("{}: inputs.{key} out of i32 range", v.id))
}

fn u8_input(v: &Vector, key: &str) -> u8 {
    u8::try_from(
        v.inputs[key]
            .as_u64()
            .unwrap_or_else(|| panic!("{}: inputs.{key} must be an integer", v.id)),
    )
    .unwrap_or_else(|_| panic!("{}: inputs.{key} out of u8 range", v.id))
}

fn expected_frame(v: &Vector) -> Vec<u8> {
    // The fixture stores the frame both decomposed and whole; reject a
    // fixture whose own parts disagree before using it as an oracle.
    let frame = hex_decode(&v.frame.frame_hex);
    let mut composed = vec![v.frame.header, v.frame.path_len];
    composed.extend_from_slice(&hex_decode(&v.frame.path_hex));
    composed.extend_from_slice(&hex_decode(&v.frame.payload_hex));
    assert_eq!(
        frame, composed,
        "{}: frame_hex disagrees with header/path_len/path/payload_hex",
        v.id
    );
    frame
}

fn compose(header: u8, path_len: u8, path: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + path.len() + payload.len());
    out.push(header);
    out.push(path_len);
    out.extend_from_slice(path);
    out.extend_from_slice(payload);
    out
}

/// Parse the checked-in frame the way a receiver does, so every vector also
/// exercises the real header/path_len/offset arithmetic.
fn expected_payload(v: &Vector) -> Vec<u8> {
    let frame = expected_frame(v);
    let parsed = parse_rx_frame_header(&frame)
        .unwrap_or_else(|e| panic!("{}: expected frame does not parse: {e:?}", v.id));
    assert_eq!(parsed.header_byte, v.frame.header, "{} header", v.id);
    assert_eq!(parsed.path_len_byte, v.frame.path_len, "{} path_len", v.id);
    assert_eq!(
        parsed.route_type,
        Header(v.frame.header).route_type(),
        "{}: route bits decode consistently",
        v.id
    );
    frame[parsed.payload_off..].to_vec()
}

fn check_frame(v: &Vector, built: &[u8]) {
    let expected = expected_frame(v);
    assert_eq!(
        built,
        expected,
        "{}: built frame ({} bytes) differs from the upstream vector ({} bytes)\n  built:    {}\n  expected: {}",
        v.id,
        built.len(),
        expected.len(),
        hex_encode(built),
        hex_encode(&expected),
    );
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0F) as usize] as char);
    }
    out
}

/// Trim AES zero padding at the first NUL, the receive-side convention both
/// upstream and MeshCadet use for text fields.
fn trim_nul(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    &bytes[..end]
}

fn permissions_value(p: RoomPermission) -> u8 {
    match p {
        RoomPermission::Guest => 0,
        RoomPermission::ReadOnly => 1,
        RoomPermission::ReadWrite => 2,
        RoomPermission::Admin => 3,
    }
}

// ── Per-vector tests ────────────────────────────────────────────────────────
//
// One named test per checked-in frame so a failure names the exact frame in
// CI output rather than just "conformance".

#[test]
fn advert_v118() {
    check_vector(&v118(), "advert_v118");
}

#[test]
fn dm_txt_v118() {
    check_vector(&v118(), "dm_txt_v118");
}

#[test]
fn ack_v118() {
    check_vector(&v118(), "ack_v118");
}

#[test]
fn grp_txt_v118() {
    check_vector(&v118(), "grp_txt_v118");
}

#[test]
fn room_login_req_v118() {
    check_vector(&v118(), "room_login_req_v118");
}

#[test]
fn room_login_resp_v118() {
    check_vector(&v118(), "room_login_resp_v118");
}

#[test]
fn telemetry_req_v118() {
    check_vector(&v118(), "telemetry_req_v118");
}

#[test]
fn telemetry_resp_gps_v118() {
    check_vector(&v118(), "telemetry_resp_gps_v118");
}

/// v1.17.1 target case: the advert block is byte-identical upstream, so this
/// vector must build to exactly the v1.18.0 advert frame. This is the
/// "one older tag proves the target is not a v1.18-only accident" check.
#[test]
fn advert_v1171_byte_identical() {
    let fixture = v1171();
    let v = vector(&fixture, "advert_v1171");
    assert_eq!(v.byte_identical_to.as_deref(), Some("advert_v118"));
    let v118_fixture = v118();
    let v118_advert = vector(&v118_fixture, "advert_v118");
    assert_eq!(
        expected_frame(v),
        expected_frame(v118_advert),
        "advert_v1171 must be byte-identical to advert_v118"
    );
    check_vector(&fixture, "advert_v1171");
}

fn check_vector(fixture: &Fixture, id: &str) {
    let v = vector(fixture, id);
    match v.kind.as_str() {
        "advert" => check_advert(v),
        "dm_txt" => check_dm_txt(v),
        "ack" => check_ack(v),
        "grp_txt" => check_grp_txt(v),
        "room_login_req" => check_room_login_req(v),
        "room_login_resp" => check_room_login_resp(v),
        "telemetry_req" => check_telemetry_req(v),
        "telemetry_resp_gps" => check_telemetry_resp_gps(v),
        other => panic!("{}: unknown vector kind {other:?}", v.id),
    }
}

fn check_advert(v: &Vector) {
    let identity = Identity::from_seed(hex_seed(str_input(v, "identity_seed_hex")));
    let timestamp = u32_input(v, "timestamp");
    let name = str_input(v, "name");

    // `build_self_advert_card` returns the complete frame (header + path_len
    // included), unlike the payload-only codec helpers used by the other
    // vectors — so it is compared directly rather than re-framed.
    let mut card = [0u8; MAX_ADVERT_CARD_LEN];
    let n = build_self_advert_card(&identity, timestamp, name, &mut card);
    check_frame(v, &card[..n]);

    // Decode the checked-in frame: header/path_len via the shared RX parser,
    // then the advert body and its Ed25519 signature.
    let payload = expected_payload(v);
    assert_eq!(&payload[..32], &identity.pubkey, "{}: pubkey", v.id);
    assert_eq!(
        u32::from_le_bytes(payload[32..36].try_into().unwrap()),
        timestamp,
        "{}: timestamp",
        v.id
    );
    let signature_bytes: [u8; 64] = payload[36..100].try_into().unwrap();
    let appdata = &payload[100..];
    assert_eq!(appdata[0], 0x81, "{}: advert flags", v.id);
    assert_eq!(&appdata[1..], name.as_bytes(), "{}: advert name", v.id);

    let mut message = payload[..36].to_vec();
    message.extend_from_slice(appdata);
    let verifying_key = VerifyingKey::from_bytes(&identity.pubkey).unwrap();
    verifying_key
        .verify(&message, &Signature::from_bytes(&signature_bytes))
        .unwrap_or_else(|e| panic!("{}: advert signature must verify: {e:?}", v.id));
}

fn check_dm_txt(v: &Vector) {
    let sender = Identity::from_seed(hex_seed(str_input(v, "sender_seed_hex")));
    let recipient = Identity::from_seed(hex_seed(str_input(v, "recipient_seed_hex")));
    let timestamp = u32_input(v, "timestamp");
    let txt_type = u8_input(v, "txt_type");
    let attempt = u8_input(v, "attempt");
    let text = str_input(v, "text").as_bytes();

    let shared_tx = sender.ecdh_shared_secret(&recipient.pubkey);
    let mut plaintext = [0u8; 256];
    let pt_len = encode_txt_msg_plaintext(timestamp, txt_type, attempt, text, &mut plaintext);
    let mut payload = [0u8; 256];
    let n = encode_dm_payload(
        &shared_tx,
        recipient.pub_hash(),
        sender.pub_hash(),
        &plaintext[..pt_len],
        &mut payload,
    );
    let built = compose(v.frame.header, v.frame.path_len, &[], &payload[..n]);
    check_frame(v, &built);

    let expected = expected_payload(v);
    let shared_rx = recipient.ecdh_shared_secret(&sender.pubkey);
    let mut decoded = [0u8; 256];
    let (dest, src, decoded_len) = decode_dm_payload(&shared_rx, &expected, &mut decoded)
        .unwrap_or_else(|e| panic!("{}: DM decode failed: {e:?}", v.id));
    assert_eq!(dest, recipient.pub_hash(), "{}: dest hash", v.id);
    assert_eq!(src, sender.pub_hash(), "{}: src hash", v.id);
    let (rx_ts, rx_type, rx_attempt, off) =
        protocol::decode_txt_msg_plaintext(&decoded, decoded_len).unwrap();
    assert_eq!(rx_ts, timestamp, "{}: timestamp", v.id);
    assert_eq!(rx_type, txt_type, "{}: txt_type", v.id);
    assert_eq!(rx_attempt, attempt, "{}: attempt", v.id);
    assert_eq!(trim_nul(&decoded[off..decoded_len]), text, "{}: text", v.id);
}

fn check_ack(v: &Vector) {
    let sender = Identity::from_seed(hex_seed(str_input(v, "sender_seed_hex")));
    let timestamp = u32_input(v, "timestamp");
    let txt_type = u8_input(v, "txt_type");
    let attempt = u8_input(v, "attempt");
    let text = str_input(v, "text").as_bytes();

    let type_byte = (txt_type << 2) | (attempt & 0x03);
    let ack = compute_ack_hash(timestamp, type_byte, text, &sender.pubkey);
    let built = compose(v.frame.header, v.frame.path_len, &[], &ack);
    check_frame(v, &built);

    // Receivers gate on payload_len >= 4 and copy exactly the first 4 bytes
    // (`Mesh.cpp`'s PAYLOAD_TYPE_ACK arms), so the prefix is the contract.
    let payload = expected_payload(v);
    assert_eq!(payload.len(), 4, "{}: ACK payload length", v.id);
    assert_eq!(payload, ack, "{}: ACK hash", v.id);
}

fn check_grp_txt(v: &Vector) {
    let secret = hex_seed(str_input(v, "channel_secret_hex"));
    let timestamp = u32_input(v, "timestamp");
    let sender_name = str_input(v, "sender_name").as_bytes();
    let body = str_input(v, "text").as_bytes();

    let mut text = [0u8; 256];
    let text_len = format_channel_text(sender_name, body, &mut text);
    let mut payload = [0u8; 512];
    let n = encode_grp_txt_var(&secret, timestamp, 0, 0, &text[..text_len], &mut payload);
    let built = compose(v.frame.header, v.frame.path_len, &[], &payload[..n]);
    check_frame(v, &built);

    let expected = expected_payload(v);
    assert_eq!(
        expected[0],
        channel_hash_var(&secret),
        "{}: channel hash",
        v.id
    );
    let mut decoded = [0u8; 512];
    let fields = decode_grp_txt_var(&secret, &expected, &mut decoded)
        .unwrap_or_else(|e| panic!("{}: GRP_TXT decode failed: {e:?}", v.id));
    assert_eq!(fields.timestamp, timestamp, "{}: timestamp", v.id);
    let rx_text = trim_nul(&decoded[fields.text_offset..fields.text_offset + fields.text_len]);
    assert_eq!(rx_text, &text[..text_len], "{}: full text", v.id);
    let (name, rest) = parse_channel_text(rx_text);
    assert_eq!(name, Some(sender_name), "{}: sender prefix", v.id);
    assert_eq!(rest, body, "{}: body", v.id);
}

fn check_room_login_req(v: &Vector) {
    let client = Identity::from_seed(hex_seed(str_input(v, "client_seed_hex")));
    let server = Identity::from_seed(hex_seed(str_input(v, "server_seed_hex")));
    let timestamp = u32_input(v, "timestamp");
    let sync_since = u32_input(v, "sync_since");
    let password = str_input(v, "password").as_bytes();

    let shared = client.ecdh_shared_secret(&server.pubkey);
    let mut payload = [0u8; 256];
    let n = encode_anon_req_login(
        &shared,
        server.pub_hash(),
        &client.pubkey,
        timestamp,
        sync_since,
        password,
        &mut payload,
    );
    let built = compose(v.frame.header, v.frame.path_len, &[], &payload[..n]);
    check_frame(v, &built);

    // Decode the upstream frame manually: ANON_REQ is dest_hash || full sender
    // pubkey || encrypt-then-MAC, so there is no public "login request decode"
    // helper (the room server does this arithmetic inline).
    let expected = expected_payload(v);
    assert_eq!(expected[0], server.pub_hash(), "{}: dest hash", v.id);
    assert_eq!(&expected[1..33], &client.pubkey, "{}: sender pubkey", v.id);
    let (aes_key, hmac_key) = Identity::split_secret(&shared);
    let mut plaintext = [0u8; 256];
    let pt_len = mac_then_decrypt(&aes_key, &hmac_key, &expected[33..], &mut plaintext)
        .unwrap_or_else(|_| panic!("{}: login request MAC did not verify", v.id));
    assert_eq!(
        u32::from_le_bytes(plaintext[0..4].try_into().unwrap()),
        timestamp,
        "{}: timestamp",
        v.id
    );
    assert_eq!(
        u32::from_le_bytes(plaintext[4..8].try_into().unwrap()),
        sync_since,
        "{}: sync_since",
        v.id
    );
    assert_eq!(
        &plaintext[8..8 + password.len()],
        password,
        "{}: password",
        v.id
    );
    assert!(
        pt_len >= 8 + password.len(),
        "{}: plaintext shorter than password",
        v.id
    );
    // Upstream's tlen is 8 + password length; MeshCadet writes an explicit
    // NUL at that position, which lands inside the same AES zero-padded block
    // for this vector's password length (see the fixture's derivation note).
    assert_eq!(
        plaintext[8 + password.len()],
        0,
        "{}: NUL terminator in the decrypted block",
        v.id
    );
}

fn check_room_login_resp(v: &Vector) {
    let server = Identity::from_seed(hex_seed(str_input(v, "server_seed_hex")));
    let client = Identity::from_seed(hex_seed(str_input(v, "client_seed_hex")));
    let server_ts = u32_input(v, "server_ts");
    let code = u8_input(v, "code");
    let legacy_keepalive = u8_input(v, "legacy_keepalive");
    let role_code = u8_input(v, "role_code");
    let permissions = u8_input(v, "permissions");
    let random = hex_decode(str_input(v, "random_hex"));
    let firmware_ver_level = u8_input(v, "firmware_ver_level");
    assert_eq!(random.len(), 4, "{}: random blob length", v.id);

    let mut plaintext = Vec::new();
    plaintext.extend_from_slice(&server_ts.to_le_bytes());
    plaintext.push(code);
    plaintext.push(legacy_keepalive);
    plaintext.push(role_code);
    plaintext.push(permissions);
    plaintext.extend_from_slice(&random);
    plaintext.push(firmware_ver_level);

    let shared = server.ecdh_shared_secret(&client.pubkey);
    let mut payload = [0u8; 256];
    let n = encode_dm_payload(
        &shared,
        client.pub_hash(),
        server.pub_hash(),
        &plaintext,
        &mut payload,
    );
    let built = compose(v.frame.header, v.frame.path_len, &[], &payload[..n]);
    check_frame(v, &built);

    let expected = expected_payload(v);
    let shared_rx = client.ecdh_shared_secret(&server.pubkey);
    let mut decoded = [0u8; 256];
    let (dest, src, decoded_len) = decode_dm_payload(&shared_rx, &expected, &mut decoded)
        .unwrap_or_else(|e| panic!("{}: RESPONSE decode failed: {e:?}", v.id));
    assert_eq!(dest, client.pub_hash(), "{}: dest hash", v.id);
    assert_eq!(src, server.pub_hash(), "{}: src hash", v.id);
    assert!(decoded_len >= 13, "{}: login response too short", v.id);
    let response = decode_login_response(&decoded[..decoded_len])
        .unwrap_or_else(|e| panic!("{}: login response decode failed: {e:?}", v.id));
    assert_eq!(response.server_ts, server_ts, "{}: server_ts", v.id);
    assert_eq!(response.code, code, "{}: response code", v.id);
    assert_eq!(response.role_code, role_code, "{}: legacy role code", v.id);
    assert_eq!(
        permissions_value(response.permissions),
        permissions & protocol::PERM_ACL_ROLE_MASK,
        "{}: permissions",
        v.id
    );
    assert_eq!(
        response.firmware_ver_level, firmware_ver_level,
        "{}: firmware version level",
        v.id
    );
}

fn check_telemetry_req(v: &Vector) {
    let requester = Identity::from_seed(hex_seed(str_input(v, "requester_seed_hex")));
    let responder = Identity::from_seed(hex_seed(str_input(v, "responder_seed_hex")));
    let tag = u32_input(v, "tag");
    let req_type = u8_input(v, "req_type");
    let reserved = hex_decode(str_input(v, "reserved_hex"));
    let random = hex_decode(str_input(v, "random_hex"));
    assert_eq!(reserved.len(), 4, "{}: reserved length", v.id);
    assert_eq!(random.len(), 4, "{}: random length", v.id);

    let mut plaintext = Vec::new();
    plaintext.extend_from_slice(&tag.to_le_bytes());
    plaintext.push(req_type);
    plaintext.extend_from_slice(&reserved);
    plaintext.extend_from_slice(&random);

    let shared = requester.ecdh_shared_secret(&responder.pubkey);
    let mut payload = [0u8; 256];
    let n = encode_dm_payload(
        &shared,
        responder.pub_hash(),
        requester.pub_hash(),
        &plaintext,
        &mut payload,
    );
    let built = compose(v.frame.header, v.frame.path_len, &[], &payload[..n]);
    check_frame(v, &built);

    let expected = expected_payload(v);
    let shared_rx = responder.ecdh_shared_secret(&requester.pubkey);
    let mut decoded = [0u8; 256];
    let (_, _, decoded_len) = decode_dm_payload(&shared_rx, &expected, &mut decoded)
        .unwrap_or_else(|e| panic!("{}: REQ decode failed: {e:?}", v.id));
    let req = parse_telemetry_req(&decoded[..decoded_len])
        .unwrap_or_else(|| panic!("{}: telemetry REQ did not parse", v.id));
    assert_eq!(req.tag, tag, "{}: tag", v.id);
    assert_eq!(req.req_type, req_type, "{}: req_type", v.id);
    assert_eq!(&decoded[5..9], &reserved, "{}: reserved bytes", v.id);
    assert_eq!(&decoded[9..13], &random, "{}: random bytes", v.id);
}

fn check_telemetry_resp_gps(v: &Vector) {
    let responder = Identity::from_seed(hex_seed(str_input(v, "responder_seed_hex")));
    let requester = Identity::from_seed(hex_seed(str_input(v, "requester_seed_hex")));
    let tag = u32_input(v, "tag");
    let lat_e7 = i32_input(v, "lat_e7");
    let lon_e7 = i32_input(v, "lon_e7");
    let altitude_m = i32_input(v, "altitude_m");

    let mut plaintext = [0u8; 64];
    let pt_len = encode_telemetry_response_lpp(
        tag,
        Some((lat_e7, lon_e7)),
        None,
        None,
        None,
        None,
        &mut plaintext,
    );
    assert_eq!(
        altitude_m, 0,
        "{}: MeshCadet reports altitude 0; a non-zero vector would need a codec change",
        v.id
    );

    let shared = responder.ecdh_shared_secret(&requester.pubkey);
    let mut payload = [0u8; 256];
    let n = encode_dm_payload(
        &shared,
        requester.pub_hash(),
        responder.pub_hash(),
        &plaintext[..pt_len],
        &mut payload,
    );
    let built = compose(v.frame.header, v.frame.path_len, &[], &payload[..n]);
    check_frame(v, &built);

    let expected = expected_payload(v);
    let shared_rx = requester.ecdh_shared_secret(&responder.pubkey);
    let mut decoded = [0u8; 256];
    let (_, _, decoded_len) = decode_dm_payload(&shared_rx, &expected, &mut decoded)
        .unwrap_or_else(|e| panic!("{}: RESPONSE decode failed: {e:?}", v.id));
    let response = decode_telemetry_response_lpp(&decoded[..decoded_len])
        .unwrap_or_else(|| panic!("{}: telemetry RESPONSE did not decode", v.id));
    assert_eq!(response.tag, tag, "{}: tag", v.id);
    assert_eq!(
        response.gps_e4,
        Some((lat_e7 / 1000, lon_e7 / 1000)),
        "{}: GPS fix (degrees x1e4, truncated)",
        v.id
    );
    assert_eq!(response.battery, None, "{}: no battery entry", v.id);
    assert_eq!(response.battery_raw_mv, None, "{}: no raw-mV entry", v.id);
    assert_eq!(
        response.battery_held_raw_mv, None,
        "{}: no held-raw-mV entry",
        v.id
    );
    assert_eq!(response.battery_level, None, "{}: no level entry", v.id);

    // Independently pin the raw LPP bytes: channel 1, LPP_GPS (136), then
    // three 24-bit big-endian two's-complement values.
    assert_eq!(decoded[4], 1, "{}: LPP channel", v.id);
    assert_eq!(decoded[5], 136, "{}: LPP type", v.id);
    let lat = sign_extend_i24(decoded[6], decoded[7], decoded[8]);
    let lon = sign_extend_i24(decoded[9], decoded[10], decoded[11]);
    let alt = sign_extend_i24(decoded[12], decoded[13], decoded[14]);
    assert_eq!(lat, lat_e7 / 1000, "{}: raw LPP lat", v.id);
    assert_eq!(lon, lon_e7 / 1000, "{}: raw LPP lon", v.id);
    assert_eq!(alt, altitude_m * 100, "{}: raw LPP altitude", v.id);
}

// ── Exhaustive inventory test ──────────────────────────────────────────────

/// Walk every payload-type nibble (0x00..=0x0F), every route-bit pair
/// (0b00..=0b11), and every payload-version pair (bits 7:6), and pin exactly
/// which combinations `Header` accepts. The accepted payload-type set must
/// equal the inventory extracted from the pinned upstream `src/Packet.h`, so
/// a new upstream type (or a MeshCadet parser regression) fails here instead
/// of landing silently.
#[test]
fn payload_type_and_route_inventory_is_exhaustive() {
    let fixture = v118();
    assert_eq!(fixture.schema, 1, "fixture schema");
    assert_eq!(fixture.upstream_tag, "companion-v1.18.0");
    assert_eq!(fixture.upstream_commit, V118_COMMIT);

    let declared_payloads: BTreeSet<u8> = fixture
        .inventory
        .payload_types
        .iter()
        .map(|entry| entry.value)
        .collect();
    let declared_routes: BTreeSet<u8> = fixture
        .inventory
        .route_types
        .iter()
        .map(|entry| entry.value)
        .collect();
    assert_eq!(
        declared_payloads,
        BTreeSet::from([
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0F
        ]),
        "upstream payload-type inventory changed — re-read src/Packet.h and update \
         protocol::PayloadType deliberately"
    );
    assert_eq!(
        declared_routes,
        BTreeSet::from([0x00, 0x01, 0x02, 0x03]),
        "upstream route-type inventory changed"
    );
    assert!(
        fixture
            .inventory
            .payload_types
            .iter()
            .all(|entry| entry.name.starts_with("PAYLOAD_TYPE_")),
        "payload-type inventory names must be upstream #define names"
    );
    assert!(
        fixture
            .inventory
            .route_types
            .iter()
            .all(|entry| entry.name.starts_with("ROUTE_TYPE_")),
        "route-type inventory names must be upstream #define names"
    );

    let mut accepted: BTreeSet<u8> = BTreeSet::new();
    let mut rejected: BTreeSet<u8> = BTreeSet::new();
    for version in 0..=3u8 {
        for payload in 0..=0x0Fu8 {
            for route in 0..=3u8 {
                let byte = (version << 6) | (payload << 2) | route;
                let header = Header(byte);

                assert_eq!(
                    header.version(),
                    version,
                    "header 0x{byte:02X}: version bits must decode as {version}"
                );
                assert_eq!(
                    header.route_type(),
                    Some(route_from_bits(route)),
                    "header 0x{byte:02X}: route bits must decode as {route}"
                );

                match header.payload_type() {
                    Some(decoded) => {
                        assert_eq!(
                            decoded as u8, payload,
                            "header 0x{byte:02X}: payload type must decode as {payload}"
                        );
                        accepted.insert(payload);
                    }
                    None => {
                        rejected.insert(payload);
                    }
                }

                // The shared RX header parser does not validate payload types
                // (upstream forwards unknown types), so every one of these
                // frames must parse and land its payload at the documented
                // offset: 2 for non-transport routes, 6 for transport routes
                // (after the 4 transport-code bytes).
                let (frame, expected_off) = if route == 0 || route == 3 {
                    (vec![byte, 0xDE, 0xAD, 0xBE, 0xEF, 0x00], 6)
                } else {
                    (vec![byte, 0x00], 2)
                };
                let parsed = parse_rx_frame_header(&frame).unwrap_or_else(|e| {
                    panic!("header 0x{byte:02X}: expected frame must parse: {e:?}")
                });
                assert_eq!(parsed.header_byte, byte);
                assert_eq!(
                    parsed.has_transport_codes,
                    route == 0 || route == 3,
                    "header 0x{byte:02X}: transport-code presence"
                );
                assert_eq!(
                    parsed.payload_off, expected_off,
                    "header 0x{byte:02X}: payload offset"
                );
            }
        }
    }

    assert_eq!(
        accepted, declared_payloads,
        "MeshCadet's accepted payload-type set drifted from upstream's src/Packet.h \
         inventory; update protocol::PayloadType (and this fixture via regen-wire-vectors) \
         deliberately"
    );
    assert_eq!(
        rejected,
        BTreeSet::from([0x0C, 0x0D, 0x0E]),
        "the rejected payload types are exactly the upstream-unassigned 0x0C..=0x0E; \
         if upstream assigns one, that must be a deliberate parser change"
    );

    // PathLen is the other half of the framing contract: upstream's default
    // flood encoding is one-byte hash, zero hops (0x00), and reserved hash
    // size 4 is rejected on receive.
    assert_eq!(PathLen(0x00).hash_size(), 1);
    assert_eq!(PathLen(0x00).hop_count(), 0);
    assert!(PathLen(0x00).is_valid());
    assert!(!PathLen(0xC0).is_valid(), "reserved hash size 4");
}

/// Sign-extend a 24-bit big-endian two's-complement value (the LPP encoding).
fn sign_extend_i24(hi: u8, mid: u8, lo: u8) -> i32 {
    let sign = if hi & 0x80 == 0 { 0x00 } else { 0xFF };
    i32::from_be_bytes([sign, hi, mid, lo])
}

fn route_from_bits(bits: u8) -> RouteType {
    match bits {
        0 => RouteType::TransportFlood,
        1 => RouteType::Flood,
        2 => RouteType::Direct,
        3 => RouteType::TransportDirect,
        _ => unreachable!(),
    }
}

/// The fixture's recorded constants must match the mirror in
/// `protocol::constants` — a second, independent check that an upstream
/// framing constant changed (the inventory test above only covers enums).
#[test]
fn mirrored_constants_match_upstream_inventory() {
    let fixture = v118();
    let constants = &fixture.inventory.constants;
    let pairs: &[(&str, u64)] = &[
        ("PUB_KEY_SIZE", protocol::constants::PUB_KEY_SIZE as u64),
        ("SIGNATURE_SIZE", protocol::constants::SIGNATURE_SIZE as u64),
        (
            "MAX_ADVERT_DATA_SIZE",
            protocol::constants::MAX_ADVERT_DATA_SIZE as u64,
        ),
        (
            "CIPHER_KEY_SIZE",
            protocol::constants::CIPHER_KEY_SIZE as u64,
        ),
        (
            "CIPHER_BLOCK_SIZE",
            protocol::constants::CIPHER_BLOCK_SIZE as u64,
        ),
        (
            "CIPHER_MAC_SIZE",
            protocol::constants::CIPHER_MAC_SIZE as u64,
        ),
        (
            "MAX_PACKET_PAYLOAD",
            protocol::constants::MAX_PACKET_PAYLOAD as u64,
        ),
        ("MAX_PATH_SIZE", protocol::constants::MAX_PATH_SIZE as u64),
    ];
    for (name, local) in pairs {
        assert_eq!(
            constants.get(*name),
            Some(local),
            "{name}: protocol::constants no longer matches upstream's src/MeshCore.h"
        );
    }
    assert_eq!(
        constants.get("PATH_HASH_SIZE"),
        Some(&1u64),
        "upstream's 1-byte path hash is the default this fixture's path_len=0x00 encodes"
    );
}

/// Every vector must carry usable provenance and a derivation narrative — the
/// whole point of a checked-in expected frame is that a reviewer can re-derive
/// it from upstream source without trusting MeshCadet.
#[test]
fn every_vector_records_provenance_and_derivation() {
    for fixture in [v118(), v1171()] {
        assert!(!fixture.vectors.is_empty());
        for v in &fixture.vectors {
            assert!(
                !v.provenance.is_empty(),
                "{}: needs at least one upstream source citation",
                v.id
            );
            for citation in &v.provenance {
                assert!(
                    citation.file.starts_with("src/") || citation.file.starts_with("examples/")
                );
                assert!(citation.file.ends_with(".cpp") || citation.file.ends_with(".h"));
                assert!(citation.line > 0, "{}: citation line", v.id);
                assert!(!citation.snippet.is_empty(), "{}: citation snippet", v.id);
                assert!(!citation.note.is_empty(), "{}: citation note", v.id);
            }
            assert!(
                v.derivation.len() > 40,
                "{}: derivation must explain how the frame was composed",
                v.id
            );
            assert!(
                matches!(v.direction.as_str(), "encode" | "decode"),
                "{}: direction",
                v.id
            );
            assert_eq!(
                v.upstream.tag, fixture.upstream_tag,
                "{}: per-vector upstream tag",
                v.id
            );
            assert_eq!(
                v.upstream.commit, fixture.upstream_commit,
                "{}: per-vector upstream commit",
                v.id
            );
            if let Some(note) = &v.compat {
                assert!(
                    note.contains("4-byte") || note.contains("byte-identical"),
                    "{}: compatibility note must state the exact caveat",
                    v.id
                );
            }
        }
    }
}

/// The two target versions must agree on the upstream inventory this repo
/// mirrors. If v1.17.1 and v1.18.0 disagree, the fixture pair is stale.
#[test]
fn both_target_tags_record_the_same_inventory() {
    let a = v118();
    let b = v1171();
    assert_eq!(a.upstream_commit, V118_COMMIT);
    assert_eq!(b.upstream_commit, V1171_COMMIT);
    let values = |inventory: &Inventory| -> Vec<u8> {
        inventory
            .payload_types
            .iter()
            .map(|entry| entry.value)
            .collect()
    };
    assert_eq!(values(&a.inventory), values(&b.inventory));
    assert_eq!(a.inventory.constants, b.inventory.constants);
}
