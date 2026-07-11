// SPDX-License-Identifier: GPL-3.0-only
//! MeshCadet host library — USB-serial provisioning session layer.
//!
//! Exposes `transport` and `session` as public modules so the `meshcadet`
//! binary and integration tests share the same implementation.

pub mod history_format;
pub mod session;
pub mod transport;
