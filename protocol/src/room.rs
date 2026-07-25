// SPDX-License-Identifier: GPL-3.0-only
//! MeshCore room-server client codec — `simple_room_server` protocol.
//!
//! This module is the **client** side of the room-server protocol only: login,
//! decoding pushed posts, posting, and keep-alive. It does not implement a room
//! server (MeshCadet-as-room-server is out of scope), but it does ship a
//! faithful [`RoomServerDouble`] test double so the rest of the room-server
//! support work has something byte-accurate to test against without
//! hardware-in-the-loop.
//!
//! Source references (@ MeshCore v1.16.0, `07a3ca9e`, tag `v1.16.0`):
//!   `examples/simple_room_server/MyMesh.cpp` — room-server application logic
//!   `src/Mesh.cpp`                           — `createAnonDatagram`, `createDatagram`, `createAck`
//!   `src/helpers/ClientACL.h`                — permission constants
//!
//! Wire layouts (see each function's doc comment for the precise citation):
//!
//!   ANON_REQ (login, client→server):
//!     [dest_hash(1)] [sender_pubkey(32)] [HMAC(2)] [AES-128-ECB ciphertext]
//!     plaintext: [timestamp_le(4)] [sync_since_le(4)] [password NUL-terminated]
//!
//!   RESPONSE (login reply, server→client), 13-byte plaintext:
//!     [server_ts(4)] [code(1)] [legacy_keepalive(1)] [role_code(1)]
//!     [permissions(1)] [random(4)] [firmware_ver_level(1)]
//!
//!   TXT_MSG push (post, server→client), TXT_TYPE_SIGNED_PLAIN:
//!     [post_ts(4)] [(SIGNED_PLAIN<<2)|attempt(1)] [author_pubkey_prefix(4)] [text]
//!
//!   TXT_MSG post (client→server), TXT_TYPE_PLAIN:
//!     [ts(4)] [(PLAIN<<2)|attempt(1)] [text NUL-terminated]
//!
//!   REQ keep-alive (client→server), route-direct only:
//!     [ts(4)] [REQ_TYPE_KEEP_ALIVE=0x02(1)] [force_since_le(4)]
//!
//!   ACK payloads (unencrypted, raw on the wire):
//!     push ack:       [ack_hash(4)]                     = sha256(push_plaintext || client_pubkey)[0..4]
//!     post ack:       [ack_hash(4)]                     = sha256(post_plaintext || client_pubkey)[0..4]
//!     keep-alive ack: [ack_hash(4)] [unsynced_count(1)] = sha256(ts||type||force_since || client_pubkey)[0..4], + count

use crate::codec::{
    compute_ack_hash, decode_dm_payload, decode_txt_msg_plaintext, encode_dm_payload, CodecError,
};
use crate::constants::PUB_KEY_SIZE;
use crate::crypto::{encrypt_then_mac_var, mac_then_decrypt_var};
use crate::identity::Identity;

// ── Errors ───────────────────────────────────────────────────────────────────

/// Errors from room-protocol encode/decode operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomCodecError {
    /// Wraps a lower-level DM/AES/MAC codec error.
    Codec(CodecError),
    /// Payload is shorter than the minimum for this room-protocol structure.
    TruncatedPayload,
    /// `decode_room_push` received a TXT_MSG that is not `TXT_TYPE_SIGNED_PLAIN`
    /// — either a mis-routed plain DM or an unrecognized push variant.
    NotSignedPlain,
    /// `decode_login_response` saw `code != RESP_SERVER_LOGIN_OK`.
    LoginRejected(u8),
}

impl From<CodecError> for RoomCodecError {
    fn from(e: CodecError) -> Self {
        RoomCodecError::Codec(e)
    }
}

// ── Permissions (ClientACL.h:7-11 @ MeshCore v1.16.0) ──────────────────────────

/// Lower-2-bit role mask applied to the raw `permissions` byte.
pub const PERM_ACL_ROLE_MASK: u8 = 0x03;

/// A room client's access-control role.
///
/// `Guest` is READ-ONLY and cannot post — it is the wrong-password fallback a
/// server hands out when it has `allow_read_only` set. The distinct *guest
/// password* (a correct password, just not the admin one) grants `ReadWrite`.
/// Do not confuse the two: this type exists so a `u8` permissions byte can't be
/// silently misread as "authenticated" when it is in fact the no-password
/// fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomPermission {
    /// Wrong-password (or no-password) fallback role. Read-only; cannot post.
    Guest = 0,
    /// Reserved role value; not reachable via `simple_room_server`'s login path
    /// (only `Guest`/`ReadWrite`/`Admin` are ever granted at login) but a valid
    /// wire value an ACL-managed client could carry. Cannot post.
    ReadOnly = 1,
    /// Granted by a correct *guest password*. Can post.
    ReadWrite = 2,
    /// Granted by a correct admin password. Can post; admin-only commands are
    /// out of scope for this crate.
    Admin = 3,
}

impl RoomPermission {
    /// Decode the role from a raw `permissions` byte, masking off any bits
    /// outside `PERM_ACL_ROLE_MASK`.
    pub fn from_u8(raw: u8) -> Self {
        match raw & PERM_ACL_ROLE_MASK {
            0 => RoomPermission::Guest,
            1 => RoomPermission::ReadOnly,
            2 => RoomPermission::ReadWrite,
            _ => RoomPermission::Admin,
        }
    }

    /// Whether this role is permitted to post (`>= ReadWrite`). A sub-`ReadWrite`
    /// client's post is silently dropped by the server — no ACK, no error
    /// (`MyMesh.cpp:466`).
    pub fn can_post(self) -> bool {
        matches!(self, RoomPermission::ReadWrite | RoomPermission::Admin)
    }
}

// ── TXT_MSG flags (TxtDataHelpers.h @ MeshCore v1.16.0) ─────────────────────────

/// Plain client-authored text (posts).
pub const TXT_TYPE_PLAIN: u8 = 0;
/// CLI command / response (admin-only; out of scope for this crate).
pub const TXT_TYPE_CLI_DATA: u8 = 1;
/// Server-authored, author-attributed push (room posts pushed to clients).
pub const TXT_TYPE_SIGNED_PLAIN: u8 = 2;

/// `REQ_TYPE_KEEP_ALIVE` request-type byte (`MyMesh.cpp:15`).
pub const REQ_TYPE_KEEP_ALIVE: u8 = 0x02;

/// `RESP_SERVER_LOGIN_OK` response code (`MyMesh.cpp:19`).
pub const RESP_SERVER_LOGIN_OK: u8 = 0;

// ── ANON_REQ login (client→server) ──────────────────────────────────────────

/// `[dest_hash(1)] [sender_pubkey(32)]` header prefix, ahead of the
/// `[HMAC(2)][ciphertext]` blob (`Mesh.cpp::createAnonDatagram`, ANON_REQ arm).
const ANON_REQ_HEADER_LEN: usize = 1 + PUB_KEY_SIZE;

/// Max password length this codec will encode, NUL byte included
/// (`CommonCLI.h`'s `password[16]` / `guest_password[16]`).
pub const MAX_LOGIN_PASSWORD_LEN: usize = 16;

