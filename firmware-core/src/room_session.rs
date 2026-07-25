// SPDX-License-Identifier: GPL-3.0-only
//! Room-server client session state machine — pure logic gluing
//! `protocol::room`'s wire codec to a room contact's persisted
//! [`crate::config_store::RoomExtra`] and its conversation history, with no
//! radio or flash I/O of its own. `firmware/src/main.rs` owns the hardware
//! half (TX enqueue, RX dispatch, NVS persistence) and calls into this
//! module for every decision — this is the "MILESTONE-1 WALKING SKELETON"
//! for `meshcadet-room-server-support`: login, learn the granted permission
//! and (on a flood reply) the mesh route, then decode/ACK/dedup/append
//! inbound pushes. Posting, permission-gated compose, keep-alive, and
//! notification-suppression parity are milestone 2 — out of scope here.
//!
//! # The two login-reply forms
//!
//! `simple_room_server`'s `ANON_REQ` login reply can arrive two ways
//! (`protocol::room`'s module doc cites the exact `MyMesh.cpp` lines):
//! - a direct `RESPONSE` datagram — [`decode_login_response_datagram`];
//! - a flood-routed login's bundled `PathExtra::Response`, riding inside a
//!   PATH-return packet — [`decode_login_path_return`]. **This is the case
//!   that actually happens on first contact**: the device has no learned
//!   route to the room server yet, so the `ANON_REQ` itself must be
//!   flood-routed, and the reply comes back the same way. Only the
//!   PATH-return form teaches a mesh route (`RoomLoginOutcome::out_path`);
//!   the direct form implies the sender already had one.
//!
//! # The ACK is non-negotiable
//!
//! The server pushes one post at a time and will not advance `sync_since` or
//! push again until it sees the client's ACK (`MyMesh.cpp:53-110`) — a client
//! that fails to ACK stalls its own sync permanently. [`handle_room_push`]
//! therefore always returns an `ack_hash` the caller MUST transmit, even for
//! a push it recognises as an already-seen duplicate (`entry: None`) —
//! withholding the ACK on a dedup hit would recreate the exact stall this
//! invariant exists to prevent; the dedup is scoped only to "don't
//! double-append history", not "don't re-ACK".
//!
//! # Dedup is content-level, not frame-level
//!
//! `firmware-core::dispatcher::DuplicateFilter` already dedups inbound
//! frames by `protocol::dedup::packet_dedup_key` (payload-type + payload
//! bytes). That catches a byte-identical repeat, but a room-server retry of
//! an unacked push re-encodes with a bumped `attempt` counter
//! (`MyMesh.cpp`'s retry path), which changes the AES-ECB ciphertext and so
//! the frame-level dedup key — even though the logical post is identical.
//! [`handle_room_push`] instead dedups by `(timestamp, text)` against the
//! room conversation's already-known history entries, exactly the content
//! identity `HistoryStore::append_conversation` would otherwise duplicate.

use protocol::codec::{decode_dm_payload, decode_path_return, CodecError, PathExtra};
use protocol::constants::MAX_PATH_SIZE;
use protocol::history::{HistoryEntry, HistoryMsgType, MAX_HISTORY_TEXT_LEN};
use protocol::room::{decode_login_response, decode_room_push, room_push_ack_hash};
pub use protocol::room::{LoginResponse, RoomCodecError, RoomPermission, RoomPush};
use protocol::{Header, PathLen, PayloadType, RouteType};

use crate::config_store::RoomExtra;

// ── Errors ───────────────────────────────────────────────────────────────────

/// Errors from this module's decode helpers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomSessionError {
    /// Wraps a lower-level DM/PATH-return codec error (MAC mismatch,
    /// truncated payload, …).
    Codec(CodecError),
    /// Wraps a `protocol::room` decode error (truncated login response, a
    /// non-OK login code, …).
    Room(RoomCodecError),
    /// [`decode_login_path_return`] received a well-formed PATH-return with
    /// no bundled `RESPONSE` extra (`PathExtra::Ack` or `PathExtra::None`) —
    /// not a login reply at all.
    NotLoginReply,
}

impl From<CodecError> for RoomSessionError {
    fn from(e: CodecError) -> Self {
        RoomSessionError::Codec(e)
    }
}

