// SPDX-License-Identifier: GPL-3.0-only
//! Provisioning-codec golden-vector generator.
//!
//! `site/provisioner/codec.js` is a hand-ported, pure-JS reimplementation of
//! `protocol::provisioning` (+ the `FRAME_RSP_HISTORY_ENTRY` codec in
//! `protocol::history`) — see `docs/adr/0007-provisioner-codec.md` for why
//! it's pure JS instead of WASM. A hand port has no compiler to catch wire
//! drift, so this module is the guard: it calls the REAL Rust encode/decode
//! functions (the single source of truth) to build a representative set of
//! frames, and emits them as JSON so a JS test harness can assert its own
//! codec reproduces every one byte-for-byte
//! (`site/provisioner/codec.conformance.test.mjs`, driven by
//! `.github/workflows/pages-check.yml`).
//!
//! # Vector shape
//!
//! Each vector is either:
//! - `"direction": "encode"` — a host→device command. `params` are the
//!   logical inputs; the JS codec must call its own `encode*` function with
//!   the same params and reproduce `frame_hex`/`payload_hex` exactly.
//! - `"direction": "decode"` — a device→host response. `frame_hex` is built
//!   by the REAL Rust `encode_*` function, and `expect` is populated from
//!   the REAL Rust `decode_*` function's output (a full encode→decode round
//!   trip through the Rust codec, not hand-copied input values) — so a
//!   vector can never silently encode an encode/decode asymmetry that
//!   doesn't already exist in the Rust codec itself. The JS codec must
//!   decode `frame_hex` and reproduce `expect` field-for-field.
//!
//! Frame-only response types (`RSP_OK`, `RSP_CONTACTS_DONE`,
//! `RSP_CHANNELS_DONE`, `RSP_HISTORY_DONE`) carry `"op": "frame_only"` and no
//! `expect` — there is no payload struct, just an empty-payload frame to
//! round-trip through `decode_frame`.
//!
//! `#[cfg(test)]` below is the "cargo test fixture" half of the mission's
//! Scope item 2: a self-check that every generated vector's `frame_hex`
//! actually re-decodes (via `decode_frame`) to the `frame_type`/`payload_hex`
//! recorded alongside it, catching a bug in this generator itself before it
//! ever reaches the JS side.

use protocol::history::{
    decode_rsp_history_entry, encode_rsp_history_entry, HistoryEntry, HistoryMsgType,
};
use protocol::provisioning::*;

// ── Minimal JSON writer ──────────────────────────────────────────────────────
//
// Hand-rolled rather than pulling in serde_json: this generator emits ~30
// fixed-shape records, and the consumer is a plain-JS `JSON.parse` — no need
// for a derive-based serializer to produce that.

