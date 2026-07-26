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
use protocol::room::{
    decode_login_response, decode_room_push, encode_keep_alive, encode_room_post,
    room_post_ack_hash, room_push_ack_hash, MAX_POST_TEXT_LEN,
};
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

// ── Session-learned state: standalone persistence ───────────────────────────
//
// `main.rs::run()` hands the whole `ProvisionedConfig` (which owns `RoomExtra`)
// off to the `admin_server` thread before the dispatcher loop that logs into
// rooms and receives pushes ever runs — so that loop has no safe in-place
// handle to `RoomExtra` to mutate. `PersistedRoomSession` is the same three
// learned fields (`permissions`/`sync_since`/`out_path`), persisted instead
// through a small, dedicated NVS store the dispatcher loop owns directly
// (`firmware::room_session::{load,save}_room_session`, mirroring
// `advert_ts_store.rs`'s "own small store" shape) — additive to, not a
// replacement for, the provisioning-time `RoomExtra` seed.

/// Bytes needed to encode a [`PersistedRoomSession`]:
/// `permissions(1) + sync_since(4) + out_path_len(1) + out_path(MAX_PATH_SIZE)
/// + last_room_ts(4)`.
pub const PERSISTED_ROOM_SESSION_LEN: usize = 1 + 4 + 1 + MAX_PATH_SIZE + 4;

/// The session-learned subset of a room's [`RoomExtra`] fields — see the
/// section doc above for why this is persisted through its own store rather
/// than `RoomExtra` in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersistedRoomSession {
    pub permissions: u8,
    pub sync_since: u32,
    pub out_path: [u8; MAX_PATH_SIZE],
    pub out_path_len: u8,
    /// High-water mark of every wall-clock timestamp this room session has
    /// ever sent the server (login, keep-alive, or a prior post — the
    /// server tracks one `last_timestamp` counter per client across all
    /// three, `MyMesh.cpp:435`). Persisted (not just kept in RAM) so a
    /// reboot's fresh, not-yet-GPS-synced clock can never regress below a
    /// value this room has already used — see [`RoomPostError`]'s doc for
    /// why a regression here must be refused, not sent.
    pub last_room_ts: u32,
}

impl PersistedRoomSession {
    /// No session learned anything yet — the state a freshly provisioned
    /// room (or one whose dedicated store has never been written) starts
    /// from.
    pub const EMPTY: Self = Self {
        permissions: 0,
        sync_since: 0,
        out_path: [0u8; MAX_PATH_SIZE],
        out_path_len: 0,
        last_room_ts: 0,
    };

    /// Snapshot the provisioning-time seed a `RoomExtra` carries (used as the
    /// fallback when this room's dedicated store has nothing persisted yet —
    /// e.g. the very first login attempt after provisioning).
    pub fn from_room_extra(extra: &RoomExtra) -> Self {
        Self {
            permissions: extra.permissions,
            sync_since: extra.sync_since,
            out_path: extra.out_path,
            out_path_len: extra.out_path_len,
            last_room_ts: 0,
        }
    }

    /// The decoded permission role — mirrors [`RoomExtra::permission`].
    pub fn permission(&self) -> RoomPermission {
        RoomPermission::from_u8(self.permissions)
    }

    /// Apply a login outcome's learned fields — mirrors [`apply_login_outcome`]
    /// for callers persisting through this standalone struct.
    pub fn apply_login_outcome(&mut self, outcome: &RoomLoginOutcome) {
        self.permissions = outcome.permissions as u8;
        if let Some((path, path_byte_count)) = outcome.out_path {
            self.out_path = path;
            self.out_path_len = path_byte_count.min(u8::MAX as usize) as u8;
        }
    }

    /// Record that this room session has just sent the server a frame
    /// timestamped `ts` (login, keep-alive, or a post) — advances
    /// `last_room_ts` to `ts` if it is a genuine increase. Never regresses:
    /// callers that raced or retried with a smaller `ts` leave the
    /// high-water mark untouched.
    pub fn record_sent_timestamp(&mut self, ts: u32) {
        if ts > self.last_room_ts {
            self.last_room_ts = ts;
        }
    }
}

/// Encode a [`PersistedRoomSession`] into `out` (at least
/// [`PERSISTED_ROOM_SESSION_LEN`] bytes). Returns the number of bytes written.
pub fn encode_persisted_room_session(state: &PersistedRoomSession, out: &mut [u8]) -> usize {
    out[0] = state.permissions;
    out[1..5].copy_from_slice(&state.sync_since.to_le_bytes());
    out[5] = state.out_path_len;
    out[6..6 + MAX_PATH_SIZE].copy_from_slice(&state.out_path);
    let ts_off = 6 + MAX_PATH_SIZE;
    out[ts_off..ts_off + 4].copy_from_slice(&state.last_room_ts.to_le_bytes());
    PERSISTED_ROOM_SESSION_LEN
}

/// Decode a [`PersistedRoomSession`] blob. `None` if truncated or if
/// `out_path_len` exceeds [`MAX_PATH_SIZE`] (a corrupt/foreign blob).
pub fn decode_persisted_room_session(blob: &[u8]) -> Option<PersistedRoomSession> {
    if blob.len() < PERSISTED_ROOM_SESSION_LEN {
        return None;
    }
    let permissions = blob[0];
    let sync_since = u32::from_le_bytes(blob[1..5].try_into().ok()?);
    let out_path_len = blob[5];
    if out_path_len as usize > MAX_PATH_SIZE {
        return None;
    }
    let mut out_path = [0u8; MAX_PATH_SIZE];
    out_path.copy_from_slice(&blob[6..6 + MAX_PATH_SIZE]);
    let ts_off = 6 + MAX_PATH_SIZE;
    let last_room_ts = u32::from_le_bytes(blob[ts_off..ts_off + 4].try_into().ok()?);
    Some(PersistedRoomSession {
        permissions,
        sync_since,
        out_path,
        out_path_len,
        last_room_ts,
    })
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

// ── Outbound post: Phase A "post semantics" ─────────────────────────────────

/// Errors from [`encode_room_post_checked`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomPostError {
    /// `candidate_ts` is not STRICTLY greater than `last_room_ts` — the
    /// high-water mark of every timestamp this room session has already
    /// sent the server (login, keep-alive, OR a prior post; the server
    /// tracks one `last_timestamp` counter per client across all three,
    /// `MyMesh.cpp:435`). Sending anyway risks exactly the trap this
    /// campaign's Objective calls out: the server treats an EQUAL timestamp
    /// as a retry (post silently discarded, still ACKed) and a LESSER one as
    /// an outright replay (no ACK at all). Typically caused by the local
    /// wall clock not yet being GPS-synced this boot (still on its per-boot
    /// random seed) or having gone backwards relative to a value this room
    /// has already used — the caller must surface this to the user rather
    /// than transmit.
    NonMonotonicTimestamp,
}