impl From<RoomCodecError> for RoomSessionError {
    fn from(e: RoomCodecError) -> Self {
        RoomSessionError::Room(e)
    }
}

// ── Login: encode ────────────────────────────────────────────────────────────

/// Generous capacity for [`encode_room_login_frame`]'s output: 2-byte outer
/// header/path_len + `1(dest_hash) + 32(sender_pubkey) + 2(HMAC) + 32(AES-ECB
/// ciphertext, comfortably above the `ceil_16(8 + 15 + 1)` = 32-byte worst
/// case)`.
pub const MAX_LOGIN_FRAME_LEN: usize = 2 + 1 + 32 + 2 + 32;

/// Encode the full outbound frame for a room login: `[header][path_len =
/// 0 hops][ANON_REQ payload]`. Always flood-routed (`RouteType::Flood`) — a
/// room contact has no learned mesh route on first contact (`out_path`
/// starts empty), and this module does not implement re-login over a learned
/// route (that belongs to milestone 2's keep-alive scheduler).
///
/// `sync_since` should be the room's persisted watermark (`RoomExtra::sync_since`)
/// so a resumed session does not re-drain posts the server already delivered.
///
/// Returns the number of bytes written to `out` (`out` must be at least
/// [`MAX_LOGIN_FRAME_LEN`] bytes).
pub fn encode_room_login_frame(
    shared_secret: &[u8; 32],
    dest_hash: u8,
    sender_pubkey: &[u8; 32],
    timestamp: u32,
    sync_since: u32,
    password: &[u8],
    out: &mut [u8],
) -> usize {
    out[0] = Header::new(RouteType::Flood, PayloadType::AnonReq).0;
    out[1] = PathLen::new(2, 0).map(|p| p.0).unwrap_or(0x40);
    let n = protocol::room::encode_anon_req_login(
        shared_secret,
        dest_hash,
        sender_pubkey,
        timestamp,
        sync_since,
        password,
        &mut out[2..],
    );
    2 + n
}

// ── Login: decode ────────────────────────────────────────────────────────────

/// Outcome of a successful room login, ready to persist into the room's
/// [`RoomExtra`] via [`apply_login_outcome`].
///
/// **Do not assume the permission.** The guest password grants
/// [`RoomPermission::ReadWrite`], but a wrong password on an
/// `allow_read_only` server silently grants [`RoomPermission::Guest`]
/// (read-only) instead — this struct carries whatever the server actually
/// said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoomLoginOutcome {
    /// The granted role, as reported by the server's `RESPONSE` — never
    /// assumed from which password was sent.
    pub permissions: RoomPermission,
    /// `Some((path, path_byte_count))` when this reply taught a mesh route
    /// (the flood PATH-return leg). `None` for a direct `RESPONSE` datagram —
    /// no new path to learn.
    pub out_path: Option<([u8; MAX_PATH_SIZE], usize)>,
}

/// Decode a direct `RESPONSE` datagram login reply
/// (`Header::payload_type() == PayloadType::Response`).
pub fn decode_login_response_datagram(
    shared_secret: &[u8; 32],
    payload: &[u8],
) -> Result<RoomLoginOutcome, RoomSessionError> {
    let mut pt = [0u8; 32];
    let (_dest_hash, _src_hash, pt_len) = decode_dm_payload(shared_secret, payload, &mut pt)?;
    let resp = decode_login_response(&pt[..pt_len])?;
    Ok(RoomLoginOutcome {
        permissions: resp.permissions,
        out_path: None,
    })
}

/// Decode a flood-routed login reply bundled inside a PATH-return packet
/// (`Header::payload_type() == PayloadType::Path`) — the case that actually
/// happens on first contact. Fails with [`RoomSessionError::NotLoginReply`]
/// if the PATH-return carries no bundled `RESPONSE` extra (an ordinary
/// bundled-ACK PATH-return, or none at all) — use
/// `firmware::handle_path_return`'s existing ACK handling for those.
pub fn decode_login_path_return(
    shared_secret: &[u8; 32],
    payload: &[u8],
) -> Result<RoomLoginOutcome, RoomSessionError> {
    let mut pt = [0u8; 256];
    let (_dest_hash, _src_hash, rp) = decode_path_return(shared_secret, payload, &mut pt)?;
    match rp.extra {
        PathExtra::Response(bundled) => {
            let resp = decode_login_response(&bundled)?;
            Ok(RoomLoginOutcome {
                permissions: resp.permissions,
                out_path: Some((rp.path, rp.path_byte_count)),
            })
        }
        _ => Err(RoomSessionError::NotLoginReply),
    }
}

