// SPDX-License-Identifier: GPL-3.0-only
//! Pure-logic halves of `firmware`'s Slint-backed screens.
//!
//! Each screen's `slint::slint! { ... }` markup and its Rust-side wrapper
//! struct depend on Slint and stay in `firmware/src/ui/screens/`; only the
//! plain-data formatting helpers move here so their tests execute under
//! `cargo test --workspace`.

pub mod gps_status;
