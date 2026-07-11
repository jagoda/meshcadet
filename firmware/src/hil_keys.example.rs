// SPDX-License-Identifier: GPL-3.0-only
//! EXAMPLE HIL key config — copy to `hil_keys.rs`, then fill in REAL values.
//!
//! ```sh
//! cp firmware/src/hil_keys.example.rs firmware/src/hil_keys.rs
//! # edit firmware/src/hil_keys.rs with the real seed / peer pubkey / channel secret
//! cd firmware && cargo build --features hil --target xtensa-esp32s3-espidf
//! ```
//!
//! `hil_keys.rs` is GITIGNORED (see `/.gitignore`). The real self seed, peer
//! public key, and channel secret are SECRET MATERIAL: NEVER commit `hil_keys.rs`,
//! and never paste real values into logs or chat. The firmware
//! prints only public, on-the-wire bytes (its own pubkey, the 1-byte channel hash,
//! 1-byte routing hashes) — never the seed or channel secret.
//!
//! This module is pulled in by `#[path = "hil_keys.rs"] mod hil_config;` only
//! under `--features hil`.

/// Fixed compiled Ed25519 seed (32 bytes) for THIS MeshCadet node's HIL identity.
///
/// Pinning it in the binary makes the node's public key STABLE across reflash and
/// NVS-erase, so the operator registers the contact once. Generate a random
/// 32-byte value for real use (this all-zero placeholder is NOT a valid identity
/// to rely on). SECRET — never commit the real value.
pub const HIL_SELF_SEED: [u8; 32] = [0u8; 32];

/// The real peer node's Ed25519 PUBLIC key (32 bytes).
///
/// Copy it from the companion app's contact/QR export for the node MeshCadet
/// should DM. Used as the ECDH DM target and as the expected DM source hash.
pub const HIL_PEER_PUBKEY: [u8; 32] = [0u8; 32];

/// The real group-channel secret (32-byte buffer).
///
/// For a 128-bit channel, place the 16-byte secret in the FIRST 16 bytes (leave
/// the rest zero) and set `HIL_CHANNEL_KEY_LEN = 16`. For a 256-bit channel, fill
/// all 32 bytes and set `HIL_CHANNEL_KEY_LEN = 32`. SECRET — never commit the
/// real value.
pub const HIL_CHANNEL_SECRET: [u8; 32] = [0u8; 32];

/// Channel key length in bytes, selecting the channel-hash convention:
///   - `16` → 128-bit channel, hash = `SHA256(secret[0:16])[0]` (most public channels)
///   - `32` → 256-bit channel, hash = `SHA256(secret)[0]`
///
/// Must match the real node's convention EXACTLY or GRP_TXT will not decode.
pub const HIL_CHANNEL_KEY_LEN: usize = 16;
