// SPDX-License-Identifier: GPL-3.0-only
//! MeshCadet pull-only GPS telemetry request/response codec.
//!
//! # Protocol (application-layer text carried inside a MeshCore DM payload)
//!
//! **Request** (contact → MeshCadet, DM text field):
//! ```text
//! ?loc
//! ```
//!
//! **Response with fix** (MeshCadet → contact, DM text field):
//! ```text
//! loc:lat=XX.XXXXXXX,lon=YY.YYYYYYY,age=Zs
//! ```
//! - `lat` / `lon`: decimal degrees, always 7 decimal places (derived from
//!   the e7 fixed-point integer representation)
//! - `age`: seconds since the last cached GPS fix was captured
//!
//! **No-fix response** (when no fix has ever been obtained):
//! ```text
//! loc:nofix
//! ```
//!
//! # Policy invariant (call-site responsibility)
//!
//! The response is built **only** after
//! [`PolicyFilter::telemetry_enabled`](super::policy::PolicyFilter::telemetry_enabled)
//! returns `true` for the requesting contact's source hash.  Contacts without
//! the telemetry flag, and unknown contacts, are **silently dropped** at the
//! call site in the firmware dispatcher — this module is policy-agnostic and
//! handles only encoding/decoding.  The gate tests live in
//! [`protocol::policy`](super::policy).
//!
//! # no_std compatibility
//!
//! No heap allocation; pure integer arithmetic; no `std` dependency.
//! The module compiles for `xtensa-esp32s3-espidf` without changes.

// ── Constants ─────────────────────────────────────────────────────────────────

/// Magic text that identifies a telemetry pull request (DM text field).
///
/// A contact sends this as the DM text to request MeshCadet's last-known GPS
/// fix.  Detection via [`is_telemetry_request`].
pub const TELEMETRY_REQUEST_MAGIC: &[u8] = b"?loc";

/// Maximum length of a location response text in bytes.
///
/// Worst case: `loc:lat=-90.0000000,lon=-180.0000000,age=4294967295s`
///   = 8+11+5+12+5+10+1 = 52 bytes.  64 gives a comfortable ceiling.
pub const MAX_RESPONSE_LEN: usize = 64;

/// No-fix response literal (no valid GPS position available).
const RESP_NO_FIX: &[u8] = b"loc:nofix";

// ── Request detection ─────────────────────────────────────────────────────────

/// Return `true` if `text` is a telemetry pull request.
///
/// Detection is a prefix match on [`TELEMETRY_REQUEST_MAGIC`] (`?loc`) that is
/// tolerant of two real-world text-entry artifacts seen on companion apps:
///
/// - **Leading ASCII whitespace** is trimmed first (`" ?loc"` → matches). A
///   touch keyboard or copy/paste can prepend a space.
/// - **ASCII case is ignored** on the magic (`"?Loc"`, `"?LOC"` → match). Phone
///   keyboards commonly auto-capitalize, which would otherwise silently defeat
///   detection and drop the request — the exact HIL failure this guards against.
///
/// Prefix matching (after trim) still allows future extensions like
/// `?loc verbose` without breaking the gate.
pub fn is_telemetry_request(text: &[u8]) -> bool {
    let trimmed = trim_ascii_start(text);
    trimmed.len() >= TELEMETRY_REQUEST_MAGIC.len()
        && trimmed[..TELEMETRY_REQUEST_MAGIC.len()].eq_ignore_ascii_case(TELEMETRY_REQUEST_MAGIC)
}

/// Drop leading ASCII whitespace bytes (no_std, no allocation).
fn trim_ascii_start(s: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    &s[i..]
}

// ── Response encoding ─────────────────────────────────────────────────────────

/// Encode a telemetry location response into `out`.
///
/// Output format: `loc:lat=XX.XXXXXXX,lon=YY.YYYYYYY,age=Zs`
///
/// - `lat_e7`: latitude in units of 1e-7 degrees (i32).
/// - `lon_e7`: longitude in units of 1e-7 degrees (i32).
/// - `age_secs`: seconds since the cached GPS fix was last refreshed.
///
/// Returns the number of bytes written.  `out` must have `len ≥ MAX_RESPONSE_LEN`.
pub fn encode_telemetry_response(lat_e7: i32, lon_e7: i32, age_secs: u32, out: &mut [u8]) -> usize {
    let mut w = Writer { buf: out, pos: 0 };
    w.push_bytes(b"loc:lat=");
    w.push_coord(lat_e7);
    w.push_bytes(b",lon=");
    w.push_coord(lon_e7);
    w.push_bytes(b",age=");
    w.push_u32(age_secs);
    w.push_byte(b's');
    w.pos
}

/// Encode a no-fix response into `out`.
///
/// Output: `loc:nofix` — emitted when no GPS fix has ever been obtained.
///
/// Returns the number of bytes written.
pub fn encode_no_fix_response(out: &mut [u8]) -> usize {
    let n = RESP_NO_FIX.len().min(out.len());
    out[..n].copy_from_slice(&RESP_NO_FIX[..n]);
    n
}

// ── Response decoding ─────────────────────────────────────────────────────────

/// Parsed telemetry location response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TelemetryResponse {
    /// Latitude in units of 1e-7 degrees.
    pub lat_e7: i32,
    /// Longitude in units of 1e-7 degrees.
    pub lon_e7: i32,
    /// Fix age in whole seconds at time of response.
    pub age_secs: u32,
}

/// Parse a location response text back into numeric fields.
///
/// Returns `None` if the text is not a valid location response (e.g. a request,
/// `loc:nofix`, or malformed format).  Primary use: host-side validation and
/// HIL test assertions.
pub fn decode_telemetry_response(text: &[u8]) -> Option<TelemetryResponse> {
    // Format: "loc:lat=XX.XXXXXXX,lon=YY.YYYYYYY,age=Zs"
    let rest = text.strip_prefix(b"loc:lat=")?;
    let (lat_str, rest) = split_once(rest, b',')?;
    let lat_e7 = parse_coord_e7(lat_str)?;
    let rest = rest.strip_prefix(b"lon=")?;
    let (lon_str, rest) = split_once(rest, b',')?;
    let lon_e7 = parse_coord_e7(lon_str)?;
    let rest = rest.strip_prefix(b"age=")?;
    let age_str = rest.strip_suffix(b"s")?;
    let age_secs = parse_u32(age_str)?;
    Some(TelemetryResponse {
        lat_e7,
        lon_e7,
        age_secs,
    })
}

/// Return `true` if `text` is a no-fix response.
pub fn is_no_fix_response(text: &[u8]) -> bool {
    text == RESP_NO_FIX
}

// ── Internal: no_std writer ───────────────────────────────────────────────────

struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    #[inline]
    fn push_byte(&mut self, b: u8) {
        if self.pos < self.buf.len() {
            self.buf[self.pos] = b;
            self.pos += 1;
        }
    }

    fn push_bytes(&mut self, s: &[u8]) {
        for &b in s {
            self.push_byte(b);
        }
    }

    /// Write a decimal-degree coordinate (e7 integer) as "DD.DDDDDDD".
    ///
    /// Always emits exactly 7 decimal places (zero-padded left if needed).
    /// Negative values are prefixed with `-`.
    fn push_coord(&mut self, val_e7: i32) {
        let neg = val_e7 < 0;
        // Promote to i64 before negating to avoid overflow on i32::MIN.
        let abs = if neg { -(val_e7 as i64) } else { val_e7 as i64 };
        if neg {
            self.push_byte(b'-');
        }
        let int_part = abs / 10_000_000i64;
        let frac_part = abs % 10_000_000i64; // 0..9_999_999

        self.push_u64(int_part as u64);
        self.push_byte(b'.');

        // 7-digit fraction, zero-padded on the left.
        let frac_digits = [
            ((frac_part / 1_000_000) % 10) as u8 + b'0',
            ((frac_part / 100_000) % 10) as u8 + b'0',
            ((frac_part / 10_000) % 10) as u8 + b'0',
            ((frac_part / 1_000) % 10) as u8 + b'0',
            ((frac_part / 100) % 10) as u8 + b'0',
            ((frac_part / 10) % 10) as u8 + b'0',
            ((frac_part) % 10) as u8 + b'0',
        ];
        for &d in &frac_digits {
            self.push_byte(d);
        }
    }

    fn push_u32(&mut self, val: u32) {
        self.push_u64(val as u64);
    }

    /// Write the low 24 bits of `val` big-endian (Cayenne LPP GPS/altitude
    /// packing — two's complement is preserved by taking the low 3 bytes).
    fn push_i24_be(&mut self, val: i32) {
        let u = val as u32;
        self.push_byte((u >> 16) as u8);
        self.push_byte((u >> 8) as u8);
        self.push_byte(u as u8);
    }

    /// Write all 32 bits of `val` big-endian (the raw-millivolt
    /// `LPP_GENERIC_SENSOR` entry — a plain exact integer, not Cayenne's
    /// scaled fixed-point; see [`LPP_GENERIC_SENSOR`]'s doc).
    fn push_u32_be(&mut self, val: u32) {
        self.push_byte((val >> 24) as u8);
        self.push_byte((val >> 16) as u8);
        self.push_byte((val >> 8) as u8);
        self.push_byte(val as u8);
    }

    /// Write both bytes of `val` big-endian (the held-raw-millivolt
    /// `LPP_LUMINOSITY`-repurposed entry — a plain exact integer, not
    /// Cayenne's scaled fixed-point; see [`LPP_LUMINOSITY`]'s doc).
    fn push_u16_be(&mut self, val: u16) {
        self.push_byte((val >> 8) as u8);
        self.push_byte(val as u8);
    }

    fn push_u64(&mut self, val: u64) {
        if val == 0 {
            self.push_byte(b'0');
            return;
        }
        // Collect digits in reverse order, then write forward.
        let mut digits = [0u8; 20]; // u64 max is 20 decimal digits
        let mut n = 0usize;
        let mut v = val;
        while v > 0 {
            digits[n] = (v % 10) as u8 + b'0';
            v /= 10;
            n += 1;
        }
        for i in (0..n).rev() {
            self.push_byte(digits[i]);
        }
    }
}