/// Apply a decoded login outcome to a room's persisted extras: the granted
/// permission is recorded unconditionally (never assumed — see
/// [`RoomLoginOutcome`]'s doc), and `out_path`/`out_path_len` are updated
/// only when this reply actually taught a route (`out_path: Some(..)`) — a
/// direct `RESPONSE` reply teaches no new path, so an already-learned one is
/// left in place rather than blanked back to empty.
///
/// The caller is responsible for persisting the mutated config afterward
/// (same contract [`crate::config_store::ProvisionedConfig::room_extra_mut`]'s
/// own doc states).
pub fn apply_login_outcome(extra: &mut RoomExtra, outcome: &RoomLoginOutcome) {
    extra.permissions = outcome.permissions as u8;
    if let Some((path, path_byte_count)) = outcome.out_path {
        extra.out_path = path;
        // `path_byte_count` is `ReturnPath::path_byte_count`, itself bounded
        // to `path.len() == MAX_PATH_SIZE == 64` by `decode_path_return_plaintext`;
        // `.min` is a defensive belt against `out_path_len` (a `u8`) ever
        // narrowing unsafely if either bound changes independently later.
        extra.out_path_len = path_byte_count.min(u8::MAX as usize) as u8;
    }
}

// ── Inbound push: decode + ACK + dedup ──────────────────────────────────────

/// Outcome of successfully decoding an inbound room push.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoomPushOutcome {
    /// The 4-byte ACK the caller MUST transmit back to the room server —
    /// unconditionally, even when `entry` is `None`. See this module's doc
    /// ("The ACK is non-negotiable").
    pub ack_hash: [u8; 4],
    /// The post's original timestamp — the new `sync_since` watermark to
    /// persist once the caller's ACK transmission actually lands.
    pub post_ts: u32,
    /// `Some(entry)`, ready to append to this room's conversation history,
    /// unless `(timestamp, text)` was already present in the `recent_history`
    /// slice passed to [`handle_room_push`] — a re-delivered push the server
    /// retried because it never heard the first ACK. `None` in that case, so
    /// the caller does not double-append (but still sends `ack_hash` above).
    pub entry: Option<HistoryEntry>,
}

/// Decode an inbound room push (`TXT_MSG`, `TXT_TYPE_SIGNED_PLAIN`), compute
/// its ACK hash, and decide — by content, against `recent_history` — whether
/// it duplicates an already-known post.
///
/// `conv_hash` is the room's conversation key (its contact hash — the same
/// value `firmware::history_store` keys the region by and the UI's
/// `messages` map keys by). `recent_history` should be this same
/// conversation's already-known entries (e.g. the flash store's live tail);
/// only `(timestamp, text)` are compared, matching what a room server retry
/// preserves across a bumped `attempt` counter (see this module's doc).
pub fn handle_room_push(
    shared_secret: &[u8; 32],
    payload: &[u8],
    client_pubkey: &[u8; 32],
    conv_hash: u8,
    recent_history: &[HistoryEntry],
) -> Result<RoomPushOutcome, RoomSessionError> {
    let mut pt = [0u8; 256];
    let (_dest_hash, _src_hash, push) = decode_room_push(shared_secret, payload, &mut pt)?;

    let push_body = push.push_body(&pt);
    let ack_hash = room_push_ack_hash(push.post_ts, push.attempt, push_body, client_pubkey);
    let text = &pt[push.text_offset..push.text_offset + push.text_len];

    let entry = if is_duplicate_post(recent_history, push.post_ts, text) {
        None
    } else {
        Some(build_history_entry(conv_hash, push.post_ts, text))
    };

    Ok(RoomPushOutcome {
        ack_hash,
        post_ts: push.post_ts,
        entry,
    })
}

/// A room post is a duplicate of one already in `recent`, by `(timestamp,
/// text)` — see [`handle_room_push`]'s doc for why this is content-level,
/// not the radio's frame-level dedup ring.
fn is_duplicate_post(recent: &[HistoryEntry], post_ts: u32, text: &[u8]) -> bool {
    recent.iter().any(|e| {
        e.timestamp == post_ts
            && e.text_len as usize == text.len()
            && &e.text[..text.len().min(e.text.len())] == text
    })
}

