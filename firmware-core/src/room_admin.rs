// SPDX-License-Identifier: GPL-3.0-only
//! Pure logic backing `FRAME_ADD_ROOM` / `FRAME_DEL_ROOM` / `FRAME_QUERY_ROOMS`
//! (`firmware/src/admin_server.rs` and `firmware/src/provisioning_server.rs`'s
//! room-provisioning handlers). No NVS, no USB-serial I/O — the
//! hardware-owning half (persisting the mutated `ProvisionedConfig` to NVS,
//! writing the reply frames) stays in those two files, mirroring every other
//! `firmware-core` module (see this crate's top-level doc) and specifically
//! [`crate::advert::handle_query_advert`]'s "decode → mutate/compute → encode"
//! shape for `FRAME_QUERY_ADVERT`.
//!
//! # Why this module exists (the M1 gap)
//!
//! The frames were byte-pinned, the codec (`encode_add_room`/`decode_add_room`/
//! `decode_del_room`/`encode_rsp_room`) was written and round-trip tested, and
//! the storage primitives ([`ProvisionedConfig::upsert_room`] /
//! [`ProvisionedConfig::remove_room`]) were written and tested — but nothing
//! on the device ever *called* them: `admin_server.rs`'s `handle_frame` match
//! had no `FRAME_ADD_ROOM`/`FRAME_DEL_ROOM`/`FRAME_QUERY_ROOMS` arm at all, so
//! those frames fell through to the unknown-frame arm (logs at debug, sends
//! **no reply**), and the equivalent `provisioning_server.rs` staging path had
//! no way to fill `room_extras` either.
//!
//! Because `firmware/`'s own `#[cfg(test)]` blocks type-check but never
//! execute (a detached, cross-compiled-only workspace — see
//! `firmware/src/config_store.rs`'s doc comment), that wiring gap was
//! invisible to `cargo test --workspace`: the only "device" exercising these
//! frames was `host/tests/integration.rs`'s `MockDevice`, a mock authored in
//! the same mission that added the frames, which of course implemented them
//! correctly. A green host-side suite proved nothing about the real
//! firmware peer.
//!
//! [`handle_add_room`], [`handle_del_room`], and [`handle_query_rooms`] are
//! the fix for *that* blind spot: they are the exact functions
//! `admin_server::handle_frame`'s and `provisioning_server::process_frame`'s
//! room-frame arms call at runtime (not a parallel reimplementation), and
//! because they carry no NVS/serial dependency they live in this
//! host-testable crate — so a test driving them with the identical encoded
//! bytes the host CLI sends now actually proves the device peer, not a mock,
//! handles these frames.

use crate::config_store::{
    Contact, ContactListFull, ContactUpsert, ProvisionedConfig, RoomExtra, ROLE_CHAT,
};
use protocol::constants::MAX_PATH_SIZE;
use protocol::provisioning::{
    decode_add_room, decode_del_room, encode_rsp_room, FRAME_RSP_ROOM, FRAME_RSP_ROOMS_DONE,
    MAX_ROOM_PATH_LEN,
};

/// Outcome of dispatching a `FRAME_ADD_ROOM` payload — mirrors
/// [`ContactUpsert`]/[`ContactListFull`] (the result [`ProvisionedConfig::upsert_room`]
/// already returns) plus a dedicated decode-failure variant, so the caller
/// can map every branch straight onto its existing `RSP_OK`/`RSP_ERROR`
/// error-code table with no branching logic of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddRoomOutcome {
    /// A new room-server contact was appended.
    Added,
    /// An existing room (same pubkey) was updated in place.
    Updated,
    /// A genuinely new room could not be appended: the contact list is full.
    ListFull,
    /// The payload failed to decode (truncated, or a length field over its cap).
    DecodeError,
}