// ── Internal: no_std parser helpers ──────────────────────────────────────────

/// Split `s` at the first occurrence of byte `sep`.
///
/// Returns `Some((before_sep, after_sep))` or `None` if `sep` is not found.
fn split_once(s: &[u8], sep: u8) -> Option<(&[u8], &[u8])> {
    let pos = s.iter().position(|&b| b == sep)?;
    Some((&s[..pos], &s[pos + 1..]))
}

/// Parse an ASCII decimal string as `u32`.  Returns `None` for empty input or
/// any non-digit character.
fn parse_u32(s: &[u8]) -> Option<u32> {
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

/// Parse a signed decimal-degree string produced by `push_coord` back to an
/// e7 integer.
///
/// Expected format: `[-]DD.DDDDDDD` (exactly 7 decimal places, matching the
/// encoder).  Returns `None` for any deviation (wrong number of decimals,
/// non-digit characters, overflow).
fn parse_coord_e7(s: &[u8]) -> Option<i32> {
    let (neg, s) = if s.first() == Some(&b'-') {
        (true, &s[1..])
    } else {
        (false, s)
    };
    let (int_str, frac_str) = split_once(s, b'.')?;
    let int_part = parse_u32(int_str)? as i64;

    // Require exactly 7 decimal places (encoder always emits exactly 7).
    if frac_str.len() != 7 {
        return None;
    }
    let frac_part = parse_u32(frac_str)? as i64;

    let val_i64 = int_part * 10_000_000 + frac_part;
    if val_i64 > i32::MAX as i64 {
        return None;
    }
    let result = val_i64 as i32;
    Some(if neg { -result } else { result })
}

// ── MeshCore-native telemetry REQ / RESPONSE (companion-app compatible) ─────────
//
// The `?loc` codec above is a MeshCadet-bespoke text-DM protocol that NO stock
// MeshCore companion app speaks. The companion's "request telemetry / location"
// button issues a `CMD_SEND_TELEMETRY_REQ`, which the local node turns into an
// on-air `PAYLOAD_TYPE_REQ` (0x00) datagram and then waits for a
// `PAYLOAD_TYPE_RESPONSE` (0x01) matched by a reflected tag. If no RESPONSE
// arrives the companion shows "Telemetry unavailable. You might not have
// permission, or this contact may be unreachable." — the exact reported HIL
// symptom. This section implements the responder half of that native exchange so
// MeshCadet answers the companion's real request, not a made-up text command.
//
// Wire layout (source: ripplebiz/MeshCore BaseChatMesh.cpp `onPeerDataRecv` +
// `sendRequest`, companion_radio MyMesh.cpp `onContactRequest`):
//
//   REQ plaintext (inside a DM-style [dest][src][MAC][ciphertext] datagram):
//     [tag(4, LE)] [req_type(1)] [reserved/random...]
//   where req_type == REQ_TYPE_GET_TELEMETRY_DATA (0x03) for a telemetry pull.
//   `tag` is an opaque request id the requester matches the response against.
//
//   RESPONSE plaintext (inside a DM-style datagram, PAYLOAD_TYPE_RESPONSE):
//     [tag(4, LE)] [CayenneLPP entries...]
//   The tag is reflected verbatim from the REQ. LPP entries carry the telemetry;
//   a GPS fix is one `LPP_GPS` entry on channel `TELEM_CHANNEL_SELF`. A battery
//   reading, when present, is a percentage + charging-state pair followed by a
//   raw-millivolt entry — always appended in that order, never inserted before
//   the pair, so a peer whose decoder breaks on an unrecognised entry type
//   still recovers percent/charging.

/// MeshCore `REQ_TYPE_GET_TELEMETRY_DATA` — a remote telemetry/location pull.
pub const REQ_TYPE_GET_TELEMETRY_DATA: u8 = 0x03;

/// MeshCore `TELEM_CHANNEL_SELF` — the LPP data channel for the node's own data.
pub const TELEM_CHANNEL_SELF: u8 = 1;

/// Cayenne LPP data-type byte for a GPS reading (`LPP_GPS`).
const LPP_GPS: u8 = 136;

/// Cayenne LPP data-type byte for a presence/boolean reading (`LPP_PRESENCE`).
///
/// Used as an honest 1-byte "no location fix" marker in the no-fix RESPONSE so
/// the plaintext still exceeds the 4-byte tag. The MeshCore companion only
/// matches a telemetry RESPONSE to its pending request when `len > 4`
/// (`onContactResponse`, companion_radio/MyMesh.cpp) — a bare tag-only frame is
/// received but silently unmatched, which would leave an enabled contact seeing
/// "Telemetry unavailable…" until the first GPS lock. This entry closes that gap.
const LPP_PRESENCE: u8 = 102;

/// Cayenne LPP data-type byte for a percentage reading (`LPP_PERCENTAGE`,
/// "1 byte 1-100% unsigned" per the Cayenne LPP spec). Carries the battery
/// charge percentage.
const LPP_PERCENTAGE: u8 = 120;

/// Cayenne LPP data-type byte for a signed 0.1 °C temperature reading
/// (`LPP_TEMPERATURE`, "2 bytes, 0.1°C signed").
///
/// This is what a *stock* MeshCore node actually puts in a base telemetry
/// reply: `onContactRequest` writes the battery voltage and then — when the
/// board has a built-in MCU sensor — the MCU temperature, both on
/// [`TELEM_CHANNEL_SELF`] (`examples/companion_radio/MyMesh.cpp`, companion
/// v1.18.0, ~lines 668-674 for the `CMD_SEND_TELEMETRY_REQ` self reply and
/// ~lines 1701-1711 for the on-air `REQ_TYPE_GET_TELEMETRY_DATA` reply).
/// MeshCadet never emits it (it sends its own battery detail on
/// `LPP_GENERIC_SENSOR`/`LPP_LUMINOSITY`), but it MUST decode it, or a stock
/// peer's reply loses every field after the voltage entry.
const LPP_TEMPERATURE: u8 = 103;

/// Cayenne LPP data-type byte for a 0.01 V/LSB unsigned voltage reading
/// (`LPP_VOLTAGE`, "2 bytes 0.01V unsigned").
///
/// The **first** entry in every stock MeshCore base telemetry reply
/// (`addVoltage(TELEM_CHANNEL_SELF, batt_mV / 1000.0f)` — see
/// [`LPP_TEMPERATURE`]'s doc for the exact call sites), which makes it the
/// single most load-bearing unknown-type byte in the protocol: a decoder that
/// aborts on it discards the entire remainder of the reply.
const LPP_VOLTAGE: u8 = 116;

/// Cayenne LPP data-type byte for a digital-input / boolean reading
/// (`LPP_DIGITAL_INPUT`, "1 byte"). Reused here — distinct from
/// [`LPP_PRESENCE`], which already means "no GPS fix" in this protocol — for
/// the battery "is-charging" boolean, so the two booleans in one RESPONSE
/// stay unambiguous by TYPE byte, not just by position.
const LPP_DIGITAL_INPUT: u8 = 0;

/// Cayenne LPP data-type byte for a generic-sensor reading (`LPP_GENERIC_SENSOR`,
/// "4 bytes"). Repurposed here — like [`LPP_DIGITAL_INPUT`] above — to carry
/// [`battery::BatteryStatus::raw_mv`](super) as a plain 4-byte big-endian
/// *exact* millivolt integer, NOT the real Cayenne LPP 0.0001-per-LSB scaled
/// fixed-point.
///
/// Chosen over `LPP_VOLTAGE` (116, 2 bytes, 10 mV/LSB resolution): `raw_mv` is
/// the ADC-calibration diagnostic field (see `battery.rs` module docs), whose
/// entire purpose is sub-10mV precision — rounding it to the nearest 10 mV
/// before it reaches the air would defeat the diagnosis it exists to support
/// (e.g. distinguishing a 4038 mV read from a 4042 mV one). The 4-byte cost is
/// affordable here: MeshCore RESPONSE frames have no tight budget comparable to
/// the request-side REQ.
const LPP_GENERIC_SENSOR: u8 = 100;

/// Cayenne LPP data-type byte for a 2-byte unsigned sensor reading
/// (`LPP_LUMINOSITY`, real meaning "1 lux/LSB unsigned"). Repurposed here —
/// like [`LPP_GENERIC_SENSOR`] above — to carry
/// [`battery::BatteryStatus::held_raw_mv`](super) (the percent basis, in
/// millivolts — `meshcadet-battery-three-state-pipeline`, 2026-08-22) as a
/// plain 2-byte big-endian *exact* integer. 2 bytes (not 4, unlike
/// `raw_mv`'s [`LPP_GENERIC_SENSOR`] entry) is enough: a resting
/// single-cell pack's mV reading is always well under `u16::MAX`, the same
/// bound [`super::battery::clamp_raw_mv_for_wire`] already assumes for the
/// wire `battery_raw_mv: u16` field.
const LPP_LUMINOSITY: u8 = 101;

/// Cayenne LPP data-type byte for a 1-byte digital-output reading
/// (`LPP_DIGITAL_OUTPUT`). Repurposed here to carry
/// [`battery::BatteryLevel`](super)'s wire ordinal (`0..=4`, the same
/// mapping as `firmware_core::ui::battery_indicator::
/// level_to_indicator_level` — `meshcadet-battery-three-state-pipeline`,
/// 2026-08-22) — distinct from [`LPP_DIGITAL_INPUT`] (already the battery
/// charging bool) and [`LPP_PRESENCE`] (already the "no GPS fix" marker),
/// so every small-int/boolean entry in one RESPONSE stays unambiguous by
/// TYPE byte, not just by position.
const LPP_DIGITAL_OUTPUT: u8 = 1;

/// Bytes of one LPP GPS entry on the wire: channel(1) + type(1) + lat(3) +
/// lon(3) + alt(3) = 11.
const LPP_GPS_ENTRY_LEN: usize = 11;

/// Bytes of one 1-byte-payload LPP entry (percentage, digital-input,
/// digital-output, presence): channel(1) + type(1) + data(1) = 3.
const LPP_BYTE_ENTRY_LEN: usize = 3;

/// Bytes of one 2-byte-payload LPP entry (the held-raw-millivolt
/// luminosity-repurposed entry): channel(1) + type(1) + data(2) = 4.
const LPP_U16_ENTRY_LEN: usize = 4;

/// Bytes of one 4-byte-payload LPP entry (the raw-millivolt generic-sensor
/// entry): channel(1) + type(1) + data(4) = 6.
const LPP_U32_ENTRY_LEN: usize = 6;

/// Cayenne LPP entry length in bytes — channel byte + type byte + the payload
/// length implied by the type — for a data-type id.
///
/// Only used to **skip** an entry: every type this decoder actually extracts a
/// value from is matched explicitly in [`decode_telemetry_response_lpp`], so the
/// `LPP_*_ENTRY_LEN` constants above stay the single statement of each known
/// entry's size.
///
/// Cayenne LPP has no per-entry length field on the wire; the payload length is
/// implied by the type byte, so tolerance for an unknown type requires a table
/// like this one. The sizes below are the Cayenne LPP 1.6.1 `LPP_*_SIZE` macros
/// (`src/CayenneLPP.h:56-82`) — the library MeshCore's telemetry writer links
/// against (`platformio.ini` `lib_deps`: `electroniccats/CayenneLPP @ 1.6.1`) —
/// and they agree with MeshCore's own `LPPData::getDataSize`
/// (`src/helpers/sensors/LPPDataHelpers.h:63-106`) except for
/// `LPP_RELATIVE_HUMIDITY`, where upstream's table claims a 2-byte payload while
/// upstream's own writer *and* reader use 1 ("1 byte, 0.5% unsigned",
/// `LPPDataHelpers.h:15`); 1 is the on-wire size, so 1 is what is used here.
fn lpp_entry_len(entry_type: u8) -> usize {
    match entry_type {
        // DIGITAL_INPUT, DIGITAL_OUTPUT, PRESENCE, RELATIVE_HUMIDITY,
        // PERCENTAGE, SWITCH.
        0 | 1 | 102 | 104 | 120 | 142 => LPP_BYTE_ENTRY_LEN,
        // ANALOG_INPUT, ANALOG_OUTPUT, LUMINOSITY, TEMPERATURE,
        // BAROMETRIC_PRESSURE, VOLTAGE, CURRENT, ALTITUDE, CONCENTRATION,
        // POWER, DIRECTION — all carry 2 payload bytes.
        2 | 3 | 101 | 103 | 115 | 116 | 117 | 121 | 125 | 128 | 132 => LPP_U16_ENTRY_LEN,
        // COLOUR: one payload byte per RGB channel.
        135 => 5,
        // GENERIC_SENSOR, FREQUENCY, DISTANCE, ENERGY, UNIXTIME.
        100 | 118 | 130 | 131 | 133 => LPP_U32_ENTRY_LEN,
        // ACCELEROMETER, GYROMETER: 2 payload bytes per axis.
        113 | 134 => 8,
        // GPS: 3B lat + 3B lon + 3B altitude.
        136 => LPP_GPS_ENTRY_LEN,
        // POLYLINE: upstream calls an 8-byte payload the minimum.
        240 => 10,
        // Same default as upstream's `LPPData::getDataSize`
        // (`LPPDataHelpers.h:105`): a type it does not know is assumed to carry
        // ONE payload byte, i.e. a 3-byte entry. That keeps the scan alive
        // (MeshCore's own reader does the same — `skipData(type)` in its
        // `default:` arm, `examples/companion_radio/ui-new/UITask.cpp:497`) at
        // the cost of possibly desynchronising for a genuinely multi-byte
        // foreign type — far better than discarding the rest of a peer's reply.
        _ => LPP_BYTE_ENTRY_LEN,
    }
}

/// Maximum RESPONSE plaintext length: tag(4) + one LPP GPS entry(11) + one
/// battery-percentage entry(3) + one charging-state entry(3) + one
/// raw-millivolt entry(6) + one held-raw-millivolt entry(4) + one level
/// entry(3) = 34. A round 40 gives headroom for a future extra LPP entry
/// without a reallocation surprise at the call site.
pub const MAX_TELEMETRY_RESPONSE_LEN: usize = 40;

/// A parsed inbound telemetry REQ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TelemetryReq {
    /// Opaque request tag; MUST be reflected verbatim in the RESPONSE so the
    /// requester can match reply to request.
    pub tag: u32,
    /// Request type byte (e.g. [`REQ_TYPE_GET_TELEMETRY_DATA`]).
    pub req_type: u8,
}

