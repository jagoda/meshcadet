// SPDX-License-Identifier: GPL-3.0-only
//! Region provisioning-frame logic (ADR-0015 D2/D5) — the pure half of the
//! `FRAME_SET_REGION` / `FRAME_QUERY_REGION` handlers that both USB-serial
//! servers dispatch (`firmware/src/provisioning_server.rs` for the first-boot
//! path, `firmware/src/admin_server.rs` for the post-provisioning path).
//!
//! This module only **decides**; it never touches NVS or the serial port. The
//! servers keep the `EspNvs` write (`region_store::save_name`) and the
//! `send_frame` call, exactly as `room_admin` splits the room frames.
//!
//! # Why a shared decider
//!
//! Both servers have their own `match frame_type` dispatch and NEITHER knew
//! `SET_REGION`/`QUERY_REGION` before this change: an unhandled frame fell
//! through to each file's `unknown` arm and answered
//! `RSP_ERROR{code: 0xFF, msg: "unknown frame type"}`. That is the broken
//! state this module exists to close — `meshcadet set-region` (host CLI) and
//! the web provisioner's region field both send these frames, so a device
//! without handler arms looks like it rejected a perfectly valid request.
//!
//! # Policy (ADR-0015 D4/D5)
//!
//! - `SET_REGION`'s payload is `name_len(1) | name(name_len)`; `name_len == 0`
//!   is the explicit **clear** command (there is no separate clear frame).
//! - A non-empty name is validated with `protocol::region`'s shared rules:
//!   ≤ 31 bytes, every byte in upstream's `is_name_char` set.
//! - A leading `$` (upstream *private* region) is a valid name shape but is
//!   rejected with a distinct error: upstream's private hardware keystore is
//!   unimplemented, so keying it as a public region would scope traffic to a
//!   key the operator did not choose.
//! - Over-length names are rejected, never truncated: a truncated name derives
//!   a different key and therefore a different region.
//!
//! # When a change takes effect
//!
//! Immediately to flash, at the next boot on air. The emission scope is
//! derived once at boot (`firmware_core::region_scope::RegionScope`) and held
//! by the radio loop, so — exactly like every other provisioned setting this
//! firmware boot-snapshots (channels, contacts, radio preset) — a
//! `SET_REGION` persists immediately (`RSP_OK` follows a completed NVS write,
//! same ack semantics as `SET_DEVICE_NAME`) but only changes emission after a
//! reboot. That is deliberate: mutating the live emission scope mid-flight
//! would apply a different transport code to a payload already in the TX
//! queue, and `code0` must stay byte-identical across a frame's retries.

use protocol::provisioning::{decode_set_region, encode_rsp_region, ProvError};
use protocol::region::{validate_region_name, RegionNameError, REGION_NAME_MAX};

/// A validated region name, ready for the caller to persist.
///
/// Zero-padded to [`REGION_NAME_MAX`], mirroring the `name`/`name_len`
/// convention every other firmware store uses (`identity_store`'s device name,
/// `Contact::display_name`, `Channel::name`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidatedRegionName {
    /// UTF-8 region name, zero-padded.
    pub name: [u8; REGION_NAME_MAX],
    /// Actual byte length (0 ⇒ clear the configured region).
    pub name_len: u8,
}

impl ValidatedRegionName {
    /// The name bytes, without padding.
    pub fn as_bytes(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}

/// Decision for one `FRAME_SET_REGION` payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetRegionOutcome {
    /// Validated. The caller must persist `name` (an empty one clears the
    /// stored region) and reply `RSP_OK` only after that write succeeds.
    Apply(ValidatedRegionName),
    /// A `$`-prefixed (private) region: valid upstream name shape, but
    /// upstream cannot key it (no private keystore), so it is refused with a
    /// distinct, actionable error instead of being silently mis-scoped.
    PrivateUnsupported,
    /// The name is too long or contains a byte outside upstream's set.
    InvalidName(RegionNameError),
    /// The frame payload itself is malformed (empty frame, or a length prefix
    /// that runs past the end of the payload).
    DecodeError,
}