/// Generous upper bound on [`encode_room_post_checked`]'s output: outer
/// `[header(1)][path_len(1)]` + `encode_room_post`'s own DM-envelope worst
/// case (`2(dest/src hash) + 2(HMAC) + ceil_16(6 + MAX_POST_TEXT_LEN + 1)`).
pub const MAX_POST_FRAME_LEN: usize = 2 + 2 + 2 + (((6 + MAX_POST_TEXT_LEN + 1) / 16) + 1) * 16;

/// Checked, wire-ready encode of a room post (Phase A "post semantics"):
/// `[header=Flood/TxtMsg][path_len=0-hop][DM envelope]` — the same "flood,
/// addressed by dest_hash" shape [`encode_room_login_frame`] already uses,
/// since a room contact worth posting to has necessarily already reached
/// the server once (over flood or a learned direct route) and flood
/// addressing reaches it either way.
///
/// Refuses to encode (returns `Err`, writes nothing) unless `candidate_ts`
/// is STRICTLY greater than `last_room_ts` — see [`RoomPostError`]'s doc.
/// `last_room_ts` must be this room session's full high-water mark (login /
/// keep-alive / prior post), not just a prior call to this function.
///
/// On success returns `(frame_len, expected_ack_hash)`. The caller MUST:
/// - remember `expected_ack_hash` to recognise the server's post-ACK
///   (`room_post_ack_hash`'s doc: `sha256(post_plaintext || client_pubkey)[0..4]`);
/// - advance its own high-water mark to `candidate_ts`
///   ([`PersistedRoomSession::record_sent_timestamp`]) so the NEXT call's
///   `last_room_ts` reflects this send, whether or not the ACK ever arrives
///   (the monotonic guard is about what this client has SENT, not what it
///   has had acknowledged).
#[allow(clippy::too_many_arguments)]
pub fn encode_room_post_checked(
    shared_secret: &[u8; 32],
    dest_hash: u8,
    src_hash: u8,
    candidate_ts: u32,
    last_room_ts: u32,
    text: &[u8],
    client_pubkey: &[u8; 32],
    out: &mut [u8],
) -> Result<(usize, [u8; 4]), RoomPostError> {
    if candidate_ts <= last_room_ts {
        return Err(RoomPostError::NonMonotonicTimestamp);
    }
    out[0] = Header::new(RouteType::Flood, PayloadType::TxtMsg).0;
    out[1] = PathLen::new(2, 0).map(|p| p.0).unwrap_or(0x40);
    let n = encode_room_post(
        shared_secret,
        dest_hash,
        src_hash,
        candidate_ts,
        0,
        text,
        &mut out[2..],
    );
    let text_len = text.len().min(MAX_POST_TEXT_LEN);
    let ack = room_post_ack_hash(candidate_ts, 0, &text[..text_len], client_pubkey);
    Ok((2 + n, ack))
}

// ── Keep-alive: Phase C "keep-alive scheduler" ──────────────────────────────

/// Build the outer `[header][path_len][path bytes...]` prefix for a frame
/// this client sends straight to a room server over its learned `out_path` —
/// the ONLY route a room server ever answers a `REQ_TYPE_KEEP_ALIVE` over
/// (`MyMesh.cpp:536`, `packet->isRouteDirect()`). `out_path` is 1-byte-hash
/// encoded (matches [`RoomLoginOutcome::out_path`]/[`RoomExtra::out_path`]'s
/// own convention — the path a `decode_path_return` PATH-return taught this
/// client). Returns `None` if `out_path` is empty (nothing learned yet — the
/// caller must re-flood the `ANON_REQ` login to relearn it instead, per this
/// module's `encode_room_login_frame`) or longer than 63 hops (`PathLen`'s
/// own range).
pub fn encode_room_direct_prefix(
    payload_type: PayloadType,
    out_path: &[u8],
    out: &mut [u8],
) -> Option<usize> {
    if out_path.is_empty() {
        return None;
    }
    let path_len = PathLen::new(1, u8::try_from(out_path.len()).ok()?)?;
    out[0] = Header::new(RouteType::Direct, payload_type).0;
    out[1] = path_len.0;
    let n = path_len.hop_count() as usize;
    out[2..2 + n].copy_from_slice(&out_path[..n]);
    Some(2 + n)
}

/// Generous upper bound on [`encode_room_keep_alive_frame`]'s output: the
/// direct-route prefix's worst case (`2 + MAX_PATH_SIZE`) plus the keep-alive
/// DM envelope's own worst case (`2 + 2 + ceil_16(9)` = 20).
pub const MAX_KEEP_ALIVE_FRAME_LEN: usize = 2 + MAX_PATH_SIZE + 20;

