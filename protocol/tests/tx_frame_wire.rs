// SPDX-License-Identifier: GPL-3.0-only
//! Over-air TX frame regression guard (HIL squawk #4: "TX not received").
//!
//! A HIL investigation audited the
//! full TX path (PHY preset, CAD→TX sequencing, codec, addressing, frame format)
//! and found the firmware byte- and register-correct: the "not received" fault
//! lies downstream of the source (RF-leg or peer-side config), not in the bytes
//! meshcadet emits. This file pins the *complete on-air frame contract* so that
//! conclusion stays true under refactor.
//!
//! The existing `protocol::codec` and `protocol::header` suites pin the pieces
//! (header byte, path_len bit-pack, payload codecs) in isolation. What they do
//! NOT pin is the **composition** the firmware actually puts on the air —
//! `[header(1)][path_len(1)][path(0)][payload...]` — parsed exactly the way the
//! receiver (`firmware/src/main.rs::on_receive`) parses it. That composition is
//! the locus of the "TX leaves clean but companion can't parse it" failure class,
//! so it is the right regression guard for this finding.
//!
//! Each test mirrors a firmware frame builder verbatim:
//!   - `build_test_dm` / `build_ui_dm` / `build_telemetry_reply` → flood TXT_MSG
//!   - `build_grp_txt_beacon` / `build_ui_grp_txt`               → flood GRP_TXT
//!   - `build_ack_frame`                                          → flood ACK
//!
//! and a `parse_frame` helper mirrors `on_receive`'s header/path_len/offset math.

use protocol::{
    compute_ack_hash, decode_dm_payload, decode_grp_txt_var, decode_txt_msg_plaintext,
    encode_dm_payload, encode_grp_txt_var, encode_txt_msg_plaintext, Header, Identity, PathLen,
    PayloadType, RouteType,
};

/// The locked outbound prefix every meshcadet frame carries: 2-byte path-hash
/// mode, 0 hops → `path_len = 0x40`, 0 path bytes, payload at offset 2. This is
/// the exact value `PathLen::new(2, 0)` produces in every firmware builder.
const FLOOD_PATH_LEN: u8 = 0x40;
const PAYLOAD_OFFSET: usize = 2;

/// Receiver-side frame parse, byte-for-byte identical to the header/path_len/
/// offset arithmetic in `firmware/src/main.rs::on_receive`. Returns
/// `(payload_type, payload_slice)`.
fn parse_frame(frame: &[u8]) -> (u8, &[u8]) {
    assert!(
        frame.len() >= 2,
        "frame must carry at least header + path_len"
    );
    let header_byte = frame[0];
    let path_len_byte = frame[1];
    let hash_size = ((path_len_byte >> 6) + 1) as usize;
    let hop_count = (path_len_byte & 0x3F) as usize;
    let path_bytes = hop_count * hash_size;
    let payload_off = 2 + path_bytes;
    assert!(
        frame.len() >= payload_off,
        "frame shorter than encoded path"
    );
    let payload_type = (header_byte >> 2) & 0x0F;
    (payload_type, &frame[payload_off..])
}

/// Flood TXT_MSG (DM): a frame meshcadet emits must (1) carry header 0x09 and
/// path_len 0x40, (2) place the DM payload at offset 2, and (3) round-trip back
/// to the original text + addressing through the receiver's parse path.
#[test]
fn dm_frame_on_air_contract() {
    let sender = Identity::from_seed([0x11u8; 32]);
    let receiver = Identity::from_seed([0x42u8; 32]);
    let shared_tx = sender.ecdh_shared_secret(&receiver.pubkey);
    let shared_rx = receiver.ecdh_shared_secret(&sender.pubkey);

    let text = b"hi from meshcadet";
    let timestamp: u32 = 0x0A1B_2C3D;

    // ── Build the frame exactly as build_test_dm / build_ui_dm do ─────────────
    let mut pt = [0u8; 64];
    let pt_len = encode_txt_msg_plaintext(timestamp, 0, 0, text, &mut pt);

    let mut frame = [0u8; 255];
    frame[0] = Header::new(RouteType::Flood, PayloadType::TxtMsg).0;
    frame[1] = PathLen::new(2, 0).unwrap().0;
    let dm_len = encode_dm_payload(
        &shared_tx,
        receiver.pub_hash(),
        sender.pub_hash(),
        &pt[..pt_len],
        &mut frame[PAYLOAD_OFFSET..],
    );
    let frame_len = PAYLOAD_OFFSET + dm_len;

    // ── Pin the on-air prefix (known-answer) ──────────────────────────────────
    assert_eq!(
        frame[0], 0x09,
        "flood TXT_MSG header must be 0x09 (TXT_MSG<<2 | FLOOD)"
    );
    assert_eq!(
        frame[1], FLOOD_PATH_LEN,
        "path_len must be 0x40 (2-byte hash, 0 hops)"
    );
    assert_eq!(
        frame[PAYLOAD_OFFSET],
        receiver.pub_hash(),
        "DM payload[0] = dest_hash"
    );
    assert_eq!(
        frame[PAYLOAD_OFFSET + 1],
        sender.pub_hash(),
        "DM payload[1] = src_hash"
    );

    // ── Receiver parses the composed frame and recovers everything ────────────
    let (ptype, payload) = parse_frame(&frame[..frame_len]);
    assert_eq!(
        ptype,
        PayloadType::TxtMsg as u8,
        "parsed payload type must be TXT_MSG"
    );

    let mut dec = [0u8; 256];
    let (dest, src, dec_len) = decode_dm_payload(&shared_rx, payload, &mut dec).unwrap();
    assert_eq!(dest, receiver.pub_hash(), "dest hash routes to receiver");
    assert_eq!(src, sender.pub_hash(), "src hash identifies sender");

    let (rx_ts, rx_type, _attempt, text_off) = decode_txt_msg_plaintext(&dec, dec_len).unwrap();
    let text_region = &dec[text_off..dec_len];
    let end = text_region
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(text_region.len());
    assert_eq!(
        rx_ts, timestamp,
        "timestamp survives the full frame round-trip"
    );
    assert_eq!(rx_type, 0, "txt_type survives");
    assert_eq!(
        &text_region[..end],
        text,
        "text survives the full frame round-trip"
    );
}