/// Decide what a `FRAME_SET_REGION` payload means.
///
/// Uses the shared `protocol::provisioning::decode_set_region` for the
/// length-prefixed shape (so the firmware and the host CLI cannot disagree
/// about truncation), then re-runs `protocol::region::validate_region_name`
/// to distinguish *why* a name was refused — `decode_set_region` collapses
/// every validation failure into one `ProvError`, and the two failures the
/// operator can act on (private region vs. bad characters) deserve different
/// error strings.
pub fn handle_set_region(payload: &[u8]) -> SetRegionOutcome {
    // Derive the name slice first so a refusal can be classified precisely;
    // the shared decoder then cross-checks the same payload so this function
    // cannot accept a shape the host CLI / web provisioner encoder would
    // refuse.
    let Some((&len_byte, rest)) = payload.split_first() else {
        return SetRegionOutcome::DecodeError;
    };
    let name_len = len_byte as usize;
    if name_len > REGION_NAME_MAX {
        // Mirrors `decode_set_region`'s precedence: an over-length prefix is a
        // name error (the operator asked for a 32-byte region), not a framing
        // error, and it must never be truncated to fit.
        return SetRegionOutcome::InvalidName(RegionNameError::TooLong { len: name_len });
    }
    if rest.len() < name_len {
        return SetRegionOutcome::DecodeError;
    }
    match decode_set_region(payload) {
        // Shape accepted, and (for `RegionNameInvalid`) the shape is all this
        // decoder can tell us — the precise reason comes from `classify` below.
        Ok(_) | Err(ProvError::RegionNameInvalid) => {}
        Err(_) => return SetRegionOutcome::DecodeError,
    }
    let name = &rest[..name_len];
    if name.is_empty() {
        // Explicit clear (`name_len == 0` is not a name, so it must never
        // reach `classify`, which would reject an empty name).
        return SetRegionOutcome::Apply(ValidatedRegionName {
            name: [0u8; REGION_NAME_MAX],
            name_len: 0,
        });
    }
    classify(name)
}

/// Classify one non-empty candidate name.
fn classify(name: &[u8]) -> SetRegionOutcome {
    match validate_region_name(name) {
        Ok(()) => {
            let mut padded = [0u8; REGION_NAME_MAX];
            padded[..name.len()].copy_from_slice(name);
            SetRegionOutcome::Apply(ValidatedRegionName {
                name: padded,
                name_len: name.len() as u8,
            })
        }
        Err(RegionNameError::PrivateUnsupported) => SetRegionOutcome::PrivateUnsupported,
        Err(e) => SetRegionOutcome::InvalidName(e),
    }
}

/// Decision for one `FRAME_QUERY_REGION` request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryRegionOutcome {
    /// `RSP_REGION` payload encoded into the caller's buffer; send those
    /// `len` bytes as `FRAME_RSP_REGION`. `name_len == 0` means "no region
    /// configured" (the device emits plain unscoped `FLOOD` frames).
    Encoded(usize),
    /// The stored name cannot be reported as a valid region (over-length,
    /// private, or a byte outside upstream's set) — answer `RSP_ERROR`
    /// instead of a name every host-side decoder would reject. This is the
    /// "surface it in status" half of ADR-0015's fail-closed rule: `status`
    /// prints the error rather than a plausible-looking region.
    StoredNameUnusable(RegionNameError),
}

