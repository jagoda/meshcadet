// SPDX-License-Identifier: GPL-3.0-only
//! PIN-gated on-device admin menu — runtime toggle logic.
//!
//! Pure Rust, no ESP-IDF dependency — the whole module now lives in
//! [`firmware_core::pin_menu`] so its tests execute under `cargo test
//! --workspace` (this crate is a detached, cross-compiled workspace — see
//! `Cargo.toml`'s doc comment — so `#[cfg(test)]` blocks written here would
//! type-check but never run). This is a thin re-export shim so every existing
//! call site (`crate::pin_menu::verify_pin`, `pin_menu::RuntimeSettings`, …)
//! keeps resolving unchanged. See `docs/adr/0005-firmware-core-extraction.md`.
pub use firmware_core::pin_menu::*;
