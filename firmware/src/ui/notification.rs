// SPDX-License-Identifier: GPL-3.0-only
//! Notification model — visual + audible alerts, per-event configurable.
//!
//! Pure Rust, no ESP-IDF/Slint dependency — the whole model now lives in
//! [`firmware_core::notification`] so its tests execute under `cargo test
//! --workspace` (this crate is a detached, cross-compiled workspace — see
//! `Cargo.toml`'s doc comment — so `#[cfg(test)]` blocks written here would
//! type-check but never run). This is a thin re-export shim so every existing
//! call site (`notification::NotifDispatcher`, `notification::ToneBurst`, …)
//! keeps resolving unchanged. The I2S buzzer playback (`ui::BuzzerDriver`)
//! that actually plays the [`firmware_core::notification::ToneBurst`]
//! sequences stays here — it owns real hardware.
//! See `docs/adr/0005-firmware-core-extraction.md`.
pub use firmware_core::notification::*;
