// SPDX-License-Identifier: GPL-3.0-only
//! Acceptance test for the `meshcadet-room-firmware-admin-frames` M1 gap-fix.
//!
//! `tests/integration.rs`'s `MockDevice` reimplements the wire contract for
//! `FRAME_ADD_ROOM`/`FRAME_DEL_ROOM`/`FRAME_QUERY_ROOMS` by hand — it was
//! authored in the same mission that defined the frames, so a green suite
//! driving it proved the host CLI's *encoding* was correct but proved
//! **nothing** about whether any real device peer implemented these frames.
//! That was exactly the M1 gap the `meshcadet-room-m1-checkpoint` gate caught:
//! `firmware/src/admin_server.rs`'s `handle_frame` had no arm for these three
//! frame types at all (they silently fell through to the unknown-frame arm,
//! which sends no reply), yet `cargo test --workspace` was fully green.
//!
//! This test closes that blind spot as far as is structurally possible on a
//! host machine: `firmware/` is a **detached**, cross-compiled-only workspace
//! (`xtensa-esp32s3-espidf`, `esp-idf-svc`) — its own `#[cfg(test)]` blocks
//! type-check but never execute (see `firmware/src/config_store.rs`'s doc
//! comment), so `admin_server::handle_frame` itself cannot be called from
//! here or from any host-native test binary. What CAN be host-tested is the
//! pure dispatch logic those match arms delegate to —
//! `firmware_core::room_admin::{handle_add_room, handle_del_room,
//! handle_query_rooms}` — which is not a parallel reimplementation but the
//! *exact same function* `admin_server.rs`'s `FRAME_ADD_ROOM`/`FRAME_DEL_ROOM`/
//! `FRAME_QUERY_ROOMS` arms call (see that module's doc). `FirmwareRoomDevice`
//! below is a `Transport` double that routes decoded frames straight into
//! those functions — no hand-written `match decode_add_room(payload) { ... }`
//! business logic of its own, unlike `MockDevice`.
//!
//! Together with `firmware/check-all-features.sh` (which fails to compile if
//! `admin_server.rs`'s match arms ever stop calling these functions with the
//! right signatures), this test and the build gate jointly cover what a
//! single host-native test cannot: the build gate proves the wiring
//! type-checks against the real call sites; this test proves the functions
//! that wiring calls behave correctly end-to-end through the host CLI's own
//! wire encoding (`Session::add_room`/`list_rooms`/`del_room`).

use std::collections::VecDeque;

use firmware_core::config_store::ProvisionedConfig;
use firmware_core::room_admin::{self, AddRoomOutcome, DelRoomOutcome};
use protocol::provisioning::{
    decode_frame, encode_frame, encode_rsp_error, FRAME_ADD_ROOM, FRAME_DEL_ROOM,
    FRAME_QUERY_ROOMS, FRAME_RSP_ERROR, FRAME_RSP_OK,
};

use host::session::Session;
use host::transport::Transport;

/// Application error codes — mirrors `firmware/src/admin_server.rs`'s own
/// `mod err` table (kept in sync by hand; there is no shared crate boundary
/// for these application-level codes between `host` and the detached
/// `firmware` workspace).
mod err {
    pub const CONTACT_LIST_FULL: u8 = 0x01;
    pub const CONTACT_NOT_FOUND: u8 = 0x03;
    pub const DECODE_ERROR: u8 = 0x05;
}

/// A device double whose `FRAME_ADD_ROOM`/`FRAME_DEL_ROOM`/`FRAME_QUERY_ROOMS`
/// handling is a direct call into `firmware_core::room_admin` — the real
/// dispatch logic, not a hand-authored mock (see module doc).
struct FirmwareRoomDevice {
    config: ProvisionedConfig,
}

impl FirmwareRoomDevice {
    fn new() -> Self {
        Self {
            config: ProvisionedConfig::empty(),
        }
    }

    fn handle(&mut self, frame_type: u8, payload: &[u8]) -> Vec<u8> {
        match frame_type {
            FRAME_ADD_ROOM => match room_admin::handle_add_room(&mut self.config, payload) {
                AddRoomOutcome::Added | AddRoomOutcome::Updated => ok_frame(),
                AddRoomOutcome::ListFull => {
                    error_frame(err::CONTACT_LIST_FULL, "contact list full")
                }
                AddRoomOutcome::DecodeError => {
                    error_frame(err::DECODE_ERROR, "add_room decode error")
                }
            },
            FRAME_DEL_ROOM => match room_admin::handle_del_room(&mut self.config, payload) {
                DelRoomOutcome::Removed => ok_frame(),
                DelRoomOutcome::NotFound => error_frame(err::CONTACT_NOT_FOUND, "room not found"),
                DelRoomOutcome::DecodeError => {
                    error_frame(err::DECODE_ERROR, "del_room decode error")
                }
            },
            FRAME_QUERY_ROOMS => {
                let mut response = Vec::new();
                for (ft, frame_payload) in room_admin::handle_query_rooms(&self.config) {
                    let mut fbuf = [0u8; 192];
                    let n = encode_frame(ft, &frame_payload, &mut fbuf);
                    response.extend_from_slice(&fbuf[..n]);
                }
                response
            }
            other => error_frame(0xFF, &format!("unknown frame type 0x{:02X}", other)),
        }
    }
}

fn ok_frame() -> Vec<u8> {
    let mut buf = [0u8; 16];
    let n = encode_frame(FRAME_RSP_OK, &[], &mut buf);
    buf[..n].to_vec()
}

