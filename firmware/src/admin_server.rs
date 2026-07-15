// SPDX-License-Identifier: GPL-3.0-only
//! Post-provisioning admin USB-serial server.
//!
//! Runs as a background thread after the device is provisioned.  Handles
//! admin requests that arrive over USB-JTAG serial at runtime — history
//! export, status/identity/contact/channel queries, and runtime edits to the
//! provisioned contact/channel set.  The PIN-gated on-device menu (`pin_menu`)
//! handles on-device toggles without a laptop; this server handles
//! laptop-originated requests.
//!
//! # Protocol
//!
//! Identical framing to the provisioning wire format (ADR-0002): MAGIC(2) +
//! type(1) + len_lo(1) + len_hi(1) + payload(N) + CRC16(2).
//!
//! # Frame types handled
//!
//! | Frame type              | Direction   | Action |
//! |-------------------------|-------------|--------|
//! | `FRAME_QUERY_STATUS`    | host→device | reply `RSP_STATUS` (provisioned) + `RSP_IDENTITY` |
//! | `FRAME_QUERY_CONTACTS`  | host→device | stream `RSP_CONTACT` per contact, then `RSP_CONTACTS_DONE` |
//! | `FRAME_QUERY_CHANNELS`  | host→device | stream `RSP_CHANNEL` per channel, then `RSP_CHANNELS_DONE` |
//! | `FRAME_ADD_CONTACT`     | host→device | append contact, persist to NVS, reply `RSP_OK` / `RSP_ERROR` |
//! | `FRAME_DEL_CONTACT`     | host→device | remove contact, persist to NVS, reply `RSP_OK` / `RSP_ERROR` |
//! | `FRAME_ADD_CHANNEL`     | host→device | upsert channel by secret (known key updates in place, new key appends), persist to NVS, reply `RSP_OK` / `RSP_ERROR` |
//! | `FRAME_DEL_CHANNEL`     | host→device | remove channel, persist to NVS, reply `RSP_OK` / `RSP_ERROR` |
//! | `FRAME_SET_NOTIF_DEFAULTS`| host→device | update notif defaults, persist to NVS, reply `RSP_OK` / `RSP_ERROR` |
//! | `FRAME_SET_PIN`         | host→device | set/reset PIN, persist to NVS, reply `RSP_OK` / `RSP_ERROR` |
//! | `FRAME_SET_DEVICE_NAME` | host→device | set device display name, persist to identity store, reply `RSP_OK` / `RSP_ERROR` |
//! | `FRAME_COMMIT_PROVISIONING`| host→device | re-persist live config, reply `RSP_OK` (no reboot) |
//! | `FRAME_EXPORT_HISTORY`  | host→device | stream all history entries, then send DONE |
//! | `FRAME_CLEAR_HISTORY`   | host→device | erase ALL persisted conversation history (every DM + channel, both directions), reply `RSP_OK` / `RSP_ERROR` — flash effect immediate, UI in-memory state needs a reboot to reflect it (see handler doc comment) |
//! | `FRAME_QUERY_ADVERT`    | host→device | build + sign this device's self-advert card, reply `RSP_ADVERT` with the raw card bytes — written straight to this serial reply, **never** enqueued onto `txq`/the radio dispatcher (see handler doc comment) |
//!
//! `QUERY_STATUS` is the host CLI's connect-and-verify command (`meshcadet
//! status` / `identity`).  The unprovisioned [`provisioning_server`] answers it
//! during first-boot provisioning; once the device is provisioned that server
//! is gone and `admin_server` is the only runtime serial handler, so it must
//! answer `QUERY_STATUS` too — otherwise `meshcadet status` against a
//! provisioned device hangs until the host's retry deadline expires.
//!
//! The same reasoning applies to every other host CLI command.  Earlier this
//! server held a *read-only boot snapshot* of the contact/channel set and
//! answered only the `QUERY_*` frames; the edit frames (`ADD_CHANNEL`,
//! `DEL_CHANNEL`, `ADD_CONTACT`, `DEL_CONTACT`) fell through to the
//! unknown-frame arm (logged at `debug`, no reply emitted), so e.g. `meshcadet
//! add-channel` against a provisioned device hung until the host's retry
//! deadline expired.  Because the snapshot was immutable, runtime edits were
//! also invisible — `list-channels` could not reflect anything the host added.
//!
//! This server now owns the loaded [`ProvisionedConfig`] as the single mutable
//! source of truth plus an NVS partition handle.  The `QUERY_*` replies read it
//! and the edit frames mutate it and persist the whole config blob back to NVS
//! (`config_store::save_provisioned_config`) — so `QUERY_STATUS` counts and the
//! `QUERY_CONTACTS` / `QUERY_CHANNELS` streams stay consistent by construction,
//! and an added channel appears in the very next `list-channels`.  The channel
//! `secret` is held in this thread's memory (as in the config) but never enters
//! a response: only the on-air `channel_hash` is encoded into `RSP_CHANNEL`.
//!
//! Runtime edits persist to NVS immediately (no separate commit/reboot, unlike
//! first-boot provisioning).  The live radio loop continues to use the
//! channel/contact set captured at boot; persisted edits take full effect on
//! the next boot — consistent with the existing provisioning model, in which
//! the provisioned channel set is not yet wired to the radio.

use std::io::Write;

use anyhow::anyhow;
use esp_idf_svc::nvs::{EspNvsPartition, NvsDefault};

