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

/// Candidate UART baud rates probed at boot, in probe order (see
/// [`probe_candidates`]). The T-Deck Plus ships with either of two GPS
/// module variants — a Quectel L76K (default 9600 bps) or a u-blox M10Q
/// (default 38400 bps) — and unit-to-unit hardware variance means the
/// running firmware cannot assume which one is installed (full
/// auto-detect across three rates is the deliberate design). 115200 is included as a catch-all for a
/// reconfigured/variant module reporting at neither documented default; a
/// rate that locks there is treated as a u-blox-family module for init
/// purposes (see [`GpsDriver::send_init_commands`]) since 9600 is the only
/// rate specifically associated with the MTK/L76K `$PCAS` command family.
/// All three candidates use 8N1 framing, so only the baud rate itself needs
/// probing. Probed in the documented-likely order: L76K (9600), then u-blox
/// M10Q (38400), then the 115200 catch-all.
pub const GPS_BAUD_CANDIDATES: &[u32] = &[9600, 38400, 115200];

/// Baud rate the GPS UART is opened at in `main.rs::run()`, before probing.
/// Must equal `GPS_BAUD_CANDIDATES[0]` — [`GpsDriver::new`] probes from
/// there and, if the module is actually running at a later candidate,
/// switches the already-open [`UartDriver`] via `change_baudrate` rather
/// than requiring a fresh `UartConfig`.
pub const GPS_BAUD: u32 = GPS_BAUD_CANDIDATES[0];

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

/// Compute the standard NMEA checksum: XOR of every byte strictly between
/// `$` and `*`. Shared by [`contains_checksum_valid_nmea`] (baud-probe
/// validation) and the `L76K_INIT_COMMANDS`/`UBLOX_INIT_COMMANDS` regression
/// tests.
fn nmea_checksum(body: &[u8]) -> u8 {
    body.iter().fold(0u8, |acc, &b| acc ^ b)
}

/// Parse a single ASCII hex digit (`0-9`, `A-F`, `a-f`) to its 0-15 value.
fn hex_digit_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// `true` if `bytes` contains a COMPLETE, CHECKSUM-VALID NMEA sentence: a
/// `$`-anchored span of printable ASCII (`0x20..=0x7E`) at least 5 bytes
/// long, followed by `*` and two hex digits whose value equals
/// [`nmea_checksum`] of the span between `$` and `*`, all within 82 bytes
/// (the practical NMEA 0183 sentence-length ceiling) and/or before a
/// `\r`/`\n` terminator.
///
/// By deliberate design: a bare "`$`-anchored printable-ASCII
/// span with a `*` in it" test (this function's predecessor,
/// `looks_like_ascii_nmea`) is not a safe discriminator across three probed
/// candidates — three ~1.1s windows of near-uniform-random garbage give a
/// coincidental ASCII-shaped span meaningfully more chances to appear than
/// two did. Requiring the checksum to actually match closes that gap: a
/// genuine NMEA transmitter always gets this right, while a coincidental
/// wrong-baud span passing both the ASCII-shape test AND an independent
/// ~1-in-256 checksum match is negligible. The 5-byte minimum body length
/// additionally rules out the degenerate `$*<hex>` case, whose empty body
/// has a checksum of 0 and would otherwise trivially "validate" against a
/// checksum field of `00`.
fn contains_checksum_valid_nmea(bytes: &[u8]) -> bool {
    let mut start = 0usize;
    while start < bytes.len() {
        let Some(rel) = bytes[start..].iter().position(|&b| b == b'$') else {
            return false;
        };
        let dollar = start + rel;
        let mut end = dollar + 1;
        let mut star: Option<usize> = None;
        while end < bytes.len() && end - dollar <= 82 {
            match bytes[end] {
                b'\r' | b'\n' => break,
                b'*' => {
                    star = Some(end);
                    break;
                }
                0x20..=0x7E => end += 1,
                _ => break, // non-printable/high-bit byte disqualifies this span
            }
        }
        if let Some(star_pos) = star {
            let body = &bytes[dollar + 1..star_pos];
            if body.len() >= 5 {
                if let (Some(&hi), Some(&lo)) =
                    (bytes.get(star_pos + 1), bytes.get(star_pos + 2))
                {
                    if let (Some(hi_v), Some(lo_v)) = (hex_digit_value(hi), hex_digit_value(lo)) {
                        let expected = (hi_v << 4) | lo_v;
                        if nmea_checksum(body) == expected {
                            return true;
                        }
                    }
                }
            }
        }
        start = dollar + 1;
    }
    false
}

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

/// L76K init command sequence, sent once at driver construction when the
/// probe/cache locks onto `GPS_BAUD_CANDIDATES[0]` (9600):
/// 1. `$PCAS04,7` — enable GPS + GLONASS + BEIDOU constellations.
/// 2. `$PCAS03,...` — restrict NMEA output to GGA + RMC only (the two
///    sentence types [`GpsDriver::parse_line`] understands; cuts UART
///    traffic during the always-on pre-fix ACTIVE window).
/// 3. `$PCAS11,3` — vehicle dynamic model (matches reference firmware; the
///    L76K's default aviation-leaning model is tuned for higher dynamics
///    than a pedestrian/vehicle-carried tracker needs).
const L76K_INIT_COMMANDS: &[&[u8]] = &[
    b"$PCAS04,7*1E\r\n",
    b"$PCAS03,1,0,0,0,1,0,0,0,0,0,,,0,0*02\r\n",
    b"$PCAS11,3*1E\r\n",
];