/// Encode a full route-direct keep-alive frame — Phase C's periodic
/// liveness/backlog-depth probe. Returns `None` (writes nothing) if
/// `out_path` is empty: **route-direct is a hard prerequisite**
/// (`encode_room_direct_prefix`'s doc) — a caller that gets `None` here must
/// re-flood the login instead of ever attempting a flood-routed keep-alive
/// (the server ignores one outright, `MyMesh.cpp:536`).
///
/// `force_since`, passed straight through to [`encode_keep_alive`], recovers
/// a stalled sync by force-updating the server's view of this client's
/// `sync_since`; pass `0` to leave it untouched.
#[allow(clippy::too_many_arguments)]
pub fn encode_room_keep_alive_frame(
    shared_secret: &[u8; 32],
    dest_hash: u8,
    src_hash: u8,
    out_path: &[u8],
    timestamp: u32,
    force_since: u32,
    out: &mut [u8],
) -> Option<usize> {
    let prefix_len = encode_room_direct_prefix(PayloadType::Req, out_path, out)?;
    let n = encode_keep_alive(
        shared_secret,
        dest_hash,
        src_hash,
        timestamp,
        force_since,
        &mut out[prefix_len..],
    );
    Some(prefix_len + n)
}

// ── Keep-alive: reconnect-stall detector ────────────────────────────────────
//
// The gap this closes: `out_path` is only ever zeroed at init/decode — there
// is no ACK-timeout handler and no failure counter anywhere upstream of this
// module, so a repeater/topology change that leaves the persisted `out_path`
// stale-but-nonzero keeps getting route-directed down a dead route forever.
// The `out_path_len == 0` re-flood-login branch this scheduler already has
// (`encode_room_login_frame`'s doc) never fires, and the session stalls
// until reboot — corroborated by this module's own "a client that fails to
// ACK stalls its own sync permanently" doc and the M1 checkpoint's "stalled
// until reboot" finding. [`RoomKeepAliveStall`] is the missing failure
// counter: `firmware::main`'s scheduler feeds it one bool per tick (was the
// PRIOR tick's `pending_keep_alive_ack` still outstanding?) and it decides
// when the session has had enough chances to recover on its own.

/// Consecutive missed keep-alive ACKs [`RoomKeepAliveStall`] tolerates before
/// concluding a client's `out_path` is dead — a topology/repeater change,
/// not just one dropped frame on an otherwise-live route.
///
/// **N=2**: a single miss is exactly what ordinary LoRa frame loss looks
/// like on a route that is otherwise fine — invalidating `out_path` on that
/// alone would spuriously re-flood (costing every relaying node airtime,
/// unlike a route-direct keep-alive) on routine link noise. A SECOND
/// consecutive miss, a full `ROOM_KEEP_ALIVE_INTERVAL_MS` (5 minutes) later,
/// is the discriminator: back-to-back failures that far apart are no longer
/// explainable as one unlucky frame — the route itself stopped working. At
/// that 5-minute cadence, N=2 costs ~10 minutes worst-case to
/// detect-and-recover: still well inside the "human notices the room went
/// quiet, hasn't yet reached for the power button" window
/// `ROOM_KEEP_ALIVE_INTERVAL_MS`'s own doc (`firmware/src/main.rs`) argues
/// for, and it adds no extra airtime at all — the flood re-login this
/// triggers is the exact same frame the `out_path_len == 0` branch already
/// sends; this constant only changes when that branch fires.
pub const KEEP_ALIVE_STALL_THRESHOLD: u8 = 2;

/// Reconnect-stall detector: counts consecutive keep-alive ticks whose PRIOR
/// tick's ACK never arrived, and — once [`KEEP_ALIVE_STALL_THRESHOLD`] is
/// reached — zeroes a session's `out_path_len` so the caller's very next
/// tick falls onto the re-flood-login branch (clean relearn) instead of
/// route-directing another keep-alive down the same dead path.
///
/// Reset conditions mirror the server's own `push_failures` reset
/// conditions (see [`KEEP_ALIVE_STALL_THRESHOLD`]'s doc): a successful
/// keep-alive ACK, an inbound post, or a fresh login should all call
/// [`Self::reset`] — never let a miss streak from a stale interaction carry
/// into a freshly-proven-live session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoomKeepAliveStall {
    missed: u8,
}

impl RoomKeepAliveStall {
    /// A fresh detector — no misses counted yet.
    pub const fn new() -> Self {
        Self { missed: 0 }
    }

    /// Current consecutive-miss count (test/diagnostic visibility only —
    /// callers drive behaviour through [`Self::on_tick`]/[`Self::reset`],
    /// never by inspecting this directly).
    pub fn missed(&self) -> u8 {
        self.missed
    }

    /// Clear the miss streak — call on any evidence the session is live
    /// (see this type's doc for the three reset conditions).
    pub fn reset(&mut self) {
        self.missed = 0;
    }

    /// Record one keep-alive scheduler tick. `ack_outstanding` is whether
    /// the PRIOR tick's `pending_keep_alive_ack` was still `Some` (i.e.
    /// never ACKed) at the moment this tick fires; a tick with nothing
    /// outstanding (the routine case — the prior keep-alive was ACKed
    /// in time, or none had been sent yet) is a no-op that leaves the
    /// counter untouched.
    ///
    /// Returns `true` the moment the miss streak reaches
    /// [`KEEP_ALIVE_STALL_THRESHOLD`] — at which point `session.out_path_len`
    /// has ALREADY been zeroed and the counter reset to 0 (a clean slate for
    /// whatever session the relearn produces next) — `false` otherwise.
    pub fn on_tick(&mut self, ack_outstanding: bool, session: &mut PersistedRoomSession) -> bool {
        if !ack_outstanding {
            return false;
        }
        self.missed = self.missed.saturating_add(1);
        if self.missed >= KEEP_ALIVE_STALL_THRESHOLD {
            session.out_path_len = 0;
            self.missed = 0;
            true
        } else {
            false
        }
    }
}

// ── Notification-suppression parity: Phase D ────────────────────────────────

