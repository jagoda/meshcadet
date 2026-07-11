// SPDX-License-Identifier: GPL-3.0-only
//! GPS driver for LilyGo T-Deck Plus — Quectel L76K or u-blox M10Q GNSS module.
//!
//! # Hardware (from LilyGo `examples/UnitTest/utilities.h` and `examples/GPSShield/`)
//!
//! | Signal | GPIO | Direction |
//! |--------|------|-----------|
//! | GPS TX | 43   | ESP32-S3 → GPS RX |
//! | GPS RX | 44   | GPS TX → ESP32-S3  |
//! | Baud rate | 9600 (L76K), 38400 (u-blox M10Q), or 115200 (catch-all) | 8N1, no flow control, probed at boot / cached in NVS |
//! | Module | Quectel L76K *or* u-blox M10Q | GNSS receiver — varies unit-to-unit |
//!
//! The T-Deck Plus wires the GPS to UART1 (routed to GPIO43/44 via the GPIO
//! matrix).  UART0 (console) is redirected to USB-Serial-JTAG via
//! `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y` in `sdkconfig.defaults` so that
//! GPIO43/44 are exclusively available to the GPS UART.
//!
//! # Baud-rate probing + caching
//!
//! The T-Deck Plus GPS is documented as inconsistent hardware: units ship
//! with either a Quectel L76K (default 9600 bps) or a u-blox M10Q (default
//! 38400 bps), and there is no way to tell which is on a given board without
//! probing it at runtime. A field capture (`--features diagnostics` raw-RX
//! hexdump) showed bytes arriving steadily on UART1 RX but decoding as
//! structured, periodic, high-bit-set binary rather than ASCII NMEA — the
//! textbook signature of sampling a real bitstream at the wrong baud rate,
//! not a dead module.
//!
//! By design, [`GpsDriver::new`] probes
//! [`GPS_BAUD_CANDIDATES`] (9600, 38400, 115200 — see [`probe_candidates`])
//! and locks onto whichever rate yields a COMPLETE, CHECKSUM-VALID NMEA
//! sentence (see [`contains_checksum_valid_nmea`]) — not merely a
//! `$…*`-shaped span — before sending any init commands. The result is
//! persisted to [`gps_baud_store`](crate::gps_baud_store) (NVS) so probing
//! is a one-time (first-boot) cost: later boots switch straight to the
//! cached rate with no verification probe, self-healing via a full re-probe
//! only if that cached rate turns out to produce no NMEA traffic (see
//! [`GpsDriver::maybe_reprobe_stale_cached_baud`]).
//!
//! `GpsDriver::new`'s probe above runs BEFORE `main.rs`'s dispatcher loop
//! (and so before the boot splash's render loop) ever starts, so it may block
//! synchronously without visible effect. The runtime self-heal re-probe is a
//! different story: it runs from [`GpsDriver::poll`], called every
//! dispatcher-loop iteration — including during the splash's on-screen
//! window — so this was rewritten
//! as a non-blocking state machine ([`GpsDriver::begin_reprobe_window`] /
//! [`GpsDriver::service_reprobe`]) serviced one small slice per `poll()` call,
//! rather than a single call that used to block for up to ~3.3 s. See
//! `service_reprobe`'s doc for the full mechanism this closes.
//!
//! # Duty cycle (~2-min period, ~30-s active window — AFTER first fix)
//!
//! To conserve power the driver alternates between an active reading window
//! and a quiescent interval, but **only once a fix has been captured at least
//! once since boot**:
//!
//! ```text
//! ──[ACTIVE until first fix]──[ACTIVE 30 s]──[QUIET 120 s]──[ACTIVE 30 s]── …
//! ```
//!
//! Rationale: ADR-0001 promises a
//! *cached last-known fix, refreshed periodically* — that promise presumes a
//! fix already exists to cache. Applying the power-conserving duty cycle
//! before the first fix instead throttles time-to-first-fix: a cold
//! acquisition that completes mid-QUIET-interval would sit unreported for up
//! to `GPS_QUIET_INTERVAL_MS` before the next ACTIVE window reads it, and the
//! L76K's own NMEA stream is not read at all during QUIET (see below), so the
//! host cannot even tell acquisition is in progress. Staying continuously
//! ACTIVE until [`GpsDriver::cached_fix`] is first populated removes that
//! self-inflicted latency; the duty cycle then applies as documented for
//! refreshing an already-cached fix, where the charter's "instant-fix not
//! required" explicitly applies.
//!
//! During an active window, UART bytes are drained on every [`GpsDriver::poll`]
//! call and accumulated into a line buffer.  Complete NMEA sentences are
//! parsed; a valid fix updates the cached [`GpsFix`].  Outside an active
//! window, the UART is not read (the L76K continues emitting NMEA on its own;
//! bytes accumulate in the ESP32-S3 UART hardware FIFO and are discarded when
//! the window reopens).
//!
//! # Cached fix
//!
//! [`GpsDriver::get_fix_and_age`] always returns the best available fix: if a
//! fix was ever obtained it is returned with its age in seconds.  Staleness is
//! surfaced by the caller and communicated to the telemetry requester as the
//! `age=Zs` field in the response.  When no fix has ever been obtained
//! (e.g. device indoors since power-on), `None` is returned.
//!
//! # Status feedback (fix / acquiring / no signal)
//!
//! [`GpsDriver::status`] reports a three-state [`FixState`] rather than a bare
//! has-fix boolean, so the admin-menu GPS status screen can distinguish "the
//! receiver is alive and searching" from "nothing has been heard from the GPS
//! module at all" — a distinction a binary flag collapses into one silent,
//! never-updating "No fix yet" state indistinguishable from a dead antenna or
//! wiring fault. Liveness is tracked as "has any NMEA sentence (any talker,
//! any type) been observed" — independent of whether that sentence carried a
//! valid fix — so `Acquiring` lights up the moment the L76K starts talking,
//! well before a fix is captured. Satellite count (GGA field 7, `numSV`) is
//! tracked the same way — captured from every parsed GGA sentence regardless
//! of fix quality — so the UI can show "N satellites" while still acquiring.
//!
//! # NMEA parsing
//!
//! `$GPGGA` / `$GNGGA` sentences are parsed for lat/lon + fix quality + sat
//! count.  `$GPRMC` / `$GNRMC` sentences are parsed for UTC date+time (GGA
//! carries time-of-day but no date, so RMC is the sentence used to set the
//! system clock — see "Clock sync" below).  Checksum validation is skipped
//! for minimal complexity; the very-short UART path makes bit errors
//! negligible, and each sentence's own validity field (GGA fix-quality ≥ 1 /
//! RMC status `A`) acts as an application-level validity gate.
//!
//! # Clock sync
//!
//! The T-Deck Plus has no battery-backed RTC: system time is volatile and
//! resets to the ESP-IDF epoch (boot = 1970-01-01T00:00:00Z) on every
//! power-off.  [`GpsDriver::parse_line`] calls `settimeofday` on the first
//! (and every subsequent) valid `$GPRMC`/`$GNRMC` fix, so wall-clock time is
//! re-established shortly after boot once the GPS acquires a fix. Sync state
//! (`synced` / `never-synced` + last-sync age) is exposed via
//! [`GpsDriver::status`] for display (admin-menu GPS status view, host
//! `status` command) — there are no time-sync *controls*, only status.
//!
//! # RX diagnostics (`--features diagnostics` only)
//!
//! A field capture showed the L76K
//! init triad sent successfully (byte-accurate write counts) but zero NMEA
//! traffic ever observed — `FixState` stuck at `NoSignal`. The
//! `first-NMEA-seen` breadcrumb (see "Status feedback" above) cannot
//! distinguish "the module transmits nothing" from "bytes arrive but this
//! driver's read/parse loop drops them", because it only fires once a `$`
//! has already been recognized *by that same loop*.
//!
//! [`GpsDriver::drain_uart`] therefore counts and hex-dumps every raw byte
//! read from UART1 *before* any framing/parsing is applied, logging a `GPS:
//! raw RX n=… total=… tail=[…]` line whenever bytes arrive and a `GPS: raw
//! RX heartbeat` line every 5 s regardless — so a desk `espflash monitor`
//! capture is conclusive either way: raw RX lines with nonzero counts prove
//! bytes are on the wire (pointing at a firmware framing/parsing bug — see
//! `drain_uart`/`parse_line`), while heartbeat-only lines (`total=0`
//! forever) prove the wire itself is silent (module/baud/antenna/wiring
//! fault, outside firmware's reach). Compiled out of production builds.

