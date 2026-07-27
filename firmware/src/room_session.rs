// SPDX-License-Identifier: GPL-3.0-only
//! Room-client session logic — the pure decode/ACK/dedup state machine and
//! its persistence codec live in [`firmware_core::room_session`] (re-exported
//! below, ADR-0005 shim pattern) so their tests execute under `cargo test
//! --workspace`. This file keeps only the hardware-owning half: a small,
//! dedicated NVS store for a room's SESSION-learned state (permission /
//! `out_path` / `sync_since`).
//!
//! # Why a store separate from `config_store`'s `RoomExtra`
//!
//! `main.rs::run()` hands the whole loaded `ProvisionedConfig` (which owns
//! every `RoomExtra`) off to the `admin_server` thread before the dispatcher
//! loop that logs into rooms and receives their pushes ever starts running —
//! see that spawn site's own comment. That loop therefore has no safe
//! in-place handle to mutate `admin_server`'s copy of `RoomExtra` from the
//! main thread. This store is the dispatcher loop's OWN durable memory of
//! what a live session has learned instead: additive to (not a replacement
//! for) the provisioning-time `RoomExtra` seed, and invisible to
//! `admin_server`/any future `QUERY_CONTACTS`-style surface until a later
//! change unifies the two (accepted M1 gap; see PR discussion for the
//! rationale).
//!
//! # NVS layout
//!
//! One namespace, one blob key per room, keyed by its pubkey hash byte
//! (`"r{:02x}"`, e.g. `"r4a"`) — same "small dedicated store, one key per
//! item" shape as `advert_ts_store.rs`/`gps_baud_store.rs`.
//!
//! | Namespace | Key       | Type                                    | Contents |
//! |-----------|-----------|------------------------------------------|----------|
//! | `mc_room` | `r{:02x}` | blob (`PERSISTED_ROOM_SESSION_LEN` bytes) | one room's learned `permissions`/`sync_since`/`out_path` |

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};

pub use firmware_core::room_session::*;

const NVS_NAMESPACE: &str = "mc_room";

// Every call site of the functions below lives in `main.rs` or `admin_server.rs`
// behind `#[cfg(not(feature = "hil"))]` (there are no rooms under `hil`), so a
// `hil` build never calls into this file at all — `allow(dead_code)` there
// is genuinely dead in that profile, not a mistake.

/// Format a room's NVS key as `"r{:02x}"` into `buf` — mirrors
/// `history_store.rs::legacy_slot_key`'s exact technique.
#[cfg_attr(feature = "hil", allow(dead_code))]
fn room_key(hash: u8, buf: &mut [u8; 3]) -> &str {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    buf[0] = b'r';
    buf[1] = HEX[(hash >> 4) as usize];
    buf[2] = HEX[(hash & 0x0F) as usize];
    // SAFETY: buf contains only ASCII — valid UTF-8.
    core::str::from_utf8(buf).unwrap()
}

/// Load a room's session-learned state, keyed by its pubkey hash byte.
/// Returns `None` if nothing has been persisted yet (first login this boot,
/// or ever) or on any NVS error (logged, non-fatal) — callers fall back to
/// [`PersistedRoomSession::from_room_extra`]'s provisioning-time seed.
#[cfg_attr(feature = "hil", allow(dead_code))]
pub fn load_room_session(
    nvs_partition: EspNvsPartition<NvsDefault>,
    hash: u8,
) -> Option<PersistedRoomSession> {
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!("room_session: failed to open NVS namespace ({:?})", e);
            return None;
        }
    };
    let mut key_buf = [0u8; 3];
    let key = room_key(hash, &mut key_buf);
    let mut blob = [0u8; PERSISTED_ROOM_SESSION_LEN];
    let bytes = match nvs.get_blob(key, &mut blob) {
        Ok(opt) => opt,
        Err(e) => {
            log::warn!(
                "room_session: NVS read failed for room 0x{:02x} ({:?})",
                hash,
                e
            );
            return None;
        }
    }?;
    decode_persisted_room_session(bytes)
}

/// Persist a room's session-learned state, overwriting any previous value.
/// A failed write is logged and non-fatal — worst case, the next boot
/// resumes from a stale (or absent) watermark and re-syncs a bounded backlog
/// rather than losing data or corrupting the store.
#[cfg_attr(feature = "hil", allow(dead_code))]
pub fn save_room_session(
    nvs_partition: EspNvsPartition<NvsDefault>,
    hash: u8,
    state: &PersistedRoomSession,
) {
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!(
                "room_session: failed to open NVS namespace for write ({:?})",
                e
            );
            return;
        }
    };
    let mut blob = [0u8; PERSISTED_ROOM_SESSION_LEN];
    let n = encode_persisted_room_session(state, &mut blob);
    let mut key_buf = [0u8; 3];
    let key = room_key(hash, &mut key_buf);
    if let Err(e) = nvs.set_blob(key, &blob[..n]) {
        log::warn!(
            "room_session: NVS write failed for room 0x{:02x} ({:?})",
            hash,
            e
        );
    }
}

/// Erase a room's dedicated session-learned state, keyed by its pubkey hash
/// byte. Call sites: `admin_server`'s `DEL_ROOM` handler (the room is gone —
/// leaving its blob behind would let a *different* future room that happens
/// to collide on the same hash byte silently inherit a stale watermark/route/
/// permission) and its `ADD_ROOM` handler (a re-add is a documented
/// full-replace of `RoomExtra` — see `room_admin::handle_add_room`'s doc —
/// and this dedicated store must not be allowed to shadow that reset seed at
/// the next boot's `load_room_session(..).unwrap_or(seed)` resume, which is
/// exactly what happens if this store still holds the pre-reset blob).
///
/// A missing key is not an error (`EspNvs::remove` reports `Ok(false)`, not
/// `Err`) — nothing to do. An actual NVS error is logged and non-fatal, same
/// posture as [`save_room_session`]: worst case a re-add's dedicated-store
/// blob lingers stale, exactly the FINDING D defect this function exists to
/// close, surfaced in the log for a human to notice.
#[cfg_attr(feature = "hil", allow(dead_code))]
pub fn delete_room_session(nvs_partition: EspNvsPartition<NvsDefault>, hash: u8) {
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!(
                "room_session: failed to open NVS namespace for erase ({:?})",
                e
            );
            return;
        }
    };
    let mut key_buf = [0u8; 3];
    let key = room_key(hash, &mut key_buf);
    if let Err(e) = nvs.remove(key) {
        log::warn!(
            "room_session: NVS erase failed for room 0x{:02x} ({:?})",
            hash,
            e
        );
    }
}