/// Parse a `PAYLOAD_TYPE_REQ` plaintext (already ECDH-decrypted).
///
/// Returns `None` if the plaintext is too short to carry a tag + req_type.
/// Callers gate on [`TelemetryReq::req_type`] to decide whether to answer.
pub fn parse_telemetry_req(plaintext: &[u8]) -> Option<TelemetryReq> {
    if plaintext.len() < 5 {
        return None;
    }
    let tag = u32::from_le_bytes([plaintext[0], plaintext[1], plaintext[2], plaintext[3]]);
    Some(TelemetryReq {
        tag,
        req_type: plaintext[4],
    })
}

/// `true` if a parsed REQ is a telemetry-data pull.
pub fn is_telemetry_req(req: &TelemetryReq) -> bool {
    req.req_type == REQ_TYPE_GET_TELEMETRY_DATA
}

/// Encode a `PAYLOAD_TYPE_RESPONSE` plaintext for a telemetry pull.
///
/// Layout: `[tag(4, LE)] [LPP entry]... `. The `tag` is reflected from the REQ.
/// When `gps` is `Some((lat_e7, lon_e7))` a single Cayenne `LPP_GPS` entry is
/// appended on channel [`TELEM_CHANNEL_SELF`]; when `None` (no fix) an
/// `LPP_PRESENCE` = 0 marker is appended instead. Either way the plaintext
/// exceeds 4 bytes, which the MeshCore companion **requires** (`len > 4`) to
/// match a RESPONSE to its pending request — a bare tag-only frame is silently
/// ignored, so the no-fix path MUST still carry an LPP entry.
///
/// When `battery` is `Some((percent, charging))`, a battery-percentage
/// (`LPP_PERCENTAGE`) entry and a charging-state (`LPP_DIGITAL_INPUT`) entry
/// are appended after the GPS/presence entry. `percent` is clamped to 0..=100.
/// `battery` is `None` only when no battery reading is available at all
/// (e.g. a bench/HIL rig with no battery ADC wired) — the field is simply
/// omitted from the wire rather than encoding a fake value.
///
/// When `raw_mv` is `Some(millivolts)`, a raw-millivolt (`LPP_GENERIC_SENSOR`)
/// entry is appended **after** the percentage/charging pair (never before —
/// an older peer's break-on-unknown-type decoder must still recover
/// percent/charging from the entries it does recognise). This is the
/// diagnostic ADC reading (`BatteryStatus::raw_mv`), which is NOT subject to
/// the charging latch that `percent` is: while charging, `raw_mv` keeps
/// tracking the live, rail-contaminated pin voltage and can read noticeably
/// higher than what `percent`/`charging` alone would suggest. Callers should
/// not treat a `raw_mv` reading above the nominal full-scale voltage while
/// `charging == true` as a decode error — see `battery.rs` module docs' "ADC
/// calibration" section for why. `raw_mv` is independent of `battery`: it may
/// be `Some` or `None` regardless of whether a percent/charging reading is
/// available, though in practice the current call site
/// (`main.rs::build_telemetry_response`) always supplies both from the same
/// `BatteryStatus` snapshot or neither.
///
/// Cayenne LPP GPS packs lat/lon as `degrees * 1e4` and altitude as
/// `metres * 1e2`, each a 24-bit big-endian two's-complement integer. MeshCadet
/// stores coordinates as e7 (1e-7 deg), so we scale down by 1000; altitude is
/// not tracked and is reported as 0. `out` must be ≥ [`MAX_TELEMETRY_RESPONSE_LEN`].
///
/// When `held_raw_mv` is `Some(millivolts)`, a held-raw-millivolt
/// (`LPP_LUMINOSITY`-repurposed) entry is appended **after** the raw-mV
/// entry (never before — same "never reorder before an existing entry"
/// rule as `raw_mv` above). This is `BatteryStatus::held_raw_mv` — the SAME
/// basis `percent`/`level` derive from, frozen while charging (unlike
/// `raw_mv`) — added 2026-08-22 (`meshcadet-battery-three-state-pipeline`)
/// as a HIL capture probe (see `battery.rs` module docs' "Three-state
/// voltage-domain bucket" section).
///
/// When `level` is `Some(ordinal)`, a battery-level (`LPP_DIGITAL_OUTPUT`-
/// repurposed) entry is appended **after** the held-raw-mV entry —
/// `ordinal` is the same `0..=4` wire encoding
/// `firmware_core::ui::battery_indicator::level_to_indicator_level`
/// produces (this crate does not depend on `firmware_core`, so the caller
/// converts the enum before calling). Added alongside `held_raw_mv`, same
/// mission.
///
/// Returns the number of bytes written.
#[allow(clippy::too_many_arguments)]
pub fn encode_telemetry_response_lpp(
    tag: u32,
    gps: Option<(i32, i32)>,
    battery: Option<(u8, bool)>,
    raw_mv: Option<u32>,
    held_raw_mv: Option<u32>,
    level: Option<u8>,
    out: &mut [u8],
) -> usize {
    let mut w = Writer { buf: out, pos: 0 };
    let t = tag.to_le_bytes();
    w.push_bytes(&t);

    match gps {
        Some((lat_e7, lon_e7)) => {
            // e7 → e4 (Cayenne GPS resolution is 1e-4 deg). Integer truncation is
            // fine: the companion's own encoder truncates identically.
            let lat_e4 = lat_e7 / 1000;
            let lon_e4 = lon_e7 / 1000;
            let alt_e2 = 0i32; // altitude not tracked

            w.push_byte(TELEM_CHANNEL_SELF);
            w.push_byte(LPP_GPS);
            w.push_i24_be(lat_e4);
            w.push_i24_be(lon_e4);
            w.push_i24_be(alt_e2);
        }
        None => {
            // No GPS fix: emit an honest "no location" presence marker so the
            // frame is still > 4 bytes and the companion registers the response
            // instead of timing out with "unavailable".
            w.push_byte(TELEM_CHANNEL_SELF);
            w.push_byte(LPP_PRESENCE);
            w.push_byte(0);
        }
    }

    if let Some((percent, charging)) = battery {
        w.push_byte(TELEM_CHANNEL_SELF);
        w.push_byte(LPP_PERCENTAGE);
        w.push_byte(percent.min(100));

        w.push_byte(TELEM_CHANNEL_SELF);
        w.push_byte(LPP_DIGITAL_INPUT);
        w.push_byte(charging as u8);
    }

    if let Some(mv) = raw_mv {
        w.push_byte(TELEM_CHANNEL_SELF);
        w.push_byte(LPP_GENERIC_SENSOR);
        w.push_u32_be(mv);
    }

    if let Some(mv) = held_raw_mv {
        w.push_byte(TELEM_CHANNEL_SELF);
        w.push_byte(LPP_LUMINOSITY);
        w.push_u16_be(mv.min(u16::MAX as u32) as u16);
    }

    if let Some(ordinal) = level {
        w.push_byte(TELEM_CHANNEL_SELF);
        w.push_byte(LPP_DIGITAL_OUTPUT);
        w.push_byte(ordinal);
    }

    w.pos
}