// UartDriver is used directly; FreeRtos::delay_ms paces the init command
// bursts and the baud probe's collection window below. `Hertz` is needed
// for `UartDriver::change_baudrate` when a probe (see `probe_candidates`)
// or the NVS-cache fast path (see `GpsDriver::new`) locks onto a candidate
// other than the one the UART was opened at. `EspNvsPartition`/`NvsDefault`
// and `gps_baud_store` back the baud-rate cache. Other esp-idf-hal types (Config,
// AnyIOPin, FromValueType, etc.) appear in doc-comment examples in main.rs.
use esp_idf_hal::{delay::FreeRtos, uart::UartDriver, units::Hertz};
use esp_idf_svc::nvs::{EspNvsPartition, NvsDefault};

use crate::gps_baud_store;

// Pure NMEA parsing, checksum/baud validation, duty-cycle math, and the
// baud-rate/init-command tables now live in `firmware_core::gps` so their
// tests execute under `cargo test --workspace` (this crate is a detached,
// cross-compiled workspace — see `Cargo.toml`'s doc comment — so a
// `#[cfg(test)]` block written here would type-check but never run). This
// glob brings every moved symbol (`GPS_BAUD_CANDIDATES`, `FixState`,
// `GpsStatus`, `contains_checksum_valid_nmea`, `parse_gga`,
// `parse_rmc_datetime`, `L76K_INIT_COMMANDS`, …) into scope unchanged, so
// every call site below resolves exactly as before the move.
// See `docs/adr/0005-firmware-core-extraction.md`.
pub use firmware_core::gps::*;

// ── Pin constants ─────────────────────────────────────────────────────────────
//
// TX/RX pin numbers are NOT expressed as constants here: esp-idf-hal exposes
// each GPIO as a distinct compile-time type (`peripherals.pins.gpio43`, not a
// runtime-indexable `u32`), so the wiring in `main.rs::run()` and the doc
// example below necessarily hardcode `gpio43`/`gpio44` field names — a loose
// `u32` constant could never be substituted there and would just be dead
// weight. The pin table in the module doc comment above is the source of
// truth for the numbers; `GPS_BAUD`/`GPS_BAUD_CANDIDATES` below ARE wired
// into the init call since baud rate (unlike a pin) is an ordinary runtime
// `u32`.

// `GPS_BAUD_CANDIDATES`/`GPS_BAUD` now live in `firmware_core::gps` (see the
// glob re-export above) — kept only as a doc pointer here since
// `probe_candidates`/`GpsDriver::new` below reference them heavily.

// ── Baud-rate probe ────────────────────────────────────────────────────────

/// Duration each candidate baud rate is sampled for during a probe (see
/// [`probe_candidates`]). Both known module variants emit a GGA+RMC burst at
/// ~1 Hz, so the window must comfortably exceed 1 s to avoid straddling a
/// burst boundary and missing a complete sentence; kept short otherwise so
/// probing the full candidate list does not meaningfully delay boot (or a
/// runtime re-probe — see [`GpsDriver::maybe_reprobe_stale_cached_baud`]).
const BAUD_PROBE_WINDOW_MS: u32 = 1_100;

/// Poll interval used while collecting bytes during a probe window — see
/// [`collect_probe_bytes`]. Matches the non-blocking `uart.read(_, 0)` style
/// already used by [`GpsDriver::drain_uart`].
const BAUD_PROBE_POLL_INTERVAL_MS: u32 = 20;

/// Grace period, from driver construction, that an NVS-cached baud rate
/// used WITHOUT a boot-time verification probe (see [`GpsDriver::new`]) is
/// allowed to produce zero NMEA traffic before
/// [`GpsDriver::maybe_reprobe_stale_cached_baud`] concludes the cache is
/// stale and runs a full re-probe. Generous relative to the ~1 Hz sentence
/// rate both known module variants use, to avoid false-triggering on a
/// merely slow-starting module.
const CACHED_BAUD_SILENCE_TIMEOUT_MS: u64 = 5_000;

/// Try each rate in `candidates`, in order: switch `uart` to it (via
/// `change_baudrate`), flush stale FIFO contents, sample for
/// [`BAUD_PROBE_WINDOW_MS`], and check the captured bytes for a complete,
/// checksum-valid NMEA sentence (see [`contains_checksum_valid_nmea`]).
/// Returns the first candidate that validates, `uart` left locked at that
/// rate. Returns `None` if no candidate validates; `uart` is left at
/// whatever rate the last candidate in the list was.
///
/// # Mechanism
/// A UART baud-rate mismatch does not stop bytes from arriving — the L76K
/// or u-blox module keeps transmitting on its own schedule regardless of
/// what the ESP32-S3 thinks the line rate is — it corrupts *framing*: the
/// UART hardware's bit sampler decodes the incoming waveform at the wrong
/// rate, turning genuine ASCII NMEA text into structured, periodic,
/// high-bit-set binary garbage (bytes ≥ 0x80 cannot occur in valid 7-bit
/// ASCII NMEA). This is exactly the field signature that motivated this
/// probe. Requiring a
/// *checksum-valid* sentence (not merely a `$…*`-shaped printable-ASCII
/// span) is a deliberate design choice: probing three
/// candidates instead of two gives roughly 50% more garbage-sampling
/// opportunity for a coincidental ASCII-shaped span to appear, so the
/// checksum match (an independent ~1-in-256 coincidence on top of the
/// ASCII-shape coincidence) is what keeps the false-lock rate negligible.
fn probe_candidates(uart: &UartDriver, candidates: &[u32]) -> Option<u32> {
    for &baud in candidates {
        if let Err(e) = uart.change_baudrate(Hertz(baud)) {
            log::warn!(
                "GPS: baud probe — failed to switch UART to {} bps ({:?}); skipping",
                baud, e,
            );
            continue;
        }
        // Drop whatever partial/garbage bytes accumulated at the old baud
        // (or from before this probe run) before sampling at this rate.
        let _ = uart.clear_rx();
        let (buf, len) = collect_probe_bytes(uart, BAUD_PROBE_WINDOW_MS);
        if contains_checksum_valid_nmea(&buf[..len]) {
            log::info!(
                "GPS: baud probe — {} bps yields a checksum-valid NMEA sentence, locking",
                baud,
            );
            return Some(baud);
        }
        log::info!(
            "GPS: baud probe — {} bps: {} bytes captured, no checksum-valid NMEA sentence found",
            baud, len,
        );
    }
    None
}