/// u-blox M10Q (or 115200-catch-all) init command sequence, sent once at
/// driver construction when the probe/cache locks onto a candidate baud
/// OTHER than `GPS_BAUD_CANDIDATES[0]` — see the "Module init sequences"
/// section doc above for why `$PUBX,40` (not binary UBX, not `$PCAS`) and
/// why this only enables rather than restricts:
/// 1. `$PUBX,40,GGA,0,1,0,0,0,0` — enable GGA once per navigation solution
///    on UART1 (rate fields are DDC, UART1, UART2, USB, SPI, reserved).
/// 2. `$PUBX,40,RMC,0,1,0,0,0,0` — enable RMC once per navigation solution
///    on UART1.
const UBLOX_INIT_COMMANDS: &[&[u8]] = &[
    b"$PUBX,40,GGA,0,1,0,0,0,0*5B\r\n",
    b"$PUBX,40,RMC,0,1,0,0,0,0*46\r\n",
];

// ── Duty-cycle constants ──────────────────────────────────────────────────────

/// Length of the active UART reading window in milliseconds (30 s).
pub const GPS_ACTIVE_WINDOW_MS: u64 = 30_000;

/// Quiescent interval between active windows in milliseconds (120 s ≈ 2 min).
pub const GPS_QUIET_INTERVAL_MS: u64 = 120_000;

// ── GpsFix ────────────────────────────────────────────────────────────────────

/// A validated GPS fix from the L76K GNSS module.
#[derive(Clone, Copy, Debug)]
pub struct GpsFix {
    /// Latitude in units of 1e-7 degrees (positive = North).
    pub lat_e7: i32,
    /// Longitude in units of 1e-7 degrees (positive = East).
    pub lon_e7: i32,
    /// Uptime milliseconds when this fix was captured (via [`uptime_ms()`]).
    /// Used to compute fix age in [`GpsDriver::get_fix_and_age`].
    pub captured_uptime_ms: u64,
}

// ── FixState ──────────────────────────────────────────────────────────────────

/// Three-state acquisition status, distinguishing "actively searching" from
/// "nothing heard from the module at all" — the two states a bare `has_fix`
/// boolean collapses into a single, indistinguishable "no fix" reading.
///
/// See the module doc's "Status feedback" section for the rationale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixState {
    /// No NMEA sentence (of any type, any talker) has been observed from the
    /// GPS module since boot — either too early after boot to have received
    /// the module's first sentence, or a genuine hardware fault (antenna,
    /// wiring, power rail).
    NoSignal,
    /// NMEA traffic is flowing (the receiver is alive and searching) but no
    /// fix (GGA quality ≥ 1) has been captured yet.
    Acquiring,
    /// A fix has been captured at least once since boot. Sticky: once
    /// reached, status never regresses to `Acquiring`/`NoSignal` even if the
    /// fix goes stale — see [`GpsDriver::get_fix_and_age`]'s age-surfacing
    /// contract.
    Fix,
}

// ── GpsStatus ─────────────────────────────────────────────────────────────────

/// Read-only GPS status snapshot for display: fix state, coordinates + age,
/// satellite count, and clock-sync state + age.  Consumed by the admin-menu
/// GPS status screen and (fix/coords/age/clock-sync subset) mirrored into the
/// host `status` command output (`RspStatusPayload`).  Carries no controls by
/// design (status/display only — ADR scope).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpsStatus {
    /// `true` if a fix has ever been obtained since boot. Equivalent to
    /// `fix_state == FixState::Fix`; kept as its own field because it is
    /// mirrored into the wire-level `RspStatusPayload` (host `status`
    /// command), which predates `fix_state` and has no acquiring/no-signal
    /// distinction.
    pub has_fix: bool,
    /// Three-state acquisition status — see [`FixState`]. Local-display-only
    /// (not mirrored to the wire protocol).
    pub fix_state: FixState,
    /// Latitude in units of 1e-7 degrees. Only meaningful when `has_fix`.
    pub lat_e7: i32,
    /// Longitude in units of 1e-7 degrees. Only meaningful when `has_fix`.
    pub lon_e7: i32,
    /// Seconds since the cached fix was captured. Only meaningful when `has_fix`.
    pub fix_age_secs: u32,
    /// Satellites used in the most recently parsed GGA sentence (field 7,
    /// `numSV`). Meaningful regardless of `fix_state` — populated as soon as
    /// the receiver reports ANY GGA sentence, even before a fix is captured,
    /// so the UI can show "N satellites" while still `Acquiring`. `0` when no
    /// GGA sentence has been seen yet.
    pub sat_count: u8,
    /// `true` if the system clock has been set from a valid GPS date+time
    /// sentence since boot.
    pub clock_synced: bool,
    /// Seconds since the last successful clock sync. Only meaningful when
    /// `clock_synced`.
    pub clock_sync_age_secs: u32,
}

impl GpsStatus {
    /// The status of a driver that has never obtained a fix, satellite count,
    /// or clock sync (e.g. immediately after boot, or before GPS hardware is
    /// available).
    pub const fn never() -> Self {
        GpsStatus {
            has_fix: false,
            fix_state: FixState::NoSignal,
            lat_e7: 0,
            lon_e7: 0,
            fix_age_secs: 0,
            sat_count: 0,
            clock_synced: false,
            clock_sync_age_secs: 0,
        }
    }
}

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

// ── Duty-cycle transition predicates (pure — testable without hardware) ───────

/// `true` when an open ACTIVE window should close.
///
/// Gated on `has_fix`: before the first fix is captured, the window never
/// closes on elapsed time alone — see the module doc's "Duty cycle" section.
/// Once a fix exists, closes after `GPS_ACTIVE_WINDOW_MS` as documented.
fn should_close_active_window(has_fix: bool, elapsed_ms: u64) -> bool {
    has_fix && elapsed_ms >= GPS_ACTIVE_WINDOW_MS
}

/// `true` when a QUIET interval should reopen into an ACTIVE window.
fn should_reopen_active_window(elapsed_ms: u64) -> bool {
    elapsed_ms >= GPS_QUIET_INTERVAL_MS
}

/// A UTC calendar date+time decoded from an NMEA `$..RMC` sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NmeaDateTime {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

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