/// A parsed telemetry RESPONSE (host-side validation / HIL assertions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TelemetryResponseLpp {
    /// Tag reflected from the originating REQ.
    pub tag: u32,
    /// GPS fix as `(lat_e4, lon_e4)` in 1e-4 degrees, if a GPS LPP entry present.
    pub gps_e4: Option<(i32, i32)>,
    /// Battery status as `(percent, charging)`, if BOTH the percentage and
    /// charging-state LPP entries were present. A response encoded by
    /// [`encode_telemetry_response_lpp`] always carries both or neither, so a
    /// partial pair here indicates a malformed/foreign RESPONSE.
    pub battery: Option<(u8, bool)>,
    /// Raw battery millivolt reading (`BatteryStatus::raw_mv`), if the
    /// `LPP_GENERIC_SENSOR` entry was present. Independent of `battery` above
    /// — a response from a peer running before this field was added carries
    /// `battery: Some(_)` but `battery_raw_mv: None`; decoding must (and does)
    /// still recover `battery` in that case, since this entry is always
    /// appended strictly after the percentage/charging pair.
    pub battery_raw_mv: Option<u32>,
    /// Held (percent-basis) battery millivolt reading
    /// (`BatteryStatus::held_raw_mv`), if the `LPP_LUMINOSITY`-repurposed
    /// entry was present (`meshcadet-battery-three-state-pipeline`,
    /// 2026-08-22). Independent of `battery`/`battery_raw_mv` above — same
    /// forward-compat reasoning: always appended strictly after them.
    pub battery_held_raw_mv: Option<u32>,
    /// Battery level bucket wire ordinal (`0..=4`, same encoding as
    /// `firmware_core::ui::battery_indicator::level_to_indicator_level`), if
    /// the `LPP_DIGITAL_OUTPUT`-repurposed entry was present. Added
    /// alongside `battery_held_raw_mv`, same mission.
    pub battery_level: Option<u8>,
    /// Battery voltage in millivolts from a stock `LPP_VOLTAGE` entry on
    /// [`TELEM_CHANNEL_SELF`] (Cayenne resolution 0.01 V/LSB, so the wire
    /// value is scaled by 10 here). This is the battery reading every
    /// MeshCore v1.18 companion/repeater emits first in a base telemetry
    /// reply; MeshCadet's own encoder never emits it (see [`LPP_VOLTAGE`]'s
    /// doc), so a `Some(_)` here means the reply came from a stock peer.
    pub battery_voltage_mv: Option<u32>,
    /// MCU temperature in tenths of a degree Celsius from a stock
    /// `LPP_TEMPERATURE` entry on [`TELEM_CHANNEL_SELF`] (Cayenne resolution
    /// 0.1 °C/LSB, signed) — the same `raw` the wire carried, left unscaled so
    /// no precision is invented. `None` when the peer board has no built-in
    /// temperature sensor (upstream skips the entry when it reads `NaN`).
    pub temperature_c_x10: Option<i16>,
}

/// Parse a `PAYLOAD_TYPE_RESPONSE` plaintext produced by
/// [`encode_telemetry_response_lpp`] — or by a stock MeshCore node.
///
/// Returns `None` if it is too short to hold a tag. Scans **every** LPP entry
/// in the body (GPS, stock voltage/temperature, presence, percentage,
/// digital-input, generic-sensor raw-mv, luminosity-repurposed held-raw-mv,
/// digital-output-repurposed level).
///
/// Entries whose type this decoder does not extract a value from are **skipped
/// by their Cayenne-implied entry length** ([`lpp_entry_len`]) and the scan
/// continues — they are never rendered as a value, and they never end the
/// scan. This matches upstream's own reader, which forwards over what it does
/// not display (`default: r.skipData(type)`, `examples/companion_radio/
/// ui-new/UITask.cpp:497`, companion v1.18.0), so a peer running extended or
/// future telemetry does not silently truncate the rest of the reply. The
/// previous abort-on-first-unknown behaviour was not upstream-compatible: a
/// stock v1.18 base reply *leads* with `LPP_VOLTAGE`, so it decoded to an
/// all-`None` struct.
///
/// A truncated trailing entry (fewer bytes left than its type declares) ends
/// the scan without reading past the buffer.
pub fn decode_telemetry_response_lpp(plaintext: &[u8]) -> Option<TelemetryResponseLpp> {
    if plaintext.len() < 4 {
        return None;
    }
    let tag = u32::from_le_bytes([plaintext[0], plaintext[1], plaintext[2], plaintext[3]]);
    let mut gps_e4 = None;
    let mut percent: Option<u8> = None;
    let mut charging: Option<bool> = None;
    let mut raw_mv: Option<u32> = None;
    let mut held_raw_mv: Option<u32> = None;
    let mut level: Option<u8> = None;
    let mut voltage_mv: Option<u32> = None;
    let mut temperature_c_x10: Option<i16> = None;
    let mut body = &plaintext[4..];

    while body.len() >= 2 {
        let channel = body[0];
        let entry_type = body[1];
        // Channel byte + type byte + the payload length implied by the type.
        let entry_len = lpp_entry_len(entry_type);
        if body.len() < entry_len {
            break; // truncated trailing entry — never read past the buffer.
        }
        match entry_type {
            LPP_GPS => {
                let lat_e4 = read_i24_be(&body[2..5]);
                let lon_e4 = read_i24_be(&body[5..8]);
                gps_e4 = Some((lat_e4, lon_e4));
            }
            // Stock MeshCore base telemetry. Gated on the self channel because
            // upstream also emits VOLTAGE/TEMPERATURE for *external* sensors on
            // channels 2+ (`EnvironmentSensorManager::querySensors`); those are
            // not this node's base readings and must not overwrite them.
            LPP_VOLTAGE if channel == TELEM_CHANNEL_SELF => {
                // 0.01 V/LSB -> millivolts.
                voltage_mv = Some(read_u16_be(&body[2..4]) as u32 * 10);
            }
            LPP_TEMPERATURE if channel == TELEM_CHANNEL_SELF => {
                // 0.1 °C/LSB, two's complement.
                temperature_c_x10 = Some(read_u16_be(&body[2..4]) as i16);
            }
            LPP_PERCENTAGE => {
                percent = Some(body[2]);
            }
            LPP_DIGITAL_INPUT => {
                charging = Some(body[2] != 0);
            }
            LPP_PRESENCE => {
                // "No GPS fix" marker — no data to extract, just consume it.
            }
            LPP_GENERIC_SENSOR => {
                raw_mv = Some(read_u32_be(&body[2..6]));
            }
            LPP_LUMINOSITY => {
                held_raw_mv = Some(read_u16_be(&body[2..4]) as u32);
            }
            LPP_DIGITAL_OUTPUT => {
                level = Some(body[2]);
            }
            // Any other type (stock humidity/pressure/altitude/current/power/
            // … , an env sensor on channel 2+, or a type nobody has defined
            // yet): skip exactly its declared payload and keep scanning. No
            // placeholder value is recorded — this struct is a typed field
            // set, not a UI list, so silently skipping is the only outcome
            // that cannot invent a reading.
            _ => {}
        }
        body = &body[entry_len..];
    }

    let battery = match (percent, charging) {
        (Some(p), Some(c)) => Some((p, c)),
        _ => None,
    };
    Some(TelemetryResponseLpp {
        tag,
        gps_e4,
        battery,
        battery_raw_mv: raw_mv,
        battery_held_raw_mv: held_raw_mv,
        battery_level: level,
        battery_voltage_mv: voltage_mv,
        temperature_c_x10,
    })
}

/// Read a 24-bit big-endian two's-complement integer from a 3-byte slice.
fn read_i24_be(b: &[u8]) -> i32 {
    let raw = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
    // Sign-extend from 24 to 32 bits.
    if raw & 0x0080_0000 != 0 {
        (raw | 0xFF00_0000) as i32
    } else {
        raw as i32
    }
}

/// Read a 32-bit big-endian unsigned integer from a 4-byte slice (the
/// raw-millivolt `LPP_GENERIC_SENSOR` entry — plain, unscaled).
fn read_u32_be(b: &[u8]) -> u32 {
    ((b[0] as u32) << 24) | ((b[1] as u32) << 16) | ((b[2] as u32) << 8) | (b[3] as u32)
}

