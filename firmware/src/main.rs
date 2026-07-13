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
};

#[cfg(not(feature = "hil"))]
use esp_idf_hal::i2c::{I2cDriver, config::Config as I2cConfig};
#[cfg(not(feature = "hil"))]
use esp_idf_hal::ledc::{LedcTimerDriver, config::TimerConfig as LedcTimerConfig};

mod battery;
mod dispatcher;
mod gps;
mod gps_baud_store;
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
mod radio;
mod signal_tracker;
mod ui;
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

use battery::BatteryDriver;
use dispatcher::{AirtimeBudget, DuplicateFilter, TxQueue, lora_airtime_ms};
use gps::GpsDriver;
use radio::Radio;
use signal_tracker::{SignalConfig, SignalTracker};

// ── RX diagnostic log macro ───────────────────────────────────────────────────

#[cfg(feature = "hil")]
macro_rules! rx_diag {
    ($($arg:tt)*) => { log::info!($($arg)*) }
}
#[cfg(not(feature = "hil"))]
macro_rules! rx_diag {
    ($($arg:tt)*) => { log::debug!($($arg)*) }
}

// ── HIL test channel ──────────────────────────────────────────────────────────

#[cfg(not(feature = "hil"))]
const HIL_TEST_CHANNEL_SECRET: [u8; 32] = [0x6du8; 32]; // 'm' — meshcadet HIL channel

#[cfg(feature = "hil")]
const TX_INTERVAL_MS: u64 = 30_000;

// ── Pending outbound ACK ──────────────────────────────────────────────────────

