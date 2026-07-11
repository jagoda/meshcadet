// SPDX-License-Identifier: GPL-3.0-only
//! GPS: pure NMEA parsing, checksum/baud validation, and duty-cycle math.
//!
//! `firmware::gps::GpsDriver` (UART1 ownership, baud probing/self-heal, NVS
//! baud cache, `settimeofday`) stays in `firmware/src/gps.rs` — it needs real
//! hardware. Everything below it does NOT: NMEA sentence parsing (GGA
//! position + RMC date/time), the NMEA checksum + `$…*HH` framing validator
//! that the baud probe uses to distinguish real traffic from wrong-baud
//! garbage, the L76K/u-blox init command tables, calendar arithmetic
//! (civil-date → Unix timestamp), and the ACTIVE/QUIET duty-cycle transition
//! predicates — all pure functions over bytes/integers, with no UART or NVS
//! dependency. This is where `firmware::gps`'s pure subset now lives so its
//! tests execute under `cargo test --workspace` (`firmware/` is a detached,
//! cross-compiled workspace — see root `Cargo.toml`'s doc comment — so a
//! `#[cfg(test)]` block written there type-checks but never runs).
//! `firmware/src/gps.rs` re-consumes this module via `pub use
//! firmware_core::gps::*;` so every existing call site resolves unchanged.
//! See `docs/adr/0005-firmware-core-extraction.md`.

// ── Baud-rate candidates ─────────────────────────────────────────────────────

/// Candidate UART baud rates probed at boot, in probe order (see
/// `firmware::gps::probe_candidates`). The T-Deck Plus ships with either of
/// two GPS module variants — a Quectel L76K (default 9600 bps) or a u-blox
/// M10Q (default 38400 bps) — and unit-to-unit hardware variance means the
/// running firmware cannot assume which one is installed (full auto-detect
/// across three rates is the deliberate design). 115200 is included as a
/// catch-all for a reconfigured/variant module reporting at neither
/// documented default; a rate that locks there is treated as a u-blox-family
/// module for init purposes (see [`L76K_INIT_COMMANDS`]/[`UBLOX_INIT_COMMANDS`])
/// since 9600 is the only rate specifically associated with the MTK/L76K
/// `$PCAS` command family. All three candidates use 8N1 framing, so only the
/// baud rate itself needs probing. Probed in the documented-likely order:
/// L76K (9600), then u-blox M10Q (38400), then the 115200 catch-all.
pub const GPS_BAUD_CANDIDATES: &[u32] = &[9600, 38400, 115200];

/// Baud rate the GPS UART is opened at in `main.rs::run()`, before probing.
/// Must equal `GPS_BAUD_CANDIDATES[0]` — `firmware::gps::GpsDriver::new`
/// probes from there and, if the module is actually running at a later
/// candidate, switches the already-open UART via `change_baudrate` rather
/// than requiring a fresh config.
pub const GPS_BAUD: u32 = GPS_BAUD_CANDIDATES[0];

