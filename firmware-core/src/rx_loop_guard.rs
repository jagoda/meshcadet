// SPDX-License-Identifier: GPL-3.0-only
//! Shared `TruncatedFrame`-arm RX-buffer-full flush guard for
//! `firmware/src/admin_server.rs`'s and `firmware/src/provisioning_server.rs`'s
//! frame-receive loops.
//!
//! # The hazard this guards against
//!
//! Both loops read from a USB-Serial-JTAG driver ring buffer that carries
//! ONLY host→device bytes (`usb_serial_jtag_read_bytes`, draining the
//! driver's RX ring installed at boot). **An earlier draft of this guard's
//! rationale claimed the hazard was a false `PROV_MAGIC` match inside this
//! device's own boot-time log noise arriving inside its own RX stream —
//! that mechanism is directionally impossible and is retracted:** this
//! device's `log::info!`/`log::warn!` output leaves over a separate VFS TX
//! path with no loopback anywhere in this firmware, so a byte the device
//! writes out can never reappear in a buffer the device reads from — traced
//! and refuted by re-reading the driver install/read/write call graph
//! directly, not inferred.
//!
//! The real hazard is a legitimate, HOST-sent frame that is oversized or
//! desynced: its two length bytes decode to a `plen` bigger than
//! `rx_buf_len` will ever hold. That candidate can *never* resolve:
//! `find_magic_start` re-confirms the same match at offset 0 every
//! iteration (it trusts any two-byte magic match unconditionally), so
//! `rx_len` latches at `rx_buf_len` once the read-gate (`if rx_len <
//! rx_buf_len`) stops accepting new bytes — and from there the loop spins
//! on resync/decode forever with no `delay_ms` in the `TruncatedFrame` arm,
//! starving every future host command, including a fresh `QUERY_STATUS`
//! from a *new* connection, until a physical reset re-zeroes the buffer.
//! This failure mechanism (buffer latched full, loop starved) is real and
//! source-confirmed regardless of the retraction above — only the claim
//! about *how* the buffer reliably gets stuck full in practice was wrong.
//!
//! `provisioning_server.rs` has carried the equivalent guard since this
//! repo's import commit; `admin_server.rs` never had it, and there is no
//! identified commit that removed it or introduced a regression window —
//! this is an omission that existed for as long as the file has, not a
//! recently-drifted copy. Both loops now call this one function from their
//! `TruncatedFrame` arm instead of each hand-rolling the same `if rx_len >=
//! RX_BUF_LEN { rx_len = 0 }` comparison, so a future edit to either loop
//! cannot silently drop the guard out of one copy while leaving it in the
//! other.

/// If `rx_len` has reached (or, defensively, exceeded) `rx_buf_len` with no
/// decodable frame in hand, reset `*rx_len` to `0` so the loop can resync
/// from a clean buffer on the next read. Returns `true` if it flushed (the
/// caller should log a warning); `false` if there is still room to read more
/// bytes before giving up on the current candidate (the ordinary case).
///
/// Call this from the `TruncatedFrame` arm of an RX frame-receive loop only —
/// it is specifically the "buffer full with no valid frame" escape valve, not
/// a general-purpose buffer helper.
pub fn flush_if_rx_buffer_full(rx_len: &mut usize, rx_buf_len: usize) -> bool {
    if *rx_len >= rx_buf_len {
        *rx_len = 0;
        true
    } else {
        false
    }
}

/// Upper bound on any legitimate HOST→DEVICE command frame payload —
/// mirrors, on this side of the link, the same guard `host/src/session.rs`'s
/// `MAX_VALID_FRAME_PAYLOAD_LEN` and `site/provisioner/session.js`'s
/// `MAX_VALID_FRAME_PAYLOAD_LEN` already apply to the opposite direction
/// (DEVICE→HOST response payloads). `FRAME_ADD_ROOM`'s payload — the widest
/// request this protocol defines (`protocol::provisioning::encode_add_room`:
/// `pubkey(32) + pw_len(1) + guest_password(<=MAX_ROOM_PASSWORD_LEN) +
/// name_len(1) + name(<=MAX_NAME_LEN)`) — is ahead of `FRAME_ADD_CHANNEL`
/// and `FRAME_ADD_CONTACT`. Computed from the same protocol constants the
/// encoder itself is built from, rather than hand-copied as a bare number,
/// so a future change to either constant cannot silently leave this guard's
/// own ceiling stale.
pub const MAX_VALID_REQUEST_PAYLOAD_LEN: usize = 32
    + 1
    + protocol::provisioning::MAX_ROOM_PASSWORD_LEN
    + 1
    + protocol::provisioning::MAX_NAME_LEN;