/// What a caller should do about one incoming room post, per
/// [`RoomSyncPhase`]'s classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomNotification {
    /// Currently draining the post-login backlog: suppress any per-post
    /// notification/badge increment. Silently folded into the eventual
    /// [`RoomNotification::Aggregate`] the drain window's close will emit.
    None,
    /// The drain window just closed (a keep-alive ACK reported the unsynced
    /// count reached 0): fire exactly ONE aggregate notification for the
    /// whole backlog just absorbed. `count` is how many genuinely new posts
    /// (post-dedup) were folded in — `0` never reaches the caller (see
    /// [`RoomSyncPhase::on_keep_alive_ack`]'s doc: nothing drained, nothing
    /// to announce).
    Aggregate { count: u32 },
    /// Not draining: this is a live post, full parity with the channel
    /// notification path (fire it exactly like `IncomingGroupMsg`).
    Live,
}

/// Per-room session-phase tracker driving Phase D's notification
/// classification — **by session phase, not post count or a timer** (the
/// Objective's own non-negotiable: a count/timer heuristic misclassifies a
/// live post that arrives during a slow drain). A sync-drain window opens
/// the moment a room session starts (a fresh boot always needs to reconfirm
/// whether backlog remains) and closes only when a keep-alive ACK's
/// unsynced-count byte reports `0` — never on a post count reaching some
/// threshold, never on a wall-clock timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoomSyncPhase {
    draining: bool,
    drained_count: u32,
}

impl RoomSyncPhase {
    /// A fresh room session, right after login: the drain window starts
    /// open — the client cannot yet know whether it has any backlog until a
    /// keep-alive ACK confirms zero.
    pub const fn new_after_login() -> Self {
        Self {
            draining: true,
            drained_count: 0,
        }
    }

    /// Whether the drain window is currently open.
    pub fn is_draining(&self) -> bool {
        self.draining
    }

    /// Classify one genuinely-new incoming post (a dedup HIT must never
    /// reach this — see [`Self::on_push_outcome`], the call site this exists
    /// to keep in lockstep with dedup).
    fn on_post_received(&mut self) -> RoomNotification {
        if self.draining {
            self.drained_count += 1;
            RoomNotification::None
        } else {
            RoomNotification::Live
        }
    }

    /// The single call site a caller's push handler should use: feeds a
    /// [`RoomPushOutcome`] straight in, so the dedup rule and the
    /// notification-suppression rule can never drift apart. A duplicate
    /// (`entry: None` — a room-server retry of a post already in history)
    /// is not counted or classified at all: it must still be ACKed
    /// unconditionally (`handle_room_push`'s doc), but it must never inflate
    /// the drain aggregate's count or fire a live notification — a re-drain
    /// after reboot must not duplicate history OR re-notify.
    pub fn on_push_outcome(&mut self, outcome: &RoomPushOutcome) -> RoomNotification {
        if outcome.entry.is_none() {
            return RoomNotification::None;
        }
        self.on_post_received()
    }

