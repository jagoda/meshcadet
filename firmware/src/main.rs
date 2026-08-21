// SPDX-License-Identifier: GPL-3.0-only
//! MeshCadet T-Deck Plus firmware — radio + identity + policy + GPS telemetry.
//!
//! # Boot sequence
//! 1. esp-idf runtime init (`link_patches`, default logger).
//! 1.5. Install USB-Serial-JTAG interrupt-driven driver (production — enables stdin).
//! 2. Load (or generate + persist) Ed25519 identity from NVS — OR, in a `hil`
//!    build, derive a fixed compiled Ed25519 seed (`hil_keys.rs`).
//! 3. Initialise SX1262 at the locked preset.
//! 4. Initialise GPS UART1 (GPIO43/44, 9600 baud, L76K).
//! 4.5. Initialise the battery ADC (GPIO4, `battery` module).
//! 5. Provision a single TEST contact for M1 on-air validation.
//! 6. Run the dispatcher loop: CAD → TX (if pending) → RX poll → dedup → decode.
//!
//! # Application-layer paths (M1 HIL interop gate)
//! - **REQ (0x00)**: MeshCore-native request datagram. `REQ_TYPE_GET_TELEMETRY_DATA`
//!   is the stock companion app's telemetry/location button; if the contact has
//!   telemetry enabled, reply with a `RESPONSE` (0x01) carrying the reflected tag
//!   + a Cayenne-LPP GPS fix + a battery percentage/charging-state pair. Non-enabled
//!   contacts get no reply. This is the real on-air telemetry pull (the `?loc` DM
//!   below is a bespoke fallback no stock companion sends, and carries location only).
//! - **DM (TXT_MSG, 0x02)**: decode, log, ACK. If the DM text is `?loc` (a
//!   telemetry pull request) and the contact has telemetry enabled, reply with
//!   the cached GPS fix (age included). Non-enabled contacts receive no reply.
//! - **ACK (0x03)**: match against pending ACK.
//! - **PATH-return (0x08)**: extract bundled ACK.
//! - **GRP_TXT (0x05)**: decode and log.
//!
//! # GPS / telemetry (M3)
//! - GPS UART1 on GPIO43 (TX) / GPIO44 (RX), Quectel L76K, 9600 baud, 8N1.
//! - Console redirected to USB-Serial-JTAG (`CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y`
//!   in `sdkconfig.defaults`) so GPIO43/44 are exclusively available for GPS.
//! - Duty cycle: 30 s active reading window every 120 s (2 min), power-conserving.
//! - Cached last-known fix; `age_secs` surfaced in every telemetry response.
//! - Pull-only: MeshCadet NEVER pushes location unsolicited.
//! - Policy gate: `policy.telemetry_enabled(src_hash)` guards every reply —
//!   non-enabled contacts' requests are silently dropped (no ACK, no log leak).
//!
//! # Battery status
//! - ADC voltage divider on GPIO4 (`BOARD_BAT_ADC`) — no PMU/fuel-gauge IC on
//!   this board; see `battery` module docs for the full hardware-feasibility
//!   gate and the charging-state inference mechanism.
//! - Two fields surfaced to the on-air telemetry RESPONSE and the on-device
//!   admin-menu screen: charge percentage + charging state (never raw
//!   voltage there — a deliberate design decision, 2026-07-03). The host CLI `status`
//!   command additionally surfaces two diagnostic-only raw millivolt
//!   readings: `RspStatusPayload.battery_raw_mv` (added 2026-07-05 for the
//!   ADC-calibration investigation) — the LIVE, rail-contaminated-while-
//!   charging voltage — and `battery_held_raw_mv` (added 2026-07-05,
//!   follow-on) — the last non-charge-inflated ("resting") voltage,
//!   contamination-free even though USB carries both the CLI UART and charge
//!   power on this board (see `battery` module docs). Neither is read by
//!   either of the other two consumers. Percent is also re-anchored
//!   (2026-07-05 follow-on) to a resting-voltage curve rather than the
//!   charging terminal voltage, so a rested-full pack now reads ~100% — see
//!   `battery` module docs' "Full-scale anchor" section.
//! - Single shared source (`battery::BatteryStatus`) wired into all three
//!   consumers so percent/charging always agree: the native telemetry
//!   RESPONSE, the host `status` command
//!   (`RspStatusPayload.battery_percent/battery_charging/battery_raw_mv/battery_held_raw_mv`),
//!   and the admin-menu screen.
//!
//! # Policy layer
//! [`protocol::PolicyFilter`] enforces allowlist policy for every inbound frame:
//! - **Allowlist-only DMs**: unknown senders silently dropped.
//! - **No ADVERT emission**: `PolicyFilter::is_advert_type` guards the TX path.
//! - **Telemetry gating**: GPS replies only to contacts with the telemetry flag.
//! - **No auto-discovery**: contacts never added from the air.

use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    nvs::EspDefaultNvsPartition,
    sys::link_patches,
    log::EspLogger,
};
use esp_idf_hal::{
    gpio::{AnyIOPin, PinDriver, Pull},
    peripherals::Peripherals,
    spi::{SpiDeviceDriver, SpiDriver, SpiDriverConfig, config::Config as SpiConfig},
    uart::{UartDriver, config::Config as UartConfig},
    units::FromValueType,
};
// ADR-0012 (`meshcadet-perf-rearchitecture` M1): the dispatcher ↔ UI queue
// boundary. `SyncSender::try_send`/`Receiver::try_recv` never block (C2) —
// see `send_ui_event` and the dispatcher-loop command-drain site below.
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use protocol::{
    Header, PayloadType, RouteType, PathLen,
    encode_dm_payload, decode_dm_payload, encode_txt_msg_plaintext,
    compute_ack_hash, decode_grp_txt_var, channel_hash_var,
    decode_path_return, PathExtra,
    Identity,
    PolicyFilter,
    is_telemetry_request, encode_telemetry_response, encode_no_fix_response,
    MAX_RESPONSE_LEN,
    parse_telemetry_req, is_telemetry_req, encode_telemetry_response_lpp,
    MAX_TELEMETRY_RESPONSE_LEN,
    packet_dedup_key,
    MAX_LOGIN_PASSWORD_LEN,
};
// Only referenced from `handle_path_return`'s room-login arm, which is
// itself `#[cfg(not(feature = "hil"))]` — a separate `use` (rather than
// folding into the block above) keeps a `hil` build warning-free instead of
// importing a name that build never references.
#[cfg(not(feature = "hil"))]
use protocol::decode_login_response;

#[cfg(not(feature = "hil"))]
use esp_idf_hal::i2c::{I2cDriver, config::Config as I2cConfig};
#[cfg(not(feature = "hil"))]
use esp_idf_hal::ledc::{LedcTimerDriver, config::TimerConfig as LedcTimerConfig};

mod battery;
mod dispatcher;
mod gps;
mod gps_baud_store;
// Persisted self-advert anti-replay timestamp — production builds only
// (mirrors `identity_store`/`config_store`'s HIL gate: HIL never emits an
// advert and never touches NVS for it).
#[cfg(not(feature = "hil"))]
mod advert_ts_store;
// NVS-backed identity is only used by production builds; HIL builds pin a
// fixed seed and never touch NVS, so gate the module out to keep them warning-free.
#[cfg(not(feature = "hil"))]
mod identity_store;
// Config store + provisioning server — production builds only.
#[cfg(not(feature = "hil"))]
mod config_store;
#[cfg(not(feature = "hil"))]
mod provisioning_server;
// Rotating history store (NVS-backed, per-slot write design) — production only.
#[cfg(not(feature = "hil"))]
mod history_store;
// Persisted per-contact inbound replay guard (`handle_dm`'s anti-replay
// gate) — production only, same HIL exemption as `identity_store` /
// `history_store` above (HIL is a bench rig, not exposed to the outsider
// threat model this store defends against).
#[cfg(not(feature = "hil"))]
mod inbound_replay_store;
mod radio;
// Room-server client session — the pure decode/ACK/dedup state machine lives
// in `firmware_core::room_session` (re-exported); this file additionally
// keeps a small dedicated NVS store for session-learned state. Compiled in
// ALL builds (like `gps_baud_store`) — HIL simply never populates a room
// contact, so the store's functions are never called there, but nothing
// about them is NVS-role-restricted the way `config_store`'s provisioning
// blob is.
mod room_session;
mod signal_tracker;
mod ui;
// The UI task (ADR-0012, `meshcadet-perf-rearchitecture` M1) — the ONLY
// module in this crate allowed to `use crate::ui::UiRuntime` (D4.2). This
// file (`run()`, below) can no longer name `UiRuntime` at all; it talks to
// the UI exclusively through the two bounded channels `ui_task::spawn`
// returns. Declared unconditionally (like `ui` above) so it always
// type-checks; `run()` only ever calls `ui_task::spawn` under
// `#[cfg(not(feature = "hil"))]` (HIL rigs have no display — see the
// "Touch UI" bring-up section below).
mod ui_task;
// On-device superloop timing instrumentation (M0 of
// `meshcadet-perf-rearchitecture`) — pure Rust, no ESP-IDF deps, lives in
// `firmware_core::perf` (re-exported). Declared in ALL builds (like
// `signal_tracker` above) so the module always exists; every item inside is
// itself gated on `--features diagnostics` (see that module's doc), so this
// is a no-op without the feature.
mod perf;
// PIN-menu — pure Rust, no ESP-IDF deps; compiled in ALL builds so that
// ui/mod.rs can call pin_menu::verify_pin without a #[cfg] gate.
mod pin_menu;
// On-device admin-menu RuntimeSettings persistence (NVS-backed) — production
// builds only (hil skips NVS entirely, same as config_store).
#[cfg(not(feature = "hil"))]
mod runtime_settings_store;
// History store and admin USB-serial server — production builds only.
// HIL rigs have no display, no NVS history, and no admin laptop.
#[cfg(not(feature = "hil"))]
mod admin_server;
// USB-Serial-JTAG stdout write serialisation — production builds only. Routes
// the ESP-IDF C logger and the binary frame TX through one mutex so log lines
// cannot interleave mid-frame (list-channels corruption fix).
#[cfg(not(feature = "hil"))]
mod serial_console;

/// Real HIL keys, sourced from a GITIGNORED local file (`src/hil_keys.rs`).
///
/// Copy `src/hil_keys.example.rs` → `src/hil_keys.rs` and fill in the REAL
/// values (this MeshCadet node's fixed seed, the paired test node's peer pubkey, and
/// the real channel secret + key length). `src/hil_keys.rs` is git-ignored.
#[cfg(feature = "hil")]
#[path = "hil_keys.rs"]
mod hil_config;

/// NVS-backed rotating message history. Initialised in `run()` after the NVS
/// partition is taken; `handle_dm` appends to it on every received DM.
/// Wrapped in `Mutex<Option<...>>` so it can be a `static` (init is deferred
/// until after peripherals are claimed).
#[cfg(not(feature = "hil"))]
static HISTORY: std::sync::Mutex<Option<history_store::HistoryStore>> =
    std::sync::Mutex::new(None);

/// Latest GPS status snapshot (fix state, coordinates + age, clock-sync state
/// + age). The main thread owns the [`gps::GpsDriver`] and refreshes this
/// static on every dispatcher-loop iteration; `admin_server` (a separate
/// thread — see [`HISTORY`] for the same cross-thread pattern) reads it to
/// answer `QUERY_STATUS` with live GPS fields instead of a boot-time snapshot.
#[cfg(not(feature = "hil"))]
static GPS_STATUS: std::sync::Mutex<gps::GpsStatus> =
    std::sync::Mutex::new(gps::GpsStatus::never());

/// Latest battery status snapshot (charge percentage + charging state). The
/// main thread owns the [`battery::BatteryDriver`] and refreshes this static
/// on every dispatcher-loop iteration; `admin_server` (a separate thread —
/// see [`HISTORY`] for the same cross-thread pattern) reads it to answer
/// `QUERY_STATUS` with a live battery reading instead of a boot-time snapshot.
#[cfg(not(feature = "hil"))]
static BATTERY_STATUS: std::sync::Mutex<battery::BatteryStatus> =
    std::sync::Mutex::new(battery::BatteryStatus::unknown());

/// Latest room wall-clock provenance (`meshcadet-room-adopt-server-time`):
/// `None` (no trusted wall clock at all), `Gps`, or `RoomServer` (adopted
/// from a room server's own clock while GPS has none —
/// `room_session::adopt_server_clock`/`trusted_wall_clock_secs`). The main
/// thread refreshes this on every dispatcher-loop iteration, same
/// cross-thread pattern as [`GPS_STATUS`]; `meshcadet-room-clock-ux` is the
/// consumer that surfaces it (GPS status screen — "why does this say no fix
/// but the time is right?").
#[cfg(not(feature = "hil"))]
static ROOM_CLOCK_SOURCE: std::sync::Mutex<room_session::ClockSource> =
    std::sync::Mutex::new(room_session::ClockSource::None);

use battery::BatteryDriver;
use dispatcher::{
    AirtimeBudget, DuplicateFilter, OutstandingKind, OutstandingSend,
    OutstandingSends, TxQueue, lora_airtime_ms, tx_guard_allows,
};
use gps::GpsDriver;
use radio::Radio;
use signal_tracker::{SignalConfig, SignalLevel, SignalTracker};

// ── RX diagnostic log macro ───────────────────────────────────────────────────

#[cfg(feature = "hil")]
macro_rules! rx_diag {
    ($($arg:tt)*) => { log::info!($($arg)*) }
}
#[cfg(not(feature = "hil"))]
macro_rules! rx_diag {
    ($($arg:tt)*) => { log::debug!($($arg)*) }
}

// DEFECT FIX (`meshcadet-grptxt-rx-open-on-published-test-channel-secret`):
// this file used to define a compiled-in `HIL_TEST_CHANNEL_SECRET` constant
// (`[0x6d; 32]`, published in this repo and named as a dummy key in
// SECURITY.md) and fall back to it whenever no channel was provisioned —
// so a contacts-only device still computed a real, on-air channel hash
// under a secret any outsider could compute from the public source, and
// both transmitted and accepted GRP_TXT on it. The fallback is gone;
// `ProvisionedConfig::resolve_channel_secret` returns `None` instead, and
// every TX/RX call site treats `None` as "no channel — GRP_TXT
// unreachable", not "substitute a placeholder secret". There is
// deliberately no replacement constant here: removing it (rather than
// randomizing it) is what closes the defect class instead of just hiding it.

#[cfg(feature = "hil")]
const TX_INTERVAL_MS: u64 = 30_000;

// ── Pending outbound ACK ──────────────────────────────────────────────────────
//
// DM and room-post delivery tracking (the two ACKed send paths) now lives in
// `dispatcher::OutstandingSends` — a fixed-size table keyed by wire ACK hash,
// replacing what used to be two single-slot trackers here: a bare
// `PendingAck { hash, to_hash }` for the DM path, and
// `RoomRuntime::pending_post_ack: Option<[u8; 4]>` for the room-post path.
// Both assumed only ONE outstanding send of their kind at a time (the
// invariant `firmware_core::ui::mark_last_unacked_outbound`'s doc used to
// document) — replaced so two DMs (or a DM and a room post) in flight
// concurrently each track and resolve independently. See
// `dispatcher::OutstandingSends`'s own doc for the table and
// `main.rs::match_outstanding_ack` for the one shared ACK-resolution call
// site (bare `Ack` datagram AND PATH-return-bundled `PathExtra::Ack` both
// call it — see that function's doc for why that matters).

// ── Pending outbound channel (GRP_TXT) ack ────────────────────────────────────

/// The dedup key of our own most-recently-sent channel message, together with
/// the channel it was sent on — awaiting the first heard repeat.
///
/// A broadcast/GRP_TXT message has no per-recipient delivery ACK on the wire,
/// so it is treated as delivered once the device hears its OWN transmission
/// repeated back into the mesh by another node. `protocol::packet_dedup_key`
/// already gives every
/// flood-relayed copy of one logical packet the same key (path/hop bytes
/// excluded — see `dispatcher.rs`'s module doc), and the dispatcher already
/// marks our own transmission as seen (`dedup.insert(&tx_frame[..n])`) so a
/// relay flooding it back is dropped as a duplicate rather than displayed —
/// this struct reuses that exact key to recognise WHICH duplicate was our own
/// pending send, rather than adding a second, parallel tracker.
///
/// Single-slot — only the most recently sent channel message's ack is ever
/// recognised live, and it is NEVER auto-retried: there is no wire ACK to
/// correlate a retry against (only an implicit "heard our own repeat"
/// signal), so a channel send never enters `dispatcher::OutstandingSends`
/// (`meshcadet-dm-room-send-auto-retry`'s auto-retry sweep only ever walks
/// that table) and this stays a single slot unlike the DM/room-post paths.
struct PendingChannelAck {
    hash: [u8; 4],
    channel_hash: u8,
}

// ── Room contact runtime state ────────────────────────────────────────────────

/// In-memory runtime state for one provisioned room contact — built once at
/// boot from the loaded `ProvisionedConfig` (production builds only; see
/// `run()`'s provisioning-gate arm) and owned by the main dispatcher loop for
/// the rest of `run()`.
///
/// This is deliberately NOT feature-gated (unlike `config_store`/`RoomExtra`
/// themselves): `on_receive`/`handle_path_return` reference this type
/// unconditionally so their signatures don't fork between `hil` and
/// production builds — a `hil` build simply never populates the `Vec`, so
/// every loop over it is a no-op there, exactly like `gps_baud_store`'s
/// "compiled everywhere, meaningfully used only in production" shape.
struct RoomRuntime {
    /// The room server's full Ed25519 public key (ECDH partner for every
    /// login/push/ACK exchange).
    pubkey: [u8; 32],
    /// The room's 1-byte routing hash (`pubkey[0]`) — this session's
    /// conversation key into `HISTORY`/the UI's `messages` map, matching
    /// every other DM/channel conversation's own hash-keying convention.
    hash: u8,
    /// This boot's guest password, from the provisioning-time `RoomExtra`
    /// seed — not re-read from `config_store` afterward (see
    /// `room_session.rs`'s module doc for why the main thread has no safe
    /// handle back into the moved `ProvisionedConfig`).
    guest_password: [u8; MAX_LOGIN_PASSWORD_LEN],
    guest_password_len: u8,
    /// Session-learned state — persisted permission/out_path/sync_since, and
    /// the boot-time resume point [`room_session::load_room_session`] loaded
    /// (falling back to the provisioning-time `RoomExtra` seed if nothing had
    /// been persisted yet).
    session: room_session::PersistedRoomSession,
    /// This room's dedicated session-store erase epoch, as it stood at boot
    /// ([`room_session::load_room_epoch`]) — FINDING G. Every
    /// [`room_session::save_room_session`] call for this room passes this
    /// value; that function re-reads the CURRENT epoch immediately before
    /// writing and refuses the write if `admin_server`'s `ADD_ROOM`/
    /// `DEL_ROOM` erased this room's store since this boot bumped it past
    /// what's remembered here. This field is intentionally NEVER updated
    /// after construction — a live `RoomRuntime` has no safe way to learn
    /// its erase was legitimate rather than stale (that's the whole
    /// "no cross-thread channel" limit this mechanism works around), so it
    /// stays pinned to its boot-time value until the next reboot rebuilds
    /// this `Vec` from scratch.
    #[cfg_attr(feature = "hil", allow(dead_code))]
    session_epoch: u8,
    /// Whether this boot has already enqueued the initial flood login for
    /// this room. A re-login can still happen later (Phase C: the
    /// keep-alive scheduler re-floods if `out_path` is ever lost), but the
    /// one-shot BOOT login only ever fires once.
    login_sent: bool,
    /// Wall-clock ms (`uptime_ms()`-scale, matching every other `last_*_ms`
    /// scheduler in this loop) this room last sent a keep-alive — `0`
    /// initially, doubling as the scheduler's "never ticked yet" sentinel:
    /// `room_session::room_keep_alive_interval_ms` reads this exact `== 0`
    /// check to gate the FIRST tick on `ROOM_FIRST_KEEP_ALIVE_DELAY_MS`
    /// instead of the routine `ROOM_KEEP_ALIVE_INTERVAL_MS` (see that
    /// function's doc for the F2 defect this fixes — gating the first tick
    /// on the full routine cadence stranded every fresh boot's Phase D drain
    /// window open for up to 5 minutes). See `ROOM_KEEP_ALIVE_INTERVAL_MS`'s
    /// doc for the routine cadence's own airtime/liveness justification.
    #[cfg_attr(feature = "hil", allow(dead_code))]
    last_keep_alive_ms: u64,
    /// Wall-clock ms this room last sent a RE-FLOOD login attempt via the
    /// `!session.has_route()` branch — `0` initially, doubling as that branch's
    /// own "never yet" sentinel, exactly like `last_keep_alive_ms`'s.
    /// `meshcadet-room-reflood-login-backoff`'s fix (FINDING B): kept
    /// deliberately SEPARATE from `last_keep_alive_ms` so the reflood
    /// cadence can never again be silently re-coupled to the route-direct/
    /// drain cadence — see `room_session::room_reflood_interval_ms`'s doc.
    #[cfg_attr(feature = "hil", allow(dead_code))]
    last_reflood_ms: u64,
    /// Consecutive re-flood-login attempts sent since the last proof this
    /// session is live — a successful login reply
    /// (`apply_room_login_outcome`) or an inbound push
    /// (`handle_room_push_frame`) both reset it to `0`, mirroring
    /// `keep_alive_stall`'s own reset conditions. Feeds
    /// `room_session::room_reflood_interval_ms`'s exponential backoff.
    #[cfg_attr(feature = "hil", allow(dead_code))]
    reflood_attempts: u32,
    /// Phase D's session-phase notification classifier — see
    /// `firmware_core::room_session::RoomSyncPhase`'s doc. Starts assuming a
    /// drain is needed (every fresh boot does, until a keep-alive ACK proves
    /// otherwise).
    #[cfg_attr(feature = "hil", allow(dead_code))]
    sync_phase: room_session::RoomSyncPhase,
    /// The ACK hash expected back for this room's one in-flight keep-alive,
    /// if any. `None` when no keep-alive is outstanding.
    #[cfg_attr(feature = "hil", allow(dead_code))]
    pending_keep_alive_ack: Option<[u8; 4]>,
    /// Reconnect-stall detector — see
    /// `firmware_core::room_session::RoomKeepAliveStall`'s doc. Reset via
    /// `.reset()` on any successful keep-alive ACK (`handle_ack`), an
    /// inbound post (`handle_room_push_frame`), or a fresh login
    /// (`apply_room_login_outcome`) — mirroring the server's own
    /// `push_failures` reset conditions.
    #[cfg_attr(feature = "hil", allow(dead_code))]
    keep_alive_stall: room_session::RoomKeepAliveStall,
    /// Set whenever a login reply (fresh boot login OR a stall-triggered
    /// re-flood) applies a new session — consumed by the NEXT keep-alive
    /// tick, which passes `session.sync_since` as that keep-alive's
    /// `force_since` instead of the routine `0`, explicitly re-affirming the
    /// server's view of this client's sync watermark rather than relying
    /// solely on the login reply itself. Cleared once that keep-alive is
    /// sent. See this mission's "resumed keep-alive" fix bullet.
    #[cfg_attr(feature = "hil", allow(dead_code))]
    resync_pending: bool,
    /// A small in-memory tail of this room's already-known conversation
    /// entries — the content-dedup input
    /// `firmware_core::room_session::handle_room_push` compares an inbound
    /// push against (see that function's doc: a room-server retry changes
    /// the wire frame's ciphertext but not the logical post, so dedup must
    /// be content-level, not the radio's frame-level dedup ring). Seeded
    /// from flash at boot (history hydrate, alongside `ui.seed_conversation`)
    /// and appended to live, capped at [`ROOM_RECENT_CAP`].
    ///
    /// Every reader/writer of this field lives behind
    /// `#[cfg(not(feature = "hil"))]` (there are no rooms under `hil`), so a
    /// `hil` build never touches it at all — `allow(dead_code)` there is
    /// genuinely dead in that profile, not a mistake.
    #[cfg_attr(feature = "hil", allow(dead_code))]
    recent: Vec<protocol::history::HistoryEntry>,
}

/// Cap on [`RoomRuntime::recent`]'s length — a room server's own cyclic push
/// queue (`protocol::MAX_UNSYNCED_POSTS`, 32) is the deepest a retry could
/// ever reach back into, but M1's dedup only needs to catch a retry of the
/// single most-recently-unacked post, so a much smaller cap keeps this
/// bounded without ever meaningfully weakening the dedup.
#[cfg(not(feature = "hil"))]
const ROOM_RECENT_CAP: usize = 8;

/// Phase C keep-alive cadence: 5 minutes (300_000 ms).
///
/// **Airtime**: a keep-alive frame is tiny — a 9-byte plaintext wrapped in a
/// DM envelope, well under 40 bytes on the wire — sent route-direct (one
/// unicast hop-count, not a flood rebroadcast every node repeats). At 12
/// sends/hour it is over an order of magnitude below this device's own
/// `TX_INTERVAL_MS` (30 s advert cadence, a much larger flood frame) and
/// negligible against any reasonable LoRa duty-cycle budget.
///
/// **Liveness**: `meshcadet-room-m1-checkpoint`'s Findings pin the failure
/// mode this exists to catch — three consecutive push-ACK timeouts evict a
/// client from the server's push list UNTIL REBOOT, and `push_failures`
/// only resets on an ACK, an inbound post, an inbound REQ (this keep-alive),
/// or a fresh login. A push timeout is 12 s (flood) or up to ~4+2×(hops+1) s
/// (direct); three in a row is on the order of tens of seconds to a minute.
/// 5 minutes is short enough that a silently-decayed `out_path` (this
/// scheduler's OTHER job: re-flooding the login the moment `out_path_len`
/// reads 0) or a wedged push list gets a recovery attempt well within the
/// timescale a human would notice a room "went quiet" and long before
/// they'd consider power-cycling the device, while staying far enough above
/// the server's own per-push timeouts that this isn't just adding to the
/// same airtime pressure that caused the stall in the first place.
#[cfg(not(feature = "hil"))]
const ROOM_KEEP_ALIVE_INTERVAL_MS: u64 = 300_000;

/// F2 fix (this mission's Objective): the FIRST post-login keep-alive tick
/// must not be gated on the full [`ROOM_KEEP_ALIVE_INTERVAL_MS`] the way
/// `RoomRuntime::last_keep_alive_ms`'s `0` sentinel naively did against a
/// same-scale `now` (`uptime_ms()`) — that left every fresh boot's Phase D
/// drain window (`firmware_core::room_session::RoomSyncPhase`) unable to
/// close for up to 5 minutes, no matter how quickly the actual backlog
/// drained. 10 s is short enough that a user notices no meaningful lag, but
/// long enough to give the boot-time flood login (`room_session::
/// encode_room_login_frame`) a realistic chance to route-return before this
/// tick's re-flood branch (`!session.has_route()`) would otherwise
/// re-flood it again.
#[cfg(not(feature = "hil"))]
const ROOM_FIRST_KEEP_ALIVE_DELAY_MS: u64 = 10_000;

/// F2 fix: cadence used while `RoomSyncPhase::is_draining()` is still true —
/// far tighter than the routine [`ROOM_KEEP_ALIVE_INTERVAL_MS`], since a
/// keep-alive ACK is the ONLY thing that can ever close that drain window
/// (`RoomSyncPhase::on_keep_alive_ack`'s doc). Once the window closes the
/// scheduler falls back to the routine cadence — see
/// `room_session::room_keep_alive_interval_ms`.
#[cfg(not(feature = "hil"))]
const ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS: u64 = 15_000;

/// `meshcadet-room-reflood-login-backoff` fix (FINDING B): the
/// `!session.has_route()` re-flood-login branch's OWN cadence — deliberately
/// NOT [`ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS`]. That constant gates the
/// route-direct keep-alive/drain-window poll only; letting the reflood
/// branch share it meant a room whose server never answers (offline,
/// out-of-range, decommissioned) re-flooded a full `ANON_REQ` login every
/// 15 s FOREVER, since [`firmware_core::room_session::RoomSyncPhase`]'s
/// drain window — the only thing [`ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS`]
/// exists to poll fast for — never closes against a server that never ACKs.
/// See `firmware_core::room_session::room_reflood_interval_ms`'s doc for the
/// full airtime/regulatory-duty-cycle rationale this fixes.
///
/// Starting wait for the first reflood attempt of a backoff epoch (an epoch
/// resets — `RoomRuntime::reflood_attempts` back to `0` — the moment a login
/// reply or inbound push proves the session live again).
#[cfg(not(feature = "hil"))]
const ROOM_REFLOOD_INITIAL_BACKOFF_MS: u64 = 30_000;

/// Ceiling the reflood backoff exponentially climbs to and then holds at.
/// Deliberately equal to [`ROOM_KEEP_ALIVE_INTERVAL_MS`] (the fix's own
/// "at or above the routine 300s" requirement, satisfied at the boundary):
/// a permanently-dead room server is never re-flooded more often than a
/// routine keep-alive would have polled it anyway.
#[cfg(not(feature = "hil"))]
const ROOM_REFLOOD_BACKOFF_CEILING_MS: u64 = ROOM_KEEP_ALIVE_INTERVAL_MS;

// A stall can only ever be DETECTED (and `out_path_len` zeroed) after at
// least `ROOM_FIRST_KEEP_ALIVE_DELAY_MS` plus `KEEP_ALIVE_STALL_THRESHOLD`
// draining-cadence ticks of uptime have elapsed — this is the floor the
// scheduler loop's post-invalidation reflood relies on: since that floor is
// always ABOVE `ROOM_REFLOOD_INITIAL_BACKOFF_MS`, the reflood branch's own
// `now.saturating_sub(room.last_reflood_ms) < interval` gate is already
// satisfied (given `last_reflood_ms` is at most that same age) the moment
// invalidation happens, without the scheduler needing to special-case a
// "just invalidated" reset — the very next scheduler pass re-floods
// promptly, same as the pre-fix same-tick fallthrough did. If this ever
// fires, that numeric relationship broke and the "prompt post-invalidation
// reflood" property above needs re-deriving (or the scheduler needs an
// explicit reset), not just a constant bumped past it.
#[cfg(not(feature = "hil"))]
const _: () = assert!(
    ROOM_FIRST_KEEP_ALIVE_DELAY_MS
        + room_session::KEEP_ALIVE_STALL_THRESHOLD as u64 * ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS
        > ROOM_REFLOOD_INITIAL_BACKOFF_MS
);

// `DRAIN_SUPPRESSION_CEILING_MS`'s size is justified against THIS cadence
// (see that constant's doc: 4x the draining keep-alive interval, so three
// consecutive missed round-trips still fall inside the ceiling and a
// genuinely progressing drain is never truncated by it in practice). The
// constant lives in `firmware_core`, the cadence lives here, and neither
// crate can see the other's rationale — so pin the relationship itself. If
// this fires, either re-derive the ceiling for the new cadence or accept
// that a healthy drain will now sometimes be force-flushed mid-backlog;
// don't just bump a number past the assertion.
#[cfg(not(feature = "hil"))]
const _: () = assert!(
    room_session::DRAIN_SUPPRESSION_CEILING_MS >= 3 * ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS
        && room_session::DRAIN_SUPPRESSION_CEILING_MS
            < room_session::DRAIN_WINDOW_STALL_TIMEOUT_MS
);

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    link_patches();
    EspLogger::initialize_default();

    // Serialise the ESP-IDF C logger against the binary frame TX: both share the
    // USB-Serial-JTAG stdout, and without one lock a radio/UI-thread log line can
    // interleave mid-frame and corrupt the host's frame parse (list-channels
    // "no channels configured" defect). Install before any frame-TX thread spawns.
    // Production builds only — HIL rigs have no host frame protocol.
    #[cfg(not(feature = "hil"))]
    serial_console::install();

    log::info!("meshcadet firmware — radio+identity+policy+GPS telemetry bring-up");
    // Authoritative build identity for the flashed Rust app (firmware git
    // describe, refreshed every incremental build by build.rs). Use THIS line —
    // not the esp-idf "App version" boot tag — to confirm `cargo run` landed the
    // latest build: the esp-idf tag is generated by esp-idf-sys's CMake and can
    // lag on incremental runs.
    log::info!("firmware build: {}", env!("MESHCADET_BUILD_VERSION"));

    if let Err(e) = run() {
        log::error!("fatal error in run(): {:?}", e);
        unsafe { esp_idf_svc::sys::esp_restart() };
    }
}

/// Post `event` to the UI task, never blocking (ADR-0012 C2).
///
/// `SyncSender::try_send` degrades rather than stalls: on a full queue (the
/// UI task fell behind — capacity 32 against ≲2 events/iteration production
/// makes this a safety valve, not a design path) OR a disconnected one (the
/// UI task's `Receiver<UiEvent>` was dropped — either it never spawned at
/// all, e.g. HIL or a headless boot, or it exited after a construction
/// failure; see `ui_task`'s module doc), the event is dropped and
/// `dropped` is incremented. The dispatcher NEVER waits on the UI — a
/// missed CAD window or a late RX drain is a priority-1 violation this
/// boundary exists specifically to make unreachable. Callers accumulate
/// `dropped` into the existing periodic (`RX_STATS_INTERVAL_MS`) stats log
/// rather than logging per-occurrence.
fn send_ui_event(evt_tx: &SyncSender<ui::UiEvent>, dropped: &mut u32, event: ui::UiEvent) {
    // The drop-and-count policy itself is `firmware_core::ui::ui_task_
    // boundary::send_or_count` — a generic, host-tested function over the
    // real `std::sync::mpsc::SyncSender<T>` this crate also uses. This
    // wrapper exists only to fix the type to `ui::UiEvent` at every call
    // site, matching `main.rs`'s existing convention of thin
    // `firmware_core`-delegating wrappers.
    firmware_core::ui::ui_task_boundary::send_or_count(evt_tx, event, dropped);
}