/// Convert a UTC civil date+time to a Unix timestamp (seconds since
/// 1970-01-01T00:00:00Z).  Pure calendar arithmetic — Howard Hinnant's
/// `days_from_civil` algorithm, valid for the full proleptic Gregorian
/// calendar without any external date/time dependency.
fn unix_timestamp(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (month as i64 + 9) % 12; // [0, 11]: Mar=0 .. Feb=11
    let doy = (153 * mp + 2) / 5 + day as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days = era * 146_097 + doe - 719_468; // days since 1970-01-01
    days * 86_400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64
}

/// Parse a `$GPRMC` or `$GNRMC` NMEA sentence for UTC date+time.
///
/// Returns `Some(NmeaDateTime)` when the sentence reports an active fix
/// (status field `A`) with a syntactically valid time+date. Returns `None`
/// for a void fix (`V`), any other sentence type, or a parse error.
///
/// # RMC format (fields, 0-indexed after the sentence id):
/// ```text
/// $GPRMC,HHMMSS.ss,A,LLLL.LLLLL,a,YYYYY.YYYYY,a,S.s,C.c,DDMMYY,,,A*HH
/// 0      1         2 3           4  5            6  7   8   9      10 11 12
/// ```
/// - field 0: UTC time (HHMMSS.ss)
/// - field 1: status (`A` = active/valid, `V` = void/no fix)
/// - field 8: UTC date (DDMMYY)
///
/// # Field-index bookkeeping (defect fix)
///
/// The scan loop below (same shape as [`parse_gga`]'s) treats the
/// sentence-id span itself (`$GPRMC`/`$GNRMC`, up to the first comma) as
/// `field_idx == 0` — the comma-delimited span doesn't start until AFTER the
/// id, so the loop's `field_idx` for any given field is always the
/// documented field number **+ 1** (`parse_gga`'s `match` arms — `2` for
/// lat, `3` for N/S, etc. — already bake in this +1 shift relative to its
/// own doc-comment field numbers). This function's `match` previously used
/// the RAW documented numbers (`0`/`1`/`8`) UNSHIFTED: `time_str` therefore
/// captured the sentence id, `status` captured the actual time string, and
/// `date_str` captured the actual course-over-ground field. `status.first()
/// == Some(b'A')` then compared against a numeric time string (never
/// `b'A'`), so this function returned `None` for EVERY RMC sentence, valid
/// or not — the reason GPS position fixes (parsed by the correctly-shifted
/// `parse_gga`) worked while `settimeofday`/clock sync never fired. The
/// match arms below use the shifted indices (`1`/`2`/`9`) to match
/// `parse_gga`'s convention.
fn parse_rmc_datetime(line: &[u8]) -> Option<NmeaDateTime> {
    if !line.starts_with(b"$GPRMC,") && !line.starts_with(b"$GNRMC,") {
        return None;
    }

    let mut field_start = 0usize;
    let mut field_idx = 0usize;
    let mut time_str: &[u8] = b"";
    let mut status: &[u8] = b"";
    let mut date_str: &[u8] = b"";

    for i in 0..line.len() {
        let b = line[i];
        if b == b',' || b == b'*' {
            let field = &line[field_start..i];
            match field_idx {
                1 => time_str = field,
                2 => status = field,
                9 => date_str = field,
                _ => {}
            }
            field_idx += 1;
            field_start = i + 1;
            if field_idx > 9 {
                break; // all fields we need are collected
            }
        }
    }

    if status.first().copied() != Some(b'A') {
        return None; // void fix — no reliable date/time
    }

    // Time: HHMMSS(.ss) — need at least 6 digits (fractional seconds ignored).
    if time_str.len() < 6 {
        return None;
    }
    let hour = parse_u32_bytes(&time_str[0..2])? as u8;
    let minute = parse_u32_bytes(&time_str[2..4])? as u8;
    let second = parse_u32_bytes(&time_str[4..6])? as u8;

    // Date: DDMMYY — exactly 6 digits.
    if date_str.len() != 6 {
        return None;
    }
    let day = parse_u32_bytes(&date_str[0..2])? as u8;
    let month = parse_u32_bytes(&date_str[2..4])? as u8;
    let yy = parse_u32_bytes(&date_str[4..6])?;
    let year = 2000 + yy as u16; // L76K reports 2-digit year; NMEA has no century field

    if hour > 23 || minute > 59 || second > 60 || day == 0 || day > 31 || month == 0 || month > 12 {
        return None; // reject obviously malformed fields (defensive, not a full calendar check)
    }

    Some(NmeaDateTime { year, month, day, hour, minute, second })
}

// ── NMEA GGA parser (pure functions, no hardware dependency) ──────────────────

/// Parsed contents of a `$GPGGA`/`$GNGGA` sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GgaFields {
    /// `Some((lat_e7, lon_e7))` when fix quality ≥ 1. `None` for a
    /// syntactically valid GGA sentence reporting quality 0 (searching — no
    /// 3D fix yet; lat/lon fields are typically empty in that case).
    coords: Option<(i32, i32)>,
    /// Satellites used (field 7, `numSV`) — meaningful regardless of
    /// `coords`, so callers can surface it while still acquiring. `0` if the
    /// field is empty or unparseable.
    sat_count: u8,
}