/// Whether the candidate frame currently at the front of `rx_buf` (already
/// synced to a confirmed two-byte magic match by the caller's own
/// `find_magic_start`) carries a `plen` too large to ever be a real
/// request. Returns `false` (nothing to decide yet) if fewer than 5 bytes
/// are buffered — the length field isn't readable yet.
///
/// Why this matters: without this check, an oversized or desynced `plen`
/// (a corrupted length field, or a false magic match landing on the wrong
/// two bytes) is indistinguishable from a genuine, merely-not-fully-arrived
/// frame — `decode_frame` reports both as `TruncatedFrame` and the loop
/// just keeps reading, waiting for `7 + plen` bytes to show up. For an
/// oversized `plen` that wait can never resolve; the candidate only gets
/// dislodged once `rx_len` reaches `rx_buf_len` and
/// [`flush_if_rx_buffer_full`] resets the whole buffer to empty — which
/// also discards any legitimate frame that arrived right behind the bad
/// one. Calling this first, from the same `TruncatedFrame` arm, and
/// resyncing one byte at a time (like a `BadMagic`/`CrcMismatch` result)
/// instead of waiting, means a desynced candidate is dislodged immediately
/// rather than after absorbing up to `rx_buf_len` bytes of good traffic
/// behind it.
pub fn oversized_plen(rx_buf: &[u8], max_valid_payload_len: usize) -> bool {
    if rx_buf.len() < 5 {
        return false;
    }
    let plen = rx_buf[3] as usize | ((rx_buf[4] as usize) << 8);
    plen > max_valid_payload_len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_capacity_does_not_flush() {
        let mut rx_len = 511;
        assert!(!flush_if_rx_buffer_full(&mut rx_len, 512));
        assert_eq!(
            rx_len, 511,
            "must not touch rx_len when there is still room"
        );
    }

    #[test]
    fn exactly_at_capacity_flushes() {
        let mut rx_len = 512;
        assert!(flush_if_rx_buffer_full(&mut rx_len, 512));
        assert_eq!(rx_len, 0);
    }

    #[test]
    fn defensively_flushes_if_somehow_over_capacity() {
        // Should never happen (the read-gate stops accepting bytes once
        // `rx_len == rx_buf_len`), but the guard must not depend on that
        // invariant holding exactly to still escape a stuck loop.
        let mut rx_len = 600;
        assert!(flush_if_rx_buffer_full(&mut rx_len, 512));
        assert_eq!(rx_len, 0);
    }

    #[test]
    fn empty_buffer_never_flushes() {
        let mut rx_len = 0;
        assert!(!flush_if_rx_buffer_full(&mut rx_len, 512));
        assert_eq!(rx_len, 0);
    }

    // ── oversized_plen ────────────────────────────────────────────────────

    fn candidate(plen: u16) -> [u8; 5] {
        let [lo, hi] = plen.to_le_bytes();
        // magic(2) + type(1) + len_lo(1) + len_hi(1) — enough for the guard
        // to read the length field; the magic/type bytes themselves are
        // irrelevant to this function (the caller already confirmed them).
        [b'M', b'C', 0, lo, hi]
    }

    #[test]
    fn fewer_than_5_bytes_is_undecided() {
        assert!(!oversized_plen(
            &candidate(9999)[..4],
            MAX_VALID_REQUEST_PAYLOAD_LEN
        ));
    }

    #[test]
    fn max_valid_payload_is_not_oversized() {
        assert!(!oversized_plen(
            &candidate(MAX_VALID_REQUEST_PAYLOAD_LEN as u16),
            MAX_VALID_REQUEST_PAYLOAD_LEN
        ));
    }

    #[test]
    fn one_byte_past_max_is_oversized() {
        assert!(oversized_plen(
            &candidate(MAX_VALID_REQUEST_PAYLOAD_LEN as u16 + 1),
            MAX_VALID_REQUEST_PAYLOAD_LEN
        ));
    }

    #[test]
    fn ascii_log_noise_reads_as_oversized() {
        // A false "MC" match landing inside ASCII log text: the two bytes at
        // the length-field offset are ordinary printable characters, whose
        // high byte alone (>= 0x20) already pushes plen well past any real
        // payload.
        let noise = *b"MCxes"; // len_hi = b's' = 0x73
        assert!(oversized_plen(&noise, MAX_VALID_REQUEST_PAYLOAD_LEN));
    }
}