// ── NMEA checksum + framing validation ───────────────────────────────────────

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
pub fn contains_checksum_valid_nmea(bytes: &[u8]) -> bool {
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
                if let (Some(&hi), Some(&lo)) = (bytes.get(star_pos + 1), bytes.get(star_pos + 2)) {
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
// The L76K triad is sent once per boot by `firmware::gps::GpsDriver`, right
// after the boot-time baud probe/cache lookup locks onto
// `GPS_BAUD_CANDIDATES[0]` (9600 — the L76K's rate), and mirrors two
// independent, hardware-proven references for this exact module or
// lookalikes on the same/nearby T-Deck hardware family: LilyGo's own
// reference firmware (`examples/UnitTest/UnitTest.ino::setupGPS()`) and the
// production Meshtastic T-Deck driver (`src/gps/GPS.cpp`, `GNSS_MODEL_MTK`
// branch) — both send this same sequence (with the same 250 ms inter-command
// pacing) before relying on the L76K's NMEA stream.
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

/// L76K init command sequence, sent once at driver construction when the
/// probe/cache locks onto `GPS_BAUD_CANDIDATES[0]` (9600):
/// 1. `$PCAS04,7` — enable GPS + GLONASS + BEIDOU constellations.
/// 2. `$PCAS03,...` — restrict NMEA output to GGA + RMC only (the two
///    sentence types [`parse_gga`]/[`parse_rmc_datetime`] understand; cuts
///    UART traffic during the always-on pre-fix ACTIVE window).
/// 3. `$PCAS11,3` — vehicle dynamic model (matches reference firmware; the
///    L76K's default aviation-leaning model is tuned for higher dynamics
///    than a pedestrian/vehicle-carried tracker needs).
pub const L76K_INIT_COMMANDS: &[&[u8]] = &[
    b"$PCAS04,7*1E\r\n",
    b"$PCAS03,1,0,0,0,1,0,0,0,0,0,,,0,0*02\r\n",
    b"$PCAS11,3*1E\r\n",
];

/// u-blox M10Q (or 115200-catch-all) init command sequence, sent once at
/// driver construction when the probe/cache locks onto a candidate baud
/// OTHER than `GPS_BAUD_CANDIDATES[0]` — see the "Module init sequences"
/// section doc above `L76K_INIT_COMMANDS` for why `$PUBX,40` (not binary UBX,
/// not `$PCAS`) and why this only enables rather than restricts:
/// 1. `$PUBX,40,GGA,0,1,0,0,0,0` — enable GGA once per navigation solution
///    on UART1 (rate fields are DDC, UART1, UART2, USB, SPI, reserved).
/// 2. `$PUBX,40,RMC,0,1,0,0,0,0` — enable RMC once per navigation solution
///    on UART1.
pub const UBLOX_INIT_COMMANDS: &[&[u8]] = &[
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
    /// Uptime milliseconds when this fix was captured. Used to compute fix
    /// age in `firmware::gps::GpsDriver::get_fix_and_age`.
    pub captured_uptime_ms: u64,
}

// ── FixState ──────────────────────────────────────────────────────────────────

/// Three-state acquisition status, distinguishing "actively searching" from
/// "nothing heard from the module at all" — the two states a bare `has_fix`
/// boolean collapses into a single, indistinguishable "no fix" reading.
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
    /// fix goes stale — see `firmware::gps::GpsDriver::get_fix_and_age`'s
    /// age-surfacing contract.
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

// ── Duty-cycle transition predicates (pure — testable without hardware) ───────

/// `true` when an open ACTIVE window should close.
///
/// Gated on `has_fix`: before the first fix is captured, the window never
/// closes on elapsed time alone — see `firmware::gps`'s module doc "Duty
/// cycle" section. Once a fix exists, closes after `GPS_ACTIVE_WINDOW_MS` as
/// documented.
pub fn should_close_active_window(has_fix: bool, elapsed_ms: u64) -> bool {
    has_fix && elapsed_ms >= GPS_ACTIVE_WINDOW_MS
}

/// `true` when a QUIET interval should reopen into an ACTIVE window.
pub fn should_reopen_active_window(elapsed_ms: u64) -> bool {
    elapsed_ms >= GPS_QUIET_INTERVAL_MS
}

// ── RMC date/time parsing + calendar arithmetic ────────────────────────────────

/// A UTC calendar date+time decoded from an NMEA `$..RMC` sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NmeaDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Convert a UTC civil date+time to a Unix timestamp (seconds since
/// 1970-01-01T00:00:00Z).  Pure calendar arithmetic — Howard Hinnant's
/// `days_from_civil` algorithm, valid for the full proleptic Gregorian
/// calendar without any external date/time dependency.
pub fn unix_timestamp(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
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
pub fn parse_rmc_datetime(line: &[u8]) -> Option<NmeaDateTime> {
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

    Some(NmeaDateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    })
}

// ── NMEA GGA parser (pure functions, no hardware dependency) ──────────────────

/// Parsed contents of a `$GPGGA`/`$GNGGA` sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GgaFields {
    /// `Some((lat_e7, lon_e7))` when fix quality ≥ 1. `None` for a
    /// syntactically valid GGA sentence reporting quality 0 (searching — no
    /// 3D fix yet; lat/lon fields are typically empty in that case).
    pub coords: Option<(i32, i32)>,
    /// Satellites used (field 7, `numSV`) — meaningful regardless of
    /// `coords`, so callers can surface it while still acquiring. `0` if the
    /// field is empty or unparseable.
    pub sat_count: u8,
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
pub fn parse_gga(line: &[u8]) -> Option<GgaFields> {
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
pub fn hex_dump_tail(bytes: &[u8]) -> ([u8; 32], usize) {
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
        if !b.is_ascii_digit() {
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
        buf.extend_from_slice(b"$GNRMC,000000,A,0000.000,N,00000.000,E,0.0,0.0,010180,,,A*66\r\n");
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
        assert!(!contains_checksum_valid_nmea(
            b"no sentence marker here at all"
        ));
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
    // `firmware::gps::GpsDriver::new` — walks the same const array it sends.
    // Reuses the top-level `nmea_checksum` (also used by the baud-probe's
    // `contains_checksum_valid_nmea`), not a locally duplicated copy.

    #[test]
    fn l76k_init_commands_are_well_formed_with_correct_checksum() {
        for (i, cmd) in L76K_INIT_COMMANDS.iter().enumerate() {
            assert!(
                cmd.starts_with(b"$"),
                "command {} must start with '$': {:?}",
                i,
                cmd
            );
            assert!(
                cmd.ends_with(b"\r\n"),
                "command {} must end with CRLF: {:?}",
                i,
                cmd
            );
            let star = cmd.iter().position(|&b| b == b'*').unwrap_or_else(|| {
                panic!("command {} missing '*' checksum delimiter: {:?}", i, cmd)
            });
            let body = &cmd[1..star]; // between '$' and '*'
            let checksum_hex = &cmd[star + 1..cmd.len() - 2]; // between '*' and CRLF
            let expected = format!("{:02X}", nmea_checksum(body));
            assert_eq!(
                core::str::from_utf8(checksum_hex).unwrap(),
                expected,
                "command {} checksum mismatch: {:?}",
                i,
                cmd,
            );
        }
    }

    #[test]
    fn l76k_init_commands_enable_gga_and_rmc_only() {
        // The PCAS03 sentence (2nd command) must request exactly the two
        // sentence types this driver's parser understands (GGA, RMC) — see
        // `parse_gga`/`parse_rmc_datetime`. Field order:
        // GGA,GLL,GSA,GSV,RMC,VTG,ZDA,ANT,DHV,LPS,,,UTC,GST
        let pcas03 = L76K_INIT_COMMANDS
            .iter()
            .find(|c| c.starts_with(b"$PCAS03,"))
            .expect("must include a PCAS03 sentence-select command");
        assert!(
            pcas03.starts_with(b"$PCAS03,1,0,0,0,1,0,0,0,0,0,,,0,0*"),
            "PCAS03 field layout mismatch (GGA+RMC only expected): {:?}",
            pcas03
        );
    }

    // ── UBLOX_INIT_COMMANDS ───────────────────────────────────────────────────
    //
    // Regression guard for sending the correct init for the detected module.

    #[test]
    fn ublox_init_commands_are_well_formed_with_correct_checksum() {
        for (i, cmd) in UBLOX_INIT_COMMANDS.iter().enumerate() {
            assert!(
                cmd.starts_with(b"$"),
                "command {} must start with '$': {:?}",
                i,
                cmd
            );
            assert!(
                cmd.ends_with(b"\r\n"),
                "command {} must end with CRLF: {:?}",
                i,
                cmd
            );
            let star = cmd.iter().position(|&b| b == b'*').unwrap_or_else(|| {
                panic!("command {} missing '*' checksum delimiter: {:?}", i, cmd)
            });
            let body = &cmd[1..star];
            let checksum_hex = &cmd[star + 1..cmd.len() - 2];
            let expected = format!("{:02X}", nmea_checksum(body));
            assert_eq!(
                core::str::from_utf8(checksum_hex).unwrap(),
                expected,
                "command {} checksum mismatch: {:?}",
                i,
                cmd,
            );
        }
    }

    #[test]
    fn ublox_init_commands_enable_gga_and_rmc_on_uart1() {
        let gga = UBLOX_INIT_COMMANDS
            .iter()
            .find(|c| c.starts_with(b"$PUBX,40,GGA,"))
            .expect("must include a PUBX,40,GGA command");
        assert!(
            gga.starts_with(b"$PUBX,40,GGA,0,1,0,0,0,0*"),
            "PUBX GGA field layout mismatch (UART1 enable expected): {:?}",
            gga
        );

        let rmc = UBLOX_INIT_COMMANDS
            .iter()
            .find(|c| c.starts_with(b"$PUBX,40,RMC,"))
            .expect("must include a PUBX,40,RMC command");
        assert!(
            rmc.starts_with(b"$PUBX,40,RMC,0,1,0,0,0,0*"),
            "PUBX RMC field layout mismatch (UART1 enable expected): {:?}",
            rmc
        );
    }

    // ── parse_gga ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_gga_typical_fix() {
        let line = b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
        let gga = parse_gga(line).expect("should parse valid GGA");
        assert_eq!(gga.sat_count, 8);
        let (lat_e7, lon_e7) = gga.coords.expect("fix quality 1 must yield coords");
        assert_eq!(lat_e7, 481_173_000);
        assert_eq!(lon_e7, 115_166_667);
    }

    #[test]
    fn parse_gga_no_fix_returns_none_coords() {
        let line = b"$GPGGA,123519,,,,,0,00,,,M,,M,,*66";
        let gga = parse_gga(line).expect("recognized GGA even at quality 0");
        assert!(gga.coords.is_none());
    }

    #[test]
    fn parse_gga_no_fix_nonzero_sat_count_captured() {
        let line = b"$GPGGA,123519,,,,,0,04,,,M,,M,,*62";
        let gga = parse_gga(line).expect("recognized GGA");
        assert_eq!(
            gga.sat_count, 4,
            "sat count must be captured even without a fix"
        );
        assert!(gga.coords.is_none());
    }

    #[test]
    fn parse_gga_south_west_negative() {
        let line = b"$GPGGA,123519,3351.000,S,15112.000,W,1,08,0.9,545.4,M,46.9,M,,*7F";
        let gga = parse_gga(line).expect("should parse S/W");
        let (lat_e7, lon_e7) = gga.coords.expect("fix quality 1 must yield coords");
        assert!(lat_e7 < 0, "south latitude must be negative");
        assert!(lon_e7 < 0, "west longitude must be negative");
    }

    #[test]
    fn parse_gga_gngga_prefix_accepted() {
        let line = b"$GNGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*74";
        assert!(parse_gga(line).is_some_and(|g| g.coords.is_some()));
    }

    #[test]
    fn parse_gga_wrong_sentence_returns_none() {
        let line = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,010180,003.1,W*6A";
        assert!(parse_gga(line).is_none());
    }

    // ── nmea_coord_to_e7 ─────────────────────────────────────────────────────

    #[test]
    fn nmea_lat_known_answer() {
        let result = nmea_coord_to_e7(b"4807.038", b"N").expect("should parse");
        assert_eq!(result, 481_173_000);
    }

    #[test]
    fn nmea_lon_known_answer() {
        let result = nmea_coord_to_e7(b"01131.000", b"E").expect("should parse");
        assert_eq!(result, 115_166_667);
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

    // ── Duty-cycle constants + predicates ────────────────────────────────────

    #[test]
    fn duty_cycle_constants_reasonable() {
        const {
            assert!(
                GPS_ACTIVE_WINDOW_MS < GPS_QUIET_INTERVAL_MS,
                "active window should be shorter than the quiet interval"
            );
        }
        assert_eq!(GPS_ACTIVE_WINDOW_MS, 30_000);
        assert_eq!(GPS_QUIET_INTERVAL_MS, 120_000);
    }

    #[test]
    fn active_window_never_closes_before_first_fix() {
        assert!(!should_close_active_window(false, GPS_ACTIVE_WINDOW_MS));
        assert!(!should_close_active_window(
            false,
            GPS_ACTIVE_WINDOW_MS * 100
        ));
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
        // Date field "010180" is DDMMYY = day 01, month 01, yy 80 -> per
        // `parse_rmc_datetime`'s documented "L76K reports 2-digit year; NMEA
        // has no century field" rule (also exercised by
        // `parse_rmc_two_digit_year_normalizes_to_2000s` below), that decodes
        // to year 2000+80 = 2080, not the 1980 the field digits might suggest
        // at a glance.
        //
        // FOUND BY THIS TEST FIRST EXECUTING (firmware-core extraction,
        // ADR-0005): the original assertion expected `year: 1980, day: 10` —
        // internally inconsistent with its own stale comment ("1980-01-10",
        // i.e. day 10) AND with the DDMMYY decode of "010180" (day 01) AND
        // with the 2000+yy rule the sibling test below already pins. `firmware/`'s
        // detached workspace meant this `#[cfg(test)]` block only ever
        // type-checked, never ran, so the error was latent until this crate
        // made it host-executable.
        let line = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,010180,003.1,W*6A";
        let dt = parse_rmc_datetime(line).expect("should parse active RMC");
        assert_eq!(
            dt,
            NmeaDateTime {
                year: 2080,
                month: 1,
                day: 1,
                hour: 12,
                minute: 35,
                second: 19
            }
        );
    }

    #[test]
    fn parse_rmc_gnrmc_prefix_accepted() {
        let line = b"$GNRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,010180,003.1,W*69";
        assert!(parse_rmc_datetime(line).is_some());
    }

    #[test]
    fn parse_rmc_void_status_returns_none() {
        let line = b"$GPRMC,123519,V,,,,,,,010180,,,N*53";
        assert!(parse_rmc_datetime(line).is_none());
    }

    #[test]
    fn parse_rmc_wrong_sentence_returns_none() {
        let line = b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
        assert!(parse_rmc_datetime(line).is_none());
    }

    #[test]
    fn parse_rmc_two_digit_year_normalizes_to_2000s() {
        let line = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,010126,003.1,W*6B";
        let dt = parse_rmc_datetime(line).expect("should parse");
        assert_eq!(dt.year, 2026);
    }

    #[test]
    fn parse_rmc_rejects_malformed_time() {
        let line = b"$GPRMC,999999,A,4807.038,N,01131.000,E,022.4,084.4,010180,003.1,W*6A";
        assert!(parse_rmc_datetime(line).is_none());
    }

    #[test]
    fn parse_rmc_rejects_short_time_field() {
        let line = b"$GPRMC,123,A,4807.038,N,01131.000,E,022.4,084.4,010180,003.1,W*6A";
        assert!(parse_rmc_datetime(line).is_none());
    }

    // ── unix_timestamp ───────────────────────────────────────────────────────

    #[test]
    fn unix_timestamp_epoch_is_zero() {
        assert_eq!(unix_timestamp(1970, 1, 1, 0, 0, 0), 0);
    }

    #[test]
    fn unix_timestamp_known_answer() {
        assert_eq!(unix_timestamp(2024, 1, 1, 0, 0, 0), 1_704_067_200);
    }

    #[test]
    fn unix_timestamp_known_answer_y2k() {
        assert_eq!(unix_timestamp(2000, 1, 1, 0, 0, 0), 946_684_800);
    }

    #[test]
    fn unix_timestamp_leap_day_2024() {
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