/// Encode an ANON_REQ login request.
///
/// `shared_secret` is the ECDH shared secret to the room server's pubkey —
/// compute it with [`Identity::ecdh_shared_secret`] before calling. `dest_hash`
/// is the room server's 1-byte routing hash (its `pubkey[0]`); `sender_pubkey`
/// is the caller's own full 32-byte pubkey, carried inline (ANON_REQ has no
/// prior session to address by hash — the server doesn't know the caller yet).
///
/// `password` is truncated to `MAX_LOGIN_PASSWORD_LEN - 1` bytes and always
/// NUL-terminated in the plaintext, matching `data[8]` onward in
/// `onAnonDataRecv` (`MyMesh.cpp:310-345`).
///
/// Returns the number of bytes written to `out`. `out` must be at least
/// `ANON_REQ_HEADER_LEN + 2 + ceil_16(8 + password.len().min(15) + 1)` bytes.
pub fn encode_anon_req_login(
    shared_secret: &[u8; 32],
    dest_hash: u8,
    sender_pubkey: &[u8; 32],
    timestamp: u32,
    sync_since: u32,
    password: &[u8],
    out: &mut [u8],
) -> usize {
    let mut pt = [0u8; 8 + MAX_LOGIN_PASSWORD_LEN];
    pt[0..4].copy_from_slice(&timestamp.to_le_bytes());
    pt[4..8].copy_from_slice(&sync_since.to_le_bytes());
    let pw_len = password.len().min(MAX_LOGIN_PASSWORD_LEN - 1);
    pt[8..8 + pw_len].copy_from_slice(&password[..pw_len]);
    // pt[8 + pw_len] is already 0 — the NUL terminator.
    let pt_len = 8 + pw_len + 1;

    out[0] = dest_hash;
    out[1..1 + PUB_KEY_SIZE].copy_from_slice(sender_pubkey);

    let mut aes_key = [0u8; 16];
    aes_key.copy_from_slice(&shared_secret[..16]);
    let mac_ct_len = encrypt_then_mac_var(
        &aes_key,
        shared_secret,
        &pt[..pt_len],
        &mut out[ANON_REQ_HEADER_LEN..],
    );
    ANON_REQ_HEADER_LEN + mac_ct_len
}

// ── Login response (server→client) ──────────────────────────────────────────

/// The 13-byte decoded RESPONSE plaintext to a room-server login.
///
/// Layout (`MyMesh.cpp:366-395`): `[server_ts(4)][code(1)][legacy_keepalive(1)]
/// [role_code(1)][permissions(1)][random(4)][firmware_ver_level(1)]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoginResponse {
    /// The server's clock at reply time (also used as a packet-hash uniquifier).
    pub server_ts: u32,
    /// The response code; always `RESP_SERVER_LOGIN_OK` when this struct is
    /// returned (a non-OK code is rejected as `Err` — see [`decode_login_response`]).
    pub code: u8,
    /// Legacy role-code byte (`1` = admin, `2` = guest/no-permissions, `0`
    /// otherwise). Superseded by `permissions`; kept for completeness.
    pub role_code: u8,
    /// The granted role.
    pub permissions: RoomPermission,
    /// Server firmware version level (`FIRMWARE_VER_LEVEL`).
    pub firmware_ver_level: u8,
}

/// Decode a 13-byte login-RESPONSE plaintext, from either leg:
/// - a direct `RESPONSE` datagram (decrypt with [`crate::codec::decode_dm_payload`] first), or
/// - a flood-routed login's bundled `PathExtra::Response` (from
///   [`crate::codec::decode_path_return`]).
///
/// Rejects `code != RESP_SERVER_LOGIN_OK` rather than returning a struct the
/// caller might mistake for a successful login.
pub fn decode_login_response(plaintext: &[u8]) -> Result<LoginResponse, RoomCodecError> {
    if plaintext.len() < 13 {
        return Err(RoomCodecError::TruncatedPayload);
    }
    let server_ts = u32::from_le_bytes([plaintext[0], plaintext[1], plaintext[2], plaintext[3]]);
    let code = plaintext[4];
    let role_code = plaintext[6];
    let permissions = RoomPermission::from_u8(plaintext[7]);
    let firmware_ver_level = plaintext[12];

    if code != RESP_SERVER_LOGIN_OK {
        return Err(RoomCodecError::LoginRejected(code));
    }

    Ok(LoginResponse {
        server_ts,
        code,
        role_code,
        permissions,
        firmware_ver_level,
    })
}

// ── Room push decode (server→client TXT_MSG, SIGNED_PLAIN) ─────────────────────

/// A decoded room push: a post the server relayed to this client.
///
/// `text_offset`/`text_len` index into the `plaintext_buf` passed to
/// [`decode_room_push`], exactly like [`crate::codec::GrpTxtFields`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoomPush {
    /// The post's original timestamp (`post.post_timestamp` server-side).
    pub post_ts: u32,
    /// Retry/attempt counter carried in the low 2 bits of the type byte.
    pub attempt: u8,
    /// First 4 bytes of the post author's pubkey.
    pub author_pubkey_prefix: [u8; 4],
    /// Start offset of the post text within `plaintext_buf`.
    pub text_offset: usize,
    /// Length of the post text, trimmed to the first zero byte within the
    /// AES-ECB zero-padded region (mirrors `BaseChatMesh.cpp`'s
    /// `data[len]=0; strlen(...)` c-string convention on the receive side) —
    /// this is the *exact* text extent the sender hashed into the push ACK,
    /// not the padded ciphertext length.
    pub text_len: usize,
}

impl RoomPush {
    /// The exact byte range `[author_pubkey_prefix(4)][text]` this push was
    /// ACK-hashed over (`room_push_ack_hash`'s `push_body` argument), sliced out
    /// of the same `plaintext_buf` passed to [`decode_room_push`].
    pub fn push_body<'a>(&self, plaintext_buf: &'a [u8]) -> &'a [u8] {
        &plaintext_buf[self.text_offset - 4..self.text_offset + self.text_len]
    }
}

/// Decode a room push: a `TXT_MSG` DM whose flags are `TXT_TYPE_SIGNED_PLAIN`
/// (`MyMesh.cpp:53-90`, `pushPostToClient`).
///
/// The existing [`crate::codec::decode_dm_payload`] + [`decode_txt_msg_plaintext`]
/// pair assumes `TXT_TYPE_PLAIN` (no author prefix); a `SIGNED_PLAIN` frame has a
/// 4-byte author-pubkey prefix ahead of the text that would otherwise be
/// mis-parsed as text. This function branches on the decoded flags rather than
/// giving callers two DM entry points to pick between (and possibly pick wrong).
///
/// Returns `(dest_hash, src_hash, RoomPush)`. Fails with
/// [`RoomCodecError::NotSignedPlain`] if the decoded TXT_MSG is not
/// `TXT_TYPE_SIGNED_PLAIN` — use [`crate::codec::decode_dm_payload`] +
/// [`decode_txt_msg_plaintext`] directly for plain DMs.
pub fn decode_room_push(
    shared_secret: &[u8; 32],
    payload: &[u8],
    plaintext_buf: &mut [u8],
) -> Result<(u8, u8, RoomPush), RoomCodecError> {
    let (dest_hash, src_hash, pt_len) = decode_dm_payload(shared_secret, payload, plaintext_buf)?;
    let (post_ts, txt_type, attempt, off) = decode_txt_msg_plaintext(plaintext_buf, pt_len)?;
    if txt_type != TXT_TYPE_SIGNED_PLAIN {
        return Err(RoomCodecError::NotSignedPlain);
    }
    if pt_len < off + 4 {
        return Err(RoomCodecError::TruncatedPayload);
    }
    let mut author_pubkey_prefix = [0u8; 4];
    author_pubkey_prefix.copy_from_slice(&plaintext_buf[off..off + 4]);
    let text_offset = off + 4;
    let raw_text_len = pt_len - text_offset;
    // Trim AES-ECB zero-padding at the first embedded zero byte, exactly as
    // the firmware's `data[len]=0; strlen(&data[9])` does on receive.
    let text_len = plaintext_buf[text_offset..text_offset + raw_text_len]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(raw_text_len);
    Ok((
        dest_hash,
        src_hash,
        RoomPush {
            post_ts,
            attempt,
            author_pubkey_prefix,
            text_offset,
            text_len,
        },
    ))
}

/// Compute the push ACK hash a client sends back to the server after receiving
/// a room push: `sha256(push_plaintext || client_pubkey)[0..4]`
/// (`MyMesh.cpp:71`), where `push_plaintext = [post_ts(4)][type_byte(1)]
/// [author_pubkey_prefix(4)][text]` and `client_pubkey` is the *receiving*
/// client's own pubkey (not the post author's).
///
/// `push_body` must be `[author_pubkey_prefix(4)][text]` — exactly
/// [`RoomPush::push_body`]'s output, so this composes directly with
/// [`decode_room_push`]'s result.
pub fn room_push_ack_hash(
    post_ts: u32,
    attempt: u8,
    push_body: &[u8],
    client_pubkey: &[u8; 32],
) -> [u8; 4] {
    let type_byte = (TXT_TYPE_SIGNED_PLAIN << 2) | (attempt & 0x03);
    compute_ack_hash(post_ts, type_byte, push_body, client_pubkey)
}