/// Non-blockingly collect up to 256 bytes from `uart` over `window_ms`,
/// polling every [`BAUD_PROBE_POLL_INTERVAL_MS`] — same non-blocking
/// `uart.read(_, 0)` style as [`GpsDriver::drain_uart`]. Returns a
/// fixed-size buffer and the number of bytes actually captured.
fn collect_probe_bytes(uart: &UartDriver, window_ms: u32) -> ([u8; 256], usize) {
    let mut buf = [0u8; 256];
    let mut len = 0usize;
    let mut elapsed_ms = 0u32;
    while elapsed_ms < window_ms {
        let mut byte = [0u8; 1];
        while len < buf.len() {
            match uart.read(&mut byte, 0) {
                Ok(1) => {
                    buf[len] = byte[0];
                    len += 1;
                }
                _ => break,
            }
        }
        if len >= buf.len() {
            break;
        }
        FreeRtos::delay_ms(BAUD_PROBE_POLL_INTERVAL_MS);
        elapsed_ms += BAUD_PROBE_POLL_INTERVAL_MS;
    }
    (buf, len)
}

// `nmea_checksum`/`hex_digit_value`/`contains_checksum_valid_nmea` now live
// in `firmware_core::gps` (see the glob re-export above) — `probe_candidates`
// above and `service_reprobe` below call `contains_checksum_valid_nmea`
// unqualified via that glob.

// ── Module init sequences ───────────────────────────────────────────────────
//
// Defect fix: earlier code
// assumed the L76K "powers up and immediately emits NMEA sentences in its
// default configuration" and sent it no commands at all. That assumption is
// false for the real module: its NMEA-sentence-output configuration
// (`$PCAS03`) and constellation selection (`$PCAS04`) are stored in
// non-volatile config and are NOT guaranteed to default to "GGA+RMC enabled"
// on power-up — a module left in (or shipped in) a quiesced/non-default
// state emits ZERO NMEA sentences forever, which is exactly the field
// symptom this fixes (`FixState::NoSignal` never advancing outdoors).
//
// The L76K triad is sent once per boot, right after the boot-time baud
// probe/cache lookup (see `GpsDriver::new`) locks onto `GPS_BAUD_CANDIDATES[0]`
// (9600 — the L76K's rate), and mirrors two independent, hardware-proven
// references for this exact module or lookalikes on the same/nearby T-Deck
// hardware family: LilyGo's own reference firmware
// (`examples/UnitTest/UnitTest.ino::setupGPS()`) and the production
// Meshtastic T-Deck driver (`src/gps/GPS.cpp`, `GNSS_MODEL_MTK` branch) —
// both send this same sequence (with the same 250 ms inter-command pacing)
// before relying on the L76K's NMEA stream.
//
// If the probe instead locks onto a later candidate (38400 u-blox M10Q, or
// the 115200 catch-all — see `GPS_BAUD_CANDIDATES`'s doc), the L76K's
// MTK-specific `$PCAS` triad is meaningless to it and `UBLOX_INIT_COMMANDS`
// is sent instead ("send the correct init for the
// DETECTED module — u-blox = UBX/NMEA config, not `$PCAS`"). u-blox modules
// document standard NMEA output (GGA/GLL/GSA/GSV/RMC/VTG) as enabled by
// default, so rather than the binary UBX config protocol (unverifiable
// without real u-blox hardware in this sandbox), this
// sends the vendor `$PUBX,40,...` NMEA-extension sentence u-blox receivers
// document for enabling a specific sentence on a specific port: an
// explicit, checksum-verifiable, ASCII command in the same style as the
// L76K's `$PCAS`, rather than a silent assumption that defaults are already
// correct. It only ENABLES GGA/RMC (matching what this driver parses) and
// does not attempt to disable other sentence types the way `$PCAS03` does
// for the L76K — left as a documented gap (extra UART traffic, not a
// correctness defect) rather than risk mis-configuring a receiver family
// this codebase cannot flash-test.
/// Delay between successive init commands (L76K or u-blox), matching the
/// L76K reference firmware's pacing (the module needs time to parse and
/// apply each command before the next one arrives). Reused for the u-blox
/// sequence in the absence of a documented u-blox-specific pacing
/// requirement — 250 ms is generous for either module family.
const GPS_INIT_CMD_DELAY_MS: u32 = 250;

// `L76K_INIT_COMMANDS`/`UBLOX_INIT_COMMANDS` now live in `firmware_core::gps`
// (see the glob re-export above) — `send_init_commands` below references
// them unqualified via that glob.

// `GPS_ACTIVE_WINDOW_MS`/`GPS_QUIET_INTERVAL_MS` (duty-cycle constants),
// `GpsFix`, `FixState`, and `GpsStatus` (+ its `never()`) also now live in
// `firmware_core::gps` — same glob re-export, referenced unqualified
// throughout `GpsDriver` below.

// ── GpsDriver ─────────────────────────────────────────────────────────────────

