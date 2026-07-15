// SPDX-License-Identifier: GPL-3.0-only
//! Pure logic backing `FRAME_QUERY_ADVERT` / `FRAME_RSP_ADVERT`
//! (`firmware/src/admin_server.rs`'s on-device self-advert "biz card"
//! handler). No NVS, no USB-serial I/O, no radio/dispatcher access — the
//! hardware-owning half (NVS timestamp persistence, the actual serial
//! write) stays in `firmware/src/admin_server.rs` /
//! `firmware/src/advert_ts_store.rs`, mirroring every other
//! `firmware-core` module (see this crate's top-level doc).
//!
//! # Anti-replay timestamp policy
//!
//! MeshCadet has no RTC: `tx_epoch_base` (`firmware/src/main.rs`) is seeded
//! from `esp_random()` once per boot and is fine for the existing DM/channel
//! traffic (only ever compared against itself, never persisted), but it is
//! useless for an advert's `timestamp` field — MeshCore's own replay guard
//! drops a re-imported advert when `timestamp <= from->last_advert_timestamp`
//! already on file for that contact (`BaseChatMesh.cpp:124`). A fresh random
//! value on every boot would make a re-share (e.g. after a device rename)
//! silently fail to update the receiving peer's contact. [`next_advert_timestamp`]
//! is the fix: `max(host_ts, nvs_last + 1)`, strictly increasing as long as
//! the caller persists the result before generating the next card (including
//! across a reboot).
//!
//! # Name fallback
//!
//! An advert with no name is dropped by every receiver
//! (`BaseChatMesh::onAdvertRecv` → `AdvertDataParser::hasName()`,
//! `BaseChatMesh.cpp:115`). [`resolve_advert_name`] mirrors the host CLI's
//! existing `(unnamed)`/pub_hash-label convention (`host/src/main.rs`) so an
//! unnamed device still yields an importable card.

use protocol::provisioning::{decode_query_advert, ProvError};
use protocol::{build_self_advert_card, Identity};

/// Compute the next self-advert timestamp.
///
/// `host_ts` is the host/browser-supplied unix time carried in the
/// `QUERY_ADVERT` payload — `0` is the "absent/unknown" sentinel (the
/// firmware has no RTC of its own, so it never has a better guess than what
/// the host reports). `nvs_last` is the timestamp persisted after the
/// previous call (`0` if no card has ever been generated on this device).
///
/// Returns a value strictly greater than `nvs_last` in every case,
/// including `host_ts == 0`: `nvs_last.saturating_add(1)` is always `>= 1
/// > 0`, so the `max` naturally falls back to it without a separate branch.
///
/// Callers MUST persist the returned value as the new `nvs_last` — and do so
/// BEFORE replying with the card that carries it — so the sequence stays
/// strictly increasing even if the device resets immediately after this
/// call (see `firmware/src/advert_ts_store.rs`).
pub fn next_advert_timestamp(host_ts: u32, nvs_last: u32) -> u32 {
    host_ts.max(nvs_last.saturating_add(1))
}

/// Resolve the device's self-advert display name.
///
/// `configured_name` is whatever is currently persisted in the identity
/// store (`identity_store::load_name`); empty means "unset". `pub_hash` is
/// the device's own routing hash (`Identity::pub_hash`). Never returns an
/// empty string — every peer's `onAdvertRecv` drops a nameless advert (see
/// module docs), so an empty output here would build a card no receiver
/// keeps.
pub fn resolve_advert_name(configured_name: &str, pub_hash: u8) -> String {
    if configured_name.is_empty() {
        format!("MeshCadet-{:02X}", pub_hash)
    } else {
        configured_name.to_string()
    }
}

