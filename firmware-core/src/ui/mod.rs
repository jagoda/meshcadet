// SPDX-License-Identifier: GPL-3.0-only
//! Pure-logic halves of `firmware`'s Slint-backed screens.
//!
//! Each screen's `slint::slint! { ... }` markup and its Rust-side wrapper
//! struct depend on Slint and stay in `firmware/src/ui/screens/`; only the
//! plain-data formatting helpers move here so their tests execute under
//! `cargo test --workspace`.
//!
//! `keyboard::key_text` is the one exception noted in `keyboard`'s own doc:
//! it feeds Slint's `Key`/`SharedString` types directly, so it stays behind
//! in `firmware/src/ui/keyboard.rs` (still compile-only) rather than pulling
//! `slint` into this crate's dependency graph — see that module's doc for
//! the full reclassification note.

pub mod admin_menu;
pub mod compose;
pub mod contact_list;
pub mod gps_status;
pub mod keyboard;
pub mod message_view;
pub mod theme;
pub mod touch;
