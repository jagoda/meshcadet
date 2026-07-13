// SPDX-License-Identifier: GPL-3.0-only
//! Firmware logic decoupled from `esp-idf-*`/Slint — pure Rust, `std`,
//! depends only on [`protocol`].
//!
//! # Why this crate exists
//!
//! `firmware/` is a **detached** Cargo workspace (its own `[workspace]` table
//! in `firmware/Cargo.toml`) because it cross-compiles for
//! `xtensa-esp32s3-espidf` under the Espressif `esp` toolchain — see the root
//! `Cargo.toml`'s doc comment. That keeps the fast host lane (`cargo test`,
//! `clippy`, `fmt`) free of the ESP-IDF sysroot, but it also means every
//! `#[cfg(test)]` block written *inside* `firmware/src/**` type-checks but
//! never **executes**: `firmware`'s `[[bin]]` sets `harness = false`, and
//! nothing at the repo root ever builds the detached workspace. A large and
//! growing share of firmware logic is, in fact, pure Rust with no hardware
//! dependency at all — dispatcher/dedup logic, the PIN-menu state machine,
//! the notification model, NMEA/battery/config parsing and codecs — and its
//! tests sat compile-only, never run, for as long as the detached-workspace
//! split existed.
//!
//! `firmware-core` follows the *exact* pattern [`protocol`] already
//! established: a root-workspace member that is **also** a `path` dependency
//! of the detached `firmware` crate. It compiles for host (so its tests
//! execute under `cargo test --workspace`) **and** for `xtensa-esp32s3-espidf`
//! (as a firmware dependency) — plain Rust + `std` has no target-specific
//! surface to break either way.
//!
//! Each module here is the pure-logic half of a firmware module that also has
//! an impure, hardware-owning half left behind in `firmware/src/`:
//!
//! | This crate       | Firmware hardware/Slint half (stays in `firmware/src/`)        |
//! |-------------------|----------------------------------------------------------------|
//! | [`dispatcher`]    | *(none — this module is already 100% pure)*                    |
//! | [`pin_menu`]      | *(none — this module is already 100% pure)*                    |
//! | [`notification`]  | `ui::BuzzerDriver` (I2S tone playback)                          |
//! | [`ui::gps_status`]| `ui::screens::gps_status::GpsStatusScreen` (the `slint!{}` view) |
//! | [`ui::contact_list`] | `ui::screens::contact_list::{ContactListScreen, ContactItem}` (the `slint!{}` view) |
//! | [`ui::admin_menu`] | `ui::screens::admin_menu::AdminMenuScreen` (the `slint!{}` view) |
//! | [`ui::message_view`] | `ui::screens::message_view::{MessageViewScreen, MessageItem}` (the `slint!{}` view) |
//! | [`ui::compose`]   | `ui::screens::compose::ComposeScreen` (the `slint!{}` view)     |
//! | [`ui::touch`]     | `ui::touch::TouchDriver` (GT911 I2C driver)                     |
//! | [`ui::keyboard`]  | `ui::keyboard::KeyboardDriver` + `key_text` (Slint `Key`-coupled; see that module's doc) |
//! | [`ui::theme`]     | *(none — this module is already 100% pure)*                    |
//! | [`gps`]           | `gps::GpsDriver` (UART1, baud probing, NVS baud cache)          |
//! | [`battery`]       | `battery::BatteryDriver` (ADC1 sampling)                        |
//! | [`runtime_settings_store`] | `runtime_settings_store::{load, save}` (`EspNvs`)     |
//! | [`signal_tracker`] | *(rx-tap in `firmware/src/main.rs` + a Slint `SignalMeter` widget — the UI child of the `meshcadet-signal-meter` campaign; not yet present)* |
//!
//! `firmware/src/<module>.rs` re-consumes the moved logic via a thin
//! `pub use firmware_core::<module>::*;` shim, so every existing call site
//! (`crate::gps::FixState`, `crate::dispatcher::DuplicateFilter`, …) resolves
//! completely UNCHANGED — this is a behavior-preserving move, not a rewrite.
//!
//! See `docs/adr/0005-firmware-core-extraction.md` for the full design
//! record. This crate is the first of several planned extraction passes
//! moving more of `firmware`'s pure logic out from behind the detached
//! workspace boundary.

pub mod battery;
pub mod dispatcher;
pub mod gps;
pub mod notification;
pub mod pin_menu;
pub mod runtime_settings_store;
pub mod signal_tracker;
pub mod ui;