// ── Room post (client→server TXT_MSG, PLAIN) ────────────────────────────────

/// Max post text this codec will encode (generous; well under
/// `MAX_PACKET_PAYLOAD` minus the DM envelope and NUL terminator).
pub const MAX_POST_TEXT_LEN: usize = 150;

/// Encode a room post: a `TXT_MSG` DM to the server, `TXT_TYPE_PLAIN`
/// (`MyMesh.cpp:427-470`, `onPeerDataRecv`'s `TXT_TYPE_PLAIN` arm).
///
/// `text` is truncated to `MAX_POST_TEXT_LEN` bytes and always NUL-terminated
/// in the plaintext.
///
/// Returns the number of bytes written to `out`.
pub fn encode_room_post(
    shared_secret: &[u8; 32],
    dest_hash: u8,
    src_hash: u8,
    timestamp: u32,
    attempt: u8,
    text: &[u8],
    out: &mut [u8],
) -> usize {
    let mut pt = [0u8; 6 + MAX_POST_TEXT_LEN];
    pt[0..4].copy_from_slice(&timestamp.to_le_bytes());
    pt[4] = (TXT_TYPE_PLAIN << 2) | (attempt & 0x03);
    let text_len = text.len().min(MAX_POST_TEXT_LEN);
    pt[5..5 + text_len].copy_from_slice(&text[..text_len]);
    // pt[5 + text_len] is already 0 — the NUL terminator.
    let pt_len = 5 + text_len + 1;
    encode_dm_payload(shared_secret, dest_hash, src_hash, &pt[..pt_len], out)
}

/// Compute the post ACK hash the server sends back to a client after ingesting
/// a post: `sha256(post_plaintext || client_pubkey)[0..4]` (`MyMesh.cpp:445-448`),
/// where `post_plaintext = [ts(4)][type_byte(1)][text]` (NUL terminator
/// excluded — the hash covers exactly `5 + strlen(text)` bytes) and
/// `client_pubkey` is the *posting* client's own pubkey.
pub fn room_post_ack_hash(
    timestamp: u32,
    attempt: u8,
    text: &[u8],
    client_pubkey: &[u8; 32],
) -> [u8; 4] {
    let type_byte = (TXT_TYPE_PLAIN << 2) | (attempt & 0x03);
    compute_ack_hash(timestamp, type_byte, text, client_pubkey)
}

// ── Keep-alive (client→server REQ) ──────────────────────────────────────────

/// `[ts(4)][REQ_TYPE_KEEP_ALIVE(1)][force_since(4)]` plaintext length.
const KEEP_ALIVE_PLAINTEXT_LEN: usize = 9;

/// Encode a keep-alive request: a `REQ` DM, **route-direct only**
/// (`MyMesh.cpp:524-560`) — a room server ignores `REQ_TYPE_KEEP_ALIVE` unless
/// `packet->isRouteDirect()`, so callers must not flood-route this frame.
///
/// `force_since`, if non-zero, force-updates the server's view of this
/// client's `sync_since` (letting a client rewind or fast-forward its sync
/// point). Pass `0` to leave `sync_since` untouched.
pub fn encode_keep_alive(
    shared_secret: &[u8; 32],
    dest_hash: u8,
    src_hash: u8,
    timestamp: u32,
    force_since: u32,
    out: &mut [u8],
) -> usize {
    let mut pt = [0u8; KEEP_ALIVE_PLAINTEXT_LEN];
    pt[0..4].copy_from_slice(&timestamp.to_le_bytes());
    pt[4] = REQ_TYPE_KEEP_ALIVE;
    pt[5..9].copy_from_slice(&force_since.to_le_bytes());
    encode_dm_payload(shared_secret, dest_hash, src_hash, &pt, out)
}

/// Compute the ACK hash a server sends back for a keep-alive:
/// `sha256(ts(4) || REQ_TYPE_KEEP_ALIVE(1) || force_since(4) || client_pubkey)[0..4]`
/// (`MyMesh.cpp:552-558`) — this is `sha256(data[0..9] || client_pubkey)[0..4]`
/// where `data` is the keep-alive plaintext, zero-filled at `[5..9]` when the
/// client omitted `force_since`.
pub fn keep_alive_ack_hash(timestamp: u32, force_since: u32, client_pubkey: &[u8; 32]) -> [u8; 4] {
    compute_ack_hash(
        timestamp,
        REQ_TYPE_KEEP_ALIVE,
        &force_since.to_le_bytes(),
        client_pubkey,
    )
}

/// A decoded keep-alive ACK: the standard 4-byte ACK hash plus the
/// **appended unsynced-count byte** (`MyMesh.cpp:558`) — this is how a client
/// learns its backlog depth without a full sync round trip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeepAliveAck {
    /// The 4-byte ACK hash (compare against [`keep_alive_ack_hash`]).
    pub ack_hash: [u8; 4],
    /// Count of posts newer than this client's `sync_since` still awaiting push.
    pub unsynced_count: u8,
}

/// Decode a keep-alive ACK payload (raw, unencrypted — ACK frames are never
/// AES-wrapped; see `Mesh.cpp::createAck`).
pub fn decode_keep_alive_ack(payload: &[u8]) -> Result<KeepAliveAck, RoomCodecError> {
    if payload.len() < 5 {
        return Err(RoomCodecError::TruncatedPayload);
    }
    let mut ack_hash = [0u8; 4];
    ack_hash.copy_from_slice(&payload[0..4]);
    Ok(KeepAliveAck {
        ack_hash,
        unsynced_count: payload[4],
    })
}

// ── Room-server test double ─────────────────────────────────────────────────
//
// A from-scratch, faithful re-implementation of `simple_room_server`'s server
// side, driven entirely off this module's client-side codec. It is the
// campaign's standing substitute for mid-campaign hardware-in-the-loop and is
// reused by later children — a first-class deliverable, not a test fixture.
//
// Two behaviours it MUST reproduce (they drive later children):
//   - The sync is a serialized drip, not a burst: the server pushes ONE post,
//     sets `pending_ack`, and refuses to push again (even to a DIFFERENT
//     eligible post) until that ACK arrives (`MyMesh.cpp:53-110`, `loop()`
//     `did_push` gating @ `MyMesh.cpp:962-985`). A client that never ACKs
//     stalls its own sync permanently.
//   - A sub-`ReadWrite` client's post is silently dropped: no ACK, no error
//     (`MyMesh.cpp:466`).

/// Max concurrently-registered clients the double tracks.
pub const MAX_ROOM_CLIENTS: usize = 8;
/// Cyclic post-queue depth, matching `MAX_UNSYNCED_POSTS` (`MyMesh.h`).
pub const MAX_UNSYNCED_POSTS: usize = 32;

#[derive(Clone, Copy)]
struct DoubleClient {
    used: bool,
    pubkey: [u8; 32],
    shared_secret: [u8; 32],
    permissions: RoomPermission,
    sync_since: u32,
    last_timestamp: u32,
    /// `Some(hash)` while a push is in flight and unacknowledged — the drip gate.
    pending_ack: Option<[u8; 4]>,
    /// The timestamp `sync_since` advances to once `pending_ack` is satisfied.
    push_post_ts: u32,
}

impl DoubleClient {
    const EMPTY: Self = Self {
        used: false,
        pubkey: [0; 32],
        shared_secret: [0; 32],
        permissions: RoomPermission::Guest,
        sync_since: 0,
        last_timestamp: 0,
        pending_ack: None,
        push_post_ts: 0,
    };
}

#[derive(Clone, Copy)]
struct DoublePost {
    used: bool,
    author_pubkey: [u8; 32],
    timestamp: u32,
    text: [u8; MAX_POST_TEXT_LEN],
    text_len: usize,
}

impl DoublePost {
    const EMPTY: Self = Self {
        used: false,
        author_pubkey: [0; 32],
        timestamp: 0,
        text: [0; MAX_POST_TEXT_LEN],
        text_len: 0,
    };
}

