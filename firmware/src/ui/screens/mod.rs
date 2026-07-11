// SPDX-License-Identifier: GPL-3.0-only
//! Screen module hub.
//!
//! Each screen module below (`unprovisioned`, `contact_list`, etc.) exposes a
//! Slint component defined with the `slint!{}` macro plus a thin Rust wrapper
//! struct. The component is created once and held by the wrapper; only the
//! **active** wrapper's component is rendered.
//!
//! # Navigation
//!
//! Navigation state itself does NOT live here. `ui::mod::ActiveScreen` (an
//! enum owning the live component instance per variant, replaced wholesale on
//! navigation) is the single source of truth for "what screen is showing" —
//! see its doc comment for why: an earlier design kept a lightweight
//! stack-of-tags (`ScreenState`/`Screen`/`PinReason`) here that never held an
//! actual Slint component, which left the software renderer with nothing to
//! draw on navigation (the gear→PIN-pad blank-display bug). That stack
//! abstraction was replaced outright by `ActiveScreen` and is not reinstated
//! here.

pub mod unprovisioned;
pub mod contact_list;
pub mod message_view;
pub mod compose;
pub mod pin_entry;
pub mod admin_menu;
pub mod gps_status;
pub mod splash;

pub use unprovisioned::UnprovisionedScreen;
pub use contact_list::ContactListScreen;
pub use message_view::MessageViewScreen;
pub use compose::ComposeScreen;
pub use pin_entry::PinEntryScreen;
pub use admin_menu::AdminMenuScreen;
pub use gps_status::GpsStatusScreen;
pub use splash::SplashScreen;