    /// Feed a keep-alive ACK's unsynced-count byte in. If the drain window
    /// was open and this count reports `0`, closes it and returns the
    /// aggregate notification to fire — `None` if either the window was
    /// already closed, the count is still nonzero (still draining, however
    /// slowly), or the window closed with nothing actually drained (nothing
    /// to announce).
    pub fn on_keep_alive_ack(&mut self, unsynced_count: u8) -> Option<RoomNotification> {
        if !self.draining || unsynced_count != 0 {
            return None;
        }
        self.draining = false;
        let count = self.drained_count;
        self.drained_count = 0;
        if count > 0 {
            Some(RoomNotification::Aggregate { count })
        } else {
            None
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::identity::Identity;
    use protocol::room::{
        decode_keep_alive_ack, encode_anon_req_login, encode_room_post, keep_alive_ack_hash,
        room_post_ack_hash, RoomServerDouble,
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

    // ── PersistedRoomSession: standalone codec round trip ───────────────────

    #[test]
    fn persisted_room_session_round_trips_through_encode_decode() {
        let mut out_path = [0u8; MAX_PATH_SIZE];
        out_path[0] = 0x11;
        out_path[1] = 0x22;
        let state = PersistedRoomSession {
            permissions: RoomPermission::ReadWrite as u8,
            sync_since: 0xDEAD_BEEF,
            out_path,
            out_path_len: 2,
            last_room_ts: 0xC0FF_EE42,
        };

        let mut blob = [0u8; PERSISTED_ROOM_SESSION_LEN];
        let n = encode_persisted_room_session(&state, &mut blob);
        assert_eq!(n, PERSISTED_ROOM_SESSION_LEN);

        let decoded = decode_persisted_room_session(&blob).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn persisted_room_session_decode_rejects_truncated_blob() {
        let short = [0u8; PERSISTED_ROOM_SESSION_LEN - 1];
        assert_eq!(decode_persisted_room_session(&short), None);
    }

    #[test]
    fn persisted_room_session_from_room_extra_snapshots_the_provisioning_seed() {
        let mut extra = RoomExtra::EMPTY;
        extra.permissions = RoomPermission::Guest as u8;
        extra.sync_since = 7;

        let state = PersistedRoomSession::from_room_extra(&extra);
        assert_eq!(state.permission(), RoomPermission::Guest);
        assert_eq!(state.sync_since, 7);
    }

    #[test]
    fn persisted_room_session_apply_login_outcome_matches_room_extra_variant() {
        let outcome = RoomLoginOutcome {
            permissions: RoomPermission::Admin,
            out_path: Some(([0xAAu8; MAX_PATH_SIZE], 3)),
        };

        let mut extra = RoomExtra::EMPTY;
        apply_login_outcome(&mut extra, &outcome);

        let mut state = PersistedRoomSession::EMPTY;
        state.apply_login_outcome(&outcome);

        assert_eq!(state.permissions, extra.permissions);
        assert_eq!(state.out_path_len, extra.out_path_len);
        assert_eq!(
            &state.out_path[..state.out_path_len as usize],
            &extra.out_path[..extra.out_path_len as usize]
        );
    }

    #[test]
    fn record_sent_timestamp_never_regresses() {
        let mut state = PersistedRoomSession::EMPTY;
        state.record_sent_timestamp(100);
        assert_eq!(state.last_room_ts, 100);
        state.record_sent_timestamp(50); // smaller: ignored
        assert_eq!(state.last_room_ts, 100);
        state.record_sent_timestamp(101);
        assert_eq!(state.last_room_ts, 101);
    }

    // ── Phase A: encode_room_post_checked's monotonic-timestamp guard ──────

    #[test]
    fn encode_room_post_checked_rejects_equal_and_lesser_timestamps() {
        let (client, server) = make_pair();
        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut out = [0u8; MAX_POST_FRAME_LEN];

        assert_eq!(
            encode_room_post_checked(
                &shared,
                server.pub_hash(),
                client.pub_hash(),
                1000,
                1000, // equal: the "silently discarded as a retry" trap
                b"hi",
                &client.pubkey,
                &mut out,
            ),
            Err(RoomPostError::NonMonotonicTimestamp)
        );
        assert_eq!(
            encode_room_post_checked(
                &shared,
                server.pub_hash(),
                client.pub_hash(),
                999,
                1000, // lesser: outright replay
                b"hi",
                &client.pubkey,
                &mut out,
            ),
            Err(RoomPostError::NonMonotonicTimestamp)
        );
    }

    #[test]
    fn encode_room_post_checked_accepts_strictly_greater_and_round_trips_through_double() {
        let (client, server) = make_pair();
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);
        login_direct(&mut double, &client, &server, b"guest-pw", 1000);

        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut out = [0u8; MAX_POST_FRAME_LEN];
        let (n, expected_ack) = encode_room_post_checked(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            2000,
            1000,
            b"a genuinely new post",
            &client.pubkey,
            &mut out,
        )
        .expect("candidate_ts > last_room_ts must encode");

        // The frame is a real, wire-valid flood TXT_MSG the double accepts —
        // strip the 2-byte outer header/path_len this function prepends
        // before handing the DM payload to the double, mirroring how
        // `on_receive` peels the same prefix off in production.
        assert_eq!(
            out[0],
            Header::new(RouteType::Flood, PayloadType::TxtMsg).0,
            "post frame must be flood-routed, matching every other DM send"
        );
        let ack = double
            .handle_post(&client.pubkey, &out[2..n])
            .expect("ReadWrite client's post must be accepted by the double");
        assert_eq!(
            ack, expected_ack,
            "double's ack must match this function's computed ack_hash"
        );
    }

    // ── Phase C: route-direct keep-alive framing ────────────────────────────

    #[test]
    fn encode_room_direct_prefix_refuses_empty_out_path() {
        let mut out = [0u8; 8];
        assert_eq!(
            encode_room_direct_prefix(PayloadType::Req, &[], &mut out),
            None,
            "no learned route: caller must re-flood the login instead"
        );
    }

    #[test]
    fn encode_room_keep_alive_frame_is_never_flood_routed() {
        let (client, server) = make_pair();
        let shared = client.ecdh_shared_secret(&server.pubkey);
        let out_path = [0xABu8, 0xCD];

        let mut out = [0u8; MAX_KEEP_ALIVE_FRAME_LEN];
        let n = encode_room_keep_alive_frame(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            &out_path,
            5000,
            0,
            &mut out,
        )
        .expect("a learned out_path must encode");

        let header = Header(out[0]);
        assert_eq!(
            header.route_type(),
            Some(RouteType::Direct),
            "a room keep-alive must NEVER be flood-routed (MyMesh.cpp:536 ignores one)"
        );
        assert_eq!(header.payload_type(), Some(PayloadType::Req));
        let path_len = PathLen(out[1]);
        assert_eq!(path_len.hop_count(), 2);
        assert_eq!(&out[2..4], &out_path);

        // Round trip through the double: strip the same prefix production's
        // `on_receive` would peel off before forwarding the DM payload.
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);
        login_direct(&mut double, &client, &server, b"guest-pw", 1000);
        let ack = double
            .handle_keep_alive(&client.pubkey, &out[4..n])
            .expect("keep-alive must be accepted");
        assert_eq!(
            decode_keep_alive_ack(&ack).unwrap().ack_hash,
            keep_alive_ack_hash(5000, 0, &client.pubkey)
        );
    }

    #[test]
    fn encode_room_keep_alive_frame_none_without_a_learned_path() {
        let (client, server) = make_pair();
        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut out = [0u8; MAX_KEEP_ALIVE_FRAME_LEN];
        assert_eq!(
            encode_room_keep_alive_frame(
                &shared,
                server.pub_hash(),
                client.pub_hash(),
                &[],
                5000,
                0,
                &mut out,
            ),
            None
        );
    }

    #[test]
    fn keep_alive_force_since_recovers_a_stalled_sync() {
        // Phase C's stall-recovery bullet: a nonzero force_since force-updates
        // the server's view of this client's sync_since, letting the client
        // rewind (recover posts it never got pushed) without a fresh login.
        // Driven directly through `protocol::room::encode_keep_alive` (this
        // module's own `encode_room_keep_alive_frame` is exercised for its
        // route-direct FRAMING by `encode_room_keep_alive_frame_is_never_
        // flood_routed`, above; this test is about the `force_since`
        // VALUE's effect on the server, which composes with either encoder).
        let (client, server) = make_pair();
        let other_author = Identity::from_seed([0x90u8; 32]);
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);
        login_direct(&mut double, &client, &server, b"guest-pw", 1000);

        double.seed_post(&other_author.pubkey, 5000, b"missed post");

        // Drain it normally first, so the server's sync_since has already
        // moved past 5000 — simulating a client that legitimately received
        // everything, then later wants to force a re-drain from an earlier
        // point (e.g. after losing local history).
        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut wire = [0u8; 256];
        let n = double
            .push_next(&client.pubkey, &mut wire)
            .expect("the seeded post must be eligible");
        let mut pt = [0u8; 256];
        let (_dest, _src, push) = decode_room_push(&shared, &wire[..n], &mut pt).unwrap();
        let ack = room_push_ack_hash(
            push.post_ts,
            push.attempt,
            push.push_body(&pt),
            &client.pubkey,
        );
        assert!(double.handle_ack(&client.pubkey, &ack));
        assert_eq!(double.client_sync_since(&client.pubkey), Some(5000));

        // A keep-alive with force_since == the current watermark: no-op on
        // the unsynced count (nothing new becomes eligible).
        let mut raw = [0u8; 64];
        let raw_n = protocol::room::encode_keep_alive(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            6000,
            5000,
            &mut raw,
        );
        let ka_ack = double
            .handle_keep_alive(&client.pubkey, &raw[..raw_n])
            .expect("keep-alive must be accepted");
        assert_eq!(
            decode_keep_alive_ack(&ka_ack).unwrap().unsynced_count,
            0,
            "force_since==current watermark: no new unsynced posts"
        );
        assert_eq!(double.client_sync_since(&client.pubkey), Some(5000));

        // Force further BACK, past the post's own timestamp: it becomes
        // re-eligible for push, proving the rewind actually recovers it —
        // the concrete "recover a stalled sync" behaviour this bullet pins.
        let raw_n2 = protocol::room::encode_keep_alive(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            7000,
            1000,
            &mut raw,
        );
        let ka_ack2 = double
            .handle_keep_alive(&client.pubkey, &raw[..raw_n2])
            .expect("keep-alive must be accepted");
        assert_eq!(
            decode_keep_alive_ack(&ka_ack2).unwrap().unsynced_count,
            1,
            "force_since=1000 rewinds past the post at ts=5000: it is unsynced again"
        );
        let mut wire2 = [0u8; 256];
        assert!(
            double.push_next(&client.pubkey, &mut wire2).is_some(),
            "force_since must have made the already-delivered post re-eligible for push"
        );
    }

    // ── Keep-alive: reconnect-stall detector ─────────────────────────────

    #[test]
    fn stall_detector_never_trips_while_acks_keep_arriving() {
        let mut session = PersistedRoomSession::EMPTY;
        session.out_path_len = 2;
        let mut stall = RoomKeepAliveStall::new();
        for _ in 0..(KEEP_ALIVE_STALL_THRESHOLD as u32 * 5) {
            assert!(
                !stall.on_tick(false, &mut session),
                "an ACKed tick must never invalidate out_path"
            );
        }
        assert_eq!(
            session.out_path_len, 2,
            "a session receiving ACKs normally must never spuriously re-flood"
        );
        assert_eq!(stall.missed(), 0);
    }

    #[test]
    fn stall_detector_invalidates_out_path_exactly_at_the_threshold() {
        let mut session = PersistedRoomSession::EMPTY;
        session.out_path_len = 2;
        let mut stall = RoomKeepAliveStall::new();
        for i in 1..KEEP_ALIVE_STALL_THRESHOLD {
            assert!(
                !stall.on_tick(true, &mut session),
                "miss {i}/{KEEP_ALIVE_STALL_THRESHOLD} must not yet invalidate out_path"
            );
            assert_eq!(
                session.out_path_len, 2,
                "out_path survives before the threshold"
            );
        }
        assert!(
            stall.on_tick(true, &mut session),
            "the {KEEP_ALIVE_STALL_THRESHOLD}th consecutive miss must invalidate out_path"
        );
        assert_eq!(
            session.out_path_len, 0,
            "out_path must be zeroed on stall detection"
        );
        assert_eq!(
            stall.missed(),
            0,
            "the counter itself resets once it fires, for the session the relearn produces"
        );
    }

    #[test]
    fn stall_detector_reset_clears_a_partial_miss_streak() {
        // The miss-counter reset condition, isolated from any ACK/post/login
        // plumbing: a streak that was interrupted must need a FULL fresh
        // threshold's worth of misses to trip, not just one more.
        let mut session = PersistedRoomSession::EMPTY;
        session.out_path_len = 2;
        let mut stall = RoomKeepAliveStall::new();
        assert!(!stall.on_tick(true, &mut session)); // one miss
        stall.reset();
        for _ in 1..KEEP_ALIVE_STALL_THRESHOLD {
            assert!(!stall.on_tick(true, &mut session));
        }
        assert_eq!(
            session.out_path_len, 2,
            "a reset streak needs a full fresh threshold to trip, not the pre-reset remainder"
        );
    }

    #[test]
    fn stall_then_relearn_recovers_backlog_without_reboot() {
        // This mission's core acceptance bullet, end to end: a changed-path
        // reconnect recovers WITHOUT reboot. `RoomKeepAliveStall` detects the
        // stall and zeroes `out_path_len`; a re-flood login (modelled here
        // via the double's `handle_login`, exactly like `login_direct`)
        // relearns the session; and the resumed keep-alive's `force_since`
        // re-affirms the watermark BEFORE any push retry, so the server's
        // queue does not re-deliver a post the client already has (dedup
        // stays intact) while still recovering the one it genuinely missed.
        let (client, server) = make_pair();
        let other_author = Identity::from_seed([0xB0u8; 32]);
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);

        let outcome = login_direct(&mut double, &client, &server, b"guest-pw", 1000);
        let mut session = PersistedRoomSession::EMPTY;
        session.apply_login_outcome(&outcome);
        // Simulate an earlier PATH-return having taught a route (the
        // double's direct-RESPONSE login form never carries one — see
        // `decode_login_path_return_records_permission_and_learns_out_path`
        // for that leg's own coverage).
        session.out_path[..2].copy_from_slice(&[0xAB, 0xCD]);
        session.out_path_len = 2;
        let mut stall = RoomKeepAliveStall::new();
        let shared = client.ecdh_shared_secret(&server.pubkey);

        // Drain one post normally, before anything goes wrong.
        double.seed_post(&other_author.pubkey, 2000, b"already delivered");
        let mut wire = [0u8; 256];
        let n = double
            .push_next(&client.pubkey, &mut wire)
            .expect("the seeded post must be eligible");
        let mut pt = [0u8; 256];
        let (_dest, _src, push) = decode_room_push(&shared, &wire[..n], &mut pt).unwrap();
        let ack = room_push_ack_hash(
            push.post_ts,
            push.attempt,
            push.push_body(&pt),
            &client.pubkey,
        );
        assert!(double.handle_ack(&client.pubkey, &ack));
        session.sync_since = push.post_ts; // mirrors `handle_room_push_frame`'s watermark update
        stall.reset(); // mirrors the inbound-post reset condition
        assert_eq!(double.client_sync_since(&client.pubkey), Some(2000));

        // A routine keep-alive succeeds — Case A / "no spurious re-flood".
        let mut raw = [0u8; 64];
        let ka_n = encode_keep_alive(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            2100,
            0,
            &mut raw,
        );
        let ka_ack = double
            .handle_keep_alive(&client.pubkey, &raw[..ka_n])
            .expect("routine keep-alive must be accepted");
        assert_eq!(decode_keep_alive_ack(&ka_ack).unwrap().unsynced_count, 0);
        assert!(!stall.on_tick(false, &mut session));
        assert_eq!(
            session.out_path_len, 2,
            "a live route must survive a routine keep-alive"
        );

        // The path changes: the server attempts a push right as the route
        // dies (it has no way to know), then every subsequent keep-alive
        // vanishes — nothing reaches `double` at all, exactly what a dead
        // `out_path` means on the wire.
        double.seed_post(&other_author.pubkey, 3000, b"missed during the stall");
        let mut stuck_wire = [0u8; 256];
        assert!(
            double.push_next(&client.pubkey, &mut stuck_wire).is_some(),
            "the server attempts the push (it has no way to know the route just died)"
        );

        for i in 1..KEEP_ALIVE_STALL_THRESHOLD {
            assert!(
                !stall.on_tick(true, &mut session),
                "miss {i}/{KEEP_ALIVE_STALL_THRESHOLD} must not yet invalidate out_path"
            );
            assert_eq!(session.out_path_len, 2);
        }
        assert!(
            stall.on_tick(true, &mut session),
            "the {KEEP_ALIVE_STALL_THRESHOLD}th consecutive miss must invalidate out_path"
        );
        assert_eq!(
            session.out_path_len, 0,
            "out_path must be zeroed on stall detection"
        );
        assert_eq!(stall.missed(), 0);

        // Relearn: `out_path_len == 0` routes `firmware::main`'s scheduler
        // to re-flood the login. The double's simplified login helper
        // always claims `sync_since=0` in its ANON_REQ (unlike production
        // firmware, which sends the real persisted watermark) —
        // deliberately exercising the pessimistic case where the server's
        // own view of `sync_since` regresses on relogin, which is exactly
        // the case `force_since` exists to correct.
        let relearn = login_direct(&mut double, &client, &server, b"guest-pw", 4000);
        session.apply_login_outcome(&relearn);
        session.out_path[..3].copy_from_slice(&[0x11, 0x22, 0x33]); // a NEW (changed) path
        session.out_path_len = 3;
        stall.reset();
        assert_eq!(
            double.client_sync_since(&client.pubkey),
            Some(0),
            "the double's login helper regresses sync_since to 0 on relogin"
        );

        // The resumed keep-alive carries `force_since = session.sync_since`
        // (this mission's fix, not the routine `0`) — re-affirming the
        // watermark BEFORE any push retry.
        let mut raw2 = [0u8; 64];
        let ka_n2 = encode_keep_alive(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            4100,
            session.sync_since,
            &mut raw2,
        );
        let ka_ack2 = double
            .handle_keep_alive(&client.pubkey, &raw2[..ka_n2])
            .expect("the resumed keep-alive must be accepted");
        assert_eq!(
            decode_keep_alive_ack(&ka_ack2).unwrap().unsynced_count,
            1,
            "only the genuinely-missed post is outstanding"
        );
        assert_eq!(
            double.client_sync_since(&client.pubkey),
            Some(2000),
            "force_since corrected the server's regressed watermark"
        );
        assert!(!stall.on_tick(false, &mut session));

        // The backlog drains: exactly the missed post, not a duplicate of
        // the already-delivered one.
        let mut retry_wire = [0u8; 256];
        let retry_n = double
            .push_next(&client.pubkey, &mut retry_wire)
            .expect("the missed post must now be eligible");
        let mut pt2 = [0u8; 256];
        let (_dest2, _src2, push2) =
            decode_room_push(&shared, &retry_wire[..retry_n], &mut pt2).unwrap();
        assert_eq!(
            push2.post_ts, 3000,
            "the genuinely-missed post, not a re-delivery of the already-acked one"
        );
        let ack2 = room_push_ack_hash(
            push2.post_ts,
            push2.attempt,
            push2.push_body(&pt2),
            &client.pubkey,
        );
        assert!(double.handle_ack(&client.pubkey, &ack2));
        assert_eq!(
            double.client_sync_since(&client.pubkey),
            Some(3000),
            "sync_since advances past the recovered post — dedup/backlog intact"
        );
    }