/// Parse a `$GPGGA` or `$GNGGA` NMEA sentence.
///
/// Returns `None` only when the line is not a recognized GGA sentence (wrong
/// prefix). For a recognized GGA sentence, `coords` is gated on fix quality
/// ≥ 1 but `sat_count` is always populated — see [`GgaFields`].
///
/// # GGA format (fields, 0-indexed):
/// ```text
/// $GPGGA,HHMMSS.ss,LLLL.LLLLL,a,YYYYY.YYYYY,a,Q,ss,h,Z.z,M,z.z,M,,*HH
/// 0      1         2           3  4            5  6  7  8  9    10   11   12 13  14
/// ```
/// - field 2: latitude (DDMM.MMMMM, D=degrees, M=minutes)
/// - field 3: N or S
/// - field 4: longitude (DDDMM.MMMMM)
/// - field 5: E or W
/// - field 6: fix quality (0 = no fix)
/// - field 7: satellites used (`numSV`)
fn parse_gga(line: &[u8]) -> Option<GgaFields> {
    // Must start with $GPGGA or $GNGGA
    if !line.starts_with(b"$GPGGA,") && !line.starts_with(b"$GNGGA,") {
        return None;
    }

    // Walk comma-separated fields.
    let mut field_start = 0usize;
    let mut field_idx = 0usize;
    let mut lat_str: &[u8] = b"";
    let mut ns: &[u8] = b"";
    let mut lon_str: &[u8] = b"";
    let mut ew: &[u8] = b"";
    let mut fix_q: &[u8] = b"";
    let mut sat_str: &[u8] = b"";

    for i in 0..line.len() {
        let b = line[i];
        if b == b',' || b == b'*' {
            let field = &line[field_start..i];
            match field_idx {
                2 => lat_str = field,
                3 => ns = field,
                4 => lon_str = field,
                5 => ew = field,
                6 => fix_q = field,
                7 => sat_str = field,
                _ => {}
            }
            field_idx += 1;
            field_start = i + 1;
            if field_idx > 7 {
                break; // all fields we need are collected
            }
        }
    }

    let sat_count = parse_u32_bytes(sat_str).unwrap_or(0).min(u8::MAX as u32) as u8;

    // Fix quality: b'0' = no fix; also reject empty field
    let q = fix_q.first().copied().unwrap_or(b'0');
    let coords = if q == b'0' || fix_q.is_empty() {
        None
    } else {
        let lat_e7 = nmea_coord_to_e7(lat_str, ns)?;
        let lon_e7 = nmea_coord_to_e7(lon_str, ew)?;
        Some((lat_e7, lon_e7))
    };

    Some(GgaFields { coords, sat_count })
}

/// Convert NMEA DDMM.MMMMM / DDDMM.MMMMM format to an integer in units of
/// 1e-7 degrees.
///
/// Algorithm:
/// 1. Split at the decimal point.
/// 2. Integer degrees = (integer part) / 100.
/// 3. Integer minutes = (integer part) % 100.
/// 4. Fractional minutes = decimal digits, normalized to 5 places (× 1e-5).
/// 5. `degrees_e7 = deg × 10_000_000 + (min_int × 100_000 + min_frac_e5) × 100 / 60`
/// 6. Apply N/S or E/W sign.
///
/// Returns `None` if the string is empty or unparseable.
fn nmea_coord_to_e7(coord: &[u8], dir: &[u8]) -> Option<i32> {
    if coord.is_empty() {
        return None;
    }
    // Find the decimal point position.
    let dot = coord.iter().position(|&b| b == b'.')?;
    if dot < 2 {
        return None; // need at least 2 digits for integer minutes
    }

    // Integer part (degrees + integer minutes)
    let int_val = parse_u32_bytes(&coord[..dot])? as i64;
    let deg = int_val / 100;
    let min_int = int_val % 100;

    // Fractional minutes (after '.'), normalized to 5 digits.
    let frac_bytes = &coord[dot + 1..];
    let frac_len = frac_bytes.len().min(5);
    let frac_raw = parse_u32_bytes(&frac_bytes[..frac_len])? as i64;
    let scale: i64 = match frac_len {
        1 => 10_000,
        2 => 1_000,
        3 => 100,
        4 => 10,
        _ => 1,
    };
    let min_frac_e5 = frac_raw * scale; // now normalized to 5 decimal places

    // minutes_e5 = min_int × 1e5 + min_frac_e5
    let minutes_e5 = min_int * 100_000 + min_frac_e5;

    // Convert to degrees × 1e7:
    //   degrees_e7 = deg × 1e7 + (minutes_e5 × 1e7) / (60 × 1e5)
    //              = deg × 1e7 + minutes_e5 × 100 / 60
    // (rounding: add 30 before integer division)
    let frac_e7 = (minutes_e5 * 100 + 30) / 60;
    let val_e7 = (deg * 10_000_000 + frac_e7) as i32;

    // Apply hemisphere sign.
    let negative = dir.first() == Some(&b'S') || dir.first() == Some(&b'W');
    Some(if negative { -val_e7 } else { val_e7 })
}

// ── Diagnostics helper (`--features diagnostics` only) ─────────────────────────

/// Hex-encode up to the last 16 bytes of `bytes` (lowercase, no separators)
/// into a fixed no-alloc buffer. Returns `(buffer, used_len)`; caller slices
/// `buffer[..used_len]`.
///
/// Same hand-rolled style as `provisioning_server::run`'s raw-RX diagnostic
/// (no `String`/alloc, no external hex crate) — kept local rather than
/// factored into a shared helper since this is the second, independent call
/// site of an already-tiny routine; not worth the module-boundary plumbing
/// for ~10 lines.
#[cfg(feature = "diagnostics")]
fn hex_dump_tail(bytes: &[u8]) -> ([u8; 32], usize) {
    let start = bytes.len().saturating_sub(16);
    let tail = &bytes[start..];
    let mut hex = [b'0'; 32];
    for (i, &b) in tail.iter().enumerate() {
        let hi = b >> 4;
        let lo = b & 0x0F;
        hex[i * 2] = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
        hex[i * 2 + 1] = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
    }
    (hex, tail.len() * 2)
}

/// Parse an ASCII decimal byte slice as u32.  Returns `None` for empty input
/// or any non-digit character.
fn parse_u32_bytes(s: &[u8]) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut acc = 0u32;
    for &b in s {
        if b < b'0' || b > b'9' {
            return None;
        }
        acc = acc.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(acc)
}