/// Flood GRP_TXT: header 0x15, path_len 0x40, channel hash as payload[0], and a
/// channel-keyed receiver recovers the text from the composed frame.
#[test]
fn grp_txt_frame_on_air_contract() {
    // Mirror the HIL channel convention used by build_grp_txt_beacon (32-byte secret).
    let channel_secret = [0x6du8; 32]; // 'm'
    let text = b"meshcadet grp beacon";
    let timestamp: u32 = 0xCAFE_BABE;

    let mut frame = [0u8; 255];
    frame[0] = Header::new(RouteType::Flood, PayloadType::GrpTxt).0;
    frame[1] = PathLen::new(2, 0).unwrap().0;
    let n = encode_grp_txt_var(
        &channel_secret,
        timestamp,
        0,
        0,
        text,
        &mut frame[PAYLOAD_OFFSET..],
    );
    let frame_len = PAYLOAD_OFFSET + n;

    assert_eq!(
        frame[0], 0x15,
        "flood GRP_TXT header must be 0x15 (GRP_TXT<<2 | FLOOD)"
    );
    assert_eq!(frame[1], FLOOD_PATH_LEN, "path_len must be 0x40");
    assert_eq!(
        frame[PAYLOAD_OFFSET],
        protocol::channel_hash(&channel_secret),
        "GRP_TXT payload[0] = channel hash",
    );

    let (ptype, payload) = parse_frame(&frame[..frame_len]);
    assert_eq!(
        ptype,
        PayloadType::GrpTxt as u8,
        "parsed payload type must be GRP_TXT"
    );

    let mut pt = [0u8; 256];
    let fields = decode_grp_txt_var(&channel_secret, payload, &mut pt).unwrap();
    assert_eq!(
        fields.timestamp, timestamp,
        "timestamp survives the full frame round-trip"
    );
    let rx_text = &pt[fields.text_offset..fields.text_offset + fields.text_len];
    let end = rx_text
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(rx_text.len());
    assert_eq!(
        &rx_text[..end],
        text,
        "text survives the full frame round-trip"
    );
}

/// Flood ACK: the round-trip-with-ACK acceptance criterion depends on this exact
/// 6-byte frame — header 0x0D, path_len 0x40, then the 4-byte v1.15 ack hash at
/// offset 2, recovered by the receiver's parse path.
#[test]
fn ack_frame_on_air_contract() {
    let sender = Identity::from_seed([0x11u8; 32]);
    let ack_hash = compute_ack_hash(0xDEAD_BEEF, 0, b"hi from meshcadet", &sender.pubkey);

    // Mirror build_ack_frame.
    let mut frame = [0u8; 255];
    frame[0] = Header::new(RouteType::Flood, PayloadType::Ack).0;
    frame[1] = PathLen::new(2, 0).unwrap().0;
    frame[2..6].copy_from_slice(&ack_hash);
    let frame_len = 6;

    assert_eq!(
        frame[0], 0x0D,
        "flood ACK header must be 0x0D (ACK<<2 | FLOOD)"
    );
    assert_eq!(frame[1], FLOOD_PATH_LEN, "path_len must be 0x40");

    let (ptype, payload) = parse_frame(&frame[..frame_len]);
    assert_eq!(
        ptype,
        PayloadType::Ack as u8,
        "parsed payload type must be ACK"
    );
    assert_eq!(payload.len(), 4, "v1.15 ACK payload is exactly 4 bytes");
    assert_eq!(
        payload, &ack_hash,
        "ACK hash recovered intact from the composed frame"
    );
}

/// Cross-guard: the path_len byte every builder emits decodes to the zero-path
/// layout the receiver assumes. If a future change bumps hash_size/hops, the
/// payload offset shifts and the companion silently mis-parses — this pins it.
#[test]
fn flood_path_len_decodes_to_zero_path() {
    let pl = PathLen::new(2, 0).unwrap();
    assert_eq!(pl.0, FLOOD_PATH_LEN);
    assert_eq!(pl.hash_size(), 2);
    assert_eq!(pl.hop_count(), 0);
    assert_eq!(
        pl.path_byte_len(),
        0,
        "0 path bytes ⇒ payload sits at offset 2"
    );
}