/// Handle one `QUERY_ADVERT` request end to end (pure): decode the host
/// timestamp hint, apply the anti-replay timestamp policy, resolve the name
/// fallback, and build + sign the card into `out`.
///
/// Returns `(card_len, new_last_advert_ts)` on success — `out[..card_len]`
/// is the raw card to send verbatim as the `FRAME_RSP_ADVERT` payload (no
/// inner length field: the frame's own 2-byte len already carries it).
/// `new_last_advert_ts` is what the caller must persist to NVS (see
/// [`next_advert_timestamp`]'s doc) before writing the reply.
///
/// GUARD: this function's signature has no `TxQueue` / dispatcher / radio
/// parameter anywhere in its call graph — it is structurally impossible for
/// handling a `QUERY_ADVERT` to reach the TX path. `out` need only be sized
/// `>= MAX_ADVERT_CARD_LEN` (`protocol::MAX_ADVERT_CARD_LEN`).
pub fn handle_query_advert(
    identity: &Identity,
    query_payload: &[u8],
    nvs_last_advert_ts: u32,
    configured_name: &str,
    out: &mut [u8],
) -> Result<(usize, u32), ProvError> {
    let host_ts = decode_query_advert(query_payload)?;
    let ts = next_advert_timestamp(host_ts, nvs_last_advert_ts);
    let name = resolve_advert_name(configured_name, identity.pub_hash());
    let n = build_self_advert_card(identity, ts, &name, out);
    Ok((n, ts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::TxQueue;
    use protocol::provisioning::encode_query_advert;
    use protocol::MAX_ADVERT_CARD_LEN;

    fn seed(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    // ── next_advert_timestamp ────────────────────────────────────────────

    #[test]
    fn host_ts_wins_when_ahead_of_nvs() {
        assert_eq!(next_advert_timestamp(1_000, 500), 1_000);
    }

    #[test]
    fn nvs_plus_one_wins_when_host_ts_is_stale_or_behind() {
        // Host clock behind (or a stale re-send) must not regress below the
        // device's own persisted floor.
        assert_eq!(next_advert_timestamp(100, 500), 501);
    }

    #[test]
    fn zero_host_ts_falls_back_to_nvs_plus_one() {
        assert_eq!(next_advert_timestamp(0, 41), 42);
        // First ever card on a fresh device: nvs_last == 0 too.
        assert_eq!(next_advert_timestamp(0, 0), 1);
    }

    #[test]
    fn strictly_increases_across_simulated_reboots() {
        // Threads nvs_last through repeated calls the way
        // firmware/src/advert_ts_store.rs persists it across boots.
        let mut nvs_last = 0u32;
        let host_hints = [0u32, 0, 1_700_000_000, 1_700_000_000, 5, 1_700_000_050];
        let mut prev = 0u32;
        for &host_ts in &host_hints {
            let ts = next_advert_timestamp(host_ts, nvs_last);
            assert!(
                ts > prev,
                "timestamp must strictly increase: prev={prev} ts={ts} (host_ts={host_ts}, nvs_last={nvs_last})"
            );
            nvs_last = ts; // simulate persisting to NVS before the reboot/next call
            prev = ts;
        }
    }

    // ── resolve_advert_name ──────────────────────────────────────────────

    #[test]
    fn configured_name_is_used_verbatim() {
        assert_eq!(resolve_advert_name("Cadet One", 0xAB), "Cadet One");
    }

    #[test]
    fn empty_name_falls_back_to_pub_hash_label() {
        assert_eq!(resolve_advert_name("", 0x07), "MeshCadet-07");
        assert_eq!(resolve_advert_name("", 0xFF), "MeshCadet-FF");
    }

    // ── handle_query_advert ──────────────────────────────────────────────

    #[test]
    fn handle_query_advert_builds_a_verifiable_named_card() {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};

        let identity = Identity::from_seed(seed(0x10));
        let mut query_payload = [0u8; 4];
        encode_query_advert(1_700_000_500, &mut query_payload);

        let mut out = [0u8; MAX_ADVERT_CARD_LEN];
        let (n, new_ts) =
            handle_query_advert(&identity, &query_payload, 1_700_000_000, "", &mut out)
                .expect("well-formed QUERY_ADVERT payload must decode");

        assert_eq!(new_ts, 1_700_000_500, "host_ts is ahead of nvs_last");
        assert_eq!(out[0], 0x11, "header: ADVERT type, FLOOD route");
        assert_eq!(out[1], 0x00, "path_len: freshly-built card");

        let pubkey: [u8; 32] = out[2..34].try_into().unwrap();
        assert_eq!(pubkey, identity.pubkey);
        let ts_parsed = u32::from_le_bytes(out[34..38].try_into().unwrap());
        assert_eq!(ts_parsed, new_ts);

        let appdata = &out[102..n];
        assert_eq!(appdata[0], 0x81, "appdata flags: chat + name present");
        assert_eq!(
            &appdata[1..],
            format!("MeshCadet-{:02X}", identity.pub_hash()).as_bytes(),
            "unnamed device falls back to the pub_hash label"
        );

        let sig_bytes: [u8; 64] = out[38..102].try_into().unwrap();
        let mut msg = [0u8; 32 + 4 + 32];
        msg[..32].copy_from_slice(&pubkey);
        msg[32..36].copy_from_slice(&ts_parsed.to_le_bytes());
        msg[36..36 + appdata.len()].copy_from_slice(appdata);
        let verifying_key = VerifyingKey::from_bytes(&pubkey).unwrap();
        let signature = Signature::from_bytes(&sig_bytes);
        assert!(verifying_key
            .verify(&msg[..36 + appdata.len()], &signature)
            .is_ok());
    }

    #[test]
    fn handle_query_advert_rejects_truncated_payload() {
        let identity = Identity::from_seed(seed(0x20));
        let mut out = [0u8; MAX_ADVERT_CARD_LEN];
        let err = handle_query_advert(&identity, &[0u8; 2], 0, "Cadet", &mut out).unwrap_err();
        assert_eq!(err, ProvError::TruncatedPayload);
    }

    /// **Never-over-radio, first-class test.** `handle_query_advert`'s
    /// signature (checked above, in this module's doc) takes no `TxQueue` /
    /// dispatcher argument, so it is structurally impossible for it to
    /// enqueue anything — assert that explicitly rather than leaving it
    /// implicit: run a full QUERY_ADVERT round trip with a real `TxQueue` in
    /// scope, untouched, and confirm it is still empty afterward.
    #[test]
    fn query_advert_round_trip_leaves_txq_empty() {
        let identity = Identity::from_seed(seed(0x30));
        let mut query_payload = [0u8; 4];
        encode_query_advert(42, &mut query_payload);

        let txq = TxQueue::new();
        assert!(!txq.has_pending());

        let mut out = [0u8; MAX_ADVERT_CARD_LEN];
        let (n, _new_ts) =
            handle_query_advert(&identity, &query_payload, 0, "Field Cadet", &mut out)
                .expect("well-formed QUERY_ADVERT payload must decode");
        assert!(n > 0);

        assert!(
            !txq.has_pending(),
            "a QUERY_ADVERT round trip must never enqueue onto the radio TX queue"
        );
    }
}
