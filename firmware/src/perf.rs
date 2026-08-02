// SPDX-License-Identifier: GPL-3.0-only
//! On-device superloop timing instrumentation (M0 of the
//! `meshcadet-perf-rearchitecture` design), `--features
//! diagnostics` only. Pure Rust, no ESP-IDF dependency — the whole module
//! lives in [`firmware_core::perf`] so its tests execute under `cargo test
//! --workspace` (this crate is a detached, cross-compiled workspace — see
//! `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block written here would
//! type-check but never run). This is a thin re-export shim so every call
//! site (`crate::perf::PerfRollup`, `perf::per_core_utilization_pct`, …)
//! resolves unchanged. See `docs/adr/0005-firmware-core-extraction.md`.
pub use firmware_core::perf::*;
