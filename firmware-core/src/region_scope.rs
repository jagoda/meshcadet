// SPDX-License-Identifier: GPL-3.0-only
//! Boot-time region scope — the firmware's single source of truth for whether
//! an outbound flood frame carries MeshCore transport codes.
//!
//! ADR-0015 D1/D3/D4 pin the three rules this module exists to enforce:
//!
//! 1. **Opt-in, outbound-only.** A device with no region configured emits a
//!    plain `ROUTE_TYPE_FLOOD` frame, byte-for-byte identical to the
//!    pre-region path ([`RegionScope::Unscoped`]).
//! 2. **`{0, 0}` is forbidden.** "No region" is expressed by *omitting* the
//!    four transport-code bytes, never by writing a zero code:
//!    upstream `RegionMap::findMatch` short-circuits `codes[0] == 0` to the
//!    wildcard region and ignores its `DENY` flags, so a `{0,0}` frame looks
//!    scoped while actually broadcasting everywhere. Every frame this module
//!    writes is either code-free ([`RegionScope::Unscoped`]) or carries a
//!    `code0` that `protocol::region`'s remap guarantees is non-zero
//!    ([`RegionScope::Scoped`]).
//! 3. **Fail closed.** A configured region whose key cannot be derived must
//!    surface as an error and emit *nothing* — never fall back to unscoped
//!    emission ([`RegionScope::Unusable`]). "Healthy-looking device whose
//!    traffic every region drops" is the exact defect region scoping exists
//!    to fix, so silently unscoping is worse than going quiet.
//!
//! [`RegionScope::emission`] is the fail-closed gate: its `Result<Option<..>>`
//! shape forces every caller to distinguish "unscoped" (`Ok(None)`) from
//! "configured but unusable" (`Err`). Collapsing those two into a single
//! `None` is precisely the silent-unscoped fallback rule 3 forbids, so it is
//! made unrepresentable at the API boundary rather than left to convention.
//!
//! # Where the scope is computed
//!
//! Exactly once per boot, from the persisted name (see
//! `firmware/src/region_store.rs` for the NVS half and
//! `firmware/src/main.rs`'s `region_scope` local for the call site), and then
//! held for the lifetime of the radio loop. `code0` is keyed on the payload
//! (`TransportKeyStore::calcTransportCode`), so it is computed once per
//! distinct frame and never recomputed per retry — the dispatcher's cached
//! retry frame is already byte-identical and is re-enqueued verbatim
//! (`firmware_core::dispatcher::OutstandingSend`), so nothing in the retry
//! path re-runs any of this.
//!
//! # What is NOT scoped here
//!
//! Direct-route frames (`ROUTE_TYPE_DIRECT`) and the room keep-alive that
//! rides them (`room_session::encode_room_keep_alive_frame`) carry no
//! transport-code bytes at all — upstream only suffixes `TRANSPORT_FLOOD` and
//! `TRANSPORT_DIRECT`, and this module deliberately touches neither.

use protocol::header::{Header, PathLen};
use protocol::region::{self, RegionNameError};
use protocol::{PayloadType, RouteType};

/// Number of transport-code bytes present in a scoped flood frame
/// (`code0` lo/hi + `code1` lo/hi).
pub const TRANSPORT_CODE_BYTES: usize = 4;

/// Why a configured region could not be turned into an emission scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionScopeError {
    /// A name is stored but it cannot be validated/keyed (over-length, a byte
    /// outside upstream's set, or `$`-private).
    Name(RegionNameError),
    /// The region store could not be read at all, so whether this device is
    /// configured is *unknown*. Fail closed rather than assume "no region":
    /// an unreadable store must not turn a region-managed device into an
    /// unscoped flooder.
    StoreUnavailable,
}