/// Read a 16-bit big-endian unsigned integer from a 2-byte slice (the
/// held-raw-millivolt `LPP_LUMINOSITY`-repurposed entry — plain, unscaled).
fn read_u16_be(b: &[u8]) -> u16 {
    ((b[0] as u16) << 8) | (b[1] as u16)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_telemetry_request ─────────────────────────────────────────────────

    #[test]
    fn request_magic_detected() {
        assert!(is_telemetry_request(b"?loc"), "exact magic must match");
        assert!(
            is_telemetry_request(b"?loc verbose"),
            "prefix match must pass"
        );
    }

    #[test]
    fn non_request_not_detected() {
        assert!(!is_telemetry_request(b"hello"));
        assert!(!is_telemetry_request(b""));
        assert!(!is_telemetry_request(
            b"loc:lat=1.0000000,lon=2.0000000,age=0s"
        )); // response
        assert!(!is_telemetry_request(b"?lox")); // wrong magic
        assert!(!is_telemetry_request(b"   ")); // whitespace only
        assert!(!is_telemetry_request(b"x?loc")); // magic not at (trimmed) start
    }

    #[test]
    fn request_tolerates_leading_whitespace() {
        // A touch keyboard / paste can prepend whitespace — must still detect.
        assert!(is_telemetry_request(b" ?loc"));
        assert!(is_telemetry_request(b"\t?loc"));
        assert!(is_telemetry_request(b"  ?loc verbose"));
    }

    #[test]
    fn request_is_case_insensitive() {
        // Phone keyboards auto-capitalize — "?Loc" must still detect (the exact
        // silent-drop this guards against).
        assert!(is_telemetry_request(b"?Loc"));
        assert!(is_telemetry_request(b"?LOC"));
        assert!(is_telemetry_request(b"?lOc"));
        assert!(is_telemetry_request(b"  ?LOC")); // both: trimmed + case-insensitive
    }

    // ── encode / decode roundtrip — acceptance: correct age/timestamp ────────

    #[test]
    fn encode_decode_roundtrip_positive() {
        // Munich-ish: 48.1173°N, 11.5169°E, 42 s old
        let lat_e7 = 481_173_000i32;
        let lon_e7 = 115_169_000i32;
        let age = 42u32;

        let mut buf = [0u8; MAX_RESPONSE_LEN];
        let n = encode_telemetry_response(lat_e7, lon_e7, age, &mut buf);
        let resp = decode_telemetry_response(&buf[..n]).expect("must parse positive coords");
        assert_eq!(resp.lat_e7, lat_e7, "lat roundtrip");
        assert_eq!(resp.lon_e7, lon_e7, "lon roundtrip");
        assert_eq!(resp.age_secs, age, "age roundtrip");
    }

    #[test]
    fn encode_decode_roundtrip_negative() {
        // Southern / Western hemisphere
        let lat_e7 = -338_688_000i32;
        let lon_e7 = -1_512_093_000i32;
        let age = 120u32;

        let mut buf = [0u8; MAX_RESPONSE_LEN];
        let n = encode_telemetry_response(lat_e7, lon_e7, age, &mut buf);
        let resp = decode_telemetry_response(&buf[..n]).expect("must parse negative coords");
        assert_eq!(resp.lat_e7, lat_e7);
        assert_eq!(resp.lon_e7, lon_e7);
        assert_eq!(resp.age_secs, age);
    }

    #[test]
    fn encode_decode_zero_coords() {
        let mut buf = [0u8; MAX_RESPONSE_LEN];
        let n = encode_telemetry_response(0, 0, 0, &mut buf);
        let resp = decode_telemetry_response(&buf[..n]).unwrap();
        assert_eq!(resp.lat_e7, 0);
        assert_eq!(resp.lon_e7, 0);
        assert_eq!(resp.age_secs, 0);
    }

    #[test]
    fn response_contains_age_field() {
        // Acceptance criterion: response carries staleness timestamp.
        let mut buf = [0u8; MAX_RESPONSE_LEN];
        let n = encode_telemetry_response(0, 0, 99, &mut buf);
        let text = core::str::from_utf8(&buf[..n]).expect("must be UTF-8");
        assert!(
            text.contains("age="),
            "response must have age= field: {}",
            text
        );
        assert!(
            text.contains("99s"),
            "response must carry the age value: {}",
            text
        );
    }

    #[test]
    fn age_large_value_roundtrips() {
        // Non-trivial age to catch overflow bugs.
        let mut buf = [0u8; MAX_RESPONSE_LEN];
        let n = encode_telemetry_response(0, 0, 7200, &mut buf);
        let resp = decode_telemetry_response(&buf[..n]).unwrap();
        assert_eq!(resp.age_secs, 7200);
    }

    #[test]
    fn response_text_fits_in_max_response_len() {
        // Worst-case coordinate + max u32 age
        let mut buf = [0u8; MAX_RESPONSE_LEN];
        let n = encode_telemetry_response(-900_000_000, -1_800_000_000, u32::MAX, &mut buf);
        assert!(
            n <= MAX_RESPONSE_LEN,
            "response must fit in MAX_RESPONSE_LEN: {} > {}",
            n,
            MAX_RESPONSE_LEN
        );
    }

    // ── no-fix encoding ──────────────────────────────────────────────────────

    #[test]
    fn no_fix_response_detected() {
        let mut buf = [0u8; 16];
        let n = encode_no_fix_response(&mut buf);
        assert!(is_no_fix_response(&buf[..n]));
    }

    #[test]
    fn coord_response_is_not_no_fix() {
        let mut buf = [0u8; MAX_RESPONSE_LEN];
        let n = encode_telemetry_response(0, 0, 0, &mut buf);
        assert!(!is_no_fix_response(&buf[..n]));
    }

    #[test]
    fn decode_rejects_no_fix_as_coord_response() {
        // loc:nofix should not decode as a TelemetryResponse with coordinates.
        assert!(decode_telemetry_response(b"loc:nofix").is_none());
    }

    // ── reject malformed input ───────────────────────────────────────────────

    #[test]
    fn decode_rejects_malformed() {
        assert!(decode_telemetry_response(b"").is_none());
        assert!(decode_telemetry_response(b"?loc").is_none()); // request, not response
        assert!(decode_telemetry_response(b"loc:lat=abc,lon=1.0000000,age=0s").is_none()); // non-numeric lat
        assert!(decode_telemetry_response(b"loc:lat=1.00000,lon=0.0000000,age=0s").is_none()); // 5 dp, not 7
        assert!(decode_telemetry_response(b"loc:lat=1.0000000,lon=0.0000000,age=xs").is_none());
        // non-numeric age
    }

    // ── coord encoder precision ──────────────────────────────────────────────

    #[test]
    fn coord_zero_pad_fractional() {
        // lat_e7 = 10_000 → 0.0010000 degrees — the fraction needs leading zeros.
        let lat_e7 = 10_000i32;
        let mut buf = [0u8; MAX_RESPONSE_LEN];
        let n = encode_telemetry_response(lat_e7, 0, 0, &mut buf);
        let text = core::str::from_utf8(&buf[..n]).unwrap();
        // Should contain "lat=0.0010000"
        assert!(
            text.contains("lat=0.0010000"),
            "zero padding wrong: {}",
            text
        );
    }

    #[test]
    fn coord_max_longitude_roundtrips() {
        // 179.9999999°E (≈ International Date Line)
        let lon_e7 = 1_799_999_999i32;
        let mut buf = [0u8; MAX_RESPONSE_LEN];
        let n = encode_telemetry_response(0, lon_e7, 0, &mut buf);
        let resp = decode_telemetry_response(&buf[..n]).unwrap();
        assert_eq!(resp.lon_e7, lon_e7);
    }

    // ── MeshCore-native REQ / RESPONSE (companion-app path) ───────────────────

    #[test]
    fn parse_telemetry_req_extracts_tag_and_type() {
        // [tag(4 LE)=0xAABBCCDD] [req_type=0x03] [reserved/random...]
        let pt = [
            0xDD, 0xCC, 0xBB, 0xAA, 0x03, 0x00, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44,
        ];
        let req = parse_telemetry_req(&pt).expect("valid REQ must parse");
        assert_eq!(req.tag, 0xAABB_CCDD);
        assert_eq!(req.req_type, REQ_TYPE_GET_TELEMETRY_DATA);
        assert!(is_telemetry_req(&req));
    }

    #[test]
    fn parse_telemetry_req_rejects_short() {
        assert!(parse_telemetry_req(b"").is_none());
        assert!(parse_telemetry_req(&[0, 1, 2, 3]).is_none()); // tag but no req_type
    }

    #[test]
    fn parse_telemetry_req_non_telemetry_type_not_a_pull() {
        // req_type 0x01 (GET_STATUS) is a REQ but not a telemetry pull.
        let pt = [0x01, 0x00, 0x00, 0x00, 0x01, 0x00];
        let req = parse_telemetry_req(&pt).unwrap();
        assert!(!is_telemetry_req(&req));
    }

    #[test]
    fn response_lpp_with_gps_roundtrips() {
        // Munich-ish: 48.1173000°N, 11.5169000°E → e4 = 481173, 115169.
        let tag = 0x1234_5678u32;
        let lat_e7 = 481_173_000i32;
        let lon_e7 = 115_169_000i32;
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(
            tag,
            Some((lat_e7, lon_e7)),
            None,
            None,
            None,
            None,
            &mut buf,
        );
        assert!(n <= MAX_TELEMETRY_RESPONSE_LEN);
        let resp = decode_telemetry_response_lpp(&buf[..n]).expect("must parse");
        assert_eq!(
            resp.tag, tag,
            "tag must reflect so the companion matches reply→request"
        );
        assert_eq!(resp.gps_e4, Some((lat_e7 / 1000, lon_e7 / 1000)));
        assert_eq!(resp.battery, None, "no battery reading was supplied");
        assert_eq!(resp.battery_raw_mv, None, "no raw_mv reading was supplied");
        assert_eq!(resp.battery_held_raw_mv, None);
        assert_eq!(resp.battery_level, None);
    }

    #[test]
    fn response_lpp_negative_coords_roundtrip() {
        // Southern/Western hemisphere → negative 24-bit two's complement.
        let tag = 42u32;
        let lat_e7 = -338_688_000i32; // -33.8688°
        let lon_e7 = -151_209_300i32; // -15.12093°
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(
            tag,
            Some((lat_e7, lon_e7)),
            None,
            None,
            None,
            None,
            &mut buf,
        );
        let resp = decode_telemetry_response_lpp(&buf[..n]).unwrap();
        assert_eq!(resp.gps_e4, Some((lat_e7 / 1000, lon_e7 / 1000)));
    }

    /// REGRESSION (post-green review): the companion matches a telemetry
    /// RESPONSE only when `len > 4` (`onContactResponse`). A no-fix response must
    /// therefore still carry an LPP entry — a bare 4-byte tag would be silently
    /// unmatched, leaving an enabled contact stuck on "unavailable" until first
    /// GPS lock. Assert BOTH the fix and no-fix responses exceed 4 bytes.
    #[test]
    fn response_lpp_no_fix_still_exceeds_tag_so_companion_matches() {
        let tag = 0xCAFEu32;
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(tag, None, None, None, None, None, &mut buf);
        assert!(
            n > 4,
            "no-fix response must exceed the 4-byte tag (got {n}) or the companion ignores it"
        );
        let resp = decode_telemetry_response_lpp(&buf[..n]).unwrap();
        assert_eq!(
            resp.tag, tag,
            "tag must still reflect for the companion to match"
        );
        assert_eq!(
            resp.gps_e4, None,
            "no-fix response carries no GPS coordinates"
        );
    }

    #[test]
    fn response_lpp_fix_exceeds_tag() {
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(
            1,
            Some((481_173_000, 115_169_000)),
            None,
            None,
            None,
            None,
            &mut buf,
        );
        assert!(n > 4, "fix response must exceed the 4-byte tag (got {n})");
    }

    // ── Battery LPP entries ─────

    #[test]
    fn response_lpp_battery_roundtrips_alongside_gps() {
        let tag = 0x7777_7777u32;
        let lat_e7 = 481_173_000i32;
        let lon_e7 = 115_169_000i32;
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(
            tag,
            Some((lat_e7, lon_e7)),
            Some((82, true)),
            None,
            None,
            None,
            &mut buf,
        );
        assert!(
            n <= MAX_TELEMETRY_RESPONSE_LEN,
            "must fit the documented ceiling: {n}"
        );
        let resp = decode_telemetry_response_lpp(&buf[..n]).expect("must parse");
        assert_eq!(resp.gps_e4, Some((lat_e7 / 1000, lon_e7 / 1000)));
        assert_eq!(resp.battery, Some((82, true)));
    }

    #[test]
    fn response_lpp_battery_roundtrips_alongside_no_fix() {
        // No-fix (presence marker) + battery must coexist without the scanner
        // mis-parsing the presence entry as the start of a battery entry.
        let tag = 0x1u32;
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n =
            encode_telemetry_response_lpp(tag, None, Some((14, false)), None, None, None, &mut buf);
        let resp = decode_telemetry_response_lpp(&buf[..n]).unwrap();
        assert_eq!(resp.gps_e4, None);
        assert_eq!(
            resp.battery,
            Some((14, false)),
            "not-charging low battery must decode intact"
        );
    }

    #[test]
    fn response_lpp_battery_not_charging_roundtrips() {
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n =
            encode_telemetry_response_lpp(1, None, Some((50, false)), None, None, None, &mut buf);
        let resp = decode_telemetry_response_lpp(&buf[..n]).unwrap();
        assert_eq!(resp.battery, Some((50, false)));
    }

    #[test]
    fn response_lpp_battery_percent_clamped_at_100() {
        // Defensive: a caller passing an out-of-range percent must not corrupt
        // the wire format with a value a real receiver would reject/misread.
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n =
            encode_telemetry_response_lpp(1, None, Some((250, true)), None, None, None, &mut buf);
        let resp = decode_telemetry_response_lpp(&buf[..n]).unwrap();
        assert_eq!(resp.battery, Some((100, true)), "percent must clamp to 100");
    }

    #[test]
    fn response_lpp_no_battery_reading_omits_entries() {
        // `None` means "no reading available" (e.g. a bench rig with no
        // battery ADC wired) — must not fabricate a battery reading on the wire.
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(1, Some((0, 0)), None, None, None, None, &mut buf);
        let resp = decode_telemetry_response_lpp(&buf[..n]).unwrap();
        assert_eq!(resp.battery, None);
    }

    #[test]
    fn response_lpp_battery_fits_within_max_len() {
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(
            u32::MAX,
            Some((i32::MIN, i32::MAX)),
            Some((100, true)),
            Some(u32::MAX),
            Some(u32::MAX),
            Some(4),
            &mut buf,
        );
        assert!(
            n <= MAX_TELEMETRY_RESPONSE_LEN,
            "worst case (max tag + GPS + battery + raw_mv + held_raw_mv + level) must fit: \
             {n} > {MAX_TELEMETRY_RESPONSE_LEN}"
        );
    }

    // ── raw_mv LPP entry ──────────────────────────────────────────────────

    #[test]
    fn response_lpp_raw_mv_roundtrips_alongside_battery_and_gps() {
        let tag = 0xABCDu32;
        let lat_e7 = 481_173_000i32;
        let lon_e7 = 115_169_000i32;
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(
            tag,
            Some((lat_e7, lon_e7)),
            Some((82, true)),
            Some(4_142),
            None,
            None,
            &mut buf,
        );
        assert!(n <= MAX_TELEMETRY_RESPONSE_LEN, "must fit ceiling: {n}");
        let resp = decode_telemetry_response_lpp(&buf[..n]).expect("must parse");
        assert_eq!(resp.gps_e4, Some((lat_e7 / 1000, lon_e7 / 1000)));
        assert_eq!(resp.battery, Some((82, true)));
        assert_eq!(
            resp.battery_raw_mv,
            Some(4_142),
            "raw_mv must round-trip exactly (no 10 mV rounding)"
        );
    }

    #[test]
    fn response_lpp_raw_mv_is_exact_not_rounded_to_10mv() {
        // The whole point of choosing LPP_GENERIC_SENSOR over LPP_VOLTAGE: a
        // value that is NOT a multiple of 10 mV must still round-trip exactly.
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(1, None, None, Some(4_037), None, None, &mut buf);
        let resp = decode_telemetry_response_lpp(&buf[..n]).unwrap();
        assert_eq!(resp.battery_raw_mv, Some(4_037));
    }

    #[test]
    fn response_lpp_raw_mv_absent_when_not_supplied() {
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n =
            encode_telemetry_response_lpp(1, None, Some((50, false)), None, None, None, &mut buf);
        let resp = decode_telemetry_response_lpp(&buf[..n]).unwrap();
        assert_eq!(
            resp.battery_raw_mv, None,
            "must not fabricate a raw_mv reading when None was supplied"
        );
    }

    /// Wire-golden byte-layout test: the raw_mv entry MUST be appended strictly
    /// after the percentage/charging pair (never inserted before), so a peer
    /// running before this change — whose decoder does not recognise
    /// `LPP_GENERIC_SENSOR` and breaks on the first unknown type — still
    /// recovers `percent`/`charging` intact from the bytes preceding it.
    #[test]
    fn response_lpp_raw_mv_entry_appended_after_battery_pair() {
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(
            1,
            None,
            Some((82, true)),
            Some(4_142),
            None,
            None,
            &mut buf,
        );
        // skip the 4-byte tag + the 3-byte no-fix presence marker (gps=None).
        let body = &buf[4 + LPP_BYTE_ENTRY_LEN..n];

        // [channel, LPP_PERCENTAGE, 82] [channel, LPP_DIGITAL_INPUT, 1]
        // [channel, LPP_GENERIC_SENSOR, mv(4 BE)]
        assert_eq!(&body[0..3], &[TELEM_CHANNEL_SELF, LPP_PERCENTAGE, 82]);
        assert_eq!(&body[3..6], &[TELEM_CHANNEL_SELF, LPP_DIGITAL_INPUT, 1]);
        assert_eq!(
            &body[6..12],
            &[
                TELEM_CHANNEL_SELF,
                LPP_GENERIC_SENSOR,
                0x00,
                0x00,
                0x10,
                0x2e, // 4142 = 0x0000102e, big-endian
            ]
        );
        assert_eq!(body.len(), 12, "no trailing bytes beyond the three entries");

        // A decoder that stops at the first unrecognised entry type (the
        // pre-this-change behaviour) still recovers percent/charging: replay
        // the scan by hand up to (not including) the LPP_GENERIC_SENSOR byte.
        assert_eq!(body[1], LPP_PERCENTAGE);
        assert_eq!(body[4], LPP_DIGITAL_INPUT);
        assert_eq!(body[7], LPP_GENERIC_SENSOR);
    }

    // ── held_raw_mv / level LPP entries (meshcadet-battery-three-state-pipeline) ──

    #[test]
    fn response_lpp_held_raw_mv_and_level_roundtrip_alongside_everything_else() {
        let tag = 0xBEEFu32;
        let lat_e7 = 481_173_000i32;
        let lon_e7 = 115_169_000i32;
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(
            tag,
            Some((lat_e7, lon_e7)),
            Some((82, true)),
            Some(4_888),
            Some(3_775),
            Some(1), // Charging ordinal
            &mut buf,
        );
        assert!(n <= MAX_TELEMETRY_RESPONSE_LEN, "must fit ceiling: {n}");
        let resp = decode_telemetry_response_lpp(&buf[..n]).expect("must parse");
        assert_eq!(resp.battery, Some((82, true)));
        assert_eq!(resp.battery_raw_mv, Some(4_888));
        assert_eq!(
            resp.battery_held_raw_mv,
            Some(3_775),
            "held_raw_mv must round-trip exactly"
        );
        assert_eq!(resp.battery_level, Some(1));
    }

    #[test]
    fn response_lpp_held_raw_mv_and_level_absent_when_not_supplied() {
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n =
            encode_telemetry_response_lpp(1, None, Some((50, false)), None, None, None, &mut buf);
        let resp = decode_telemetry_response_lpp(&buf[..n]).unwrap();
        assert_eq!(resp.battery_held_raw_mv, None);
        assert_eq!(resp.battery_level, None);
    }

    /// Wire-golden byte-layout test: held_raw_mv/level MUST be appended
    /// strictly after raw_mv (never inserted before it), so an older peer's
    /// break-on-unknown-type decoder still recovers percent/charging/raw_mv.
    #[test]
    fn response_lpp_held_raw_mv_and_level_appended_after_raw_mv() {
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(
            1,
            None,
            Some((82, true)),
            Some(4_142),
            Some(3_624),
            Some(2), // Low ordinal
            &mut buf,
        );
        // skip tag(4) + no-fix presence(3) + percentage(3) + digital-input(3) + raw_mv(6).
        let body = &buf[4
            + LPP_BYTE_ENTRY_LEN
            + LPP_BYTE_ENTRY_LEN
            + LPP_BYTE_ENTRY_LEN
            + LPP_U32_ENTRY_LEN..n];

        // [channel, LPP_LUMINOSITY, mv(2 BE)] [channel, LPP_DIGITAL_OUTPUT, ordinal]
        assert_eq!(
            &body[0..4],
            &[TELEM_CHANNEL_SELF, LPP_LUMINOSITY, 0x0e, 0x28] // 3624 = 0x0e28, big-endian
        );
        assert_eq!(&body[4..7], &[TELEM_CHANNEL_SELF, LPP_DIGITAL_OUTPUT, 2]);
        assert_eq!(body.len(), 7, "no trailing bytes beyond the two entries");
    }

    /// REGRESSION: the full native
    /// companion exchange, byte-for-byte. A stock MeshCore companion sends a
    /// `PAYLOAD_TYPE_REQ` (NOT the bespoke `?loc` text). MeshCadet must decrypt
    /// it, recognise the telemetry req_type, and answer with a
    /// `PAYLOAD_TYPE_RESPONSE` carrying the reflected tag + GPS — decryptable and
    /// tag-matchable by the requester. This is the seam whose absence let three
    /// `?loc`-only fixes land green while the hardware never moved.
    #[test]
    fn native_companion_req_to_response_end_to_end() {
        use crate::{decode_dm_payload, encode_dm_payload, Identity};

        let companion = Identity::from_seed([0xC0u8; 32]); // requester
        let meshcadet = Identity::from_seed([0x3Du8; 32]); // responder

        let shared_c = companion.ecdh_shared_secret(&meshcadet.pubkey);
        let shared_m = meshcadet.ecdh_shared_secret(&companion.pubkey);

        // 1. Companion builds a REQ plaintext: [tag][req_type][reserved/random].
        let tag = 0x0BAD_F00Du32;
        let mut req_pt = [0u8; 13];
        req_pt[..4].copy_from_slice(&tag.to_le_bytes());
        req_pt[4] = REQ_TYPE_GET_TELEMETRY_DATA;
        // bytes 5..9 reserved (0), 9..13 random blob — irrelevant to the responder.

        // 2. Companion encrypts it into a PAYLOAD_TYPE_REQ datagram.
        let mut req_dm = [0u8; 128];
        let req_len = encode_dm_payload(
            &shared_c,
            meshcadet.pub_hash(),
            companion.pub_hash(),
            &req_pt,
            &mut req_dm,
        );

        // 3. MeshCadet decrypts and parses the REQ.
        let mut dec = [0u8; 128];
        let (dest, _src, pt_len) =
            decode_dm_payload(&shared_m, &req_dm[..req_len], &mut dec).expect("REQ must decrypt");
        assert_eq!(dest, meshcadet.pub_hash());
        let req = parse_telemetry_req(&dec[..pt_len]).expect("REQ must parse");
        assert!(
            is_telemetry_req(&req),
            "must be recognised as a telemetry pull"
        );
        assert_eq!(req.tag, tag);

        // 4. MeshCadet builds the RESPONSE plaintext (reflect tag + GPS fix + battery).
        let lat_e7 = 481_173_000i32;
        let lon_e7 = 115_169_000i32;
        let mut resp_pt = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let resp_pt_len = encode_telemetry_response_lpp(
            req.tag,
            Some((lat_e7, lon_e7)),
            Some((77, false)),
            Some(3_931),
            Some(3_900),
            Some(3), // Partial ordinal
            &mut resp_pt,
        );

        // 5. MeshCadet encrypts it into a PAYLOAD_TYPE_RESPONSE datagram.
        let mut resp_dm = [0u8; 128];
        let resp_len = encode_dm_payload(
            &shared_m,
            companion.pub_hash(),
            meshcadet.pub_hash(),
            &resp_pt[..resp_pt_len],
            &mut resp_dm,
        );

        // 6. Companion decrypts the RESPONSE and matches it by tag.
        let mut rdec = [0u8; 128];
        let (rdest, _rsrc, rpt_len) = decode_dm_payload(&shared_c, &resp_dm[..resp_len], &mut rdec)
            .expect("RESPONSE must decrypt");
        assert_eq!(rdest, companion.pub_hash());
        let resp = decode_telemetry_response_lpp(&rdec[..rpt_len]).expect("RESPONSE must parse");
        assert_eq!(
            resp.tag, tag,
            "tag must round-trip for the companion to match"
        );
        assert_eq!(resp.gps_e4, Some((lat_e7 / 1000, lon_e7 / 1000)));
        assert_eq!(
            resp.battery,
            Some((77, false)),
            "battery status must round-trip end-to-end too"
        );
        assert_eq!(
            resp.battery_raw_mv,
            Some(3_931),
            "raw_mv must round-trip end-to-end too"
        );
        assert_eq!(resp.battery_held_raw_mv, Some(3_900));
        assert_eq!(resp.battery_level, Some(3));
    }

    // ── stock MeshCore base telemetry (LPP_VOLTAGE / LPP_TEMPERATURE) ─────────
    //
    // Byte sequences below are derived from UPSTREAM SOURCE, not captured from a
    // device: the entry order and payload encoding come from MeshCore
    // companion-v1.18.0 (commit 81dbae5) — `examples/companion_radio/MyMesh.cpp`
    // `onContactRequest` writes `addVoltage(TELEM_CHANNEL_SELF, …)` FIRST, then
    // `sensors.querySensors(...)` (`GPS` on channel 1,
    // `src/helpers/sensors/EnvironmentSensorManager.cpp:688`), then
    // `addTemperature(TELEM_CHANNEL_SELF, …)` — with payload sizes from
    // `electroniccats/CayenneLPP @ 1.6.1` `src/CayenneLPP.h:56-82`
    // (`platformio.ini` `lib_deps`).

    /// A stock v1.18 base telemetry reply — `LPP_VOLTAGE` first (the entry that
    /// used to abort decoding), then `LPP_GPS`, then `LPP_TEMPERATURE` — must
    /// decode all three. Regression test for "stock reply decodes to all-`None`".
    #[test]
    fn decode_stock_v118_base_reply_decodes_voltage_temperature_and_gps() {
        let tag = 0x0BAD_F00Du32;
        let mut buf = [0u8; 32];
        buf[..4].copy_from_slice(&tag.to_le_bytes());

        // [ch=1, LPP_VOLTAGE(116), 0x01A3] -> 419 * 0.01 V = 4.19 V.
        buf[4..8].copy_from_slice(&[TELEM_CHANNEL_SELF, 116, 0x01, 0xA3]);
        // [ch=1, LPP_GPS(136), lat=481173 (0x075795), lon=115169 (0x01C1E1),
        //  alt=0] — 3-byte big-endian two's complement each, 1e-4 deg / 0.01 m.
        buf[8..19].copy_from_slice(&[
            TELEM_CHANNEL_SELF,
            136,
            0x07,
            0x57,
            0x95, // 48.1173 deg
            0x01,
            0xC1,
            0xE1, // 11.5169 deg
            0x00,
            0x00,
            0x00, // 0 m altitude
        ]);
        // [ch=1, LPP_TEMPERATURE(103), 0x013B] -> 315 = 31.5 C.
        buf[19..23].copy_from_slice(&[TELEM_CHANNEL_SELF, 103, 0x01, 0x3B]);

        let resp = decode_telemetry_response_lpp(&buf[..23]).expect("must parse");
        assert_eq!(resp.tag, tag);
        assert_eq!(
            resp.battery_voltage_mv,
            Some(4_190),
            "LPP_VOLTAGE must be decoded into millivolts"
        );
        assert_eq!(
            resp.gps_e4,
            Some((481_173, 115_169)),
            "GPS after LPP_VOLTAGE must not be truncated away"
        );
        assert_eq!(
            resp.temperature_c_x10,
            Some(315),
            "LPP_TEMPERATURE after LPP_VOLTAGE must not be truncated away"
        );
        // Nothing else in this reply — the encoder-only fields stay empty.
        assert_eq!(resp.battery, None);
        assert_eq!(resp.battery_raw_mv, None);
        assert_eq!(resp.battery_held_raw_mv, None);
        assert_eq!(resp.battery_level, None);
    }

    /// Sub-zero `LPP_TEMPERATURE` is two's complement (`isSigned(LPP_TEMPERATURE)`
    /// is true, `src/helpers/sensors/LPPDataHelpers.h:119-123`).
    #[test]
    fn decode_stock_temperature_is_signed() {
        let mut buf = [0u8; 16];
        buf[..4].copy_from_slice(&1u32.to_le_bytes());
        // -5.0 C -> raw -50 -> 0xFFCE big-endian.
        buf[4..8].copy_from_slice(&[TELEM_CHANNEL_SELF, 103, 0xFF, 0xCE]);
        let resp = decode_telemetry_response_lpp(&buf[..8]).expect("must parse");
        assert_eq!(resp.temperature_c_x10, Some(-50));
    }

    /// Every Cayenne type this decoder skips only implicitly — stock environment
    /// sensors (`addRelativeHumidity`, `addBarometricPressure`, `addAltitude`,
    /// `addCurrent`, `addPower`, … `EnvironmentSensorManager.cpp`) — must be
    /// stepped over by its true on-wire size so the fields after them survive.
    /// `LPP_RELATIVE_HUMIDITY` is deliberately included: MeshCore's own
    /// `LPPData::getDataSize` claims 2 payload bytes for it
    /// (`LPPDataHelpers.h:91-105`) while its writer and reader use 1
    /// ("1 byte, 0.5% unsigned", `LPPDataHelpers.h:15`), so a 1-byte skip here is
    /// what keeps this test's trailing GPS entry aligned.
    #[test]
    fn decode_skips_undecoded_cayenne_types_and_keeps_later_fields() {
        let mut buf = [0u8; 32];
        buf[..4].copy_from_slice(&7u32.to_le_bytes());
        let mut i = 4;
        // [ch=1, LPP_VOLTAGE, 0x01A3]
        buf[i..i + 4].copy_from_slice(&[TELEM_CHANNEL_SELF, 116, 0x01, 0xA3]);
        i += 4;
        // [ch=2, LPP_RELATIVE_HUMIDITY(104), 122] -> 61.0 % (1-byte payload).
        buf[i..i + 3].copy_from_slice(&[2, 104, 122]);
        i += 3;
        // [ch=2, LPP_BAROMETRIC_PRESSURE(115), 0x27C7] -> 1018.3 hPa (2 bytes).
        buf[i..i + 4].copy_from_slice(&[2, 115, 0x27, 0xC7]);
        i += 4;
        // [ch=2, LPP_CURRENT(117), 0x01F4] -> 0.500 A (2 bytes).
        buf[i..i + 4].copy_from_slice(&[2, 117, 0x01, 0xF4]);
        i += 4;
        // [ch=1, LPP_TEMPERATURE(103), 0x013B]
        buf[i..i + 4].copy_from_slice(&[TELEM_CHANNEL_SELF, 103, 0x01, 0x3B]);
        i += 4;
        let total = i;

        let resp = decode_telemetry_response_lpp(&buf[..total]).expect("must parse");
        assert_eq!(resp.battery_voltage_mv, Some(4_190));
        assert_eq!(
            resp.temperature_c_x10,
            Some(315),
            "temperature after three skipped entries must still decode"
        );
    }

    /// An entirely unknown channel type must be skipped by upstream's own default
    /// length (1 payload byte, `LPPData::getDataSize`'s fall-through at
    /// `LPPDataHelpers.h:105`) and must NOT end the scan: the fields after it
    /// still decode, and the unknown entry contributes no value at all.
    #[test]
    fn decode_skips_unknown_channel_type_and_keeps_later_fields() {
        let mut buf = [0u8; 32];
        buf[..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        let mut i = 4;
        // [ch=1, LPP_VOLTAGE, 0x01A3]
        buf[i..i + 4].copy_from_slice(&[TELEM_CHANNEL_SELF, 116, 0x01, 0xA3]);
        i += 4;
        // [ch=9, type 126 (not a Cayenne LPP id), 0x2A] — unknown, 1-byte payload.
        buf[i..i + 3].copy_from_slice(&[9, 126, 0x2A]);
        i += 3;
        // [ch=1, LPP_TEMPERATURE, 0x013B]
        buf[i..i + 4].copy_from_slice(&[TELEM_CHANNEL_SELF, 103, 0x01, 0x3B]);
        i += 4;
        // [ch=1, LPP_GPS, lat/lon/alt]
        buf[i..i + 11].copy_from_slice(&[
            TELEM_CHANNEL_SELF,
            136,
            0x07,
            0x57,
            0x95,
            0x01,
            0xC1,
            0xE1,
            0x00,
            0x00,
            0x00,
        ]);
        i += 11;
        let total = i;

        let resp = decode_telemetry_response_lpp(&buf[..total]).expect("must parse");
        assert_eq!(resp.battery_voltage_mv, Some(4_190));
        assert_eq!(
            resp.temperature_c_x10,
            Some(315),
            "field after the unknown entry must decode"
        );
        assert_eq!(
            resp.gps_e4,
            Some((481_173, 115_169)),
            "field after the unknown entry must decode"
        );
        // The unknown entry is skipped silently, never surfaced as a value.
        assert_eq!(resp.battery, None);
        assert_eq!(resp.battery_raw_mv, None);
    }

    /// Truncation safety: a buffer that ends mid-entry must not panic, must not
    /// read past its end, and must keep every field decoded before the cut. The
    /// GPS entry is cut to 5 of its 11 bytes.
    #[test]
    fn decode_tolerates_buffer_ending_mid_gps_entry() {
        let mut buf = [0u8; 32];
        buf[..4].copy_from_slice(&3u32.to_le_bytes());
        buf[4..8].copy_from_slice(&[TELEM_CHANNEL_SELF, 116, 0x01, 0xA3]);
        buf[8..12].copy_from_slice(&[TELEM_CHANNEL_SELF, 103, 0x01, 0x3B]);
        // [ch=1, LPP_GPS, lat(3B)] then the buffer simply stops.
        buf[12..17].copy_from_slice(&[TELEM_CHANNEL_SELF, 136, 0x07, 0x57, 0x95]);

        for cut in 12..17 {
            let resp = decode_telemetry_response_lpp(&buf[..cut]).expect("must parse");
            assert_eq!(
                resp.battery_voltage_mv,
                Some(4_190),
                "prefix before the cut must still decode (cut={cut})"
            );
            assert_eq!(resp.temperature_c_x10, Some(315), "cut={cut}");
            assert_eq!(
                resp.gps_e4, None,
                "truncated GPS must not decode (cut={cut})"
            );
        }
    }

    /// Truncation safety for a 2-byte-payload entry cut one byte short: the
    /// partial `LPP_TEMPERATURE` must be dropped, not half-read.
    #[test]
    fn decode_tolerates_buffer_ending_mid_temperature_entry() {
        let mut buf = [0u8; 16];
        buf[..4].copy_from_slice(&5u32.to_le_bytes());
        buf[4..8].copy_from_slice(&[TELEM_CHANNEL_SELF, 116, 0x01, 0xA3]);
        buf[8..11].copy_from_slice(&[TELEM_CHANNEL_SELF, 103, 0x01]);

        let resp = decode_telemetry_response_lpp(&buf[..11]).expect("must parse");
        assert_eq!(resp.battery_voltage_mv, Some(4_190));
        assert_eq!(resp.temperature_c_x10, None, "half an entry is no reading");

        // ...and a lone trailing type byte with no payload at all.
        let resp = decode_telemetry_response_lpp(&buf[..10]).expect("must parse");
        assert_eq!(resp.battery_voltage_mv, Some(4_190));
        assert_eq!(resp.temperature_c_x10, None);

        // ...and a tag-only body (upstream's "no entries" floor).
        let resp = decode_telemetry_response_lpp(&buf[..4]).expect("must parse");
        assert_eq!(resp.battery_voltage_mv, None);
        assert_eq!(resp.temperature_c_x10, None);
    }

    /// Upstream also emits `LPP_VOLTAGE`/`LPP_TEMPERATURE` for *external* sensors
    /// on channels 2+ (`EnvironmentSensorManager::querySensors` assigns
    /// `TELEM_CHANNEL_SELF + 1` upward, `EnvironmentSensorManager.cpp:685-698`).
    /// Those are not the peer's base readings, so they must be skipped rather
    /// than overwrite the channel-1 value.
    #[test]
    fn decode_ignores_voltage_and_temperature_on_other_channels() {
        let mut buf = [0u8; 16];
        buf[..4].copy_from_slice(&9u32.to_le_bytes());
        // [ch=3, LPP_VOLTAGE, 0x1388] -> 50.00 V on an INA219 channel.
        buf[4..8].copy_from_slice(&[3, 116, 0x13, 0x88]);
        // [ch=2, LPP_TEMPERATURE, 0x00FA] -> 25.0 C on a BME280 channel.
        buf[8..12].copy_from_slice(&[2, 103, 0x00, 0xFA]);

        let resp = decode_telemetry_response_lpp(&buf[..12]).expect("must parse");
        assert_eq!(resp.battery_voltage_mv, None);
        assert_eq!(resp.temperature_c_x10, None);
    }

    /// The skip table must agree with the `LPP_*_ENTRY_LEN` constants the encoder
    /// and the rest of this module are written against — otherwise "skip by
    /// declared length" would quietly desynchronise the scan.
    #[test]
    fn lpp_entry_len_table_matches_entry_len_constants() {
        assert_eq!(lpp_entry_len(LPP_DIGITAL_INPUT), LPP_BYTE_ENTRY_LEN);
        assert_eq!(lpp_entry_len(LPP_DIGITAL_OUTPUT), LPP_BYTE_ENTRY_LEN);
        assert_eq!(lpp_entry_len(LPP_PRESENCE), LPP_BYTE_ENTRY_LEN);
        assert_eq!(lpp_entry_len(LPP_PERCENTAGE), LPP_BYTE_ENTRY_LEN);
        assert_eq!(lpp_entry_len(LPP_LUMINOSITY), LPP_U16_ENTRY_LEN);
        assert_eq!(lpp_entry_len(LPP_TEMPERATURE), LPP_U16_ENTRY_LEN);
        assert_eq!(lpp_entry_len(LPP_VOLTAGE), LPP_U16_ENTRY_LEN);
        assert_eq!(lpp_entry_len(LPP_GENERIC_SENSOR), LPP_U32_ENTRY_LEN);
        assert_eq!(lpp_entry_len(LPP_GPS), LPP_GPS_ENTRY_LEN);
        // Unknown types fall back to a 1-byte payload, as upstream does.
        assert_eq!(lpp_entry_len(126), LPP_BYTE_ENTRY_LEN);
        assert_eq!(lpp_entry_len(0xFF), LPP_BYTE_ENTRY_LEN);
    }

    // ── policy gate (documented, tested indirectly) ───────────────────────────
    //
    // The telemetry_enabled gate lives in protocol::policy::PolicyFilter and has
    // its own tests (telemetry_flag_respected, telemetry_unknown_contact_denied).
    // This module is codec-only; policy enforcement is at the call site in the
    // firmware dispatcher's handle_dm function.
    //
    // Acceptance criterion "non-enabled contact request is dropped" is verified
    // by:
    //   1. protocol::policy tests (policy gate returns false → no response built)
    //   2. firmware handle_dm (returns early when policy gate is false — HIL logged)
}
