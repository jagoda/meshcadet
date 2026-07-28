// SPDX-License-Identifier: GPL-3.0-only
//! Pure detection logic backing `provisioning_server::run`'s diagnostics-only
//! raw-RX hex dump (`--features diagnostics`).
//!
//! That dump exists purely for bring-up debugging (seeing non-frame bytes —
//! banner text, line noise — before the host CLI's first provisioning frame
//! arrives). It must never print raw channel-secret bytes: `ADD_CHANNEL` and
//! `DEL_CHANNEL` are the only two provisioning-frame types whose payload
//! carries the secret verbatim (see `AddChannelPayload`/`DelChannelPayload`
//! in `protocol::provisioning`).
//!
//! This is deliberately factored out of `firmware/src/provisioning_server.rs`
//! (unlike `gps::hex_dump_tail`'s hex-formatting, which stays local because
//! it is trivial cosmetics) because this function *is* the security decision
//! the whole raw-RX redaction depends on — it earns a host-run regression
//! test, which nothing under the detached `xtensa-esp32s3-espidf` firmware
//! workspace ever gets (`firmware`'s `[[bin]]` sets `harness = false`; see
//! this crate's top-level doc comment).

#[cfg(feature = "diagnostics")]
use protocol::provisioning::{FRAME_ADD_CHANNEL, FRAME_DEL_CHANNEL, PROV_MAGIC};

/// True if `buf` contains a `PROV_MAGIC` + `ADD_CHANNEL`/`DEL_CHANNEL` frame
/// header starting anywhere within it.
///
/// Scans the whole buffer rather than only checking offset 0: a
/// not-yet-synced `rx_buf` can carry leading non-frame bytes (banner text,
/// line noise) ahead of a legitimate frame arriving in the very same
/// non-blocking read, so the frame's header need not sit at the buffer's
/// front for its (secret-bearing) payload to already be inside the tail
/// window a diagnostic hex dump would print.
///
/// Once true for a given `rx_buf` state, the caller should keep treating
/// every subsequent read against that same buffer as secret-bearing too,
/// until the frame is consumed — a large `ADD_CHANNEL`/`DEL_CHANNEL` frame
/// routinely arrives split across several non-blocking reads, and every one
/// of those reads' tail windows can land inside the secret payload.
#[cfg(feature = "diagnostics")]
pub fn buffer_holds_secret_bearing_frame(buf: &[u8]) -> bool {
    if buf.len() < 3 {
        return false;
    }
    buf.windows(3).any(|w| {
        w[0] == PROV_MAGIC[0]
            && w[1] == PROV_MAGIC[1]
            && matches!(w[2], FRAME_ADD_CHANNEL | FRAME_DEL_CHANNEL)
    })
}

#[cfg(all(test, feature = "diagnostics"))]
mod tests {
    use super::*;
    use protocol::provisioning::{FRAME_ADD_CONTACT, FRAME_QUERY_STATUS};

    #[test]
    fn empty_and_short_buffers_are_never_secret_bearing() {
        assert!(!buffer_holds_secret_bearing_frame(&[]));
        assert!(!buffer_holds_secret_bearing_frame(&PROV_MAGIC));
    }

    #[test]
    fn add_channel_header_at_offset_zero_is_detected() {
        let buf = [PROV_MAGIC[0], PROV_MAGIC[1], FRAME_ADD_CHANNEL, 0xAA, 0xBB];
        assert!(buffer_holds_secret_bearing_frame(&buf));
    }

    #[test]
    fn del_channel_header_at_offset_zero_is_detected() {
        let buf = [PROV_MAGIC[0], PROV_MAGIC[1], FRAME_DEL_CHANNEL, 0xAA, 0xBB];
        assert!(buffer_holds_secret_bearing_frame(&buf));
    }

    #[test]
    fn header_preceded_by_unsynced_garbage_is_still_detected() {
        // The exact gap a naive "check only rx_buf[0..3]" would miss: banner
        // text / line noise ahead of a legitimate frame arriving in the same
        // non-blocking read.
        let mut buf = b"garbage-before-sync\n".to_vec();
        buf.extend_from_slice(&[PROV_MAGIC[0], PROV_MAGIC[1], FRAME_ADD_CHANNEL, 0x01]);
        assert!(buffer_holds_secret_bearing_frame(&buf));
    }

    #[test]
    fn non_secret_frame_types_are_not_flagged() {
        let buf = [PROV_MAGIC[0], PROV_MAGIC[1], FRAME_ADD_CONTACT, 0xAA, 0xBB];
        assert!(!buffer_holds_secret_bearing_frame(&buf));
        let buf = [PROV_MAGIC[0], PROV_MAGIC[1], FRAME_QUERY_STATUS];
        assert!(!buffer_holds_secret_bearing_frame(&buf));
    }

    #[test]
    fn magic_without_a_following_type_byte_is_not_flagged() {
        assert!(!buffer_holds_secret_bearing_frame(&[
            PROV_MAGIC[0],
            PROV_MAGIC[1]
        ]));
    }

    #[test]
    fn lone_magic_bytes_scattered_in_noise_do_not_false_positive() {
        // A single PROV_MAGIC[0] byte appearing in unrelated noise, not
        // followed by PROV_MAGIC[1], must not trigger redaction.
        let buf = [PROV_MAGIC[0], 0x00, 0x00, FRAME_ADD_CHANNEL];
        assert!(!buffer_holds_secret_bearing_frame(&buf));
    }
}