/// Duty-cycling GPS driver for the T-Deck Plus's GPS module (Quectel L76K or
/// u-blox M10Q — probed at construction, see [`GpsDriver::new`]).
///
/// Constructed once in `run()` and polled on every dispatcher-loop iteration.
/// The driver owns the UART1 peripheral for the lifetime of the running system.
pub struct GpsDriver<'d> {
    uart: UartDriver<'d>,

    // ── Baud-rate cache/self-heal state ──
    /// NVS partition handle, retained for [`maybe_reprobe_stale_cached_baud`](Self::maybe_reprobe_stale_cached_baud)
    /// to persist a freshly re-detected rate. Cheap to hold (`EspNvsPartition`
    /// is a lightweight handle, `Clone` is a refcount bump — see `esp-idf-svc`).
    nvs_partition: EspNvsPartition<NvsDefault>,
    /// The baud rate this driver is currently operating at (whichever
    /// [`GpsDriver::new`] locked onto — cached, probed, or the documented
    /// fallback).
    detected_baud: u32,
    /// `true` once `detected_baud` has been confirmed correct — either by
    /// observing real NMEA traffic, or by a self-heal re-probe having
    /// already run once (see [`maybe_reprobe_stale_cached_baud`](Self::maybe_reprobe_stale_cached_baud)).
    /// Starts `false` only when [`GpsDriver::new`] used an NVS-cached rate
    /// WITHOUT a verification probe (the fast path); starts `true` whenever
    /// a boot-time probe already validated the rate (first boot, or a
    /// failed-cache fallback), since that validation makes a runtime
    /// self-heal redundant.
    baud_confirmed: bool,
    /// Uptime ms when this driver was constructed — the reference point for
    /// [`CACHED_BAUD_SILENCE_TIMEOUT_MS`].
    driver_start_uptime_ms: u64,
    /// Self-heal re-probe progress: `None` when idle; `Some(idx)` while
    /// actively sampling `GPS_BAUD_CANDIDATES[idx]`'s window (see
    /// [`service_reprobe`](Self::service_reprobe)). Unlike the
    /// construction-time probe in [`GpsDriver::new`] — which runs strictly
    /// before the dispatcher loop / first UI render tick, so blocking there
    /// is invisible — this SAME probe algorithm, run at runtime from
    /// [`poll`](Self::poll), used to block for up to
    /// `BAUD_PROBE_WINDOW_MS * GPS_BAUD_CANDIDATES.len()` (~3.3 s) inside a
    /// single call, starving `ui::UiRuntime::step()` (touch/keyboard poll +
    /// Slint render) for that entire span whenever it fired mid-boot-splash —
    /// see `service_reprobe`'s doc for the full mechanism. These four fields
    /// turn that single blocking call into a state machine serviced by one
    /// non-blocking slice of work per `poll()` call instead.
    reprobe_candidate_idx: Option<usize>,
    /// Uptime ms the current candidate's sampling window started.
    reprobe_window_start_ms: u64,
    /// Bytes captured so far for the current candidate's window.
    reprobe_buf: [u8; 256],
    /// Number of valid bytes in `reprobe_buf`.
    reprobe_len: usize,

    // ── Cached fix ────────────────────────────────────────────────────────────
    /// Most recently validated GPS fix.  `None` until the first GGA sentence
    /// with fix quality ≥ 1 is parsed.
    cached_fix: Option<GpsFix>,

    // ── Clock sync state ──────────────────────────────────────────────────────
    /// Uptime ms when the system clock was last set from a valid RMC
    /// date+time sentence.  `None` until the first successful sync.
    last_clock_sync_uptime_ms: Option<u64>,

    // ── Liveness / acquisition-status state ─────────────────────────────────
    /// Uptime ms of the most recently observed NMEA sentence (any talker, any
    /// type) — the [`FixState::Acquiring`] vs [`FixState::NoSignal`] signal.
    /// `None` until the first sentence is seen.
    last_nmea_seen_uptime_ms: Option<u64>,
    /// Satellites used, from the most recently parsed GGA sentence (field 7),
    /// regardless of that sentence's fix quality. `0` until the first GGA.
    last_sat_count: u8,

    // ── Duty cycle state ──────────────────────────────────────────────────────
    /// `true` during an active UART reading window.
    active: bool,
    /// Uptime ms when the current period (active or quiet) started.
    period_start_ms: u64,

    // ── NMEA line accumulator ─────────────────────────────────────────────────
    /// Partial NMEA sentence accumulation buffer.
    line_buf: [u8; 128],
    /// Number of bytes currently in `line_buf`.
    line_len: usize,

    // ── Diagnostics: raw UART1 RX byte-activity counter ─────────────────────
    // `--features diagnostics` only:
    // a field-serial-log-visible, wire-level counter/hexdump that is
    // independent of NMEA framing/parsing, so a desk capture can tell
    // "zero bytes ever arrive on UART1 RX" (module/HW-side fault) apart from
    // "bytes arrive but the read/parse loop drops them" (firmware bug) —
    // see `drain_uart`'s diagnostics block for what gets logged.
    #[cfg(feature = "diagnostics")]
    rx_byte_count: u32,
    /// Uptime ms of the last diagnostics heartbeat log (see `drain_uart`) —
    /// emitted periodically even when zero bytes have arrived, so the
    /// absence of "raw RX" log lines in a capture is distinguishable from
    /// "the diagnostic build isn't running" rather than ambiguous silence.
    #[cfg(feature = "diagnostics")]
    last_rx_heartbeat_ms: u64,
}

impl<'d> GpsDriver<'d> {
    /// Construct a GPS driver from an already-configured UART1 driver.
    ///
    /// # Baud-rate determination
    ///
    /// The T-Deck Plus ships with either a Quectel L76K (9600 bps) or a
    /// u-blox M10Q (38400 bps), varying unit-to-unit, so the actual rate
    /// cannot be assumed. By design:
    ///
    /// 1. **NVS cache hit** (a previous boot already detected a working
    ///    rate): switch straight to the cached rate with NO verification
    ///    probe — "probing is a one-time (first-boot) cost". If the cached
    ///    rate later turns out stale (module swapped/reconfigured), that is
    ///    caught and corrected at runtime by
    ///    [`maybe_reprobe_stale_cached_baud`](Self::maybe_reprobe_stale_cached_baud),
    ///    not here.
    /// 2. **NVS cache miss** (first boot, or a corrupt/unreadable cache): run
    ///    the full [`GPS_BAUD_CANDIDATES`] probe (see [`probe_candidates`]),
    ///    requiring a COMPLETE, CHECKSUM-VALID NMEA sentence (not merely a
    ///    `$…*`-shaped span) before locking — see
    ///    [`contains_checksum_valid_nmea`]'s doc for why that matters across
    ///    three candidates. The winning rate is persisted to NVS so future
    ///    boots take the cache-hit path. If NO candidate validates, falls
    ///    back to `GPS_BAUD_CANDIDATES[0]` (logged, nothing persisted) — the
    ///    driver still starts and reports `FixState::NoSignal` via normal
    ///    liveness tracking rather than failing boot.
    ///
    /// Either way, [`send_init_commands`](Self::send_init_commands) is then
    /// called for the resulting rate: the L76K `$PCAS` triad for
    /// `GPS_BAUD_CANDIDATES[0]` (9600), or the u-blox `$PUBX,40` sequence for
    /// any other candidate — see that method's doc and the "Module init
    /// sequences" section above `L76K_INIT_COMMANDS` for why.
    ///
    /// The driver starts in the ACTIVE state and — per the module doc's "Duty
    /// cycle" section — STAYS active continuously until the first fix is
    /// captured, to minimize cold-start time-to-first-fix. Only after that
    /// first fix does it start alternating ACTIVE/QUIET at ~2-min intervals.
    ///
    /// # UART initialisation (in `main.rs::run()`)
    ///
    /// ```rust,ignore
    /// use esp_idf_hal::uart::{UartDriver, config::Config as UartConfig};
    /// use esp_idf_hal::units::FromValueType;
    ///
    /// let uart = UartDriver::new(
    ///     peripherals.uart1,
    ///     peripherals.pins.gpio43,                        // TX: ESP → GPS
    ///     peripherals.pins.gpio44,                        // RX: GPS → ESP
    ///     Option::<AnyIOPin>::None,                       // CTS unused
    ///     Option::<AnyIOPin>::None,                       // RTS unused
    ///     &UartConfig::new().baudrate(gps::GPS_BAUD.Hz()), // opened at the first candidate; `new` probes+locks from there
    /// )?;
    /// let gps = GpsDriver::new(uart, uptime_ms(), nvs_partition.clone());
    /// ```
    pub fn new(uart: UartDriver<'d>, now_ms: u64, nvs_partition: EspNvsPartition<NvsDefault>) -> Self {
        let cached = gps_baud_store::load_cached_baud(nvs_partition.clone());
        let (detected_baud, baud_confirmed) = match cached {
            Some(cached_baud) if cached_baud != GPS_BAUD_CANDIDATES[0] => {
                match uart.change_baudrate(Hertz(cached_baud)) {
                    Ok(_) => {
                        let _ = uart.clear_rx();
                        log::info!(
                            "GPS: baud cache — using cached rate {} bps without a boot-time \
                             probe; self-heals via a full re-probe if silent for {}ms",
                            cached_baud, CACHED_BAUD_SILENCE_TIMEOUT_MS,
                        );
                        (cached_baud, false)
                    }
                    Err(e) => {
                        log::warn!(
                            "GPS: baud cache — failed to switch UART to cached {} bps ({:?}); \
                             falling back to a full probe",
                            cached_baud, e,
                        );
                        (Self::full_probe_and_persist(&uart, nvs_partition.clone()), true)
                    }
                }
            }
            Some(cached_baud) => {
                // Cached rate equals the rate the UART is already open at
                // (main.rs opens at GPS_BAUD_CANDIDATES[0]) — nothing to switch.
                log::info!(
                    "GPS: baud cache — using cached rate {} bps (matches boot-open rate) \
                     without a boot-time probe; self-heals via a full re-probe if silent \
                     for {}ms",
                    cached_baud, CACHED_BAUD_SILENCE_TIMEOUT_MS,
                );
                (cached_baud, false)
            }
            None => (Self::full_probe_and_persist(&uart, nvs_partition.clone()), true),
        };

        Self::send_init_commands(&uart, detected_baud);

        Self {
            uart,
            nvs_partition,
            detected_baud,
            baud_confirmed,
            driver_start_uptime_ms: now_ms,
            reprobe_candidate_idx: None,
            reprobe_window_start_ms: now_ms,
            reprobe_buf: [0u8; 256],
            reprobe_len: 0,
            cached_fix: None,
            last_clock_sync_uptime_ms: None,
            last_nmea_seen_uptime_ms: None,
            last_sat_count: 0,
            active: true,          // start active to obtain first fix quickly
            period_start_ms: now_ms,
            line_buf: [0u8; 128],
            line_len: 0,
            #[cfg(feature = "diagnostics")]
            rx_byte_count: 0,
            #[cfg(feature = "diagnostics")]
            last_rx_heartbeat_ms: now_ms,
        }
    }

