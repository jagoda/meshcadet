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
//   a GPS fix is one `LPP_GPS` entry on channel `TELEM_CHANNEL_SELF`.

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

/// Cayenne LPP data-type byte for a digital-input / boolean reading
/// (`LPP_DIGITAL_INPUT`, "1 byte"). Reused here — distinct from
/// [`LPP_PRESENCE`], which already means "no GPS fix" in this protocol — for
/// the battery "is-charging" boolean, so the two booleans in one RESPONSE
/// stay unambiguous by TYPE byte, not just by position.
const LPP_DIGITAL_INPUT: u8 = 0;

/// Bytes of one LPP GPS entry on the wire: channel(1) + type(1) + lat(3) +
/// lon(3) + alt(3) = 11.
const LPP_GPS_ENTRY_LEN: usize = 11;

/// Bytes of one 1-byte-payload LPP entry (percentage, digital-input,
/// presence): channel(1) + type(1) + data(1) = 3.
const LPP_BYTE_ENTRY_LEN: usize = 3;

/// Maximum RESPONSE plaintext length: tag(4) + one LPP GPS entry(11) + one
/// battery-percentage entry(3) + one charging-state entry(3) = 21. A round 24
/// gives headroom for a future extra LPP entry without a reallocation
/// surprise at the call site.
pub const MAX_TELEMETRY_RESPONSE_LEN: usize = 24;

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
/// Cayenne LPP GPS packs lat/lon as `degrees * 1e4` and altitude as
/// `metres * 1e2`, each a 24-bit big-endian two's-complement integer. MeshCadet
/// stores coordinates as e7 (1e-7 deg), so we scale down by 1000; altitude is
/// not tracked and is reported as 0. `out` must be ≥ [`MAX_TELEMETRY_RESPONSE_LEN`].
///
/// Returns the number of bytes written.
pub fn encode_telemetry_response_lpp(
    tag: u32,
    gps: Option<(i32, i32)>,
    battery: Option<(u8, bool)>,
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
}

/// Parse a `PAYLOAD_TYPE_RESPONSE` plaintext produced by
/// [`encode_telemetry_response_lpp`]. Returns `None` if it is too short to hold
/// a tag. Scans every LPP entry in the body (GPS, presence, percentage,
/// digital-input); unrecognised entry types stop the scan (defensive against
/// a foreign/corrupt RESPONSE rather than misreading past it).
pub fn decode_telemetry_response_lpp(plaintext: &[u8]) -> Option<TelemetryResponseLpp> {
    if plaintext.len() < 4 {
        return None;
    }
    let tag = u32::from_le_bytes([plaintext[0], plaintext[1], plaintext[2], plaintext[3]]);
    let mut gps_e4 = None;
    let mut percent: Option<u8> = None;
    let mut charging: Option<bool> = None;
    let mut body = &plaintext[4..];

    while body.len() >= 2 {
        let entry_type = body[1];
        match entry_type {
            LPP_GPS if body.len() >= LPP_GPS_ENTRY_LEN => {
                let lat_e4 = read_i24_be(&body[2..5]);
                let lon_e4 = read_i24_be(&body[5..8]);
                gps_e4 = Some((lat_e4, lon_e4));
                body = &body[LPP_GPS_ENTRY_LEN..];
            }
            LPP_PERCENTAGE if body.len() >= LPP_BYTE_ENTRY_LEN => {
                percent = Some(body[2]);
                body = &body[LPP_BYTE_ENTRY_LEN..];
            }
            LPP_DIGITAL_INPUT if body.len() >= LPP_BYTE_ENTRY_LEN => {
                charging = Some(body[2] != 0);
                body = &body[LPP_BYTE_ENTRY_LEN..];
            }
            LPP_PRESENCE if body.len() >= LPP_BYTE_ENTRY_LEN => {
                // "No GPS fix" marker — no data to extract, just consume it.
                body = &body[LPP_BYTE_ENTRY_LEN..];
            }
            _ => break, // unknown/truncated entry — stop scanning defensively.
        }
    }

    let battery = match (percent, charging) {
        (Some(p), Some(c)) => Some((p, c)),
        _ => None,
    };
    Some(TelemetryResponseLpp {
        tag,
        gps_e4,
        battery,
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
        let n = encode_telemetry_response_lpp(tag, Some((lat_e7, lon_e7)), None, &mut buf);
        assert!(n <= MAX_TELEMETRY_RESPONSE_LEN);
        let resp = decode_telemetry_response_lpp(&buf[..n]).expect("must parse");
        assert_eq!(
            resp.tag, tag,
            "tag must reflect so the companion matches reply→request"
        );
        assert_eq!(resp.gps_e4, Some((lat_e7 / 1000, lon_e7 / 1000)));
        assert_eq!(resp.battery, None, "no battery reading was supplied");
    }

    #[test]
    fn response_lpp_negative_coords_roundtrip() {
        // Southern/Western hemisphere → negative 24-bit two's complement.
        let tag = 42u32;
        let lat_e7 = -338_688_000i32; // -33.8688°
        let lon_e7 = -151_209_300i32; // -15.12093°
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(tag, Some((lat_e7, lon_e7)), None, &mut buf);
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
        let n = encode_telemetry_response_lpp(tag, None, None, &mut buf);
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
        let n = encode_telemetry_response_lpp(1, Some((481_173_000, 115_169_000)), None, &mut buf);
        assert!(n > 4, "fix response must exceed the 4-byte tag (got {n})");
    }

    // ── Battery LPP entries ─────

    #[test]
    fn response_lpp_battery_roundtrips_alongside_gps() {
        let tag = 0x7777_7777u32;
        let lat_e7 = 481_173_000i32;
        let lon_e7 = 115_169_000i32;
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n =
            encode_telemetry_response_lpp(tag, Some((lat_e7, lon_e7)), Some((82, true)), &mut buf);
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
        let n = encode_telemetry_response_lpp(tag, None, Some((14, false)), &mut buf);
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
        let n = encode_telemetry_response_lpp(1, None, Some((50, false)), &mut buf);
        let resp = decode_telemetry_response_lpp(&buf[..n]).unwrap();
        assert_eq!(resp.battery, Some((50, false)));
    }

    #[test]
    fn response_lpp_battery_percent_clamped_at_100() {
        // Defensive: a caller passing an out-of-range percent must not corrupt
        // the wire format with a value a real receiver would reject/misread.
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(1, None, Some((250, true)), &mut buf);
        let resp = decode_telemetry_response_lpp(&buf[..n]).unwrap();
        assert_eq!(resp.battery, Some((100, true)), "percent must clamp to 100");
    }

    #[test]
    fn response_lpp_no_battery_reading_omits_entries() {
        // `None` means "no reading available" (e.g. a bench rig with no
        // battery ADC wired) — must not fabricate a battery reading on the wire.
        let mut buf = [0u8; MAX_TELEMETRY_RESPONSE_LEN];
        let n = encode_telemetry_response_lpp(1, Some((0, 0)), None, &mut buf);
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
            &mut buf,
        );
        assert!(
            n <= MAX_TELEMETRY_RESPONSE_LEN,
            "worst case (max tag + GPS + battery) must fit: {n} > {MAX_TELEMETRY_RESPONSE_LEN}"
        );
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
