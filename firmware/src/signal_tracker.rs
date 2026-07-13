// SPDX-License-Identifier: GPL-3.0-only
//! Repeater signal-strength tracker (ADR-0010). Pure Rust, no ESP-IDF
//! dependency — the whole module lives in [`firmware_core::signal_tracker`]
//! so its tests execute under `cargo test --workspace` (this crate is a
//! detached, cross-compiled workspace — see `Cargo.toml`'s doc comment — so
//! a `#[cfg(test)]` block written here would type-check but never run).
//! This is a thin re-export shim so every call site
//! (`crate::signal_tracker::SignalTracker`, `signal_tracker::SignalLevel`,
//! …) resolves unchanged. See `docs/adr/0005-firmware-core-extraction.md`
//! and `docs/adr/0010-signal-meter.md`.
pub use firmware_core::signal_tracker::*;