    /// Run the full [`GPS_BAUD_CANDIDATES`] probe and persist whatever it
    /// finds to NVS (or the documented fallback, unpersisted, if nothing
    /// validates). Shared by [`GpsDriver::new`] (NVS cache miss / cache
    /// failure) and [`maybe_reprobe_stale_cached_baud`](Self::maybe_reprobe_stale_cached_baud)
    /// (self-heal path).
    fn full_probe_and_persist(uart: &UartDriver, nvs_partition: EspNvsPartition<NvsDefault>) -> u32 {
        match probe_candidates(uart, GPS_BAUD_CANDIDATES) {
            Some(baud) => {
                gps_baud_store::save_cached_baud(nvs_partition, baud);
                baud
            }
            None => {
                log::warn!(
                    "GPS: baud probe — no candidate baud ({:?}) yielded a checksum-valid NMEA \
                     sentence; falling back to {} bps — FixState will report NoSignal until \
                     traffic is observed",
                    GPS_BAUD_CANDIDATES, GPS_BAUD_CANDIDATES[0],
                );
                if let Err(e) = uart.change_baudrate(Hertz(GPS_BAUD_CANDIDATES[0])) {
                    log::warn!(
                        "GPS: baud probe — failed to restore fallback baud {} bps ({:?})",
                        GPS_BAUD_CANDIDATES[0], e,
                    );
                }
                GPS_BAUD_CANDIDATES[0]
            }
        }
    }

    /// Send the module-appropriate init command sequence for `detected_baud`:
    /// the L76K `$PCAS` triad ([`L76K_INIT_COMMANDS`]) when locked at
    /// `GPS_BAUD_CANDIDATES[0]` (9600), or the u-blox `$PUBX,40` sequence
    /// ([`UBLOX_INIT_COMMANDS`]) for any other candidate — see the "Module
    /// init sequences" section doc above `L76K_INIT_COMMANDS` for the
    /// rationale. A write failure for any individual command is logged and
    /// non-fatal (matches this driver's existing non-fatal posture for GPS
    /// faults).
    fn send_init_commands(uart: &UartDriver, detected_baud: u32) {
        let (label, commands) = if detected_baud == GPS_BAUD_CANDIDATES[0] {
            ("L76K", L76K_INIT_COMMANDS)
        } else {
            ("u-blox", UBLOX_INIT_COMMANDS)
        };
        for (i, cmd) in commands.iter().enumerate() {
            match uart.write(cmd) {
                Ok(n) if n == cmd.len() => {
                    log::info!("GPS: {} init command {}/{} sent ({} bytes)",
                        label, i + 1, commands.len(), n);
                }
                Ok(n) => {
                    log::warn!(
                        "GPS: {} init command {}/{} short write ({} of {} bytes) — \
                         module may not apply this command",
                        label, i + 1, commands.len(), n, cmd.len(),
                    );
                }
                Err(e) => {
                    log::warn!(
                        "GPS: {} init command {}/{} failed ({:?}) — \
                         proceeding without it; module may stay on defaults",
                        label, i + 1, commands.len(), e,
                    );
                }
            }
            FreeRtos::delay_ms(GPS_INIT_CMD_DELAY_MS);
        }
    }

    /// Self-healing check for the NVS-cache fast path (see [`GpsDriver::new`]):
    /// if `detected_baud` came from the cache WITHOUT a boot-time
    /// verification probe (`baud_confirmed == false`) and no NMEA sentence
    /// (any talker/type — the same liveness signal behind
    /// [`FixState::Acquiring`]) has been observed within
    /// [`CACHED_BAUD_SILENCE_TIMEOUT_MS`] of construction, the cached rate is
    /// presumed stale (module swapped, reconfigured, or the cached value
    /// itself corrupted) and a full [`GPS_BAUD_CANDIDATES`] re-probe kicks off
    /// — see [`begin_reprobe_window`](Self::begin_reprobe_window) /
    /// [`service_reprobe`](Self::service_reprobe) for how that probe now runs
    /// (non-blockingly, spread across `poll()` calls, rather than in a single
    /// multi-second blocking call).
    ///
    /// Runs at MOST ONCE per boot: `baud_confirmed` latches `true`
    /// afterward regardless of outcome, so a genuinely dead/disconnected
    /// module does not repeatedly re-trigger a re-probe every time this check
    /// runs. Real NMEA traffic observed before the timeout also latches
    /// `baud_confirmed` (the cheapest possible confirmation) without ever
    /// running a re-probe.
    fn maybe_reprobe_stale_cached_baud(&mut self, now_ms: u64) {
        if self.baud_confirmed {
            return;
        }
        if self.last_nmea_seen_uptime_ms.is_some() {
            // Real traffic at the cached rate is the cheapest possible proof
            // the cache is still correct.
            self.baud_confirmed = true;
            return;
        }
        if now_ms.saturating_sub(self.driver_start_uptime_ms) < CACHED_BAUD_SILENCE_TIMEOUT_MS {
            return; // still within the grace period
        }
        log::warn!(
            "GPS: baud cache — cached rate {} bps produced no NMEA traffic within {}ms; \
             running a full re-probe (self-healing, non-blocking — spread across poll() calls)",
            self.detected_baud, CACHED_BAUD_SILENCE_TIMEOUT_MS,
        );
        // Any bytes already sitting in the line accumulator were collected
        // at the (about-to-be-stale) old baud and are therefore garbage —
        // discard them so they don't corrupt the first real sentence parsed
        // after the re-probe locks a rate (the re-probe itself samples via
        // `reprobe_buf`, not `line_buf`, so this is purely about not carrying
        // forward pre-re-probe leftovers).
        self.line_len = 0;
        self.begin_reprobe_window(0, now_ms);
    }