/// Handle one `FRAME_ADD_ROOM` request end to end (pure): decode the wire
/// payload, then upsert the room's `Contact` (forced to `ROLE_ROOM` by
/// [`ProvisionedConfig::upsert_room`]) and its [`RoomExtra`], keyed on the
/// room's pubkey.
///
/// The `AddRoomPayload` wire format deliberately carries no `sync_since` /
/// `permissions` / `out_path` (those are runtime-learned, not host-supplied —
/// see ADR-0002 §7), so every call seeds a **fresh** `RoomExtra` (zeroed
/// session state) for that pubkey. A re-add of an already-known room
/// therefore resets its learned session state exactly as it resets its
/// guest password and display name — the same "last write wins" full-replace
/// semantics `upsert_contact`/`upsert_channel` already have for every other
/// field, applied consistently here rather than carved out as a special
/// case. A device that re-logs-in after a credential rotation re-learns its
/// route and re-syncs from scratch, which is the conservative, always-safe
/// behaviour to default to when a room's password (its credential) is
/// changed.
pub fn handle_add_room(config: &mut ProvisionedConfig, payload: &[u8]) -> AddRoomOutcome {
    let Ok(add) = decode_add_room(payload) else {
        return AddRoomOutcome::DecodeError;
    };
    let contact = Contact {
        pubkey: add.pubkey,
        telemetry_enable: false,
        role: ROLE_CHAT, // forced to ROLE_ROOM by upsert_room below
        display_name: add.name,
        display_name_len: add.name_len,
    };
    let extra = RoomExtra {
        pubkey: add.pubkey,
        guest_password: add.guest_password,
        guest_password_len: add.guest_password_len,
        sync_since: 0,
        permissions: 0,
        out_path: [0u8; MAX_PATH_SIZE],
        out_path_len: 0,
    };
    match config.upsert_room(contact, extra) {
        Ok(ContactUpsert::Added) => AddRoomOutcome::Added,
        Ok(ContactUpsert::Updated) => AddRoomOutcome::Updated,
        Err(ContactListFull) => AddRoomOutcome::ListFull,
    }
}

/// Outcome of dispatching a `FRAME_DEL_ROOM` payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelRoomOutcome {
    /// The room (its `Contact` entry AND its `RoomExtra`) was removed.
    Removed,
    /// No room with that pubkey was configured.
    NotFound,
    /// The payload failed to decode.
    DecodeError,
}

/// Handle one `FRAME_DEL_ROOM` request end to end (pure): decode the wire
/// payload, then remove the room via [`ProvisionedConfig::remove_room`]
/// (which deletes both the `Contact` entry and its paired `RoomExtra`,
/// leaving every neighbouring contact/channel untouched).
pub fn handle_del_room(config: &mut ProvisionedConfig, payload: &[u8]) -> DelRoomOutcome {
    let Ok(del) = decode_del_room(payload) else {
        return DelRoomOutcome::DecodeError;
    };
    if config.remove_room(&del.pubkey) {
        DelRoomOutcome::Removed
    } else {
        DelRoomOutcome::NotFound
    }
}