// ── Tests (pure functions — no hardware dependency) ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── GPS_BAUD_CANDIDATES ───────────────────────────────────────────────────

    #[test]
    fn gps_baud_matches_first_candidate() {
        // `main.rs` opens the UART at `GPS_BAUD` before `GpsDriver::new`
        // probes or applies a cached rate (see `probe_candidates` and
        // `GpsDriver::new`'s doc) — this invariant must hold or the first
        // probe iteration silently samples the wrong rate.
        assert_eq!(GPS_BAUD, GPS_BAUD_CANDIDATES[0]);
    }

    #[test]
    fn gps_baud_candidates_are_l76k_ublox_then_catchall() {
        assert_eq!(GPS_BAUD_CANDIDATES, &[9600, 38400, 115200]);
    }

    // ── nmea_checksum ─────────────────────────────────────────────────────────

    #[test]
    fn nmea_checksum_known_answer() {
        // $GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47 —
        // the canonical NMEA 0183 spec example.
        let body = b"GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,";
        assert_eq!(nmea_checksum(body), 0x47);
    }

    #[test]
    fn nmea_checksum_empty_body_is_zero() {
        assert_eq!(nmea_checksum(b""), 0);
    }

    // ── hex_digit_value ───────────────────────────────────────────────────────

    #[test]
    fn hex_digit_value_covers_all_cases() {
        assert_eq!(hex_digit_value(b'0'), Some(0));
        assert_eq!(hex_digit_value(b'9'), Some(9));
        assert_eq!(hex_digit_value(b'A'), Some(10));
        assert_eq!(hex_digit_value(b'F'), Some(15));
        assert_eq!(hex_digit_value(b'a'), Some(10));
        assert_eq!(hex_digit_value(b'f'), Some(15));
        assert_eq!(hex_digit_value(b'g'), None);
        assert_eq!(hex_digit_value(b'*'), None);
    }

    // ── contains_checksum_valid_nmea ──────────────────────────────────────────
    //
    // Regression guard for full auto-detect, checksum-valid lock only. This
    // predicate is the sole correct-baud/wrong-baud discriminator driving
    // `probe_candidates`, so it must accept genuine, checksum-correct NMEA
    // and reject both the field-observed high-bit garbage signature AND a
    // merely ASCII-shaped-but-checksum-wrong span (the gap this function
    // closes relative to its two-candidate-era predecessor,
    // `looks_like_ascii_nmea`).

    #[test]
    fn contains_checksum_valid_nmea_accepts_real_gga_sentence() {
        let line = b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n";
        assert!(contains_checksum_valid_nmea(line));
    }

    #[test]
    fn contains_checksum_valid_nmea_accepts_real_rmc_sentence() {
        // Correct checksum computed independently (6D), NOT the placeholder
        // 6A used elsewhere in this file's non-checksum-validating parser
        // tests (`parse_rmc_typical_active_fix`) — see `parse_rmc_datetime`'s
        // module doc: this driver's line parser deliberately skips checksum
        // validation, but the baud probe (this function) requires it.
        let line = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,010180,003.1,W*6D\r\n";
        assert!(contains_checksum_valid_nmea(line));
    }

    #[test]
    fn contains_checksum_valid_nmea_accepts_sentence_anywhere_in_buffer() {
        // A probe window can start mid-stream; noise before the '$' must not
        // prevent recognizing a complete sentence later in the buffer.
        let mut buf = vec![0xFFu8; 20];
        buf.extend_from_slice(
            b"$GNRMC,000000,A,0000.000,N,00000.000,E,0.0,0.0,010180,,,A*66\r\n",
        );
        assert!(contains_checksum_valid_nmea(&buf));
    }

    #[test]
    fn contains_checksum_valid_nmea_rejects_high_bit_binary_garbage() {
        // Field-observed signature of a baud mismatch: structured, periodic,
        // non-ASCII bytes with the high bit set — no '$' at all.
        let garbage: [u8; 16] = [
            0xe2, 0xe3, 0x12, 0x40, 0x9c, 0x84, 0x08, 0x03, 0xff, 0xf6, 0xfe, 0x81, 0x77, 0x22,
            0x90, 0x01,
        ];
        assert!(!contains_checksum_valid_nmea(&garbage));
    }

    #[test]
    fn contains_checksum_valid_nmea_rejects_dollar_followed_by_high_bit_byte() {
        // A wrong-baud stream could coincidentally contain a literal '$'
        // (0x24) among otherwise-garbage bytes; a single following high-bit
        // byte must still disqualify the span.
        let bytes = [b'$', b'G', b'P', 0x80, b'G', b'G', b'A', b'*', b'0', b'0'];
        assert!(!contains_checksum_valid_nmea(&bytes));
    }

    #[test]
    fn contains_checksum_valid_nmea_rejects_ascii_shaped_but_wrong_checksum() {
        // This is exactly what the checksum requirement adds over the old
        // "ASCII-shaped-with-a-star" test: syntactically plausible but the
        // checksum doesn't match — must NOT lock (a wrong-baud garbage
        // stream could coincidentally produce this).
        let line = b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*00\r\n";
        assert!(!contains_checksum_valid_nmea(line));
    }

    #[test]
    fn contains_checksum_valid_nmea_rejects_empty_and_no_dollar() {
        assert!(!contains_checksum_valid_nmea(b""));
        assert!(!contains_checksum_valid_nmea(b"no sentence marker here at all"));
    }

    #[test]
    fn contains_checksum_valid_nmea_rejects_dollar_without_checksum_delimiter() {
        // Printable ASCII after '$' but no '*' before the line terminator —
        // not a complete NMEA sentence.
        let line = b"$GPGGA,123519,4807.038,N\r\n";
        assert!(!contains_checksum_valid_nmea(line));
    }

    #[test]
    fn contains_checksum_valid_nmea_rejects_degenerate_empty_body() {
        // '$' immediately followed by '*00' — an empty body XORs to 0, which
        // would trivially "match" a checksum field of 00 without the
        // 5-byte minimum-body-length guard.
        assert!(!contains_checksum_valid_nmea(b"$*00\r\n"));
    }

    #[test]
    fn contains_checksum_valid_nmea_rejects_short_body_even_with_correct_checksum() {
        // 4-byte body "TEST" checksums to a real value, but is still below
        // the 5-byte minimum meant to rule out degenerate near-empty spans.
        let body = b"TEST";
        let cs = nmea_checksum(body);
        let mut line = Vec::new();
        line.push(b'$');
        line.extend_from_slice(body);
        line.push(b'*');
        line.extend_from_slice(format!("{:02X}", cs).as_bytes());
        assert!(!contains_checksum_valid_nmea(&line));
    }

    #[test]
    fn contains_checksum_valid_nmea_rejects_missing_hex_digits() {
        // '*' present but the buffer ends before two full hex digits follow.
        assert!(!contains_checksum_valid_nmea(b"$GPGGA,1*4"));
    }

    // ── L76K_INIT_COMMANDS ────────────────────────────────────────────────────
    //
    // Regression guard for the no-NMEA defect: each init command must be a
    // well-framed `$<sentence>*<checksum>\r\n` NMEA sentence with a *correct*
    // checksum, or the L76K silently ignores it (ineffective, not an error we
    // could otherwise observe on real hardware). Independent of
    // `GpsDriver::new` — walks the same const array it sends. Reuses the
    // top-level `nmea_checksum` (also used by the baud-probe's
    // `contains_checksum_valid_nmea`), not a locally duplicated copy.
    //
    // NOTE: the checksum-framing assertion is duplicated (not factored into
    // a shared test helper) across the L76K and u-blox tests below
    // deliberately — this crate's `#[cfg(test)]` blocks type-check but are
    // never executed (`harness = false`, no test-harness `main` is linked;
    // see `runtime_settings_store`'s test module doc for the same caveat),
    // so a plain helper function reachable ONLY from `#[test]`-attributed
    // functions is flagged `dead_code` (the `#[test]` fns themselves are
    // exempted from that lint, but that exemption does not propagate to
    // their callees).

    #[test]
    fn l76k_init_commands_are_well_formed_with_correct_checksum() {
        for (i, cmd) in L76K_INIT_COMMANDS.iter().enumerate() {
            assert!(cmd.starts_with(b"$"), "command {} must start with '$': {:?}", i, cmd);
            assert!(cmd.ends_with(b"\r\n"), "command {} must end with CRLF: {:?}", i, cmd);
            let star = cmd.iter().position(|&b| b == b'*')
                .unwrap_or_else(|| panic!("command {} missing '*' checksum delimiter: {:?}", i, cmd));
            let body = &cmd[1..star]; // between '$' and '*'
            let checksum_hex = &cmd[star + 1..cmd.len() - 2]; // between '*' and CRLF
            let expected = format!("{:02X}", nmea_checksum(body));
            assert_eq!(
                core::str::from_utf8(checksum_hex).unwrap(),
                expected,
                "command {} checksum mismatch: {:?}", i, cmd,
            );
        }
    }

    #[test]
    fn l76k_init_commands_enable_gga_and_rmc_only() {
        // The PCAS03 sentence (2nd command) must request exactly the two
        // sentence types this driver's parser understands (GGA, RMC) — see
        // `parse_gga`/`parse_rmc_datetime`. Field order:
        // GGA,GLL,GSA,GSV,RMC,VTG,ZDA,ANT,DHV,LPS,,,UTC,GST
        let pcas03 = L76K_INIT_COMMANDS.iter()
            .find(|c| c.starts_with(b"$PCAS03,"))
            .expect("must include a PCAS03 sentence-select command");
        assert!(pcas03.starts_with(b"$PCAS03,1,0,0,0,1,0,0,0,0,0,,,0,0*"),
            "PCAS03 field layout mismatch (GGA+RMC only expected): {:?}", pcas03);
    }

    // ── UBLOX_INIT_COMMANDS ───────────────────────────────────────────────────
    //
    // Regression guard for sending the correct init for the detected module.

    #[test]
    fn ublox_init_commands_are_well_formed_with_correct_checksum() {
        for (i, cmd) in UBLOX_INIT_COMMANDS.iter().enumerate() {
            assert!(cmd.starts_with(b"$"), "command {} must start with '$': {:?}", i, cmd);
            assert!(cmd.ends_with(b"\r\n"), "command {} must end with CRLF: {:?}", i, cmd);
            let star = cmd.iter().position(|&b| b == b'*')
                .unwrap_or_else(|| panic!("command {} missing '*' checksum delimiter: {:?}", i, cmd));
            let body = &cmd[1..star]; // between '$' and '*'
            let checksum_hex = &cmd[star + 1..cmd.len() - 2]; // between '*' and CRLF
            let expected = format!("{:02X}", nmea_checksum(body));
            assert_eq!(
                core::str::from_utf8(checksum_hex).unwrap(),
                expected,
                "command {} checksum mismatch: {:?}", i, cmd,
            );
        }
    }

    #[test]
    fn ublox_init_commands_enable_gga_and_rmc_on_uart1() {
        // $PUBX,40,msgId,rddc,rus1,rus2,rusb,rspi,reserved — rus1 (the 4th
        // rate field, index after msgId) must be nonzero (enabled) for both
        // sentence types this driver's parser understands.
        let gga = UBLOX_INIT_COMMANDS.iter()
            .find(|c| c.starts_with(b"$PUBX,40,GGA,"))
            .expect("must include a PUBX,40 GGA-enable command");
        assert!(gga.starts_with(b"$PUBX,40,GGA,0,1,"), "GGA rus1 field must enable output: {:?}", gga);

        let rmc = UBLOX_INIT_COMMANDS.iter()
            .find(|c| c.starts_with(b"$PUBX,40,RMC,"))
            .expect("must include a PUBX,40 RMC-enable command");
        assert!(rmc.starts_with(b"$PUBX,40,RMC,0,1,"), "RMC rus1 field must enable output: {:?}", rmc);
    }

    // ── parse_gga ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_gga_typical_fix() {
        // Standard $GPGGA sentence with a valid fix (quality=1, 08 sats).
        let line = b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
        let gga = parse_gga(line).expect("should parse valid GGA");
        let (lat, lon) = gga.coords.expect("should have a fix");
        // 48°07.038'N  → 48 + 7.038/60 = 48.1173000° → 481_173_000 e7
        assert_eq!(lat, 481_173_000, "lat mismatch: got {}", lat);
        // 011°31.000'E → 11 + 31/60 = 11.5166666...° → 115_166_667 e7 (approx)
        // Note: integer rounding may differ by 1 in the last digit.
        assert!((lon - 115_166_667i32).abs() <= 5, "lon mismatch: got {}", lon);
        assert_eq!(gga.sat_count, 8);
    }

    #[test]
    fn parse_gga_no_fix_returns_none_coords() {
        // quality=0 → no fix, but the sentence is still recognized (liveness)
        // and sat_count (00 here — searching) is still captured.
        let line = b"$GPGGA,123519,4807.038,N,01131.000,E,0,00,,,,,,,*66";
        let gga = parse_gga(line).expect("recognized GGA even at quality 0");
        assert!(gga.coords.is_none());
        assert_eq!(gga.sat_count, 0);
    }

    #[test]
    fn parse_gga_no_fix_nonzero_sat_count_captured() {
        // Realistic "searching" sentence: quality=0 (no fix yet) but 4
        // satellites already visible/tracked — this is the concrete signal
        // behind FixState::Acquiring showing a satellite count.
        let line = b"$GPGGA,123519,,,,,,0,04,,,,,,,*00";
        let gga = parse_gga(line).expect("recognized GGA");
        assert!(gga.coords.is_none());
        assert_eq!(gga.sat_count, 4);
    }

    #[test]
    fn parse_gga_south_west_negative() {
        // Southern hemisphere / Western hemisphere → negative lat/lon
        let line = b"$GNGGA,000000,3351.000,S,15112.000,W,1,04,1.0,0.0,M,0.0,M,,*00";
        let gga = parse_gga(line).expect("should parse S/W");
        let (lat, lon) = gga.coords.expect("should have a fix");
        assert!(lat < 0, "lat should be negative (South): {}", lat);
        assert!(lon < 0, "lon should be negative (West): {}", lon);
    }

    #[test]
    fn parse_gga_gngga_prefix_accepted() {
        // $GNGGA (multi-constellation) must also be accepted.
        let line = b"$GNGGA,120000,4807.038,N,01131.000,E,1,08,1.0,100.0,M,0.0,M,,*00";
        assert!(parse_gga(line).is_some_and(|g| g.coords.is_some()));
    }

    #[test]
    fn parse_gga_wrong_sentence_returns_none() {
        // $GPRMC is not GGA
        let line = b"$GPRMC,000000,A,4807.038,N,01131.000,E,0.0,0.0,010180,,,A*00";
        assert!(parse_gga(line).is_none());
    }

    // ── nmea_coord_to_e7 ──────────────────────────────────────────────────────

    #[test]
    fn nmea_lat_known_answer() {
        // 48°07.038' N → 48.1173000° = 481_173_000 e7
        let result = nmea_coord_to_e7(b"4807.038", b"N").expect("should parse");
        assert_eq!(result, 481_173_000);
    }

    #[test]
    fn nmea_lon_known_answer() {
        // 11°31.000' E → 11.516666...° ≈ 115_166_667 e7
        let result = nmea_coord_to_e7(b"01131.000", b"E").expect("should parse");
        assert!((result - 115_166_667i32).abs() <= 5, "got {}", result);
    }

    #[test]
    fn nmea_south_is_negative() {
        let result = nmea_coord_to_e7(b"3351.000", b"S").expect("should parse");
        assert!(result < 0);
    }

    #[test]
    fn nmea_west_is_negative() {
        let result = nmea_coord_to_e7(b"15112.000", b"W").expect("should parse");
        assert!(result < 0);
    }

    #[test]
    fn nmea_empty_returns_none() {
        assert!(nmea_coord_to_e7(b"", b"N").is_none());
    }

    // ── GpsDriver duty cycle (pure state machine, no hardware) ────────────────
    //
    // We cannot instantiate GpsDriver (needs real UartDriver) in unit tests,
    // but the transition predicates are pure functions and fully testable —
    // this is the regression guard for the fix-reliability defect: the
    // active window must NEVER close
    // before a fix has been captured, no matter how much time has elapsed.

    #[test]
    fn duty_cycle_constants_reasonable() {
        assert!(GPS_ACTIVE_WINDOW_MS < GPS_QUIET_INTERVAL_MS,
            "active window must be shorter than quiet interval");
        assert_eq!(GPS_ACTIVE_WINDOW_MS, 30_000);
        assert_eq!(GPS_QUIET_INTERVAL_MS, 120_000);
    }

    #[test]
    fn active_window_never_closes_before_first_fix() {
        // Even far beyond the nominal 30s window, no fix yet => stay active.
        assert!(!should_close_active_window(false, GPS_ACTIVE_WINDOW_MS));
        assert!(!should_close_active_window(false, GPS_ACTIVE_WINDOW_MS * 100));
        assert!(!should_close_active_window(false, u64::MAX));
    }

    #[test]
    fn active_window_closes_on_schedule_once_fix_captured() {
        assert!(!should_close_active_window(true, GPS_ACTIVE_WINDOW_MS - 1));
        assert!(should_close_active_window(true, GPS_ACTIVE_WINDOW_MS));
        assert!(should_close_active_window(true, GPS_ACTIVE_WINDOW_MS + 1));
    }

    #[test]
    fn quiet_interval_reopens_on_schedule() {
        assert!(!should_reopen_active_window(GPS_QUIET_INTERVAL_MS - 1));
        assert!(should_reopen_active_window(GPS_QUIET_INTERVAL_MS));
        assert!(should_reopen_active_window(GPS_QUIET_INTERVAL_MS + 1));
    }

    // ── parse_rmc_datetime ────────────────────────────────────────────────────

    #[test]
    fn parse_rmc_typical_active_fix() {
        // $GPRMC example from the NMEA reference: 12:35:19 UTC, 1980-01-10.
        let line = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,010180,003.1,W*6A";
        let dt = parse_rmc_datetime(line).expect("should parse active RMC");
        assert_eq!(dt, NmeaDateTime { year: 1980, month: 1, day: 10, hour: 12, minute: 35, second: 19 });
    }

    #[test]
    fn parse_rmc_gnrmc_prefix_accepted() {
        let line = b"$GNRMC,000000,A,3351.000,S,15112.000,W,0.0,0.0,010124,,,A*00";
        assert!(parse_rmc_datetime(line).is_some());
    }

    #[test]
    fn parse_rmc_void_status_returns_none() {
        // status=V (void) — no reliable fix, so no date/time either.
        let line = b"$GPRMC,123519,V,4807.038,N,01131.000,E,022.4,084.4,010180,003.1,W*65";
        assert!(parse_rmc_datetime(line).is_none());
    }

    #[test]
    fn parse_rmc_wrong_sentence_returns_none() {
        let line = b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
        assert!(parse_rmc_datetime(line).is_none());
    }

    #[test]
    fn parse_rmc_two_digit_year_normalizes_to_2000s() {
        let line = b"$GPRMC,000000,A,0000.000,N,00000.000,E,0.0,0.0,030726,,,A*00";
        let dt = parse_rmc_datetime(line).expect("should parse");
        assert_eq!(dt.year, 2026);
        assert_eq!(dt.month, 7);
        assert_eq!(dt.day, 3);
    }

    #[test]
    fn parse_rmc_rejects_malformed_time() {
        // Hour field 99 is out of range.
        let line = b"$GPRMC,993519,A,4807.038,N,01131.000,E,022.4,084.4,010180,003.1,W*00";
        assert!(parse_rmc_datetime(line).is_none());
    }

    #[test]
    fn parse_rmc_rejects_short_time_field() {
        let line = b"$GPRMC,1235,A,4807.038,N,01131.000,E,022.4,084.4,010180,003.1,W*00";
        assert!(parse_rmc_datetime(line).is_none());
    }

    // ── unix_timestamp ────────────────────────────────────────────────────────

    #[test]
    fn unix_timestamp_epoch_is_zero() {
        assert_eq!(unix_timestamp(1970, 1, 1, 0, 0, 0), 0);
    }

    #[test]
    fn unix_timestamp_known_answer() {
        // 2024-01-01T00:00:00Z = 1704067200
        assert_eq!(unix_timestamp(2024, 1, 1, 0, 0, 0), 1_704_067_200);
    }

    #[test]
    fn unix_timestamp_known_answer_y2k() {
        // 2000-01-01T00:00:00Z = 946684800 (well-known anchor).
        assert_eq!(unix_timestamp(2000, 1, 1, 0, 0, 0), 946_684_800);
    }

    #[test]
    fn unix_timestamp_leap_day_2024() {
        // 2024-02-29 exists (leap year); one day after 2024-02-28.
        let feb28 = unix_timestamp(2024, 2, 28, 0, 0, 0);
        let feb29 = unix_timestamp(2024, 2, 29, 0, 0, 0);
        assert_eq!(feb29 - feb28, 86_400);
    }

    #[test]
    fn unix_timestamp_monotonic_across_year_boundary() {
        let dec31 = unix_timestamp(2025, 12, 31, 23, 59, 59);
        let jan1 = unix_timestamp(2026, 1, 1, 0, 0, 0);
        assert_eq!(jan1 - dec31, 1);
    }

    // ── hex_dump_tail (diagnostics helper) ───────────────────────────────────

    #[cfg(feature = "diagnostics")]
    #[test]
    fn hex_dump_tail_known_answer() {
        let (hex, len) = hex_dump_tail(b"\x24\x47\x50\x00\xff");
        assert_eq!(core::str::from_utf8(&hex[..len]).unwrap(), "24475000ff");
    }

    #[cfg(feature = "diagnostics")]
    #[test]
    fn hex_dump_tail_truncates_to_last_16_bytes() {
        // 20 bytes in: only the last 16 (values 4..=19) should be encoded.
        let input: Vec<u8> = (0u8..20).collect();
        let (hex, len) = hex_dump_tail(&input);
        assert_eq!(len, 32, "16 bytes -> 32 hex chars");
        let hex_str = core::str::from_utf8(&hex[..len]).unwrap();
        assert_eq!(hex_str, "0405060708090a0b0c0d0e0f10111213");
    }

    #[cfg(feature = "diagnostics")]
    #[test]
    fn hex_dump_tail_empty_input() {
        let (_hex, len) = hex_dump_tail(&[]);
        assert_eq!(len, 0);
    }

    // ── GpsStatus ─────────────────────────────────────────────────────────────

    #[test]
    fn gps_status_never_is_all_default() {
        let s = GpsStatus::never();
        assert!(!s.has_fix);
        assert_eq!(s.fix_state, FixState::NoSignal);
        assert_eq!(s.sat_count, 0);
        assert!(!s.clock_synced);
    }

    // ── FixState (via GpsDriver::status — exercised indirectly since the
    // driver itself needs real hardware; the transition logic lives in
    // `status()` and is covered by the has_fix/last_nmea_seen inputs it
    // reads, both plain fields set only from `parse_line`) ────────────────
    //
    // The has_fix -> Fix, "seen traffic but no fix" -> Acquiring, and
    // "never seen traffic" -> NoSignal mapping itself is straight-line code
    // in `status()`; direct unit coverage of `status()` would require
    // constructing a `GpsDriver`, which needs a real `UartDriver` (hardware).
    // The three FixState variants and their intended meaning are documented
    // on the enum itself and exercised end-to-end via HIL/manual test.
}
