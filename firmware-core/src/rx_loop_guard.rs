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
}