/// The device's region scope for this boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionScope {
    /// No region configured (`name_len == 0`): plain `ROUTE_TYPE_FLOOD`, the
    /// pre-region emission, byte-for-byte.
    Unscoped,
    /// A region name was configured and its 16-byte transport key derived.
    /// Flood frames carry `[code0][0,0]`.
    Scoped([u8; 16]),
    /// A region is configured (or cannot be read) but no key can be derived.
    /// FAIL CLOSED: flood emission is suppressed entirely until the
    /// configuration is corrected or cleared.
    Unusable(RegionScopeError),
}

impl RegionScope {
    /// The "no region configured" scope — the default this device boots with
    /// before (and without) any provisioning.
    pub const fn unscoped() -> Self {
        RegionScope::Unscoped
    }

    /// Resolve a stored/provisioned region name.
    ///
    /// An empty name means "no region" and resolves to
    /// [`RegionScope::Unscoped`] — emptiness is the provisioning layer's
    /// explicit *clear* command (`protocol::provisioning::decode_set_region`),
    /// not a malformed name. Any non-empty name is validated and keyed with
    /// exactly the same `protocol::region` helpers the host CLI and the web
    /// provisioner use, so the three surfaces cannot disagree.
    pub fn from_name(name: &[u8]) -> Result<Self, RegionNameError> {
        if name.is_empty() {
            return Ok(RegionScope::Unscoped);
        }
        let key = region::region_key(name)?;
        Ok(RegionScope::Scoped(key))
    }

    /// Resolve the raw blob read back from the region store, converting an
    /// underivable name into [`RegionScope::Unusable`] instead of an error.
    ///
    /// This is the boot path (`firmware/src/main.rs`): there is no caller to
    /// return an error to, and "unusable" must be a *state* the emission
    /// gate can refuse on, not a boot failure that would otherwise leave the
    /// device emitting unscoped traffic.
    pub fn from_stored_name(name: &[u8]) -> Self {
        match Self::from_name(name) {
            Ok(scope) => scope,
            Err(e) => RegionScope::Unusable(RegionScopeError::Name(e)),
        }
    }

    /// `true` for a device with no region configured.
    pub fn is_unscoped(&self) -> bool {
        matches!(self, RegionScope::Unscoped)
    }

    /// Why the configured region is unusable, if it is.
    pub fn unusable_reason(&self) -> Option<RegionScopeError> {
        match self {
            RegionScope::Unusable(e) => Some(*e),
            _ => None,
        }
    }

    /// The fail-closed emission gate every flood send path must pass through.
    ///
    /// - `Ok(None)` — no region configured: emit an unscoped `FLOOD` frame.
    /// - `Ok(Some(key))` — emit a scoped `TRANSPORT_FLOOD` frame keyed on
    ///   `key`.
    /// - `Err(_)` — a region IS configured but unusable: emit nothing at all.
    ///   The caller logs and drops the frame rather than falling back to
    ///   unscoped emission (ADR-0015 D4).
    ///
    /// The nested `Option` is deliberate: it makes the three outcomes
    /// impossible to conflate, so a caller cannot accidentally treat a broken
    /// region configuration as "no region".
    pub fn emission(&self) -> Result<Option<&[u8; 16]>, RegionScopeError> {
        match self {
            RegionScope::Unscoped => Ok(None),
            RegionScope::Scoped(key) => Ok(Some(key)),
            RegionScope::Unusable(e) => Err(*e),
        }
    }
}

/// Map a region-store read onto the boot-time scope.
///
/// `stored_name == None` means the store itself could not be read (an
/// `EspNvs` error) and therefore FAILS CLOSED
/// ([`RegionScopeError::StoreUnavailable`]); `Some(&[])` is the ordinary "no
/// region configured" case and resolves to [`RegionScope::Unscoped`].
pub fn boot_scope(stored_name: Option<&[u8]>) -> RegionScope {
    match stored_name {
        Some(name) => RegionScope::from_stored_name(name),
        None => RegionScope::Unusable(RegionScopeError::StoreUnavailable),
    }
}