use protocol::channel_hash_var;
use protocol::provisioning::{
    FRAME_ADD_CHANNEL, FRAME_ADD_CONTACT, FRAME_CLEAR_HISTORY, FRAME_COMMIT_PROVISIONING,
    FRAME_DEL_CHANNEL, FRAME_DEL_CONTACT, FRAME_EXPORT_HISTORY, FRAME_QUERY_ADVERT,
    FRAME_QUERY_CHANNELS, FRAME_QUERY_CONTACTS, FRAME_QUERY_STATUS, FRAME_RSP_ADVERT,
    FRAME_RSP_CHANNEL, FRAME_RSP_CHANNELS_DONE, FRAME_RSP_CONTACT, FRAME_RSP_CONTACTS_DONE,
    FRAME_RSP_ERROR, FRAME_RSP_HISTORY_DONE, FRAME_RSP_HISTORY_ENTRY, FRAME_RSP_IDENTITY,
    FRAME_RSP_OK, FRAME_RSP_STATUS, FRAME_SET_DEVICE_NAME, FRAME_SET_NOTIF_DEFAULTS,
    FRAME_SET_PIN, FRAME_OVERHEAD, MAX_NAME_LEN, PROV_MAGIC, ProvError, RspStatusPayload,
    decode_add_channel, decode_add_contact, decode_del_channel, decode_del_contact, decode_frame,
    decode_set_device_name, decode_set_notif_defaults, decode_set_pin,
    encode_frame, encode_rsp_channel, encode_rsp_contact, encode_rsp_error, encode_rsp_identity,
    encode_rsp_status,
};
use protocol::{encode_rsp_history_entry, Identity, MAX_ADVERT_CARD_LEN, MAX_RSP_HISTORY_ENTRY_PAYLOAD};

use crate::advert_ts_store;
use crate::config_store::{
    Channel, ChannelListFull, ChannelUpsert, Contact, ContactListFull, ContactUpsert,
    NotifDefaults, ProvisionedConfig,
};
use crate::history_store::HistoryStore;

/// Frame receive buffer.  Sized to match `provisioning_server` (512 B) so the
/// largest host command frame always fits: an `ADD_CONTACT` / `ADD_CHANNEL`
/// with a full `MAX_NAME_LEN` name is up to 74 bytes, and the host's retry
/// logic can burst several command frames into the USB-JTAG ring before this
/// thread next polls.  The previous 64-byte buffer could not hold a long-name
/// edit frame (it would forever decode as `TruncatedFrame`, hanging that
/// command) nor absorb a retry burst without spanning frame boundaries
/// awkwardly — a latent desync source under the host's 500 ms retry cadence.
const RX_BUF_LEN: usize = 512;

/// Application error codes sent in `RspError.error_code`.  Mirrors the
/// `provisioning_server` codes so the host sees identical errors in either state.
mod err {
    pub const CONTACT_LIST_FULL: u8 = 0x01;
    pub const CHANNEL_LIST_FULL: u8 = 0x02;
    pub const CONTACT_NOT_FOUND: u8 = 0x03;
    pub const CHANNEL_NOT_FOUND: u8 = 0x04;
    pub const DECODE_ERROR: u8      = 0x05;
    pub const STORAGE_ERROR: u8     = 0x06;
}