fn run() -> anyhow::Result<()> {
    // 1.5. USB-Serial-JTAG interrupt-driven RX driver (production builds only).
    //
    // With CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y, ESP-IDF startup wires VFS
    // output via the polling path but leaves INPUT unconfigured — all stdin
    // reads return EAGAIN (os error 11) forever without the driver.
    //
    // `usb_serial_jtag_driver_install` attaches an ISR-backed ring buffer;
    // `esp_vfs_usb_serial_jtag_use_driver` switches VFS from polling to
    // driver-backed I/O.  After this, stdin reads block until host bytes
    // arrive rather than returning EAGAIN immediately.
    //
    // Blocking is correct: provisioning_server and admin_server each run
    // on their own threads, so blocking in read() does not stall main.
    //
    // Must run before any thread that reads stdin (prov_server spawned at
    // the unprovisioned gate below; admin_server at step 2.7).
    #[cfg(not(feature = "hil"))]
    {
        let mut usj_cfg = esp_idf_svc::sys::usb_serial_jtag_driver_config_t {
            tx_buffer_size: 256,
            rx_buffer_size: 512,
        };
        let ret = unsafe {
            esp_idf_svc::sys::usb_serial_jtag_driver_install(
                &mut usj_cfg as *mut _,
            )
        };
        if ret == 0 {
            unsafe { esp_idf_svc::sys::esp_vfs_usb_serial_jtag_use_driver() };
            // Disable CR→LF translation on the VFS RX path so that binary
            // 0x0D/0x0A bytes in provisioning frames are not mangled.
            // Belt-and-suspenders: provisioning_server and admin_server now
            // read via usb_serial_jtag_read_bytes (bypassing VFS entirely),
            // but this guards any remaining VFS reader.
            unsafe {
                esp_idf_svc::sys::esp_vfs_dev_usb_serial_jtag_set_rx_line_endings(
                    esp_idf_svc::sys::esp_line_endings_t_ESP_LINE_ENDINGS_LF,
                );
            }
            // Disable LF→CRLF translation on the VFS TX path. admin_server
            // writes provisioning response frames via std::io::stdout(), which
            // routes through this VFS console. The default LF→CRLF expansion
            // inserts a 0x0D before every 0x0A byte — corrupting any frame
            // whose header (e.g. an RSP_CHANNEL length byte of 0x0A == 10) or
            // payload contains 0x0A. The host then reads a bad length, fails to
            // parse, and drops the entire channel enumeration ("no channels
            // configured"). Force raw LF so transmitted bytes are verbatim.
            unsafe {
                esp_idf_svc::sys::esp_vfs_dev_usb_serial_jtag_set_tx_line_endings(
                    esp_idf_svc::sys::esp_line_endings_t_ESP_LINE_ENDINGS_LF,
                );
            }
            log::info!(
                "USB-Serial-JTAG driver installed — raw-binary RX enabled (512B ring buffer)"
            );
        } else {
            log::warn!(
                "usb_serial_jtag_driver_install failed (0x{:08x}) — \
                 stdin reads will return EAGAIN; provisioning will not work",
                ret
            );
        }
    }

    let peripherals = Peripherals::take()?;
    let _sysloop = EspSystemEventLoop::take()?;

    // 2. Load identity
    let nvs_partition = EspDefaultNvsPartition::take()?;

    #[cfg(feature = "hil")]
    let identity = {
        let _ = &nvs_partition;
        log::info!("identity: HIL build — fixed compiled seed");
        Identity::from_seed(hil_config::HIL_SELF_SEED)
    };
    #[cfg(not(feature = "hil"))]
    let identity = identity_store::load_or_generate(nvs_partition.clone())?;

    log::info!(
        "identity ready: pub_hash=0x{:02x}, pubkey={}",
        identity.pub_hash(),
        hex_full(&identity.pubkey),
    );

    // 2.5. Policy filter
    let mut policy = PolicyFilter::new();

    // Pubkey-hash (`Contact::pub_hash`, i.e. `pubkey[0]`) → display name, for
    // every provisioned contact — including room-server contacts and the
    // OTHER room members a room push's `author_pubkey_prefix` might identify
    // (see `handle_room_push_frame`'s use below). Populated alongside
    // `policy` in the contact-provisioning loop and kept in this scope
    // (unlike `provisioned_config`, which is moved into the admin_server
    // thread) so the dispatch loop can still resolve a room post's sender
    // name after that move. Deliberately a lookup table snapshot, not a live
    // handle back into `provisioned_config` — a runtime ADD_ROOM/DEL_ROOM
    // contact edit (admin/provisioning server) won't retroactively update an
    // already-decoded sender name, matching how `policy` itself is already a
    // boot-time snapshot with the same limitation.
    let mut contact_display_names: std::collections::HashMap<u8, String> =
        std::collections::HashMap::new();

    // 4. Board peripheral power-enable (BOARD_POWERON = GPIO10).
    //    Must be HIGH before any SPI/UART peripheral traffic, including the
    //    display — moved before the provisioning gate so the display is
    //    initialised on unprovisioned first boot (§A acceptance).
    let mut board_power = PinDriver::output(peripherals.pins.gpio10)?;
    board_power.set_high()?;
    esp_idf_hal::delay::FreeRtos::delay_ms(100); // rail + TCXO settle
    let _board_power = board_power; // hold HIGH for program lifetime

    // 5a. SPI2 bus driver — shared between radio (CS=GPIO9) and LCD (CS=GPIO12).
    //     Declared here (before the display init below) so its lifetime covers
    //     both the LCD SpiDeviceDriver and the radio SpiDeviceDriver
    //     (registered immediately below, step 5b).
    //
    // ADR-0012 D5: `Box::leak`'d to `'static` — post-split, the LCD's
    // SpiDeviceDriver is moved into `ui_task` (a separate, core-1-pinned
    // OS thread), so it can no longer borrow a `run()` stack local. The
    // leaked bus is never dropped (`run()` never returns; `spi_bus_free`
    // was never reachable pre-split either), immutable after construction
    // as far as `Borrow` consumers are concerned, and every mutating SPI
    // operation goes through `&mut SpiDeviceDriver` with each device
    // exclusively owned by one task (D2) — the three conditions the ADR
    // cites for this being sound. `SpiDriver<'static>: Sync` auto-derives
    // (D5's source-level chain through `esp-idf-hal`/`embassy-sync`), so
    // `&'static SpiDriver<'static>: Send`, so
    // `SpiDeviceDriver<'static, &'static SpiDriver<'static>>: Send` — CI's
    // `xtensa-esp32s3-espidf` cross-compile is the oracle for this (this
    // container has no `esp` toolchain); D5's pre-authorised `SpiBus`
    // newtype fallback exists if CI disagrees.
    let spi_driver: &'static SpiDriver<'static> = Box::leak(Box::new(SpiDriver::new(
        peripherals.spi2,
        peripherals.pins.gpio40,          // SCK
        peripherals.pins.gpio41,          // MOSI
        Some(peripherals.pins.gpio38),    // MISO
        &SpiDriverConfig::new(),
    )?));

    // 5b. Radio SpiDeviceDriver — REGISTRATION only (`spi_bus_add_device`),
    // moved up beside the LCD's (ADR-0012 D2's corollary: every
    // `spi_bus_add_device` call happens on this task, before `ui_task`
    // exists, so device registration stays strictly sequential and
    // single-threaded exactly as pre-split — only SPI *transactions* ever
    // become concurrent). `Radio::init`'s actual chip bring-up (the RST/
    // BUSY/DIO1 pins + the SPI transactions themselves) stays at its
    // original later call site, after the provisioning gate — this device
    // handle is simply held until then.
    let spi_device = SpiDeviceDriver::new(
        spi_driver,
        Some(peripherals.pins.gpio9),      // CS
        &SpiConfig::new().baudrate(8u32.MHz().into()),
    )?;

    // 7. Touch UI — display + touch + notification runtime (production only).
    //
    // The T-Deck Plus SPI bus (GPIO40/41/38) is shared between the SX1262 radio
    // (CS=GPIO9) and the ST7789 LCD (CS=GPIO12). Both devices use SPI2 via
    // borrowed SpiDeviceDriver<'_, &SpiDriver<'_>>; only one CS is ever
    // asserted at a time. Post-split (ADR-0012 D2/D10), the radio device is
    // touched from this task and the LCD device from `ui_task` — two
    // different tasks on two different cores — so that serialisation is no
    // longer "single-task loop" software serialisation; it comes from
    // ESP-IDF's `spi_bus_lock` per-bus arbitration, which the driver applies
    // to every device registered on a bus regardless of which task/core
    // issues the transaction (see docs/perf/spi2-arbitration-r1.md Q1).
    //
    // Touch IC: GT911, I2C1, SDA=GPIO18, SCL=GPIO8.
    // Buzzer: onboard I2S speaker, WS=GPIO5 / BCK=GPIO7 / DOUT=GPIO6 (I2S0).
    // CORRECTION: earlier revisions of
    // this comment claimed a passive piezo on GPIO46 driven via LEDC PWM.
    // That hardware does not exist on the T-Deck / T-Deck Plus — GPIO46 is
    // the keyboard co-processor's interrupt line (`BOARD_KEYBOARD_INT` per
    // LilyGo's own `utilities.h`), not a buzzer. The board's actual — and
    // only — audio-output path is the I2S peripheral driving the onboard
    // speaker; see `ui/mod.rs`'s "Buzzer" module doc for the corroborating
    // sources (LilyGo's own `SimpleTone.ino`, the upstream MeshCore firmware,
    // and the shipped MCTerm companion firmware).
    //
    // MOVED BEFORE the provisioning gate (step 2.6) so the §A wordmark+pubkey
    // screen renders while awaiting USB provisioning on unprovisioned first boot.
    //
    // Provisioning state is checked once here; the result is reused by the gate
    // below to avoid a second NVS read (EspError: Copy so Result is Copy).
    #[cfg(not(feature = "hil"))]
    let prov_result = config_store::is_provisioned(nvs_partition.clone());

    // ADR-0012: `ui_task` (the whole touch UI — display, touch, keyboard,
    // trackball, buzzer, and every Slint call) now runs on its OWN
    // core-1-pinned task, spawned by `ui_task::spawn` below. This task
    // (`main.rs::run()`) can no longer name `ui::UiRuntime` at all (D4.2) —
    // it talks to the UI exclusively through `evt_tx`/`cmd_rx`, the two
    // bounded channels `ui_task::spawn` returns (D3). Every raw peripheral
    // `ui_task` will own is still constructed HERE, on this task, before the
    // spawn (SPI/I2C device REGISTRATION must stay single-threaded — D2's
    // corollary); only the actual display/touch bring-up (fallible
    // transactions, not registration) and `UiRuntime::new()` itself move
    // into the spawned thread — see `ui_task`'s module doc for the full
    // headless-fallback contract this preserves.
    #[cfg(not(feature = "hil"))]
    let (evt_tx, cmd_rx): (SyncSender<ui::UiEvent>, Receiver<ui::UiCommand>) = {
        // ── I2C1 for GT911 capacitive touch ─────────────────────────────────
        // Bus clock = 100 kHz (standard mode), NOT 400 kHz fast mode.
        //
        // This bus is SHARED by the GT911 touch IC (0x5D) and the ESP32-C3
        // keyboard co-processor (0x55).  The GT911 is a hardware IC rated for
        // fast mode and ACKs fine at 400 kHz — so "touch works at 400 kHz" does
        // NOT clear the keyboard.  The C3 keyboard is a firmware I2C *slave*
        // (LilyGo's keyboard firmware) and LilyGo's own reference brings the
        // bus up at the Wire default (100 kHz) — it is only proven at standard
        // mode.  A 400 kHz clock the C3 slave cannot service presents host-side
        // as ESP_ERR_TIMEOUT / no-ACK at 0x55 while touch keeps working: the
        // exact reported symptom.  Standard mode is within GT911 spec, so the
        // only cost is slightly slower touch transactions (sub-ms, imperceptible
        // in the cooperative UI loop).
        let i2c1_result = I2cDriver::new(
            peripherals.i2c1,
            peripherals.pins.gpio18,        // SDA
            peripherals.pins.gpio8,         // SCL
            &I2cConfig::new().baudrate(100_000u32.Hz()),
        );

        // ── LCD SPI device — shares SPI2 (same bus as radio, CS=GPIO12) ─────
        // Registration only (like the radio's `spi_device` above); the
        // fallible display BRING-UP transactions run on `ui_task` itself.
        let lcd_spi_result = SpiDeviceDriver::new(
            spi_driver,
            Some(peripherals.pins.gpio12), // LCD CS
            &SpiConfig::new().baudrate(40u32.MHz().into()),
        );

        let dc  = PinDriver::output(peripherals.pins.gpio11)?; // LCD DC
        let lcd_rst = PinDriver::output(peripherals.pins.gpio16)?; // LCD RST
        // Backlight: LEDC PWM on GPIO42 (channel1 / timer1 / 2 kHz / 10-bit / 100% duty).
        // A plain GPIO set_high() does NOT activate the T-Deck Plus backlight: the
        // boost converter on GPIO42 needs a PWM switching signal, not static DC.
        // Channel1 / timer1 are reserved for the backlight; LEDC channel0/timer0
        // are unused by this firmware (the buzzer is I2S, not LEDC — see above).
        let bl_timer = LedcTimerDriver::new(
            peripherals.ledc.timer1,
            &LedcTimerConfig::new()
                .frequency(2_000u32.Hz())
                .resolution(esp_idf_hal::ledc::config::Resolution::Bits10),
        )?;

        // ── I2S buzzer — onboard speaker (WS=GPIO5, BCK=GPIO7, DOUT=GPIO6) ──
        // Independent of the touch/display bring-up below; a failure here
        // degrades to visual-only notifications rather than failing UI init
        // entirely (same graceful-degradation pattern as the keyboard probe).
        let buzzer = match ui::BuzzerDriver::new(
            peripherals.i2s0,
            peripherals.pins.gpio7, // BCK
            peripherals.pins.gpio5, // WS (LRCK)
            peripherals.pins.gpio6, // DOUT
        ) {
            Ok(b) => Some(b),
            Err(e) => {
                log::warn!(
                    "I2S buzzer init failed: {:?} — notifications will be visual-only",
                    e,
                );
                None
            }
        };

        // ── Trackball — roll (Up=GPIO3/Down=GPIO15/Left=GPIO1/Right=GPIO2) +
        // center click (GPIO0) — a PARALLEL input modality alongside touch and
        // the physical keyboard. None of
        // these five GPIOs are claimed anywhere else in this firmware's pin
        // budget (see `ui::trackball` module doc for the full feasibility
        // check). Independent of the touch/display bring-up below, same
        // graceful-degradation pattern as the buzzer/keyboard probes: a
        // failure here degrades to touch+keyboard-only, not a headless boot.
        let trackball = match ui::trackball::TrackballDriver::new(
            peripherals.pins.gpio3,  // Up
            peripherals.pins.gpio15, // Down
            peripherals.pins.gpio1,  // Left
            peripherals.pins.gpio2,  // Right
            peripherals.pins.gpio0,  // Click
        ) {
            Ok(t) => {
                log::info!(
                    "trackball initialised — Up=GPIO3 Down=GPIO15 Left=GPIO1 Right=GPIO2 Click=GPIO0"
                );
                Some(t)
            }
            Err(e) => {
                log::warn!(
                    "trackball init failed: {:?} — navigation stays touch/keyboard-only",
                    e,
                );
                None
            }
        };

        // Use the already-queried provisioning state (reused by step 2.6
        // below — avoids a second NVS read).
        let provisioned = prov_result.unwrap_or(false);
        let pubkey_str = format!("{}", hex_full(&identity.pubkey));
        // Self-name for @mention wrap (send) / self-tier highlight (receive)
        // — see UiRuntime::self_name's doc. Read once here at UI
        // construction, same as the channel-send path's per-send live read
        // (device_sender_name); a name change made after boot takes effect
        // on the next reboot for the UI copy (acceptable — mentions are a
        // display/typing aid, not a wire-correctness concern).
        let self_name = device_sender_name(&identity, nvs_partition.clone());

        // Bundled behind one `Box` — not passed as separate by-value
        // arguments — see `ui_task::UiHardware`'s doc for why.
        ui_task::spawn(
            Box::new(ui_task::UiHardware {
                i2c1: i2c1_result,
                lcd_spi: lcd_spi_result,
                dc,
                rst: lcd_rst,
                backlight_channel: peripherals.ledc.channel1,
                backlight_timer: bl_timer,
                backlight_pin: peripherals.pins.gpio42,
                buzzer,
                trackball,
            }),
            provisioned,
            pubkey_str,
            self_name,
        )?
    };
    // HIL builds: UI is absent (no display hardware on the HIL rig). The
    // channel pair is still constructed so every later dispatcher-loop
    // `evt_tx.try_send(..)`/`cmd_rx.try_recv()` call site compiles and runs
    // unchanged — nothing is ever on the other end (mirrors the pre-split
    // `ui_opt: Option<ui::UiRuntime> = None` HIL fallback exactly).
    #[cfg(feature = "hil")]
    let (evt_tx, cmd_rx): (SyncSender<ui::UiEvent>, Receiver<ui::UiCommand>) = {
        let (evt_tx, _evt_rx) = sync_channel::<ui::UiEvent>(32);
        let (_cmd_tx, cmd_rx) = sync_channel::<ui::UiCommand>(16);
        (evt_tx, cmd_rx)
    };
    // Count of `UiEvent`s dropped because `evt_tx.try_send` found the queue
    // full or disconnected (ADR-0012 C2) — logged at `warn` once per RX
    // stats rollup window (`RX_STATS_INTERVAL_MS`) rather than per
    // occurrence, alongside the existing periodic stats block below.
    let mut evt_dropped: u32 = 0;

    // The loaded provisioned config — the single mutable source of truth the
    // admin_server uses to answer QUERY_STATUS / QUERY_CONTACTS / QUERY_CHANNELS
    // and to apply ADD_*/DEL_* edits (step 2.7).  Populated below from NVS; stays
    // empty on an unprovisioned device (which runs the provisioning_server
    // instead, not admin_server) or if the config blob fails to load.  Moved
    // into the admin_server thread along with an NVS handle so runtime edits
    // persist back to flash.
    #[cfg(not(feature = "hil"))]
    let mut provisioned_config = config_store::ProvisionedConfig::empty();

    // Room contacts' runtime state (login/session tracking) — see
    // `RoomRuntime`'s doc for why this is a separate, main-thread-owned
    // snapshot rather than a handle back into `provisioned_config` above
    // (which is moved into the `admin_server` thread further down). Built
    // from `provisioned_config`'s room contacts in the provisioning-gate arm
    // below, BEFORE that move happens. Always declared (even under `hil`,
    // where it stays empty) so `on_receive`/`handle_path_return` keep one
    // signature across both build profiles.
    let mut room_runtime: Vec<RoomRuntime> = Vec::new();

    // ADR-0012 C5: every boot-time `UiRuntime` seed call (`register_room`/
    // `register_contact`/`set_channels`/`set_pin`/`set_runtime_settings`/
    // `seed_conversation`) used to be a direct call, inline, from the
    // bring-up match arms below — safe pre-split, when this task and the UI
    // shared one thread. Post-split this task cannot even name `UiRuntime`
    // (D4.2), so each site below accumulates into these locals instead; the
    // whole bundle is sent as ONE `UiEvent::BootSeed` immediately before
    // `UiEvent::AppReady`, right before the dispatcher loop starts (D8 step
    // 8) — see `ui::BootSeed`'s doc.
    let mut boot_seed_rooms: Vec<(u8, bool)> = Vec::new();
    let mut boot_seed_contacts: Vec<(u8, String)> = Vec::new();
    let mut boot_seed_channels: Vec<ui::screens::contact_list::ChannelItem> = Vec::new();
    let mut boot_seed_pin: [u8; pin_menu::MAX_PIN_LEN] = [0u8; pin_menu::MAX_PIN_LEN];
    let mut boot_seed_pin_len: u8 = 0;
    let mut boot_seed_runtime_settings: pin_menu::RuntimeSettings =
        pin_menu::RuntimeSettings::default_enabled();
    let mut boot_seed_conversations: Vec<(u8, bool, Vec<ui::MessageRecord>)> = Vec::new();

    // 2.6. First-boot provisioning gate + policy population (production only)
    #[cfg(not(feature = "hil"))]
    {
        match prov_result {
            Ok(false) => {
                log::warn!("╔══════════════════════════════════════════════════╗");
                log::warn!("║  UNPROVISIONED — connect to an admin over USB   ║");
                log::warn!("║  Run the meshcadet host CLI to provision this   ║");
                log::warn!("║  device before it can join the mesh network.    ║");
                log::warn!("╚══════════════════════════════════════════════════╝");
                log::warn!("pubkey: {}", hex_full(&identity.pubkey));
                // Spawn the provisioning server on its own thread so the main
                // thread can pump the UI render loop while awaiting USB
                // provisioning — §A wordmark + pubkey visible on the panel.
                // Mirrors the admin_server spawn pattern (main.rs:533).
                let prov_done = std::sync::Arc::new(
                    std::sync::atomic::AtomicBool::new(false)
                );
                let prov_done_tx = prov_done.clone();
                // Diagnostic counter: shared between prov_server thread (writer)
                // and the UI pump loop (reader → on-screen display).
                // Compiled in only with --features diagnostics.
                #[cfg(feature = "diagnostics")]
                let prov_rx_count = std::sync::Arc::new(
                    std::sync::atomic::AtomicU32::new(0)
                );
                #[cfg(feature = "diagnostics")]
                let prov_rx_count_tx = prov_rx_count.clone();
                let nvs_for_prov = nvs_partition.clone();
                let own_pubkey   = identity.pubkey;
                std::thread::Builder::new()
                    .name("prov_server".into())
                    .stack_size(8192)
                    .spawn(move || {
                        #[cfg(feature = "diagnostics")]
                        let run_result = provisioning_server::run(
                            nvs_for_prov, &own_pubkey, &prov_rx_count_tx,
                        );
                        #[cfg(not(feature = "diagnostics"))]
                        let run_result = provisioning_server::run(nvs_for_prov, &own_pubkey);
                        match run_result {
                            Ok(()) => {
                                log::info!(
                                    "prov_server: committed — signalling main to reboot"
                                );
                                prov_done_tx.store(
                                    true,
                                    std::sync::atomic::Ordering::Release,
                                );
                            }
                            Err(e) => {
                                // stdout write failure — log it; prov_done stays
                                // false so the device remains on the unprovisioned
                                // screen and the admin can retry.
                                log::error!("prov_server: fatal: {:?} — retry from host", e);
                            }
                        }
                    })
                    .expect("prov_server thread spawn failed");
                log::info!("prov_server thread started — signalling UI ready while waiting");
                // Waiting for USB provisioning IS the "ready" state for an
                // unprovisioned device (no radio/GPS bring-up to wait on) —
                // signals the boot-splash dismissal gate the same way the
                // provisioned path does right before its dispatcher loop.
                // ADR-0012 D8 step 6: `ui_task` intercepts `AppReady` itself
                // (mark_app_ready + the dedicated-render-loop splash ripple)
                // — this task can no longer call either directly (D4.2).
                send_ui_event(&evt_tx, &mut evt_dropped, ui::UiEvent::AppReady);
                // Wait for provisioning to complete. ADR-0012 D8 step 6: the
                // `ui.step()` pump loop this used to share the thread with is
                // DELETED — the UI now steps itself, continuously, on its own
                // task; this thread only needs to poll `prov_done`. 50 ms
                // matches the prov_server EAGAIN yield cadence.
                while !prov_done.load(std::sync::atomic::Ordering::Acquire) {
                    // Mirror the RX counter onto the on-screen display (diagnostics
                    // build only — lets the operator observe USB-serial RX activity
                    // without a serial monitor attached).
                    #[cfg(feature = "diagnostics")]
                    {
                        let rx_n = prov_rx_count.load(std::sync::atomic::Ordering::Relaxed);
                        send_ui_event(&evt_tx, &mut evt_dropped, ui::UiEvent::ProvRxBytes(rx_n));
                    }
                    esp_idf_hal::delay::FreeRtos::delay_ms(50);
                }
                log::info!("provisioning complete — rebooting");
                unsafe { esp_idf_svc::sys::esp_restart() };
                #[allow(unreachable_code)]
                return Ok(());
            }
            Ok(true) => {
                log::info!("provisioning: device is provisioned — loading contact allowlist");
                match config_store::load_provisioned_config(nvs_partition.clone()) {
                    Ok(Some(cfg)) => {
                        // The loaded config is the admin_server's single mutable
                        // source of truth for QUERY_STATUS / QUERY_CONTACTS /
                        // QUERY_CHANNELS and the ADD_*/DEL_* edits.  It is moved
                        // into `provisioned_config` at the end of this arm (after
                        // the UI/policy wiring below borrows it).  The channel
                        // secret stays on-device: the server only ever encodes the
                        // on-air channel_hash into RSP_CHANNEL.
                        let n = cfg.contact_count as usize;
                        // Room entries built alongside the contact loop below
                        // (Contacts-tab filter: `is_room()` is the one-field
                        // predicate that routes a room OUT of the Contacts tab
                        // and INTO the Groups tab — see `RoomExtra`'s own doc
                        // on why storing rooms in the one contacts store makes
                        // this filter/union a one-field affair, and
                        // `contact_list::route_contact`'s doc for why the
                        // actual routing decision below is a pure,
                        // host-testable call rather than inline branching).
                        let mut room_channel_items: Vec<ui::screens::contact_list::ChannelItem> =
                            Vec::new();
                        for i in 0..n {
                            policy.add_contact(
                                &cfg.contacts[i].pubkey,
                                cfg.contacts[i].telemetry_enable,
                            );
                            let hash = cfg.contacts[i].pub_hash();
                            let display_name = if cfg.contacts[i].display_name_len > 0 {
                                let len = cfg.contacts[i].display_name_len as usize;
                                String::from_utf8_lossy(
                                    &cfg.contacts[i].display_name[..len]
                                ).into_owned()
                            } else {
                                String::new()
                            };
                            if !display_name.is_empty() {
                                contact_display_names.insert(hash, display_name.clone());
                            }
                            // `route`'s variant is matched exhaustively below
                            // (never re-tested against `is_room()` again) so a
                            // future divergence between `route_contact`'s
                            // routing and this loop's room-session branch
                            // can't silently drop a contact into neither
                            // list — the match is the single source of truth
                            // for where this contact goes.
                            let route = ui::screens::contact_list::route_contact(
                                hash,
                                cfg.contacts[i].is_room(),
                                &display_name,
                            );

                            match route {
                                ui::screens::contact_list::ContactRoute::Room(item) => {
                                    if let Some(extra) = cfg.room_extra(&cfg.contacts[i].pubkey) {
                                        // Resume point: prefer this room's dedicated
                                        // session store (what a PRIOR live session
                                        // learned) over the provisioning-time seed,
                                        // so a reboot mid-sync doesn't re-drain the
                                        // server's whole backlog.
                                        let seed = room_session::PersistedRoomSession::from_room_extra(
                                            extra,
                                        );
                                        let session = room_session::load_room_session(
                                            nvs_partition.clone(),
                                            hash,
                                        )
                                        .unwrap_or(seed);
                                        // FINDING G: this room's erase epoch as it
                                        // stands right now — every later
                                        // `save_room_session` call for this room
                                        // passes this exact value back so it can
                                        // detect (and refuse to persist through) an
                                        // `ADD_ROOM`/`DEL_ROOM` erase that happens
                                        // later this boot without a reboot in
                                        // between. See `RoomRuntime::session_epoch`'s
                                        // doc.
                                        let session_epoch = room_session::load_room_epoch(
                                            nvs_partition.clone(),
                                            hash,
                                        );
                                        // Phase B: never present a compose box
                                        // for a message the server will swallow
                                        // — register this room's CURRENT (resumed
                                        // session, not just the provisioning-time
                                        // seed) permission with the UI so
                                        // `navigate_to_compose` can gate on it.
                                        // ADR-0012 C5: accumulated into the
                                        // BootSeed bundle, not a direct call.
                                        boot_seed_rooms.push((hash, session.permission().can_post()));
                                        room_runtime.push(RoomRuntime {
                                            pubkey: cfg.contacts[i].pubkey,
                                            hash,
                                            guest_password: extra.guest_password,
                                            guest_password_len: extra.guest_password_len,
                                            session,
                                            session_epoch,
                                            login_sent: false,
                                            last_keep_alive_ms: 0,
                                            last_reflood_ms: 0,
                                            reflood_attempts: 0,
                                            sync_phase: room_session::RoomSyncPhase::new_after_login(uptime_ms()),
                                            pending_keep_alive_ack: None,
                                            keep_alive_stall: room_session::RoomKeepAliveStall::new(),
                                            // The boot-time login below is itself
                                            // a "fresh login" — its reply should
                                            // drive the first post-login
                                            // keep-alive's `force_since` too, so
                                            // this starts true rather than false.
                                            resync_pending: true,
                                            // Seeded from flash below, at the
                                            // history-hydrate step (this room's
                                            // provisioned-config entry is
                                            // constructed before that step runs).
                                            recent: Vec::new(),
                                        });
                                    }
                                    room_channel_items.push(item);
                                }
                                ui::screens::contact_list::ContactRoute::Contact { hash, name } => {
                                    // BUG FIX: wire contact names into the UI runtime so the
                                    // contact list screen shows the provisioned contacts (§B).
                                    // register_contact() was defined but never called from main.rs.
                                    // ADR-0012 C5: accumulated into the BootSeed bundle.
                                    boot_seed_contacts.push((hash, name));
                                }
                            }
                        }
                        log::info!(
                            "room: {} room contact(s) loaded from provisioned config",
                            room_runtime.len(),
                        );
                        // BUG FIX: push channel list into the UI so the Groups tab
                        // shows the provisioned channel(s) (§B channels-tab acceptance).
                        // ADR-0012 C5: accumulated into the BootSeed bundle.
                        {
                            let ch_count = cfg.channel_count as usize;
                            let mut channel_items: Vec<ui::screens::contact_list::ChannelItem> =
                                cfg.channels[..ch_count].iter().map(|ch| {
                                    // key_len-aware channel hash (matches the
                                    // on-air hash and admin_server RSP_CHANNEL):
                                    // a 128-bit channel hashes only secret[0..16].
                                    let kl = (ch.key_len as usize).min(ch.secret.len());
                                    let ch_hash = channel_hash_var(&ch.secret[..kl]);
                                    let name = if ch.name_len > 0 {
                                        let len = ch.name_len as usize;
                                        String::from_utf8_lossy(&ch.name[..len]).into_owned()
                                    } else {
                                        format!("ch {:02x}", ch_hash)
                                    };
                                    ui::screens::contact_list::ChannelItem {
                                        name,
                                        preview: String::new(),
                                        time_str: String::new(),
                                        unread: 0,
                                        hash: ch_hash,
                                        is_room: false,
                                    }
                                }).collect();
                            // Rooms render read-only in this SAME, unified
                            // Groups tab, unioned with the true channels above
                            // — visually distinguished by `ChannelItem::is_room`
                            // (see contact_list.rs's `ContactRow`/room styling).
                            channel_items.append(&mut room_channel_items);
                            boot_seed_channels = channel_items;
                        }
                        // Wire the provisioned PIN into the UI runtime so the
                        // settings button can gate entry via pin_menu::verify_pin.
                        // ADR-0012 C5: accumulated into the BootSeed bundle.
                        boot_seed_pin = cfg.pin;
                        boot_seed_pin_len = cfg.pin_len;
                        // Load any previously-saved on-device admin-menu
                        // RuntimeSettings so the AdminMenu screen's toggles
                        // persist across reboot (separate store from the
                        // provisioning config blob — see runtime_settings_store
                        // module docs). On first boot (nothing saved yet in
                        // that store), seed the notif toggles from the admin's
                        // provisioning-time `set-notif-defaults` value rather
                        // than a hardcoded true/true default. ADR-0012 C6: the
                        // UI never writes flash itself anymore — persistence
                        // now flows the other way, via
                        // `UiCommand::PersistRuntimeSettings` (see the
                        // dispatcher-loop command-drain site below).
                        {
                            let notif_defaults =
                                (cfg.notif_defaults.visual, cfg.notif_defaults.audible);
                            match runtime_settings_store::load(nvs_partition.clone(), notif_defaults) {
                                Ok(settings) => boot_seed_runtime_settings = settings,
                                Err(e) => log::warn!(
                                    "runtime_settings_store: load failed: {:?} — using defaults",
                                    e,
                                ),
                            }
                        }
                        log::info!(
                            "policy: allowlist — {} contact(s) loaded from provisioned config",
                            n,
                        );
                        // Hand the loaded config to the admin_server (moved into
                        // its thread below) as the mutable source of truth.
                        provisioned_config = cfg;
                    }
                    Ok(None) => {
                        log::warn!(
                            "policy: provisioned but config unavailable — \
                             no contacts in allowlist; all DMs will be silently dropped"
                        );
                    }
                    Err(e) => {
                        log::error!(
                            "policy: NVS config load failed ({:?}) — \
                             no contacts in allowlist; all DMs will be silently dropped",
                            e,
                        );
                    }
                }
            }
            Err(e) => {
                // Defect fix (safe-state, criterion 4): the original code looped
                // `FreeRtos::delay_ms(5000)` forever "waiting for watchdog reboot",
                // but no Task-WDT was configured on the main task — so the device
                // wedged indefinitely.  Fix: bounded delay then explicit esp_restart()
                // so a transient NVS fault self-recovers.  If the fault is persistent
                // (e.g. flash corruption), repeated reboots surface it in the log
                // rather than silently hanging.
                log::error!(
                    "provisioning: NVS check failed ({:?}) — rebooting for self-recovery",
                    e,
                );
                log::warn!(
                    "NVS transient fault: device will reboot in 2 s. \
                     If reboots persist, re-provision over USB."
                );
                esp_idf_hal::delay::FreeRtos::delay_ms(2000); // flush log to USB-Serial-JTAG
                unsafe { esp_idf_svc::sys::esp_restart() };
                // esp_restart() does not return at runtime; satisfy the type
                // checker (the unreachable branch has no code-gen cost).
                #[allow(unreachable_code)]
                loop {}
            }
        }
    }

    // 3. Resolve peer pubkey and channel secret.
    #[cfg(feature = "hil")]
    let peer_pubkey: [u8; 32] = hil_config::HIL_PEER_PUBKEY;

    #[cfg(feature = "hil")]
    let channel_secret_buf: [u8; 32] = hil_config::HIL_CHANNEL_SECRET;
    #[cfg(feature = "hil")]
    let channel_key_len: usize = hil_config::HIL_CHANNEL_KEY_LEN;
    #[cfg(feature = "hil")]
    let channel_secret_owned: Option<([u8; 32], usize)> =
        Some((channel_secret_buf, channel_key_len));

    // Production: the on-air channel is the PROVISIONED primary channel,
    // resolved key_len-aware (16-byte 128-bit or 32-byte 256-bit secret).
    //
    // DEFECT FIX:
    // this path previously fell through to the compiled-in HIL_TEST_CHANNEL_SECRET
    // ([0x6d;32]) whenever no channel was provisioned, so a contacts-only
    // device still computed a real, on-air channel hash under that
    // published, guessable secret and both transmitted AND accepted GRP_TXT
    // on it (`meshcadet-grptxt-rx-open-on-published-test-channel-secret`) —
    // any outsider could compute the same hash and inject attributed channel
    // text into a device that never provisioned a channel at all (ADR-0001
    // §2: "no public channels — none supported at all").
    //
    // FIX: `resolve_channel_secret` returns `None` when `channel_count == 0`;
    // `channel_secret` below is `Option<&[u8]>` and every TX/RX call site
    // (`on_receive`'s `GrpTxt` arm, `handle_grp_txt`, the `SendGroupMsg` UI
    // command) is gated on `Some` — `None` makes GRP_TXT TX/RX unreachable,
    // it does not fall back to a placeholder secret. Snapshot it here at
    // boot (consistent with the UI channel list, which is also a boot
    // snapshot; a channel change requires a reboot to take effect on air).
    #[cfg(not(feature = "hil"))]
    let channel_secret_owned: Option<([u8; 32], usize)> =
        provisioned_config.resolve_channel_secret();

    let channel_secret: Option<&[u8]> =
        channel_secret_owned.as_ref().map(|(buf, len)| &buf[..*len]);

    // Production diagnosability: make the zero-channel case explicit rather
    // than silent (the `channel hash=0x..`/`channel: none provisioned` log
    // line below already distinguishes the two cases).
    #[cfg(not(feature = "hil"))]
    if channel_secret.is_none() {
        log::warn!(
            "no channel provisioned — channel (GRP_TXT) messaging is disabled; \
             provision a channel via the admin CLI to enable it"
        );
    }

    // 3.1. HIL: register the compiled-in peer as the single allowlisted contact.
    //
    // Telemetry flag:
    //   hil        → true  (HIL test exercises the telemetry pull path; the
    //                        test rig sends ?loc and must receive a location response)
    //   production → loaded from NVS provisioned config (set by admin CLI)
    #[cfg(feature = "hil")]
    {
        policy.add_contact(&peer_pubkey, true); // HIL: telemetry enabled for GPS test
        log::info!(
            "policy: HIL allowlist — 1 contact (peer pub_hash=0x{:02x}, telemetry=true)",
            peer_pubkey[0],
        );
    }

    // 3.2. HIL: precompute ECDH shared secret for outbound TEST DMs.
    #[cfg(feature = "hil")]
    let shared_secret = identity.ecdh_shared_secret(&peer_pubkey);

    #[cfg(feature = "hil")]
    log::info!("peer pub_hash=0x{:02x}", peer_pubkey[0]);
    match channel_secret {
        Some(secret) => log::info!(
            "channel hash=0x{:02x}, policy contacts={}",
            channel_hash_var(secret),
            policy.contact_count(),
        ),
        None => log::info!(
            "channel: none provisioned, policy contacts={}",
            policy.contact_count(),
        ),
    }

    // Per-boot random base for outbound message timestamps (anti-replay).
    // MUTABLE: the dispatcher loop below rebases this to the real GPS-synced
    // wall clock the moment (and every tick after) `gps` first syncs — see
    // the "tx timestamp base" rebase right after `gps.poll` for why this
    // alone is sufficient to make every `tx_epoch_base.wrapping_add((now_ms
    // / 1000) as u32)` call site below (DM/GRP_TXT/telemetry-reply
    // timestamps) read real time post-sync with no other code change.
    //
    // ROOM FRAMES ARE THE ONE EXCEPTION (`meshcadet-room-monotonic-tx-
    // timestamp`): a room login/keep-alive/post never reads `tx_epoch_base`
    // — they use `room_session::room_tx_timestamp`, seeded from each room's
    // OWN persisted `last_room_ts`, never from this random seed. Broadening
    // that room-scoped source into DM/GRP_TXT/advert (which still rely on
    // `tx_epoch_base` here) is explicitly out of scope — see that mission's
    // Scope section.
    let mut tx_epoch_base: u32 = unsafe { esp_idf_svc::sys::esp_random() };
    log::info!("tx timestamp base seeded (per-boot anti-replay)");

    // Device-wide wall clock adopted from a room server's own clock
    // (`meshcadet-room-adopt-server-time`) — `None` until either a room
    // login reply's `server_ts` or an inbound push's `post_ts` is adopted
    // (`room_session::adopt_server_clock`). Deliberately NOT per-room: any
    // room server's clock is an equally trustworthy wall-clock reading, and
    // a device may have provisioned more than one room — whichever answers
    // first seeds this for all of them. Combined with GPS every tick via
    // `room_session::trusted_wall_clock_secs` (GPS always wins while
    // synced — see that function's doc).
    //
    // Staying global is safe DESPITE the value flowing into every room's own
    // `room_tx_timestamp` (`meshcadet-room-clock-plausibility-bounds`,
    // Finding C): `adopt_server_clock` refuses any `server_ts`/`post_ts` at
    // or above `room_session::ROOM_CLOCK_PLAUSIBILITY_CEILING_SECS` before it
    // can ever become this shared clock, so one misconfigured or hostile
    // room server can no longer ratchet every OTHER, correctly-clocked
    // room's persisted `last_room_ts` (and so burn their replay ceilings) by
    // handing this device an implausible reading.
    //
    // Deliberately NOT `#[cfg(not(feature = "hil"))]`, unlike most other
    // room state: `on_receive` below threads it through unconditionally
    // (same shape as `room_runtime`/`nvs_partition`, both already compiled
    // in every build) — a `hil` build simply never reaches the call sites
    // that ever adopt anything into it (`room_runtime` is empty there), so
    // it stays `None` for the life of the process, exactly like a `hil`
    // build's `room_runtime` stays empty.
    let mut adopted_server_clock: Option<room_session::AdoptedServerClock> = None;

    // 5b. Initialise SX1262 radio (pins per LilyGo utilities.h).
    //     Board power enable (step 4), the SPI2 bus driver, and the radio's
    //     SpiDeviceDriver REGISTRATION (step 5a/5b) were all moved above the
    //     provisioning gate (ADR-0012 D2's corollary) so the display is
    //     available on first boot AND every `spi_bus_add_device` call stays
    //     single-threaded. `spi_device` (this task's exclusively-owned
    //     radio SPI handle) is still in scope from there; only the actual
    //     chip bring-up below is new at this point in `run()`.
    let rst  = PinDriver::output(peripherals.pins.gpio17)?;
    let busy = PinDriver::input(peripherals.pins.gpio13, Pull::Floating)?;
    let dio1 = PinDriver::input(peripherals.pins.gpio45, Pull::Floating)?;

    // D9/D11 SPI2 bus-hold probe (`radio::PIN_SPI_PROBE`, GPIO39/BOARD_SDCARD_CS
    // — see that constant's doc for the pin choice). `--features diagnostics`
    // only, so a production build claims no extra GPIO.
    #[cfg(feature = "diagnostics")]
    let probe = PinDriver::output(peripherals.pins.gpio39)?;

    let mut radio = Radio::init(
        spi_device,
        rst,
        busy,
        dio1,
        #[cfg(feature = "diagnostics")]
        probe,
    )?;
    log::info!("radio initialised");

    // 6. Initialise GPS UART1 (GPIO43 TX, GPIO44 RX; baud auto-probed —
    //    see below).
    //
    // The console has been redirected to USB-Serial-JTAG via
    // `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y` in sdkconfig.defaults, so
    // GPIO43/44 are free for UART1.
    //
    // The UART is opened at `GPS_BAUD` (= `GPS_BAUD_CANDIDATES[0]`, 9600) but
    // GpsDriver::new() immediately determines the actual rate the module on
    // this unit is transmitting at — the T-Deck Plus ships with a Quectel
    // L76K (9600 bps), a u-blox M10Q (38400 bps), or (rarely) a reconfigured
    // variant at 115200, and a field capture proved a fixed 9600 assumption
    // decodes a real non-L76K unit's NMEA stream as garbage. The fix:
    // an NVS-cached rate from a previous boot is used directly (self-healing
    // at runtime if it turns out stale); otherwise a full
    // `gps::GPS_BAUD_CANDIDATES` probe runs (see `gps::probe_candidates`),
    // requiring a checksum-valid NMEA sentence before locking, and persists
    // the winning rate via `gps_baud_store` so later boots skip the probe.
    // Once locked, `new()` sends the L76K `$PCAS` init triad only if the
    // detected rate is the L76K's, or the u-blox `$PUBX,40` sequence
    // otherwise — see `gps::L76K_INIT_COMMANDS`'s doc for why an init
    // sequence is required at all (this fixed a real-world "receiver emits
    // zero NMEA sentences" defect).
    let uart_config = UartConfig::new().baudrate(gps::GPS_BAUD.Hz());
    let gps_uart = UartDriver::new(
        peripherals.uart1,
        peripherals.pins.gpio43,          // TX: ESP → GPS RX
        peripherals.pins.gpio44,          // RX: GPS TX → ESP
        Option::<AnyIOPin>::None,         // CTS unused (no flow control on either module variant)
        Option::<AnyIOPin>::None,         // RTS unused
        &uart_config,
    )?;
    let now0 = uptime_ms();
    let mut gps = GpsDriver::new(gps_uart, now0, nvs_partition.clone());
    log::info!(
        "GPS UART1 initialised — GPIO43 TX / GPIO44 RX / baud auto-detected (cached in NVS; \
         see \"GPS: baud\" log lines above) (active window: {}s every {}s duty cycle)",
        gps::GPS_ACTIVE_WINDOW_MS / 1000,
        (gps::GPS_ACTIVE_WINDOW_MS + gps::GPS_QUIET_INTERVAL_MS) / 1000,
    );

    // 6.5. Initialise the battery ADC (GPIO4 / BOARD_BAT_ADC — see `battery.rs`
    // module docs for the hardware-feasibility rationale: plain ADC voltage
    // divider, no PMU/fuel-gauge IC, no pin collision). Propagates on failure,
    // matching this boot sequence's existing convention for peripheral
    // bring-up (SPI2, GPS UART1 above both do the same via `?`). Passes
    // `nvs_partition` so `settled_mv` can be restored/persisted under the
    // `mc_cfg` provisioning namespace (see `battery.rs`'s "NVS layout"
    // section) — same `.clone()` convention as `GpsDriver::new` above.
    let mut battery = BatteryDriver::new(
        peripherals.adc1,
        peripherals.pins.gpio4,
        now0,
        nvs_partition.clone(),
    )?;

    // 2.7. History store init + admin USB-serial server thread (production only).
    //
    // HistoryStore::new locates the dedicated `mc_hist` raw partition (flash-
    // backed, per-conversation regions — see history_store.rs module docs)
    // and, on first boot after this firmware, runs the one-shot legacy-NVS
    // migration.  Both can fail (partition missing from a stale flashed
    // table, NVS I/O error), so this propagates via `?` like the other
    // peripheral bring-up steps above (SPI2, GPS UART1, battery ADC).  The
    // HISTORY static is populated here — exactly once per boot — so every
    // subsequent `HISTORY.lock()` in handle_dm finds `Some(store)`.
    //
    // admin_server::run() blocks its own thread waiting for host requests;
    // spawn it with std::thread so it does not interrupt the radio loop.
    #[cfg(not(feature = "hil"))]
    {
        let mut store = history_store::HistoryStore::new(nvs_partition.clone())?;

        // BUG FIX: `UiRuntime::messages`
        // previously started empty every boot and was only ever populated by the
        // live radio-event path (`on_send_message` / the RX handlers below) — a
        // power-cycle silently discarded the on-screen view of history that was
        // still sitting, intact, in the just-opened `mc_hist` flash store. Read it
        // back here, once, and seed the UI's `messages` map directly.
        //
        // MUST run after `register_contact`/`set_channels` above (so hydrated
        // previews land against known contact/channel names) and BEFORE the first
        // `navigate_to_contact_list` (driven by `dismiss_splash`, gated on
        // `mark_app_ready` + the splash-minimum timer — always later than this
        // point in `run()`), so the very first contact-list paint already reflects
        // restored history instead of only a live send/receive filling it in.
        // ADR-0012 C5: accumulated into the BootSeed bundle rather than a
        // direct `ui.seed_conversation(..)` call — this task can no longer
        // name `UiRuntime` (D4.2). Unlike pre-split (gated on `ui_opt` being
        // `Some`), history is now always hydrated regardless of whether
        // `ui_task` ends up running headless: `room.recent`'s content-dedup
        // tail (below) depends on this same read, and this task has no way
        // to observe `ui_task`'s construction outcome by the time it reaches
        // this point (D4.2's visibility barrier cuts both ways) — a strictly
        // more-correct default, at the cost of one extra flash read in the
        // rare real-hardware-failure case.
        {
            match store.load_all_conversations() {
                Ok(conversations) => {
                    for (kind, conv_hash, entries) in conversations {
                        let is_channel = kind == protocol::history::HistoryMsgType::GrpTxt;
                        // Seed this room's content-dedup tail from flash
                        // (`RoomRuntime::recent`'s doc) BEFORE `entries` is
                        // consumed below — a room's pushed posts are stored
                        // as ordinary `HistoryMsgType::Dm` entries keyed by
                        // the room's own hash (see `handle_room_push_frame`),
                        // so this is exactly the same lookup a true DM
                        // contact would no-op on.
                        if !is_channel {
                            if let Some(room) = room_runtime.iter_mut().find(|r| r.hash == conv_hash) {
                                room.recent = entries
                                    .iter()
                                    .map(|(entry, _is_ours, _acked)| *entry)
                                    .collect();
                            }
                        }
                        let records: Vec<ui::MessageRecord> = entries
                            .into_iter()
                            .map(|(entry, is_ours, acked)| {
                                // Defensive clamp: `decode_entry_blob` (protocol
                                // crate) does not itself bound-check `text_len`
                                // against `entry.text`'s fixed 64-byte capacity, so
                                // a corrupted flash blob (bit-flip, torn write) could
                                // otherwise carry `text_len > 64` and panic this
                                // slice. `.min(entry.text.len())` makes hydrate
                                // resilient to that without touching the shared
                                // codec (pre-existing latent risk in the codec's
                                // export path too — out of scope for this fix).
                                let text_len = (entry.text_len as usize).min(entry.text.len());
                                ui::MessageRecord {
                                    text: String::from_utf8_lossy(&entry.text[..text_len])
                                        .into_owned(),
                                    is_ours,
                                    // BUG FIX: this used to hardcode `true`
                                    // regardless of the
                                    // entry's real pre-reboot state, so every restored
                                    // outbound message showed "✓✓" even if it was
                                    // still pending when the device powered off. The
                                    // slot codec now persists the actual ack/delivery
                                    // bit (`protocol::history_region::FLAG_ACKED`);
                                    // restore it as-is so the checkmark matches
                                    // whatever it showed before the power cycle.
                                    //
                                    // `Undelivered` (red) is never restored here — the
                                    // flash slot only ever persists a 2-state ack bit
                                    // (`acked`), and the dispatcher's outstanding-sends
                                    // table that would know a send timed out or was
                                    // evicted is in-memory only and empty again fresh
                                    // off a reboot. A message that was red when the
                                    // device powered off simply restores as `Pending`
                                    // — no worse than the pre-tri-state behavior, which
                                    // had no red state to lose at all.
                                    delivery: if acked { ui::DeliveryState::Acked } else { ui::DeliveryState::Pending },
                                    // `None`: the dispatcher's outstanding-sends table
                                    // (and the wire ACK hash a live entry would carry)
                                    // is in-memory only and starts empty every boot —
                                    // there is no ack_hash left to restore from flash.
                                    // A DM record restored here that was genuinely
                                    // still pending pre-reboot simply stays `Pending`
                                    // forever (matches the pre-tri-state "acked=false
                                    // never re-checked" restore behavior — not a
                                    // regression this mission introduces).
                                    ack_hash: None,
                                    // NOTE: unused for rendering today (no message
                                    // view shows a timestamp — see `MessageRecord::
                                    // ts_ms`'s own doc comment). A future "time
                                    // sent" label should source unix-seconds from
                                    // `entry.timestamp` here instead of `0`.
                                    ts_ms: 0,
                                }
                            })
                            .collect();
                        boot_seed_conversations.push((conv_hash, is_channel, records));
                    }
                }
                Err(e) => log::warn!(
                    "history hydrate: load_all_conversations failed ({:?}) — \
                     conversation views start empty this boot",
                    e,
                ),
            }
        }

        *HISTORY.lock().expect("HISTORY mutex poisoned on init") = Some(store);
        log::info!("history store initialised (mc_hist partition, per-conversation regions)");

        // Pass the shared HISTORY mutex so the server reads and main-thread
        // appends are mutually excluded (history_store module-level discipline).
        // Also pass a clone of the identity (the seed is needed to sign the
        // QUERY_ADVERT self-advert card, not just report the pubkey — see
        // admin_server::run's doc comment), the loaded provisioned config (the
        // mutable source of truth for QUERY_STATUS / QUERY_CONTACTS /
        // QUERY_CHANNELS and the ADD_*/DEL_* edits), and an NVS handle so runtime
        // edits persist back to flash.  The config + NVS handle are moved into
        // the thread.
        let identity_for_admin = identity.clone();
        let nvs_for_parent = nvs_partition.clone();
        std::thread::Builder::new()
            .name("admin_server".into())
            // 12 KiB. Originally bumped from 8 KiB (see `admin_server.rs`'s
            // stale doc comment at `run`'s definition, since corrected) on the
            // premise that the server owning the loaded `ProvisionedConfig`
            // plus the per-persist serialize buffer cost only ~1.6 KiB each —
            // wrong by ~2x (`size_of::<ProvisionedConfig>()` is 3560 B, and
            // `save_provisioned_config`'s own blob buffer is another 3544 B),
            // which is what actually caused a boot-time `pthread`-task stack
            // overflow (`boot-pthread-stack-overflow-fix` mission). Both are
            // now heap-allocated instead (`Box<ProvisionedConfig>` below;
            // `config_store`'s serialize/deserialize blob buffers) rather than
            // resident/transient on this stack, so 12 KiB is now generous
            // headroom rather than a tight fit — kept at 12 KiB rather than
            // trimmed back, since no HIL measurement of the new HWM exists yet
            // to size a smaller budget from (see `admin_server::run`'s own
            // `log_thread_stack_hwm` calls, added this same mission, for that
            // measurement once hardware is available).
            .stack_size(12288)
            .spawn(move || {
                admin_server::run(
                    &HISTORY,
                    &GPS_STATUS,
                    &BATTERY_STATUS,
                    identity_for_admin,
                    Box::new(provisioned_config),
                    nvs_for_parent,
                );
            })
            .expect("admin_server thread spawn failed");
        log::info!("admin server thread started");
    }

    // 8. Dispatcher state
    let mut dedup  = DuplicateFilter::new();
    let mut budget = AirtimeBudget::new();
    let mut txq    = TxQueue::new();

    // Repeater signal-strength tracker (ADR-0010) — in-memory only, no reboot
    // persistence (see `SignalTracker::new`'s doc), so it is seeded fresh here
    // every boot, starting at `SignalLevel::DirectOnly` until the first
    // hop>=1 packet is recorded by the RX-poll tap below.
    let mut signal_tracker = SignalTracker::new(SignalConfig::default());

    let mut outstanding = OutstandingSends::new();
    let mut pending_channel_ack: Option<PendingChannelAck> = None;

    // ADR-0012 C4: per-iteration state snapshots become change-detected
    // events. Four values are recomputed every dispatcher iteration but
    // almost always unchanged — holding the last-sent value here and
    // comparing before every `send_ui_event` call keeps queue traffic well
    // below the old per-iteration call rate rather than raising it.
    // `UiRuntime::set_*`'s own early-return (unchanged) stays as defence in
    // depth on the receiving end.
    let mut last_sent_gps_status: Option<gps::GpsStatus> = None;
    let mut last_sent_room_clock: Option<(room_session::ClockSource, Option<u32>, u32)> = None;
    let mut last_sent_battery_status: Option<battery::BatteryStatus> = None;
    let mut last_sent_signal_level: Option<SignalLevel> = None;

    // RX counters
    let mut rx_done_count: u32 = 0;
    let mut crc_err_count: u32 = 0;
    let mut rx_none_count: u32 = 0;
    let mut last_rx_stats_ms: u64 = 0;
    const RX_STATS_INTERVAL_MS: u64 = 30_000;

    // ── On-device superloop timing instrumentation (M0 of
    // `meshcadet-perf-rearchitecture`, `--features diagnostics` only) ────────
    //
    // `Box`ed rather than a plain stack local: `PerfRollup` holds seven
    // `PhaseStats` accumulators (a histogram apiece, ~150 B each), and this
    // binding lives for the entire dispatcher loop's lifetime alongside every
    // other `run()`-frame local — `firmware/sdkconfig.defaults`'s stack-
    // budget comment documents a confirmed release-only main-task stack
    // overflow from exactly this kind of frame growth. Diagnostics builds
    // aren't covered by that comment's HWM measurements, so boxing this one
    // (heap, not stack) rather than adding ~1 KB to an already-tight budget
    // is the conservative choice.
    #[cfg(feature = "diagnostics")]
    let mut perf_rollup = Box::new(perf::PerfRollup::new());
    // Wall-clock (microsecond) timestamp of the last time this loop *entered*
    // the RX-poll call — used to derive the RX-notice-latency proxy at that
    // call site below (see its doc for the exact definition).
    #[cfg(feature = "diagnostics")]
    let mut last_rx_poll_entry_us: u64 = uptime_us();

    // Per-iteration RX-poll yield window.
    //
    // `radio.try_receive` does not need this wait for RX *correctness* — the
    // radio stays in continuous RX and DIO1 latches high on RxDone until
    // explicitly cleared, so a packet completing between polls is still
    // caught on the very next call regardless of how long this window is (see
    // `Radio::try_receive`'s doc). Its only job is to bound how long this
    // task can go without servicing GPS poll, battery poll, room keep-alive,
    // and draining `UiCommand`s TO the (now separate) UI task.
    //
    // CORRECTION (`meshcadet-perf-radio-dio1-interrupt`): the 5 ms value and
    // the comment that used to justify it (touch/keyboard sampling cadence,
    // "`ui.step()` ... runs once per dispatcher loop iteration") both predate
    // ADR-0012's task split. `ui.step()` has not run on this task since that
    // split — the UI (touch, keyboard, render) is `ui_task.rs`, pinned to
    // core 1, on its own cadence entirely independent of this loop. Nothing
    // this task still owns (GPS poll — duty-cycled; battery poll — throttled
    // ADC; room keep-alive — its own scheduler; the 30 s RX-stats rollup) is
    // gated on wall-clock elapsed time rather than iteration count (see
    // `gps::should_close_active_window`/`should_reopen_active_window` for the
    // pattern), so none of them regress from a looser iteration cadence.
    //
    // Retuned 5 ms → 20 ms now that `try_receive`'s DIO1 wait is interrupt/
    // notification-driven rather than a `FreeRtos::delay_ms(1)` spin: an idle
    // iteration used to cost up to 5 separate 1 ms sleep/wake cycles just to
    // find nothing; it now costs exactly one blocking wait, so widening the
    // bound reduces scheduler wake-ups on this task without reintroducing the
    // pre-split 50 ms "DEFECT" (still 2.5x tighter), and matches `channel_
    // activity_detection`'s own 20 ms CAD window for one consistent cadence
    // on this task rather than two arbitrary ones. The perf_loop_model host
    // harness (`meshcadet-perf-radio-host-validation`) is the tool that
    // measures this window's actual effect on UI-unserviced-gap and
    // RX-notice-latency numbers; this value is a documented, reasoned
    // starting point, not a claimed measurement.
    const RX_POLL_YIELD_MS: u32 = 20;

    #[cfg(feature = "hil")]
    let mut last_tx_ms: u64 = 0;

    let mut frame_buf = [0u8; 255];

    let mut cad_err_streak: u32 = 0;
    const CAD_FAIL_LIMIT: u32 = 3;

    // Non-blocking CAD-busy backoff gate.
    //
    // DEFECT: `channel_activity_detection()` reporting the channel busy used to
    // be handled with `FreeRtos::delay_ms(backoff_ms)` — a straight-line,
    // 1000–3000 ms block of the ENTIRE dispatcher loop, including the touch
    // and keyboard polling in `ui.step()` later in this same iteration. Any
    // tap or keypress that started and finished inside that window was lost
    // outright (the GT911/keyboard co-processor state was never read), not
    // merely delayed — this was the dominant contributor to the reported
    // "sometimes drops" symptom on a mesh with any co-channel traffic (every
    // received DM enqueues an ACK, which re-triggers CAD).
    //
    // FIX: replace the blocking sleep with a deadline. When CAD reports busy,
    // record `now + backoff_ms` here instead of sleeping; the CAD+TX block
    // below skips re-attempting CAD until that deadline passes, but every
    // other part of the loop (RX poll, `ui.step()`) keeps running every
    // iteration in the meantime — CAD retry timing is unchanged, only the
    // full-thread stall is removed.
    let mut cad_backoff_until_ms: u64 = 0;

    // ── Task Watchdog subscription (defect fix — criterion 3) ────────────────
    //
    // Subscribe the main task to the ESP-IDF Task WDT so that a hung SPI/BUSY
    // wait or any other stall in the dispatcher loop triggers a panic → safe
    // reboot within CONFIG_ESP_TASK_WDT_TIMEOUT_S seconds (30 s in sdkconfig).
    //
    // Prerequisites (sdkconfig.defaults):
    //   CONFIG_ESP_TASK_WDT_EN=y          — enable TWDT (auto-init at startup)
    //   CONFIG_ESP_TASK_WDT_PANIC=y       — trigger panic (not just a warning)
    //   CONFIG_ESP_TASK_WDT_TIMEOUT_S=30  — 30 s timeout (generous vs. ~50 ms loop)
    //
    // esp_task_wdt_add(NULL) subscribes the *calling* task (main/app_main).
    // esp_task_wdt_reset() resets ("pets") the timer; called each loop iteration.
    {
        let ret = unsafe { esp_idf_svc::sys::esp_task_wdt_add(core::ptr::null_mut()) };
        if ret == 0 {
            log::info!("dispatcher: subscribed to Task WDT (30 s timeout)");
        } else {
            // Non-fatal: log and continue.  The TWDT may not be initialised in
            // HIL builds where sdkconfig.defaults differs, or if the IDF
            // version uses a different init sequence.  A missing TWDT subscription
            // is acceptable for HIL; production sdkconfig.defaults enables it.
            log::warn!(
                "dispatcher: esp_task_wdt_add failed (0x{:08x}) — loop not WDT-covered",
                ret,
            );
        }
    }

    // ── Room logins: flood ANON_REQ once per provisioned room ────────────────
    //
    // Always flood-routed (`room_session::encode_room_login_frame` hardcodes
    // `RouteType::Flood`) — first contact has no learned mesh route to the
    // room server, so a direct send isn't an option yet (see that function's
    // doc). `sync_since` is each room's RESUMED watermark (session store if
    // one exists, else the provisioning-time seed), so a reboot mid-sync
    // does not re-drain the server's whole backlog. `room_runtime` is empty
    // under `hil` (no rooms there), so this loop is a no-op in that build.
    {
        let boot_now = uptime_ms();
        // This login send runs before the dispatcher loop's first GPS poll
        // ever executes, so `gps` has had no chance to sync yet — this is
        // always `None` today. Read the real driver anyway (rather than
        // hardcoding it) so `room_session::room_tx_timestamp` stays correct
        // even if this ordering ever changes, instead of silently assuming
        // it never will.
        let boot_wall_clock_secs = gps.synced_wall_clock_secs(boot_now);
        for room in room_runtime.iter_mut() {
            // Defensive: this loop runs exactly once per boot today (no
            // keep-alive/re-login scheduler until milestone 2), but guard on
            // the flag anyway rather than relying on that being the only
            // caller forever.
            if room.login_sent {
                continue;
            }
            // `meshcadet-room-monotonic-tx-timestamp`: monotonic, never
            // random, seeded from THIS room's own persisted watermark — not
            // `tx_epoch_base` (that stays `esp_random()`-seeded pre-sync for
            // DM/GRP_TXT/advert, out of scope here; see this fn's `tx_epoch_
            // base` doc).
            let boot_ts =
                room_session::room_tx_timestamp(boot_wall_clock_secs, room.session.last_room_ts);
            let shared = identity.ecdh_shared_secret(&room.pubkey);
            let mut frame = [0u8; room_session::MAX_LOGIN_FRAME_LEN];
            let n = room_session::encode_room_login_frame(
                &shared,
                room.hash,
                &identity.pubkey,
                boot_ts,
                room.session.sync_since,
                &room.guest_password[..room.guest_password_len as usize],
                &mut frame,
            );
            log_tx_queue_eviction(
                txq.enqueue(&frame[..n], None),
                "room login",
                &mut outstanding,
                |ev| send_ui_event(&evt_tx, &mut evt_dropped, ev),
            );
            room.login_sent = true;
            room.session.record_sent_timestamp(boot_ts);
            // `meshcadet-room-ts-watermark-write-behind`: persist the
            // advanced watermark now, not just on the next login
            // reply/push/stall event — see that mission's doc for why
            // leaving `last_room_ts` RAM-only between those rarer events
            // lets a reboot resume below a value already used with the
            // server.
            room_session::save_room_session(
                nvs_partition.clone(),
                room.hash,
                room.session_epoch,
                &room.session,
            );
            log::info!(
                "room: queued flood login for 0x{:02x} (sync_since={})",
                room.hash, room.session.sync_since,
            );
        }
    }

    // Boot sequence complete — radio, GPS, history store, and the
    // admin-server thread are all live. ADR-0012 D8 step 8: send the whole
    // BootSeed bundle FIRST, then AppReady — `ui_task` applies the seed via
    // `handle_event`'s `UiEvent::BootSeed` arm before it ever sees
    // `AppReady` (C3: single producer, so mpsc's FIFO gives total order per
    // direction). `ui_task`'s own loop intercepts `AppReady` directly
    // (mark_app_ready + the dedicated-render-loop splash ripple, D8 step 9)
    // — this task can no longer call either (D4.2). Unlike the pre-split
    // direct call, this does NOT block this task for ~1.15s: the ripple now
    // runs on `ui_task`'s own core-1 thread while this dispatcher loop
    // starts immediately below — the documented boot RX gap this used to
    // cause is gone (a real, if secondary, win the ADR calls out).
    send_ui_event(
        &evt_tx,
        &mut evt_dropped,
        ui::UiEvent::BootSeed(Box::new(ui::BootSeed {
            rooms: boot_seed_rooms,
            contacts: boot_seed_contacts,
            channels: boot_seed_channels,
            pin: boot_seed_pin,
            pin_len: boot_seed_pin_len,
            runtime_settings: boot_seed_runtime_settings,
            conversations: boot_seed_conversations,
        })),
    );
    send_ui_event(&evt_tx, &mut evt_dropped, ui::UiEvent::AppReady);

    // ── Dispatcher loop ───────────────────────────────────────────────────────
    loop {
        let now = uptime_ms();

        // Pet the Task WDT: this task is still alive and iterating.
        // Called unconditionally at the top of every iteration so that any
        // stall deeper in the loop (SPI/BUSY wait, crypto, NVS write) is
        // bounded by the TWDT timeout.
        unsafe { esp_idf_svc::sys::esp_task_wdt_reset(); }

        // ── Outstanding-sends deadline sweep (auto-retry) ─────────────────────
        // Every DM/room-post send whose current deadline has passed with no
        // ACK lands here — the ONLY place a send times out rather than
        // sitting pending forever (a TX-queue eviction is the other
        // undelivered path, resolved inline at each `log_tx_queue_eviction`/
        // `insert_outstanding` call site instead). With attempts remaining
        // (`dispatcher::MAX_SEND_RETRIES`, hard cap of 2), `retry_if`
        // re-enqueues the BYTE-IDENTICAL cached frame from `send.frame` —
        // NEVER re-encoded, see `OutstandingSend`'s doc for why a room-post
        // re-encode would be a genuine duplicate post — and reports `true` to
        // keep tracking it under `dispatcher::retry_backoff_ms`'s expanded,
        // jittered deadline. A room post additionally checks the room's
        // CURRENT session state before retrying (never blind-retry into a
        // session that may have logged out / gone read-only since the
        // original send); a DM has no such gate. Attempts exhausted, or
        // `retry_if` declining, finalizes the record red (undelivered) via
        // `on_undelivered`, same event `delivery_event` raises for a
        // deadline timeout today. Bounded by `MAX_OUTSTANDING_SENDS` (8
        // slots), so scanning it every iteration is cheap — same cost class
        // as `TxQueue::has_pending()`'s own per-iteration check just below.
        //
        // A retry's own `txq.enqueue` can itself evict an unrelated queued
        // frame (TX_QUEUE_SLOTS is small); `outstanding.resolve_evicted`
        // can't be called from inside this closure (it would re-borrow
        // `outstanding`, which `sweep_expired` already holds `&mut` for), so
        // evictions are collected here and resolved in the follow-up loop
        // below instead — the same "collect now, process after" idiom the
        // UI-command drain above already uses.
        let mut retry_evictions: Vec<(usize, Option<[u8; 4]>)> = Vec::new();
        outstanding.sweep_expired(
            now,
            |send| {
                if let OutstandingKind::RoomPost { room_hash } = send.kind {
                    let can_post = room_runtime
                        .iter()
                        .find(|r| r.hash == room_hash)
                        .map(|r| r.session.permission().can_post())
                        .unwrap_or(false);
                    if !can_post {
                        log::warn!(
                            "room 0x{:02x} session no longer postable — abandoning retry \
                             of ack {} rather than blind-retrying into a stale session",
                            room_hash, hex4(&send.ack_hash),
                        );
                        return false;
                    }
                }
                log::info!(
                    "retrying unacked send with no ACK yet ({:?}, ack {}, attempt {})",
                    send.kind, hex4(&send.ack_hash), send.attempts + 1,
                );
                if let Some(dropped) = txq.enqueue(&send.frame[..send.frame_len], Some(send.ack_hash)) {
                    retry_evictions.push(dropped);
                }
                true
            },
            |send| {
                log::warn!(
                    "outstanding send undelivered ({:?}, ack {}, {} attempt(s) made) — marking red",
                    send.kind, hex4(&send.ack_hash), send.attempts + 1,
                );
                send_ui_event(&evt_tx, &mut evt_dropped, delivery_event(send, false));
            },
        );
        for dropped in retry_evictions {
            log_tx_queue_eviction(
                Some(dropped),
                "retry re-enqueue",
                &mut outstanding,
                |ev| send_ui_event(&evt_tx, &mut evt_dropped, ev),
            );
        }

        // ── GPS poll (duty-cycle NMEA read + fix cache refresh) ──────────────
        #[cfg(feature = "diagnostics")]
        let phase_t0 = uptime_us();
        gps.poll(now);
        #[cfg(feature = "diagnostics")]
        perf_rollup.gps.record((uptime_us().saturating_sub(phase_t0)) as u32);

        // ── Rebase the tx timestamp origin onto GPS-synced wall-clock time ───
        // Runs unconditionally (both `hil` and production — `gps` and its
        // clock-sync tracking exist in both) every iteration, not just once
        // on the sync transition: rebasing every tick self-corrects for any
        // GPS-vs-uptime-clock drift the same way a fresh `settimeofday` call
        // would, and is a cheap no-op (`synced_wall_clock_secs` returns
        // `None`, `tx_epoch_base` untouched) before the first fix ever syncs.
        // The `wrapping_sub` is the exact inverse of every call site below's
        // `tx_epoch_base.wrapping_add((now_ms / 1000) as u32)`, so from this
        // point on that SAME formula evaluates to real Unix time — no other
        // call site needs to change. Pre-sync, `tx_epoch_base` stays the
        // original per-boot `esp_random()` value: fine for DM/GRP_TXT
        // anti-replay (only ever compared against itself — see
        // `firmware/src/advert_ts_store.rs`'s doc), just not a real clock
        // reading yet.
        // This same reading also feeds every room frame's timestamp this
        // tick (`room_session::room_tx_timestamp`,
        // `meshcadet-room-monotonic-tx-timestamp`) — captured once here so
        // `tx_epoch_base`'s rebase below and every room send this iteration
        // agree on exactly the same "is the clock synced right now, and to
        // what" answer.
        #[cfg(not(feature = "hil"))]
        let synced_wall_clock_secs = gps.synced_wall_clock_secs(now);
        if let Some(unix_secs) = gps.synced_wall_clock_secs(now) {
            tx_epoch_base = unix_secs.wrapping_sub((now / 1000) as u32);
        }
        // Whether the wall clock is genuinely GPS-synced right now, from a
        // real fix — the same source GPS Status reads. Feeds the room-post
        // refusal message below (a real, still-relevant distinction: an
        // unsynced clock is no longer a refusal reason at all — see
        // `room_tx_timestamp`'s doc — vs. "clock hasn't advanced since the
        // last send", the one refusal reason that remains) AND
        // `room_session::adopt_server_clock`'s priority rule. This is
        // deliberately `clock_sync_verified()`, NOT `synced_wall_clock_secs
        // (now).is_some()` (which was this variable's definition before
        // `meshcadet-clock-source-provenance-and-sync-age` decided the
        // room-server-vs-drifted-GNSS-RTC policy): an UNVERIFIED, RTC-
        // derived GPS sync must not block adopting a room server's own,
        // externally-confirmed clock — only a real fix should. Computed
        // unconditionally (both `hil` and production, mirroring the rebase
        // just above) so `on_receive`'s room clock-adoption threading below
        // has a value in every build — a `hil` build never reaches the call
        // sites that actually consume it (`room_runtime` is empty there).
        let gps_verified_now = gps.clock_sync_verified();

        // Combine GPS with any adopted room-server clock
        // (`meshcadet-room-adopt-server-time`): a VERIFIED GPS sync always
        // wins; otherwise an adopted room-server clock outranks an
        // unverified, RTC-derived GPS sync (`room_session::trusted_wall_
        // clock_secs`'s doc — same three-tier priority `gps_verified_now`
        // above feeds); every room-scoped TX-timestamp call site below reads
        // `room_wall_clock_secs` instead of `synced_wall_clock_secs`
        // directly, so a GPS-denied device's room frames — and, once
        // `meshcadet-room-clock-ux` lands, its rendered room timestamps —
        // carry a real wall-clock reading as soon as any room server has
        // answered, not only once GPS fixes.
        #[cfg(not(feature = "hil"))]
        let (room_wall_clock_secs, room_clock_source) = room_session::trusted_wall_clock_secs(
            synced_wall_clock_secs,
            gps_verified_now,
            adopted_server_clock,
            now,
        );

        // ── Battery poll (throttled ADC read + charging-trend refresh) ───────
        #[cfg(feature = "diagnostics")]
        let phase_t0 = uptime_us();
        battery.poll(now);
        #[cfg(feature = "diagnostics")]
        perf_rollup.battery.record((uptime_us().saturating_sub(phase_t0)) as u32);

        // Refresh the shared GPS status snapshot: the touch UI (same thread,
        // fed directly) and admin_server (separate thread, via the
        // GPS_STATUS mutex — same cross-thread pattern as HISTORY) both
        // display fix state, coordinates + age, and clock-sync state + age.
        #[cfg(not(feature = "hil"))]
        {
            let gps_status = gps.status(now);
            match GPS_STATUS.lock() {
                Ok(mut guard) => *guard = gps_status,
                // Poisoned (a panic elsewhere while holding the lock): log
                // once per occurrence rather than silently skipping the
                // refresh, so a stuck/stale QUERY_STATUS GPS field is
                // diagnosable from the boot log rather than a silent gap.
                Err(e) => {
                    log::warn!("GPS_STATUS mutex poisoned — admin_server will see stale GPS data");
                    *e.into_inner() = gps_status;
                }
            }
            if firmware_core::ui::ui_task_boundary::changed_on_send(&mut last_sent_gps_status, gps_status) {
                send_ui_event(&evt_tx, &mut evt_dropped, ui::UiEvent::GpsStatusChanged(gps_status));
            }

            // Refresh the shared room-clock-provenance snapshot — same
            // cross-thread mutex pattern as GPS_STATUS immediately above.
            match ROOM_CLOCK_SOURCE.lock() {
                Ok(mut guard) => *guard = room_clock_source,
                Err(e) => {
                    log::warn!(
                        "ROOM_CLOCK_SOURCE mutex poisoned — stale room clock provenance may be served"
                    );
                    *e.into_inner() = room_clock_source;
                }
            }
            // `meshcadet-room-clock-ux`: the GPS status screen's Time-sync
            // row surfaces this provenance directly (`UiRuntime::set_room_
            // clock_source`) — "why does this say no fix but the time is
            // right?" now has a visible answer. The relative-age half of
            // that row mirrors whichever source is actually active:
            // `GpsStatus::clock_sync_age_secs` while GPS (verified OR
            // unverified — both tick off the same driver-side sync anchor,
            // see `gps::GpsDriver::status`'s doc) is synced (the row already
            // read this before this mission), or `AdoptedServerClock::
            // age_secs` once a room server's clock has been adopted instead
            // — both answer the same "how long ago did THIS source last
            // confirm the time" question, just for different sources.
            let room_clock_age_secs = match room_clock_source {
                room_session::ClockSource::Gps | room_session::ClockSource::GpsUnverified => {
                    gps_status.clock_sync_age_secs
                }
                room_session::ClockSource::RoomServer => {
                    adopted_server_clock.map(|c| c.age_secs(now)).unwrap_or(0)
                }
                room_session::ClockSource::None => 0,
            };
            let room_clock_snapshot = (room_clock_source, room_wall_clock_secs, room_clock_age_secs);
            if firmware_core::ui::ui_task_boundary::changed_on_send(&mut last_sent_room_clock, room_clock_snapshot) {
                send_ui_event(&evt_tx, &mut evt_dropped, ui::UiEvent::RoomClockChanged {
                    source: room_clock_source,
                    wall_clock_secs: room_wall_clock_secs,
                    age_secs: room_clock_age_secs,
                });
            }
        }

        // Refresh the shared battery status snapshot — same cross-thread
        // mutex pattern as GPS_STATUS immediately above (touch UI fed
        // directly; admin_server reads BATTERY_STATUS from its own thread).
        #[cfg(not(feature = "hil"))]
        {
            let battery_status = battery.status();
            match BATTERY_STATUS.lock() {
                Ok(mut guard) => *guard = battery_status,
                Err(e) => {
                    log::warn!("BATTERY_STATUS mutex poisoned — admin_server will see stale battery data");
                    *e.into_inner() = battery_status;
                }
            }
            if firmware_core::ui::ui_task_boundary::changed_on_send(&mut last_sent_battery_status, battery_status) {
                send_ui_event(&evt_tx, &mut evt_dropped, ui::UiEvent::BatteryStatusChanged(battery_status));
            }
        }

        // Refresh the signal-meter reading — no cross-thread mutex needed
        // (unlike GPS/battery above): the tracker is local dispatcher-loop
        // state, read only by this same thread's UI push, so this runs in
        // every build (not gated on `not(feature = "hil")`). `level(now)`
        // recomputes the tracker's max-with-decay reading fresh every
        // iteration (see `SignalTracker::level`'s doc); pushing it is what
        // lets the four operational screens' meter age down live even with
        // no further packets arriving. `UiRuntime::set_signal_level` no-ops
        // routing the value to a screen that has no meter (splash,
        // unprovisioned, pin_entry, admin_menu — ADR-0010 D5).
        {
            let level = signal_tracker.level(now);
            if firmware_core::ui::ui_task_boundary::changed_on_send(&mut last_sent_signal_level, level) {
                send_ui_event(&evt_tx, &mut evt_dropped, ui::UiEvent::SignalLevelChanged(level));
            }
        }

        // ── Enqueue periodic TEST DM (HIL only) ──────────────────────────────
        #[cfg(feature = "hil")]
        if now.saturating_sub(last_tx_ms) >= TX_INTERVAL_MS {
            if let Some((n, ack)) =
                build_test_dm(now, tx_epoch_base, &identity, &peer_pubkey, &shared_secret, &mut frame_buf)
            {
                log_tx_queue_eviction(
                    txq.enqueue(&frame_buf[..n], Some(ack)),
                    "test DM",
                    &mut outstanding,
                    |ev| send_ui_event(&evt_tx, &mut evt_dropped, ev),
                );
                insert_outstanding(
                    &mut outstanding,
                    OutstandingSend::new(
                        ack,
                        OutstandingKind::Dm { to_hash: peer_pubkey[0] },
                        now,
                        &frame_buf[..n],
                    ),
                    |ev| send_ui_event(&evt_tx, &mut evt_dropped, ev),
                );
                log::debug!(
                    "dispatcher: enqueued TEST DM ({} bytes), expecting ack {}",
                    n,
                    hex4(&ack),
                );
            }
            last_tx_ms = now;
        }

        // ── Room keep-alive scheduler (Phase C) ───────────────────────────────
        //
        // TWO INDEPENDENT cadences per provisioned, logged-in room —
        // `meshcadet-room-reflood-login-backoff`'s fix (FINDING B). Before
        // this fix both branches below shared ONE gate
        // (`room_keep_alive_interval_ms`), which meant a room whose server
        // never answers (offline/out-of-range/decommissioned) re-flooded a
        // full `ANON_REQ` login every `ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS`
        // (15 s) FOREVER — that gate never relaxes to the routine cadence
        // for such a room, because its only relaxer
        // (`RoomSyncPhase::on_keep_alive_ack`) needs an ACK that will never
        // arrive. A flood frame is rebroadcast by every relaying node in the
        // mesh, so an unbounded 15 s cadence is an airtime/regulatory-duty-
        // cycle defect, not merely a battery one. See
        // `room_session::room_reflood_interval_ms`'s doc for the full
        // rationale.
        //   - `!has_route()`: no learned route — re-flood the login on
        //     its OWN, backed-off cadence (`room_reflood_interval_ms`),
        //     never on the drain/routine cadence below.
        //   - otherwise: route-direct keep-alive on the pre-existing
        //     drain/routine cadence (`room_keep_alive_interval_ms`,
        //     unchanged by this fix) — consuming the ACK's appended
        //     unsynced-count byte closes Phase D's drain window (see
        //     `handle_ack`). A flood-routed keep-alive is a no-op the server
        //     ignores outright (`MyMesh.cpp:536`), so this branch must never
        //     attempt one.
        // `room_runtime` is empty under `hil` (no rooms there), so this is a
        // no-op in that build, exactly like the boot-time login loop above.
        #[cfg(not(feature = "hil"))]
        for room in room_runtime.iter_mut() {
            if !room.login_sent {
                continue; // login not even queued yet this boot
            }

            // Periodic, event-independent drain-window re-evaluation —
            // `meshcadet-room-drain-window-out-path-never-learned-fix`. Runs
            // every scheduler pass for every room, BEFORE either cadence
            // branch below, regardless of `out_path_len`: a room whose
            // `out_path` is never learned at all never sends a keep-alive,
            // so neither `on_post_received` (needs a post to arrive) nor the
            // keep-alive-stall detector's `note_closer_failed` (needs a
            // learned route to ever tick) can re-evaluate
            // `DRAIN_WINDOW_STALL_TIMEOUT_MS` on their own — a session that
            // absorbs exactly one post and no successor would otherwise sit
            // with that post's notification lost forever. See
            // `room_session::RoomSyncPhase::on_scheduler_tick`'s doc for the
            // full history of this failure mode.
            if let Some(room_session::RoomNotification::Aggregate { count }) =
                room.sync_phase.on_scheduler_tick(now)
            {
                log::info!(
                    "room: 0x{:02x} drain window force-closed by the scheduler's own \
                     periodic tick — firing one aggregate notification for {} post(s) \
                     absorbed while draining",
                    room.hash, count,
                );
                send_ui_event(
                    &evt_tx, &mut evt_dropped,
                    ui::UiEvent::RoomDrainComplete { room_hash: room.hash, count },
                );
            }

            // `has_route()`, never `out_path_len == 0`
            // (`meshcadet-room-notify-suppression-full-enumeration-fix`): a
            // ZERO-HOP learned route — the room server is this device's
            // direct radio neighbour, the ordinary bench topology — is a
            // real, usable route whose `out_path_len` is legitimately 0.
            // Reading the hop count as "no route" made this branch fire
            // forever on such a room, so no keep-alive was ever sent, so the
            // drain window's normal closer never ran and every inbound post
            // was silently absorbed. See
            // `room_session::PersistedRoomSession::route_known`'s doc.
            if !room.session.has_route() {
                // Decoupled reflood cadence — deliberately does NOT read
                // `ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS`,
                // `room.sync_phase.is_draining()`, or
                // `room.last_keep_alive_ms`: see this block's doc above and
                // `room_reflood_interval_ms`'s for why re-coupling this to
                // the drain/routine gate is exactly the regression this
                // mission fixes.
                let interval = room_session::room_reflood_interval_ms(
                    room.reflood_attempts,
                    ROOM_REFLOOD_INITIAL_BACKOFF_MS,
                    ROOM_REFLOOD_BACKOFF_CEILING_MS,
                );
                if now.saturating_sub(room.last_reflood_ms) < interval {
                    continue;
                }
                let ts =
                    room_session::room_tx_timestamp(room_wall_clock_secs, room.session.last_room_ts);
                let shared = identity.ecdh_shared_secret(&room.pubkey);
                let mut frame = [0u8; room_session::MAX_LOGIN_FRAME_LEN];
                let n = room_session::encode_room_login_frame(
                    &shared,
                    room.hash,
                    &identity.pubkey,
                    ts,
                    room.session.sync_since,
                    &room.guest_password[..room.guest_password_len as usize],
                    &mut frame,
                );
                log_tx_queue_eviction(
                    txq.enqueue(&frame[..n], None),
                    "room re-flood login",
                    &mut outstanding,
                    |ev| send_ui_event(&evt_tx, &mut evt_dropped, ev),
                );
                room.session.record_sent_timestamp(ts);
                // `meshcadet-room-ts-watermark-write-behind`: same
                // write-through as the boot login above — a reflood is
                // exactly the "route was lost" case where losing the
                // in-RAM-only watermark to a reboot matters most.
                room_session::save_room_session(
                    nvs_partition.clone(),
                    room.hash,
                    room.session_epoch,
                    &room.session,
                );
                room.last_reflood_ms = now;
                room.reflood_attempts = room.reflood_attempts.saturating_add(1);
                log::info!(
                    "room: 0x{:02x} has no learned out_path — re-flooding login \
                     (attempt {}, next retry in {}ms if unanswered)",
                    room.hash,
                    room.reflood_attempts,
                    room_session::room_reflood_interval_ms(
                        room.reflood_attempts,
                        ROOM_REFLOOD_INITIAL_BACKOFF_MS,
                        ROOM_REFLOOD_BACKOFF_CEILING_MS,
                    ),
                );
                continue;
            }

            // Route known: route-direct keep-alive, gated by the pre-
            // existing three-cadence schedule (first-tick/draining/
            // routine) — see `room_session::room_keep_alive_interval_ms`'s
            // doc; unchanged by this mission's fix.
            let interval = room_session::room_keep_alive_interval_ms(
                room.last_keep_alive_ms,
                room.sync_phase.is_draining(),
                ROOM_FIRST_KEEP_ALIVE_DELAY_MS,
                ROOM_DRAINING_KEEP_ALIVE_INTERVAL_MS,
                ROOM_KEEP_ALIVE_INTERVAL_MS,
            );
            if now.saturating_sub(room.last_keep_alive_ms) < interval {
                continue;
            }
            room.last_keep_alive_ms = now;
            let ts = room_session::room_tx_timestamp(room_wall_clock_secs, room.session.last_room_ts);
            let shared = identity.ecdh_shared_secret(&room.pubkey);

            // Reconnect-stall detector: BEFORE possibly overwriting
            // `pending_keep_alive_ack` below, a still-`Some` value left over
            // from the PRIOR tick means that keep-alive was never ACKed —
            // feed it to `RoomKeepAliveStall`, which counts consecutive
            // misses and — once it decides the route is dead, not just one
            // dropped frame — zeroes `out_path_len` itself. The re-flood
            // branch above picks that up on the very next scheduler pass,
            // on its own decoupled cadence (see the `ROOM_REFLOOD_INITIAL_
            // BACKOFF_MS`-vs-detection-floor const assertion above this
            // loop's constants for why "next pass" is effectively
            // immediate, same as the pre-fix same-tick fallthrough was).
            // See `firmware_core::room_session::RoomKeepAliveStall`'s doc
            // for the full "why N misses, why zero out_path" rationale.
            // Short of that threshold, the overwrite below is now GUARDED
            // (`keep_alive_tick_should_send`, just past this block) — a
            // still-outstanding-but-tolerated miss must not discard the
            // prior keep-alive's expected `ack_hash` before a legitimately
            // late reply can still match it.
            let ack_outstanding = room.pending_keep_alive_ack.is_some();
            if ack_outstanding {
                let invalidated = room.keep_alive_stall.on_tick(true, &mut room.session);
                if invalidated {
                    room.pending_keep_alive_ack = None;
                    // `meshcadet-room-post-no-notification`: this
                    // invalidation IS the stronger evidence
                    // `RoomSyncPhase::note_closer_failed`'s doc describes —
                    // feed it in now rather than leaving the drain window's
                    // independent 5-minute stall bound to find out the same
                    // thing on its own, far later, with no reflood-backoff
                    // escalation to justify the wait (this room's LOGIN
                    // keeps succeeding, which is exactly the case that
                    // resets the backoff to its floor every cycle).
                    //
                    // `meshcadet-room-post-still-no-notify-hil`: unlike the
                    // per-post call sites (`handle_ack`, `handle_room_push_
                    // frame`), THIS call site carries no post of its own —
                    // it fires from the keep-alive scheduler, possibly with
                    // no post pending at all. `note_closer_failed` now
                    // returns `Some(Aggregate)` directly whenever a backlog
                    // was already silently absorbed at the moment the
                    // closer is confirmed dead, so that backlog gets its
                    // badge/tone/blink right here instead of waiting on a
                    // next post that may never arrive (the HIL capture that
                    // motivates this: a single test post, no follow-up
                    // ever sent).
                    if let Some(room_session::RoomNotification::Aggregate { count }) =
                        room.sync_phase.note_closer_failed()
                    {
                        log::info!(
                            "room: 0x{:02x} drain window closed — firing one aggregate \
                             notification for {} post(s) absorbed while draining",
                            room.hash, count,
                        );
                        // This scheduler loop isn't already collecting into a
                        // `ui_events` buffer (unlike `on_receive`'s call
                        // sites) — send directly, same as `RoomPostSent`/
                        // `RoomPostRefused` further down this same loop.
                        send_ui_event(
                            &evt_tx, &mut evt_dropped,
                            ui::UiEvent::RoomDrainComplete { room_hash: room.hash, count },
                        );
                    }
                    log::warn!(
                        "room: 0x{:02x} exceeded {} consecutive missed keep-alive ACKs — \
                         invalidating out_path to force a relearn",
                        room.hash, room_session::KEEP_ALIVE_STALL_THRESHOLD,
                    );
                    room_session::save_room_session(
                        nvs_partition.clone(),
                        room.hash,
                        room.session_epoch,
                        &room.session,
                    );
                    continue; // no learned path left this tick; reflood branch picks it up next pass
                } else {
                    log::warn!(
                        "room: 0x{:02x} missed keep-alive ACK ({}/{})",
                        room.hash, room.keep_alive_stall.missed(), room_session::KEEP_ALIVE_STALL_THRESHOLD,
                    );
                }
            }
            // `meshcadet-room-keepalive-ack-overwritten-before-reply-window`:
            // the miss streak is still within `RoomKeepAliveStall`'s
            // tolerance (or nothing was outstanding at all) — but a still-
            // outstanding prior ack must NOT be clobbered by a fresh send.
            // `room.pending_keep_alive_ack` is a single `Option<[u8; 4]>`
            // slot; encoding-and-sending a new keep-alive below overwrites
            // it with the NEW frame's expected hash, discarding the PRIOR
            // one before a legitimately late (but still valid) reply can
            // possibly match it — the exact defect this mission fixes. Wait
            // for the outstanding ack (or for `RoomKeepAliveStall` to give
            // up on it above) instead. See
            // `room_session::keep_alive_tick_should_send`'s doc.
            if !room_session::keep_alive_tick_should_send(ack_outstanding) {
                continue;
            }

            // Resumed keep-alive after a (re)login: force_since re-affirms
            // `sync_since` explicitly rather than relying solely on the
            // login reply — see `resync_pending`'s doc. Routine ticks pass 0
            // (no override).
            let force_since = if room.resync_pending {
                room.session.sync_since
            } else {
                0
            };

            let mut frame = [0u8; room_session::MAX_KEEP_ALIVE_FRAME_LEN];
            match room_session::encode_room_keep_alive_frame(
                &shared,
                room.hash,
                identity.pub_hash(),
                &room.session.out_path[..room.session.out_path_len as usize],
                ts,
                force_since,
                &mut frame,
            ) {
                Some(n) => {
                    log_tx_queue_eviction(
                        txq.enqueue(&frame[..n], None),
                        "room keep-alive",
                        &mut outstanding,
                        |ev| send_ui_event(&evt_tx, &mut evt_dropped, ev),
                    );
                    room.pending_keep_alive_ack = Some(protocol::room::keep_alive_ack_hash(
                        ts,
                        force_since,
                        &identity.pubkey,
                    ));
                    room.session.record_sent_timestamp(ts);
                    // `meshcadet-room-ts-watermark-write-behind`: this is
                    // the routine cadence — the ONE call site that used to
                    // leave `last_room_ts` unpersisted for an entire healthy
                    // room's remaining uptime (login reply, inbound push,
                    // and stall-invalidation are all rarer than every
                    // keep-alive). Same write-through as the two login send
                    // sites above.
                    room_session::save_room_session(
                        nvs_partition.clone(),
                        room.hash,
                        room.session_epoch,
                        &room.session,
                    );
                    room.resync_pending = false;
                    log::info!(
                        "room: TX route-direct keep-alive for 0x{:02x} (ts={}, force_since={})",
                        room.hash, ts, force_since,
                    );
                }
                None => {
                    // Can only happen if out_path_len somehow exceeds 63
                    // hops — defensive; out_path_len is bounded by
                    // `apply_login_outcome`'s own `.min` clamp.
                    log::warn!(
                        "room: keep-alive encode failed for 0x{:02x} despite a learned out_path",
                        room.hash,
                    );
                }
            }
        }

        // ── CAD + TX ─────────────────────────────────────────────────────────
        // `now < cad_backoff_until_ms` skips the CAD attempt entirely while a
        // prior busy result is still being backed off from (see
        // `cad_backoff_until_ms`'s doc above) — this replaces what used to be
        // a blocking `FreeRtos::delay_ms(backoff_ms)` here. The loop still
        // falls through to RX poll and `ui.step()` every iteration during the
        // gate instead of stalling the whole thread.
        if txq.has_pending() && now >= cad_backoff_until_ms {
            // Timed narrowly around the CAD call itself (not the whole
            // `if txq.has_pending() ...` block, which also does bookkeeping
            // that isn't "CAD" and only sometimes runs at all) — see
            // `perf::PerfRollup::cad`'s doc for what this feeds.
            #[cfg(feature = "diagnostics")]
            let cad_t0 = uptime_us();
            let clear_to_send = match radio.channel_activity_detection() {
                Ok(busy) => {
                    cad_err_streak = 0;
                    Some(!busy)
                }
                Err(e) => {
                    cad_err_streak += 1;
                    if cad_err_streak >= CAD_FAIL_LIMIT {
                        log::warn!(
                            "CAD error: {:?} ({}x consecutive) — transmitting without LBT",
                            e, cad_err_streak,
                        );
                        cad_err_streak = 0;
                        Some(true)
                    } else {
                        log::warn!("CAD error: {:?} ({}x)", e, cad_err_streak);
                        None
                    }
                }
            };
            #[cfg(feature = "diagnostics")]
            perf_rollup.cad.record((uptime_us().saturating_sub(cad_t0)) as u32);

            match clear_to_send {
                Some(false) => {
                    let backoff_ms = 1000u64 + (identity.pub_hash() as u64 % 2000);
                    log::debug!(
                        "CAD: channel busy, deferring retry {}ms (non-blocking — \
                         RX/UI keep running)",
                        backoff_ms,
                    );
                    cad_backoff_until_ms = now + backoff_ms;
                }
                Some(true) => {
                    // `peek` (not `take`): a transient failure below — a
                    // radio.transmit() error, or the airtime budget denying
                    // this exact frame — must leave the frame IN the queue
                    // for the next iteration to retry. The old `take` pulled
                    // the frame out unconditionally, so either failure mode
                    // discarded it permanently: a single dropped LoRa packet
                    // (or one attempt that lands mid-budget-window) was a
                    // silently lost message with no retry, matching the
                    // reported "sends once, sometimes never arrives" defect.
                    // Only `pop_front()` on the confirmed-`Ok` path below
                    // actually removes it.
                    let mut tx_frame = [0u8; 255];
                    let n = txq.peek(&mut tx_frame);
                    if n > 0 {
                        let payload_type = (tx_frame[0] >> 2) & 0x0F;
                        // RELEASE-LIVE policy enforcement (not a `debug_assert!`,
                        // which `[profile.release]` — root `Cargo.toml` — compiles
                        // to a no-op, since `debug-assertions` is not enabled
                        // there): "MeshCadet never emits an ADVERT" must hold in
                        // shipped firmware, not just debug builds.
                        // `PolicyFilter::is_advert_type` itself is unmodified; only
                        // this guard around it is new. A frame that ever reaches
                        // here with an ADVERT payload_type is a policy violation in
                        // the calling code (nothing legitimate enqueues one — see
                        // `admin_server::run`'s `FRAME_QUERY_ADVERT` handler, which
                        // replies over the provisioning serial link directly and
                        // never touches `txq`) — refuse to transmit it and drop the
                        // frame outright rather than retry it forever.
                        if !tx_guard_allows(payload_type) {
                            log::error!(
                                "policy violation: refusing to transmit an ADVERT frame \
                                 (0x{:02x}) — dropped, not retried",
                                payload_type,
                            );
                            txq.pop_front();
                        } else {
                            let required = lora_airtime_ms(n);
                            if budget.can_transmit(now, required) {
                                // Timed narrowly around `radio.transmit()`
                                // itself — this is the campaign's §2 "dominant
                                // finding" call site (full LoRa-airtime block,
                                // up to ~800 ms for a 255 B frame per
                                // `docs/perf/ui-perf-baseline.md` §4) — see
                                // `perf::PerfRollup::tx`'s doc.
                                #[cfg(feature = "diagnostics")]
                                let tx_t0 = uptime_us();
                                let tx_result = radio.transmit(&tx_frame[..n]);
                                #[cfg(feature = "diagnostics")]
                                perf_rollup.tx.record((uptime_us().saturating_sub(tx_t0)) as u32);
                                match tx_result {
                                    Ok(airtime) => {
                                        txq.pop_front();
                                        budget.record_tx(now, airtime);
                                        // Mark our own transmission as seen so a relay
                                        // flooding it back to us is dropped rather than
                                        // displayed as an inbound copy (MeshCore marks
                                        // its sends seen — Mesh.cpp:636). Keyed on
                                        // payload_type||payload, so the echo (same
                                        // payload, mutated path) matches.
                                        dedup.insert(&tx_frame[..n]);
                                        log::info!("TX: {} bytes, {}ms airtime", n, airtime);
                                    }
                                    Err(e) => {
                                        // Frame stays queued (no pop_front) — retried
                                        // next iteration. Back off like a CAD-busy
                                        // result so a persistent radio fault doesn't
                                        // hot-spin retrying the same frame every
                                        // ~5-50ms; a transient one (the common case)
                                        // just retries on the very next backoff-free
                                        // pass once the gate reopens.
                                        let backoff_ms = 1000u64 + (identity.pub_hash() as u64 % 2000);
                                        log::warn!(
                                            "TX error: {:?} — frame retained for retry in {}ms",
                                            e, backoff_ms,
                                        );
                                        cad_backoff_until_ms = now + backoff_ms;
                                    }
                                }
                            } else {
                                // Same reasoning as the TX-error arm: the frame is
                                // NOT dropped, only deferred. The airtime budget
                                // window slides forward every ms, so a short
                                // backoff is enough for `can_transmit` to clear on
                                // retry without hammering the check every loop
                                // iteration in the meantime.
                                let backoff_ms = 1000u64 + (identity.pub_hash() as u64 % 2000);
                                log::debug!(
                                    "TX deferred: airtime budget exhausted, retry in {}ms",
                                    backoff_ms,
                                );
                                cad_backoff_until_ms = now + backoff_ms;
                            }
                        }
                    }
                }
                None => {}
            }
        }

        // ── RX poll ──────────────────────────────────────────────────────────
        //
        // `rx_notice_gap_us` (diagnostics-only) is the elapsed time since
        // this loop last ENTERED this call — an honest-proxy upper bound on
        // "how long could a ready frame have sat before the dispatcher
        // noticed it", not a hardware RxDone-edge timestamp: `try_receive`
        // itself waits on DIO1 (interrupt/notification-driven since
        // `meshcadet-perf-radio-dio1-interrupt`, not a busy-poll) for up to
        // `RX_POLL_YIELD_MS`, but DIO1 latches high on RxDone and stays
        // latched until cleared (see
        // `Radio::try_receive`'s doc), so a frame that became ready during a
        // long CAD/TX block earlier THIS SAME iteration is already latched
        // by the time this call runs and returns almost instantly — hiding
        // exactly the delay this campaign cares about if measured only
        // within-call. The entry-to-entry gap captures that outer delay
        // instead: steady state (nothing blocking) it reads ≈
        // `RX_POLL_YIELD_MS`; it spikes to the CAD/TX block's own duration
        // whenever one runs first in the same iteration.
        #[cfg(feature = "diagnostics")]
        let rx_poll_t0 = uptime_us();
        #[cfg(feature = "diagnostics")]
        let rx_notice_gap_us = (rx_poll_t0.saturating_sub(last_rx_poll_entry_us)) as u32;
        #[cfg(feature = "diagnostics")]
        {
            last_rx_poll_entry_us = rx_poll_t0;
        }
        match radio.try_receive(&mut frame_buf, RX_POLL_YIELD_MS) {
            Ok(Some(n)) => {
                rx_done_count += 1;
                #[cfg(feature = "diagnostics")]
                perf_rollup.rx_notice.record(rx_notice_gap_us);
                if let Ok((rssi_raw, snr_raw)) = radio.get_packet_status() {
                    let rssi_dbm = -(rssi_raw as i32) / 2;
                    let snr_db   = (snr_raw as i32) / 4;
                    log::info!("RX RxDone: {} bytes, rssi={}dBm snr={}dB (raw {}/{})",
                               n, rssi_dbm, snr_db, rssi_raw, snr_raw);

                    // ── Signal-meter rx-tap (ADR-0010) ────────────────────────
                    // Record on EVERY RxDone, including a frame the dedup check
                    // right below is about to drop — a dedup'd duplicate from a
                    // repeater still proves it is audible right now (decision 6
                    // in the ADR), so this MUST run before that drop, not after.
                    // `frame_buf[1]` is the `path_len` byte (`n >= 2` guards the
                    // index — a frame this short is truncated garbage the rest
                    // of this match arm would reject anyway). `hop_count == 0`
                    // (a zero-hop, direct-from-origin packet) is filtered out
                    // here explicitly, mirroring the ADR's "hop >= 1" gate —
                    // `SignalTracker::record` would also no-op on it internally,
                    // but gating here avoids the call entirely on MeshCadet's
                    // single-hop-common-case traffic. `rssi_dbm`/`snr_db` are
                    // already decoded above; `now` is this dispatcher-loop
                    // iteration's real monotonic `esp_timer_get_time`-backed
                    // clock (`uptime_ms()`), never a loop-iteration counter.
                    if n >= 2 {
                        let hop_count = PathLen(frame_buf[1]).hop_count();
                        if hop_count >= 1 {
                            signal_tracker.record(rssi_dbm as i16, snr_db as i8, hop_count, now);
                            rx_diag!(
                                "signal-meter: recorded hop_count={} rssi={}dBm snr={}dB -> level={:?}",
                                hop_count, rssi_dbm, snr_db, signal_tracker.level(now),
                            );
                        }
                    }
                } else {
                    log::info!("RX RxDone: {} bytes (GetPacketStatus failed)", n);
                }
                if dedup.is_duplicate(&frame_buf[..n]) {
                    rx_diag!("RX: duplicate frame dropped ({} bytes)", n);
                    // A "duplicate" can be a genuine repeat of one of OUR OWN
                    // prior sends — the TX path above marks its own frame
                    // seen (`dedup.insert(&tx_frame[..n])`) precisely so a
                    // relay flooding it back dedups here instead of being
                    // re-displayed. If the repeated key matches our
                    // outstanding channel send, hearing it IS the implicit
                    // ack a GRP_TXT has no per-recipient delivery ACK for on
                    // the wire.
                    let key = packet_dedup_key(&frame_buf[..n]);
                    let mut ui_events: Vec<ui::UiEvent> = Vec::new();
                    let acked_channel = match_pending_channel_ack(key, &mut pending_channel_ack, &mut ui_events);
                    for ev in ui_events {
                        send_ui_event(&evt_tx, &mut evt_dropped, ev);
                    }
                    // Persist the flip to flash so it survives a power-cycle —
                    // the channel counterpart of the DM ack-state persistence
                    // fix below. Production builds only — HISTORY doesn't exist
                    // under `hil`.
                    #[cfg(not(feature = "hil"))]
                    if let Some(channel_hash) = acked_channel {
                        let mut guard = HISTORY.lock().expect("HISTORY mutex should not be poisoned");
                        if let Some(hs) = guard.as_mut() {
                            if let Err(e) = hs.mark_last_ours_acked(
                                protocol::history::HistoryMsgType::GrpTxt,
                                channel_hash,
                            ) {
                                log::warn!("channel ack: history persist failed: {:?}", e);
                            }
                        }
                    }
                    #[cfg(feature = "hil")]
                    let _ = acked_channel;
                } else {
                    dedup.insert(&frame_buf[..n]);
                    // Pre-fetch GPS + battery snapshots so the handler has them ready.
                    let gps_snapshot = gps.get_fix_and_age(now);
                    let battery_snapshot = battery.status();
                    {
                        let mut ui_events: Vec<ui::UiEvent> = Vec::new();
                        on_receive(
                            &frame_buf[..n],
                            &identity,
                            &policy,
                            channel_secret,
                            &mut outstanding,
                            &mut txq,
                            gps_snapshot,
                            battery_snapshot,
                            now,
                            tx_epoch_base,
                            &mut ui_events,
                            &mut room_runtime,
                            nvs_partition.clone(),
                            &contact_display_names,
                            gps_verified_now,
                            &mut adopted_server_clock,
                        );
                        // Persist any DM ack flip to flash so it survives a
                        // power-cycle — before this fix, ACK matching
                        // (reached from both a bare ACK frame and a bundled
                        // PATH-return ACK) only raised `UiEvent::DmAcked` for
                        // the live in-memory UI/radio state; the flash-side
                        // record `append_history` wrote at send time
                        // (`acked=false`) was never subsequently updated, so
                        // a reset between ack-receipt and any later history
                        // write lost the ack and the checkmark reverted to
                        // un-acked on reboot. Mirrors the
                        // channel counterpart's persistence block above.
                        // Production builds only — HISTORY doesn't exist
                        // under `hil`.
                        #[cfg(not(feature = "hil"))]
                        for ev in &ui_events {
                            // `is_channel` is irrelevant here: a room post's
                            // ack persists as `HistoryMsgType::Dm` too (see
                            // `SendRoomPost`'s own `append_history` call —
                            // rooms mirror a DM's history representation),
                            // so this persist step is correct for both
                            // shapes `DmAcked` now carries.
                            if let ui::UiEvent::DmAcked { to_hash, is_channel: _, ack_hash: _ } = ev {
                                let mut guard =
                                    HISTORY.lock().expect("HISTORY mutex should not be poisoned");
                                if let Some(hs) = guard.as_mut() {
                                    if let Err(e) = hs.mark_last_ours_acked(
                                        protocol::history::HistoryMsgType::Dm,
                                        *to_hash,
                                    ) {
                                        log::warn!("DM ack: history persist failed: {:?}", e);
                                    }
                                }
                            }
                        }
                        // Forward radio events to the UI task.
                        for ev in ui_events {
                            send_ui_event(&evt_tx, &mut evt_dropped, ev);
                        }
                    }
                }
            }
            Ok(None) => {
                rx_none_count += 1;
            }
            Err(radio::RadioError::CrcError) => {
                crc_err_count += 1;
                if let Ok((rssi_raw, snr_raw)) = radio.get_packet_status() {
                    let rssi_dbm = -(rssi_raw as i32) / 2;
                    let snr_db   = (snr_raw as i32) / 4;
                    rx_diag!("RX: CRC error — rssi={}dBm snr={}dB (raw {}/{})",
                             rssi_dbm, snr_db, rssi_raw, snr_raw);
                } else {
                    rx_diag!("RX: CRC error (GetPacketStatus failed)");
                }
            }
            Err(e) => log::warn!("RX error: {:?}", e),
        }
        #[cfg(feature = "diagnostics")]
        perf_rollup.rx_poll.record((uptime_us().saturating_sub(rx_poll_t0)) as u32);

        // ── Periodic RX stats + stack HWM ───────────────────────────────────
        if now.saturating_sub(last_rx_stats_ms) >= RX_STATS_INTERVAL_MS {
            log::info!(
                "RX stats ({}s): {} RxDone, {} CrcErr, {} none",
                RX_STATS_INTERVAL_MS / 1000,
                rx_done_count, crc_err_count, rx_none_count,
            );
            rx_done_count = 0;
            crc_err_count = 0;
            rx_none_count = 0;
            last_rx_stats_ms = now;

            // ADR-0012 C2: `send_ui_event`'s drop-and-count policy — logged
            // here, at `warn`, once per rollup window, rather than per
            // occurrence (capacity 32 against ≲2 events/iteration production
            // makes this a safety valve, not a design path — see that
            // function's doc).
            if evt_dropped > 0 {
                log::warn!(
                    "ui event queue: {} event(s) dropped (full/disconnected) in the last {}s",
                    evt_dropped, RX_STATS_INTERVAL_MS / 1000,
                );
                evt_dropped = 0;
            }

            // ── Main-task stack high-water mark (acceptance criterion) ────────
            //
            // uxTaskGetStackHighWaterMark(NULL) returns the minimum free stack
            // space remaining since the task started (includes the init path).
            // Logged every RX_STATS_INTERVAL_MS (30 s) to verify the headroom
            // after the stack-size increase to 49 152 B
            // (sdkconfig.defaults: CONFIG_ESP_MAIN_TASK_STACK_SIZE=49152 —
            // raised again, from 32 768 B, after a release-build settings-nav
            // stack overflow; see that fix's stack-
            // budget rationale comment for the full history). This periodic
            // sample can miss a stack-overflow reboot entirely if the task
            // resets before its next 30 s tick — see
            // `ui::mod::navigate_to_pin_entry`'s own unconditional HWM log at
            // the exact screen-swap transition an on-hardware
            // backtrace confirmed as the overflow site (`navigate_to_admin_menu`
            // carries the same log as secondary coverage for the next-densest
            // transition on the same "open Settings" path).
            //
            // If this log reads < 4096 B the budget should be re-evaluated.
            // A follow-on trim pass can lower the budget once HIL confirms
            // a stable margin over several boot cycles.
            {
                const MAIN_TASK_STACK_B: u32 = 49_152;
                log_thread_stack_hwm("main-task", MAIN_TASK_STACK_B);
            }

            // ── On-device perf rollup (diagnostics-only; M0 of
            // `meshcadet-perf-rearchitecture`) ────────────────────────────────
            //
            // Same 30 s cadence as the RX stats/stack-HWM block above — this
            // is the instrument the whole design measures against (M0):
            // per-phase superloop wall-clock min/mean/max/p95 (microseconds),
            // the UI-starvation counter, input-to-first-paint latency, RX-
            // notice latency, and per-core utilization. No behavior change —
            // every number below is read from timers/counters this same
            // build already carries; nothing here alters scheduling, radio,
            // or UI behavior.
            #[cfg(feature = "diagnostics")]
            {
                // ADR-0012 D9 row 10: `ui_step` dropped from this task's
                // phases — this dispatcher no longer calls `ui.step()` at
                // all (see the removed "PERF ui-starvation" log below); the
                // phase moves to `ui_task`'s own periodic rollup instead.
                let phases: [(&str, &perf::PhaseStats); 5] = [
                    ("gps", &perf_rollup.gps),
                    ("battery", &perf_rollup.battery),
                    ("cad", &perf_rollup.cad),
                    ("tx", &perf_rollup.tx),
                    ("rx_poll", &perf_rollup.rx_poll),
                ];
                for (name, stats) in phases {
                    let snap = stats.snapshot();
                    log::info!(
                        "PERF phase={}: n={} min={}us mean={}us max={}us p95={}us",
                        name, snap.count, snap.min, snap.mean, snap.max, snap.p95,
                    );
                }
                let rx_notice = perf_rollup.rx_notice.snapshot();
                log::info!(
                    "PERF rx-notice-latency: n={} min={}us mean={}us max={}us p95={}us",
                    rx_notice.count, rx_notice.min, rx_notice.mean, rx_notice.max, rx_notice.p95,
                );
                // ADR-0012 D9 row 10: `ui_step` and input-to-first-paint move
                // to `ui_task`'s own periodic rollup (it now owns the loop
                // those measured) — this task no longer calls `ui.step()` at
                // all, so "ui-starvation" (how long THIS loop went without
                // servicing the UI) is no longer a coherent measurement here;
                // the whole structural gap it tracked is what this split
                // removes. Both lines are dropped rather than logged as
                // permanently-zero.

                // Per-core utilization via FreeRTOS run-time stats
                // (`CONFIG_FREERTOS_GENERATE_RUN_TIME_STATS` +
                // `CONFIG_FREERTOS_USE_TRACE_FACILITY` +
                // `CONFIG_FREERTOS_USE_STATS_FORMATTING_FUNCTIONS` — see
                // `sdkconfig.defaults`). 1024 B comfortably covers this
                // firmware's small task count (main, admin_server, the two
                // FreeRTOS idle tasks, the timer service task, …) at the
                // ~40 B/task `vTaskGetRunTimeStats` needs — heap-allocated,
                // not a `run()`-stack local, same stack-budget reasoning as
                // `perf_rollup`'s own `Box` above.
                let mut stats_buf = vec![0u8; 1024];
                unsafe {
                    esp_idf_svc::sys::vTaskGetRunTimeStats(
                        stats_buf.as_mut_ptr() as *mut core::ffi::c_char,
                    );
                }
                let stats_text = std::ffi::CStr::from_bytes_until_nul(&stats_buf)
                    .ok()
                    .and_then(|c| c.to_str().ok())
                    .unwrap_or("");
                let (core0_pct, core1_pct) = perf::per_core_utilization_pct(stats_text);
                log::info!(
                    "PERF core-utilization: core0={} core1={}",
                    core0_pct.map(|p| p.to_string()).unwrap_or_else(|| "n/a".into()),
                    core1_pct.map(|p| p.to_string()).unwrap_or_else(|| "n/a".into()),
                );

                // ── Free internal-heap headroom (ADR-0012 D-H,
                // `ui-perf-baseline.md` §9.2) ───────────────────────────────
                //
                // `MALLOC_CAP_INTERNAL`, deliberately, not total free heap:
                // this firmware also has PSRAM backing some allocations, and
                // PSRAM availability does not answer whether the split's
                // +32 768 B of additional task stack (carved from the same
                // internal SRAM every other internal-only allocation
                // competes for) left adequate headroom. A total-heap number
                // here would silently paper over an internal-only squeeze.
                //
                // Two readings, not one: `free` is the instantaneous value
                // at this 30 s tick, which can miss a transient squeeze that
                // recovered before the next sample. `min_ever` is
                // `heap_caps_get_minimum_free_size`'s own lifetime-low-water
                // mark for this capability since boot — free from ESP-IDF,
                // no extra state to carry across this block's own per-window
                // reset below (unlike `perf_rollup`, this number is not
                // windowed; it never resets).
                let heap_free_internal = unsafe {
                    esp_idf_svc::sys::heap_caps_get_free_size(
                        esp_idf_svc::sys::MALLOC_CAP_INTERNAL,
                    )
                };
                let heap_min_ever_internal = unsafe {
                    esp_idf_svc::sys::heap_caps_get_minimum_free_size(
                        esp_idf_svc::sys::MALLOC_CAP_INTERNAL,
                    )
                };
                log::info!(
                    "PERF heap-internal: free={} min_ever={}",
                    heap_free_internal, heap_min_ever_internal,
                );

                // Reset every accumulator for the next window — same
                // "assign fresh state" reset idiom as `rx_done_count = 0;`
                // etc. above, just for the boxed aggregate.
                *perf_rollup = perf::PerfRollup::new();
            }
        }

        // ── UI command drain (ADR-0012 C7) ────────────────────────────────────
        //
        // `ui.step()`/`ui.drain_commands()` are GONE from this task entirely
        // — the UI now runs on its own core-1-pinned `ui_task`, stepping
        // itself continuously (see `ui_task`'s module doc). This is exactly
        // the structural change the whole campaign targets: this dispatcher
        // loop no longer waits on UI work of any kind, at any point, in any
        // iteration. What remains here is C7's non-blocking drain of
        // UI-initiated commands, at exactly the point `drain_commands()` ran
        // pre-split — collected into an owned `Vec` first (not drained
        // inline) for the same reason the pre-split code did: the room-post
        // handling below calls `send_ui_event(...)` mid-loop to confirm or
        // refuse a send back to the UI (`UiEvent::RoomPostSent`/
        // `RoomPostRefused`), and mixing that with the receive loop below
        // would be needlessly tangled.
        {
            let mut cmds: Vec<ui::UiCommand> = Vec::new();
            while let Ok(cmd) = cmd_rx.try_recv() {
                cmds.push(cmd);
            }
            for cmd in cmds {
                match cmd {
                    ui::UiCommand::SendDm { to_hash, text } => {
                        // Resolve contact pubkey by 1-byte hash; unknown hashes are silently
                        // dropped — allowlist-only policy.
                        match policy.contact_pubkey(to_hash) {
                            None => log::warn!(
                                "UI send DM: unknown contact 0x{:02x} — not in allowlist",
                                to_hash,
                            ),
                            Some(contact_pubkey) => {
                                match build_ui_dm(
                                    now, tx_epoch_base, &identity,
                                    contact_pubkey, to_hash,
                                    text.as_bytes(), &mut frame_buf,
                                ) {
                                    Some((n, ack)) => {
                                        log_tx_queue_eviction(
                                            txq.enqueue(&frame_buf[..n], Some(ack)),
                                            "UI DM",
                                            &mut outstanding,
                                            |ev| send_ui_event(&evt_tx, &mut evt_dropped, ev),
                                        );
                                        insert_outstanding(
                                            &mut outstanding,
                                            OutstandingSend::new(
                                                ack,
                                                OutstandingKind::Dm { to_hash },
                                                now,
                                                &frame_buf[..n],
                                            ),
                                            |ev| send_ui_event(&evt_tx, &mut evt_dropped, ev),
                                        );
                                        // Tag the optimistic bubble
                                        // `on_send_message` already pushed
                                        // (UI-side, before the dispatcher
                                        // even saw this command) with the
                                        // hash now that it's known — see
                                        // `UiEvent::DmQueued`'s doc.
                                        send_ui_event(
                                            &evt_tx, &mut evt_dropped,
                                            ui::UiEvent::DmQueued { to_hash, ack_hash: ack },
                                        );
                                        log::info!(
                                            "TX UI DM to 0x{:02x}: {:?} ({} bytes)",
                                            to_hash, text, n,
                                        );
                                        // ── Persist to rotating history (outbound) ─────────
                                        // Mirrors handle_dm's append-on-receipt so a DM
                                        // conversation's region holds both directions.
                                        // `to_hash` is the conversation key (matches
                                        // `ui::UiRuntime.messages`'s map key for this
                                        // contact); is_ours=true distinguishes direction.
                                        // Only appended on successful frame encoding — a
                                        // failed send never reaches the wire, so it must
                                        // not appear in history either.
                                        // acked=false: the send has just been enqueued, no
                                        // ACK has arrived yet.
                                        #[cfg(not(feature = "hil"))]
                                        {
                                            let ts = tx_epoch_base.wrapping_add((now / 1000) as u32);
                                            append_history(
                                                to_hash,
                                                protocol::history::HistoryMsgType::Dm,
                                                ts,
                                                text.as_bytes(),
                                                true,
                                                false,
                                            );
                                        }
                                    }
                                    None => log::warn!("UI send DM: frame encoding failed"),
                                }
                            }
                        }
                    }
                    // No channel provisioned (`channel_secret == None`): drop
                    // any UI-initiated group send outright rather than
                    // computing a hash against a placeholder secret — see
                    // `meshcadet-grptxt-rx-open-on-published-test-channel-secret`.
                    ui::UiCommand::SendGroupMsg { channel_hash, text: _ } if channel_secret.is_none() => {
                        log::warn!(
                            "UI send GRP_TXT: no channel provisioned — dropped (channel_hash=0x{:02x})",
                            channel_hash,
                        );
                    }
                    ui::UiCommand::SendGroupMsg { channel_hash, text } => {
                        let channel_secret = channel_secret
                            .expect("guarded by the no-channel arm above");
                        // Only transmit on the provisioned channel; silently drop mismatches.
                        let expected_ch = channel_hash_var(channel_secret);
                        if channel_hash != expected_ch {
                            log::warn!(
                                "UI send GRP_TXT: channel_hash 0x{:02x} != provisioned 0x{:02x} — dropped",
                                channel_hash, expected_ch,
                            );
                        } else {
                            // Channel messages carry no per-sender addressing, so
                            // prepend our node name as MeshCore expects ("<name>: <msg>")
                            // — without it the companion cannot attribute the body.
                            let sender_name = device_sender_name(&identity, nvs_partition.clone());
                            let n = build_ui_grp_txt(
                                now, tx_epoch_base, channel_secret,
                                sender_name.as_bytes(), text.as_bytes(), &mut frame_buf,
                            );
                            log_tx_queue_eviction(
                                txq.enqueue(&frame_buf[..n], None),
                                "UI channel message",
                                &mut outstanding,
                                |ev| send_ui_event(&evt_tx, &mut evt_dropped, ev),
                            );
                            // Record the dedup key of this send so a later heard repeat
                            // (this exact frame flooded back into the mesh by another
                            // node) can be recognised as the implicit channel ack —
                            // see `match_pending_channel_ack`'s doc. Computed from the
                            // frame bytes directly, same key the dispatcher's dedup
                            // ring will key the eventual repeat on.
                            pending_channel_ack = Some(PendingChannelAck {
                                hash: packet_dedup_key(&frame_buf[..n]),
                                channel_hash,
                            });
                            log::info!(
                                "TX UI GRP_TXT ch=0x{:02x} as \"{}\": {:?} ({} bytes)",
                                channel_hash, sender_name, text, n,
                            );
                            // ── Persist to rotating history (outbound) ─────────────
                            // Mirrors handle_grp_txt's append-on-receipt so a channel
                            // conversation's region holds both directions. `channel_hash`
                            // is the conversation key (matches `ui::UiRuntime.messages`'s
                            // map key for this channel); is_ours=true distinguishes
                            // direction. Stored text is the body only (no "<name>: "
                            // prefix) — matches `on_send_message`'s own MessageRecord for
                            // is_ours=true sends, unlike the full "<name>: <msg>" text
                            // captured on inbound receipt. acked=false: matches
                            // `on_send_message`'s live UI default — a broadcast GRP_TXT
                            // has no per-message ACK on the wire, so this starts
                            // pending; `match_pending_channel_ack` flips it (both the
                            // live `MessageRecord` and, via `mark_last_ours_acked`, this
                            // very flash entry) on the first heard repeat, composing
                            // with the ack-state-persistence bit above.
                            #[cfg(not(feature = "hil"))]
                            {
                                let ts = tx_epoch_base.wrapping_add((now / 1000) as u32);
                                append_history(
                                    channel_hash,
                                    protocol::history::HistoryMsgType::GrpTxt,
                                    ts,
                                    text.as_bytes(),
                                    true,
                                    false,
                                );
                            }
                        }
                    }
                    ui::UiCommand::SendRoomPost { room_hash, text } => {
                        // Phase A: post send. `room_session::encode_room_post_checked`
                        // enforces the strictly-monotonic per-room timestamp
                        // invariant — a refusal is surfaced (logged, AND
                        // raised to the UI as `UiEvent::RoomPostRefused` —
                        // see `meshcadet-room-post-refusal-surface`'s
                        // Objective), never silently sent, since the server
                        // would either treat an equal timestamp as a retry
                        // (discarded, still ACKed) or a lesser one as an
                        // outright replay (no ACK at all).
                        #[cfg(not(feature = "hil"))]
                        match room_runtime.iter_mut().find(|r| r.hash == room_hash) {
                            None => log::warn!(
                                "UI send room post: unknown room 0x{:02x}",
                                room_hash,
                            ),
                            Some(room) => {
                                // Defense in depth (Phase B): the UI should
                                // already have refused to queue this for a
                                // read-only session (`on_send_message`'s own
                                // guard) — never trust a cross-module
                                // invariant with no local check too.
                                if !room.session.permission().can_post() {
                                    log::warn!(
                                        "UI send room post: room 0x{:02x} session is \
                                         read-only — dropped",
                                        room_hash,
                                    );
                                } else {
                                    // `meshcadet-room-monotonic-tx-timestamp`:
                                    // monotonic, never random, and never
                                    // above real wall-clock time while
                                    // untrusted — see `room_tx_timestamp`'s
                                    // doc. Always strictly greater than
                                    // `last_room_ts` by construction, so a
                                    // device with no GPS fix yet can still
                                    // post; `encode_room_post_checked`'s
                                    // `NonMonotonicTimestamp` refusal below
                                    // remains as defense-in-depth against a
                                    // genuine same-tick collision, not a
                                    // "clock not synced" gate anymore.
                                    let candidate_ts = room_session::room_tx_timestamp(
                                        room_wall_clock_secs,
                                        room.session.last_room_ts,
                                    );
                                    let shared = identity.ecdh_shared_secret(&room.pubkey);
                                    let last_room_ts = room.session.last_room_ts;
                                    match room_session::encode_room_post_checked(
                                        &shared,
                                        room.hash,
                                        identity.pub_hash(),
                                        candidate_ts,
                                        last_room_ts,
                                        text.as_bytes(),
                                        &identity.pubkey,
                                        &mut frame_buf,
                                    ) {
                                        Ok((n, ack)) => {
                                            log_tx_queue_eviction(
                                                txq.enqueue(&frame_buf[..n], Some(ack)),
                                                "room post",
                                                &mut outstanding,
                                                |ev| send_ui_event(&evt_tx, &mut evt_dropped, ev),
                                            );
                                            insert_outstanding(
                                                &mut outstanding,
                                                OutstandingSend::new(
                                                    ack,
                                                    OutstandingKind::RoomPost { room_hash: room.hash },
                                                    now,
                                                    &frame_buf[..n],
                                                ),
                                                |ev| send_ui_event(&evt_tx, &mut evt_dropped, ev),
                                            );
                                            room.session.record_sent_timestamp(candidate_ts);
                                            // `meshcadet-room-post-watermark-persist`: same
                                            // write-through as the boot/reflood login and
                                            // keep-alive sites (a56c7b7) — this room-post
                                            // site was the one that fix missed.
                                            room_session::save_room_session(
                                                nvs_partition.clone(),
                                                room.hash,
                                                room.session_epoch,
                                                &room.session,
                                            );
                                            log::info!(
                                                "TX room post to 0x{:02x}: {:?} ({} bytes)",
                                                room.hash, text, n,
                                            );
                                            // ── Persist to rotating history (outbound) ──
                                            // Mirrors SendDm's append-on-send — a room's
                                            // posts render through the exact same
                                            // hash-keyed history region a DM's do.
                                            //
                                            // `meshcadet-room-clock-ux`: NEVER `candidate_ts`
                                            // here — that's the wire nonce
                                            // (`room_session::room_tx_timestamp`'s
                                            // "monotonic anti-replay value, not a clock
                                            // reading" contract), and on a GPS-denied
                                            // device it can be nothing more than
                                            // `last_room_ts + 1`. Using it as a display
                                            // timestamp is exactly the bug this mission
                                            // fixes: our own posts rendering at a
                                            // fabricated date in our own thread while
                                            // every other client in the room sees the
                                            // server-re-stamped time fine (the server
                                            // always re-stamps on receipt — see
                                            // `MyMesh.cpp:41-51` — so only OUR echo of
                                            // OUR OWN send was ever wrong; an inbound
                                            // push's `post_ts` was always server-stamped
                                            // and unaffected). `room_wall_clock_secs` is
                                            // the actual trusted wall clock in effect
                                            // this tick (GPS, or an adopted room-server
                                            // clock — `room_session::trusted_wall_clock_
                                            // secs`); `TIMESTAMP_UNKNOWN` when neither has
                                            // ever synced, so a renderer shows "unknown"
                                            // rather than computing a false epoch date
                                            // (`host::history_format::format_local_
                                            // timestamp`).
                                            append_history(
                                                room.hash,
                                                protocol::history::HistoryMsgType::Dm,
                                                room_session::room_post_history_timestamp(
                                                    room_wall_clock_secs,
                                                ),
                                                text.as_bytes(),
                                                true,
                                                false,
                                            );
                                            // Confirm to the UI that this post actually
                                            // reached the wire — see
                                            // `ui::UiEvent::RoomPostSent`'s doc:
                                            // `on_send_message`'s room branch queues
                                            // this command WITHOUT rendering an
                                            // optimistic bubble first, precisely so a
                                            // refusal below never has one to retract.
                                            send_ui_event(&evt_tx, &mut evt_dropped, ui::UiEvent::RoomPostSent {
                                                room_hash: room.hash,
                                                text,
                                                ack_hash: ack,
                                            });
                                        }
                                        Err(e) => {
                                            // Phase A's non-negotiable: never send a
                                            // post the server's replay gate would
                                            // silently discard — surface it (logged)
                                            // rather than transmit. This mission's
                                            // Objective (`meshcadet-room-post-refusal-
                                            // surface`): logging alone left the user
                                            // with no explanation for a post that
                                            // simply never appeared — `RoomPostRefused`
                                            // tells them why, directly in the thread.
                                            //
                                            // `meshcadet-room-monotonic-tx-timestamp`,
                                            // Scope item 6: an unsynced clock is no
                                            // longer a valid reason a post is
                                            // refused — `candidate_ts` above is always
                                            // strictly greater than `last_room_ts` by
                                            // construction, synced or not. Reaching
                                            // this arm at all means the clock genuinely
                                            // hasn't advanced past this room's last send
                                            // yet (two sends within the same wall-clock
                                            // second, or a real clock regression) — the
                                            // one case the refusal path still exists for.
                                            let reason = "clock has not advanced since this room's \
                                                 last send — try again in a moment"
                                                .to_string();
                                            log::warn!(
                                                "UI send room post to 0x{:02x} refused: \
                                                 {:?} (gps_clock_verified={}, candidate_ts={}, \
                                                 last_room_ts={})",
                                                room.hash, e, gps_verified_now, candidate_ts, last_room_ts,
                                            );
                                            send_ui_event(&evt_tx, &mut evt_dropped, ui::UiEvent::RoomPostRefused {
                                                room_hash: room.hash,
                                                reason,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        #[cfg(feature = "hil")]
                        let _ = (room_hash, text);
                    }
                    // ADR-0012 C6: the UI never writes flash itself anymore
                    // — `UiRuntime::set_nvs_partition` and its four direct
                    // `runtime_settings_store::save` call sites are deleted;
                    // this dispatcher persists on the UI's behalf instead.
                    ui::UiCommand::PersistRuntimeSettings(settings) => {
                        #[cfg(not(feature = "hil"))]
                        {
                            if let Err(e) = runtime_settings_store::save(nvs_partition.clone(), &settings) {
                                log::error!(
                                    "runtime_settings_store: save failed (from UI command): {:?}",
                                    e,
                                );
                            }
                        }
                        // hil builds have no NVS-backed runtime_settings_store
                        // (see the module's `#[cfg]` at the top of this file)
                        // — unreachable in practice (hil never spawns
                        // `ui_task`, so no `PersistRuntimeSettings` command is
                        // ever produced), kept only so this arm type-checks
                        // under `--features hil`.
                        #[cfg(feature = "hil")]
                        let _ = settings;
                    }
                }
            }
        }
    }
}

// ── Frame builders ────────────────────────────────────────────────────────────

/// Build a flooded DM frame for the TEST contact and the ACK hash we expect back.
/// HIL only.
#[cfg(feature = "hil")]
fn build_test_dm(
    now_ms: u64,
    tx_epoch_base: u32,
    sender: &Identity,
    dest_pubkey: &[u8; 32],
    shared: &[u8; 32],
    out: &mut [u8; 255],
) -> Option<(usize, [u8; 4])> {
    let text = b"hi from meshcadet";
    let timestamp = tx_epoch_base.wrapping_add((now_ms / 1000) as u32);
    let type_byte: u8 = 0;

    let mut pt_buf = [0u8; 64];
    let pt_len = encode_txt_msg_plaintext(timestamp, 0, 0, text, &mut pt_buf);

    let header   = Header::new(RouteType::Flood, PayloadType::TxtMsg);
    let path_len = PathLen::new(2, 0)?;
    out[0] = header.0;
    out[1] = path_len.0;
    let payload_off = 2;

    let dm_len = encode_dm_payload(
        shared,
        dest_pubkey[0],
        sender.pub_hash(),
        &pt_buf[..pt_len],
        &mut out[payload_off..],
    );
    let expected_ack = compute_ack_hash(timestamp, type_byte, text, &sender.pubkey);
    Some((payload_off + dm_len, expected_ack))
}

/// Build an ACK frame: `[header(0x0D)] [path_len(0x40)] [ack_hash(4)]`.
/// The 4-byte hash is accepted by both v1.15 and v1.16 stock nodes (see
/// `compute_ack_hash`'s doc comment in `protocol/src/codec.rs`).
fn build_ack_frame(ack_hash: &[u8; 4], out: &mut [u8]) -> usize {
    let header = Header::new(RouteType::Flood, PayloadType::Ack);
    out[0] = header.0;
    out[1] = PathLen::new(2, 0).map(|p| p.0).unwrap_or(0x40);
    out[2..6].copy_from_slice(ack_hash);
    6
}

/// Map a resolved [`OutstandingSend`] to the `ui::UiEvent` that flips its
/// `MessageRecord` — `DmAcked` for a delivered send, `DmUndelivered` for one
/// whose deadline passed or whose frame was evicted before ever reaching the
/// wire. `OutstandingKind::RoomPost` maps to the SAME event shape as
/// `OutstandingKind::Dm`, just with `is_channel: true` and `to_hash` set to
/// the room's hash — see `ui::UiEvent::DmAcked`'s doc for why a room post
/// rides the DM event rather than a bespoke one.
fn delivery_event(send: OutstandingSend, delivered: bool) -> ui::UiEvent {
    let (to_hash, is_channel) = match send.kind {
        OutstandingKind::Dm { to_hash } => (to_hash, false),
        OutstandingKind::RoomPost { room_hash } => (room_hash, true),
    };
    if delivered {
        ui::UiEvent::DmAcked { to_hash, is_channel, ack_hash: send.ack_hash }
    } else {
        ui::UiEvent::DmUndelivered { to_hash, is_channel, ack_hash: send.ack_hash }
    }
}

/// Log the eviction `TxQueue::enqueue` just reported (if any) — a full queue
/// silently dropping the oldest pending frame is otherwise invisible: the
/// call site's own "TX ... queued" log fires unconditionally right after,
/// so without this, a dropped frame and a successfully queued one are
/// indistinguishable in the log. `what` names the frame that was just queued
/// (the caller's own "TX ... queued" wording), for context on what pushed the
/// queue over the edge.
///
/// If the evicted frame was tagged (a DM or room-post send `outstanding` was
/// tracking), resolves it against `outstanding` and hands the resulting
/// undelivered event to `emit` — a full TX queue silently dropping a frame
/// before it ever reaches the wire is exactly as terminal for that send as a
/// deadline timeout, so it must land in the same red/undelivered state
/// rather than being left pending forever (this mission's Objective). `emit`
/// is a closure rather than a fixed `&mut Vec<ui::UiEvent>` / `SyncSender`
/// pair so this one function serves both calling conventions this file
/// uses: `run()`'s own body sends straight to `evt_tx`, while the RX-handler
/// functions (`handle_dm`, `handle_req`, `handle_room_push_frame`) batch into
/// a `Vec<ui::UiEvent>` forwarded afterward.
fn log_tx_queue_eviction(
    dropped: Option<(usize, Option<[u8; 4]>)>,
    what: &str,
    outstanding: &mut OutstandingSends,
    mut emit: impl FnMut(ui::UiEvent),
) {
    let Some((dropped_len, tag)) = dropped else {
        return;
    };
    log::warn!(
        "TX queue full: dropped a pending {}-byte frame to make room for {}",
        dropped_len, what,
    );
    if let Some(send) = outstanding.resolve_evicted(tag) {
        log::warn!(
            "TX queue eviction dropped an outstanding {:?} send (ack {}) — marking undelivered",
            send.kind, hex4(&send.ack_hash),
        );
        emit(delivery_event(send, false));
    }
}

/// Record a freshly-enqueued DM/room-post ACKed send in `outstanding`. If the
/// table was already full, the oldest entry it evicted to make room is
/// resolved to undelivered and handed to `emit` — same "an entry bumped out
/// has just as definitively lost its chance to ever be matched" reasoning as
/// a TX-queue eviction (see `log_tx_queue_eviction`'s doc).
fn insert_outstanding(
    outstanding: &mut OutstandingSends,
    send: OutstandingSend,
    mut emit: impl FnMut(ui::UiEvent),
) {
    if let Some(evicted) = outstanding.insert(send) {
        log::warn!(
            "outstanding-sends table full: evicted oldest entry ({:?}, ack {}) to make room",
            evicted.kind, hex4(&evicted.ack_hash),
        );
        emit(delivery_event(evicted, false));
    }
}

// ── UI transmit builders ──────────────────────────────────────────────────────
//
// These functions build real wire frames for messages originated by the touch
// UI compose screen.  They mirror the HIL test builder above but accept
// dynamic destination hashes and text bodies rather than hardcoded constants.

/// Maximum UTF-8 byte count for a UI-composed message body.
///
/// Capped so the AES-ECB-padded plaintext (5-byte header + ≤120-byte text =
/// ≤125 bytes → ceil_16 = 128 bytes) keeps the DM / GRP_TXT frame well under
/// the 255-byte LoRa MTU.  Frame worst-case:
///   header(1) + path_len(1) + dest(1) + src(1) + MAC(2) + AES(128) = 134 B.
const MAX_UI_MSG_BYTES: usize = 120;

/// Build a flooded DM frame for a UI-initiated send.
///
/// # Arguments
/// * `dest_pubkey` — full 32-byte Ed25519 public key of the destination contact.
/// * `dest_hash`   — 1-byte routing hash (`dest_pubkey[0]`); pre-resolved via
///   [`PolicyFilter::contact_pubkey`] so the caller never reaches here with an
///   unknown contact.
/// * `text`        — UTF-8 message body; truncated to [`MAX_UI_MSG_BYTES`] bytes
///   if longer (no mid-codepoint awareness needed at the wire level).
///
/// Returns `(frame_len, expected_ack_hash)` or `None` if header encoding fails
/// (only possible if `PathLen::new` returns `None`, which never happens with the
/// fixed `(hash_size=2, hop_count=0)` parameters).
fn build_ui_dm(
    now_ms: u64,
    tx_epoch_base: u32,
    sender: &Identity,
    dest_pubkey: &[u8; 32],
    dest_hash: u8,
    text: &[u8],
    out: &mut [u8; 255],
) -> Option<(usize, [u8; 4])> {
    let timestamp = tx_epoch_base.wrapping_add((now_ms / 1000) as u32);
    let type_byte: u8 = 0;

    // Clamp to wire-safe length; preserves frame ≤ 255 B.
    let text = &text[..text.len().min(MAX_UI_MSG_BYTES)];

    let shared = sender.ecdh_shared_secret(dest_pubkey);
    // 128-byte buffer: 5-byte plaintext header + ≤120-byte text = ≤125 bytes.
    let mut pt_buf = [0u8; 128];
    let pt_len = encode_txt_msg_plaintext(timestamp, 0, 0, text, &mut pt_buf);

    let header   = Header::new(RouteType::Flood, PayloadType::TxtMsg);
    let path_len = PathLen::new(2, 0)?;
    out[0] = header.0;
    out[1] = path_len.0;
    let payload_off = 2;

    let dm_len = encode_dm_payload(
        &shared,
        dest_hash,
        sender.pub_hash(),
        &pt_buf[..pt_len],
        &mut out[payload_off..],
    );
    // ACK hash invariant: keyed on the SENDER's own pubkey (v1.15 §7.1).
    let expected_ack = compute_ack_hash(timestamp, type_byte, text, &sender.pubkey);
    Some((payload_off + dm_len, expected_ack))
}

/// The device's own channel-sender display name.
///
/// Reads the current name fresh from the identity store's `"name"` NVS key
/// (`identity_store::load_name`) on every call — the same store the host
/// CLI's `identity --set-name` writes via `SET_DEVICE_NAME`
/// (`admin_server.rs`). Doing the NVS read per-send (rather than caching an
/// `Identity`-derived copy at boot) is what makes a name change take effect
/// on the very next channel send with no reboot required, matching the
/// CLI/URL output which already reads live. Falls back to the
/// `MeshCadet-<HH>` pub_hash label when no name has been set (`name_len ==
/// 0`) or the NVS read fails — the same fallback convention as
/// `host/src/main.rs`'s contact-URI default.
#[cfg(not(feature = "hil"))]
fn device_sender_name(identity: &Identity, nvs_partition: EspDefaultNvsPartition) -> String {
    match identity_store::load_name(nvs_partition) {
        Ok((name, name_len)) if name_len > 0 => {
            String::from_utf8_lossy(&name[..name_len as usize]).into_owned()
        }
        Ok(_) => format!("MeshCadet-{:02X}", identity.pub_hash()),
        Err(e) => {
            log::warn!(
                "device_sender_name: identity_store::load_name failed: {:?} — using pub_hash fallback",
                e
            );
            format!("MeshCadet-{:02X}", identity.pub_hash())
        }
    }
}

/// HIL builds have no `identity_store` (fixed compiled seed, NVS untouched —
/// see the module gate at the top of this file) and never spawn `ui_task`
/// (`run()`'s "Touch UI" bring-up section is `#[cfg(not(feature = "hil"))]`),
/// so no `UiCommand::SendGroupMsg` is ever actually produced under `hil`;
/// this arm exists only so that match still type-checks under `--features
/// hil`. Mirrors the pre-fix behavior for this build config.
#[cfg(feature = "hil")]
fn device_sender_name(identity: &Identity, _nvs_partition: EspDefaultNvsPartition) -> String {
    format!("MeshCadet-{:02X}", identity.pub_hash())
}

/// Build a flooded GRP_TXT frame for a UI-initiated group send.
///
/// The channel is identified by the secret stored in `channel_secret`; the
/// single-byte channel hash is embedded in the frame by [`encode_grp_txt_var`].
/// The on-air text is `"<sender_name>: <body>"` (MeshCore channel convention,
/// `BaseChatMesh::sendGroupMessage`), and the body is truncated so the whole
/// prefixed text fits in [`MAX_UI_MSG_BYTES`] bytes.
fn build_ui_grp_txt(
    now_ms: u64,
    tx_epoch_base: u32,
    channel_secret: &[u8],
    sender_name: &[u8],
    text: &[u8],
    out: &mut [u8; 255],
) -> usize {
    let timestamp = tx_epoch_base.wrapping_add((now_ms / 1000) as u32);
    // Compose "<name>: <body>" then clamp the whole thing to the wire-safe cap.
    // Buffer: MAX_NAME_LEN(32) + delim(2) + MAX_UI_MSG_BYTES(120) = 154 ≤ 160.
    let mut text_buf = [0u8; 160];
    let composed = protocol::format_channel_text(sender_name, text, &mut text_buf);
    let composed = composed.min(MAX_UI_MSG_BYTES);
    let header = Header::new(RouteType::Flood, PayloadType::GrpTxt);
    out[0] = header.0;
    out[1] = PathLen::new(2, 0).map(|p| p.0).unwrap_or(0x40);
    let n = protocol::encode_grp_txt_var(channel_secret, timestamp, 0, 0, &text_buf[..composed], &mut out[2..]);
    2 + n
}

/// Build a telemetry reply DM addressed back to `dest_hash`.
///
/// The text body is either a location response or `loc:nofix` depending on
/// whether `gps_snapshot` carries a cached fix.  The DM is encrypted with the
/// ECDH shared secret derived from `contact_pubkey`.
///
/// Returns `(frame_len)` or `None` if encoding fails.
fn build_telemetry_reply(
    now_ms: u64,
    tx_epoch_base: u32,
    our_id: &Identity,
    contact_pubkey: &[u8; 32],
    gps_snapshot: Option<(i32, i32, u32)>, // (lat_e7, lon_e7, age_secs)
) -> Option<([u8; 255], usize)> {
    let shared = our_id.ecdh_shared_secret(contact_pubkey);
    let reply_ts = tx_epoch_base.wrapping_add((now_ms / 1000) as u32);

    // Encode the telemetry text body.
    let mut text_buf = [0u8; MAX_RESPONSE_LEN];
    let text_len = match gps_snapshot {
        Some((lat_e7, lon_e7, age_secs)) => {
            encode_telemetry_response(lat_e7, lon_e7, age_secs, &mut text_buf)
        }
        None => encode_no_fix_response(&mut text_buf),
    };

    // TXT_MSG plaintext: [ts(4)] [type(1)] [text...]
    let mut pt_buf = [0u8; 128];
    let pt_len = encode_txt_msg_plaintext(reply_ts, 0, 0, &text_buf[..text_len], &mut pt_buf);

    // DM payload: dest_hash = contact's pub_hash, src_hash = our pub_hash
    let dest_hash = contact_pubkey[0];
    let mut frame = [0u8; 255];
    frame[0] = Header::new(RouteType::Flood, PayloadType::TxtMsg).0;
    frame[1] = PathLen::new(2, 0)?.0;
    let payload_off = 2;

    let dm_len = encode_dm_payload(
        &shared,
        dest_hash,
        our_id.pub_hash(),
        &pt_buf[..pt_len],
        &mut frame[payload_off..],
    );
    Some((frame, payload_off + dm_len))
}

/// Build a MeshCore-native telemetry RESPONSE (`PAYLOAD_TYPE_RESPONSE`, 0x01)
/// addressed back to the requesting contact.
///
/// This answers the stock MeshCore companion app's telemetry/location button
/// (a `PAYLOAD_TYPE_REQ` with `REQ_TYPE_GET_TELEMETRY_DATA`) — the real on-air
/// request, as opposed to the bespoke `?loc` text DM that `build_telemetry_reply`
/// answers and that no companion actually sends.
///
/// The response plaintext is `[tag(4 LE)] [CayenneLPP GPS entry]? [CayenneLPP
/// battery entries]?`: the `tag` is reflected verbatim from the REQ so the
/// companion matches reply to request, a GPS entry is appended when a fix is
/// cached, and a battery percentage + charging-state entry pair is appended
/// from `battery` (the same [`battery::BatteryStatus`] the host `status`
/// command and the admin-menu screen read — see that module's docs).
/// Encrypted with the ECDH shared secret; dest = the contact, src = us.
/// Returns `(frame, len)` or `None` if the path-length field cannot be encoded.
fn build_telemetry_response(
    our_id: &Identity,
    contact_pubkey: &[u8; 32],
    tag: u32,
    gps: Option<(i32, i32)>, // (lat_e7, lon_e7)
    battery: battery::BatteryStatus,
) -> Option<([u8; 255], usize)> {
    let shared = our_id.ecdh_shared_secret(contact_pubkey);

    let mut pt_buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
    let pt_len = encode_telemetry_response_lpp(
        tag,
        gps,
        Some((battery.percent, battery.charging)),
        &mut pt_buf,
    );

    let dest_hash = contact_pubkey[0];
    let mut frame = [0u8; 255];
    frame[0] = Header::new(RouteType::Flood, PayloadType::Response).0;
    frame[1] = PathLen::new(2, 0)?.0;
    let payload_off = 2;

    let dm_len = encode_dm_payload(
        &shared,
        dest_hash,
        our_id.pub_hash(),
        &pt_buf[..pt_len],
        &mut frame[payload_off..],
    );
    Some((frame, payload_off + dm_len))
}

// ── Receive handler ───────────────────────────────────────────────────────────

/// Dispatch an inbound frame by header payload-type.
///
/// `gps_snapshot`: pre-fetched GPS fix and age from the GPS driver, passed
/// through to `handle_dm` for telemetry request handling.
///
/// `battery_snapshot`: pre-fetched battery status from the battery driver,
/// passed through to `handle_req` so the native telemetry RESPONSE carries
/// the same battery reading the host `status` command and admin-menu screen
/// show (single shared source — see `battery` module docs).
///
/// `contact_display_names` is only consulted on the `not(hil)` room-push leg
/// (there are no rooms under `hil`) — see `handle_ack`'s identical
/// `#[cfg_attr]` for why the `hil` build still needs the blanket allow.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "hil", allow(unused_variables))]
fn on_receive(
    frame: &[u8],
    our_id: &Identity,
    policy: &PolicyFilter,
    channel_secret: Option<&[u8]>,
    outstanding: &mut OutstandingSends,
    txq: &mut TxQueue,
    gps_snapshot: Option<(i32, i32, u32)>,
    battery_snapshot: battery::BatteryStatus,
    now_ms: u64,
    tx_epoch_base: u32,
    ui_events: &mut Vec<ui::UiEvent>,
    room_runtime: &mut [RoomRuntime],
    nvs_partition: EspDefaultNvsPartition,
    contact_display_names: &std::collections::HashMap<u8, String>,
    gps_verified: bool,
    adopted_server_clock: &mut Option<room_session::AdoptedServerClock>,
) {
    if frame.len() < 2 {
        rx_diag!("RX: frame too short ({} bytes)", frame.len());
        return;
    }

    let header_byte  = frame[0];
    let path_len_byte = frame[1];
    let hash_size  = ((path_len_byte >> 6) + 1) as usize;
    let hop_count  = (path_len_byte & 0x3F) as usize;
    let path_bytes = hop_count * hash_size;
    let payload_off = 2 + path_bytes;
    if frame.len() < payload_off {
        log::warn!("RX: frame shorter than encoded path ({} bytes)", frame.len());
        return;
    }
    let payload = &frame[payload_off..];

    let payload_type = (header_byte >> 2) & 0x0F;
    rx_diag!(
        "RX frame: {} bytes, hdr=0x{:02x}, payload_type=0x{:02x}, hops={}, payload={}B",
        frame.len(), header_byte, payload_type, hop_count, payload.len(),
    );

    match payload_type {
        x if x == PayloadType::TxtMsg as u8 => {
            // A room contact's pushed posts arrive as ordinary TXT_MSG DMs
            // (`TXT_TYPE_SIGNED_PLAIN`, not `TXT_TYPE_PLAIN`) — route by
            // src_hash to the room-push handler instead of `handle_dm`,
            // which assumes the plain-DM flag layout and would mis-parse a
            // push's leading author-pubkey prefix as text. `raw_src` is only
            // a 1-byte hash (1/256 collision odds per room/contact pair), so
            // `handle_room_push_frame` can decline (a `NotSignedPlain`
            // decode — see its doc) and hand the frame back here to fall
            // through to the ordinary DM path instead of dropping it.
            #[cfg(not(feature = "hil"))]
            let raw_src = payload.get(1).copied().unwrap_or(0);
            #[cfg(not(feature = "hil"))]
            if let Some(room) = room_runtime.iter_mut().find(|r| r.hash == raw_src) {
                if handle_room_push_frame(
                    payload,
                    our_id,
                    room,
                    txq,
                    outstanding,
                    ui_events,
                    nvs_partition.clone(),
                    contact_display_names,
                    now_ms,
                    gps_verified,
                    adopted_server_clock,
                ) {
                    return;
                }
            }
            handle_dm(
                payload, our_id, policy, txq, outstanding, gps_snapshot, now_ms, tx_epoch_base,
                ui_events, nvs_partition.clone(),
            )
        }
        x if x == PayloadType::Req as u8 => {
            handle_req(payload, our_id, policy, txq, outstanding, ui_events, gps_snapshot, battery_snapshot)
        }
        x if x == PayloadType::Ack as u8 => {
            handle_ack(payload, outstanding, room_runtime, ui_events, now_ms)
        }
        x if x == PayloadType::Response as u8 => {
            // A direct RESPONSE datagram: the non-flood room-login-reply leg
            // (see `handle_room_login_response`'s doc). No other payload
            // type reaches this arm — a stock companion's telemetry
            // RESPONSE is unsolicited-request-only and MeshCadet never
            // sends `PAYLOAD_TYPE_REQ` itself, so nothing legitimate besides
            // a room login reply is expected here.
            #[cfg(not(feature = "hil"))]
            handle_room_login_response(
                payload,
                our_id,
                room_runtime,
                nvs_partition.clone(),
                ui_events,
                now_ms,
                gps_verified,
                adopted_server_clock,
            );
            #[cfg(feature = "hil")]
            {
                let _ = (room_runtime, nvs_partition, gps_verified, adopted_server_clock);
                rx_diag!("RX RESPONSE: ignored under hil (no rooms)");
            }
        }
        x if x == PayloadType::Path as u8 => {
            handle_path_return(
                payload,
                our_id,
                policy,
                outstanding,
                ui_events,
                room_runtime,
                nvs_partition,
                now_ms,
                gps_verified,
                adopted_server_clock,
            )
        }
        x if x == PayloadType::GrpTxt as u8 => handle_grp_txt(payload, channel_secret, ui_events),
        other => {
            rx_diag!(
                "RX: unhandled payload type 0x{:02x} (header 0x{:02x})",
                other, header_byte
            );
        }
    }
}

/// Decode a DM, apply the policy allowlist, log it, ACK it, and optionally
/// handle a telemetry pull request.
///
/// # Policy enforcement
///
/// 1. **Allowlist gate**: DMs from unknown senders are silently dropped.
/// 2. **Telemetry gate**: `?loc` requests from contacts without the telemetry
///    flag are silently dropped (no ACK, no presence leak, no log visible
///    outside the device).
///
/// # Telemetry pull path
///
/// When the decrypted DM text starts with `?loc` and
/// `policy.telemetry_enabled(src_hash)` is `true`:
/// - The cached GPS fix (or `loc:nofix`) is encoded into a reply DM.
/// - The reply DM is enqueued for transmission.
/// - A normal ACK is ALSO sent (the contact's DM is still acknowledged).
///
/// Two frames are enqueued for one inbound event here (reply, then ACK below),
/// with no drain in between — `TxQueue` (`dispatcher.rs`) must hold both
/// (FIFO), not just the most recent enqueue. It used to be a single
/// youngest-wins slot, which discarded the reply the moment the ACK was
/// enqueued: the log said "TX telemetry reply" but only the ACK ever reached
/// the wire.
///
/// # ACK invariant (unchanged from M1)
///
/// An ACK is emitted for every successfully decrypted DM from a known
/// contact, regardless of whether the text is a telemetry request or a plain
/// text message.  ACK is computed on the decrypted timestamp + type + text and
/// keyed on the originator's public key (MeshCore v1.15 §7.1 — unchanged in
/// v1.16). This is dual-compat with no version detection or negotiation:
/// MeshCadet's 4-byte ACK is accepted by both stock v1.15 and v1.16 nodes
/// (see `compute_ack_hash`'s doc comment in `protocol/src/codec.rs`).

/// Append one entry to the shared rotating `HISTORY` store (production builds
/// only — `HISTORY`/`HistoryStore` don't exist under `hil`).
///
/// Shared by every append-on-receipt path (`handle_dm`, `handle_grp_txt`) *and*
/// every append-on-send path (`SendDm`/`SendGroupMsg` handling below) so they
/// cannot drift out of sync — `handle_grp_txt` silently omitted history
/// entirely before an earlier fix, and outbound sends were never persisted
/// at all before this rewire. `sender_hash` carries the *conversation*
/// hash (contact hash for DM, channel hash for GrpTxt) regardless of
/// direction — this is the same `(msg_type, sender_hash)` key
/// `HistoryStore::append_conversation` routes to a region by, and matches the
/// UI's `messages` map key (`ui::UiRuntime.messages: HashMap<u8, _>`, keyed
/// the same way). `is_ours` sets the entry's direction flag bit
/// (`protocol::history_region::FLAG_IS_OURS`) so a single conversation region
/// holds both directions and hydrate/export can tell them apart. `text` is
/// raw bytes (not a `&str`) so an invalid-UTF-8 payload is stored verbatim
/// rather than replaced by a `"<invalid utf8>"` placeholder — matches what the
/// wire export codec expects (arbitrary bytes, no UTF-8 validity requirement).
/// `acked` is the
/// entry's ack/delivery status at write time: `true` for every inbound
/// entry (received — trivially "delivered", no pending ACK to model) and
/// `false` for an outbound entry at send time (the ACK, if any, has not
/// arrived yet — there is no post-hoc flash update when one later does; the
/// live-ack-to-flash wiring is a
/// separate, pre-existing gap, out of this fix's scope).
#[cfg(not(feature = "hil"))]
fn append_history(
    sender_hash: u8,
    msg_type: protocol::history::HistoryMsgType,
    timestamp: u32,
    text: &[u8],
    is_ours: bool,
    acked: bool,
) {
    use protocol::history::{HistoryEntry, MAX_HISTORY_TEXT_LEN};
    let text_len = text.len().min(MAX_HISTORY_TEXT_LEN) as u8;
    let mut text_buf = [0u8; MAX_HISTORY_TEXT_LEN];
    text_buf[..text_len as usize].copy_from_slice(&text[..text_len as usize]);
    let hist_entry = HistoryEntry {
        sender_hash,
        msg_type,
        timestamp,
        text: text_buf,
        text_len,
    };
    let mut guard = HISTORY.lock().expect("HISTORY mutex should not be poisoned");
    if let Some(ref mut hs) = *guard {
        if let Err(e) = hs.append_conversation(msg_type, sender_hash, &hist_entry, is_ours, acked) {
            log::warn!("history: append failed: {:?}", e);
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "hil", allow(unused_variables))]
fn handle_dm(
    payload: &[u8],
    our_id: &Identity,
    policy: &PolicyFilter,
    txq: &mut TxQueue,
    outstanding: &mut OutstandingSends,
    gps_snapshot: Option<(i32, i32, u32)>,
    now_ms: u64,
    tx_epoch_base: u32,
    ui_events: &mut Vec<ui::UiEvent>,
    nvs_partition: EspDefaultNvsPartition,
) {
    let raw_dest = payload.get(0).copied().unwrap_or(0);
    let raw_src  = payload.get(1).copied().unwrap_or(0);

    // ── Policy gate 1: allowlist ──────────────────────────────────────────────
    if !policy.allow_inbound_dm(raw_src) {
        rx_diag!(
            "RX DM: silently dropped — src_hash 0x{:02x} not in allowlist \
             (dest_hash 0x{:02x}, our 0x{:02x})",
            raw_src, raw_dest, our_id.pub_hash(),
        );
        return;
    }

    let contact_pubkey = policy.contact_pubkey(raw_src).unwrap();
    let shared = our_id.ecdh_shared_secret(contact_pubkey);

    rx_diag!(
        "RX DM payload: dest_hash=0x{:02x} src_hash=0x{:02x} len={} (our=0x{:02x})",
        raw_dest, raw_src, payload.len(), our_id.pub_hash(),
    );

    let mut dec_buf = [0u8; 256];
    match decode_dm_payload(&shared, payload, &mut dec_buf) {
        Ok((dest_hash, _src_hash, pt_len)) => {
            if dest_hash != our_id.pub_hash() {
                rx_diag!(
                    "RX DM: not for us — dest=0x{:02x} != our=0x{:02x}",
                    dest_hash, our_id.pub_hash(),
                );
                return;
            }
            if pt_len < 5 {
                log::warn!("RX DM: plaintext too short ({} bytes)", pt_len);
                return;
            }

            let ts        = u32::from_le_bytes([dec_buf[0], dec_buf[1], dec_buf[2], dec_buf[3]]);
            let type_byte = dec_buf[4];
            let text_region = &dec_buf[5..pt_len.min(dec_buf.len())];
            let text = c_str(text_region);

            let text_str = core::str::from_utf8(text).unwrap_or("<invalid utf8>");
            log::info!("RX DM from 0x{:02x} ts={}: \"{}\"", raw_src, ts, text_str);

            // ── Inbound replay gate (F3, meshcadet-outsider-boundary-security-review) ──
            // `ack` is this frame's content fingerprint (ts + type + text +
            // sender pubkey) — the SAME value the "ACK the DM" section below
            // computes for the wire ACK, hoisted here so it can also gate
            // replay acceptance; see `firmware_core::inbound_replay`'s module
            // doc for the full persisted-high-water-mark + content-ring rule
            // and its documented residual trade-off. Production builds only
            // — HIL is a bench rig, not exposed to this threat model, and
            // never touches NVS for anything else either (see
            // `inbound_replay_store`'s module doc).
            let ack = compute_ack_hash(ts, type_byte, text, contact_pubkey);
            #[cfg(not(feature = "hil"))]
            {
                let mut replay_state =
                    inbound_replay_store::load_inbound_replay_state(nvs_partition.clone(), raw_src);
                if !inbound_replay_store::check_and_record_inbound(&mut replay_state, ts, ack) {
                    rx_diag!(
                        "RX DM: rejected as a replay — src_hash 0x{:02x} ts={} ack={} \
                         (already accepted; no ACK, no history, no UI event, no telemetry reply)",
                        raw_src, ts, hex4(&ack),
                    );
                    return;
                }
                inbound_replay_store::save_inbound_replay_state(
                    nvs_partition.clone(),
                    raw_src,
                    &replay_state,
                );
            }

            // ── Persist to rotating history ───────────────────────────────────
            // Inbound entries are trivially "delivered" (acked=true) — there
            // is no pending ACK to model for a message we already received.
            #[cfg(not(feature = "hil"))]
            append_history(raw_src, protocol::history::HistoryMsgType::Dm, ts, text, false, true);

            // Post incoming DM event to the UI runtime.
            ui_events.push(ui::UiEvent::IncomingDm {
                from_hash: raw_src,
                from_name: format!("0x{:02x}", raw_src),
                text: text_str.to_owned(),
            });

            // ── Telemetry pull path ───────────────────────────────────────────
            // Detect ?loc requests and gate on policy.telemetry_enabled.
            //
            // Wire-first observability: a telemetry pull
            // touches three checkpoints — REQUEST DETECTED, GATE DECISION,
            // RESPONSE ATTEMPTED.  Log all three at info so a single HIL run is
            // conclusive about which one failed, rather than inferring from
            // source.  (This is the diagnostic an earlier pull-telemetry HIL
            // defect lacked: a silent drop was indistinguishable from no-request.)
            if is_telemetry_request(text) {
                let gate_ok = policy.telemetry_enabled(raw_src);
                log::info!(
                    "RX DM telemetry pull detected from 0x{:02x}: telemetry_enabled={} \
                     (fix={})",
                    raw_src,
                    gate_ok,
                    if gps_snapshot.is_some() { "available" } else { "none → loc:nofix" },
                );
                // Policy gate 2: telemetry flag.
                if !gate_ok {
                    // Acceptance criterion: non-enabled contact's request is silently dropped.
                    // No response, no ACK-before-gate (normal ACK below still fires —
                    // the DM itself was legitimate; we just don't answer the *location query*).
                    // We DO still send the DM ACK so the contact knows MeshCadet is alive,
                    // but emit NO telemetry response.
                    rx_diag!(
                        "RX DM ?loc: telemetry not enabled for src_hash 0x{:02x} — location reply suppressed",
                        raw_src,
                    );
                    // Fall through to ACK the DM itself.
                } else {
                    // Telemetry enabled: build and enqueue a location reply DM.
                    match build_telemetry_reply(now_ms, tx_epoch_base, our_id, contact_pubkey, gps_snapshot) {
                        Some((reply_frame, reply_len)) => {
                            log_tx_queue_eviction(
                                txq.enqueue(&reply_frame[..reply_len], None),
                                "telemetry reply",
                                outstanding,
                                |ev| ui_events.push(ev),
                            );
                            match gps_snapshot {
                                Some((_, _, age_secs)) => log::info!(
                                    "TX telemetry reply to 0x{:02x}: location (age={}s)",
                                    raw_src, age_secs,
                                ),
                                None => log::info!(
                                    "TX telemetry reply to 0x{:02x}: loc:nofix (no GPS fix yet)",
                                    raw_src,
                                ),
                            }
                        }
                        None => log::warn!("telemetry reply: frame encoding failed"),
                    }
                }
            }

            // ── ACK the DM ───────────────────────────────────────────────────
            // ACK is always sent for any successfully decrypted DM from a known
            // contact (telemetry or plain text). Keyed on originator's pubkey.
            // `ack` was already computed above (also the replay gate's content
            // fingerprint) — reused here rather than recomputed.
            let mut ack_frame = [0u8; 8];
            let n = build_ack_frame(&ack, &mut ack_frame);
            log_tx_queue_eviction(
                txq.enqueue(&ack_frame[..n], None),
                "DM ACK",
                outstanding,
                |ev| ui_events.push(ev),
            );
            log::info!("TX ACK queued for 0x{:02x}: ack_hash={}", raw_src, hex4(&ack));
        }
        Err(protocol::CodecError::MacMismatch) => {
            rx_diag!(
                "RX DM: MAC mismatch — dest_hash=0x{:02x} src_hash=0x{:02x} \
                 (contact in allowlist but ECDH key mismatch — check pubkey registration)",
                raw_dest, raw_src,
            );
        }
        Err(e) => log::warn!("RX DM: decode error: {:?}", e),
    }
}

/// Handle an inbound `PAYLOAD_TYPE_REQ` (0x00) — the MeshCore-native request
/// datagram the companion app uses for its telemetry/location button.
///
/// # Why this exists
///
/// MeshCadet originally answered only a bespoke `?loc` text DM (`handle_dm`),
/// but NO stock MeshCore companion sends that. The companion's telemetry pull is
/// a `PAYLOAD_TYPE_REQ` carrying `REQ_TYPE_GET_TELEMETRY_DATA`, and it waits for
/// a `PAYLOAD_TYPE_RESPONSE` matched by a reflected tag. With no `Req` arm in
/// `on_receive`, the request hit the "unhandled payload type" branch and was
/// dropped — the companion then showed "Telemetry unavailable…" every time,
/// while every `?loc`-only host test stayed green. This handler closes that gap.
///
/// # Policy
///
/// Same two gates as `handle_dm`: (1) the sender must be in the allowlist; (2)
/// for a telemetry pull, `policy.telemetry_enabled(src_hash)` must be true.
/// A non-enabled contact's request is silently dropped (no RESPONSE), preserving
/// the "still dropped for non-enabled contacts" half of the acceptance contract.
/// Unlike a DM, a REQ is not ACKed — MeshCore answers it with a RESPONSE only.
#[allow(clippy::too_many_arguments)]
fn handle_req(
    payload: &[u8],
    our_id: &Identity,
    policy: &PolicyFilter,
    txq: &mut TxQueue,
    outstanding: &mut OutstandingSends,
    ui_events: &mut Vec<ui::UiEvent>,
    gps_snapshot: Option<(i32, i32, u32)>,
    battery_snapshot: battery::BatteryStatus,
) {
    let raw_dest = payload.get(0).copied().unwrap_or(0);
    let raw_src  = payload.get(1).copied().unwrap_or(0);

    // ── Policy gate 1: allowlist ──────────────────────────────────────────────
    if !policy.allow_inbound_dm(raw_src) {
        rx_diag!(
            "RX REQ: silently dropped — src_hash 0x{:02x} not in allowlist \
             (dest_hash 0x{:02x}, our 0x{:02x})",
            raw_src, raw_dest, our_id.pub_hash(),
        );
        return;
    }

    let contact_pubkey = policy.contact_pubkey(raw_src).unwrap();
    let shared = our_id.ecdh_shared_secret(contact_pubkey);

    let mut dec_buf = [0u8; 256];
    match decode_dm_payload(&shared, payload, &mut dec_buf) {
        Ok((dest_hash, _src_hash, pt_len)) => {
            if dest_hash != our_id.pub_hash() {
                rx_diag!(
                    "RX REQ: not for us — dest=0x{:02x} != our=0x{:02x}",
                    dest_hash, our_id.pub_hash(),
                );
                return;
            }

            let plaintext = &dec_buf[..pt_len.min(dec_buf.len())];
            let req = match parse_telemetry_req(plaintext) {
                Some(r) => r,
                None => {
                    log::warn!("RX REQ: plaintext too short to parse ({} bytes)", pt_len);
                    return;
                }
            };

            // Only telemetry-data pulls are answered; other req_types are logged
            // and ignored (MeshCadet exposes no status/login/ACL surface).
            if !is_telemetry_req(&req) {
                rx_diag!(
                    "RX REQ from 0x{:02x}: unhandled req_type 0x{:02x} — ignored",
                    raw_src, req.req_type,
                );
                return;
            }

            let gate_ok = policy.telemetry_enabled(raw_src);
            log::info!(
                "RX REQ telemetry pull from 0x{:02x} (tag={:#010x}): telemetry_enabled={} (fix={})",
                raw_src,
                req.tag,
                gate_ok,
                if gps_snapshot.is_some() { "available" } else { "none" },
            );

            // ── Policy gate 2: telemetry flag ─────────────────────────────────
            if !gate_ok {
                rx_diag!(
                    "RX REQ telemetry pull: not enabled for src_hash 0x{:02x} — response suppressed",
                    raw_src,
                );
                return;
            }

            // Build and enqueue the RESPONSE (reflect tag + GPS fix if any +
            // battery percent/charging, always).
            let gps = gps_snapshot.map(|(lat_e7, lon_e7, _age)| (lat_e7, lon_e7));
            match build_telemetry_response(our_id, contact_pubkey, req.tag, gps, battery_snapshot) {
                Some((resp_frame, resp_len)) => {
                    log_tx_queue_eviction(
                        txq.enqueue(&resp_frame[..resp_len], None),
                        "telemetry RESPONSE",
                        outstanding,
                        |ev| ui_events.push(ev),
                    );
                    log::info!(
                        "TX telemetry RESPONSE to 0x{:02x} (tag={:#010x}): {}, battery={}%{}",
                        raw_src,
                        req.tag,
                        if gps.is_some() { "location" } else { "no-fix (presence marker)" },
                        battery_snapshot.percent,
                        if battery_snapshot.charging { " (charging)" } else { "" },
                    );
                }
                None => log::warn!("telemetry RESPONSE: frame encoding failed"),
            }
        }
        Err(protocol::CodecError::MacMismatch) => {
            rx_diag!(
                "RX REQ: MAC mismatch — dest_hash=0x{:02x} src_hash=0x{:02x} \
                 (contact in allowlist but ECDH key mismatch)",
                raw_dest, raw_src,
            );
        }
        Err(e) => log::warn!("RX REQ: decode error: {:?}", e),
    }
}

/// Compare an inbound ACK hash (bare `Ack` frame, `handle_ack`; or bundled
/// inside a PATH-return, `handle_path_return`'s `PathExtra::Ack` arm)
/// against `outstanding`'s outstanding-sends table. BOTH dispatch sites that
/// can receive an ACK call this SAME function: the room-post
/// delivery-ack defect this table replaces was exactly a matcher wired into
/// only ONE of those two call sites (a room post is sent flood-routed,
/// `room_session::encode_room_post_checked`'s `RouteType::Flood`, so the
/// responder often has no return route yet and teaches one back by bundling
/// its ACK inside a PATH-return rather than sending a bare `Ack` — the exact
/// "teach the route while replying" mechanism the flood-login's bundled
/// `PathExtra::Response` uses too). Collapsing DM and room-post tracking
/// into one table with one lookup here is what makes that class of bug
/// structurally unreachable: there is no second matcher for a future
/// call site to forget to wire up.
///
/// Returns `true` if an outstanding entry matched `got` (resolved and
/// evented); `false` otherwise. A bare `bool` rather than logging the
/// non-match case itself: `handle_ack`'s caller still has its own keep-alive
/// fallback to try before it's truly a "nothing matched" outcome, so ONLY
/// the caller that's actually last in its own fallback chain is in a
/// position to log that — same division of responsibility the old
/// `match_room_post_ack` (bool, no log) / `match_pending_ack` (the true
/// final fallback, DOES log) pair drew, just collapsed onto one table.
#[must_use]
fn match_outstanding_ack(got: [u8; 4], outstanding: &mut OutstandingSends, ui_events: &mut Vec<ui::UiEvent>) -> bool {
    match outstanding.resolve(got) {
        Some(send) => {
            log::info!(
                "ACK received: matches outstanding {:?} (ack_hash={})",
                send.kind, hex4(&got),
            );
            ui_events.push(delivery_event(send, true));
            true
        }
        None => false,
    }
}

/// Match an inbound ACK against `outstanding` ([`match_outstanding_ack`] —
/// covers both the DM and room-post paths, checked FIRST), OR a room's
/// in-flight keep-alive ACK (Phase C). A room keep-alive ACK additionally
/// carries the appended unsynced-count byte (`payload[4]`) that closes Phase
/// D's drain window — see
/// `firmware_core::room_session::RoomSyncPhase::on_keep_alive_ack`. A
/// keep-alive is sent route-direct over an already-learned path
/// (`room_session::encode_room_direct_prefix`'s doc), so its reply is always
/// a bare `Ack`, never a PATH-return — that match stays local to this
/// function; `handle_path_return`'s `PathExtra::Ack` arm never needs it.
#[cfg_attr(feature = "hil", allow(unused_variables))]
fn handle_ack(
    payload: &[u8],
    outstanding: &mut OutstandingSends,
    room_runtime: &mut [RoomRuntime],
    ui_events: &mut Vec<ui::UiEvent>,
    now_ms: u64,
) {
    if payload.len() < 4 {
        log::warn!("RX ACK: truncated ({} bytes)", payload.len());
        return;
    }
    let mut got = [0u8; 4];
    got.copy_from_slice(&payload[..4]);

    if match_outstanding_ack(got, outstanding, ui_events) {
        return;
    }

    #[cfg(not(feature = "hil"))]
    for room in room_runtime.iter_mut() {
        if room.pending_keep_alive_ack == Some(got) {
            room.pending_keep_alive_ack = None;
            // A successful ACK proves the route is live — reset the
            // reconnect-stall detector's miss counter (see
            // `RoomKeepAliveStall`'s doc).
            room.keep_alive_stall.reset();
            // Decode through the module's own validated decoder rather than
            // hand-indexing `payload[4]` — a payload truncated to exactly 4
            // bytes (no appended unsynced-count byte at all) must NOT be
            // silently treated as "0 unsynced" (which would prematurely
            // close Phase D's drain window); `Err` here just skips the
            // drain-phase update for this ACK, matching "unable to determine
            // backlog depth" rather than assuming the most optimistic case.
            match protocol::room::decode_keep_alive_ack(payload) {
                Ok(ack) => {
                    log::info!(
                        "RX room keep-alive ACK for 0x{:02x}: unsynced_count={}",
                        room.hash, ack.unsynced_count,
                    );
                    if let Some(room_session::RoomNotification::Aggregate { count }) =
                        room.sync_phase.on_keep_alive_ack(ack.unsynced_count, now_ms)
                    {
                        // Same diagnostic trail as the per-post drained/live
                        // log above: this is the moment a drain window that
                        // absorbed one or more silently-appended posts
                        // finally fires ITS single delayed badge/tone/blink
                        // — the gap between a post rendering and this line
                        // appearing in the log is exactly how long that post
                        // sat with content visible but no notification, by
                        // design (see `RoomSyncPhase`'s doc).
                        log::info!(
                            "room: 0x{:02x} drain window closed — firing one aggregate \
                             notification for {} post(s) absorbed while draining",
                            room.hash, count,
                        );
                        ui_events.push(ui::UiEvent::RoomDrainComplete {
                            room_hash: room.hash,
                            count,
                        });
                    }
                }
                Err(e) => log::warn!(
                    "RX room keep-alive ACK for 0x{:02x}: missing unsynced-count byte ({:?})",
                    room.hash, e,
                ),
            }
            return;
        }
    }

    // Nothing matched: not an outstanding DM/room-post send, and not a room
    // keep-alive either — the true "nothing left to try" fallback (see
    // `match_outstanding_ack`'s doc for why it doesn't log this itself).
    log::info!("ACK received (no outstanding send): ack_hash={}", hex4(&got));
}

/// Compare a duplicate-detected inbound frame's dedup key
/// (`protocol::packet_dedup_key`) against the outstanding `pending_channel_ack`.
/// On a match, clears `pending_channel_ack`, raises
/// `UiEvent::ChannelAcked { channel_hash }`, and returns `Some(channel_hash)`
/// so the caller can also persist the flip to flash.
///
/// Mirrors `match_outstanding_ack`'s DM/room-post counterpart, but keyed on
/// the packet dedup hash rather than a v1.15 ACK hash: a GRP_TXT has no per-recipient
/// delivery ACK on the wire at all, so hearing our own prior send repeated
/// back into the mesh — already recognised via the existing dedup ring (see
/// `dispatcher.rs`'s module doc) — IS the implicit ack. Matches at most once per
/// pending send: the first repeat clears `pending_channel_ack`, so any
/// further repeat of the same message no longer matches (idempotent).
fn match_pending_channel_ack(
    got: [u8; 4],
    pending_channel_ack: &mut Option<PendingChannelAck>,
    ui_events: &mut Vec<ui::UiEvent>,
) -> Option<u8> {
    match pending_channel_ack {
        Some(expected) if expected.hash == got => {
            let channel_hash = expected.channel_hash;
            log::info!(
                "GRP_TXT repeat heard: implicit channel ack (channel_hash=0x{:02x})",
                channel_hash,
            );
            ui_events.push(ui::UiEvent::ChannelAcked { channel_hash });
            *pending_channel_ack = None;
            Some(channel_hash)
        }
        _ => None,
    }
}

// ── Room-server client handlers ───────────────────────────────────────────────
//
// MILESTONE-1 WALKING SKELETON for `meshcadet-room-server-support`: log in to
// a provisioned room server and read its posts, end to end, on the thinnest
// path through every layer. The pure decode/ACK/dedup decisions live in
// `firmware_core::room_session` (host-tested against
// `protocol::room::RoomServerDouble`); everything here is the hardware glue
// — radio TX enqueue, `HISTORY`/`RoomRuntime`/NVS-session-store persistence,
// and the `ui::UiEvent` bridge — mirroring `handle_dm`'s own shape as closely
// as possible so the two RX paths stay easy to compare.
//
// Posting, permission-gated compose, the keep-alive scheduler, and
// notification-suppression parity are milestone 2 (`meshcadet-room-firmware-post-and-notify`)
// — out of scope here.

/// Apply a decoded room login outcome to `room`'s in-memory session state and
/// persist it to this room's dedicated NVS session store. Shared by both
/// login-reply forms (`handle_room_login_response`'s direct datagram and
/// `handle_path_return`'s bundled `PathExtra::Response`).
///
/// Raises [`ui::UiEvent::RoomPermissionUpdated`] with the outcome's fresh
/// `can_post` — F1 of this mission's Objective: `register_room` used to be
/// called ONCE at boot off the resumed session only, so a session upgrade
/// (e.g. Guest→ReadWrite on a room's very first login) never reached the UI
/// and compose stayed disabled until reboot. Every caller of this function
/// MUST have a `ui_events` sink available; there is currently no login-reply
/// path that doesn't.
///
/// Also adopts `outcome.server_ts` into `adopted_server_clock`
/// (`meshcadet-room-adopt-server-time`) via
/// `room_session::adopt_server_clock` — verified-GPS-outranks-server-time
/// and never-regresses are both enforced there, not here; this call site
/// only supplies `gps_verified` (`GpsDriver::clock_sync_verified()` — whether
/// GPS is synced from a REAL FIX right now, so a later login reply never
/// displaces a verified GPS clock that synced after adoption; an
/// unverified, RTC-derived GPS sync no longer blocks adoption here — see
/// `meshcadet-clock-source-provenance-and-sync-age`, which renamed this
/// parameter from `gps_synced`) and `now_ms` (the anchor's uptime reading).
#[cfg(not(feature = "hil"))]
fn apply_room_login_outcome(
    room: &mut RoomRuntime,
    outcome: &room_session::RoomLoginOutcome,
    nvs_partition: EspDefaultNvsPartition,
    ui_events: &mut Vec<ui::UiEvent>,
    now_ms: u64,
    gps_verified: bool,
    adopted_server_clock: &mut Option<room_session::AdoptedServerClock>,
) {
    room.session.apply_login_outcome(outcome);
    // Log only the FIRST transition into a room-server-adopted clock (`None`
    // -> `Some`) — observability for "why does this device's room clock
    // suddenly look right (or wrong)", without spamming a log line on every
    // later login that merely advances an already-adopted clock forward.
    let had_no_adopted_clock = adopted_server_clock.is_none();
    *adopted_server_clock = room_session::adopt_server_clock(
        *adopted_server_clock,
        gps_verified,
        now_ms,
        outcome.server_ts,
    );
    if had_no_adopted_clock && adopted_server_clock.is_some() {
        // "(no verified GPS sync)", not "(no GPS sync)" — an unverified,
        // RTC-derived GPS sync no longer blocks this adoption
        // (`meshcadet-clock-source-provenance-and-sync-age`), so this log
        // line can now fire while `gps_verified` is `false` but GPS is
        // synced (just unverified); the old wording would misleadingly
        // imply GPS was entirely unsynced.
        log::info!(
            "room: adopted server_ts={} from 0x{:02x}'s login reply as trusted wall clock \
             (no verified GPS sync)",
            outcome.server_ts, room.hash,
        );
    }
    // A fresh login (boot OR stall-triggered relearn) mirrors the server's
    // own push_failures reset conditions — this session is presumed live
    // again, so the missed-ACK counter must not carry a stale count into it.
    room.keep_alive_stall.reset();
    // `meshcadet-room-inbound-still-dead-after-two-fixes`, the mechanism
    // behind Lead A: a stall-triggered relearn's `RoomKeepAliveStall`
    // invalidation calls `RoomSyncPhase::note_closer_failed`, which the very
    // next scheduler tick force-closes (`is_draining() -> false`) — and
    // nothing ever reopened it, so the keep-alive scheduler's cadence
    // (`room_keep_alive_interval_ms`) silently collapsed from the 15 s
    // draining interval to the 5-minute routine one for the rest of this
    // boot, EVERY time a route ever needed relearning, even though THIS
    // login just completed and the session's caught-up state is unknown
    // again. See `RoomSyncPhase::note_relogin`'s doc for the full mechanism
    // and the two HIL captures that pin it.
    room.sync_phase.note_relogin(now_ms);
    // `meshcadet-room-inbound-still-dead-after-two-fixes`, Lead B: a
    // `sync_since` already ahead of what THIS login reply's `server_ts` just
    // admitted as the room server's own "now" can never again be exceeded
    // by any future post — the server has nothing left to push, forever.
    // See `room_session::reconcile_sync_since`'s doc for the mechanism and
    // the exact captured values this reproduces.
    if let Some(rewound) =
        room_session::reconcile_sync_since(room.session.sync_since, outcome.server_ts)
    {
        log::warn!(
            "room: 0x{:02x} sync_since={} is unreachably far ahead of server_ts={} — \
             rewinding to {} to force a full resync",
            room.hash, room.session.sync_since, outcome.server_ts, rewound,
        );
        room.session.sync_since = rewound;
    }
    // A successful login reply is one of the reflood backoff's two reset
    // conditions (`room_session::room_reflood_interval_ms`'s doc) — this
    // reply IS the reflood succeeding, so the next `!has_route()`
    // epoch (should one ever start again) begins fresh at the initial
    // backoff, not wherever this epoch's exponent left off.
    //
    // Gated on `has_route()`, never `out_path_len != 0`
    // (`meshcadet-room-messages-no-longer-received-regression`, reverting
    // `meshcadet-room-reflood-backoff-resets-without-a-learned-route`'s own
    // mistake): `out_path_len == 0` is legitimately ambiguous between "no
    // route known" and "a learned ZERO-HOP route" (the room server is this
    // device's direct radio neighbour — the ordinary bench topology, and
    // exactly the same conflation `meshcadet-room-notify-suppression-full-
    // enumeration-fix` already fixed for the re-flood branch itself, see
    // `PersistedRoomSession::has_route`'s doc and the log match 37 lines
    // below THIS SAME FUNCTION, which already asks `has_route()` for the
    // identical question). Testing `out_path_len != 0` here meant a
    // zero-hop room's `reflood_attempts` counter was NEVER reset back to
    // the 30 s floor on any successful (re)login — including the very
    // first one — so every later stall-triggered relogin cycle doubled the
    // backoff instead of resetting it, escalating an ordinary, recoverable
    // reconnect blip into an ever-lengthening outage that starved the
    // session of any stable connected window long enough to complete a
    // post push-and-ACK round trip. `has_route()` is `true` for a taught
    // route of ANY hop count (`PersistedRoomSession::apply_login_outcome`
    // sets it unconditionally whenever `outcome.out_path` is `Some(..)`,
    // zero hops included) and stays `false` for the direct-`RESPONSE` leg
    // that teaches no path at all — the exact distinction this reset
    // needs, with no separate reachability caveat required.
    if room.session.has_route() {
        room.reflood_attempts = 0;
    }
    // The next keep-alive should re-affirm this login's `sync_since` via
    // `force_since` rather than the routine `0` — see `resync_pending`'s doc.
    room.resync_pending = true;
    room_session::save_room_session(
        nvs_partition,
        room.hash,
        room.session_epoch,
        &room.session,
    );
    // Report the HOP COUNT, not just "learned a path"
    // (`meshcadet-room-notify-suppression-full-enumeration-fix`): the case
    // that hid for five missions was a reply teaching a ZERO-HOP route (a
    // direct-neighbour server), which logged identically to a multi-hop one
    // while behaving like no route at all. A log that cannot distinguish
    // them cannot be used to diagnose them — this line is what a HIL capture
    // greps for.
    match outcome.out_path {
        Some((_, hops)) => log::info!(
            "room: login complete for 0x{:02x}: permissions={:?} (learned out_path, {} hop(s){})",
            room.hash,
            room.session.permission(),
            hops,
            if hops == 0 {
                " — server is a direct radio neighbour; keep-alives go route-direct with an \
                 empty path"
            } else {
                ""
            },
        ),
        None => log::info!(
            "room: login complete for 0x{:02x}: permissions={:?} (no path taught by this \
             reply; route {})",
            room.hash,
            room.session.permission(),
            if room.session.has_route() {
                "already known"
            } else {
                "still UNKNOWN — the re-flood branch stays active"
            },
        ),
    }
    ui_events.push(ui::UiEvent::RoomPermissionUpdated {
        room_hash: room.hash,
        can_post: room.session.permission().can_post(),
    });
}

/// Handle a direct `RESPONSE` datagram (`PayloadType::Response`) login
/// reply. Decoded for completeness (both login-reply forms must decode — see
/// this mission's Acceptance), even though on M1 hardware a room's first
/// login is always the flood/PATH-return leg
/// (`room_session::encode_room_login_frame`'s doc: no learned route yet) —
/// this is the leg a later re-login (milestone 2's keep-alive scheduler)
/// would exercise once `out_path` is known.
#[cfg(not(feature = "hil"))]
#[allow(clippy::too_many_arguments)]
fn handle_room_login_response(
    payload: &[u8],
    our_id: &Identity,
    room_runtime: &mut [RoomRuntime],
    nvs_partition: EspDefaultNvsPartition,
    ui_events: &mut Vec<ui::UiEvent>,
    now_ms: u64,
    gps_verified: bool,
    adopted_server_clock: &mut Option<room_session::AdoptedServerClock>,
) {
    let raw_src = payload.get(1).copied().unwrap_or(0);
    let Some(room) = room_runtime.iter_mut().find(|r| r.hash == raw_src) else {
        rx_diag!(
            "RX RESPONSE: no provisioned room matches src_hash 0x{:02x}",
            raw_src,
        );
        return;
    };
    // A direct login-reply RESPONSE is addressed to one specific member via
    // `dest_hash = payload[0]`, but any device in radio range that
    // provisioned this room still gets this frame handed to `on_receive`
    // and routes it here on `src_hash` alone — see
    // `room_session::is_room_frame_for_us`'s doc. Filter on `dest_hash`
    // before decoding so another member's login reply doesn't cost a decode
    // attempt that's guaranteed to fail the MAC and log a misleading WARN.
    if !room_session::is_room_frame_for_us(payload, our_id.pub_hash()) {
        rx_diag!(
            "RX RESPONSE: room login reply not for us — dest=0x{:02x} != our=0x{:02x} (room 0x{:02x})",
            payload.first().copied().unwrap_or(0), our_id.pub_hash(), room.hash,
        );
        return;
    }
    let shared = our_id.ecdh_shared_secret(&room.pubkey);
    match room_session::decode_login_response_datagram(&shared, payload) {
        Ok(outcome) => apply_room_login_outcome(
            room,
            &outcome,
            nvs_partition,
            ui_events,
            now_ms,
            gps_verified,
            adopted_server_clock,
        ),
        Err(e) => log::warn!(
            "RX room login (direct RESPONSE) from 0x{:02x}: decode error: {:?}",
            raw_src, e,
        ),
    }
}

/// Handle an inbound room push (`TXT_MSG`, `TXT_TYPE_SIGNED_PLAIN`) from a
/// known room contact: decode, ACK (non-negotiable — see
/// `firmware_core::room_session::handle_room_push`'s doc), content-dedup
/// against `room.recent`, append to the shared rotating history exactly like
/// `handle_dm` does for an ordinary DM (same conversation-hash keying, so the
/// Groups-tab row and the message view Just Work), and persist the advanced
/// `sync_since` watermark.
///
/// **Sender-render parity with channel messages:** a room push carries no
/// sender NAME on the wire, only the poster's `author_pubkey_prefix` (see
/// `room_session::RoomPushOutcome`'s doc). `contact_display_names` resolves
/// `author_pubkey_prefix[0]` (== `Contact::pub_hash`, the same 1-byte
/// routing hash every other contact lookup in this codebase keys on)
/// against this device's provisioned contacts, and the resolved label — the
/// contact's display name, or a hex fallback for a poster this device
/// doesn't know as a contact — is formatted onto the body with the exact
/// `"<name>: "` MeshCore delimiter (`protocol::codec::CHANNEL_NAME_DELIM`)
/// a channel (GRP_TXT) message already carries inline on the wire. That
/// formatted text is what gets persisted (`append_history`) and posted to
/// the UI, so `firmware_core::ui::message_view::build_message_items`'s
/// existing `is_channel && !m.is_ours` bold-prefix split — already applied
/// to rooms, which render as `is_channel: true` `ChannelItem`s — picks it up
/// with no room-specific rendering logic. `room.recent` (the dedup input)
/// keeps the RAW, un-prefixed body instead (`is_duplicate_post`'s doc
/// explains why comparing against the wire's un-prefixed form must stay
/// robust to a `recent` that's been reseeded from prefixed, persisted
/// history after a reboot).
///
/// Returns `true` if the frame was handled here (decoded as a push,
/// rejected as malformed, or recognised as another member's copy of a push
/// via a `dest_hash` mismatch — see below), `false` if the caller must fall
/// through to `handle_dm` instead — the latter fires only on
/// `RoomSessionError::Room(RoomCodecError::NotSignedPlain)`: the 1-byte
/// `src_hash` routing in the caller is a 1-in-256 collision away from
/// misrouting a genuine plain DM from an ordinary contact into this
/// function, and `NotSignedPlain` is exactly the signature of that
/// misroute (a plain DM decodes fine as `TXT_TYPE_PLAIN`, which is not
/// `TXT_TYPE_SIGNED_PLAIN`) rather than of a corrupt or malicious push.
#[cfg(not(feature = "hil"))]
#[allow(clippy::too_many_arguments)]
fn handle_room_push_frame(
    payload: &[u8],
    our_id: &Identity,
    room: &mut RoomRuntime,
    txq: &mut TxQueue,
    outstanding: &mut OutstandingSends,
    ui_events: &mut Vec<ui::UiEvent>,
    nvs_partition: EspDefaultNvsPartition,
    contact_display_names: &std::collections::HashMap<u8, String>,
    now_ms: u64,
    gps_verified: bool,
    adopted_server_clock: &mut Option<room_session::AdoptedServerClock>,
) -> bool {
    // `on_receive` routes here on `src_hash` (`payload[1] == room.hash`)
    // alone, so every OTHER member's copy of the same push also reaches this
    // device — see `room_session::is_room_frame_for_us`'s doc. Filter on
    // `dest_hash` before attempting a decode at all, so this ordinary room
    // chatter doesn't cost a wasted decode + a misleading
    // `decode error: MacMismatch` WARN that reads like our own session is
    // broken.
    if !room_session::is_room_frame_for_us(payload, our_id.pub_hash()) {
        rx_diag!(
            "RX room push from 0x{:02x}: not for us — dest=0x{:02x} != our=0x{:02x}",
            room.hash, payload.first().copied().unwrap_or(0), our_id.pub_hash(),
        );
        return true;
    }
    let shared = our_id.ecdh_shared_secret(&room.pubkey);
    match room_session::handle_room_push(
        &shared,
        payload,
        &our_id.pubkey,
        room.hash,
        &room.recent,
    ) {
        Ok(outcome) => {
            // An inbound post is proof `out_path`-direction traffic is
            // flowing (the server routed a push down its own path and it
            // reached us) — reset the stall detector's miss counter exactly
            // like a successful keep-alive ACK would (see
            // `RoomKeepAliveStall`'s doc).
            room.keep_alive_stall.reset();
            // Same evidence resets the reflood backoff's other reset
            // condition — see `room_session::room_reflood_interval_ms`'s doc.
            room.reflood_attempts = 0;
            // ACK is non-negotiable: transmitted unconditionally, even for a
            // push `handle_room_push` recognises as an already-seen
            // duplicate (`outcome.entry: None`) — see that function's doc.
            let mut ack_frame = [0u8; 8];
            let n = build_ack_frame(&outcome.ack_hash, &mut ack_frame);
            log_tx_queue_eviction(
                txq.enqueue(&ack_frame[..n], None),
                "room-push ACK",
                outstanding,
                |ev| ui_events.push(ev),
            );
            log::info!(
                "TX room-push ACK queued for 0x{:02x}: ack_hash={}",
                room.hash,
                hex4(&outcome.ack_hash),
            );

            // Phase D: classify BEFORE the dedup-gated content append below,
            // off the SAME `outcome` — `RoomSyncPhase::on_push_outcome`'s
            // whole contract is that a dedup hit (`entry: None`) is neither
            // counted nor notified, keeping the notification-suppression
            // rule in lockstep with the content dedup a re-drain after
            // reboot depends on.
            let notification = room.sync_phase.on_push_outcome(&outcome, now_ms);

            if let Some(entry) = outcome.entry {
                let text_len = (entry.text_len as usize).min(entry.text.len());
                let text = &entry.text[..text_len];
                let body_str = core::str::from_utf8(text).unwrap_or("<invalid utf8>");
                // Sender-render parity with channel messages — see this
                // function's doc. `room.recent.push(entry)` below keeps the
                // pre-format `entry` (raw body, no prefix) for dedup; only
                // the persisted/displayed copy gets the "<name>: " prefix.
                // `room_session::room_post_sender_label` (the pure,
                // host-tested half) does the name-or-hex-fallback decision;
                // this call site only owns the actual contact lookup (which
                // needs `contact_display_names`, a `main.rs`-local snapshot
                // — this crate has no contact list of its own).
                let sender_label = room_session::room_post_sender_label(
                    contact_display_names
                        .get(&outcome.author_pubkey_prefix[0])
                        .map(|s| s.as_str()),
                    &outcome.author_pubkey_prefix,
                );
                let display_text = format!("{}: {}", sender_label, body_str);
                log::info!(
                    "RX room push from 0x{:02x} author={} ts={}: \"{}\"",
                    room.hash, sender_label, entry.timestamp, body_str,
                );
                append_history(
                    room.hash,
                    protocol::history::HistoryMsgType::Dm,
                    entry.timestamp,
                    display_text.as_bytes(),
                    false,
                    true,
                );
                if room.recent.len() >= ROOM_RECENT_CAP {
                    room.recent.remove(0);
                }
                room.recent.push(entry);
                // Session-phase notification classification (Phase D): a
                // still-draining backlog post is appended silently (folded
                // into the eventual aggregate); a live post gets full
                // channel-path parity. `RoomNotification::None` is also
                // reachable here in principle (can't from a fresh `Some`
                // entry today, but matching exhaustively rather than
                // wildcarding keeps this in lockstep if `RoomSyncPhase`'s
                // classification ever grows a new suppressed case).
                // Diagnostic trail for the room-notification-parity
                // investigation (`meshcadet-room-notification-parity`): a
                // HIL report of "renders but never notifies" is
                // indistinguishable, from the screen alone, between (a) this
                // post landing in the `None` arm below — by design, still
                // draining, silently folded into the eventual aggregate —
                // and (b) a genuine defect. Logging which arm fired, per
                // post, means the next HIL run's serial capture can answer
                // that question directly (grep for "post drained" vs "post
                // live" at the timestamp the tester sent the test message)
                // instead of requiring another round of static tracing.
                match notification {
                    room_session::RoomNotification::None => {
                        log::info!(
                            "room: 0x{:02x} post drained (still draining) — appended silently, \
                             no badge/tone/blink yet; folded into the pending aggregate",
                            room.hash,
                        );
                        ui_events.push(ui::UiEvent::RoomPostDrained {
                            room_hash: room.hash,
                            text: display_text,
                        });
                    }
                    room_session::RoomNotification::Live => {
                        log::info!(
                            "room: 0x{:02x} post live — full notification parity with the \
                             channel path (badge + tone + blink)",
                            room.hash,
                        );
                        ui_events.push(ui::UiEvent::RoomPostLive {
                            room_hash: room.hash,
                            text: display_text,
                        });
                    }
                    room_session::RoomNotification::Aggregate { count } => {
                        // `meshcadet-room-post-no-notification`'s defect,
                        // caught by its own explicit ask ("assert the
                        // consequence, not just the classifier's return
                        // value"): this arm's own former comment claimed
                        // `on_push_outcome` can never produce `Aggregate` —
                        // false since `meshcadet-room-drain-window-never-
                        // closes-no-notify` landed `RoomSyncPhase::
                        // on_post_received`'s stall-timeout force-close
                        // (reachable from THIS call site, `on_push_outcome`
                        // -> `on_post_received`, not just from
                        // `on_keep_alive_ack` above), and reachable again
                        // now via `RoomSyncPhase::note_closer_failed`. Both
                        // paths correctly flip the classifier's internal
                        // state to "closed" — but until this fix, this arm
                        // dropped the notification on the floor right here:
                        // the drain window closed with nobody ever told.
                        //
                        // THIS post's own text has not reached the live UI
                        // yet (unlike `handle_ack`'s keep-alive-triggered
                        // close, which carries no post of its own — every
                        // post it is closing for was already individually
                        // appended via an earlier `RoomPostDrained`) — `count`
                        // counts it in (`on_post_received`'s
                        // `drained_count.saturating_add(1)`). Append it
                        // silently first, exactly like the `None` arm above,
                        // then fire the SAME single aggregate notification
                        // `handle_ack`'s path fires — one badge/tone/blink
                        // for the whole backlog just absorbed, never one per
                        // post.
                        log::info!(
                            "room: 0x{:02x} drain window closed — firing one aggregate \
                             notification for {} post(s) absorbed while draining",
                            room.hash, count,
                        );
                        ui_events.push(ui::UiEvent::RoomPostDrained {
                            room_hash: room.hash,
                            text: display_text,
                        });
                        ui_events.push(ui::UiEvent::RoomDrainComplete {
                            room_hash: room.hash,
                            count,
                        });
                    }
                }
            }

            // Persist the advanced watermark now that the ACK is queued —
            // mirrors `append_history`'s own "record what we observed, don't
            // wait for on-air confirmation" convention (see that fn's doc on
            // `acked` for inbound entries). `record_synced_post_ts` advances
            // monotonically, mirroring `record_sent_timestamp`'s guard on
            // `last_room_ts` — see that method's doc for why an unconditional
            // assignment here would be a bug, not just an asymmetry.
            room.session.record_synced_post_ts(outcome.post_ts);
            // Treat this push's `post_ts` as a trusted lower bound on real
            // time too (`meshcadet-room-adopt-server-time`, Scope item 5) —
            // it is equally server-stamped (`MyMesh.cpp:41-51`), just a
            // weaker/continuous source than a once-per-login `server_ts`.
            // Same priority + monotonicity rule as the login path (see
            // `apply_room_login_outcome`'s doc): a verified GPS sync still
            // outranks it, and it can never regress the already-adopted
            // clock. Log only the
            // FIRST transition into an adopted clock (a login reply may
            // never arrive in time to be first — a push is just as valid a
            // seed) — see `apply_room_login_outcome`'s identical log gate
            // for why later advances stay silent.
            let had_no_adopted_clock = adopted_server_clock.is_none();
            *adopted_server_clock = room_session::adopt_server_clock(
                *adopted_server_clock,
                gps_verified,
                now_ms,
                outcome.post_ts,
            );
            if had_no_adopted_clock && adopted_server_clock.is_some() {
                // "(no verified GPS sync)" — see `apply_room_login_outcome`'s
                // identical log line for why.
                log::info!(
                    "room: adopted post_ts={} from 0x{:02x}'s push as trusted wall clock \
                     (no verified GPS sync)",
                    outcome.post_ts, room.hash,
                );
            }
            room_session::save_room_session(
                nvs_partition,
                room.hash,
                room.session_epoch,
                &room.session,
            );
            true
        }
        Err(e) if room_session::is_room_push_misroute(&e) => {
            // Not a room push at all — most likely a genuine plain DM from a
            // contact whose 1-byte src_hash collides with this room's (see
            // this fn's doc and `is_room_push_misroute`'s). Tell the caller
            // to fall through to `handle_dm` instead of silently dropping
            // what may be a legitimate message.
            false
        }
        Err(e) => {
            log::warn!(
                "RX room push from 0x{:02x}: decode error: {:?}",
                room.hash, e,
            );
            true
        }
    }
}

/// Handle a PATH-return (0x08) — decrypt and extract bundled ACK.
#[allow(clippy::too_many_arguments)]
fn handle_path_return(
    payload: &[u8],
    our_id: &Identity,
    policy: &PolicyFilter,
    outstanding: &mut OutstandingSends,
    ui_events: &mut Vec<ui::UiEvent>,
    room_runtime: &mut [RoomRuntime],
    nvs_partition: EspDefaultNvsPartition,
    now_ms: u64,
    gps_verified: bool,
    adopted_server_clock: &mut Option<room_session::AdoptedServerClock>,
) {
    let raw_src = payload.get(1).copied().unwrap_or(0);

    if !policy.allow_inbound_dm(raw_src) {
        rx_diag!(
            "RX PATH: silently dropped — src_hash 0x{:02x} not in allowlist",
            raw_src,
        );
        return;
    }

    let contact_pubkey = policy.contact_pubkey(raw_src).unwrap();
    let shared = our_id.ecdh_shared_secret(contact_pubkey);

    let mut dec_buf = [0u8; 256];
    match decode_path_return(&shared, payload, &mut dec_buf) {
        Ok((dest_hash, _src_hash, rp)) => {
            if dest_hash != our_id.pub_hash() {
                rx_diag!(
                    "RX PATH: not for us — dest=0x{:02x} != our=0x{:02x}",
                    dest_hash, our_id.pub_hash(),
                );
                return;
            }
            rx_diag!(
                "RX PATH from 0x{:02x}: {} path bytes, extra={:?}",
                raw_src, rp.path_byte_count, rp.extra,
            );
            match rp.extra {
                PathExtra::Ack(got) => {
                    // A room post's ACK often arrives bundled here rather
                    // than as a bare `Ack` datagram — see
                    // `match_outstanding_ack`'s doc for why this call
                    // MUST be (and is) the exact same matching logic
                    // `handle_ack` uses for a bare `Ack` frame.
                    if !match_outstanding_ack(got, outstanding, ui_events) {
                        log::info!(
                            "RX PATH: bundled ACK but no outstanding send matched (ack_hash={})",
                            hex4(&got),
                        );
                    }
                }
                PathExtra::None => {
                    rx_diag!("RX PATH: no bundled ACK (extra=None)");
                }
                PathExtra::Response(bundled) => {
                    // Bundled RESPONSE extra: the flood-login reply leg for a
                    // room-server ANON_REQ login — the case that actually
                    // happens on first contact (see
                    // `room_session::encode_room_login_frame`'s doc). Reuses
                    // the `rp`/`shared` this function already decoded rather
                    // than re-decrypting via
                    // `firmware_core::room_session::decode_login_path_return`.
                    #[cfg(not(feature = "hil"))]
                    {
                        match room_runtime.iter_mut().find(|r| r.hash == raw_src) {
                            Some(room) => match decode_login_response(&bundled) {
                                Ok(resp) => {
                                    let outcome = room_session::RoomLoginOutcome {
                                        permissions: resp.permissions,
                                        out_path: Some((rp.path, rp.path_byte_count)),
                                        server_ts: resp.server_ts,
                                    };
                                    apply_room_login_outcome(
                                        room,
                                        &outcome,
                                        nvs_partition,
                                        ui_events,
                                        now_ms,
                                        gps_verified,
                                        adopted_server_clock,
                                    );
                                }
                                Err(e) => log::warn!(
                                    "RX PATH room login from 0x{:02x}: decode error: {:?}",
                                    raw_src, e,
                                ),
                            },
                            None => rx_diag!(
                                "RX PATH: bundled RESPONSE extra from unrecognised room 0x{:02x}",
                                raw_src,
                            ),
                        }
                    }
                    #[cfg(feature = "hil")]
                    {
                        let _ = (
                            room_runtime,
                            nvs_partition,
                            bundled,
                            now_ms,
                            gps_verified,
                            adopted_server_clock,
                        );
                        rx_diag!("RX PATH: bundled RESPONSE extra ignored under hil (no rooms)");
                    }
                }
            }
        }
        Err(protocol::CodecError::MacMismatch) => {
            rx_diag!(
                "RX PATH: MAC mismatch (contact in allowlist but ECDH key mismatch — \
                 check pubkey registration)"
            );
        }
        Err(e) => log::warn!("RX PATH: decode error: {:?}", e),
    }
}

/// Decode + log an inbound GRP_TXT under the provisioned channel secret.
///
/// `channel_secret` is `None` whenever no channel is provisioned
/// (`ProvisionedConfig::resolve_channel_secret` returned `None` at boot) —
/// this is the RX-side half of the fix for
/// `meshcadet-grptxt-rx-open-on-published-test-channel-secret`: a
/// contacts-only device must accept NO GRP_TXT at all, so this returns
/// before touching the payload rather than falling back to a placeholder
/// secret (as `firmware/src/main.rs`'s boot sequence used to).
fn handle_grp_txt(payload: &[u8], channel_secret: Option<&[u8]>, ui_events: &mut Vec<ui::UiEvent>) {
    let Some(channel_secret) = channel_secret else {
        rx_diag!("RX GRP_TXT: no channel provisioned — dropped");
        return;
    };
    if payload.is_empty() {
        rx_diag!("RX GRP_TXT: empty payload");
        return;
    }
    let ch = payload[0];
    if ch != channel_hash_var(channel_secret) {
        rx_diag!("RX GRP_TXT: channel hash 0x{:02x} not ours (expected 0x{:02x})", ch, channel_hash_var(channel_secret));
        return;
    }
    let mut pt_buf = [0u8; 256];
    match decode_grp_txt_var(channel_secret, payload, &mut pt_buf) {
        Ok(fields) => {
            let end = (fields.text_offset + fields.text_len).min(pt_buf.len());
            let text = c_str(&pt_buf[fields.text_offset..end]);
            let text_str = core::str::from_utf8(text).unwrap_or("<invalid utf8>");
            // Channel text carries the MeshCore "<name>: <msg>" prefix; parse it
            // so the log attributes the sender. The full "<name>: <msg>" string is
            // kept for display — group conversations show the sender inline, exactly
            // as the companion does. A prefix-less body falls back to verbatim.
            let (name, body) = protocol::parse_channel_text(text);
            match name {
                Some(n) => log::info!(
                    "RX GRP_TXT (channel 0x{:02x}) ts={} from \"{}\": \"{}\"",
                    ch, fields.timestamp,
                    core::str::from_utf8(n).unwrap_or("<invalid utf8>"),
                    core::str::from_utf8(body).unwrap_or("<invalid utf8>"),
                ),
                None => log::info!(
                    "RX GRP_TXT (channel 0x{:02x}) ts={} (no name prefix): \"{}\"",
                    ch, fields.timestamp, text_str,
                ),
            }
            ui_events.push(ui::UiEvent::IncomingGroupMsg {
                channel_hash: ch,
                text: text_str.to_owned(),
            });

            // ── Persist to rotating history ───────────────────────────────────
            // Mirrors handle_dm's append-on-receipt: DMs were durably recorded
            // here but GRP_TXT (channel) receipt never touched HISTORY at all —
            // channel conversations rendered on-screen (IncomingGroupMsg above)
            // but a fresh `export-history` could never reflect them, on the
            // first export or any re-run. `sender_hash` carries the channel
            // hash (GRP_TXT has no per-message sender pubkey on the wire; the
            // sender name lives in the "<name>: <msg>" text prefix already
            // captured in `text`, the same raw bytes used for the log above).
            // Inbound: acked=true (already delivered, no pending ACK to model).
            #[cfg(not(feature = "hil"))]
            append_history(ch, protocol::history::HistoryMsgType::GrpTxt, fields.timestamp, text, false, true);
        }
        Err(e) => log::warn!("RX GRP_TXT: decode error: {:?}", e),
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Trim a decrypted text buffer to its C-string length.
#[inline]
fn c_str(buf: &[u8]) -> &[u8] {
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    &buf[..n]
}

/// Format a 4-byte hash as `aabbccdd` (no alloc).
fn hex4(h: &[u8; 4]) -> heapless_hex::Hex4 {
    heapless_hex::Hex4::new(h)
}

/// Return esp_timer uptime in milliseconds.
///
/// `pub(crate)` (not private): `ui::UiRuntime::run_splash_ripple`'s dedicated
/// render loop needs its own wall-clock reads to time the ripple's tight render loop
/// independent of the dispatcher loop's own `now` — reusing this function
/// rather than duplicating the `esp_timer_get_time` call keeps exactly one
/// uptime-reading implementation in the crate.
#[inline]
pub(crate) fn uptime_ms() -> u64 {
    unsafe { esp_idf_svc::sys::esp_timer_get_time() as u64 / 1000 }
}

/// Return esp_timer uptime in microseconds — `--features diagnostics` only.
///
/// The dispatcher-loop phases this feeds (`perf::PerfRollup`'s per-phase
/// min/mean/max/p95 rollup, M0 of `meshcadet-perf-rearchitecture`) mostly
/// complete in well under 1 ms (GPS/battery poll, an idle RX-poll pass) —
/// [`uptime_ms`]'s millisecond truncation would read almost all of them as a
/// flat `0` and report no useful signal at all. This is a separate function
/// (not a change to `uptime_ms`'s existing millisecond contract, which every
/// pre-existing call site — loop scheduling, backoff timers, `UiRuntime`'s
/// own clocks — depends on unchanged) purely so the instrumentation can see
/// sub-millisecond durations.
#[cfg(feature = "diagnostics")]
#[inline]
fn uptime_us() -> u64 {
    unsafe { esp_idf_svc::sys::esp_timer_get_time() as u64 }
}

/// Log the CALLING task's stack high-water mark: `uxTaskGetStackHighWaterMark`
/// with a `NULL` handle always reports the current task, so `task_name` /
/// `stack_total_b` are caller-supplied labels only — this must be called FROM
/// the task being measured, not about it.
///
/// Shared by every long-lived spawned thread's own HWM log (`admin_server`,
/// `provisioning_server`) and the main-task periodic sample below — pulled out
/// once these threads all needed the identical `uxTaskGetStackHighWaterMark`→
/// percentage-headroom computation, rather than three near-duplicate call
/// sites (see the `boot-pthread-stack-overflow-fix` mission: a `pthread` task
/// stack overflow that was invisible until an on-hardware crash precisely
/// because neither spawned thread had this instrumentation the main task
/// already carried).
pub(crate) fn log_thread_stack_hwm(task_name: &str, stack_total_b: u32) {
    let hwm: u32 =
        unsafe { esp_idf_svc::sys::uxTaskGetStackHighWaterMark(core::ptr::null_mut()) };
    log::info!(
        "{}: stack HWM: {} B free / {} B total = {} B peak ({}% headroom)",
        task_name,
        hwm,
        stack_total_b,
        stack_total_b.saturating_sub(hwm),
        hwm * 100 / stack_total_b,
    );
}

/// Format a full 32-byte public key as 64 lowercase hex chars (no alloc).
fn hex_full(key: &[u8; 32]) -> heapless_hex::Hex32 {
    heapless_hex::Hex32::new(key)
}

mod heapless_hex {
    use core::fmt;

    pub struct Hex4([u8; 4]);
    impl Hex4 { pub fn new(h: &[u8; 4]) -> Self { Hex4(*h) } }
    impl fmt::Display for Hex4 {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for b in self.0 { write!(f, "{:02x}", b)?; }
            Ok(())
        }
    }

    pub struct Hex32([u8; 32]);
    impl Hex32 { pub fn new(k: &[u8; 32]) -> Self { Hex32(*k) } }
    impl fmt::Display for Hex32 {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for b in self.0 { write!(f, "{:02x}", b)?; }
            Ok(())
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// FINAL TRIAGE (firmware-core-extract-ui-runtime increment): these tests
// stay here, device-only/compile-only — leave-as-is, per this campaign's
// bucket (c). `match_outstanding_ack`/`match_pending_channel_ack` themselves
// are in fact pure (`log::info!`/`log::warn!` are the only side effects), so
// the block on this pair is narrower than a hardware dependency: their sole
// non-primitive argument, `ui::UiEvent`, is the radio→UI event bridge type
// still defined in `firmware/src/ui/mod.rs`, and moving IT into
// `firmware-core` would require touching `main.rs`'s whole RX/dispatch
// pipeline (every `UiEvent` construction site across the receive-handler
// bring-up — genuinely hardware/boot-coupled code) to keep it a
// behavior-preserving move rather than a rewrite. That is a larger,
// separately-scoped change than this increment's "ui/mod.rs screen/UI-
// runtime pure helpers" mandate, so per this campaign's own abort clause the
// un-extractable remainder is filed here, explicitly, rather than forced.
// See `docs/adr/0005-firmware-core-extraction.md` for the extraction pattern
// this campaign follows. `dispatcher::OutstandingSends` itself (the
// underlying table `match_outstanding_ack` calls into) is fully covered by
// `firmware-core`'s own host-executed test suite — see that type's doc; the
// tests below exercise `main.rs`'s glue around it, not the table itself.
//
// Regression guard for "live ACK never advances the ✓→✓✓ indicator":
// `match_outstanding_ack` is the site where a confirmed-delivered DM must
// both resolve its outstanding-sends entry AND raise
// `UiEvent::DmAcked { to_hash, .. }` for the UI to act on — before this
// mission's predecessor fix it did the former but never the latter, so
// `ui::UiRuntime::handle_event`'s otherwise-correct `DmAcked` handler
// (mod.rs) simply never fired.
#[cfg(test)]
mod tests {
    use super::*;

    fn dm_send(ack_hash: [u8; 4], to_hash: u8, sent_at_ms: u64) -> OutstandingSend {
        OutstandingSend::new(ack_hash, OutstandingKind::Dm { to_hash }, sent_at_ms, b"stub dm frame")
    }

    #[test]
    fn matching_ack_resolves_and_raises_dm_acked_with_right_contact() {
        let mut outstanding = OutstandingSends::new();
        assert!(outstanding.insert(dm_send([1, 2, 3, 4], 0x42, 0)).is_none());
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        let matched = match_outstanding_ack([1, 2, 3, 4], &mut outstanding, &mut ui_events);

        assert!(matched, "a matched ack must report matched");
        assert!(outstanding.resolve([1, 2, 3, 4]).is_none(), "a matched ack must resolve the entry");
        assert_eq!(ui_events.len(), 1, "a matched ack must raise exactly one UI event");
        match &ui_events[0] {
            ui::UiEvent::DmAcked { to_hash, is_channel, ack_hash } => {
                assert_eq!(*to_hash, 0x42);
                assert_eq!(*ack_hash, [1, 2, 3, 4]);
                // A genuine DM ack must NOT claim `is_channel: true` — that
                // flag is what `UiRuntime::handle_event` uses to find the
                // right open `MessageView` to redraw (see the variant's
                // doc); getting it wrong is this mission's whole defect.
                assert!(!is_channel, "a DM ack must raise DmAcked with is_channel: false");
            }
            other => panic!("expected DmAcked, got {:?}", other),
        }
    }

    #[test]
    fn mismatched_ack_leaves_entry_outstanding_and_raises_no_event() {
        let mut outstanding = OutstandingSends::new();
        assert!(outstanding.insert(dm_send([1, 2, 3, 4], 0x42, 0)).is_none());
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        let matched = match_outstanding_ack([9, 9, 9, 9], &mut outstanding, &mut ui_events);

        assert!(!matched);
        assert!(outstanding.resolve([1, 2, 3, 4]).is_some(), "a mismatched ack must not resolve the entry");
        assert!(ui_events.is_empty(), "a mismatched ack must not raise a UI event");
    }

    #[test]
    fn ack_with_nothing_outstanding_raises_no_event() {
        let mut outstanding = OutstandingSends::new();
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        let matched = match_outstanding_ack([1, 2, 3, 4], &mut outstanding, &mut ui_events);

        assert!(!matched);
        assert!(ui_events.is_empty(), "an unexpected ack must not raise a UI event");
    }

    /// REGRESSION (this mission's acceptance criterion): two DMs sent
    /// back-to-back to the SAME contact must each track and resolve
    /// independently — the SECOND DM's ack arriving first must flip only
    /// the second send, leaving the first genuinely outstanding.
    #[test]
    fn two_dms_to_the_same_contact_resolve_independently_out_of_order() {
        let mut outstanding = OutstandingSends::new();
        assert!(outstanding.insert(dm_send([1, 1, 1, 1], 0x42, 0)).is_none());
        assert!(outstanding.insert(dm_send([2, 2, 2, 2], 0x42, 1)).is_none());
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        // The SECOND DM's ack arrives first.
        assert!(match_outstanding_ack([2, 2, 2, 2], &mut outstanding, &mut ui_events));
        assert_eq!(ui_events.len(), 1);
        assert!(
            matches!(&ui_events[0], ui::UiEvent::DmAcked { ack_hash, .. } if *ack_hash == [2, 2, 2, 2]),
        );

        // The first DM is still genuinely outstanding — this is exactly the
        // ambiguity a single-slot `PendingAck` made unreachable and this
        // table must not reintroduce.
        assert!(match_outstanding_ack([1, 1, 1, 1], &mut outstanding, &mut ui_events));
        assert_eq!(ui_events.len(), 2);
        assert!(
            matches!(&ui_events[1], ui::UiEvent::DmAcked { ack_hash, .. } if *ack_hash == [1, 1, 1, 1]),
        );
    }

    /// Forward-compat regression: a v1.16 node emits a 6-byte ACK payload
    /// (`ack_hash[0..4]` + extended-attempt byte + random byte). `handle_ack`
    /// must accept it and prefix-match on the first 4 bytes only, exactly as
    /// it does for a v1.15 4-byte payload.
    #[test]
    fn handle_ack_accepts_and_prefix_matches_a_6_byte_v1_16_ack() {
        let mut outstanding = OutstandingSends::new();
        assert!(outstanding.insert(dm_send([1, 2, 3, 4], 0x42, 0)).is_none());
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        // [ack_hash(4)] [extended-attempt byte] [random byte]
        let payload_6_byte = [1u8, 2, 3, 4, 0x07, 0xFE];
        handle_ack(&payload_6_byte, &mut outstanding, &mut [], &mut ui_events, 0);

        assert!(
            outstanding.resolve([1, 2, 3, 4]).is_none(),
            "a prefix-matched 6-byte ack must resolve the outstanding entry"
        );
        assert_eq!(ui_events.len(), 1, "a prefix-matched 6-byte ack must raise exactly one UI event");
        match &ui_events[0] {
            ui::UiEvent::DmAcked { to_hash, is_channel, .. } => {
                assert_eq!(*to_hash, 0x42);
                assert!(!is_channel, "a DM ack must raise DmAcked with is_channel: false");
            }
            other => panic!("expected DmAcked, got {:?}", other),
        }
    }

    /// An evicted or deadline-expired outstanding send raises `DmUndelivered`
    /// (red check), not `DmAcked` — the tri-state model's third state.
    #[test]
    fn delivery_event_undelivered_raises_dm_undelivered_not_dm_acked() {
        let send = dm_send([1, 2, 3, 4], 0x42, 0);
        match delivery_event(send, false) {
            ui::UiEvent::DmUndelivered { to_hash, is_channel, ack_hash } => {
                assert_eq!(to_hash, 0x42);
                assert!(!is_channel);
                assert_eq!(ack_hash, [1, 2, 3, 4]);
            }
            other => panic!("expected DmUndelivered, got {:?}", other),
        }
    }

    /// A room-post `OutstandingSend` maps to the SAME `DmAcked`/
    /// `DmUndelivered` event shape as a DM, with `is_channel: true` and
    /// `to_hash` set to the room's hash — see `delivery_event`'s doc.
    #[test]
    fn delivery_event_room_post_sets_is_channel_true() {
        let send = OutstandingSend::new([5, 6, 7, 8], OutstandingKind::RoomPost { room_hash: 0x99 }, 0, b"stub room frame");
        match delivery_event(send, true) {
            ui::UiEvent::DmAcked { to_hash, is_channel, ack_hash } => {
                assert_eq!(to_hash, 0x99);
                assert!(is_channel, "a room post ack must raise DmAcked with is_channel: true");
                assert_eq!(ack_hash, [5, 6, 7, 8]);
            }
            other => panic!("expected DmAcked, got {:?}", other),
        }
    }

    // ── log_tx_queue_eviction / insert_outstanding — main.rs's own eviction
    // wiring, not just the underlying `OutstandingSends` primitives ─────────
    //
    // Acceptance criterion: "an evicted frame renders red." The table-level
    // mechanics (`resolve_evicted`) are pinned in `firmware-core`; these two
    // tests pin the GLUE in THIS file — that a real `TxQueue` eviction (or a
    // full outstanding-sends table) actually reaches `delivery_event` and
    // produces a `DmUndelivered` the caller's `emit` closure receives.

    #[test]
    fn log_tx_queue_eviction_of_a_tagged_frame_raises_dm_undelivered() {
        let mut outstanding = OutstandingSends::new();
        assert!(outstanding.insert(dm_send([1, 2, 3, 4], 0x42, 0)).is_none());
        let mut txq = TxQueue::new();
        // Fill the queue, then enqueue one more untagged frame to evict the
        // oldest — which happens to be the tagged one enqueued first.
        assert_eq!(txq.enqueue(&[0xAA], Some([1, 2, 3, 4])), None);
        for i in 0..(dispatcher::TX_QUEUE_SLOTS - 1) {
            assert_eq!(txq.enqueue(&[i as u8], None), None);
        }
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();
        log_tx_queue_eviction(
            txq.enqueue(&[0xFF], None),
            "test frame",
            &mut outstanding,
            |ev| ui_events.push(ev),
        );

        assert_eq!(ui_events.len(), 1, "the evicted tagged frame must raise exactly one event");
        match &ui_events[0] {
            ui::UiEvent::DmUndelivered { to_hash, ack_hash, .. } => {
                assert_eq!(*to_hash, 0x42);
                assert_eq!(*ack_hash, [1, 2, 3, 4]);
            }
            other => panic!("expected DmUndelivered, got {:?}", other),
        }
        assert!(
            outstanding.resolve([1, 2, 3, 4]).is_none(),
            "the evicted entry must be resolved (removed), not left dangling"
        );
    }

    #[test]
    fn log_tx_queue_eviction_of_an_untagged_frame_raises_no_event() {
        let mut outstanding = OutstandingSends::new();
        let mut txq = TxQueue::new();
        for i in 0..dispatcher::TX_QUEUE_SLOTS {
            assert_eq!(txq.enqueue(&[i as u8], None), None);
        }
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();
        log_tx_queue_eviction(
            txq.enqueue(&[0xFF], None),
            "test frame",
            &mut outstanding,
            |ev| ui_events.push(ev),
        );
        assert!(ui_events.is_empty(), "an untagged eviction (e.g. a room login) has nothing to resolve");
    }

    #[test]
    fn insert_outstanding_full_table_evicts_oldest_and_raises_dm_undelivered() {
        let mut outstanding = OutstandingSends::new();
        for i in 0..dispatcher::MAX_OUTSTANDING_SENDS {
            let mut evicted_events: Vec<ui::UiEvent> = Vec::new();
            insert_outstanding(
                &mut outstanding,
                dm_send([i as u8, 0, 0, 0], i as u8, i as u64),
                |ev| evicted_events.push(ev),
            );
            assert!(evicted_events.is_empty(), "table not yet full at entry {}", i);
        }
        // One more: the table is full, so the OLDEST (sent_at_ms == 0) is
        // evicted and must raise `DmUndelivered`.
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();
        insert_outstanding(
            &mut outstanding,
            dm_send([0xFF, 0, 0, 0], 0xFF, 1000),
            |ev| ui_events.push(ev),
        );
        assert_eq!(ui_events.len(), 1);
        match &ui_events[0] {
            ui::UiEvent::DmUndelivered { to_hash, ack_hash, .. } => {
                assert_eq!(*to_hash, 0);
                assert_eq!(*ack_hash, [0, 0, 0, 0]);
            }
            other => panic!("expected DmUndelivered, got {:?}", other),
        }
    }

    // Regression guard for the channel counterpart: a heard repeat of our own
    // outbound GRP_TXT send must both clear `pending_channel_ack` AND raise
    // `UiEvent::ChannelAcked { channel_hash }`, exactly once.

    #[test]
    fn matching_repeat_clears_pending_and_raises_channel_acked_with_right_channel() {
        let mut pending = Some(PendingChannelAck { hash: [1, 2, 3, 4], channel_hash: 0x7a });
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        let got = match_pending_channel_ack([1, 2, 3, 4], &mut pending, &mut ui_events);

        assert_eq!(got, Some(0x7a), "must return the acked channel_hash");
        assert!(pending.is_none(), "a matched repeat must clear pending_channel_ack");
        assert_eq!(ui_events.len(), 1, "a matched repeat must raise exactly one UI event");
        match &ui_events[0] {
            ui::UiEvent::ChannelAcked { channel_hash } => assert_eq!(*channel_hash, 0x7a),
            other => panic!("expected ChannelAcked, got {:?}", other),
        }
    }

    #[test]
    fn mismatched_repeat_leaves_pending_and_raises_no_event() {
        let mut pending = Some(PendingChannelAck { hash: [1, 2, 3, 4], channel_hash: 0x7a });
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        let got = match_pending_channel_ack([9, 9, 9, 9], &mut pending, &mut ui_events);

        assert_eq!(got, None);
        assert!(pending.is_some(), "a mismatched repeat must not clear pending_channel_ack");
        assert!(ui_events.is_empty(), "a mismatched repeat must not raise a UI event");
    }

    #[test]
    fn repeat_with_no_pending_channel_send_raises_no_event() {
        let mut pending: Option<PendingChannelAck> = None;
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        let got = match_pending_channel_ack([1, 2, 3, 4], &mut pending, &mut ui_events);

        assert_eq!(got, None);
        assert!(pending.is_none());
        assert!(ui_events.is_empty(), "an unexpected repeat must not raise a UI event");
    }

    /// A SECOND repeat of the same message, after the first already cleared
    /// `pending_channel_ack`, must not re-raise the event — idempotent on
    /// repeat count, matching the "on the FIRST detected repeat"
    /// requirement.
    #[test]
    fn second_repeat_after_first_match_is_idempotent() {
        let mut pending = Some(PendingChannelAck { hash: [1, 2, 3, 4], channel_hash: 0x7a });
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        let first = match_pending_channel_ack([1, 2, 3, 4], &mut pending, &mut ui_events);
        assert_eq!(first, Some(0x7a));
        assert_eq!(ui_events.len(), 1);

        // Same frame heard again (a second relay repeating it).
        let second = match_pending_channel_ack([1, 2, 3, 4], &mut pending, &mut ui_events);
        assert_eq!(second, None, "a second repeat has nothing pending to match anymore");
        assert_eq!(ui_events.len(), 1, "no additional UI event on the second repeat");
    }

    // ── Room post-ACK regression guard ──────────────────────────────────────
    //
    // The predecessor defect this table's design closes structurally: a room
    // post's ACK routinely arrives bundled inside a PATH-return
    // (`handle_path_return`'s `PathExtra::Ack` arm), not just as a bare
    // `Ack` datagram (`handle_ack`) — see `match_outstanding_ack`'s doc for
    // why (posts are flood-routed, so the responder often teaches its
    // return route back by bundling the ACK, exactly like the flood-login's
    // bundled `PathExtra::Response`). `handle_ack` and
    // `handle_path_return`'s `PathExtra::Ack` arm now BOTH delegate to
    // `match_outstanding_ack`, so exercising that one function is the shared
    // regression guard for both call sites.

    fn room_post_send(ack_hash: [u8; 4], room_hash: u8, sent_at_ms: u64) -> OutstandingSend {
        OutstandingSend::new(ack_hash, OutstandingKind::RoomPost { room_hash }, sent_at_ms, b"stub room frame")
    }

    #[test]
    fn matching_room_post_ack_resolves_and_raises_dm_acked_for_room_hash() {
        let mut outstanding = OutstandingSends::new();
        assert!(outstanding.insert(room_post_send([5, 6, 7, 8], 0x99, 0)).is_none());
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        let matched = match_outstanding_ack([5, 6, 7, 8], &mut outstanding, &mut ui_events);

        assert!(matched, "a matching room post ack must report matched");
        assert!(
            outstanding.resolve([5, 6, 7, 8]).is_none(),
            "a matched ack must resolve the entry"
        );
        assert_eq!(ui_events.len(), 1, "a matched room post ack must raise exactly one UI event");
        match &ui_events[0] {
            ui::UiEvent::DmAcked { to_hash, is_channel, ack_hash } => {
                assert_eq!(*to_hash, 0x99);
                assert_eq!(*ack_hash, [5, 6, 7, 8]);
                // Regression guard for `meshcadet-room-ack-check-no-live-
                // redraw`: a room post's ack must set `is_channel: true` or
                // `UiRuntime::handle_event`'s live-redraw guard silently
                // skips the currently-open room view (fixed only by
                // re-navigating), because rooms open their `MessageView` as
                // `(room_hash, is_channel: true)`.
                assert!(is_channel, "a room post ack must raise DmAcked with is_channel: true");
            }
            other => panic!("expected DmAcked, got {:?}", other),
        }
    }

    #[test]
    fn mismatched_room_post_ack_leaves_entry_outstanding_and_reports_unmatched() {
        let mut outstanding = OutstandingSends::new();
        assert!(outstanding.insert(room_post_send([5, 6, 7, 8], 0x99, 0)).is_none());
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        let matched = match_outstanding_ack([1, 2, 3, 4], &mut outstanding, &mut ui_events);

        assert!(!matched);
        assert!(outstanding.resolve([5, 6, 7, 8]).is_some());
        assert!(ui_events.is_empty());
    }

    /// REGRESSION: a room post and a DM can be outstanding at the same time
    /// (this mission's whole point — the old design had two SEPARATE
    /// single-slot trackers, `PendingAck` and `RoomRuntime::pending_post_ack`,
    /// which happened to make this case untestable-as-a-collision by
    /// construction; the new shared table must not reintroduce cross-talk
    /// between kinds). Drives `handle_ack`'s full dispatch order: a room
    /// post ack must be matched and must NOT disturb the unrelated
    /// outstanding DM.
    #[test]
    fn handle_ack_matches_room_post_ack_without_disturbing_an_unrelated_outstanding_dm() {
        let mut outstanding = OutstandingSends::new();
        assert!(outstanding.insert(dm_send([9, 9, 9, 9], 0x11, 0)).is_none());
        assert!(outstanding.insert(room_post_send([5, 6, 7, 8], 0x99, 0)).is_none());
        let mut rooms: [RoomRuntime; 0] = [];
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        handle_ack(&[5, 6, 7, 8], &mut outstanding, &mut rooms, &mut ui_events, 0);

        assert!(
            outstanding.resolve([5, 6, 7, 8]).is_none(),
            "the room's post ack must be resolved"
        );
        assert!(
            outstanding.resolve([9, 9, 9, 9]).is_some(),
            "the unrelated outstanding DM must be untouched"
        );
        assert_eq!(ui_events.len(), 1);
        match &ui_events[0] {
            ui::UiEvent::DmAcked { to_hash, is_channel, .. } => {
                assert_eq!(*to_hash, 0x99);
                assert!(is_channel, "a room post ack must raise DmAcked with is_channel: true");
            }
            other => panic!("expected DmAcked, got {:?}", other),
        }
    }
}