    // ── Phase D: session-phase notification classification ─────────────────

    #[test]
    fn drain_of_32_posts_yields_exactly_one_aggregate_of_32() {
        let mut phase = RoomSyncPhase::new_after_login();
        for _ in 0..32 {
            let outcome = RoomPushOutcome {
                ack_hash: [0; 4],
                post_ts: 0,
                entry: Some(HistoryEntry {
                    sender_hash: 0,
                    msg_type: HistoryMsgType::Dm,
                    timestamp: 0,
                    text: [0; MAX_HISTORY_TEXT_LEN],
                    text_len: 0,
                }),
            };
            assert_eq!(phase.on_push_outcome(&outcome), RoomNotification::None);
        }
        assert_eq!(
            phase.on_keep_alive_ack(0),
            Some(RoomNotification::Aggregate { count: 32 })
        );
        // Idempotent: a second unsynced=0 report with nothing new drained
        // fires nothing (the window is already closed).
        assert_eq!(phase.on_keep_alive_ack(0), None);
    }

    #[test]
    fn live_post_after_drain_closes_gets_full_parity() {
        let mut phase = RoomSyncPhase::new_after_login();
        assert_eq!(
            phase.on_keep_alive_ack(0),
            None,
            "nothing drained: no aggregate"
        );
        assert!(!phase.is_draining());

        let outcome = RoomPushOutcome {
            ack_hash: [0; 4],
            post_ts: 0,
            entry: Some(HistoryEntry {
                sender_hash: 0,
                msg_type: HistoryMsgType::Dm,
                timestamp: 0,
                text: [0; MAX_HISTORY_TEXT_LEN],
                text_len: 0,
            }),
        };
        assert_eq!(phase.on_push_outcome(&outcome), RoomNotification::Live);
    }