/// Encode the response to `FRAME_QUERY_REGION` from the raw stored name.
///
/// `out` must be at least `1 + REGION_NAME_MAX` bytes
/// (`protocol::provisioning::MAX_REGION_NAME_LEN + 1`).
pub fn handle_query_region(name: &[u8], out: &mut [u8]) -> QueryRegionOutcome {
    if name.is_empty() {
        // No region: the wire form is the same length-prefixed shape with
        // `name_len == 0`, which the host CLI renders as
        // "(none — unscoped flood)".
        return match encode_rsp_region(&[], out) {
            Ok(len) => QueryRegionOutcome::Encoded(len),
            // `encode_rsp_region(&[])` cannot fail (an empty name is explicitly
            // valid), and `out` is sized `1 + REGION_NAME_MAX` by both callers;
            // keep the contract total rather than unwrapping.
            Err(_) => {
                QueryRegionOutcome::StoredNameUnusable(RegionNameError::TooLong { len: out.len() })
            }
        };
    }
    if let Err(e) = validate_region_name(name) {
        return QueryRegionOutcome::StoredNameUnusable(e);
    }
    match encode_rsp_region(name, out) {
        Ok(len) => QueryRegionOutcome::Encoded(len),
        // `encode_rsp_region` validates with the same rules, so this arm is
        // unreachable given the check above; mapping it to the same outcome
        // keeps the fail-closed contract total rather than panicking.
        Err(_) => {
            QueryRegionOutcome::StoredNameUnusable(RegionNameError::TooLong { len: name.len() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::provisioning::{decode_rsp_region, encode_set_region, MAX_REGION_NAME_LEN};

    fn set_region_payload(name: &[u8]) -> Vec<u8> {
        let mut buf = [0u8; 1 + MAX_REGION_NAME_LEN];
        let len = encode_set_region(name, &mut buf).expect("valid name encodes");
        buf[..len].to_vec()
    }

    #[test]
    fn set_region_accepts_a_public_name_and_returns_it_padded() {
        let outcome = handle_set_region(&set_region_payload(b"seattle"));
        match outcome {
            SetRegionOutcome::Apply(v) => {
                assert_eq!(v.name_len, 7);
                assert_eq!(v.as_bytes(), b"seattle");
                assert_eq!(&v.name[7..], &[0u8; REGION_NAME_MAX - 7]);
            }
            other => panic!("expected Apply, got {other:?}"),
        }
    }

    #[test]
    fn set_region_accepts_a_leading_hash_form() {
        match handle_set_region(&set_region_payload(b"#seattle")) {
            SetRegionOutcome::Apply(v) => assert_eq!(v.as_bytes(), b"#seattle"),
            other => panic!("expected Apply, got {other:?}"),
        }
    }

    #[test]
    fn set_region_empty_name_is_the_clear_command() {
        // `name_len == 0` in a well-formed frame ⇒ clear.
        let payload = [0u8];
        match handle_set_region(&payload) {
            SetRegionOutcome::Apply(v) => {
                assert_eq!(v.name_len, 0);
                assert_eq!(v.as_bytes(), b"");
            }
            other => panic!("expected Apply(clear), got {other:?}"),
        }
    }

    #[test]
    fn set_region_private_region_is_refused_distinctly() {
        // The encoder refuses `$` names (it validates), so build the payload
        // by hand: name_len prefix + raw bytes.
        let mut payload = vec![7u8];
        payload.extend_from_slice(b"$secret");
        assert_eq!(
            handle_set_region(&payload),
            SetRegionOutcome::PrivateUnsupported
        );
    }

    #[test]
    fn set_region_invalid_byte_is_refused_with_the_reason() {
        let mut payload = vec![3u8];
        payload.extend_from_slice(b"a b");
        assert_eq!(
            handle_set_region(&payload),
            SetRegionOutcome::InvalidName(RegionNameError::InvalidByte {
                index: 1,
                byte: b' '
            })
        );
    }

    #[test]
    fn set_region_over_length_is_refused_never_truncated() {
        let too_long = [b'A'; REGION_NAME_MAX + 1];
        let mut payload = vec![too_long.len() as u8];
        payload.extend_from_slice(&too_long);
        assert_eq!(
            handle_set_region(&payload),
            SetRegionOutcome::InvalidName(RegionNameError::TooLong {
                len: REGION_NAME_MAX + 1
            })
        );
    }

    #[test]
    fn set_region_malformed_frames_decode_error() {
        assert_eq!(handle_set_region(&[]), SetRegionOutcome::DecodeError);
        // Length prefix claims more bytes than the frame carries.
        assert_eq!(handle_set_region(&[5, b'a']), SetRegionOutcome::DecodeError);
        // Length prefix itself is over the ceiling: a name error, not a
        // framing error — and never truncated.
        let mut payload = vec![(REGION_NAME_MAX + 1) as u8];
        payload.extend_from_slice(&[b'A'; REGION_NAME_MAX + 1]);
        assert_eq!(
            handle_set_region(&payload),
            SetRegionOutcome::InvalidName(RegionNameError::TooLong {
                len: REGION_NAME_MAX + 1
            })
        );
    }

    #[test]
    fn query_region_encodes_the_stored_name_and_round_trips() {
        let mut out = [0u8; 1 + MAX_REGION_NAME_LEN];
        let n = match handle_query_region(b"bay-area", &mut out) {
            QueryRegionOutcome::Encoded(n) => n,
            other => panic!("expected Encoded, got {other:?}"),
        };
        let decoded = decode_rsp_region(&out[..n]).expect("host decoder accepts our reply");
        assert_eq!(decoded.name_len, 8);
        assert_eq!(&decoded.name[..8], b"bay-area");
    }

    #[test]
    fn query_region_with_no_region_reports_an_empty_name() {
        let mut out = [0u8; 1 + MAX_REGION_NAME_LEN];
        let n = match handle_query_region(b"", &mut out) {
            QueryRegionOutcome::Encoded(n) => n,
            other => panic!("expected Encoded, got {other:?}"),
        };
        assert_eq!(n, 1, "empty name ⇒ single length byte");
        assert_eq!(out[0], 0);
        let decoded = decode_rsp_region(&out[..n]).expect("decodes as no-region");
        assert_eq!(decoded.name_len, 0);
    }

    #[test]
    fn query_region_surfaces_an_unusable_stored_name_as_an_error() {
        let mut out = [0u8; 1 + MAX_REGION_NAME_LEN];
        assert_eq!(
            handle_query_region(b"$private", &mut out),
            QueryRegionOutcome::StoredNameUnusable(RegionNameError::PrivateUnsupported)
        );
        assert_eq!(
            handle_query_region(&[b'A'; REGION_NAME_MAX + 1], &mut out),
            QueryRegionOutcome::StoredNameUnusable(RegionNameError::TooLong {
                len: REGION_NAME_MAX + 1
            })
        );
    }

    #[test]
    fn set_then_query_round_trips_through_the_stored_form() {
        // The exact bytes the servers persist/read back: `name[..name_len]`.
        let payload = set_region_payload(b"bay-area");
        let stored = match handle_set_region(&payload) {
            SetRegionOutcome::Apply(v) => v,
            other => panic!("expected Apply, got {other:?}"),
        };
        let mut out = [0u8; 1 + MAX_REGION_NAME_LEN];
        let n = match handle_query_region(stored.as_bytes(), &mut out) {
            QueryRegionOutcome::Encoded(n) => n,
            other => panic!("expected Encoded, got {other:?}"),
        };
        let decoded = decode_rsp_region(&out[..n]).unwrap();
        assert_eq!(&decoded.name[..decoded.name_len as usize], b"bay-area");
    }
}