/// A from-scratch double of `simple_room_server`'s server-side state machine.
///
/// Operates on raw wire bytes in both directions (the same bytes a real client
/// built with this module's `encode_*`/`decode_*` functions would send/receive),
/// so tests exercise the full codec round trip, not just internal state.
pub struct RoomServerDouble {
    identity: Identity,
    admin_password: [u8; MAX_LOGIN_PASSWORD_LEN],
    admin_password_len: usize,
    guest_password: [u8; MAX_LOGIN_PASSWORD_LEN],
    guest_password_len: usize,
    allow_read_only: bool,
    clients: [DoubleClient; MAX_ROOM_CLIENTS],
    posts: [DoublePost; MAX_UNSYNCED_POSTS],
    next_post_idx: usize,
}

impl RoomServerDouble {
    /// Construct a double with the given server identity and password policy.
    /// `allow_read_only` mirrors `_prefs.allow_read_only`: when set, a wrong
    /// (non-admin, non-guest) password is granted `RoomPermission::Guest`
    /// instead of being rejected outright.
    pub fn new(
        identity: Identity,
        admin_password: &[u8],
        guest_password: &[u8],
        allow_read_only: bool,
    ) -> Self {
        let mut admin = [0u8; MAX_LOGIN_PASSWORD_LEN];
        let admin_password_len = admin_password.len().min(MAX_LOGIN_PASSWORD_LEN - 1);
        admin[..admin_password_len].copy_from_slice(&admin_password[..admin_password_len]);
        let mut guest = [0u8; MAX_LOGIN_PASSWORD_LEN];
        let guest_password_len = guest_password.len().min(MAX_LOGIN_PASSWORD_LEN - 1);
        guest[..guest_password_len].copy_from_slice(&guest_password[..guest_password_len]);
        Self {
            identity,
            admin_password: admin,
            admin_password_len,
            guest_password: guest,
            guest_password_len,
            allow_read_only,
            clients: [DoubleClient::EMPTY; MAX_ROOM_CLIENTS],
            posts: [DoublePost::EMPTY; MAX_UNSYNCED_POSTS],
            next_post_idx: 0,
        }
    }

    fn client_idx(&self, pubkey: &[u8; 32]) -> Option<usize> {
        self.clients
            .iter()
            .position(|c| c.used && &c.pubkey == pubkey)
    }

    fn find_or_register_client(&mut self, pubkey: &[u8; 32]) -> Option<usize> {
        if let Some(i) = self.client_idx(pubkey) {
            return Some(i);
        }
        self.clients.iter().position(|c| !c.used)
    }

