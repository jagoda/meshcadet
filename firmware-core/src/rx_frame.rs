// SPDX-License-Identifier: GPL-3.0-only
//! Pure, host-testable RX frame header parsing shared by
//! `firmware/src/main.rs::on_receive`.
//!
//! This module is a thin re-export of `protocol::frame`, where the shared
//! parse now lives — moved there so `protocol::dedup::packet_payload_view`
//! (called on EVERY received frame, ahead of the allowlist, from
//! `firmware_core::dispatcher::DuplicateFilter`) can reuse the exact same
//! transport-code branch `on_receive` uses, rather than hand-rolling a
//! second, independently-drifting copy of the offset arithmetic.
//! `firmware-core` depends on `protocol` (never the reverse), so the shared
//! logic has to live on the `protocol` side of that edge — see
//! `protocol::frame`'s module doc for the full mechanism and wire-framing
//! reference. This module keeps `firmware_core::rx_frame::*` as a stable
//! import path for `firmware/src/main.rs`, which is unchanged.

pub use protocol::frame::{parse_rx_frame_header, RxFrameError, RxFrameHeader};

#[cfg(test)]
mod tests {
    // Smoke test only — the exhaustive coverage (transport-code branching,
    // every `RxFrameError` variant) now lives with the implementation in
    // `protocol::frame`'s own test module, exercised by `cargo test -p
    // protocol`. This confirms the re-export is wired correctly.
    use super::*;

    #[test]
    fn reexport_reaches_protocol_frame_impl() {
        assert_eq!(parse_rx_frame_header(&[]), Err(RxFrameError::Empty));
    }
}