/// The ACK hash we're waiting for on our most-recently-sent DM, together with
/// the contact it was sent to.
///
/// Before this, the dispatcher tracked only the bare `[u8; 4]` ack hash — the
/// value needed to recognise a matching inbound ACK/PATH-return, but nothing
/// tying it back to *which contact's* pending message it belongs to. That was
/// enough to log "ACK received: matches last-sent DM" but not enough to raise
/// `ui::UiEvent::DmAcked { to_hash }`, which needs `to_hash` to find the right
/// `MessageRecord` to flip in `UiRuntime::handle_event`. Pairing `to_hash`
/// alongside the hash here at enqueue time is what closes that gap.
struct PendingAck {
    hash: [u8; 4],
    to_hash: u8,
}

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
/// Single-slot, same known limitation as `PendingAck` above: only the most
/// recently sent channel message's ack is ever recognised live.
struct PendingChannelAck {
    hash: [u8; 4],
    channel_hash: u8,
}

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
    //     both the LCD SpiDeviceDriver and the radio SpiDeviceDriver (step 5b,
    //     after the provisioning gate).
    let spi_driver = SpiDriver::new(
        peripherals.spi2,
        peripherals.pins.gpio40,          // SCK
        peripherals.pins.gpio41,          // MOSI
        Some(peripherals.pins.gpio38),    // MISO
        &SpiDriverConfig::new(),
    )?;

    // 7. Touch UI — display + touch + notification runtime (production only).
    //
    // The T-Deck Plus SPI bus (GPIO40/41/38) is shared between the SX1262 radio
    // (CS=GPIO9) and the ST7789 LCD (CS=GPIO12). Both devices use SPI2 via
    // borrowed SpiDeviceDriver<'_, &SpiDriver<'_>>, enforcing that only one CS
    // is asserted at a time (software-serialised by the single-task loop).
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

    #[cfg(not(feature = "hil"))]
    let mut ui_opt: Option<ui::UiRuntime<'_>> = {
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
        let touch_result = I2cDriver::new(
            peripherals.i2c1,
            peripherals.pins.gpio18,        // SDA
            peripherals.pins.gpio8,         // SCL
            &I2cConfig::new().baudrate(100_000u32.Hz()),
        );

        // ── LCD SPI device — shares SPI2 (same bus as radio, CS=GPIO12) ─────
        // FIX: use &spi_driver (borrow) so GPIO40/41 stay under SPI2 control.
        // The broken integration created a SEPARATE SPI3 on GPIO40/41, which
        // remapped those GPIOs away from SPI2 and silenced the radio.
        let lcd_spi_result = SpiDeviceDriver::new(
            &spi_driver,
            Some(peripherals.pins.gpio12), // LCD CS
            &SpiConfig::new().baudrate(40u32.MHz().into()),
        );

        let dc  = PinDriver::output(peripherals.pins.gpio11)?; // LCD DC
        let rst = PinDriver::output(peripherals.pins.gpio16)?; // LCD RST
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

        match (touch_result, lcd_spi_result) {
            (Ok(i2c), Ok(lcd_spi)) => {
                // The GT911 touch IC and the T-Deck keyboard co-processor share
                // I2C1.  Wrap the single I2cDriver in an Rc<RefCell> so both
                // drivers can borrow the bus from the cooperative UI task; the
                // borrows are software-serialised (one transaction at a time).
                let i2c_bus: ui::touch::I2cBus<'_> =
                    std::rc::Rc::new(std::cell::RefCell::new(i2c));

                let display_result = ui::display::TDeckDisplay::new(
                    lcd_spi,
                    dc,
                    rst,
                    peripherals.ledc.channel1,
                    bl_timer,
                    peripherals.pins.gpio42,
                );
                match display_result {
                    Ok(display) => {
                        match ui::touch::TouchDriver::new(i2c_bus.clone()) {
                            Ok(touch) => {
                                // Probe the physical QWERTY keyboard co-processor
                                // (0x55) on the same bus.  Absence is non-fatal:
                                // the UI degrades to touch-only.  A probe line is
                                // logged either way so the boot log shows it next
                                // to the GT911 line.
                                let keyboard = match ui::keyboard::KeyboardDriver::new(
                                    i2c_bus.clone(),
                                ) {
                                    Ok(kb) => Some(kb),
                                    Err(e) => {
                                        log::warn!(
                                            "keyboard co-processor probe failed: {:?}                                              — running touch-only (no physical keyboard)",
                                            e,
                                        );
                                        None
                                    }
                                };

                                // Use the already-queried provisioning state (reused by
                                // step 2.6 below — avoids a second NVS read).
                                let provisioned = prov_result.unwrap_or(false);

                                let pubkey_str = format!("{}", hex_full(&identity.pubkey));
                                // Self-name for @mention wrap (send) / self-tier
                                // highlight (receive) — see UiRuntime::self_name's
                                // doc. Read once here at UI construction, same as
                                // the channel-send path's per-send live read
                                // (device_sender_name); a name change made after
                                // boot takes effect on the next reboot for the UI
                                // copy (acceptable — mentions are a display/typing
                                // aid, not a wire-correctness concern).
                                let self_name = device_sender_name(&identity, nvs_partition.clone());
                                match ui::UiRuntime::new(display, touch, keyboard, buzzer, trackball, provisioned, &pubkey_str, &self_name) {
                                    Ok(runtime) => {
                                        log::info!(
                                            "touch UI runtime initialised — {}×240 ST7789 + GT911",
                                            ui::display::DISPLAY_WIDTH,
                                        );
                                        Some(runtime)
                                    }
                                    Err(e) => {
                                        log::error!("UI runtime init failed: {:?} — running headless", e);
                                        None
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("GT911 touch probe failed: {:?} — running headless", e);
                                None
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("display init failed: {:?} — running headless", e);
                        None
                    }
                }
            }
            (Err(e), _) => {
                log::error!("I2C/touch init failed: {:?} — running headless", e);
                None
            }
            (_, Err(e)) => {
                log::error!("LCD SPI init failed: {:?} — running headless", e);
                None
            }
        }
    };
    // HIL builds: UI is absent (no display hardware on the HIL rig).
    #[cfg(feature = "hil")]
    let mut ui_opt: Option<ui::UiRuntime<'_>> = None;

    // The loaded provisioned config — the single mutable source of truth the
    // admin_server uses to answer QUERY_STATUS / QUERY_CONTACTS / QUERY_CHANNELS
    // and to apply ADD_*/DEL_* edits (step 2.7).  Populated below from NVS; stays
    // empty on an unprovisioned device (which runs the provisioning_server
    // instead, not admin_server) or if the config blob fails to load.  Moved
    // into the admin_server thread along with an NVS handle so runtime edits
    // persist back to flash.
    #[cfg(not(feature = "hil"))]
    let mut provisioned_config = config_store::ProvisionedConfig::empty();

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
                log::info!("prov_server thread started — pumping UI while waiting");
                // Waiting for USB provisioning IS the "ready" state for an
                // unprovisioned device (no radio/GPS bring-up to wait on) —
                // signals the boot-splash dismissal gate the same way the
                // provisioned path does right before its dispatcher loop.
                if let Some(ref mut ui) = ui_opt {
                    ui.mark_app_ready();
                    // Play the boot splash's one-shot ripple on ITS OWN
                    // dedicated render loop, right here, BEFORE the
                    // USB-provisioning wait loop below starts sharing the
                    // thread with `prov_server` polling — see
                    // `UiRuntime::run_splash_ripple`'s doc for why the ripple
                    // needs its own dedicated render loop.
                    ui.run_splash_ripple();
                }
                // Pump the UI render loop until provisioning signals complete.
                // 50 ms matches the prov_server EAGAIN yield cadence and keeps
                // SPI traffic serialised on the single-task main thread.
                while !prov_done.load(std::sync::atomic::Ordering::Acquire) {
                    // Mirror the RX counter onto the on-screen display (diagnostics
                    // build only — lets the operator observe USB-serial RX activity
                    // without a serial monitor attached).
                    #[cfg(feature = "diagnostics")]
                    {
                        let rx_n = prov_rx_count.load(std::sync::atomic::Ordering::Relaxed);
                        if let Some(ref mut ui) = ui_opt {
                            ui.set_prov_rx_bytes(rx_n);
                        }
                    }
                    if let Some(ref mut ui) = ui_opt {
                        if let Err(e) = ui.step(uptime_ms()) {
                            log::warn!("UI step (unprovisioned): {:?}", e);
                        }
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
                        for i in 0..n {
                            policy.add_contact(
                                &cfg.contacts[i].pubkey,
                                cfg.contacts[i].telemetry_enable,
                            );
                            // BUG FIX: wire contact names into the UI runtime so the
                            // contact list screen shows the provisioned contacts (§B).
                            // register_contact() was defined but never called from main.rs.
                            if let Some(ref mut ui) = ui_opt {
                                let hash = cfg.contacts[i].pub_hash();
                                let name = if cfg.contacts[i].display_name_len > 0 {
                                    let len = cfg.contacts[i].display_name_len as usize;
                                    String::from_utf8_lossy(
                                        &cfg.contacts[i].display_name[..len]
                                    ).into_owned()
                                } else {
                                    format!("0x{:02x}", hash)
                                };
                                ui.register_contact(hash, name);
                            }
                        }
                        // BUG FIX: push channel list into the UI so the Channels tab
                        // shows the provisioned channel(s) (§B channels-tab acceptance).
                        if let Some(ref mut ui) = ui_opt {
                            let ch_count = cfg.channel_count as usize;
                            let channel_items: Vec<ui::screens::contact_list::ChannelItem> =
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
                                    }
                                }).collect();
                            ui.set_channels(&channel_items);
                        }
                        // Wire the provisioned PIN into the UI runtime so the
                        // settings button can gate entry via pin_menu::verify_pin.
                        if let Some(ref mut ui) = ui_opt {
                            ui.set_pin(cfg.pin, cfg.pin_len);
                        }
                        // Wire the NVS handle + load any previously-saved
                        // on-device admin-menu RuntimeSettings so the AdminMenu
                        // screen's toggles persist across reboot (separate store
                        // from the provisioning config blob — see
                        // runtime_settings_store module docs). On first boot
                        // (nothing saved yet in that store), seed the notif
                        // toggles from the admin's provisioning-time
                        // `set-notif-defaults` value rather than a hardcoded
                        // true/true default.
                        if let Some(ref mut ui) = ui_opt {
                            ui.set_nvs_partition(nvs_partition.clone());
                            let notif_defaults =
                                (cfg.notif_defaults.visual, cfg.notif_defaults.audible);
                            match runtime_settings_store::load(nvs_partition.clone(), notif_defaults) {
                                Ok(settings) => ui.set_runtime_settings(settings),
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

    // Production: the on-air channel is the PROVISIONED primary channel,
    // resolved key_len-aware (16-byte 128-bit or 32-byte 256-bit secret).
    //
    // DEFECT FIX:
    // this path previously fell through to the compiled-in HIL_TEST_CHANNEL_SECRET
    // ([0x6d;32]), so a provisioned device transmitted AND received GRP_TXT on a
    // hardcoded test channel instead of the real provisioned channel. Outbound
    // channel messages carried the wrong channel_hash/MAC (companions ignored
    // them) and inbound channel packets were dropped on the channel-hash gate in
    // `handle_grp_txt` — channel TX *and* RX silently broken in both directions,
    // while DMs kept working because the DM path resolves the provisioned contact
    // pubkey via `policy`. The provisioned channel was loaded into the UI list and
    // moved into the admin_server thread, but never wired to the dispatcher's
    // TX/RX crypto. Snapshot it here at boot (consistent with the UI channel list,
    // which is also a boot snapshot; a channel change requires a reboot to take
    // effect on air).
    #[cfg(not(feature = "hil"))]
    let (channel_secret_buf, channel_key_len): ([u8; 32], usize) = {
        let cnt = provisioned_config.channel_count as usize;
        match provisioned_config
            .primary_channel()
            .or_else(|| provisioned_config.channels[..cnt].first())
        {
            // key_len is 16 (128-bit) or 32 (256-bit); clamp defensively so a
            // malformed NVS blob can never panic the slice below.
            Some(ch) => (ch.secret, if ch.key_len == 16 { 16 } else { 32 }),
            // No channel provisioned: harmless placeholder — no channel traffic
            // is expected (or accepted) without a configured channel.
            None => (HIL_TEST_CHANNEL_SECRET, 32),
        }
    };

    let channel_secret: &[u8] = &channel_secret_buf[..channel_key_len];

    // Production diagnosability: if no channel is provisioned the channel_secret
    // above is only a placeholder, so the `channel hash=0x..` line below reads as
    // a normal hash while GRP_TXT TX/RX is effectively dead. Make that explicit.
    #[cfg(not(feature = "hil"))]
    if provisioned_config.channel_count == 0 {
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
    log::info!(
        "channel hash=0x{:02x}, policy contacts={}",
        channel_hash_var(channel_secret),
        policy.contact_count(),
    );

    // Per-boot random base for outbound message timestamps (anti-replay).
    let tx_epoch_base: u32 = unsafe { esp_idf_svc::sys::esp_random() };
    log::info!("tx timestamp base seeded (per-boot anti-replay)");

    // 5b. Initialise SX1262 radio (pins per LilyGo utilities.h).
    //     Board power enable (step 4) and SPI2 bus driver (step 5a) were moved
    //     above the provisioning gate so the display is available on first boot.
    // Radio SPI device — borrows spi_driver (from step 5a); LCD also shares SPI2.
    // (SpiDriver<'d> is &spi_driver here; Radio::init accepts the borrowed form.)
    let spi_device = SpiDeviceDriver::new(
        &spi_driver,
        Some(peripherals.pins.gpio9),     // CS
        &SpiConfig::new().baudrate(8u32.MHz().into()),
    )?;

    let rst  = PinDriver::output(peripherals.pins.gpio17)?;
    let busy = PinDriver::input(peripherals.pins.gpio13, Pull::Floating)?;
    let dio1 = PinDriver::input(peripherals.pins.gpio45, Pull::Floating)?;

    let mut radio = Radio::init(spi_device, rst, busy, dio1)?;
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
    // bring-up (SPI2, GPS UART1 above both do the same via `?`).
    let mut battery = BatteryDriver::new(peripherals.adc1, peripherals.pins.gpio4, now0)?;

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
        if let Some(ref mut ui) = ui_opt {
            match store.load_all_conversations() {
                Ok(conversations) => {
                    for (kind, conv_hash, entries) in conversations {
                        let is_channel = kind == protocol::history::HistoryMsgType::GrpTxt;
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
                                    acked,
                                    // NOTE: unused for rendering today (no message
                                    // view shows a timestamp — see `MessageRecord::
                                    // ts_ms`'s own doc comment). A future "time
                                    // sent" label should source unix-seconds from
                                    // `entry.timestamp` here instead of `0`.
                                    ts_ms: 0,
                                }
                            })
                            .collect();
                        ui.seed_conversation(conv_hash, is_channel, records);
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
        // Also pass the identity pubkey, the loaded provisioned config (the
        // mutable source of truth for QUERY_STATUS / QUERY_CONTACTS /
        // QUERY_CHANNELS and the ADD_*/DEL_* edits), and an NVS handle so runtime
        // edits persist back to flash.  The config + NVS handle are moved into
        // the thread.
        let own_pubkey = identity.pubkey;
        let nvs_for_parent = nvs_partition.clone();
        std::thread::Builder::new()
            .name("admin_server".into())
            // 12 KiB: the server now owns the loaded ProvisionedConfig (~1.6 KiB)
            // for its run lifetime and serializes a full ~1.6 KiB config blob on
            // the stack during each NVS persist (add/del edits) — headroom over
            // the prior 8 KiB so the edit path cannot overflow the thread stack.
            .stack_size(12288)
            .spawn(move || {
                admin_server::run(
                    &HISTORY,
                    &GPS_STATUS,
                    &BATTERY_STATUS,
                    own_pubkey,
                    provisioned_config,
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

    let mut pending_ack: Option<PendingAck> = None;
    let mut pending_channel_ack: Option<PendingChannelAck> = None;

    // RX counters
    let mut rx_done_count: u32 = 0;
    let mut crc_err_count: u32 = 0;
    let mut rx_none_count: u32 = 0;
    let mut last_rx_stats_ms: u64 = 0;
    const RX_STATS_INTERVAL_MS: u64 = 30_000;

    // Per-iteration RX-poll yield window.
    //
    // `radio.try_receive` does not need this wait for RX *correctness* — the
    // radio stays in continuous RX and DIO1 latches high on RxDone until
    // explicitly cleared, so a packet completing between polls is still
    // caught on the very next call regardless of how long this window is (see
    // `Radio::try_receive`'s doc). Its only job is to give the dispatcher a
    // yield cadence.
    //
    // DEFECT: this was previously 50 ms, hard-coded at the call site. Because
    // `ui.step()` (touch + keyboard polling, render) runs once per dispatcher
    // loop iteration and this RX poll dominates the iteration's wall-clock
    // cost when idle, 50 ms was the de-facto floor on how often touch/keyboard
    // state was sampled — already slower than the ≥20 ms cadence
    // `TouchDriver::poll_event`'s own doc says is needed for "interactive
    // response" (`touch.rs`), and a hard bound on the keyboard byte-drain rate
    // besides. Shrinking the window raises the loop's iteration rate (and so
    // the touch/keyboard sampling rate) roughly 10x without weakening RX
    // capture at all.
    const RX_POLL_YIELD_MS: u32 = 5;

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

    // Boot sequence complete — radio, GPS, history store, and the
    // admin-server thread are all live. Signals the boot-splash dismissal
    // gate; see `UiRuntime::mark_app_ready`.
    if let Some(ref mut ui) = ui_opt {
        ui.mark_app_ready();
        // Play the boot splash's one-shot ripple on ITS OWN dedicated render
        // loop, right here, BEFORE the dispatcher loop below starts
        // interleaving GPS/battery poll, CAD+TX, and RX poll with `ui.step()`
        // again — see `UiRuntime::run_splash_ripple`'s doc for why. This blocks
        // the main thread for ~1.15s; RX poll below resumes immediately
        // after — see that method's doc for why deferring it this briefly,
        // this once, at boot, is safe.
        ui.run_splash_ripple();
    }

    // ── Dispatcher loop ───────────────────────────────────────────────────────
    loop {
        let now = uptime_ms();

        // Pet the Task WDT: this task is still alive and iterating.
        // Called unconditionally at the top of every iteration so that any
        // stall deeper in the loop (SPI/BUSY wait, crypto, NVS write) is
        // bounded by the TWDT timeout.
        unsafe { esp_idf_svc::sys::esp_task_wdt_reset(); }

        // ── GPS poll (duty-cycle NMEA read + fix cache refresh) ──────────────
        gps.poll(now);

        // ── Battery poll (throttled ADC read + charging-trend refresh) ───────
        battery.poll(now);

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
            if let Some(ref mut ui) = ui_opt {
                ui.set_gps_status(gps_status);
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
            if let Some(ref mut ui) = ui_opt {
                ui.set_battery_status(battery_status);
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
        if let Some(ref mut ui) = ui_opt {
            ui.set_signal_level(signal_tracker.level(now));
        }

        // ── Enqueue periodic TEST DM (HIL only) ──────────────────────────────
        #[cfg(feature = "hil")]
        if now.saturating_sub(last_tx_ms) >= TX_INTERVAL_MS {
            if let Some((n, ack)) =
                build_test_dm(now, tx_epoch_base, &identity, &peer_pubkey, &shared_secret, &mut frame_buf)
            {
                txq.enqueue(&frame_buf[..n]);
                pending_ack = Some(PendingAck { hash: ack, to_hash: peer_pubkey[0] });
                log::debug!(
                    "dispatcher: enqueued TEST DM ({} bytes), expecting ack {}",
                    n,
                    hex4(&ack),
                );
            }
            last_tx_ms = now;
        }

        // ── CAD + TX ─────────────────────────────────────────────────────────
        // `now < cad_backoff_until_ms` skips the CAD attempt entirely while a
        // prior busy result is still being backed off from (see
        // `cad_backoff_until_ms`'s doc above) — this replaces what used to be
        // a blocking `FreeRtos::delay_ms(backoff_ms)` here. The loop still
        // falls through to RX poll and `ui.step()` every iteration during the
        // gate instead of stalling the whole thread.
        if txq.has_pending() && now >= cad_backoff_until_ms {
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
                        debug_assert!(
                            !PolicyFilter::is_advert_type(payload_type),
                            "policy violation: attempted to transmit an ADVERT frame (0x{:02x})",
                            payload_type,
                        );
                        let required = lora_airtime_ms(n);
                        if budget.can_transmit(now, required) {
                            match radio.transmit(&tx_frame[..n]) {
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
                None => {}
            }
        }

        // ── RX poll ──────────────────────────────────────────────────────────
        match radio.try_receive(&mut frame_buf, RX_POLL_YIELD_MS) {
            Ok(Some(n)) => {
                rx_done_count += 1;
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
                    if let Some(ref mut ui) = ui_opt {
                        for ev in ui_events {
                            ui.post_event(ev);
                        }
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
                            &mut pending_ack,
                            &mut txq,
                            gps_snapshot,
                            battery_snapshot,
                            now,
                            tx_epoch_base,
                            &mut ui_events,
                        );
                        // Persist any DM ack flip to flash so it survives a
                        // power-cycle — before this fix, `match_pending_ack`
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
                            if let ui::UiEvent::DmAcked { to_hash } = ev {
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
                        // Forward radio events to the UI runtime.
                        if let Some(ref mut ui) = ui_opt {
                            for ev in ui_events {
                                ui.post_event(ev);
                            }
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
                let hwm: u32 = unsafe {
                    esp_idf_svc::sys::uxTaskGetStackHighWaterMark(core::ptr::null_mut())
                };
                const MAIN_TASK_STACK_B: u32 = 49_152;
                log::info!(
                    "main-task stack HWM: {} B free / {} B total = {} B peak ({}% headroom)",
                    hwm,
                    MAIN_TASK_STACK_B,
                    MAIN_TASK_STACK_B.saturating_sub(hwm),
                    hwm * 100 / MAIN_TASK_STACK_B,
                );
            }
        }

        // ── Touch UI step (non-blocking; only in production builds) ──────────
        if let Some(ref mut ui) = ui_opt {
            if let Err(e) = ui.step(now) {
                log::warn!("UI step error: {:?}", e);
            }
            // Drain any commands the UI generated (send DM, etc.)
            for cmd in ui.drain_commands() {
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
                                        txq.enqueue(&frame_buf[..n]);
                                        pending_ack = Some(PendingAck { hash: ack, to_hash });
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
                    ui::UiCommand::SendGroupMsg { channel_hash, text } => {
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
                            txq.enqueue(&frame_buf[..n]);
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

/// Build a v1.15 ACK frame: `[header(0x0D)] [path_len(0x40)] [ack_hash(4)]`.
fn build_ack_frame(ack_hash: &[u8; 4], out: &mut [u8]) -> usize {
    let header = Header::new(RouteType::Flood, PayloadType::Ack);
    out[0] = header.0;
    out[1] = PathLen::new(2, 0).map(|p| p.0).unwrap_or(0x40);
    out[2..6].copy_from_slice(ack_hash);
    6
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
/// see the module gate at the top of this file) and never populate `ui_opt`,
/// so this arm is unreachable at runtime; it exists only so the
/// `UiCommand::SendGroupMsg` match arm still type-checks under `--features
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
#[allow(clippy::too_many_arguments)]
fn on_receive(
    frame: &[u8],
    our_id: &Identity,
    policy: &PolicyFilter,
    channel_secret: &[u8],
    pending_ack: &mut Option<PendingAck>,
    txq: &mut TxQueue,
    gps_snapshot: Option<(i32, i32, u32)>,
    battery_snapshot: battery::BatteryStatus,
    now_ms: u64,
    tx_epoch_base: u32,
    ui_events: &mut Vec<ui::UiEvent>,
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
            handle_dm(payload, our_id, policy, txq, gps_snapshot, now_ms, tx_epoch_base, ui_events)
        }
        x if x == PayloadType::Req as u8 => {
            handle_req(payload, our_id, policy, txq, gps_snapshot, battery_snapshot)
        }
        x if x == PayloadType::Ack as u8 => handle_ack(payload, pending_ack, ui_events),
        x if x == PayloadType::Path as u8 => {
            handle_path_return(payload, our_id, policy, pending_ack, ui_events)
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
/// A v1.15 ACK is emitted for every successfully decrypted DM from a known
/// contact, regardless of whether the text is a telemetry request or a plain
/// text message.  ACK is computed on the decrypted timestamp + type + text and
/// keyed on the originator's public key (MeshCore v1.15 §7.1).

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

fn handle_dm(
    payload: &[u8],
    our_id: &Identity,
    policy: &PolicyFilter,
    txq: &mut TxQueue,
    gps_snapshot: Option<(i32, i32, u32)>,
    now_ms: u64,
    tx_epoch_base: u32,
    ui_events: &mut Vec<ui::UiEvent>,
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
                            txq.enqueue(&reply_frame[..reply_len]);
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
            // contact (telemetry or plain text).  Keyed on originator's pubkey.
            let ack = compute_ack_hash(ts, type_byte, text, contact_pubkey);
            let mut ack_frame = [0u8; 8];
            let n = build_ack_frame(&ack, &mut ack_frame);
            txq.enqueue(&ack_frame[..n]);
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
fn handle_req(
    payload: &[u8],
    our_id: &Identity,
    policy: &PolicyFilter,
    txq: &mut TxQueue,
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
                    txq.enqueue(&resp_frame[..resp_len]);
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

/// Match an inbound ACK against the pending ACK for our last-sent DM.
fn handle_ack(payload: &[u8], pending_ack: &mut Option<PendingAck>, ui_events: &mut Vec<ui::UiEvent>) {
    if payload.len() < 4 {
        log::warn!("RX ACK: truncated ({} bytes)", payload.len());
        return;
    }
    let mut got = [0u8; 4];
    got.copy_from_slice(&payload[..4]);
    match_pending_ack(got, pending_ack, ui_events);
}

/// Compare an inbound ACK hash (bare `Ack` frame or bundled in a PATH-return)
/// against the outstanding `pending_ack`. On a match, clears `pending_ack` and
/// raises `UiEvent::DmAcked { to_hash }` so the touch UI can flip the sender's
/// indicator from single-grey to double-check (previously this only logged
/// the match and never told the UI which contact's message it was for).
fn match_pending_ack(got: [u8; 4], pending_ack: &mut Option<PendingAck>, ui_events: &mut Vec<ui::UiEvent>) {
    match pending_ack {
        Some(expected) if expected.hash == got => {
            log::info!(
                "ACK received: matches last-sent DM (ack_hash={}, to_hash=0x{:02x})",
                hex4(&got), expected.to_hash,
            );
            ui_events.push(ui::UiEvent::DmAcked { to_hash: expected.to_hash });
            *pending_ack = None;
        }
        Some(expected) => {
            log::warn!(
                "ACK received but no match (got {}, expected {})",
                hex4(&got),
                hex4(&expected.hash),
            );
        }
        None => {
            log::info!("ACK received (no pending DM): ack_hash={}", hex4(&got));
        }
    }
}

/// Compare a duplicate-detected inbound frame's dedup key
/// (`protocol::packet_dedup_key`) against the outstanding `pending_channel_ack`.
/// On a match, clears `pending_channel_ack`, raises
/// `UiEvent::ChannelAcked { channel_hash }`, and returns `Some(channel_hash)`
/// so the caller can also persist the flip to flash.
///
/// Mirrors `match_pending_ack`'s DM counterpart, but keyed on the packet
/// dedup hash rather than a v1.15 ACK hash: a GRP_TXT has no per-recipient
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

/// Handle a PATH-return (0x08) — decrypt and extract bundled ACK.
fn handle_path_return(
    payload: &[u8],
    our_id: &Identity,
    policy: &PolicyFilter,
    pending_ack: &mut Option<PendingAck>,
    ui_events: &mut Vec<ui::UiEvent>,
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
                PathExtra::Ack(got) => match_pending_ack(got, pending_ack, ui_events),
                PathExtra::None => {
                    rx_diag!("RX PATH: no bundled ACK (extra=None)");
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

/// Decode + log an inbound GRP_TXT under the HIL test-channel secret.
fn handle_grp_txt(payload: &[u8], channel_secret: &[u8], ui_events: &mut Vec<ui::UiEvent>) {
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
// FINAL TRIAGE (firmware-core-extract-ui-runtime increment): these 7 tests
// stay here, device-only/compile-only — leave-as-is, per this campaign's
// bucket (c). `match_pending_ack`/`match_pending_channel_ack` themselves are
// in fact pure (`PendingAck`/`PendingChannelAck` are plain structs local to
// this file, and `log::info!`/`log::warn!` are the only side effects), so
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
// this campaign follows.
//
// Regression guard for "live ACK never advances the ✓→✓✓ indicator":
// `match_pending_ack` is the site where
// a confirmed-delivered DM must both clear `pending_ack` AND raise
// `UiEvent::DmAcked { to_hash }` for the UI to act on — before this fix it did
// the former but never the latter, so `ui::UiRuntime::handle_event`'s
// otherwise-correct `DmAcked` handler (mod.rs) simply never fired.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_ack_clears_pending_and_raises_dm_acked_with_right_contact() {
        let mut pending = Some(PendingAck { hash: [1, 2, 3, 4], to_hash: 0x42 });
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        match_pending_ack([1, 2, 3, 4], &mut pending, &mut ui_events);

        assert!(pending.is_none(), "a matched ack must clear pending_ack");
        assert_eq!(ui_events.len(), 1, "a matched ack must raise exactly one UI event");
        match &ui_events[0] {
            ui::UiEvent::DmAcked { to_hash } => assert_eq!(*to_hash, 0x42),
            other => panic!("expected DmAcked, got {:?}", other),
        }
    }

    #[test]
    fn mismatched_ack_leaves_pending_and_raises_no_event() {
        let mut pending = Some(PendingAck { hash: [1, 2, 3, 4], to_hash: 0x42 });
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        match_pending_ack([9, 9, 9, 9], &mut pending, &mut ui_events);

        assert!(pending.is_some(), "a mismatched ack must not clear pending_ack");
        assert!(ui_events.is_empty(), "a mismatched ack must not raise a UI event");
    }

    #[test]
    fn ack_with_no_pending_dm_raises_no_event() {
        let mut pending: Option<PendingAck> = None;
        let mut ui_events: Vec<ui::UiEvent> = Vec::new();

        match_pending_ack([1, 2, 3, 4], &mut pending, &mut ui_events);

        assert!(pending.is_none());
        assert!(ui_events.is_empty(), "an unexpected ack must not raise a UI event");
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
}