/// Assemble a flood frame **in place** from a payload the caller has already
/// encoded at the unscoped payload offset (`out[2..2 + payload_len]`).
///
/// This is the single assembly point for every outbound flood frame in the
/// firmware, so the scoped/unscoped layout decision (ADR-0015 D3) exists
/// once instead of once per frame family:
///
/// ```text
/// unscoped: [header=ptype<<2|FLOOD]           [path_len] (4)  [payload]
/// scoped:   [header=ptype<<2|TRANSPORT_FLOOD] [code0][0,0] [path_len] (4)  [payload]
/// ```
///
/// The `(path_len)`-relative detail is what makes this a **0-hop** helper:
/// the payload starts at offset 2 in every call site (all of them outbound
/// flood frames with no learned path), and the function shifts
/// `out[2..2 + payload_len]` up by [`TRANSPORT_CODE_BYTES`] when scoped. A
/// future builder that carries a non-empty path must grow a path-aware
/// variant (`[header][codes][path_len][path][payload]`) instead of reusing
/// this one; `every_flood_builder_emits_zero_hop_only_frames` pins the
/// current contract.
///
/// Returns the total frame length. `scope == None` reproduces the pre-region
/// bytes exactly (same header, no code bytes), which is what ADR-0015 D6
/// requires for an unconfigured device.
///
/// # Panics
///
/// Panics if `out` cannot hold the assembled frame. Every call site sizes its
/// buffer for the scoped (worst-case) frame — the firmware's largest flood
/// payloads are ~160 B against 255-byte buffers — so this is a
/// contract-violation assert, not a reachable path.
pub fn assemble_flood_frame(
    scope: Option<&[u8; 16]>,
    out: &mut [u8],
    payload_type: PayloadType,
    path_len: PathLen,
    payload_len: usize,
) -> usize {
    const PAYLOAD_OFF_UNSCOPED: usize = 2;
    match scope {
        None => {
            // Unscoped: the caller's own `[header][path_len][...]` layout is
            // already correct; rewriting the two leading bytes keeps the
            // header/path_len authoritative to this one function.
            out[0] = Header::new(RouteType::Flood, payload_type).0;
            out[1] = path_len.0;
            PAYLOAD_OFF_UNSCOPED + payload_len
        }
        Some(key) => {
            let payload_end = PAYLOAD_OFF_UNSCOPED + payload_len;
            assert!(
                out.len() >= payload_end + TRANSPORT_CODE_BYTES,
                "assemble_flood_frame: buffer too small for a scoped frame",
            );
            // Codes are keyed on the PAYLOAD as it will appear on the wire,
            // i.e. the same bytes regardless of where the outer framing puts
            // them — computed before the shift, from the same slice.
            let codes = region::scoped_transport_codes(
                key,
                payload_type as u8,
                &out[PAYLOAD_OFF_UNSCOPED..payload_end],
            );
            debug_assert_ne!(codes[0] | codes[1], 0, "code0 must never be 0");
            out.copy_within(
                PAYLOAD_OFF_UNSCOPED..payload_end,
                PAYLOAD_OFF_UNSCOPED + TRANSPORT_CODE_BYTES,
            );
            out[0] = Header::new(RouteType::TransportFlood, payload_type).0;
            out[1..1 + TRANSPORT_CODE_BYTES].copy_from_slice(&codes);
            out[1 + TRANSPORT_CODE_BYTES] = path_len.0;
            payload_end + TRANSPORT_CODE_BYTES
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::frame::parse_rx_frame_header;
    use protocol::region::{region_key, REGION_NAME_MAX};

    fn zero_path() -> PathLen {
        PathLen::new(2, 0).expect("0-hop 2-byte path")
    }

    // ── Resolution ───────────────────────────────────────────────────────

    #[test]
    fn empty_name_is_unscoped_not_an_error() {
        assert_eq!(RegionScope::from_name(b""), Ok(RegionScope::Unscoped));
        assert_eq!(RegionScope::from_stored_name(b""), RegionScope::Unscoped);
        assert!(RegionScope::Unscoped.is_unscoped());
        assert_eq!(RegionScope::Unscoped.emission(), Ok(None));
    }

    #[test]
    fn valid_name_derives_the_same_key_as_protocol_region() {
        let expected = region_key(b"seattle").unwrap();
        assert_eq!(
            RegionScope::from_name(b"seattle"),
            Ok(RegionScope::Scoped(expected))
        );
        // '#'-prefixed and bare forms are the same region upstream.
        assert_eq!(
            RegionScope::from_stored_name(b"#seattle"),
            RegionScope::Scoped(expected)
        );
        // ... and the emission gate hands that key to the frame builders.
        assert_eq!(
            RegionScope::Scoped(expected).emission(),
            Ok(Some(&expected))
        );
    }

    #[test]
    fn private_region_is_unusable_not_unscoped() {
        let scope = RegionScope::from_stored_name(b"$secret");
        assert_eq!(
            scope.unusable_reason(),
            Some(RegionScopeError::Name(RegionNameError::PrivateUnsupported))
        );
        assert!(
            !scope.is_unscoped(),
            "an unusable region must never degrade to unscoped"
        );
    }

    #[test]
    fn over_length_and_bad_byte_names_are_unusable() {
        let too_long = [b'A'; REGION_NAME_MAX + 1];
        assert_eq!(
            RegionScope::from_stored_name(&too_long).unusable_reason(),
            Some(RegionScopeError::Name(RegionNameError::TooLong {
                len: REGION_NAME_MAX + 1
            }))
        );
        assert_eq!(
            RegionScope::from_stored_name(b"bad name").unusable_reason(),
            Some(RegionScopeError::Name(RegionNameError::InvalidByte {
                index: 3,
                byte: b' '
            }))
        );
    }

    // ── Fail-closed gate ────────────────────────────────────────────────

    #[test]
    fn emission_distinguishes_unscoped_from_unusable() {
        let key = region_key(b"seattle").unwrap();
        assert_eq!(RegionScope::Unscoped.emission(), Ok(None));
        assert_eq!(RegionScope::Scoped(key).emission(), Ok(Some(&key)));
        assert_eq!(
            RegionScope::Unusable(RegionScopeError::Name(RegionNameError::PrivateUnsupported))
                .emission(),
            Err(RegionScopeError::Name(RegionNameError::PrivateUnsupported))
        );
    }

    #[test]
    fn boot_scope_maps_a_store_read() {
        let key = region_key(b"seattle").unwrap();
        assert_eq!(boot_scope(Some(b"seattle")), RegionScope::Scoped(key));
        assert_eq!(boot_scope(Some(b"")), RegionScope::Unscoped);
        assert_eq!(
            boot_scope(Some(b"$secret")).unusable_reason(),
            Some(RegionScopeError::Name(RegionNameError::PrivateUnsupported))
        );
        // An unreadable store is NOT "no region": it must fail closed.
        assert_eq!(
            boot_scope(None),
            RegionScope::Unusable(RegionScopeError::StoreUnavailable)
        );
        assert!(!boot_scope(None).is_unscoped());
        assert!(boot_scope(None).emission().is_err());
    }

    #[test]
    fn unusable_scope_emits_nothing_and_never_looks_unscoped() {
        // Both failure variants — an underivable name and an unreadable store —
        // must be treated identically by the emission gate: the send site gets
        // no scope argument at all, so it cannot build a frame.
        for scope in [
            RegionScope::Unusable(RegionScopeError::Name(RegionNameError::PrivateUnsupported)),
            RegionScope::Unusable(RegionScopeError::StoreUnavailable),
        ] {
            assert!(scope.emission().is_err(), "{scope:?} must fail closed");
            assert!(!scope.is_unscoped(), "{scope:?} must not look unconfigured");
            assert!(scope.unusable_reason().is_some());
        }
        // ... while an unconfigured device is exactly the emit-unscoped case.
        assert_eq!(RegionScope::Unscoped.emission(), Ok(None));
    }

    // ── Frame assembly ──────────────────────────────────────────────────

    #[test]
    fn unscoped_assembly_is_byte_identical_to_protocol_unscoped_writer() {
        let payload = b"hello world";
        let mut ours = [0u8; 64];
        let mut theirs = [0u8; 64];
        // Callers encode the payload at offset 2 first (the existing layout in
        // every fleet builder in `main.rs`).
        ours[2..2 + payload.len()].copy_from_slice(payload);
        let n = assemble_flood_frame(
            None,
            &mut ours,
            PayloadType::TxtMsg,
            zero_path(),
            payload.len(),
        );
        let m = region::write_unscoped_flood_frame(
            &mut theirs,
            PayloadType::TxtMsg,
            zero_path(),
            &[],
            payload,
        );
        assert_eq!(n, m);
        assert_eq!(&ours[..n], &theirs[..m]);
        // Plain FLOOD route bits, no code bytes.
        assert_eq!(ours[0] & 0x03, RouteType::Flood as u8);
        assert_eq!(ours[1], zero_path().0);
    }

    #[test]
    fn scoped_assembly_is_byte_identical_to_protocol_scoped_writer() {
        let key = region_key(b"seattle").unwrap();
        let payload = b"hello world";
        let mut ours = [0u8; 64];
        let mut theirs = [0u8; 64];
        ours[2..2 + payload.len()].copy_from_slice(payload);
        let n = assemble_flood_frame(
            Some(&key),
            &mut ours,
            PayloadType::TxtMsg,
            zero_path(),
            payload.len(),
        );
        let m = region::write_scoped_flood_frame(
            &mut theirs,
            PayloadType::TxtMsg,
            &key,
            zero_path(),
            &[],
            payload,
        );
        assert_eq!(n, m);
        assert_eq!(&ours[..n], &theirs[..m]);
    }

    #[test]
    fn scoped_assembly_layout_and_roundtrip() {
        let key = region_key(b"bay-area").unwrap();
        let payload = b"xy";
        let mut out = [0u8; 32];
        out[2..4].copy_from_slice(payload);
        let n = assemble_flood_frame(Some(&key), &mut out, PayloadType::GrpTxt, zero_path(), 2);
        assert_eq!(n, 2 + TRANSPORT_CODE_BYTES + 2);
        assert_eq!(out[0], (PayloadType::GrpTxt as u8) << 2); // TransportFlood = 0x00
        assert_eq!(
            &out[1..5],
            &region::scoped_transport_codes(&key, PayloadType::GrpTxt as u8, payload)
        );
        assert_eq!(out[5], zero_path().0);
        assert_eq!(&out[6..n], payload);

        let parsed = parse_rx_frame_header(&out[..n]).expect("RX parser accepts writer output");
        assert!(parsed.has_transport_codes);
        assert_eq!(parsed.route_type, Some(RouteType::TransportFlood));
        assert_eq!(parsed.payload_off, 6);
        assert_eq!(&out[parsed.payload_off..n], payload);
    }

    #[test]
    fn scoped_codes_are_keyed_on_the_payload_not_the_framing() {
        let key = region_key(b"seattle").unwrap();
        let payload = b"payload-bytes";
        let mut out = [0u8; 64];
        out[2..2 + payload.len()].copy_from_slice(payload);
        let n = assemble_flood_frame(
            Some(&key),
            &mut out,
            PayloadType::TxtMsg,
            zero_path(),
            payload.len(),
        );
        let expected = region::transport_code(&key, PayloadType::TxtMsg as u8, payload);
        assert_eq!(u16::from_le_bytes([out[1], out[2]]), expected);
        assert_ne!(expected, 0, "code0 must never be the forbidden zero code");
        // Heterogeneous payloads must not collide in this fixture.
        let mut other = [0u8; 64];
        other[2..3].copy_from_slice(b"z");
        assemble_flood_frame(Some(&key), &mut other, PayloadType::TxtMsg, zero_path(), 1);
        assert_ne!(&out[1..5], &other[1..5]);
        let _ = n;
    }

    #[test]
    fn assembly_is_deterministic_across_retries() {
        // The dispatcher re-enqueues a cached frame on every retry; the codes
        // carry no clock/hop input, so two assemblies of the same payload must
        // be byte-identical.
        let key = region_key(b"seattle").unwrap();
        let payload = b"same bytes";
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        a[2..2 + payload.len()].copy_from_slice(payload);
        b[2..2 + payload.len()].copy_from_slice(payload);
        let na = assemble_flood_frame(
            Some(&key),
            &mut a,
            PayloadType::Ack,
            zero_path(),
            payload.len(),
        );
        let nb = assemble_flood_frame(
            Some(&key),
            &mut b,
            PayloadType::Ack,
            zero_path(),
            payload.len(),
        );
        assert_eq!(na, nb);
        assert_eq!(&a[..na], &b[..nb]);
    }

    // ── Per-frame-family layouts (DM, telemetry reply) ──────────────────
    //
    // `firmware/src/main.rs`'s `build_ui_dm` / `build_telemetry_reply` /
    // `build_telemetry_response` are thin wrappers: encode the plaintext with
    // `protocol`, encode the DM envelope with `protocol::encode_dm_payload`,
    // then hand the payload to `assemble_flood_frame` — exactly what these two
    // tests do. The `firmware/` crate is a detached Xtensa workspace that
    // cannot be compiled on the host lane, so pinning the byte layout HERE
    // (against the same protocol codecs those builders call) is the only way
    // the scoped DM/telemetry shapes get an executable regression test.

    /// Encode a DM exactly as `main.rs::build_ui_dm` does, returning the
    /// frame length. Payload is placed at the unscoped offset first, then
    /// assembled — the same contract `assemble_flood_frame` documents.
    fn build_dm_frame(
        scope: Option<&[u8; 16]>,
        sender: &protocol::identity::Identity,
        dest_pubkey: &[u8; 32],
        timestamp: u32,
        text: &[u8],
        out: &mut [u8; 255],
    ) -> (usize, [u8; 4]) {
        let shared = sender.ecdh_shared_secret(dest_pubkey);
        let mut pt = [0u8; 128];
        let pt_len = protocol::encode_txt_msg_plaintext(timestamp, 0, 0, text, &mut pt);
        let dm_len = protocol::encode_dm_payload(
            &shared,
            dest_pubkey[0],
            sender.pub_hash(),
            &pt[..pt_len],
            &mut out[2..],
        );
        let n = assemble_flood_frame(scope, out, PayloadType::TxtMsg, zero_path(), dm_len);
        let ack = protocol::compute_ack_hash(timestamp, 0, text, &sender.pubkey);
        (n, ack)
    }

    #[test]
    fn dm_frame_scoped_layout_is_byte_exact() {
        let sender = protocol::identity::Identity::from_seed([0x21u8; 32]);
        let dest = protocol::identity::Identity::from_seed([0x22u8; 32]);
        let key = region_key(b"seattle").unwrap();
        let text = b"hello mesh";

        let mut unscoped = [0u8; 255];
        let (n_unscoped, ack_unscoped) = build_dm_frame(
            None,
            &sender,
            &dest.pubkey,
            1_700_000_000,
            text,
            &mut unscoped,
        );
        let mut scoped = [0u8; 255];
        let (n_scoped, ack_scoped) = build_dm_frame(
            Some(&key),
            &sender,
            &dest.pubkey,
            1_700_000_000,
            text,
            &mut scoped,
        );

        let payload = &unscoped[2..n_unscoped];
        assert_eq!(
            ack_unscoped, ack_scoped,
            "the ACK hash is over the plaintext"
        );
        assert_eq!(n_scoped, n_unscoped + TRANSPORT_CODE_BYTES);
        assert_eq!(
            unscoped[0],
            Header::new(RouteType::Flood, PayloadType::TxtMsg).0,
            "unscoped DM keeps today's plain FLOOD header"
        );
        assert_eq!(
            scoped[0],
            Header::new(RouteType::TransportFlood, PayloadType::TxtMsg).0,
            "scoped DM is TRANSPORT_FLOOD"
        );
        assert_eq!(
            &scoped[1..5],
            &region::scoped_transport_codes(&key, PayloadType::TxtMsg as u8, payload)
        );
        assert_eq!(scoped[5], zero_path().0);
        assert_eq!(&scoped[6..n_scoped], payload, "DM envelope byte-identical");

        // The whole scoped frame round-trips through the RX header parser, so
        // a peer's `on_receive` computes the same payload offset.
        let parsed = parse_rx_frame_header(&scoped[..n_scoped]).unwrap();
        assert_eq!(parsed.payload_off, 6);
        assert_eq!(&scoped[parsed.payload_off..n_scoped], payload);
    }

    #[test]
    fn telemetry_reply_frame_scoped_layout_is_byte_exact() {
        let sender = protocol::identity::Identity::from_seed([0x31u8; 32]);
        let dest = protocol::identity::Identity::from_seed([0x32u8; 32]);
        let key = region_key(b"bay-area").unwrap();
        let timestamp = 1_700_000_100u32;

        let build = |scope: Option<&[u8; 16]>, out: &mut [u8; 255]| -> usize {
            let shared = sender.ecdh_shared_secret(&dest.pubkey);
            let mut text = [0u8; protocol::MAX_RESPONSE_LEN];
            let text_len =
                protocol::encode_telemetry_response(47_000_000, -122_000_000, 12, &mut text);
            let mut pt = [0u8; 128];
            let pt_len =
                protocol::encode_txt_msg_plaintext(timestamp, 0, 0, &text[..text_len], &mut pt);
            let dm_len = protocol::encode_dm_payload(
                &shared,
                dest.pubkey[0],
                sender.pub_hash(),
                &pt[..pt_len],
                &mut out[2..],
            );
            assemble_flood_frame(scope, out, PayloadType::TxtMsg, zero_path(), dm_len)
        };

        let mut unscoped = [0u8; 255];
        let n_unscoped = build(None, &mut unscoped);
        let mut scoped = [0u8; 255];
        let n_scoped = build(Some(&key), &mut scoped);

        let payload = &unscoped[2..n_unscoped];
        assert_eq!(n_scoped, n_unscoped + TRANSPORT_CODE_BYTES);
        assert_eq!(
            unscoped[0],
            Header::new(RouteType::Flood, PayloadType::TxtMsg).0
        );
        assert_eq!(
            scoped[0],
            Header::new(RouteType::TransportFlood, PayloadType::TxtMsg).0
        );
        assert_eq!(
            &scoped[1..5],
            &region::scoped_transport_codes(&key, PayloadType::TxtMsg as u8, payload)
        );
        assert_eq!(scoped[5], zero_path().0);
        assert_eq!(&scoped[6..n_scoped], payload);
    }

    #[test]
    fn grp_txt_frame_scoped_layout_is_byte_exact() {
        // Mirrors `main.rs::build_ui_grp_txt`: `encode_grp_txt_var` writes the
        // payload at the unscoped offset, then `assemble_flood_frame` inserts
        // the codes. The channel hash and ciphertext are outer-framing
        // independent, so they must be byte-identical either way.
        let secret = [0x5au8; 32];
        let text = b"alice: hello channel";
        let build = |scope: Option<&[u8; 16]>, out: &mut [u8; 255]| -> usize {
            let n = protocol::encode_grp_txt_var(&secret, 1_700_000_200, 0, 0, text, &mut out[2..]);
            assemble_flood_frame(scope, out, PayloadType::GrpTxt, zero_path(), n)
        };
        let key = region_key(b"seattle").unwrap();
        let mut unscoped = [0u8; 255];
        let n_unscoped = build(None, &mut unscoped);
        let mut scoped = [0u8; 255];
        let n_scoped = build(Some(&key), &mut scoped);

        let payload = &unscoped[2..n_unscoped];
        assert_eq!(n_scoped, n_unscoped + TRANSPORT_CODE_BYTES);
        assert_eq!(
            unscoped[0],
            Header::new(RouteType::Flood, PayloadType::GrpTxt).0
        );
        assert_eq!(
            scoped[0],
            Header::new(RouteType::TransportFlood, PayloadType::GrpTxt).0
        );
        assert_eq!(
            &scoped[1..5],
            &region::scoped_transport_codes(&key, PayloadType::GrpTxt as u8, payload)
        );
        assert_eq!(scoped[5], zero_path().0);
        assert_eq!(&scoped[6..n_scoped], payload);
    }

    #[test]
    fn flood_ack_frame_scoped_layout_is_byte_exact() {
        // Mirrors `main.rs::build_ack_frame`: `[header][(codes)][path_len]
        // [ack_hash(4)]`. An ACK is a flood frame like any other for scoping
        // purposes — it must be scoped when the device has a region, and it
        // must not be when it does not.
        let ack = [0xde, 0xad, 0xbe, 0xef];
        let build = |scope: Option<&[u8; 16]>, out: &mut [u8; 16]| -> usize {
            out[2..6].copy_from_slice(&ack);
            assemble_flood_frame(scope, out, PayloadType::Ack, zero_path(), 4)
        };
        let key = region_key(b"bay-area").unwrap();
        let mut unscoped = [0u8; 16];
        let n_unscoped = build(None, &mut unscoped);
        let mut scoped = [0u8; 16];
        let n_scoped = build(Some(&key), &mut scoped);

        assert_eq!(n_unscoped, 6, "header + path_len + 4-byte ack hash");
        assert_eq!(
            n_scoped, 10,
            "the four code bytes are inserted before path_len"
        );
        assert_eq!(
            unscoped[0],
            Header::new(RouteType::Flood, PayloadType::Ack).0
        );
        assert_eq!(
            scoped[0],
            Header::new(RouteType::TransportFlood, PayloadType::Ack).0
        );
        assert_eq!(
            &scoped[1..5],
            &region::scoped_transport_codes(&key, PayloadType::Ack as u8, &ack)
        );
        assert_eq!(scoped[5], zero_path().0);
        assert_eq!(&scoped[6..n_scoped], &ack);
        let parsed = parse_rx_frame_header(&scoped[..n_scoped]).expect("well-formed frame");
        assert_eq!(parsed.payload_off, 6);
    }

    #[test]
    fn every_flood_builder_emits_zero_hop_only_frames() {
        // `assemble_flood_frame` inserts the code bytes immediately after the
        // header, which is correct only for the 0-hop outbound path every
        // region-scoped send site uses (room login/post, DM, GRP_TXT,
        // telemetry, ACK). A future hop-carrying flood builder must grow a
        // path-aware variant instead of silently corrupting the framing.
        for scope in [None, Some(region_key(b"seattle").unwrap())] {
            let mut out = [0u8; 32];
            out[2..3].copy_from_slice(b"p");
            let n = assemble_flood_frame(
                scope.as_ref(),
                &mut out,
                PayloadType::TxtMsg,
                zero_path(),
                1,
            );
            let parsed = parse_rx_frame_header(&out[..n]).expect("well-formed frame");
            assert_eq!(parsed.hop_count, 0);
            assert_eq!(parsed.hash_size, 2);
            assert_eq!(&out[parsed.payload_off..n], b"p");
        }
    }
}