    /// Handle a raw ANON_REQ login frame (as built by [`encode_anon_req_login`]).
    ///
    /// Returns `None` when the real server would give **no response at all**
    /// (wrong password with `allow_read_only` unset, or a replayed timestamp)
    /// — the client "will timeout" (`MyMesh.cpp:337`), exactly as documented.
    /// On success, writes a `RESPONSE` DM to `out` and returns its length; feed
    /// that through [`crate::codec::decode_dm_payload`] + [`decode_login_response`]
    /// exactly as a real client would.
    pub fn handle_login(&mut self, anon_req_payload: &[u8], out: &mut [u8]) -> Option<usize> {
        if anon_req_payload.len() < ANON_REQ_HEADER_LEN + 2 {
            return None;
        }
        let dest_hash = anon_req_payload[0];
        if dest_hash != self.identity.pub_hash() {
            return None; // not addressed to us
        }
        let mut sender_pubkey = [0u8; 32];
        sender_pubkey.copy_from_slice(&anon_req_payload[1..ANON_REQ_HEADER_LEN]);
        let shared_secret = self.identity.ecdh_shared_secret(&sender_pubkey);
        let mut aes_key = [0u8; 16];
        aes_key.copy_from_slice(&shared_secret[..16]);

        let mut pt = [0u8; 64];
        let mac_and_ct = &anon_req_payload[ANON_REQ_HEADER_LEN..];
        let pt_len = mac_then_decrypt_var(&aes_key, &shared_secret, mac_and_ct, &mut pt).ok()?;
        if pt_len < 9 {
            return None;
        }
        let timestamp = u32::from_le_bytes([pt[0], pt[1], pt[2], pt[3]]);
        let sync_since = u32::from_le_bytes([pt[4], pt[5], pt[6], pt[7]]);
        let pw_end = pt[8..pt_len]
            .iter()
            .position(|&b| b == 0)
            .map(|i| 8 + i)
            .unwrap_or(pt_len);
        let password = &pt[8..pw_end];

        let permissions = if !password.is_empty()
            && password == &self.admin_password[..self.admin_password_len]
        {
            RoomPermission::Admin
        } else if !password.is_empty()
            && password == &self.guest_password[..self.guest_password_len]
        {
            RoomPermission::ReadWrite
        } else if self.allow_read_only {
            RoomPermission::Guest
        } else {
            return None; // incorrect password, no read-only fallback: no response
        };

        let idx = self.find_or_register_client(&sender_pubkey)?;
        let already_known = self.clients[idx].used;
        if already_known && timestamp <= self.clients[idx].last_timestamp {
            return None; // possible replay attack (MyMesh.cpp:345-348)
        }

        let client = &mut self.clients[idx];
        client.used = true;
        client.pubkey = sender_pubkey;
        client.shared_secret = shared_secret;
        client.permissions = permissions;
        client.sync_since = sync_since;
        client.last_timestamp = timestamp;
        client.pending_ack = None;

        let mut reply = [0u8; 13];
        reply[0..4].copy_from_slice(&timestamp.to_le_bytes()); // reflect a "now"; double has no RTC of its own
        reply[4] = RESP_SERVER_LOGIN_OK;
        reply[5] = 0; // legacy keepalive interval
        reply[6] = match permissions {
            RoomPermission::Admin => 1,
            RoomPermission::Guest => 2,
            _ => 0,
        };
        reply[7] = permissions as u8;
        reply[8..12].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]); // deterministic filler (real server: RNG)
        reply[12] = 1; // firmware_ver_level

        Some(encode_dm_payload(
            &shared_secret,
            sender_pubkey[0],
            self.identity.pub_hash(),
            &reply,
            out,
        ))
    }

    /// Directly seed a post into the cyclic queue, as if `client_pubkey` had
    /// posted it — bypasses [`Self::handle_post`]'s wire decode, for setting up
    /// posts authored by clients other than the one under test.
    pub fn seed_post(&mut self, author_pubkey: &[u8; 32], timestamp: u32, text: &[u8]) {
        let idx = self.next_post_idx;
        let text_len = text.len().min(MAX_POST_TEXT_LEN);
        let mut buf = [0u8; MAX_POST_TEXT_LEN];
        buf[..text_len].copy_from_slice(&text[..text_len]);
        self.posts[idx] = DoublePost {
            used: true,
            author_pubkey: *author_pubkey,
            timestamp,
            text: buf,
            text_len,
        };
        self.next_post_idx = (self.next_post_idx + 1) % MAX_UNSYNCED_POSTS;
    }

    /// Attempt to push the next unsynced, non-self-authored post to `client_pubkey`.
    ///
    /// Returns `None` — and pushes nothing — when: the client is unknown, a
    /// push is already in flight (`pending_ack.is_some()`, the drip gate), or
    /// there is no eligible post. On success, writes the push DM to `out`,
    /// marks `pending_ack`, and returns the DM's length. Sync does not advance
    /// until [`Self::handle_ack`] clears `pending_ack` — call this again with
    /// no intervening ACK and it MUST return `None` again (that is the
    /// serialized-drip invariant, not a burst).
    pub fn push_next(&mut self, client_pubkey: &[u8; 32], out: &mut [u8]) -> Option<usize> {
        let idx = self.client_idx(client_pubkey)?;
        if self.clients[idx].pending_ack.is_some() {
            return None; // still awaiting the previous push's ACK
        }
        let sync_since = self.clients[idx].sync_since;

        let mut chosen = None;
        for k in 0..MAX_UNSYNCED_POSTS {
            let pidx = (self.next_post_idx + k) % MAX_UNSYNCED_POSTS;
            let p = &self.posts[pidx];
            if p.used && p.timestamp > sync_since && &p.author_pubkey != client_pubkey {
                chosen = Some(pidx);
                break;
            }
        }
        let pidx = chosen?;
        let post = self.posts[pidx];

        let attempt: u8 = 0; // real server randomizes only to perturb the packet hash
        let mut pt = [0u8; 9 + MAX_POST_TEXT_LEN];
        pt[0..4].copy_from_slice(&post.timestamp.to_le_bytes());
        pt[4] = (TXT_TYPE_SIGNED_PLAIN << 2) | (attempt & 0x03);
        pt[5..9].copy_from_slice(&post.author_pubkey[..4]);
        pt[9..9 + post.text_len].copy_from_slice(&post.text[..post.text_len]);
        let pt_len = 9 + post.text_len;

        let ack_hash = compute_ack_hash(post.timestamp, pt[4], &pt[5..pt_len], client_pubkey);

        let client = &mut self.clients[idx];
        client.pending_ack = Some(ack_hash);
        client.push_post_ts = post.timestamp;
        let shared_secret = client.shared_secret;

        Some(encode_dm_payload(
            &shared_secret,
            client_pubkey[0],
            self.identity.pub_hash(),
            &pt[..pt_len],
            out,
        ))
    }

    /// Handle an ACK for an in-flight push. Clears `pending_ack` and advances
    /// `sync_since` to the acked post's timestamp on a match
    /// (`MyMesh.cpp::processAck`). Returns whether it matched.
    pub fn handle_ack(&mut self, client_pubkey: &[u8; 32], ack: &[u8; 4]) -> bool {
        let Some(idx) = self.client_idx(client_pubkey) else {
            return false;
        };
        let client = &mut self.clients[idx];
        if client.pending_ack == Some(*ack) {
            client.pending_ack = None;
            client.sync_since = client.push_post_ts;
            true
        } else {
            false
        }
    }

    /// Handle a raw room-post DM (as built by [`encode_room_post`]).
    ///
    /// Returns `None` — with **no ACK emitted and no post stored** — for a
    /// replayed timestamp, or for a sub-`ReadWrite` client's post
    /// (`MyMesh.cpp:466`; silently dropped, no ACK, no error). On success,
    /// returns the ACK hash the caller should send back to the client.
    pub fn handle_post(&mut self, client_pubkey: &[u8; 32], payload: &[u8]) -> Option<[u8; 4]> {
        let idx = self.client_idx(client_pubkey)?;
        let shared_secret = self.clients[idx].shared_secret;

        let mut pt = [0u8; 256];
        let (_dest, _src, pt_len) = decode_dm_payload(&shared_secret, payload, &mut pt).ok()?;
        let (timestamp, txt_type, attempt, off) = decode_txt_msg_plaintext(&pt, pt_len).ok()?;
        if txt_type != TXT_TYPE_PLAIN {
            return None; // CLI_DATA / unrecognized flags: out of scope for this double
        }
        if timestamp < self.clients[idx].last_timestamp {
            return None; // possible replay attack
        }
        let is_retry = timestamp == self.clients[idx].last_timestamp;
        self.clients[idx].last_timestamp = timestamp;

        if !self.clients[idx].permissions.can_post() {
            return None; // sub-ReadWrite: silently dropped, no ACK (MyMesh.cpp:466)
        }

        let text_end = pt[off..pt_len]
            .iter()
            .position(|&b| b == 0)
            .map(|i| off + i)
            .unwrap_or(pt_len);
        let text = &pt[off..text_end];

        if !is_retry {
            self.seed_post(client_pubkey, timestamp, text);
        }

        Some(room_post_ack_hash(timestamp, attempt, text, client_pubkey))
    }

    /// Handle a raw keep-alive DM (as built by [`encode_keep_alive`]).
    ///
    /// Returns `None` for a replayed timestamp. On success, clears
    /// `pending_ack` (the real server does this unconditionally on any valid
    /// keep-alive — `MyMesh.cpp:547`), applies `force_since` if non-zero, and
    /// returns the 5-byte ACK payload (`[ack_hash(4)][unsynced_count(1)]`).
    pub fn handle_keep_alive(
        &mut self,
        client_pubkey: &[u8; 32],
        payload: &[u8],
    ) -> Option<[u8; 5]> {
        let idx = self.client_idx(client_pubkey)?;
        let shared_secret = self.clients[idx].shared_secret;

        let mut pt = [0u8; 32];
        let (_dest, _src, pt_len) = decode_dm_payload(&shared_secret, payload, &mut pt).ok()?;
        if pt_len < 5 || pt[4] != REQ_TYPE_KEEP_ALIVE {
            return None;
        }
        let timestamp = u32::from_le_bytes([pt[0], pt[1], pt[2], pt[3]]);
        if timestamp < self.clients[idx].last_timestamp {
            return None; // possible replay attack
        }
        self.clients[idx].last_timestamp = timestamp;

        let force_since = if pt_len >= 9 {
            u32::from_le_bytes([pt[5], pt[6], pt[7], pt[8]])
        } else {
            0
        };
        if force_since > 0 {
            self.clients[idx].sync_since = force_since;
        }
        self.clients[idx].pending_ack = None;

        let unsynced_count = self.unsynced_count(idx);
        let ack_hash = keep_alive_ack_hash(timestamp, force_since, client_pubkey);
        let mut out = [0u8; 5];
        out[..4].copy_from_slice(&ack_hash);
        out[4] = unsynced_count;
        Some(out)
    }

    fn unsynced_count(&self, idx: usize) -> u8 {
        let sync_since = self.clients[idx].sync_since;
        let pubkey = self.clients[idx].pubkey;
        let mut count = 0u8;
        for p in self.posts.iter() {
            if p.used && p.timestamp > sync_since && p.author_pubkey != pubkey {
                count = count.saturating_add(1);
            }
        }
        count
    }

    /// The role currently on file for a registered client, if any.
    pub fn client_permissions(&self, client_pubkey: &[u8; 32]) -> Option<RoomPermission> {
        self.client_idx(client_pubkey)
            .map(|i| self.clients[i].permissions)
    }

    /// A registered client's current `sync_since` watermark.
    pub fn client_sync_since(&self, client_pubkey: &[u8; 32]) -> Option<u32> {
        self.client_idx(client_pubkey)
            .map(|i| self.clients[i].sync_since)
    }

    /// A registered client's in-flight push ACK hash, if a push is pending.
    pub fn client_pending_ack(&self, client_pubkey: &[u8; 32]) -> Option<[u8; 4]> {
        self.client_idx(client_pubkey)
            .and_then(|i| self.clients[i].pending_ack)
    }

    /// Test/setup-only escape hatch: force a registered client's permission to
    /// an arbitrary role, including `ReadOnly`, which `simple_room_server`'s
    /// login path never actually grants (only `Guest`/`ReadWrite`/`Admin` are
    /// reachable via password login — `ReadOnly` is only ever set by
    /// out-of-scope ACL management). Exists so the double can still honestly
    /// exercise the "sub-ReadWrite post is dropped" invariant for `ReadOnly`,
    /// not just `Guest`.
    #[cfg(test)]
    pub(crate) fn set_permissions_for_test(
        &mut self,
        client_pubkey: &[u8; 32],
        permissions: RoomPermission,
    ) {
        if let Some(i) = self.client_idx(client_pubkey) {
            self.clients[i].permissions = permissions;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{decode_dm_payload, decode_path_return_plaintext, CodecError, PathExtra};
    use crate::crypto::{mac_then_decrypt_var, sha256_2};

    fn make_pair() -> (Identity, Identity) {
        let client = Identity::from_seed([0x21u8; 32]);
        let server = Identity::from_seed([0x22u8; 32]);
        (client, server)
    }

    // ── Permissions ────────────────────────────────────────────────────────

    #[test]
    fn permission_role_mask_matches_client_acl_h() {
        // ClientACL.h:7-11 @ MeshCore v1.16.0: PERM_ACL_ROLE_MASK=3, GUEST=0,
        // READ_ONLY=1, READ_WRITE=2, ADMIN=3.
        assert_eq!(RoomPermission::from_u8(0), RoomPermission::Guest);
        assert_eq!(RoomPermission::from_u8(1), RoomPermission::ReadOnly);
        assert_eq!(RoomPermission::from_u8(2), RoomPermission::ReadWrite);
        assert_eq!(RoomPermission::from_u8(3), RoomPermission::Admin);
        // High bits (e.g. an admin flag byte with extra bits set) are masked off.
        assert_eq!(RoomPermission::from_u8(0xFC | 2), RoomPermission::ReadWrite);
    }

    #[test]
    fn guest_and_read_only_cannot_post_read_write_and_admin_can() {
        assert!(!RoomPermission::Guest.can_post());
        assert!(!RoomPermission::ReadOnly.can_post());
        assert!(RoomPermission::ReadWrite.can_post());
        assert!(RoomPermission::Admin.can_post());
    }

    // ── ANON_REQ login golden vector (MyMesh.cpp:310-345, Mesh.cpp createAnonDatagram) ──

    #[test]
    fn anon_req_login_golden_vector_layout() {
        let (client, server) = make_pair();
        let shared = client.ecdh_shared_secret(&server.pubkey);

        let mut out = [0u8; 128];
        let n = encode_anon_req_login(
            &shared,
            server.pub_hash(),
            &client.pubkey,
            0x0102_0304,
            0x0506_0708,
            b"hunter2",
            &mut out,
        );

        // Envelope: [dest_hash(1)][sender_pubkey(32)][HMAC(2)][ciphertext...]
        assert_eq!(out[0], server.pub_hash());
        assert_eq!(&out[1..33], &client.pubkey[..]);
        assert!(
            n > 33 + 2,
            "must carry at least one AES block of ciphertext"
        );

        // Decrypt the plaintext directly and pin the exact byte layout.
        let mut aes_key = [0u8; 16];
        aes_key.copy_from_slice(&shared[..16]);
        let mut pt = [0u8; 64];
        let pt_len = mac_then_decrypt_var(&aes_key, &shared, &out[33..n], &mut pt).unwrap();
        assert!(pt_len >= 16);
        assert_eq!(&pt[0..4], &0x0102_0304u32.to_le_bytes());
        assert_eq!(&pt[4..8], &0x0506_0708u32.to_le_bytes());
        assert_eq!(&pt[8..15], b"hunter2");
        assert_eq!(pt[15], 0, "password must be NUL-terminated");
    }

    #[test]
    fn anon_req_login_password_truncates_and_stays_nul_terminated() {
        let (client, server) = make_pair();
        let shared = client.ecdh_shared_secret(&server.pubkey);
        let long_password = [b'x'; 64]; // far beyond MAX_LOGIN_PASSWORD_LEN

        let mut out = [0u8; 128];
        let n = encode_anon_req_login(
            &shared,
            server.pub_hash(),
            &client.pubkey,
            1,
            0,
            &long_password,
            &mut out,
        );

        let mut aes_key = [0u8; 16];
        aes_key.copy_from_slice(&shared[..16]);
        let mut pt = [0u8; 64];
        mac_then_decrypt_var(&aes_key, &shared, &out[33..n], &mut pt).unwrap();
        // Search from offset 8 (start of the password field) — ts=1's LE bytes
        // ([1,0,0,0]) contain a zero byte too, which isn't the terminator we're
        // pinning here.
        let nul_at = pt[8..].iter().position(|&b| b == 0).map(|i| 8 + i).unwrap();
        assert_eq!(
            nul_at,
            8 + MAX_LOGIN_PASSWORD_LEN - 1,
            "password truncated to buffer capacity, then NUL-terminated"
        );
    }

    // ── Login response golden vector (MyMesh.cpp:366-395) ────────────────────

    #[test]
    fn login_response_golden_vector_decode() {
        let mut reply = [0u8; 13];
        reply[0..4].copy_from_slice(&0xAABB_CCDDu32.to_le_bytes()); // server_ts
        reply[4] = RESP_SERVER_LOGIN_OK;
        reply[5] = 0; // legacy keepalive
        reply[6] = 0; // role_code
        reply[7] = RoomPermission::ReadWrite as u8;
        reply[8..12].copy_from_slice(&[1, 2, 3, 4]); // random
        reply[12] = 1; // firmware_ver_level

        let decoded = decode_login_response(&reply).unwrap();
        assert_eq!(decoded.server_ts, 0xAABB_CCDD);
        assert_eq!(decoded.code, RESP_SERVER_LOGIN_OK);
        assert_eq!(decoded.permissions, RoomPermission::ReadWrite);
        assert_eq!(decoded.firmware_ver_level, 1);
    }

    #[test]
    fn login_response_rejects_non_ok_code() {
        let mut reply = [0u8; 13];
        reply[4] = 1; // any non-zero code
        assert_eq!(
            decode_login_response(&reply),
            Err(RoomCodecError::LoginRejected(1))
        );
    }

    #[test]
    fn login_response_truncated_is_rejected() {
        let short = [0u8; 12];
        assert_eq!(
            decode_login_response(&short),
            Err(RoomCodecError::TruncatedPayload)
        );
    }

    // ── PATH-return RESPONSE bundle (flood-login leg; decode_path_return extension) ──

    #[test]
    fn decode_path_return_accepts_bundled_response_extra() {
        let mut response = [0u8; 13];
        response[0..4].copy_from_slice(&42u32.to_le_bytes());
        response[4] = RESP_SERVER_LOGIN_OK;
        response[7] = RoomPermission::Admin as u8;
        response[12] = 1;

        let mut pt = [0u8; 32];
        pt[0] = 0x40; // path_len: 2B hash, 0 hops (server may not know a path yet)
        pt[1] = 0x01; // extra_type = PAYLOAD_TYPE_RESPONSE
        pt[2..15].copy_from_slice(&response);

        let rp = decode_path_return_plaintext(&pt, 15).unwrap();
        match rp.extra {
            PathExtra::Response(bundled) => {
                assert_eq!(bundled, response);
                let decoded = decode_login_response(&bundled).unwrap();
                assert_eq!(decoded.permissions, RoomPermission::Admin);
            }
            other => panic!("expected PathExtra::Response, got {other:?}"),
        }
    }

    #[test]
    fn decode_path_return_still_accepts_ack_bundle_unchanged() {
        // Regression guard: extending decode_path_return_plaintext for the
        // RESPONSE extra must not disturb the existing ACK bundle path.
        let mut pt = [0u8; 16];
        pt[0] = 0x40; // 0 hops
        pt[1] = 0x03; // extra_type = ACK
        pt[2..6].copy_from_slice(&[9, 8, 7, 6]);

        let rp = decode_path_return_plaintext(&pt, 6).unwrap();
        assert_eq!(rp.extra, PathExtra::Ack([9, 8, 7, 6]));
    }

    // ── Room push decode golden vector (MyMesh.cpp:53-90) ────────────────────

    #[test]
    fn decode_room_push_golden_vector() {
        let (client, server) = make_pair();
        let shared = server.ecdh_shared_secret(&client.pubkey);
        let author_prefix = [0x11, 0x22, 0x33, 0x44];
        let text = b"hello room";

        // Build the push plaintext directly (mirrors pushPostToClient's layout)
        // and wrap it in a DM envelope, same as the real server would.
        let ts: u32 = 0xDEAD_BEEF;
        let attempt: u8 = 2;
        let mut pt_in = [0u8; 32];
        pt_in[0..4].copy_from_slice(&ts.to_le_bytes());
        pt_in[4] = (TXT_TYPE_SIGNED_PLAIN << 2) | (attempt & 0x03);
        pt_in[5..9].copy_from_slice(&author_prefix);
        pt_in[9..9 + text.len()].copy_from_slice(text);

        let mut wire = [0u8; 256];
        let n = encode_dm_payload(
            &shared,
            client.pub_hash(),
            server.pub_hash(),
            &pt_in[..9 + text.len()],
            &mut wire,
        );

        let mut pt = [0u8; 256];
        let (dest, src, push) = decode_room_push(&shared, &wire[..n], &mut pt).unwrap();
        assert_eq!(dest, client.pub_hash());
        assert_eq!(src, server.pub_hash());
        assert_eq!(push.post_ts, 0xDEAD_BEEF);
        assert_eq!(push.attempt, 2);
        assert_eq!(push.author_pubkey_prefix, author_prefix);
        assert_eq!(
            &pt[push.text_offset..push.text_offset + push.text_len],
            b"hello room"
        );
    }

    #[test]
    fn decode_room_push_rejects_plain_txt_msg() {
        // A regular (TXT_TYPE_PLAIN) DM must not be mis-parsed as a push — the
        // defect this function exists to prevent.
        let (client, server) = make_pair();
        let shared = server.ecdh_shared_secret(&client.pubkey);

        let mut pt_buf = [0u8; 64];
        let pt_len = crate::codec::encode_txt_msg_plaintext(
            1,
            TXT_TYPE_PLAIN,
            0,
            b"not a push",
            &mut pt_buf,
        );
        let mut wire = [0u8; 256];
        let n = encode_dm_payload(
            &shared,
            client.pub_hash(),
            server.pub_hash(),
            &pt_buf[..pt_len],
            &mut wire,
        );

        let mut pt = [0u8; 256];
        assert_eq!(
            decode_room_push(&shared, &wire[..n], &mut pt),
            Err(RoomCodecError::NotSignedPlain)
        );
    }

    #[test]
    fn decode_room_push_wrong_shared_secret_fails_mac() {
        // Wrong ECDH secret (e.g. decoding with the wrong contact's key) must
        // be rejected at the MAC layer, not silently mis-decrypted.
        let (client, server) = make_pair();
        let shared = server.ecdh_shared_secret(&client.pubkey);
        let wrong_secret = [0xEEu8; 32];

        let mut pt_in = [0u8; 9];
        pt_in[0..4].copy_from_slice(&1u32.to_le_bytes());
        pt_in[4] = TXT_TYPE_SIGNED_PLAIN << 2;
        pt_in[5..9].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);

        let mut wire = [0u8; 256];
        let n = encode_dm_payload(
            &shared,
            client.pub_hash(),
            server.pub_hash(),
            &pt_in,
            &mut wire,
        );

        let mut pt = [0u8; 256];
        assert_eq!(
            decode_room_push(&wrong_secret, &wire[..n], &mut pt),
            Err(RoomCodecError::Codec(CodecError::MacMismatch))
        );
    }

    #[test]
    fn room_push_ack_hash_known_answer() {
        // MyMesh.cpp:71 — sha256(reply_data[0..len] || client.id.pub_key)[0..4]
        let ts: u32 = 7;
        let attempt: u8 = 1;
        let author_prefix = [0xAA, 0xBB, 0xCC, 0xDD];
        let text = b"post body";
        let client_pubkey = [0x55u8; 32];

        let type_byte = (TXT_TYPE_SIGNED_PLAIN << 2) | attempt;
        let mut reply_data = [0u8; 4 + 1 + 4 + 9];
        reply_data[0..4].copy_from_slice(&ts.to_le_bytes());
        reply_data[4] = type_byte;
        reply_data[5..9].copy_from_slice(&author_prefix);
        reply_data[9..].copy_from_slice(text);

        let expected = {
            let full = sha256_2(&reply_data, &client_pubkey);
            [full[0], full[1], full[2], full[3]]
        };

        let push_body = &reply_data[5..];
        assert_eq!(
            room_push_ack_hash(ts, attempt, push_body, &client_pubkey),
            expected
        );
    }

    // ── Room post golden vector (MyMesh.cpp:427-470) ──────────────────────────

    #[test]
    fn encode_room_post_golden_vector_layout() {
        let (client, server) = make_pair();
        let shared = client.ecdh_shared_secret(&server.pubkey);

        let mut out = [0u8; 256];
        let n = encode_room_post(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            99,
            3,
            b"first post",
            &mut out,
        );

        let mut pt = [0u8; 256];
        let (dest, src, pt_len) = decode_dm_payload(&shared, &out[..n], &mut pt).unwrap();
        assert_eq!(dest, server.pub_hash());
        assert_eq!(src, client.pub_hash());
        assert_eq!(&pt[0..4], &99u32.to_le_bytes());
        assert_eq!(
            pt[4],
            (TXT_TYPE_PLAIN << 2) | 3,
            "type byte: (TXT_TYPE_PLAIN<<2)|attempt"
        );
        assert_eq!(&pt[5..15], b"first post");
        assert_eq!(pt[15], 0, "post text must be NUL-terminated");
        assert!(pt_len >= 16);
    }

    #[test]
    fn room_post_ack_hash_known_answer() {
        // MyMesh.cpp:445-448 — sha256(data[0 .. 5+strlen(text)] || client_pubkey)[0..4]
        let ts: u32 = 55;
        let attempt: u8 = 0;
        let text = b"ack me";
        let client_pubkey = [0x77u8; 32];

        let type_byte = (TXT_TYPE_PLAIN << 2) | attempt;
        let mut data = [0u8; 5 + 6];
        data[0..4].copy_from_slice(&ts.to_le_bytes());
        data[4] = type_byte;
        data[5..].copy_from_slice(text);

        let expected = {
            let full = sha256_2(&data, &client_pubkey);
            [full[0], full[1], full[2], full[3]]
        };

        assert_eq!(
            room_post_ack_hash(ts, attempt, text, &client_pubkey),
            expected
        );
    }

    // ── Keep-alive golden vector (MyMesh.cpp:524-560) ─────────────────────────

    #[test]
    fn encode_keep_alive_golden_vector_layout() {
        let (client, server) = make_pair();
        let shared = client.ecdh_shared_secret(&server.pubkey);

        let mut out = [0u8; 128];
        let n = encode_keep_alive(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            123,
            456,
            &mut out,
        );

        let mut pt = [0u8; 64];
        let (_dest, _src, pt_len) = decode_dm_payload(&shared, &out[..n], &mut pt).unwrap();
        assert!(
            pt_len >= 9,
            "len >= 9 per MyMesh.cpp:526 (REQ handler minimum)"
        );
        assert_eq!(&pt[0..4], &123u32.to_le_bytes());
        assert_eq!(pt[4], REQ_TYPE_KEEP_ALIVE);
        assert_eq!(&pt[5..9], &456u32.to_le_bytes());
    }

    #[test]
    fn keep_alive_ack_hash_known_answer() {
        // MyMesh.cpp:552-558 — sha256(data[0..9] || client.id.pub_key)[0..4]
        let ts: u32 = 10;
        let force_since: u32 = 20;
        let client_pubkey = [0x33u8; 32];

        let mut data = [0u8; 9];
        data[0..4].copy_from_slice(&ts.to_le_bytes());
        data[4] = REQ_TYPE_KEEP_ALIVE;
        data[5..9].copy_from_slice(&force_since.to_le_bytes());

        let expected = {
            let full = sha256_2(&data, &client_pubkey);
            [full[0], full[1], full[2], full[3]]
        };

        assert_eq!(
            keep_alive_ack_hash(ts, force_since, &client_pubkey),
            expected
        );
    }

    #[test]
    fn decode_keep_alive_ack_layout() {
        let mut payload = [0u8; 5];
        payload[0..4].copy_from_slice(&[1, 2, 3, 4]);
        payload[4] = 7; // unsynced count

        let ack = decode_keep_alive_ack(&payload).unwrap();
        assert_eq!(ack.ack_hash, [1, 2, 3, 4]);
        assert_eq!(ack.unsynced_count, 7);
    }

    #[test]
    fn decode_keep_alive_ack_rejects_short_payload() {
        let payload = [1, 2, 3, 4]; // missing the unsynced-count byte
        assert_eq!(
            decode_keep_alive_ack(&payload),
            Err(RoomCodecError::TruncatedPayload)
        );
    }

    // ── Room-server test double: full protocol round trip ────────────────────

    fn login_as(
        double: &mut RoomServerDouble,
        client: &Identity,
        server: &Identity,
        password: &[u8],
        ts: u32,
    ) -> LoginResponse {
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

        let mut pt = [0u8; 32];
        let (_dest, _src, pt_len) =
            decode_dm_payload(&shared, &reply_wire[..reply_len], &mut pt).unwrap();
        decode_login_response(&pt[..pt_len]).unwrap()
    }

    #[test]
    fn double_full_login_push_ack_post_keepalive_round_trip() {
        let client = Identity::from_seed([0x30u8; 32]);
        let server = Identity::from_seed([0x31u8; 32]);
        let other_author = Identity::from_seed([0x32u8; 32]);

        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);

        let login = login_as(&mut double, &client, &server, b"guest-pw", 1000);
        assert_eq!(login.permissions, RoomPermission::ReadWrite);
        assert_eq!(
            double.client_permissions(&client.pubkey),
            Some(RoomPermission::ReadWrite)
        );

        // Seed 3 posts, authored by someone else (server never pushes a post
        // back to its own author).
        double.seed_post(&other_author.pubkey, 1001, b"post one");
        double.seed_post(&other_author.pubkey, 1002, b"post two");
        double.seed_post(&other_author.pubkey, 1003, b"post three");

        let shared = client.ecdh_shared_secret(&server.pubkey);
        let expected_texts: [&[u8]; 3] = [b"post one", b"post two", b"post three"];
        for expected_text in expected_texts {
            let mut wire = [0u8; 256];
            let n = double
                .push_next(&client.pubkey, &mut wire)
                .expect("an eligible post must be pushed");

            let mut pt = [0u8; 256];
            let (_dest, _src, push) = decode_room_push(&shared, &wire[..n], &mut pt).unwrap();
            assert_eq!(
                &pt[push.text_offset..push.text_offset + push.text_len],
                expected_text
            );

            let ack = room_push_ack_hash(
                push.post_ts,
                push.attempt,
                push.push_body(&pt),
                &client.pubkey,
            );
            assert!(
                double.handle_ack(&client.pubkey, &ack),
                "ack must match the pending push"
            );
            assert_eq!(double.client_sync_since(&client.pubkey), Some(push.post_ts));
        }

        // No more eligible posts: push_next must now return None (not re-push).
        let mut wire = [0u8; 256];
        assert!(double.push_next(&client.pubkey, &mut wire).is_none());

        // Client posts.
        let mut post_wire = [0u8; 256];
        let post_n = encode_room_post(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            2000,
            0,
            b"client post",
            &mut post_wire,
        );
        let ack = double
            .handle_post(&client.pubkey, &post_wire[..post_n])
            .expect("ReadWrite client's post must be accepted");
        let expected_ack = room_post_ack_hash(2000, 0, b"client post", &client.pubkey);
        assert_eq!(ack, expected_ack);

        // Keep-alive.
        let mut ka_wire = [0u8; 128];
        let ka_n = encode_keep_alive(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            3000,
            0,
            &mut ka_wire,
        );
        let ka_ack = double
            .handle_keep_alive(&client.pubkey, &ka_wire[..ka_n])
            .expect("keep-alive must be accepted");
        let decoded_ka = decode_keep_alive_ack(&ka_ack).unwrap();
        assert_eq!(
            decoded_ka.ack_hash,
            keep_alive_ack_hash(3000, 0, &client.pubkey)
        );
        assert_eq!(decoded_ka.unsynced_count, 0, "all 3 posts already synced");
    }

    #[test]
    fn double_drip_stalls_without_ack() {
        // This test must fail if the double is ever changed to burst multiple
        // pushes without waiting for each ACK.
        let client = Identity::from_seed([0x40u8; 32]);
        let server = Identity::from_seed([0x41u8; 32]);
        let other_author = Identity::from_seed([0x42u8; 32]);

        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);
        login_as(&mut double, &client, &server, b"guest-pw", 1000);

        double.seed_post(&other_author.pubkey, 1001, b"first");
        double.seed_post(&other_author.pubkey, 1002, b"second");

        let mut wire1 = [0u8; 256];
        let n1 = double
            .push_next(&client.pubkey, &mut wire1)
            .expect("first push must succeed");
        assert!(n1 > 0);
        let sync_since_after_first_push = double.client_sync_since(&client.pubkey);

        // Withhold the ACK: a second push_next call must be a no-op.
        let mut wire2 = [0u8; 256];
        assert!(
            double.push_next(&client.pubkey, &mut wire2).is_none(),
            "must not push a second post while the first is unacked"
        );
        assert_eq!(
            double.client_sync_since(&client.pubkey),
            sync_since_after_first_push,
            "sync_since must not advance without an ACK"
        );

        // Now ACK the first push: the second becomes deliverable.
        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut pt = [0u8; 256];
        let (_dest, _src, push) = decode_room_push(&shared, &wire1[..n1], &mut pt).unwrap();
        let ack = room_push_ack_hash(
            push.post_ts,
            push.attempt,
            push.push_body(&pt),
            &client.pubkey,
        );
        assert!(double.handle_ack(&client.pubkey, &ack));

        let mut wire3 = [0u8; 256];
        assert!(
            double.push_next(&client.pubkey, &mut wire3).is_some(),
            "ACKed: next post must now push"
        );
    }

    #[test]
    fn double_guest_post_silently_dropped() {
        let client = Identity::from_seed([0x50u8; 32]);
        let server = Identity::from_seed([0x51u8; 32]);
        // allow_read_only=true, wrong password -> Guest fallback.
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", true);
        let login = login_as(&mut double, &client, &server, b"wrong-password", 1000);
        assert_eq!(login.permissions, RoomPermission::Guest);

        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut post_wire = [0u8; 256];
        let n = encode_room_post(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            2000,
            0,
            b"guest post",
            &mut post_wire,
        );

        assert_eq!(
            double.handle_post(&client.pubkey, &post_wire[..n]),
            None,
            "a Guest's post must be silently dropped: no ACK"
        );
    }

    #[test]
    fn double_read_only_post_silently_dropped() {
        let client = Identity::from_seed([0x60u8; 32]);
        let server = Identity::from_seed([0x61u8; 32]);
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);
        login_as(&mut double, &client, &server, b"guest-pw", 1000);
        double.set_permissions_for_test(&client.pubkey, RoomPermission::ReadOnly);

        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut post_wire = [0u8; 256];
        let n = encode_room_post(
            &shared,
            server.pub_hash(),
            client.pub_hash(),
            2000,
            0,
            b"read only post",
            &mut post_wire,
        );

        assert_eq!(
            double.handle_post(&client.pubkey, &post_wire[..n]),
            None,
            "a ReadOnly client's post must be silently dropped: no ACK"
        );
    }

    #[test]
    fn double_wrong_password_without_read_only_gets_no_response() {
        let client = Identity::from_seed([0x70u8; 32]);
        let server = Identity::from_seed([0x71u8; 32]);
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);

        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut anon_req = [0u8; 128];
        let n = encode_anon_req_login(
            &shared,
            server.pub_hash(),
            &client.pubkey,
            1000,
            0,
            b"wrong",
            &mut anon_req,
        );

        let mut out = [0u8; 64];
        assert!(
            double.handle_login(&anon_req[..n], &mut out).is_none(),
            "wrong password with allow_read_only unset: no response at all"
        );
    }

    #[test]
    fn double_replayed_login_timestamp_is_ignored() {
        let client = Identity::from_seed([0x80u8; 32]);
        let server = Identity::from_seed([0x81u8; 32]);
        let mut double = RoomServerDouble::new(server.clone(), b"admin-pw", b"guest-pw", false);

        login_as(&mut double, &client, &server, b"guest-pw", 1000);

        let shared = client.ecdh_shared_secret(&server.pubkey);
        let mut anon_req = [0u8; 128];
        // Same timestamp again -> replay, no response.
        let n = encode_anon_req_login(
            &shared,
            server.pub_hash(),
            &client.pubkey,
            1000,
            0,
            b"guest-pw",
            &mut anon_req,
        );
        let mut out = [0u8; 64];
        assert!(double.handle_login(&anon_req[..n], &mut out).is_none());
    }
}
