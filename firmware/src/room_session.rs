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
//! One namespace, two keys per room, both keyed by its pubkey hash byte —
//! same "small dedicated store, one key per item" shape as
//! `advert_ts_store.rs`/`gps_baud_store.rs`.
//!
//! | Namespace | Key       | Type                                    | Contents |
//! |-----------|-----------|------------------------------------------|----------|
//! | `mc_room` | `r{:02x}` | blob (`PERSISTED_ROOM_SESSION_LEN` bytes) | one room's learned `permissions`/`sync_since`/`out_path` |
//! | `mc_room` | `x{:02x}` | `u8`                                      | that room's erase epoch (FINDING G — see below) |
//!
//! # FINDING G: erase durability across a live, un-rebooted runtime
//!
//! `admin_server`'s `ADD_ROOM`/`DEL_ROOM` arms call [`delete_room_session`]
//! on their own thread, but `main.rs`'s dispatcher loop built its
//! `RoomRuntime` for this room once at boot and keeps calling
//! [`save_room_session`] for it afterward (login replies, inbound pushes,
//! stall invalidation) — with no cross-thread channel telling that loop an
//! erase just happened. Left alone, the next one of those saves resurrects
//! the very blob the erase just removed. The `x{:02x}` epoch closes that:
//! [`delete_room_session`] bumps it every time it runs; `RoomRuntime`
//! remembers the epoch it saw at boot ([`load_room_epoch`]); every
//! [`save_room_session`] call re-reads the CURRENT epoch immediately before
//! writing and silently skips the write if it no longer matches — see
//! `firmware_core::room_session`'s "Session-store erase durability" module
//! section for the full mechanism and its host-run proof.

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

/// Format a room's erase-epoch NVS key as `"x{:02x}"` into `buf` — same
/// technique as [`room_key`], different leading byte so the epoch lives at
/// its own key, independent of whatever the session-blob key currently
/// holds (or doesn't).
#[cfg_attr(feature = "hil", allow(dead_code))]
fn room_epoch_key(hash: u8, buf: &mut [u8; 3]) -> &str {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    buf[0] = b'x';
    buf[1] = HEX[(hash >> 4) as usize];
    buf[2] = HEX[(hash & 0x0F) as usize];
    // SAFETY: buf contains only ASCII — valid UTF-8.
    core::str::from_utf8(buf).unwrap()
}

/// Load a room's current erase epoch (`0` if never bumped — the common case
/// for a room that has never been deleted/re-added this device's uptime, and
/// also the fallback on any NVS error). Callers: `RoomRuntime`'s
/// construction (captures this as `session_epoch`, the value
/// [`save_room_session`] later compares every write attempt against) and
/// [`delete_room_session`] itself (reads the current value before bumping
/// it).
#[cfg_attr(feature = "hil", allow(dead_code))]
pub fn load_room_epoch(nvs_partition: EspNvsPartition<NvsDefault>, hash: u8) -> u8 {
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!(
                "room_session: failed to open NVS namespace for epoch read ({:?})",
                e
            );
            return 0;
        }
    };
    let mut key_buf = [0u8; 3];
    let key = room_epoch_key(hash, &mut key_buf);
    match nvs.get_u8(key) {
        Ok(v) => v.unwrap_or(0),
        Err(e) => {
            log::warn!(
                "room_session: NVS epoch read failed for room 0x{:02x} ({:?})",
                hash,
                e
            );
            0
        }
    }
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

/// Persist a room's session-learned state, overwriting any previous value —
/// UNLESS `remembered_epoch` (what the caller's `RoomRuntime` captured at
/// boot, or at its last successful persist) no longer matches the store's
/// CURRENT erase epoch (re-read here, immediately before the write). A
/// mismatch means `delete_room_session` erased this room's store since —
/// the caller's in-memory session is stale relative to that erase, and
/// writing it would resurrect the exact blob the erase removed (FINDING G).
/// The write is silently skipped in that case (logged, non-fatal, same
/// posture as an NVS error below): the caller has no way to un-learn its own
/// stale state without a reboot, but it can at least stop re-persisting it.
///
/// A failed write is logged and non-fatal — worst case, the next boot
/// resumes from a stale (or absent) watermark and re-syncs a bounded backlog
/// rather than losing data or corrupting the store.
#[cfg_attr(feature = "hil", allow(dead_code))]
pub fn save_room_session(
    nvs_partition: EspNvsPartition<NvsDefault>,
    hash: u8,
    remembered_epoch: u8,
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
    let mut epoch_key_buf = [0u8; 3];
    let epoch_key = room_epoch_key(hash, &mut epoch_key_buf);
    let current_epoch = match nvs.get_u8(epoch_key) {
        Ok(v) => v.unwrap_or(0),
        Err(e) => {
            log::warn!(
                "room_session: NVS epoch read failed for room 0x{:02x} ({:?}) — \
                 skipping persist rather than risk resurrecting an erased blob",
                hash,
                e
            );
            return;
        }
    };
    if !room_session_persist_is_current(remembered_epoch, current_epoch) {
        log::info!(
            "room_session: skipping stale persist for room 0x{:02x} — erased since boot \
             (epoch {} != {})",
            hash,
            remembered_epoch,
            current_epoch
        );
        return;
    }
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
///
/// FINDING G: also bumps this room's erase epoch (see this module's doc),
/// unconditionally — regardless of whether the blob itself was actually
/// present to remove. A live `RoomRuntime` for this room (if any) keeps
/// running until reboot with no idea this happened; the bump is what makes
/// its NEXT [`save_room_session`] call refuse to resurrect the blob just
/// erased above rather than silently undoing this function's whole effect.
/// A failed bump is logged and non-fatal, same posture as every other NVS
/// error in this file — worst case a stale runtime's next persist slips
/// through, exactly the defect this bump exists to close, surfaced in the
/// log for a human to notice.
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

    let mut epoch_key_buf = [0u8; 3];
    let epoch_key = room_epoch_key(hash, &mut epoch_key_buf);
    let current_epoch = match nvs.get_u8(epoch_key) {
        Ok(v) => v.unwrap_or(0),
        Err(e) => {
            log::warn!(
                "room_session: NVS epoch read failed for room 0x{:02x} ({:?}) — \
                 bumping from 0",
                hash,
                e
            );
            0
        }
    };
    if let Err(e) = nvs.set_u8(epoch_key, next_room_session_epoch(current_epoch)) {
        log::warn!(
            "room_session: NVS epoch bump failed for room 0x{:02x} ({:?})",
            hash,
            e
        );
    }
}
