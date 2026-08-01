// SPDX-License-Identifier: GPL-3.0-only
//! Persisted per-contact inbound replay guard — the pure decode/decision
//! logic and its byte codec live in
//! [`firmware_core::inbound_replay`] (re-exported below, ADR-0005 shim
//! pattern) so their tests execute under `cargo test --workspace`. This file
//! keeps only the hardware-owning half: a small, dedicated NVS store, one
//! blob per contact, keyed by its pubkey hash byte — same "small dedicated
//! store, one key per item" shape as `advert_ts_store.rs`/`gps_baud_store.rs`.
//!
//! # NVS layout
//!
//! | Namespace | Key       | Type                                          | Contents |
//! |-----------|-----------|------------------------------------------------|----------|
//! | `mc_rpl`  | `p{:02x}` | blob (`INBOUND_REPLAY_STATE_LEN` bytes)        | one contact's [`InboundReplayState`] |
//!
//! Production builds only (`not(feature = "hil")`) — same gate as
//! `identity_store`/`config_store`/`history_store`: HIL is a bench test rig,
//! not exposed to the outsider threat model this store defends against, and
//! never touches NVS for anything else either.

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};

pub use firmware_core::inbound_replay::*;

const NVS_NAMESPACE: &str = "mc_rpl";

/// Format a contact's NVS key as `"p{:02x}"` into `buf` — mirrors
/// `room_session.rs::room_key`'s exact technique.
fn replay_key(hash: u8, buf: &mut [u8; 3]) -> &str {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    buf[0] = b'p';
    buf[1] = HEX[(hash >> 4) as usize];
    buf[2] = HEX[(hash & 0x0F) as usize];
    // SAFETY: buf contains only ASCII — valid UTF-8.
    core::str::from_utf8(buf).unwrap()
}

/// Load a contact's persisted inbound replay-guard state, keyed by its
/// pubkey hash byte. Returns [`InboundReplayState::EMPTY`] if nothing has
/// been persisted yet (this contact's first-ever inbound DM) or on any NVS
/// error (logged, non-fatal) — see that constant's doc for why this
/// fallback is always safe, never a security regression.
pub fn load_inbound_replay_state(
    nvs_partition: EspNvsPartition<NvsDefault>,
    hash: u8,
) -> InboundReplayState {
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!(
                "inbound_replay: failed to open NVS namespace ({:?}); starting from empty",
                e
            );
            return InboundReplayState::EMPTY;
        }
    };
    let mut blob = [0u8; INBOUND_REPLAY_STATE_LEN];
    let mut key_buf = [0u8; 3];
    let key = replay_key(hash, &mut key_buf);
    match nvs.get_blob(key, &mut blob) {
        Ok(Some(bytes)) => decode_inbound_replay_state(bytes).unwrap_or(InboundReplayState::EMPTY),
        Ok(None) => InboundReplayState::EMPTY,
        Err(e) => {
            log::warn!(
                "inbound_replay: NVS read failed for contact 0x{:02x} ({:?}); starting from empty",
                hash, e
            );
            InboundReplayState::EMPTY
        }
    }
}

/// Persist a contact's inbound replay-guard state, overwriting any previous
/// value. A failed write is logged and non-fatal — worst case, the next
/// acceptance from this contact re-persists a slightly stale ring (or, on an
/// unlucky crash between accept and persist, a reboot could let a genuine
/// replay through once more before this contact's traffic re-establishes the
/// mark) rather than bricking the device or corrupting the store.
pub fn save_inbound_replay_state(
    nvs_partition: EspNvsPartition<NvsDefault>,
    hash: u8,
    state: &InboundReplayState,
) {
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!(
                "inbound_replay: failed to open NVS namespace for write ({:?})",
                e
            );
            return;
        }
    };
    let mut blob = [0u8; INBOUND_REPLAY_STATE_LEN];
    let n = encode_inbound_replay_state(state, &mut blob);
    let mut key_buf = [0u8; 3];
    let key = replay_key(hash, &mut key_buf);
    if let Err(e) = nvs.set_blob(key, &blob[..n]) {
        log::warn!(
            "inbound_replay: NVS write failed for contact 0x{:02x} ({:?})",
            hash, e
        );
    }
}

/// Erase a contact's persisted replay-guard state, keyed by its pubkey hash
/// byte. Call site: `admin_server`'s `FRAME_DEL_CONTACT` handler — leaving
/// this blob behind would let a DIFFERENT future contact that happens to
/// collide on the same hash byte (1/256 odds) silently inherit a stale
/// high-water mark/ring, which could reject that new contact's genuinely
/// fresh, low-timestamp first messages until its own traffic climbs back
/// above whatever the deleted contact left behind. Same defensive posture as
/// `room_session::delete_room_session`'s `DEL_ROOM` cleanup, minus the
/// erase-epoch machinery — there is no live background thread re-persisting
/// a stale in-memory copy of this state the way a `RoomRuntime` does for
/// `PersistedRoomSession`, so no epoch is needed here.
///
/// A missing key is not an error (`EspNvs::remove` reports `Ok(false)`, not
/// `Err`) — nothing to do. An actual NVS error is logged and non-fatal.
pub fn delete_inbound_replay_state(nvs_partition: EspNvsPartition<NvsDefault>, hash: u8) {
    let nvs = match EspNvs::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!(
                "inbound_replay: failed to open NVS namespace for erase ({:?})",
                e
            );
            return;
        }
    };
    let mut key_buf = [0u8; 3];
    let key = replay_key(hash, &mut key_buf);
    if let Err(e) = nvs.remove(key) {
        log::warn!(
            "inbound_replay: NVS erase failed for contact 0x{:02x} ({:?})",
            hash, e
        );
    }
}