fn build_history_entry(conv_hash: u8, post_ts: u32, text: &[u8]) -> HistoryEntry {
    let text_len = text.len().min(MAX_HISTORY_TEXT_LEN);
    let mut buf = [0u8; MAX_HISTORY_TEXT_LEN];
    buf[..text_len].copy_from_slice(&text[..text_len]);
    HistoryEntry {
        sender_hash: conv_hash,
        msg_type: HistoryMsgType::Dm,
        timestamp: post_ts,
        text: buf,
        text_len: text_len as u8,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::identity::Identity;
    use protocol::room::{
        encode_anon_req_login, encode_room_post, room_post_ack_hash, RoomServerDouble,
    };

    fn make_pair() -> (Identity, Identity) {
        let client = Identity::from_seed([0x11u8; 32]);
        let server = Identity::from_seed([0x12u8; 32]);
        (client, server)
    }

    /// Login through the double via a *direct* `RESPONSE` datagram (as if the
    /// server already knew a route to us), then decode via this module's
    /// `decode_login_response_datagram`.
    fn login_direct(
        double: &mut RoomServerDouble,
        client: &Identity,
        server: &Identity,
        password: &[u8],
        ts: u32,
    ) -> RoomLoginOutcome {
        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut anon_req = [0u8; 128];
        let n = encode_anon_req_login(
            &shared,
            server.pub_hash(),
            &client.pubkey,
            ts,
            0,
            password,
            &mut anon_req,
        );
        let mut reply_wire = [0u8; 64];
        let reply_len = double
            .handle_login(&anon_req[..n], &mut reply_wire)
            .expect("login must succeed");
        decode_login_response_datagram(&shared, &reply_wire[..reply_len]).unwrap()
    }

    // ── Login: encode ────────────────────────────────────────────────────────

    #[test]
    fn encode_room_login_frame_layout() {
        let (client, server) = make_pair();
        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut out = [0u8; MAX_LOGIN_FRAME_LEN];
        let n = encode_room_login_frame(
            &shared,
            server.pub_hash(),
            &client.pubkey,
            1000,
            42,
            b"guest-pw",
            &mut out,
        );
        assert_eq!(
            out[0],
            Header::new(RouteType::Flood, PayloadType::AnonReq).0,
            "login frame must be flood-routed ANON_REQ"
        );
        assert_eq!(out[1], PathLen::new(2, 0).unwrap().0, "0-hop outbound path");
        assert_eq!(
            out[2],
            server.pub_hash(),
            "ANON_REQ dest_hash follows the header"
        );
        assert!(
            n > 2 + 33,
            "payload must carry the inline pubkey + HMAC + ciphertext"
        );
    }

    // ── Login: decode both reply forms ──────────────────────────────────────

    #[test]
    fn decode_login_response_datagram_records_permission_no_out_path() {
        let (client, server) = make_pair();
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);
        let outcome = login_direct(&mut double, &client, &server, b"guest-pw", 1000);
        assert_eq!(outcome.permissions, RoomPermission::ReadWrite);
        assert_eq!(
            outcome.out_path, None,
            "direct RESPONSE teaches no new path"
        );
    }

    #[test]
    fn decode_login_path_return_records_permission_and_learns_out_path() {
        // Build a synthetic PATH-return bundling a RESPONSE, mirroring
        // `protocol::room`'s own `decode_path_return_accepts_bundled_response_extra`
        // golden vector, but exercised through this module's login decoder —
        // the flood-login case that actually happens on first contact.
        let (client, server) = make_pair();
        let shared = client.ecdh_shared_secret(&server.pubkey);

        let mut response = [0u8; 13];
        response[4] = protocol::room::RESP_SERVER_LOGIN_OK;
        response[7] = RoomPermission::Admin as u8;

        let mut inner = [0u8; 32];
        inner[0] = 0x01; // path_len: 1-byte hashes (bits[7:6]=00), 1 hop (bits[5:0]=1)
        inner[1] = 0xAB; // the one learned hop
        inner[2] = 0x01; // extra_type = PAYLOAD_TYPE_RESPONSE
        inner[3..16].copy_from_slice(&response);

        let mut wire = [0u8; 256];
        let n = protocol::codec::encode_dm_payload(
            &shared,
            client.pub_hash(),
            server.pub_hash(),
            &inner[..16],
            &mut wire,
        );

        let outcome = decode_login_path_return(&shared, &wire[..n]).unwrap();
        assert_eq!(outcome.permissions, RoomPermission::Admin);
        let (path, len) = outcome.out_path.expect("PATH-return must teach a route");
        assert_eq!(len, 1);
        assert_eq!(path[0], 0xAB);
    }

    #[test]
    fn decode_login_path_return_rejects_bundled_ack_as_not_a_login_reply() {
        // An ordinary bundled-ACK PATH-return (the pre-existing DM-ack case)
        // must not be mistaken for a login reply.
        let (client, server) = make_pair();
        let shared = client.ecdh_shared_secret(&server.pubkey);

        let mut inner = [0u8; 16];
        inner[0] = 0x40; // 0 hops
        inner[1] = 0x03; // extra_type = ACK
        inner[2..6].copy_from_slice(&[1, 2, 3, 4]);

        let mut wire = [0u8; 256];
        let n = protocol::codec::encode_dm_payload(
            &shared,
            client.pub_hash(),
            server.pub_hash(),
            &inner[..6],
            &mut wire,
        );

        assert_eq!(
            decode_login_path_return(&shared, &wire[..n]),
            Err(RoomSessionError::NotLoginReply)
        );
    }

    #[test]
    fn granted_permission_is_read_from_response_not_assumed_from_password() {
        // Non-negotiable acceptance bullet: a wrong password on an
        // allow_read_only server must decode as Guest, never ReadWrite —
        // the exact confusion `RoomPermission`'s type exists to prevent.
        let (client, server) = make_pair();
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", true);
        let outcome = login_direct(&mut double, &client, &server, b"totally-wrong", 1000);
        assert_eq!(outcome.permissions, RoomPermission::Guest);
    }

    #[test]
    fn apply_login_outcome_persists_permission_and_conditionally_out_path() {
        let mut extra = RoomExtra::EMPTY;
        extra.out_path[0] = 0xCC;
        extra.out_path_len = 1;

        // A direct-datagram outcome (no new path) must not blank an
        // already-learned out_path.
        apply_login_outcome(
            &mut extra,
            &RoomLoginOutcome {
                permissions: RoomPermission::ReadWrite,
                out_path: None,
            },
        );
        assert_eq!(extra.permissions, RoomPermission::ReadWrite as u8);
        assert_eq!(
            extra.out_path_len, 1,
            "existing out_path must survive a no-path reply"
        );
        assert_eq!(extra.out_path[0], 0xCC);

        // A flood PATH-return outcome overwrites it with the newly learned path.
        let mut new_path = [0u8; MAX_PATH_SIZE];
        new_path[0] = 0xEE;
        new_path[1] = 0xFF;
        apply_login_outcome(
            &mut extra,
            &RoomLoginOutcome {
                permissions: RoomPermission::Guest,
                out_path: Some((new_path, 2)),
            },
        );
        assert_eq!(extra.permissions, RoomPermission::Guest as u8);
        assert_eq!(extra.out_path_len, 2);
        assert_eq!(&extra.out_path[..2], &[0xEE, 0xFF]);
    }

    // ── Full session: login → 3 pushes → 3 ACKs → 3 history entries ────────

    #[test]
    fn full_session_three_pushes_three_acks_three_history_entries_in_order_no_dup() {
        let (client, server) = make_pair();
        let other_author = Identity::from_seed([0x13u8; 32]);
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);

        let outcome = login_direct(&mut double, &client, &server, b"guest-pw", 1000);
        assert_eq!(outcome.permissions, RoomPermission::ReadWrite);

        double.seed_post(&other_author.pubkey, 1001, b"post one");
        double.seed_post(&other_author.pubkey, 1002, b"post two");
        double.seed_post(&other_author.pubkey, 1003, b"post three");

        let shared = client.ecdh_shared_secret(&server.pubkey);
        let conv_hash = server.pub_hash();
        let mut history: Vec<HistoryEntry> = Vec::new();
        let expected: [&[u8]; 3] = [b"post one", b"post two", b"post three"];

        for expected_text in expected {
            let mut wire = [0u8; 256];
            let n = double
                .push_next(&client.pubkey, &mut wire)
                .expect("an eligible post must be pushed");

            let outcome =
                handle_room_push(&shared, &wire[..n], &client.pubkey, conv_hash, &history)
                    .expect("push must decode");
            let entry = outcome.entry.expect("a fresh push must produce an entry");
            assert_eq!(&entry.text[..entry.text_len as usize], expected_text);
            history.push(entry);

            assert!(
                double.handle_ack(&client.pubkey, &outcome.ack_hash),
                "our computed ack_hash must match the double's pending push"
            );
        }

        assert_eq!(
            history.len(),
            3,
            "no duplicates: exactly one entry per post"
        );
        assert_eq!(
            history.iter().map(|e| e.timestamp).collect::<Vec<_>>(),
            vec![1001, 1002, 1003],
            "entries appended in delivery order"
        );
        // The double's own sync is fully drained: no 4th push.
        let mut wire = [0u8; 256];
        assert!(double.push_next(&client.pubkey, &mut wire).is_none());
    }

    // ── ACK is non-negotiable: suppressing it stalls the double's sync ─────

    #[test]
    fn suppressing_the_ack_stalls_the_sync() {
        // Regression guard for the campaign's non-negotiable ACK invariant:
        // if the caller decodes a push via `handle_room_push` but never feeds
        // `ack_hash` back to the server (the exact bug "forget to transmit
        // the ACK" would cause), the double's drip must never advance to the
        // next post. This test fails if a future change removes the ACK step
        // from the exercised call sequence below.
        let (client, server) = make_pair();
        let other_author = Identity::from_seed([0x14u8; 32]);
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);
        login_direct(&mut double, &client, &server, b"guest-pw", 1000);

        double.seed_post(&other_author.pubkey, 2001, b"first");
        double.seed_post(&other_author.pubkey, 2002, b"second");

        let shared = client.ecdh_shared_secret(&server.pubkey);
        let conv_hash = server.pub_hash();

        let mut wire1 = [0u8; 256];
        let n1 = double
            .push_next(&client.pubkey, &mut wire1)
            .expect("first push must succeed");
        let outcome1 = handle_room_push(&shared, &wire1[..n1], &client.pubkey, conv_hash, &[])
            .expect("first push must decode");
        assert!(outcome1.entry.is_some());

        // Do NOT call double.handle_ack here — simulates a client that
        // decoded the push but failed to transmit its ACK.
        let mut wire2 = [0u8; 256];
        assert!(
            double.push_next(&client.pubkey, &mut wire2).is_none(),
            "sync must stall: no ACK was ever sent for the first push"
        );

        // Now actually send the ACK our outcome computed: the stall clears.
        assert!(double.handle_ack(&client.pubkey, &outcome1.ack_hash));
        let mut wire3 = [0u8; 256];
        assert!(
            double.push_next(&client.pubkey, &mut wire3).is_some(),
            "sending the withheld ACK must unstall the next push"
        );
    }

    // ── Dedup against existing history: a replayed push is not re-appended ──

    #[test]
    fn replayed_push_still_acks_but_is_not_re_appended() {
        // A room server retries an unacked push with a bumped `attempt`
        // counter (MyMesh.cpp's retry path) — a different frame on the wire,
        // same logical post. `handle_room_push` must recognise the content
        // duplicate against `recent_history` and skip the second append,
        // while still returning an ack_hash (the ACK is not conditioned on
        // dedup — see this module's doc).
        let (client, server) = make_pair();
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);
        login_direct(&mut double, &client, &server, b"guest-pw", 1000);

        let shared = client.ecdh_shared_secret(&server.pubkey);
        let conv_hash = server.pub_hash();

        // Simulate the server's first attempt (attempt=0) and a retry
        // (attempt=1) of the identical logical post — built directly (not via
        // the double, which doesn't model retries) since only the content
        // dedup behavior is under test here.
        let mut plaintext = [0u8; 32];
        let post_ts: u32 = 5000;
        let author_prefix = [0x01, 0x02, 0x03, 0x04];
        let text = b"hello room";
        plaintext[0..4].copy_from_slice(&post_ts.to_le_bytes());
        plaintext[5..9].copy_from_slice(&author_prefix);
        plaintext[9..9 + text.len()].copy_from_slice(text);

        let mut wire_attempt0 = [0u8; 256];
        plaintext[4] = protocol::room::TXT_TYPE_SIGNED_PLAIN << 2; // attempt=0
        let n0 = protocol::codec::encode_dm_payload(
            &shared,
            client.pub_hash(),
            server.pub_hash(),
            &plaintext[..9 + text.len()],
            &mut wire_attempt0,
        );

        let mut wire_attempt1 = [0u8; 256];
        plaintext[4] = (protocol::room::TXT_TYPE_SIGNED_PLAIN << 2) | 1; // attempt=1 (retry)
        let n1 = protocol::codec::encode_dm_payload(
            &shared,
            client.pub_hash(),
            server.pub_hash(),
            &plaintext[..9 + text.len()],
            &mut wire_attempt1,
        );
        assert_ne!(
            &wire_attempt0[..n0],
            &wire_attempt1[..n1],
            "a bumped attempt counter must change the frame bytes"
        );

        let mut history: Vec<HistoryEntry> = Vec::new();
        let first = handle_room_push(
            &shared,
            &wire_attempt0[..n0],
            &client.pubkey,
            conv_hash,
            &history,
        )
        .unwrap();
        history.push(first.entry.expect("first delivery must produce an entry"));

        let retry = handle_room_push(
            &shared,
            &wire_attempt1[..n1],
            &client.pubkey,
            conv_hash,
            &history,
        )
        .unwrap();
        assert!(
            retry.entry.is_none(),
            "a content-identical retry must not produce a second history entry"
        );
        // The retry's ack_hash legitimately differs from the first attempt's —
        // `room_push_ack_hash` folds the `attempt` counter into the hashed
        // type byte (`MyMesh.cpp:71`), so each retransmit attempt has its own
        // ack_hash the server recomputes to match. The ACK is still
        // unconditional either way: this test's point is that dedup gates
        // the HISTORY APPEND only, never the ACK itself.
        assert_ne!(retry.ack_hash, [0u8; 4]);
    }

    // ── Permission byte is read from the response, never assumed ───────────

    #[test]
    fn guest_permission_from_wrong_password_is_not_confused_with_read_write() {
        let (client, server) = make_pair();
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", true);
        let outcome = login_direct(
            &mut double,
            &client,
            &server,
            b"not-the-guest-password",
            1000,
        );
        assert_eq!(
            outcome.permissions,
            RoomPermission::Guest,
            "wrong password on an allow_read_only server must decode as Guest, not ReadWrite"
        );

        let mut extra = RoomExtra::EMPTY;
        apply_login_outcome(&mut extra, &outcome);
        assert_eq!(
            extra.permission(),
            RoomPermission::Guest,
            "the stored RoomExtra must reflect exactly what the server granted"
        );
    }

    // ── A ReadWrite login round-trips a post through the double, unaffected ──

    #[test]
    fn read_write_permission_can_post_through_the_double() {
        let (client, server) = make_pair();
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);
        let outcome = login_direct(&mut double, &client, &server, b"guest-pw", 1000);
        assert_eq!(outcome.permissions, RoomPermission::ReadWrite);
        assert!(outcome.permissions.can_post());

        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut post_wire = [0u8; 256];
        let n = encode_room_post(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            2000,
            0,
            b"it works",
            &mut post_wire,
        );
        let ack = double.handle_post(&client.pubkey, &post_wire[..n]).unwrap();
        assert_eq!(
            ack,
            room_post_ack_hash(2000, 0, b"it works", &client.pubkey)
        );
    }

    // ── decode_dm_payload sanity: RESPONSE datagram error path ──────────────

    #[test]
    fn decode_login_response_datagram_wrong_secret_fails_mac() {
        let (client, server) = make_pair();
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);
        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut anon_req = [0u8; 128];
        let n = encode_anon_req_login(
            &shared,
            server.pub_hash(),
            &client.pubkey,
            1000,
            0,
            b"guest-pw",
            &mut anon_req,
        );
        let mut reply_wire = [0u8; 64];
        let reply_len = double
            .handle_login(&anon_req[..n], &mut reply_wire)
            .unwrap();

        let wrong_secret = [0xEEu8; 32];
        assert_eq!(
            decode_login_response_datagram(&wrong_secret, &reply_wire[..reply_len]),
            Err(RoomSessionError::Codec(CodecError::MacMismatch))
        );
    }
}