#[derive(Debug, Clone)]
pub enum Json {
    Bool(bool),
    Num(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(&'static str, Json)>),
}

impl Json {
    fn write(&self, out: &mut String) {
        match self {
            Json::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            Json::Num(v) => out.push_str(&v.to_string()),
            Json::Str(v) => {
                out.push('"');
                for c in v.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                        c => out.push(c),
                    }
                }
                out.push('"');
            }
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    out.push_str(k);
                    out.push_str("\":");
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn hexj(bytes: &[u8]) -> Json {
    Json::Str(hex::encode(bytes))
}

fn s(text: &str) -> Json {
    Json::Str(text.to_string())
}

fn n(v: i64) -> Json {
    Json::Num(v)
}

fn b(v: bool) -> Json {
    Json::Bool(v)
}

// ── Vector ────────────────────────────────────────────────────────────────────

struct Vector {
    name: &'static str,
    op: &'static str,
    direction: &'static str,
    frame_type: u8,
    frame_hex: String,
    payload_hex: String,
    params: Option<Json>,
    expect: Option<Json>,
}

impl Vector {
    fn to_json(&self) -> Json {
        let mut pairs = vec![
            ("name", s(self.name)),
            ("op", s(self.op)),
            ("direction", s(self.direction)),
            ("frame_type", n(self.frame_type as i64)),
            ("frame_hex", s(&self.frame_hex)),
            ("payload_hex", s(&self.payload_hex)),
        ];
        if let Some(p) = &self.params {
            pairs.push(("params", p.clone()));
        }
        if let Some(e) = &self.expect {
            pairs.push(("expect", e.clone()));
        }
        Json::Obj(pairs)
    }
}

fn frame_hex_for(frame_type: u8, payload: &[u8]) -> String {
    let mut buf = [0u8; 512];
    let flen = encode_frame(frame_type, payload, &mut buf);
    hex::encode(&buf[..flen])
}

fn encode_vector(
    name: &'static str,
    op: &'static str,
    frame_type: u8,
    payload: &[u8],
    params: Json,
) -> Vector {
    Vector {
        name,
        op,
        direction: "encode",
        frame_type,
        frame_hex: frame_hex_for(frame_type, payload),
        payload_hex: hex::encode(payload),
        params: Some(params),
        expect: None,
    }
}

fn decode_vector(
    name: &'static str,
    op: &'static str,
    frame_type: u8,
    payload: &[u8],
    expect: Json,
) -> Vector {
    Vector {
        name,
        op,
        direction: "decode",
        frame_type,
        frame_hex: frame_hex_for(frame_type, payload),
        payload_hex: hex::encode(payload),
        params: None,
        expect: Some(expect),
    }
}

fn frame_only_vector(name: &'static str, frame_type: u8) -> Vector {
    Vector {
        name,
        op: "frame_only",
        direction: "decode",
        frame_type,
        frame_hex: frame_hex_for(frame_type, &[]),
        payload_hex: String::new(),
        params: None,
        expect: None,
    }
}

// ── Response-payload builders: encode via Rust, then decode via Rust, then
//    serialize the DECODED struct — never the hand-picked inputs — as
//    `expect`. ─────────────────────────────────────────────────────────────

fn rsp_status_vector(name: &'static str, payload_in: RspStatusPayload) -> Vector {
    let mut buf = [0u8; 64];
    let plen = encode_rsp_status(&payload_in, &mut buf);
    let d = decode_rsp_status(&buf[..plen]).expect("golden generator: rsp_status self-decode");
    let expect = Json::Obj(vec![
        ("provisioned", b(d.provisioned)),
        ("pubkey", hexj(&d.pubkey)),
        ("contact_count", n(d.contact_count as i64)),
        ("channel_count", n(d.channel_count as i64)),
        ("gps_has_fix", b(d.gps_has_fix)),
        ("gps_lat_e7", n(d.gps_lat_e7 as i64)),
        ("gps_lon_e7", n(d.gps_lon_e7 as i64)),
        ("gps_fix_age_secs", n(d.gps_fix_age_secs as i64)),
        ("gps_clock_synced", b(d.gps_clock_synced)),
        (
            "gps_clock_sync_age_secs",
            n(d.gps_clock_sync_age_secs as i64),
        ),
        ("battery_percent", n(d.battery_percent as i64)),
        ("battery_charging", b(d.battery_charging)),
        ("battery_raw_mv", n(d.battery_raw_mv as i64)),
        ("battery_held_raw_mv", n(d.battery_held_raw_mv as i64)),
    ]);
    decode_vector(name, "rsp_status", FRAME_RSP_STATUS, &buf[..plen], expect)
}

fn rsp_identity_vector(name: &'static str, pubkey: [u8; 32], device_name: &[u8]) -> Vector {
    let mut buf = [0u8; 64];
    let plen = encode_rsp_identity(&pubkey, device_name, &mut buf);
    let d = decode_rsp_identity(&buf[..plen]).expect("golden generator: rsp_identity self-decode");
    let expect = Json::Obj(vec![
        ("pubkey", hexj(&d.pubkey)),
        ("pub_hash", n(d.pub_hash as i64)),
        (
            "device_name",
            s(std::str::from_utf8(&d.device_name[..d.device_name_len as usize]).unwrap()),
        ),
        ("device_name_len", n(d.device_name_len as i64)),
    ]);
    decode_vector(
        name,
        "rsp_identity",
        FRAME_RSP_IDENTITY,
        &buf[..plen],
        expect,
    )
}

fn rsp_contact_vector(
    name: &'static str,
    index: u8,
    pubkey: [u8; 32],
    telemetry_enable: bool,
    display_name: &[u8],
) -> Vector {
    let mut buf = [0u8; 64];
    let plen = encode_rsp_contact(index, &pubkey, telemetry_enable, display_name, &mut buf);
    let d = decode_rsp_contact(&buf[..plen]).expect("golden generator: rsp_contact self-decode");
    let expect = Json::Obj(vec![
        ("index", n(d.index as i64)),
        ("pubkey", hexj(&d.pubkey)),
        ("telemetry_enable", b(d.telemetry_enable)),
        (
            "display_name",
            s(std::str::from_utf8(&d.display_name[..d.display_name_len as usize]).unwrap()),
        ),
        ("display_name_len", n(d.display_name_len as i64)),
    ]);
    decode_vector(name, "rsp_contact", FRAME_RSP_CONTACT, &buf[..plen], expect)
}

fn rsp_channel_vector(
    name: &'static str,
    index: u8,
    channel_hash: u8,
    key_len: u8,
    primary: bool,
    ch_name: &[u8],
) -> Vector {
    let mut buf = [0u8; 64];
    let plen = encode_rsp_channel(index, channel_hash, key_len, primary, ch_name, &mut buf);
    let d = decode_rsp_channel(&buf[..plen]).expect("golden generator: rsp_channel self-decode");
    let expect = Json::Obj(vec![
        ("index", n(d.index as i64)),
        ("channel_hash", n(d.channel_hash as i64)),
        ("key_len", n(d.key_len as i64)),
        ("primary", b(d.primary)),
        (
            "name",
            s(std::str::from_utf8(&d.name[..d.name_len as usize]).unwrap()),
        ),
        ("name_len", n(d.name_len as i64)),
    ]);
    decode_vector(name, "rsp_channel", FRAME_RSP_CHANNEL, &buf[..plen], expect)
}

fn rsp_error_vector(name: &'static str, error_code: u8, msg: &[u8]) -> Vector {
    let mut buf = [0u8; 128];
    let plen = encode_rsp_error(error_code, msg, &mut buf);
    let d = decode_rsp_error(&buf[..plen]).expect("golden generator: rsp_error self-decode");
    let expect = Json::Obj(vec![
        ("error_code", n(d.error_code as i64)),
        (
            "msg",
            s(std::str::from_utf8(&d.msg[..d.msg_len as usize]).unwrap()),
        ),
        ("msg_len", n(d.msg_len as i64)),
    ]);
    decode_vector(name, "rsp_error", FRAME_RSP_ERROR, &buf[..plen], expect)
}

fn rsp_history_entry_vector(
    name: &'static str,
    index: u8,
    sender_hash: u8,
    msg_type: HistoryMsgType,
    timestamp: u32,
    text: &[u8],
    is_ours: bool,
) -> Vector {
    let mut text_buf = [0u8; protocol::history::MAX_HISTORY_TEXT_LEN];
    text_buf[..text.len()].copy_from_slice(text);
    let entry = HistoryEntry {
        sender_hash,
        msg_type,
        timestamp,
        text: text_buf,
        text_len: text.len() as u8,
    };
    let mut buf = [0u8; protocol::history::MAX_RSP_HISTORY_ENTRY_PAYLOAD];
    let plen = encode_rsp_history_entry(index, &entry, is_ours, &mut buf);
    let (d_index, d_entry, d_is_ours) = decode_rsp_history_entry(&buf[..plen])
        .expect("golden generator: rsp_history_entry self-decode");
    let expect = Json::Obj(vec![
        ("index", n(d_index as i64)),
        ("sender_hash", n(d_entry.sender_hash as i64)),
        ("msg_type", n(d_entry.msg_type as i64)),
        ("timestamp", n(d_entry.timestamp as i64)),
        (
            "text",
            s(std::str::from_utf8(&d_entry.text[..d_entry.text_len as usize]).unwrap()),
        ),
        ("text_len", n(d_entry.text_len as i64)),
        ("is_ours", b(d_is_ours)),
    ]);
    decode_vector(
        name,
        "rsp_history_entry",
        FRAME_RSP_HISTORY_ENTRY,
        &buf[..plen],
        expect,
    )
}

// ── The vector set ───────────────────────────────────────────────────────────

fn build_vectors() -> Vec<Vector> {
    let mut v = Vec::new();

    // ── Commands (encode direction) ─────────────────────────────────────────

    v.push(encode_vector(
        "query_status",
        "query_status",
        FRAME_QUERY_STATUS,
        &[],
        Json::Obj(vec![]),
    ));
    v.push(encode_vector(
        "query_contacts",
        "query_contacts",
        FRAME_QUERY_CONTACTS,
        &[],
        Json::Obj(vec![]),
    ));
    v.push(encode_vector(
        "query_channels",
        "query_channels",
        FRAME_QUERY_CHANNELS,
        &[],
        Json::Obj(vec![]),
    ));

    {
        let pubkey = [0xABu8; 32];
        let name = b"Alice";
        let mut buf = [0u8; 64];
        let plen = encode_add_contact(&pubkey, true, name, &mut buf);
        v.push(encode_vector(
            "add_contact_with_name",
            "add_contact",
            FRAME_ADD_CONTACT,
            &buf[..plen],
            Json::Obj(vec![
                ("pubkey", hexj(&pubkey)),
                ("telemetry_enable", b(true)),
                ("name", s(std::str::from_utf8(name).unwrap())),
            ]),
        ));
    }
    {
        let pubkey = [0x11u8; 32];
        let mut buf = [0u8; 64];
        let plen = encode_add_contact(&pubkey, false, &[], &mut buf);
        v.push(encode_vector(
            "add_contact_no_name",
            "add_contact",
            FRAME_ADD_CONTACT,
            &buf[..plen],
            Json::Obj(vec![
                ("pubkey", hexj(&pubkey)),
                ("telemetry_enable", b(false)),
                ("name", s("")),
            ]),
        ));
    }
    {
        let pubkey = [0x22u8; 32];
        let mut buf = [0u8; 32];
        let plen = encode_del_contact(&pubkey, &mut buf);
        v.push(encode_vector(
            "del_contact",
            "del_contact",
            FRAME_DEL_CONTACT,
            &buf[..plen],
            Json::Obj(vec![("pubkey", hexj(&pubkey))]),
        ));
    }
    {
        let secret = [0x6Du8; 32];
        let name = b"family";
        let mut buf = [0u8; 70];
        let plen = encode_add_channel(&secret, 32, true, name, &mut buf);
        v.push(encode_vector(
            "add_channel_primary_256",
            "add_channel",
            FRAME_ADD_CHANNEL,
            &buf[..plen],
            Json::Obj(vec![
                ("secret", hexj(&secret)),
                ("key_len", n(32)),
                ("primary", b(true)),
                ("name", s(std::str::from_utf8(name).unwrap())),
            ]),
        ));
    }
    {
        let mut secret = [0u8; 32];
        secret[..16].copy_from_slice(&[0xABu8; 16]);
        let name = b"family128";
        let mut buf = [0u8; 70];
        let plen = encode_add_channel(&secret, 16, false, name, &mut buf);
        v.push(encode_vector(
            "add_channel_secondary_128",
            "add_channel",
            FRAME_ADD_CHANNEL,
            &buf[..plen],
            Json::Obj(vec![
                ("secret", hexj(&secret)),
                ("key_len", n(16)),
                ("primary", b(false)),
                ("name", s(std::str::from_utf8(name).unwrap())),
            ]),
        ));
    }
    {
        let secret = [0x33u8; 32];
        let mut buf = [0u8; 32];
        let plen = encode_del_channel(&secret, &mut buf);
        v.push(encode_vector(
            "del_channel",
            "del_channel",
            FRAME_DEL_CHANNEL,
            &buf[..plen],
            Json::Obj(vec![("secret", hexj(&secret))]),
        ));
    }
    {
        let mut buf = [0u8; 8];
        let plen = encode_set_notif_defaults(true, false, &mut buf);
        v.push(encode_vector(
            "set_notif_defaults",
            "set_notif_defaults",
            FRAME_SET_NOTIF_DEFAULTS,
            &buf[..plen],
            Json::Obj(vec![("visual", b(true)), ("audible", b(false))]),
        ));
    }
    {
        let pin = b"1234";
        let mut buf = [0u8; 32];
        let plen = encode_set_pin(pin, &mut buf);
        v.push(encode_vector(
            "set_pin",
            "set_pin",
            FRAME_SET_PIN,
            &buf[..plen],
            Json::Obj(vec![("pin", s(std::str::from_utf8(pin).unwrap()))]),
        ));
    }
    {
        // Empty pin => pin lock disabled (see SetPinPayload::pin_len doc).
        let mut buf = [0u8; 32];
        let plen = encode_set_pin(&[], &mut buf);
        v.push(encode_vector(
            "set_pin_empty_disables_lock",
            "set_pin",
            FRAME_SET_PIN,
            &buf[..plen],
            Json::Obj(vec![("pin", s(""))]),
        ));
    }
    {
        let name = b"T-Deck Alpha";
        let mut buf = [0u8; 40];
        let plen = encode_set_device_name(name, &mut buf);
        v.push(encode_vector(
            "set_device_name",
            "set_device_name",
            FRAME_SET_DEVICE_NAME,
            &buf[..plen],
            Json::Obj(vec![("name", s(std::str::from_utf8(name).unwrap()))]),
        ));
    }
    {
        // Empty name => clear the stored name (see SetDeviceNamePayload::name_len doc).
        let mut buf = [0u8; 40];
        let plen = encode_set_device_name(&[], &mut buf);
        v.push(encode_vector(
            "set_device_name_empty_clears",
            "set_device_name",
            FRAME_SET_DEVICE_NAME,
            &buf[..plen],
            Json::Obj(vec![("name", s(""))]),
        ));
    }
    v.push(encode_vector(
        "commit_provisioning",
        "commit_provisioning",
        FRAME_COMMIT_PROVISIONING,
        &[],
        Json::Obj(vec![]),
    ));
    v.push(encode_vector(
        "export_history",
        "export_history",
        FRAME_EXPORT_HISTORY,
        &[],
        Json::Obj(vec![]),
    ));
    v.push(encode_vector(
        "clear_history",
        "clear_history",
        FRAME_CLEAR_HISTORY,
        &[],
        Json::Obj(vec![]),
    ));

    // ── Responses (decode direction) ────────────────────────────────────────

    v.push(frame_only_vector("rsp_ok", FRAME_RSP_OK));
    v.push(rsp_error_vector("rsp_error", 7, b"bad pin"));
    v.push(rsp_error_vector("rsp_error_empty_msg", 1, b""));

    v.push(rsp_status_vector(
        "rsp_status_unprovisioned_zero",
        RspStatusPayload {
            provisioned: false,
            pubkey: [0x55u8; 32],
            contact_count: 0,
            channel_count: 0,
            gps_has_fix: false,
            gps_lat_e7: 0,
            gps_lon_e7: 0,
            gps_fix_age_secs: 0,
            gps_clock_synced: false,
            gps_clock_sync_age_secs: 0,
            battery_percent: 0,
            battery_charging: false,
            battery_raw_mv: 0,
            battery_held_raw_mv: 0,
        },
    ));
    v.push(rsp_status_vector(
        "rsp_status_provisioned_with_gps_and_battery",
        RspStatusPayload {
            provisioned: true,
            pubkey: [0xAAu8; 32],
            contact_count: 2,
            channel_count: 1,
            gps_has_fix: true,
            gps_lat_e7: -481_173_000,
            gps_lon_e7: 115_166_667,
            gps_fix_age_secs: 42,
            gps_clock_synced: true,
            gps_clock_sync_age_secs: 300,
            battery_percent: 76,
            battery_charging: true,
            battery_raw_mv: 4142,
            battery_held_raw_mv: 3775,
        },
    ));

    v.push(rsp_identity_vector(
        "rsp_identity_no_name",
        [0xCCu8; 32],
        b"",
    ));
    v.push(rsp_identity_vector(
        "rsp_identity_with_name",
        [0xDDu8; 32],
        b"T-Deck Alpha",
    ));

    v.push(rsp_contact_vector(
        "rsp_contact",
        3,
        [0x44u8; 32],
        true,
        b"Bob",
    ));
    v.push(frame_only_vector(
        "rsp_contacts_done",
        FRAME_RSP_CONTACTS_DONE,
    ));

    v.push(rsp_channel_vector(
        "rsp_channel",
        1,
        0x7F,
        32,
        true,
        b"family",
    ));
    v.push(frame_only_vector(
        "rsp_channels_done",
        FRAME_RSP_CHANNELS_DONE,
    ));

    v.push(rsp_history_entry_vector(
        "rsp_history_entry_received_dm",
        0,
        0x9A,
        HistoryMsgType::Dm,
        1_700_000_000,
        b"hello world",
        false,
    ));
    v.push(rsp_history_entry_vector(
        "rsp_history_entry_sent_grptxt_empty_text",
        5,
        0x33,
        HistoryMsgType::GrpTxt,
        1_700_000_100,
        b"",
        true,
    ));
    v.push(frame_only_vector(
        "rsp_history_done",
        FRAME_RSP_HISTORY_DONE,
    ));

    v
}

/// Render every golden vector as a pretty-printed JSON array.
pub fn golden_vectors_json() -> String {
    let vectors = build_vectors();
    let mut out = String::from("[\n");
    for (i, vec) in vectors.iter().enumerate() {
        out.push_str("  ");
        vec.to_json().write(&mut out);
        if i + 1 < vectors.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("]\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every vector's `frame_hex` must decode (via the REAL `decode_frame`)
    /// back to exactly its own recorded `frame_type`/`payload_hex` — this is
    /// the generator's own self-check, independent of the JS side.
    #[test]
    fn golden_vectors_are_internally_consistent() {
        let vectors = build_vectors();
        assert!(!vectors.is_empty(), "generator produced zero vectors");
        for vec in &vectors {
            let frame_bytes = hex::decode(&vec.frame_hex)
                .unwrap_or_else(|e| panic!("{}: frame_hex is not valid hex: {e}", vec.name));
            let (frame_type, payload) = decode_frame(&frame_bytes)
                .unwrap_or_else(|e| panic!("{}: frame_hex does not decode: {e:?}", vec.name));
            assert_eq!(
                frame_type, vec.frame_type,
                "{}: frame_type mismatch",
                vec.name
            );
            assert_eq!(
                hex::encode(payload),
                vec.payload_hex,
                "{}: payload_hex mismatch",
                vec.name
            );
        }
    }

    /// Every vector name is unique — the JS harness indexes by name for
    /// failure reporting, and a duplicate would silently shadow a case.
    #[test]
    fn golden_vector_names_are_unique() {
        let vectors = build_vectors();
        let mut names: Vec<&str> = vectors.iter().map(|v| v.name).collect();
        names.sort_unstable();
        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len(), "duplicate vector name found");
    }

    /// Sanity check on the emitted JSON's gross shape (matching bracket
    /// counts) — a cheap guard against a malformed writer, independent of
    /// actually parsing JSON (this crate has no JSON parser).
    #[test]
    fn golden_vectors_json_is_balanced() {
        let json = golden_vectors_json();
        assert_eq!(json.matches('{').count(), json.matches('}').count());
        assert_eq!(json.matches('[').count(), json.matches(']').count());
        assert!(json.trim_start().starts_with('['));
    }
}