    #[test]
    fn live_post_during_a_slow_drain_is_still_classified_as_draining() {
        // The test that kills a naive count/timer heuristic: feed MORE than
        // 32 posts (a count heuristic would have flipped to "live" well
        // before this) while the drain window is still legitimately open
        // (no keep-alive has yet reported unsynced=0) — every single one
        // must still classify as None (folded into the eventual aggregate),
        // never as a standalone Live notification.
        let mut phase = RoomSyncPhase::new_after_login();
        let fresh_entry = || RoomPushOutcome {
            ack_hash: [0; 4],
            post_ts: 0,
            entry: Some(HistoryEntry {
                sender_hash: 0,
                msg_type: HistoryMsgType::Dm,
                timestamp: 0,
                text: [0; MAX_HISTORY_TEXT_LEN],
                text_len: 0,
            }),
        };
        for _ in 0..40 {
            assert_eq!(
                phase.on_push_outcome(&fresh_entry()),
                RoomNotification::None
            );
        }
        assert!(
            phase.is_draining(),
            "40 posts in is still no reason to leave the drain phase — only a keep-alive ACK can"
        );
        assert_eq!(
            phase.on_keep_alive_ack(0),
            Some(RoomNotification::Aggregate { count: 40 })
        );
    }

    #[test]
    fn dedup_hit_is_neither_counted_nor_notified() {
        // A replayed push (`entry: None`) must not inflate the drain
        // aggregate's count, and must not itself fire a notification —
        // a re-drain after reboot must not duplicate history OR re-notify.
        let mut phase = RoomSyncPhase::new_after_login();
        let dup = RoomPushOutcome {
            ack_hash: [0; 4],
            post_ts: 0,
            entry: None,
        };
        assert_eq!(phase.on_push_outcome(&dup), RoomNotification::None);
        // Nothing was actually drained — closing the window now fires no
        // aggregate at all (count is 0).
        assert_eq!(phase.on_keep_alive_ack(0), None);
    }