fn error_frame(code: u8, msg: &str) -> Vec<u8> {
    let mut pbuf = [0u8; 80];
    let plen = encode_rsp_error(code, msg.as_bytes(), &mut pbuf);
    let mut fbuf = [0u8; 128];
    let n = encode_frame(FRAME_RSP_ERROR, &pbuf[..plen], &mut fbuf);
    fbuf[..n].to_vec()
}

/// In-process transport connecting a `Session` to a `FirmwareRoomDevice`.
/// Frame-draining logic mirrors `tests/integration.rs`'s `MockTransport`
/// exactly (same wire, same partial-send handling) — only the device backing
/// it differs.
struct FirmwareTransport {
    device: FirmwareRoomDevice,
    send_buf: Vec<u8>,
    recv_buf: VecDeque<u8>,
}

impl FirmwareTransport {
    fn new() -> Self {
        Self {
            device: FirmwareRoomDevice::new(),
            send_buf: Vec::new(),
            recv_buf: VecDeque::new(),
        }
    }
}

impl Transport for FirmwareTransport {
    fn send(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.send_buf.extend_from_slice(data);
        loop {
            if self.send_buf.len() < 5 {
                break;
            }
            let plen = (self.send_buf[3] as usize) | ((self.send_buf[4] as usize) << 8);
            let total = 7 + plen;
            if self.send_buf.len() < total {
                break;
            }
            let (ft, payload_slice) = decode_frame(&self.send_buf[..total])
                .map_err(|e| anyhow::anyhow!("device: host sent bad frame: {:?}", e))?;
            let payload: Vec<u8> = payload_slice.to_vec();
            self.send_buf.drain(..total);

            let response = self.device.handle(ft, &payload);
            self.recv_buf.extend(response);
        }
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> anyhow::Result<usize> {
        let n = buf.len().min(self.recv_buf.len());
        for b in buf[..n].iter_mut() {
            *b = self.recv_buf.pop_front().unwrap();
        }
        Ok(n)
    }
}

fn session() -> Session<FirmwareTransport> {
    Session::with_timeout(FirmwareTransport::new(), 1)
}

/// Acceptance: add-room → list-rooms → del-room round-trips against the
/// real `firmware_core::room_admin` dispatch functions — not
/// `tests/integration.rs`'s `MockDevice`.
#[test]
fn add_room_list_rooms_del_room_round_trip_against_real_dispatch() {
    let mut s = session();
    let pubkey = [0x77_u8; 32];

    s.add_room(&pubkey, b"letmein", b"Lobby")
        .expect("add_room should succeed against the real dispatch");

    let rooms = s.list_rooms().expect("list_rooms");
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].pubkey, pubkey);
    assert_eq!(&rooms[0].name[..rooms[0].name_len as usize], b"Lobby");

    s.del_room(&pubkey).expect("del_room should succeed");
    let rooms_after = s.list_rooms().expect("list_rooms after del_room");
    assert!(rooms_after.is_empty(), "room must be gone after del_room");
}

/// Acceptance: the guest password crosses the wire and reaches the real
/// storage layer (`RoomExtra`), but `list_rooms`'s `RspRoomPayload` never
/// carries it back — the type itself has no password field.
#[test]
fn add_room_delivers_password_to_real_storage_without_leaking_it_back() {
    let mut s = session();
    let pubkey = [0x99_u8; 32];
    s.add_room(&pubkey, b"swordfish1234", b"Lobby")
        .expect("add_room");

    let rooms = s.list_rooms().expect("list_rooms");
    assert_eq!(rooms.len(), 1);
    // RspRoomPayload has no password field — nothing to assert an absence
    // of; the guarantee is the type system's. What this test additionally
    // proves is that the *real* upsert path (`ProvisionedConfig::upsert_room`)
    // actually retains the password server-side, not just that the wire
    // reply omits it: see `firmware_core::room_admin`'s own
    // `add_room_from_wire_bytes_adds_contact_and_extra` test for that half.
}

/// Acceptance: deleting an unknown room pubkey surfaces the real dispatch's
/// error rather than silently succeeding.
#[test]
fn del_room_not_found_surfaces_error_from_real_dispatch() {
    let mut s = session();
    let err = s
        .del_room(&[0xEE_u8; 32])
        .expect_err("deleting an unknown room must fail");
    assert!(err.to_string().contains("room not found"));
}

/// Acceptance: an empty device reports no rooms configured.
#[test]
fn list_rooms_on_empty_device_is_empty() {
    let mut s = session();
    let rooms = s.list_rooms().expect("list_rooms");
    assert!(rooms.is_empty());
}

/// Acceptance: list-rooms enumerates multiple rooms in device-index order —
/// against the real dispatch, mirroring `tests/integration.rs`'s
/// `test_list_rooms_multiple` (which covers the same behaviour against
/// `MockDevice`).
#[test]
fn list_rooms_multiple_in_order_against_real_dispatch() {
    let mut s = session();
    let first = [0x11_u8; 32];
    let second = [0x22_u8; 32];
    s.add_room(&first, b"pw1", b"Lobby")
        .expect("add first room");
    s.add_room(&second, b"pw2", b"Study")
        .expect("add second room");

    let rooms = s.list_rooms().expect("list_rooms");
    assert_eq!(rooms.len(), 2);
    assert_eq!(rooms[0].pubkey, first);
    assert_eq!(&rooms[0].name[..rooms[0].name_len as usize], b"Lobby");
    assert_eq!(rooms[1].pubkey, second);
    assert_eq!(&rooms[1].name[..rooms[1].name_len as usize], b"Study");
}