    /// Switch the UART to `GPS_BAUD_CANDIDATES[candidate_idx]` and start its
    /// sampling window — the non-blocking equivalent of one iteration of
    /// [`probe_candidates`]'s loop body. If the UART refuses the switch (rare
    /// — a register-level driver error), that candidate is skipped entirely
    /// with NO sampling window spent on it, exactly like `probe_candidates`'s
    /// `continue` on the same error: this recurses forward through
    /// `GPS_BAUD_CANDIDATES` until one switch succeeds, or concludes the
    /// probe via [`conclude_reprobe_no_match`](Self::conclude_reprobe_no_match)
    /// if none do.
    fn begin_reprobe_window(&mut self, candidate_idx: usize, now_ms: u64) {
        if candidate_idx >= GPS_BAUD_CANDIDATES.len() {
            self.conclude_reprobe_no_match();
            return;
        }
        let baud = GPS_BAUD_CANDIDATES[candidate_idx];
        if let Err(e) = self.uart.change_baudrate(Hertz(baud)) {
            log::warn!(
                "GPS: re-probe — failed to switch UART to {} bps ({:?}); skipping",
                baud, e,
            );
            self.begin_reprobe_window(candidate_idx + 1, now_ms);
            return;
        }
        // Drop whatever partial/garbage bytes accumulated at the old baud
        // before sampling at this candidate's rate.
        let _ = self.uart.clear_rx();
        self.reprobe_candidate_idx = Some(candidate_idx);
        self.reprobe_window_start_ms = now_ms;
        self.reprobe_len = 0;
    }

    /// No candidate baud validated (every window sampled negative, or every
    /// remaining switch failed) — the non-blocking equivalent of
    /// [`full_probe_and_persist`]'s `None` branch: fall back to
    /// `GPS_BAUD_CANDIDATES[0]` (unpersisted — a genuinely dead/disconnected
    /// module shouldn't overwrite a previously-good NVS cache entry) and
    /// latch `baud_confirmed` so this driver stops re-triggering re-probes
    /// for the rest of the boot.
    fn conclude_reprobe_no_match(&mut self) {
        log::warn!(
            "GPS: re-probe — no candidate baud ({:?}) yielded a checksum-valid NMEA \
             sentence; falling back to {} bps — FixState will report NoSignal until \
             traffic is observed",
            GPS_BAUD_CANDIDATES, GPS_BAUD_CANDIDATES[0],
        );
        if let Err(e) = self.uart.change_baudrate(Hertz(GPS_BAUD_CANDIDATES[0])) {
            log::warn!(
                "GPS: re-probe — failed to restore fallback baud {} bps ({:?})",
                GPS_BAUD_CANDIDATES[0], e,
            );
        }
        self.detected_baud = GPS_BAUD_CANDIDATES[0];
        self.baud_confirmed = true;
        self.reprobe_candidate_idx = None;
    }

    /// Service an in-progress self-heal re-probe with ONE small, non-blocking
    /// slice of work: drain whatever UART bytes are immediately available
    /// into `reprobe_buf` (mirrors [`drain_uart`](Self::drain_uart)'s
    /// `uart.read(_, 0)` style), and — once the current candidate's
    /// [`BAUD_PROBE_WINDOW_MS`] has elapsed (or `reprobe_buf` has filled) —
    /// check it for a checksum-valid NMEA sentence, advance to the next
    /// candidate, or conclude the probe. No-op if no re-probe is in progress.
    ///
    /// # Why this exists
    ///
    /// This is the non-blocking replacement for what used to be a single call
    /// to [`full_probe_and_persist`] from `maybe_reprobe_stale_cached_baud`.
    /// That call ran [`probe_candidates`], which samples each of
    /// [`GPS_BAUD_CANDIDATES`] for [`BAUD_PROBE_WINDOW_MS`] (1.1 s) via
    /// [`collect_probe_bytes`]'s `FreeRtos::delay_ms` busy-loop — up to ~3.3 s
    /// of the SAME thread that runs `main.rs`'s dispatcher loop, entirely
    /// blocked, once for all three candidates. `poll()` (this driver's only
    /// call site, invoked once per dispatcher-loop iteration, BEFORE
    /// `ui::UiRuntime::step()` in that same iteration) would not return for
    /// that whole span — so `step()` — the only call site of
    /// `slint::platform::update_timers_and_animations()` /
    /// `render_if_needed()`, per its own module doc — could not run either.
    ///
    /// The boot splash's one-shot intro animation (`screens::splash` module
    /// doc) already anchors its start to the wall-clock instant of the FIRST
    /// `step()` call specifically so bring-up time occurring BEFORE that call
    /// (NVS/radio/GPS-construction/history-store init, all of which run
    /// before the dispatcher loop even starts) cannot eat into the animation.
    /// But [`CACHED_BAUD_SILENCE_TIMEOUT_MS`] (5 s) is measured from GPS
    /// driver CONSTRUCTION, which happens seconds before that first `step()`
    /// call — so the self-heal re-probe's trigger instant frequently lands
    /// AFTER the dispatcher loop (and the splash animation) has already
    /// started, inside the loop's steady-state `poll()` calls. Whether that
    /// happens (and how much of the splash's ~1.15 s animation / up-to-2.4 s
    /// on-screen window it overlaps) depends on how long the rest of boot
    /// took and whether the GPS module has emitted any NMEA yet — both of
    /// which vary boot-to-boot (cold-start acquisition time, indoor/outdoor,
    /// unit-to-unit timing) — which is exactly the reported symptom: the
    /// splash sometimes freezes for seconds mid-animation and sometimes plays
    /// smoothly, depending on timing this driver has no control over.
    ///
    /// Spreading the SAME probe algorithm (same candidates, same
    /// checksum-valid-NMEA acceptance test, same per-candidate window length)
    /// across many `poll()` calls — one non-blocking slice of work per call,
    /// no `FreeRtos::delay_ms` at all — means no single `poll()` call (and so
    /// no single dispatcher-loop iteration, and so no single gap between two
    /// `ui::UiRuntime::step()` calls) can ever again take open-ended
    /// multi-second real time, regardless of when the probe fires relative to
    /// the splash window.
    fn service_reprobe(&mut self, now_ms: u64) {
        let Some(candidate_idx) = self.reprobe_candidate_idx else {
            return;
        };

        // Non-blockingly drain whatever bytes are ready right now — same
        // style as `drain_uart`'s `uart.read(_, 0)`.
        let mut byte = [0u8; 1];
        while self.reprobe_len < self.reprobe_buf.len() {
            match self.uart.read(&mut byte, 0) {
                Ok(1) => {
                    self.reprobe_buf[self.reprobe_len] = byte[0];
                    self.reprobe_len += 1;
                }
                _ => break,
            }
        }

        let window_elapsed = now_ms.saturating_sub(self.reprobe_window_start_ms);
        if (window_elapsed as u32) < BAUD_PROBE_WINDOW_MS && self.reprobe_len < self.reprobe_buf.len() {
            return; // still sampling this candidate's window
        }

        let candidate = GPS_BAUD_CANDIDATES[candidate_idx];
        if contains_checksum_valid_nmea(&self.reprobe_buf[..self.reprobe_len]) {
            log::info!(
                "GPS: re-probe — {} bps yields a checksum-valid NMEA sentence, locking",
                candidate,
            );
            gps_baud_store::save_cached_baud(self.nvs_partition.clone(), candidate);
            if candidate != self.detected_baud {
                log::info!(
                    "GPS: baud cache — re-probe locked a different rate ({} bps -> {} bps); \
                     re-sending module init",
                    self.detected_baud, candidate,
                );
                self.detected_baud = candidate;
                Self::send_init_commands(&self.uart, candidate);
            } else {
                log::info!(
                    "GPS: baud cache — re-probe confirms {} bps is still correct (module may \
                     simply be indoors/cold-starting; NoSignal is expected until it acquires)",
                    candidate,
                );
            }
            self.baud_confirmed = true;
            self.reprobe_candidate_idx = None;
            return;
        }

        log::info!(
            "GPS: re-probe — {} bps: {} bytes captured, no checksum-valid NMEA sentence found",
            candidate, self.reprobe_len,
        );
        // `begin_reprobe_window` itself concludes via
        // `conclude_reprobe_no_match` once `candidate_idx` runs past the end
        // of `GPS_BAUD_CANDIDATES` — same helper, one call site either way.
        self.begin_reprobe_window(candidate_idx + 1, now_ms);
    }