    #[test]
    fn keep_alive_ack_with_nonzero_unsynced_count_keeps_draining() {
        let mut phase = RoomSyncPhase::new_after_login();
        assert_eq!(
            phase.on_keep_alive_ack(5),
            None,
            "still draining: not yet 0"
        );
        assert!(phase.is_draining());
    }

    // ── Full integration: 32-post login backlog through the real double ────

    #[test]
    fn full_32_post_drain_through_the_double_yields_one_aggregate_and_32_deduped_entries() {
        // This mission's first Acceptance bullet, end to end: a real login,
        // a real 32-post drip through `RoomServerDouble` (one push/ACK at a
        // time, exactly like on-air), `handle_room_push`'s content dedup
        // feeding a growing `history` Vec exactly as `main.rs`'s
        // `room.recent` would, AND `RoomSyncPhase` classifying every one of
        // them — not the synthetic-outcome unit tests above, the actual
        // wire pipeline.
        let (client, server) = make_pair();
        let other_author = Identity::from_seed([0xA0u8; 32]);
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);
        login_direct(&mut double, &client, &server, b"guest-pw", 1000);

        for i in 0..32u32 {
            double.seed_post(
                &other_author.pubkey,
                2000 + i,
                format!("post {i}").as_bytes(),
            );
        }

        let shared = client.ecdh_shared_secret(&server.pubkey);
        let conv_hash = server.pub_hash();
        let mut history: Vec<HistoryEntry> = Vec::new();
        let mut phase = RoomSyncPhase::new_after_login();

        loop {
            let mut wire = [0u8; 256];
            let Some(n) = double.push_next(&client.pubkey, &mut wire) else {
                break;
            };
            let outcome =
                handle_room_push(&shared, &wire[..n], &client.pubkey, conv_hash, &history)
                    .expect("push must decode");
            assert_eq!(
                phase.on_push_outcome(&outcome),
                RoomNotification::None,
                "every post in the initial drain must be suppressed, not individually notified"
            );
            let entry = outcome.entry.expect("a fresh push must produce an entry");
            history.push(entry);
            assert!(double.handle_ack(&client.pubkey, &outcome.ack_hash));
        }

        assert_eq!(
            history.len(),
            32,
            "all 32 posts deduped into distinct entries"
        );

        // The drain window closes on the keep-alive ACK reporting 0 — this
        // mission's own liveness/backlog-depth probe.
        let mut ka_raw = [0u8; 64];
        let ka_n = encode_keep_alive(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            9000,
            0,
            &mut ka_raw,
        );
        let ka_ack = double
            .handle_keep_alive(&client.pubkey, &ka_raw[..ka_n])
            .expect("keep-alive must be accepted");
        let unsynced = decode_keep_alive_ack(&ka_ack).unwrap().unsynced_count;
        assert_eq!(unsynced, 0, "everything was drained: nothing left unsynced");
        assert_eq!(
            phase.on_keep_alive_ack(unsynced),
            Some(RoomNotification::Aggregate { count: 32 }),
            "closing the drain window fires exactly ONE aggregate notification for all 32"
        );
    }
}