/// Build the full `FRAME_QUERY_ROOMS` reply: one `(FRAME_RSP_ROOM, payload)`
/// per configured room (in `room_extras` index order) followed by a terminal
/// `(FRAME_RSP_ROOMS_DONE, [])` — ready for the caller to hand each pair
/// straight to its `send_frame` primitive. Mirrors the
/// `QUERY_CONTACTS`/`QUERY_CHANNELS` streamed-enumeration shape.
///
/// A room's display name lives on its `Contact` entry, not `RoomExtra` (see
/// `RoomExtra`'s doc comment on why the two lists are only linked by
/// pubkey), so each entry is built by looking up the matching contact; an
/// orphaned `RoomExtra` with no matching contact (should not happen —
/// [`ProvisionedConfig::upsert_room`]/[`ProvisionedConfig::remove_room`] keep
/// the two in lockstep — but NVS can always be corrupted) reports an empty
/// name rather than panicking.
pub fn handle_query_rooms(config: &ProvisionedConfig) -> Vec<(u8, Vec<u8>)> {
    let rcnt = config.room_count as usize;
    let ccnt = config.contact_count as usize;
    let mut frames = Vec::with_capacity(rcnt + 1);
    for (i, r) in config.room_extras[..rcnt].iter().enumerate() {
        let name: &[u8] = match config.contacts[..ccnt]
            .iter()
            .find(|c| c.pubkey == r.pubkey)
        {
            Some(c) => {
                let name_len = (c.display_name_len as usize).min(c.display_name.len());
                &c.display_name[..name_len]
            }
            None => &[],
        };
        let out_path_len = (r.out_path_len as usize)
            .min(r.out_path.len())
            .min(MAX_ROOM_PATH_LEN);
        // Buffer sized per encode_rsp_room's own doc: 40 + MAX_ROOM_PATH_LEN + MAX_NAME_LEN (136 B).
        let mut pbuf = [0u8; 40 + MAX_ROOM_PATH_LEN + 32];
        let plen = encode_rsp_room(
            i as u8,
            &r.pubkey,
            r.sync_since,
            r.permissions,
            &r.out_path[..out_path_len],
            name,
            &mut pbuf,
        );
        frames.push((FRAME_RSP_ROOM, pbuf[..plen].to_vec()));
    }
    frames.push((FRAME_RSP_ROOMS_DONE, Vec::new()));
    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::provisioning::{decode_rsp_room, encode_add_room, encode_del_room};

    fn contact(byte: u8, role: u8, name: &[u8]) -> Contact {
        let mut display_name = [0u8; 32];
        display_name[..name.len()].copy_from_slice(name);
        Contact {
            pubkey: [byte; 32],
            telemetry_enable: false,
            role,
            display_name,
            display_name_len: name.len() as u8,
        }
    }

    // ── handle_add_room ──────────────────────────────────────────────────

    #[test]
    fn add_room_from_wire_bytes_adds_contact_and_extra() {
        // Exactly what admin_server::handle_frame's FRAME_ADD_ROOM arm
        // receives: the raw payload slice decode_frame() sliced out.
        let pubkey = [0x77u8; 32];
        let mut buf = [0u8; 96];
        let plen = encode_add_room(&pubkey, b"letmein", b"Lobby", &mut buf);

        let mut config = ProvisionedConfig::empty();
        let outcome = handle_add_room(&mut config, &buf[..plen]);

        assert_eq!(outcome, AddRoomOutcome::Added);
        assert_eq!(config.contact_count, 1);
        assert_eq!(config.room_count, 1);
        let room_contact = &config.contacts[0];
        assert_eq!(room_contact.pubkey, pubkey);
        assert_eq!(room_contact.role, crate::config_store::ROLE_ROOM);
        let extra = config.room_extra(&pubkey).expect("room extra must exist");
        assert_eq!(extra.guest_password_len, 7);
        assert_eq!(&extra.guest_password[..7], b"letmein");
    }

    #[test]
    fn add_room_decode_error_on_truncated_payload() {
        let mut config = ProvisionedConfig::empty();
        let outcome = handle_add_room(&mut config, &[0u8; 4]);
        assert_eq!(outcome, AddRoomOutcome::DecodeError);
        assert_eq!(
            config.contact_count, 0,
            "a decode failure must not mutate config"
        );
    }

    #[test]
    fn add_room_re_add_updates_in_place_and_resets_learned_state() {
        let pubkey = [0x88u8; 32];
        let mut config = ProvisionedConfig::empty();

        // First add, then simulate a session having learned state.
        let mut buf = [0u8; 96];
        let plen = encode_add_room(&pubkey, b"first", b"Lobby", &mut buf);
        assert_eq!(
            handle_add_room(&mut config, &buf[..plen]),
            AddRoomOutcome::Added
        );
        let extra = config.room_extra_mut(&pubkey).unwrap();
        extra.sync_since = 42;
        extra.permissions = 2;
        extra.out_path_len = 2;
        extra.out_path[..2].copy_from_slice(&[0xAA, 0xBB]);

        // Re-add with a new password: must update in place (no duplicate),
        // and per this module's documented semantics, reset session state.
        let plen2 = encode_add_room(&pubkey, b"rotated", b"Lobby", &mut buf);
        let outcome = handle_add_room(&mut config, &buf[..plen2]);

        assert_eq!(outcome, AddRoomOutcome::Updated);
        assert_eq!(config.contact_count, 1, "re-add must not stack a duplicate");
        assert_eq!(config.room_count, 1);
        let extra = config.room_extra(&pubkey).unwrap();
        assert_eq!(&extra.guest_password[..7], b"rotated");
        assert_eq!(extra.sync_since, 0);
        assert_eq!(extra.permissions, 0);
        assert_eq!(extra.out_path_len, 0);
    }

    #[test]
    fn add_room_list_full_when_contact_capacity_exhausted() {
        let mut config = ProvisionedConfig::empty();
        for i in 0..crate::config_store::MAX_CONTACTS {
            config
                .upsert_contact(contact(i as u8, ROLE_CHAT, b"filler"))
                .unwrap();
        }
        let mut buf = [0u8; 96];
        let plen = encode_add_room(&[0xEEu8; 32], b"pw", b"Overflow", &mut buf);
        assert_eq!(
            handle_add_room(&mut config, &buf[..plen]),
            AddRoomOutcome::ListFull
        );
    }

    // ── handle_del_room ──────────────────────────────────────────────────

    #[test]
    fn del_room_from_wire_bytes_removes_contact_and_extra() {
        let pubkey = [0x99u8; 32];
        let mut config = ProvisionedConfig::empty();
        let mut buf = [0u8; 96];
        let plen = encode_add_room(&pubkey, b"pw", b"Study", &mut buf);
        handle_add_room(&mut config, &buf[..plen]);

        let mut del_buf = [0u8; 32];
        let del_plen = encode_del_room(&pubkey, &mut del_buf);
        let outcome = handle_del_room(&mut config, &del_buf[..del_plen]);

        assert_eq!(outcome, DelRoomOutcome::Removed);
        assert_eq!(config.contact_count, 0);
        assert_eq!(config.room_count, 0);
        assert!(config.room_extra(&pubkey).is_none());
    }

    #[test]
    fn del_room_not_found_leaves_config_untouched() {
        let mut config = ProvisionedConfig::empty();
        config
            .upsert_contact(contact(0x11, ROLE_CHAT, b"Alice"))
            .unwrap();
        let mut del_buf = [0u8; 32];
        let del_plen = encode_del_room(&[0xFFu8; 32], &mut del_buf);
        let outcome = handle_del_room(&mut config, &del_buf[..del_plen]);
        assert_eq!(outcome, DelRoomOutcome::NotFound);
        assert_eq!(
            config.contact_count, 1,
            "unrelated contact must be untouched"
        );
    }

    #[test]
    fn del_room_decode_error_on_truncated_payload() {
        let mut config = ProvisionedConfig::empty();
        assert_eq!(
            handle_del_room(&mut config, &[0u8; 4]),
            DelRoomOutcome::DecodeError
        );
    }

    #[test]
    fn del_room_does_not_disturb_neighbouring_contacts_or_channels() {
        use crate::config_store::Channel;
        let mut config = ProvisionedConfig::empty();
        config
            .upsert_contact(contact(0x11, ROLE_CHAT, b"Alice"))
            .unwrap();
        config
            .upsert_channel(Channel {
                secret: [0x22u8; 32],
                key_len: 32,
                primary: true,
                name: {
                    let mut n = [0u8; 32];
                    n[..7].copy_from_slice(b"General");
                    n
                },
                name_len: 7,
            })
            .unwrap();
        let room_pubkey = [0x33u8; 32];
        let mut buf = [0u8; 96];
        let plen = encode_add_room(&room_pubkey, b"pw", b"Lobby", &mut buf);
        handle_add_room(&mut config, &buf[..plen]);
        assert_eq!(config.contact_count, 2);

        let mut del_buf = [0u8; 32];
        let del_plen = encode_del_room(&room_pubkey, &mut del_buf);
        assert_eq!(
            handle_del_room(&mut config, &del_buf[..del_plen]),
            DelRoomOutcome::Removed
        );

        assert_eq!(config.contact_count, 1);
        assert_eq!(config.contacts[0].pubkey, [0x11u8; 32], "Alice must remain");
        assert_eq!(config.channel_count, 1, "the channel must be untouched");
        assert_eq!(config.channels[0].secret, [0x22u8; 32]);
    }

    // ── handle_query_rooms ───────────────────────────────────────────────

    #[test]
    fn query_rooms_on_empty_config_is_just_done() {
        let config = ProvisionedConfig::empty();
        let frames = handle_query_rooms(&config);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], (FRAME_RSP_ROOMS_DONE, Vec::new()));
    }

    #[test]
    fn query_rooms_streams_one_rsp_room_per_room_then_done() {
        let mut config = ProvisionedConfig::empty();
        let first = [0x01u8; 32];
        let second = [0x02u8; 32];
        let mut buf = [0u8; 96];
        let plen = encode_add_room(&first, b"pw1", b"Lobby", &mut buf);
        handle_add_room(&mut config, &buf[..plen]);
        let plen2 = encode_add_room(&second, b"pw2", b"Study", &mut buf);
        handle_add_room(&mut config, &buf[..plen2]);

        let frames = handle_query_rooms(&config);
        assert_eq!(frames.len(), 3, "2 rooms + DONE");

        assert_eq!(frames[0].0, FRAME_RSP_ROOM);
        let r0 = decode_rsp_room(&frames[0].1).unwrap();
        assert_eq!(r0.index, 0);
        assert_eq!(r0.pubkey, first);
        assert_eq!(&r0.name[..r0.name_len as usize], b"Lobby");

        assert_eq!(frames[1].0, FRAME_RSP_ROOM);
        let r1 = decode_rsp_room(&frames[1].1).unwrap();
        assert_eq!(r1.index, 1);
        assert_eq!(r1.pubkey, second);
        assert_eq!(&r1.name[..r1.name_len as usize], b"Study");

        assert_eq!(frames[2], (FRAME_RSP_ROOMS_DONE, Vec::new()));
    }

    #[test]
    fn query_rooms_reply_never_carries_the_guest_password() {
        // RspRoomPayload has no password field at all (see FRAME_RSP_ROOM's
        // doc) — the type system, not this test, is the real guarantee; this
        // asserts the encoded bytes agree by scanning for the password.
        let mut config = ProvisionedConfig::empty();
        let pubkey = [0x44u8; 32];
        let mut buf = [0u8; 96];
        let plen = encode_add_room(&pubkey, b"supersecretpw", b"Lobby", &mut buf);
        handle_add_room(&mut config, &buf[..plen]);

        let frames = handle_query_rooms(&config);
        let room_frame = &frames[0].1;
        assert!(
            !room_frame
                .windows(b"supersecretpw".len())
                .any(|w| w == b"supersecretpw"),
            "guest password must never appear in the RSP_ROOM bytes"
        );
    }

    #[test]
    fn query_rooms_index_matches_room_list_position_not_contact_position() {
        // A plain contact added before a room must not shift the room's
        // reported index — index is 0-based within the room list, not the
        // overall contact list.
        let mut config = ProvisionedConfig::empty();
        config
            .upsert_contact(contact(0x11, ROLE_CHAT, b"Alice"))
            .unwrap();
        let room_pubkey = [0x55u8; 32];
        let mut buf = [0u8; 96];
        let plen = encode_add_room(&room_pubkey, b"pw", b"Lobby", &mut buf);
        handle_add_room(&mut config, &buf[..plen]);

        let frames = handle_query_rooms(&config);
        assert_eq!(frames.len(), 2, "1 room + DONE");
        let r0 = decode_rsp_room(&frames[0].1).unwrap();
        assert_eq!(
            r0.index, 0,
            "the only configured room is index 0 in the room list"
        );
        assert_eq!(r0.pubkey, room_pubkey);
    }
}
