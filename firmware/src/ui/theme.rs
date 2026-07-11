// SPDX-License-Identifier: GPL-3.0-only
//! RGB565 twins of the Slint `Theme` global (`ui/theme.slint`).
//!
//! The whole module (the `rgb565` packer, the palette constants, and their
//! tests) is plain Rust with no Slint or ESP-IDF dependency, so it now lives
//! in [`firmware_core::ui::theme`] so its tests execute under `cargo test
//! --workspace` (this crate is a detached, cross-compiled workspace — see
//! `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block written here would
//! type-check but never run). `pub use firmware_core::ui::theme::*;` below
//! re-exports the pure half so every existing call site (`theme::OK`,
//! `crate::ui::theme::rgb565`-derived constants, …) resolves unchanged. See
//! `docs/adr/0005-firmware-core-extraction.md`.
//!
//! `#[allow(unused_imports)]`: same "kept for a documented future consumer,
//! nothing reads it today" rationale each constant's own `#[allow(dead_code)]`
//! carried before the move (see this module's doc in `firmware-core` for the
//! full writeup) — `firmware` is a `[[bin]]` crate, so this re-export's
//! pub-ness alone doesn't suppress the lint the way it would in a lib crate.
#[allow(unused_imports)]
pub use firmware_core::ui::theme::*;