    /// Poll the GPS — drain UART bytes if in the active window; advance the
    /// duty-cycle state machine.
    ///
    /// Called on every dispatcher loop iteration.  Reads as many bytes as are
    /// immediately available (non-blocking), accumulates them into the line
    /// buffer, and parses complete GGA lines.  A parsed valid fix updates
    /// [`cached_fix`](Self::cached_fix).
    ///
    /// # State transitions
    ///
    /// - ACTIVE → QUIET when `now_ms - period_start_ms ≥ GPS_ACTIVE_WINDOW_MS`
    ///   **AND** a fix has already been captured (see [`should_close_active_window`]
    ///   — before the first fix, the window never closes on its own).
    /// - QUIET → ACTIVE when `now_ms - period_start_ms ≥ GPS_QUIET_INTERVAL_MS`.
    ///
    /// Also runs the NVS-baud-cache self-heal check (see
    /// [`maybe_reprobe_stale_cached_baud`](Self::maybe_reprobe_stale_cached_baud))
    /// on every call; a no-op after the first boot or two once the cache is
    /// confirmed or a one-time re-probe has already run. If a re-probe IS in
    /// progress (own or freshly started this call), this call instead
    /// services (or kicks off) one non-blocking slice of it — see
    /// [`service_reprobe`](Self::service_reprobe)'s doc for why — and defers
    /// the normal drain/duty-cycle handling below to a later call: the UART
    /// is parked at a candidate rate under test, not `detected_baud`, so
    /// normal NMEA parsing would just see garbage until the probe concludes.
    pub fn poll(&mut self, now_ms: u64) {
        if self.reprobe_candidate_idx.is_some() {
            self.service_reprobe(now_ms);
            return;
        }
        self.maybe_reprobe_stale_cached_baud(now_ms);
        if self.reprobe_candidate_idx.is_some() {
            // Just switched the UART to the first candidate's rate this
            // tick — defer normal handling to the next poll() call.
            return;
        }
        if self.active {
            // Drain available UART bytes non-blockingly.
            self.drain_uart(now_ms);

            let elapsed = now_ms.saturating_sub(self.period_start_ms);
            if should_close_active_window(self.cached_fix.is_some(), elapsed) {
                self.active = false;
                self.period_start_ms = now_ms;
                log::debug!("GPS: active window closed — entering quiet interval");
            }
        } else {
            // QUIET: check whether it is time to reopen the active window.
            let elapsed = now_ms.saturating_sub(self.period_start_ms);
            if should_reopen_active_window(elapsed) {
                self.active = true;
                self.period_start_ms = now_ms;
                log::debug!("GPS: quiet interval done — opening active window");
            }
        }
    }

    /// Return the cached fix and its age in seconds, or `None` if no fix has
    /// ever been obtained.
    ///
    /// `age_secs` is `(now_ms - fix.captured_uptime_ms) / 1000` — the number
    /// of whole seconds since the fix was last refreshed.  This value is
    /// forwarded to [`encode_telemetry_response`](protocol::encode_telemetry_response)
    /// and sent to the requesting contact as the `age=Zs` field.
    pub fn get_fix_and_age(&self, now_ms: u64) -> Option<(i32, i32, u32)> {
        let fix = self.cached_fix?;
        let age_secs = (now_ms.saturating_sub(fix.captured_uptime_ms) / 1000) as u32;
        Some((fix.lat_e7, fix.lon_e7, age_secs))
    }

    /// Return a read-only status snapshot: three-state fix status, coordinates
    /// + age, satellite count, and clock-sync state + age.  Used by the
    /// admin-menu GPS status screen; the has-fix/coords/age/clock-sync subset
    /// is mirrored into the host `status` command output.
    pub fn status(&self, now_ms: u64) -> GpsStatus {
        let (has_fix, lat_e7, lon_e7, fix_age_secs) = match self.get_fix_and_age(now_ms) {
            Some((lat, lon, age)) => (true, lat, lon, age),
            None => (false, 0, 0, 0),
        };
        let fix_state = if has_fix {
            FixState::Fix
        } else if self.last_nmea_seen_uptime_ms.is_some() {
            FixState::Acquiring
        } else {
            FixState::NoSignal
        };
        let (clock_synced, clock_sync_age_secs) = match self.last_clock_sync_uptime_ms {
            Some(sync_ms) => (true, (now_ms.saturating_sub(sync_ms) / 1000) as u32),
            None => (false, 0),
        };
        GpsStatus {
            has_fix,
            fix_state,
            lat_e7,
            lon_e7,
            fix_age_secs,
            sat_count: self.last_sat_count,
            clock_synced,
            clock_sync_age_secs,
        }
    }

    // ── Private: UART drain + NMEA accumulation ───────────────────────────────