/// Entry point for the admin server thread.
///
/// Loops forever reading frames from stdin and responding over stdout.
/// Intended to be spawned via `std::thread::Builder::new().stack_size(8192).spawn(...)`.
///
/// `history` must be the shared global `HISTORY` mutex so that export reads
/// are mutually excluded with main-thread appends (module-level mutex discipline).
///
/// `identity` is the device's own Ed25519 identity (public key, returned in
/// the `QUERY_STATUS` / `RSP_IDENTITY` replies; the seed, used to sign the
/// `QUERY_ADVERT` self-advert card — this thread needs the whole `Identity`,
/// not just the pubkey, for that reason). It is a `Clone` of the one
/// `main.rs` holds; the seed staying in two in-memory copies is no wider an
/// exposure than the existing single-thread copy (never leaves the device
/// over any interface — see `identity_store.rs`'s doc).
///
/// `config` is the provisioned config loaded at boot — the single mutable
/// source of truth for the `QUERY_*` replies and the `ADD_*` / `DEL_*` edits.
/// It is moved into the thread; runtime edits mutate it and persist the whole
/// blob back to `nvs_partition`.
///
/// `gps_status` is the shared GPS status snapshot the main thread refreshes
/// every dispatcher-loop iteration (same cross-thread mutex pattern as
/// `history`) — read here to answer `QUERY_STATUS` with live fix / clock-sync
/// fields.
///
/// `battery_status` is the shared battery status snapshot (charge percentage +
/// charging state), refreshed the same way as `gps_status` — read here so
/// `QUERY_STATUS` reports the same battery reading the radio telemetry
/// RESPONSE and the admin-menu screen show (single shared source; see
/// `battery` module docs).
pub fn run(
    history: &'static std::sync::Mutex<Option<HistoryStore>>,
    gps_status: &'static std::sync::Mutex<crate::gps::GpsStatus>,
    battery_status: &'static std::sync::Mutex<crate::battery::BatteryStatus>,
    identity: Identity,
    mut config: ProvisionedConfig,
    nvs_partition: EspNvsPartition<NvsDefault>,
) {
    // Device display name lives in the identity store (`mc_id`/`name`), not
    // the provisioned config blob — see identity_store.rs doc comment. Load
    // whatever is already persisted (possibly set during first-boot
    // provisioning, or empty) so QUERY_STATUS reflects it, and keep it in
    // sync in-thread as SET_DEVICE_NAME arrives.
    let (mut device_name, mut device_name_len) =
        crate::identity_store::load_name(nvs_partition.clone()).unwrap_or_else(|e| {
            log::warn!("admin_server: identity_store::load_name failed: {:?} — starting unnamed", e);
            ([0u8; MAX_NAME_LEN], 0)
        });

    let mut rx_buf = [0u8; RX_BUF_LEN];
    let mut rx_len = 0usize;
    let mut stdout_ = std::io::stdout();

    loop {
        // ── Read more bytes ──────────────────────────────────────────────────
        // Bypass VFS line discipline: binary admin-server frames carry no
        // newline, so std::io::stdin() would never deliver them.  Read directly
        // from the driver ring buffer instead (ticks_to_wait = 0, non-blocking).
        if rx_len < RX_BUF_LEN {
            let slice = &mut rx_buf[rx_len..];
            // SAFETY: driver installed at boot in main.rs before this thread.
            let n_signed = unsafe {
                esp_idf_svc::sys::usb_serial_jtag_read_bytes(
                    slice.as_mut_ptr() as *mut core::ffi::c_void,
                    slice.len() as u32,
                    0, // non-blocking
                )
            };
            if n_signed <= 0 {
                esp_idf_hal::delay::FreeRtos::delay_ms(50);
                continue;
            }
            rx_len += n_signed as usize;
        }

        // ── Frame synchronisation ────────────────────────────────────────────
        let sync = find_magic_start(&rx_buf[..rx_len]);
        if sync > 0 {
            rx_buf.copy_within(sync..rx_len, 0);
            rx_len -= sync;
            continue;
        }

        // ── Try to decode one frame ──────────────────────────────────────────
        match decode_frame(&rx_buf[..rx_len]) {
            Ok((frame_type, payload)) => {
                let payload_len = payload.len();
                let frame_len = FRAME_OVERHEAD + payload_len;

                // Copy payload before shifting buffer.
                let mut payload_copy = [0u8; 256];
                let copy_len = payload_len.min(payload_copy.len());
                payload_copy[..copy_len].copy_from_slice(&payload[..copy_len]);

                // Consume the frame from rx_buf.
                rx_buf.copy_within(frame_len..rx_len, 0);
                rx_len -= frame_len;

                if let Err(e) = handle_frame(
                    frame_type,
                    &payload_copy[..copy_len],
                    history,
                    gps_status,
                    battery_status,
                    &identity,
                    &mut config,
                    &nvs_partition,
                    &mut device_name,
                    &mut device_name_len,
                    &mut stdout_,
                ) {
                    log::warn!("admin_server: handle_frame error: {}", e);
                }
            }
            Err(ProvError::TruncatedFrame) => {
                // Not enough bytes yet — wait for more.
            }
            Err(e) => {
                // Bad CRC or magic mismatch.  Discard the first byte and resync.
                log::warn!("admin_server: frame decode error: {:?}", e);
                if rx_len > 0 {
                    rx_buf.copy_within(1..rx_len, 0);
                    rx_len -= 1;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_frame(
    frame_type: u8,
    payload: &[u8],
    history: &std::sync::Mutex<Option<HistoryStore>>,
    gps_status: &std::sync::Mutex<crate::gps::GpsStatus>,
    battery_status: &std::sync::Mutex<crate::battery::BatteryStatus>,
    identity: &Identity,
    config: &mut ProvisionedConfig,
    nvs_partition: &EspNvsPartition<NvsDefault>,
    device_name: &mut [u8; MAX_NAME_LEN],
    device_name_len: &mut u8,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    match frame_type {
        // ── QUERY_STATUS ─────────────────────────────────────────────────────
        // The host CLI's connect-and-verify command.  Mirror the provisioning
        // server's two-frame reply (RSP_STATUS then RSP_IDENTITY) so that the
        // host's `Session::query_status` — which consumes both frames — works
        // identically against a provisioned device.  `provisioned` is `true`
        // here by construction: admin_server only runs after the provisioned
        // gate in main.rs.  The counts read the live config so they match the
        // QUERY_CONTACTS / QUERY_CHANNELS streams below.
        FRAME_QUERY_STATUS => {
            log::info!(
                "admin_server: QUERY_STATUS — provisioned contacts={} channels={}",
                config.contact_count, config.channel_count
            );
            // Poisoned-mutex fallback: a panic elsewhere while holding the
            // lock must not hang QUERY_STATUS forever — report "never" (no
            // fix / not synced) rather than propagating the poison here.
            let gps = gps_status.lock().map(|g| *g).unwrap_or_else(|e| *e.into_inner());
            // Same poisoned-mutex fallback for battery: report "unknown"
            // (0%, not charging) rather than propagating the poison.
            let battery = battery_status.lock().map(|b| *b).unwrap_or_else(|e| *e.into_inner());
            let status = RspStatusPayload {
                provisioned: true,
                pubkey: identity.pubkey,
                contact_count: config.contact_count,
                channel_count: config.channel_count,
                gps_has_fix: gps.has_fix,
                gps_lat_e7: gps.lat_e7,
                gps_lon_e7: gps.lon_e7,
                gps_fix_age_secs: gps.fix_age_secs,
                gps_clock_synced: gps.clock_synced,
                gps_clock_sync_age_secs: gps.clock_sync_age_secs,
                battery_percent: battery.percent,
                battery_charging: battery.charging,
                battery_raw_mv: crate::battery::clamp_raw_mv_for_wire(battery.raw_mv),
                battery_held_raw_mv: crate::battery::clamp_raw_mv_for_wire(battery.held_raw_mv),
            };
            let mut pbuf = [0u8; 64];
            let plen = encode_rsp_status(&status, &mut pbuf);
            send_frame(out, FRAME_RSP_STATUS, &pbuf[..plen])?;

            let mut ibuf = [0u8; 64];
            let ilen = encode_rsp_identity(
                &identity.pubkey,
                &device_name[..*device_name_len as usize],
                &mut ibuf,
            );
            send_frame(out, FRAME_RSP_IDENTITY, &ibuf[..ilen])?;
        }
        // ── QUERY_ADVERT ─────────────────────────────────────────────────────
        // Build + sign this device's self-advert "biz card" and reply
        // RSP_ADVERT with the raw card bytes — the host/browser-side
        // provisioner UI's "share my card" action.
        //
        // GUARD (non-negotiable, campaign-wide): the card is built by
        // `firmware_core::advert::handle_query_advert`, a pure function whose
        // signature has no `TxQueue` / dispatcher / radio parameter anywhere
        // in its call graph, and its result goes straight into `send_frame`
        // below (the provisioning serial writer) — this handler never
        // touches `txq`, the dispatcher, or any radio path, structurally,
        // not just by convention.
        //
        // Timestamp: MeshCadet has no RTC, so `identity`'s card cannot use
        // `main.rs`'s `tx_epoch_base` (a per-boot `esp_random()` value —
        // useless here, see `firmware_core::advert` module docs: MeshCore's
        // replay guard on the RECEIVING peer drops a re-imported advert when
        // `timestamp <= last_advert_timestamp` already on file for that
        // contact). `advert_ts_store` persists a durable, strictly
        // increasing counter across reboots instead; the NVS write happens
        // BEFORE the reply is sent (see `advert_ts_store::save_last_advert_ts`'s
        // doc) so a crash between the two cannot regress it.
        //
        // Name: the configured device name, or (if unset) a pub_hash-derived
        // `MeshCadet-<HH>` label — an advert with an empty name is silently
        // dropped by every receiver, so this never emits one.
        FRAME_QUERY_ADVERT => {
            let name_str =
                std::str::from_utf8(&device_name[..*device_name_len as usize]).unwrap_or("");
            let nvs_last = advert_ts_store::load_last_advert_ts(nvs_partition.clone());
            let mut card_buf = [0u8; MAX_ADVERT_CARD_LEN];
            match firmware_core::advert::handle_query_advert(
                identity,
                payload,
                nvs_last,
                name_str,
                &mut card_buf,
            ) {
                Ok((n, new_ts)) => {
                    // Persist BEFORE replying (see doc comment above).
                    advert_ts_store::save_last_advert_ts(nvs_partition.clone(), new_ts);
                    log::info!(
                        "admin_server: QUERY_ADVERT — {} byte card, timestamp={}",
                        n, new_ts
                    );
                    send_frame(out, FRAME_RSP_ADVERT, &card_buf[..n])?;
                }
                Err(e) => {
                    log::warn!("admin_server: QUERY_ADVERT decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"query_advert decode error");
                }
            }
        }
        // ── QUERY_CONTACTS ───────────────────────────────────────────────────
        // Stream every provisioned contact (index-ordered) as RSP_CONTACT
        // frames, terminated by RSP_CONTACTS_DONE — mirrors the provisioning
        // server's QUERY_CONTACTS response so the host's `Session::list_contacts`
        // works identically against a provisioned device.
        FRAME_QUERY_CONTACTS => {
            let cnt = config.contact_count as usize;
            log::info!("admin_server: QUERY_CONTACTS — {} contact(s)", cnt);
            for i in 0..cnt {
                let c = &config.contacts[i];
                // Clamp against the backing array length: a corrupt NVS blob with
                // name_len > MAX_NAME_LEN would otherwise panic the thread (the
                // build is panic=abort → device reboot), masquerading as a hung /
                // empty enumeration.
                let name_len = (c.display_name_len as usize).min(c.display_name.len());
                let name = &c.display_name[..name_len];
                let mut pbuf = [0u8; 80];
                let plen = encode_rsp_contact(i as u8, &c.pubkey, c.telemetry_enable, name, &mut pbuf);
                send_frame(out, FRAME_RSP_CONTACT, &pbuf[..plen])?;
            }
            send_frame(out, FRAME_RSP_CONTACTS_DONE, &[])?;
        }
        // ── QUERY_CHANNELS ───────────────────────────────────────────────────
        // Stream every provisioned channel (index-ordered) as RSP_CHANNEL
        // frames, terminated by RSP_CHANNELS_DONE.  The channel_hash is computed
        // here (key_len-aware) so the secret never leaves the device.
        FRAME_QUERY_CHANNELS => {
            let cnt = config.channel_count as usize;
            log::info!("admin_server: QUERY_CHANNELS — {} channel(s)", cnt);
            for i in 0..cnt {
                let ch = &config.channels[i];
                // Clamp both length fields against their backing arrays: a corrupt
                // NVS blob with key_len > 32 or name_len > MAX_NAME_LEN would
                // otherwise panic the thread (panic=abort → device reboot),
                // surfacing to the host as a hung / empty channel enumeration.
                let key_len = (ch.key_len as usize).min(ch.secret.len());
                let hash = channel_hash_var(&ch.secret[..key_len]);
                let name_len = (ch.name_len as usize).min(ch.name.len());
                let name = &ch.name[..name_len];
                let mut pbuf = [0u8; 80];
                let plen = encode_rsp_channel(i as u8, hash, ch.key_len, ch.primary, name, &mut pbuf);
                send_frame(out, FRAME_RSP_CHANNEL, &pbuf[..plen])?;
            }
            send_frame(out, FRAME_RSP_CHANNELS_DONE, &[])?;
        }
        // ── ADD_CONTACT ──────────────────────────────────────────────────────
        // Idempotent UPSERT keyed on the contact's pubkey (its cryptographic
        // identity), then persist the whole blob to NVS.  A known pubkey updates
        // the existing entry in place (telemetry flag / name refreshed; count
        // unchanged) — re-adding the same contact no longer stacks duplicates.
        // A new pubkey appends.  The next QUERY_CONTACTS / QUERY_STATUS reflects
        // it immediately.
        //
        // DEFECT FIX (pull-telemetry-not-answered-for-enabled-contact): the prior
        // append-only path stacked a duplicate when a contact was re-added to
        // enable telemetry.  Because the dispatcher's PolicyFilter and the
        // telemetry gate are first-match-wins, the stale (telemetry=false) entry
        // shadowed the refreshed one, so `list-contacts` showed telemetry=true
        // while the on-air pull was silently dropped.  Upsert keeps the stored
        // flag and the enforced gate in agreement.
        FRAME_ADD_CONTACT => {
            match decode_add_contact(payload) {
                Ok(c) => {
                    let new_contact = Contact {
                        pubkey:           c.pubkey,
                        telemetry_enable: c.telemetry_enable,
                        display_name:     c.display_name,
                        display_name_len: c.display_name_len,
                    };
                    match config.upsert_contact(new_contact) {
                        Ok(outcome) => {
                            log::info!(
                                "admin_server: ADD_CONTACT ({}) pub_hash=0x{:02x} telemetry={} name_len={}",
                                match outcome {
                                    ContactUpsert::Updated => "updated",
                                    ContactUpsert::Added => "added",
                                },
                                c.pubkey[0], c.telemetry_enable, c.display_name_len
                            );
                            persist_or_rollback(config, nvs_partition, out, ConfigKind::Contact)?;
                        }
                        Err(ContactListFull) => {
                            return send_error(out, err::CONTACT_LIST_FULL, b"contact list full");
                        }
                    }
                }
                Err(e) => {
                    log::warn!("admin_server: ADD_CONTACT decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"add_contact decode error");
                }
            }
        }
        // ── DEL_CONTACT ──────────────────────────────────────────────────────
        FRAME_DEL_CONTACT => {
            match decode_del_contact(payload) {
                Ok(d) => {
                    let cnt = config.contact_count as usize;
                    match config.contacts[..cnt].iter().position(|c| c.pubkey == d.pubkey) {
                        Some(idx) => {
                            for j in idx..cnt - 1 {
                                config.contacts[j] = config.contacts[j + 1];
                            }
                            config.contact_count -= 1;
                            log::info!("admin_server: DEL_CONTACT pub_hash=0x{:02x}", d.pubkey[0]);
                            persist_or_rollback(config, nvs_partition, out, ConfigKind::Contact)?;
                        }
                        None => return send_error(out, err::CONTACT_NOT_FOUND, b"contact not found"),
                    }
                }
                Err(e) => {
                    log::warn!("admin_server: DEL_CONTACT decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"del_contact decode error");
                }
            }
        }
        // ── ADD_CHANNEL ──────────────────────────────────────────────────────
        // Idempotent UPSERT keyed on the channel secret (its cryptographic
        // identity), then persist the whole blob to NVS.  A known key updates
        // the existing entry in place (rename / refresh primary; count
        // unchanged) — re-adding the same channel no longer stacks duplicates.
        // A new key appends.  `upsert_channel` enforces the at-most-one-primary
        // invariant.  The next QUERY_CHANNELS / QUERY_STATUS reflects it
        // immediately — this is the path that previously fell through to the
        // unknown-frame arm and made `meshcadet add-channel` time out against a
        // provisioned device.
        FRAME_ADD_CHANNEL => {
            match decode_add_channel(payload) {
                Ok(ch) => {
                    let new_channel = Channel {
                        secret:   ch.secret,
                        key_len:  ch.key_len,
                        primary:  ch.primary,
                        name:     ch.name,
                        name_len: ch.name_len,
                    };
                    match config.upsert_channel(new_channel) {
                        Ok(outcome) => {
                            log::info!(
                                "admin_server: ADD_CHANNEL ({}) secret[0]=0x{:02x} key_len={} primary={} name_len={}",
                                match outcome {
                                    ChannelUpsert::Updated => "updated",
                                    ChannelUpsert::Added => "added",
                                },
                                ch.secret[0], ch.key_len, ch.primary, ch.name_len
                            );
                            persist_or_rollback(config, nvs_partition, out, ConfigKind::Channel)?;
                        }
                        Err(ChannelListFull) => {
                            return send_error(out, err::CHANNEL_LIST_FULL, b"channel list full");
                        }
                    }
                }
                Err(e) => {
                    log::warn!("admin_server: ADD_CHANNEL decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"add_channel decode error");
                }
            }
        }
        // ── DEL_CHANNEL ──────────────────────────────────────────────────────
        FRAME_DEL_CHANNEL => {
            match decode_del_channel(payload) {
                Ok(d) => {
                    let cnt = config.channel_count as usize;
                    match config.channels[..cnt].iter().position(|ch| ch.secret == d.secret) {
                        Some(idx) => {
                            for j in idx..cnt - 1 {
                                config.channels[j] = config.channels[j + 1];
                            }
                            config.channel_count -= 1;
                            log::info!("admin_server: DEL_CHANNEL secret[0]=0x{:02x}", d.secret[0]);
                            persist_or_rollback(config, nvs_partition, out, ConfigKind::Channel)?;
                        }
                        None => return send_error(out, err::CHANNEL_NOT_FOUND, b"channel not found"),
                    }
                }
                Err(e) => {
                    log::warn!("admin_server: DEL_CHANNEL decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"del_channel decode error");
                }
            }
        }
        // ── SET_NOTIF_DEFAULTS / SET_PIN ──────────────────────────────────────
        // The host CLI's `set-notif-defaults` and `set-pin` / `reset-pin`
        // commands.  Like the edit frames above these used to fall through to
        // the unknown-frame arm and hang the host (no reply emitted).
        // `reset-pin` in particular is a documented recovery flow against a
        // *provisioned* device (host/src/main.rs), so the runtime server must
        // answer SET_PIN.  Each mutates the live config and persists the whole
        // blob immediately — same model as ADD_*/DEL_*.
        //
        // (Formerly also handled `FRAME_SET_RADIO_PRESET` and `FRAME_SET_LOCKS`
        // here — both retired as dead host commands with no firmware consumer,
        // per an audit of every host command's actual firmware usage.)
        FRAME_SET_NOTIF_DEFAULTS => {
            match decode_set_notif_defaults(payload) {
                Ok(n) => {
                    config.notif_defaults = NotifDefaults { visual: n.visual, audible: n.audible };
                    log::info!(
                        "admin_server: SET_NOTIF_DEFAULTS visual={} audible={}",
                        n.visual, n.audible
                    );
                    persist_setting(config, nvs_partition, out)?;
                }
                Err(e) => {
                    log::warn!("admin_server: SET_NOTIF_DEFAULTS decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"set_notif_defaults decode error");
                }
            }
        }
        FRAME_SET_PIN => {
            match decode_set_pin(payload) {
                Ok(p) => {
                    config.pin     = p.pin;
                    config.pin_len = p.pin_len;
                    log::info!("admin_server: SET_PIN len={}", p.pin_len);
                    persist_setting(config, nvs_partition, out)?;
                }
                Err(e) => {
                    log::warn!("admin_server: SET_PIN decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"set_pin decode error");
                }
            }
        }
        // ── SET_DEVICE_NAME ──────────────────────────────────────────────────
        // The host CLI's `identity --set-name` command. Persists straight to
        // the identity store (`mc_id`/`name`) — unlike SET_PIN/SET_NOTIF_DEFAULTS
        // this is not part of `ProvisionedConfig`, so there is no config blob
        // to re-persist; only the in-thread mirror + the identity-store write.
        FRAME_SET_DEVICE_NAME => {
            match decode_set_device_name(payload) {
                Ok(n) => {
                    match crate::identity_store::set_name(nvs_partition.clone(), &n.name[..n.name_len as usize]) {
                        Ok(()) => {
                            *device_name = n.name;
                            *device_name_len = n.name_len;
                            log::info!("admin_server: SET_DEVICE_NAME len={}", n.name_len);
                            send_ok(out)?;
                        }
                        Err(e) => {
                            log::error!("admin_server: identity_store::set_name failed: {:?}", e);
                            return send_error(out, err::STORAGE_ERROR, b"NVS save failed");
                        }
                    }
                }
                Err(e) => {
                    log::warn!("admin_server: SET_DEVICE_NAME decode: {:?}", e);
                    return send_error(out, err::DECODE_ERROR, b"set_device_name decode error");
                }
            }
        }
        // ── COMMIT_PROVISIONING ──────────────────────────────────────────────
        // The host CLI's `commit` command.  On a provisioned device every edit
        // already persists immediately, so commit is an idempotent confirmation:
        // re-persist the live config and acknowledge with RSP_OK.  This is the
        // direct fix for the HIL "commit → timeout (accumulated 0 bytes)"
        // symptom — commit previously fell through to the unknown-frame arm,
        // which logs at debug and sends NO response, so the host blocked until
        // its retry deadline and reported a 0-byte timeout.
        //
        // Unlike the first-boot `provisioning_server`, we do NOT reboot: the
        // device is already provisioned and actively running the radio/UI, so a
        // reboot would needlessly drop the mesh link (and risk losing the RSP_OK
        // to a reset/USB-drain race).
        FRAME_COMMIT_PROVISIONING => {
            log::info!("admin_server: COMMIT_PROVISIONING — re-persisting live config (no reboot)");
            persist_setting(config, nvs_partition, out)?;
        }
        FRAME_EXPORT_HISTORY => {
            // Lock the shared history store so this multi-op NVS read
            // sequence is mutually excluded with main-thread appends,
            // honouring the module-level mutex discipline.
            let mut export_err: Option<esp_idf_svc::sys::EspError> = None;
            {
                // `mut`/`ref mut`: unlike the retired NVS store (whose
                // `EspNvs` handle was cheaply `Arc`-cloned per call, so reads
                // only ever needed `&self`), the flash store's `EspPartition`
                // read/write/erase calls all take `&mut self` (real hardware
                // I/O, not a shared handle) — this is a mechanical adjustment
                // for the new store, not a change to the export handler's
                // frame semantics (still oldest-first entries, same
                // FRAME_RSP_HISTORY_ENTRY/DONE wire shape).
                let mut guard = history
                    .lock()
                    .expect("HISTORY mutex poisoned in admin_server");
                if let Some(ref mut store) = *guard {
                    let result = store.export_entries(|idx, entry, is_ours| {
                        let mut pbuf = [0u8; MAX_RSP_HISTORY_ENTRY_PAYLOAD + 1];
                        let plen = encode_rsp_history_entry(idx, entry, is_ours, &mut pbuf);
                        if let Err(e) = send_frame(out, FRAME_RSP_HISTORY_ENTRY, &pbuf[..plen]) {
                            log::warn!("admin_server: write HISTORY_ENTRY failed: {}", e);
                        }
                    });
                    if let Err(e) = result {
                        export_err = Some(e);
                    }
                }
                // guard drops here, releasing the mutex before the DONE frame.
            }
            // Always send DONE (so host doesn't hang), even if some entries failed.
            send_frame(out, FRAME_RSP_HISTORY_DONE, &[])?;
            if let Some(e) = export_err {
                log::warn!("admin_server: export_entries NVS error: {:?}", e);
            }
        }
        // ── CLEAR_HISTORY ─────────────────────────────────────────────────────
        // The host CLI's `clear-history` command. Erases every sector of every
        // conversation region on the flash-backed `mc_hist` store — ALL DM and
        // channel history, both directions — via `HistoryStore::clear_all`
        // (built on the same `erase_sector` primitive every other
        // flash-mutating path in that store already uses). Same mutex
        // discipline as `FRAME_EXPORT_HISTORY`: the multi-sector erase
        // sequence is mutually excluded with main-thread appends.
        //
        // DESIGN DECISION (reboot-required MVP, not a live in-memory clear):
        // this handler has no reach into `ui::UiRuntime`'s `messages`/`unread`
        // maps — those are owned by the main/UI thread, and this thread only
        // holds the `HISTORY` mutex + the provisioned config, not a UI handle.
        // Wiring a live cross-thread UI-clear channel is a larger change than
        // this fix scopes; a reboot re-hydrates the UI from the
        // now-empty store via the existing boot-hydrate path (main.rs), the
        // same "takes effect on next boot" contract every other runtime
        // provisioning edit already has (ADD_CONTACT/ADD_CHANNEL/etc.) — see
        // the `CLEAR_HISTORY` amendment in ADR-0002. `RSP_OK` here only
        // promises the flash erase completed; the host CLI's own output is
        // what tells the user a reboot is needed to see empty history on
        // screen.
        FRAME_CLEAR_HISTORY => {
            // Same lock-then-release-before-reply shape as FRAME_EXPORT_HISTORY:
            // the multi-sector erase is mutually excluded with main-thread
            // appends, but the guard is dropped before the ack is written so
            // the reply path never blocks other HISTORY consumers.
            let mut guard = history
                .lock()
                .expect("HISTORY mutex poisoned in admin_server");
            let outcome = match guard.as_mut() {
                Some(store) => Some(store.clear_all()),
                None => None,
            };
            drop(guard);
            match outcome {
                Some(Ok(())) => {
                    log::info!("admin_server: CLEAR_HISTORY — all conversation history erased");
                    send_ok(out)?;
                }
                Some(Err(e)) => {
                    log::error!("admin_server: CLEAR_HISTORY erase failed: {:?}", e);
                    send_error(out, err::STORAGE_ERROR, b"history erase failed")?;
                }
                None => {
                    // Should not happen post-boot (HISTORY is populated before
                    // this thread is spawned — see main.rs) but fail safe
                    // rather than panic if it ever does.
                    log::error!("admin_server: CLEAR_HISTORY — HISTORY store not initialised");
                    send_error(out, err::STORAGE_ERROR, b"history store not initialised")?;
                }
            }
        }
        other => {
            log::debug!("admin_server: unknown frame type 0x{:02X}", other);
        }
    }
    Ok(())
}

/// Which list a just-applied edit touched — used only to roll the in-memory
/// `count` back by one if the NVS persist fails, so the live config never
/// diverges from what is actually on flash.
enum ConfigKind {
    Contact,
    Channel,
}

/// Persist the whole config blob to NVS after an in-memory edit.
///
/// On success, replies `RSP_OK`.  On NVS failure, rolls the just-applied edit's
/// list count back by one (so the in-memory view matches what is durably stored)
/// and replies `RSP_ERROR(STORAGE_ERROR)` — the host then knows the edit did not
/// take.  Add/del both move the count by exactly one entry, so a single decrement
/// restores the pre-edit count; the stale entry beyond `count` is never read.
fn persist_or_rollback(
    config: &mut ProvisionedConfig,
    nvs_partition: &EspNvsPartition<NvsDefault>,
    out: &mut impl Write,
    kind: ConfigKind,
) -> anyhow::Result<()> {
    match crate::config_store::save_provisioned_config(nvs_partition.clone(), config) {
        Ok(()) => send_ok(out),
        Err(e) => {
            log::error!("admin_server: NVS save failed: {:?}", e);
            // Note: a delete has already shifted entries down; we cannot fully
            // un-shift, but the count rollback keeps the in-memory view sized to
            // the persisted blob.  This only matters if NVS is failing, which is
            // already a degraded state requiring reprovisioning.
            match kind {
                ConfigKind::Contact => {
                    config.contact_count = config.contact_count.saturating_sub(1);
                }
                ConfigKind::Channel => {
                    config.channel_count = config.channel_count.saturating_sub(1);
                }
            }
            send_error(out, err::STORAGE_ERROR, b"NVS save failed").map(|_| ())
        }
    }
}

/// Persist the live config to NVS after a scalar-setting edit (`SET_*`) or a
/// `COMMIT`, then reply `RSP_OK` / `RSP_ERROR(STORAGE_ERROR)`.
///
/// Unlike [`persist_or_rollback`], the `SET_*` / `COMMIT` paths change a scalar
/// field (radio preset, notif defaults, PIN, lock flags) or nothing at all
/// (commit), so there is no list `count` to roll back on failure — the
/// in-memory edit simply stands and the host is told the durable write did not
/// take.  An NVS failure here is already a degraded state requiring
/// reprovisioning; the in-memory/flash divergence is a scalar field, not a
/// count that would desync the `QUERY_*` enumerations.
fn persist_setting(
    config: &ProvisionedConfig,
    nvs_partition: &EspNvsPartition<NvsDefault>,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    match crate::config_store::save_provisioned_config(nvs_partition.clone(), config) {
        Ok(()) => send_ok(out),
        Err(e) => {
            log::error!("admin_server: NVS save failed: {:?}", e);
            send_error(out, err::STORAGE_ERROR, b"NVS save failed").map(|_| ())
        }
    }
}

// ── Response helpers ──────────────────────────────────────────────────────────

/// Encode and write one complete frame, then **flush immediately**.
///
/// This is the single delivery primitive for every admin_server response —
/// single-frame replies (`RSP_OK`/`RSP_ERROR`) *and* every frame of a streamed
/// enumeration (`RSP_CONTACT`/`RSP_CHANNEL`/`RSP_HISTORY_ENTRY` + their `DONE`
/// terminators).  It mirrors `provisioning_server::send_frame` byte-for-byte.
///
/// READ-PATH/WRITE-PATH SYMMETRY (the list-channels defect this fixes): the
/// receive side already bypasses VFS line discipline (`usb_serial_jtag_read_bytes`)
/// because binary frames carry no newline.  The transmit side goes through Rust's
/// `std::io::Stdout`, which is *always* a `LineWriter` — so binary frames (no
/// `\n`) are held in its buffer until an explicit `flush()`.  The streaming
/// query handlers used to `write_all` every frame and `flush()` only once at the
/// end, batching the whole enumeration into the LineWriter before a single
/// flush.  The single-frame replies (`send_ok`/`send_error`) and the
/// provisioning_server's streams flush per frame and are reliable; the batched
/// streams were not, which is exactly why `list-channels` could come back empty
/// on-device while `status` (and `add-channel`'s `RSP_OK`) worked — and why host
/// tests (in-memory transport, no LineWriter, no USB-JTAG) never caught it.
/// Flushing per frame makes the streamed enumeration use the identical, proven
/// delivery discipline as the single-frame replies.
fn send_frame(out: &mut impl Write, frame_type: u8, payload: &[u8]) -> anyhow::Result<()> {
    let mut frame_buf = [0u8; 512];
    let n = encode_frame(frame_type, payload, &mut frame_buf);
    // Hold the shared serial-TX lock across the whole frame (write + flush) so a
    // concurrent ESP-IDF log line from the radio/UI threads cannot interleave
    // mid-frame and corrupt the host's parse. The C logger takes this same lock
    // via the serial_console vprintf hook, so logs only ever land BETWEEN frames
    // (which the host resync-on-MAGIC parser already tolerates). This is the
    // list-channels "no channels configured" corruption fix. No logging happens
    // inside this critical section, so the lock never nests.
    let _tx = crate::serial_console::lock_tx();
    out.write_all(&frame_buf[..n]).map_err(|e| anyhow!("stdout write: {}", e))?;
    out.flush().map_err(|e| anyhow!("stdout flush: {}", e))?;
    Ok(())
}

fn send_ok(out: &mut impl Write) -> anyhow::Result<()> {
    send_frame(out, FRAME_RSP_OK, &[])
}

fn send_error(out: &mut impl Write, code: u8, msg: &[u8]) -> anyhow::Result<()> {
    let mut payload_buf = [0u8; 128];
    let plen = encode_rsp_error(code, msg, &mut payload_buf);
    send_frame(out, FRAME_RSP_ERROR, &payload_buf[..plen])
}

/// Find the index of the first byte that could start a `PROV_MAGIC` sequence.
/// Returns `buf.len()` if no candidate start is found.
fn find_magic_start(buf: &[u8]) -> usize {
    let m0 = PROV_MAGIC[0];
    let m1 = PROV_MAGIC[1];
    if buf.len() < 2 {
        return 0; // not enough bytes to tell yet
    }
    if buf[0] == m0 && buf[1] == m1 {
        return 0; // already synced
    }
    for i in 1..buf.len() {
        if buf[i] == m0 {
            // Possible start — check if the next byte (if present) matches.
            if i + 1 < buf.len() {
                if buf[i + 1] == m1 {
                    return i;
                }
                // Next byte doesn't match — skip past it.
            } else {
                return i; // only one byte peeked; might still be a start
            }
        }
    }
    buf.len() // no candidate found; discard everything
}