    /// Non-blockingly drain all available UART bytes into the line buffer,
    /// flushing complete lines to [`parse_line`](Self::parse_line).
    ///
    /// # Diagnostics (`--features diagnostics` only)
    ///
    /// Every raw byte read from UART1 — *before* any CR/LF framing or NMEA
    /// parsing is applied — is counted and echoed to the log, deliberately
    /// upstream of `parse_line`'s `$`-prefix liveness check. This makes the
    /// diagnostic build able to prove or disprove "any bytes at all arrive
    /// on the wire" independent of whether this loop's own framing/parsing
    /// logic is correct — an earlier field investigation found:
    /// TX to the L76K was confirmed
    /// working (byte-accurate write counts logged by `GpsDriver::new`) while
    /// RX stayed silent, and the two remaining hypotheses — zero bytes on
    /// the wire (module/HW fault) vs. bytes arriving but dropped by this
    /// loop (firmware bug) — are indistinguishable from the NMEA-level
    /// `first-NMEA-seen` log alone, since that log never fires in *either*
    /// case. A heartbeat line is emitted every 5 s regardless of traffic so
    /// a desk capture can tell "zero bytes, diagnostic confirmed running"
    /// apart from a build that simply isn't logging.
    fn drain_uart(&mut self, now_ms: u64) {
        let mut byte = [0u8; 1];
        #[cfg(feature = "diagnostics")]
        let mut raw_tail = [0u8; 16];
        #[cfg(feature = "diagnostics")]
        let mut raw_tail_len = 0usize;
        #[cfg(feature = "diagnostics")]
        let mut n_this_call: u32 = 0;

        // Read up to 256 bytes per poll call to bound latency on the main loop.
        for _ in 0..256usize {
            let n = self.uart.read(&mut byte, 0).unwrap_or(0);
            if n == 0 {
                break; // no more bytes available right now
            }
            let b = byte[0];

            #[cfg(feature = "diagnostics")]
            {
                self.rx_byte_count = self.rx_byte_count.wrapping_add(1);
                n_this_call += 1;
                if raw_tail_len < raw_tail.len() {
                    raw_tail[raw_tail_len] = b;
                    raw_tail_len += 1;
                } else {
                    // Keep only the most recent 16 bytes of this call.
                    raw_tail.copy_within(1.., 0);
                    raw_tail[raw_tail.len() - 1] = b;
                }
            }

            if b == b'\n' {
                // End of NMEA sentence — try to parse the accumulated line.
                let line_len = self.line_len;
                self.parse_line(now_ms, line_len);
                self.line_len = 0;
            } else if b != b'\r' {
                // Accumulate (skip CR; $ is included so the line starts with '$')
                if self.line_len < self.line_buf.len() {
                    self.line_buf[self.line_len] = b;
                    self.line_len += 1;
                } else {
                    // Overlong line — discard and reset (malformed sentence)
                    self.line_len = 0;
                }
            }
        }

        #[cfg(feature = "diagnostics")]
        {
            if n_this_call > 0 {
                let (hex, hex_len) = hex_dump_tail(&raw_tail[..raw_tail_len]);
                let hex_str = core::str::from_utf8(&hex[..hex_len]).unwrap_or("?");
                log::info!(
                    "GPS: raw RX n={} total={} tail=[{}]",
                    n_this_call, self.rx_byte_count, hex_str,
                );
            }
            // Heartbeat: proves the diagnostic is alive even at zero bytes —
            // a capture with NO "raw RX" lines and NO heartbeat lines means
            // the diagnostic build isn't running (or the active window never
            // opened), not that the module is silent.
            if now_ms.saturating_sub(self.last_rx_heartbeat_ms) >= 5_000 {
                log::info!(
                    "GPS: raw RX heartbeat — total={} bytes since boot",
                    self.rx_byte_count,
                );
                self.last_rx_heartbeat_ms = now_ms;
            }
        }
    }

    /// Attempt to parse the accumulated NMEA line as a GGA (position) or RMC
    /// (date+time) sentence, updating the cached fix and/or syncing the
    /// system clock as appropriate.  A line is at most one sentence type, so
    /// both parsers are tried (each rejects sentences it does not own).
    ///
    /// ANY line starting with `$` (regardless of sentence type or validity)
    /// updates [`last_nmea_seen_uptime_ms`](Self::last_nmea_seen_uptime_ms) —
    /// this is the receiver-liveness signal behind [`FixState::Acquiring`],
    /// deliberately independent of whether the sentence carried a valid fix.
    fn parse_line(&mut self, now_ms: u64, line_len: usize) {
        let line = &self.line_buf[..line_len];
        if line.first() == Some(&b'$') {
            // Log only the NoSignal -> Acquiring transition (once), not every
            // sentence: a field-diagnosis breadcrumb ("the module IS talking,
            // it just hasn't got a fix") without spamming the log at 1 Hz.
            if self.last_nmea_seen_uptime_ms.is_none() {
                log::info!(
                    "GPS: first NMEA sentence observed at t={}ms — receiver alive, acquiring",
                    now_ms,
                );
            }
            self.last_nmea_seen_uptime_ms = Some(now_ms);
        }
        if let Some(gga) = parse_gga(line) {
            // Sat count is meaningful even without a fix — captures it while
            // still `Acquiring` so the UI can show progress.
            self.last_sat_count = gga.sat_count;
            if let Some((lat_e7, lon_e7)) = gga.coords {
                log::info!(
                    "GPS fix: lat={}.{}e-7, lon={}.{}e-7",
                    lat_e7 / 10_000_000, (lat_e7 % 10_000_000).abs(),
                    lon_e7 / 10_000_000, (lon_e7 % 10_000_000).abs(),
                );
                self.cached_fix = Some(GpsFix { lat_e7, lon_e7, captured_uptime_ms: now_ms });
            }
        }
        if let Some(dt) = parse_rmc_datetime(line) {
            if set_system_clock_from_utc(dt) {
                log::info!(
                    "GPS clock sync: {:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                    dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second,
                );
                self.last_clock_sync_uptime_ms = Some(now_ms);
            }
        }
    }
}

// `should_close_active_window`/`should_reopen_active_window` (duty-cycle
// transition predicates) and `NmeaDateTime` now live in `firmware_core::gps`
// (see the glob re-export above) — `poll` above and `set_system_clock_from_utc`
// below reference them unqualified via that glob.

/// Set the system clock (`settimeofday`) from a decoded UTC date+time.
/// Returns `true` on success.
///
/// Not `cfg`-gated to a host-native fallback: this module already imports
/// `esp_idf_hal::uart::UartDriver` unconditionally at the top (see the module
/// doc), so — like the rest of `gps.rs` outside the pure parsing/math
/// functions — it only ever builds for the `xtensa-esp32s3-espidf` target
/// pinned in `firmware/.cargo/config.toml`; a host-native fallback branch
/// would be permanently dead code, never compiled or run by any real build.
fn set_system_clock_from_utc(dt: NmeaDateTime) -> bool {
    let unix_secs = unix_timestamp(
        dt.year as i64, dt.month as u32, dt.day as u32,
        dt.hour as u32, dt.minute as u32, dt.second as u32,
    );
    let tv = esp_idf_svc::sys::timeval { tv_sec: unix_secs as _, tv_usec: 0 };
    // SAFETY: `tv` is a valid, fully-initialised `timeval`; the timezone
    // argument is null (UTC, no DST offset applied by libc).
    let ret = unsafe { esp_idf_svc::sys::settimeofday(&tv, core::ptr::null()) };
    if ret != 0 {
        log::warn!("GPS clock sync: settimeofday failed (errno path, ret={})", ret);
        return false;
    }
    true
}

// `unix_timestamp`, `parse_rmc_datetime`, the NMEA GGA parser
// (`parse_gga`/`GgaFields`/`nmea_coord_to_e7`), the `--features diagnostics`
// `hex_dump_tail` helper, and `parse_u32_bytes` now live in
// `firmware_core::gps` (see the glob re-export above) — `parse_line` and
// `drain_uart` above reference them unqualified via that glob. Their
// `#[cfg(test)]` coverage moved with them (this crate is a detached,
// cross-compiled workspace — see `Cargo.toml`'s doc comment — so those tests
// now EXECUTE under `cargo test --workspace`, where they previously only
// type-checked). See `docs/adr/0005-firmware-core-extraction.md`.
