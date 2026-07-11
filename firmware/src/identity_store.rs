// SPDX-License-Identifier: GPL-3.0-only
//! On-device Ed25519 identity: generate (or load) from ESP-IDF NVS.
//!
//! # Invariants
//! - The 32-byte seed (private key material) NEVER leaves the device.  It is
//!   written to NVS once at first boot and never read back over any interface.
//! - The public key is the node identity; it is freely shareable.
//! - If the NVS key is missing or malformed, a new keypair is generated and
//!   persisted before returning.
//!
//! # NVS layout
//! Namespace : `"mc_id"`
//! Key       : `"seed"` — 32-byte blob (Ed25519 seed)
//! Key       : `"name"` — UTF-8 device display name blob (≤ `MAX_NAME_LEN`
//!             bytes, unpadded).  Absent ⇒ no name set; callers fall back to
//!             a pub_hash-derived label (see `host/src/main.rs`).
//!
//! The device display name lives in this store (not the provisioning config
//! blob in `config_store.rs`) because it is a property of the node's
//! identity, not of the mesh contact/channel provisioning the admin does once
//! per device: it is set and read back the same way regardless of whether
//! the device has been provisioned yet.
//!
//! The NVS default partition must be ≥ 0x6000 bytes (ESP-IDF default); the
//! seed blob uses < 100 bytes including NVS metadata overhead.
//!
//! # Open thread resolution
//! The NVS partition size is the ESP-IDF default (24 576 bytes), which is
//! adequate for the seed plus future provisioning data (M2 provisioning keys,
//! allowlist, radio preset overrides).  No partition table change is needed for M1.

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use protocol::identity::Identity;
use protocol::provisioning::MAX_NAME_LEN;
use rand::rngs::OsRng;

/// ESP-IDF error wrapper.
pub use esp_idf_svc::sys::EspError;

const NVS_NAMESPACE: &str = "mc_id";
const NVS_KEY_SEED: &str = "seed";
const NVS_KEY_NAME: &str = "name";

/// Load the persisted Ed25519 seed from NVS, or generate + persist a new one.
///
/// Callers take ownership of the returned `Identity`; the seed field is
/// sensitive but `Identity` is not `Zeroize`-on-drop in this crate (the
/// hardware has no secure enclave — see ADR-0001 §Security).
pub fn load_or_generate(nvs_partition: EspNvsPartition<NvsDefault>) -> Result<Identity, EspError> {
    let mut nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;

    let mut seed_buf = [0u8; 32];
    // esp-idf-svc 0.50+ get_blob returns Option<&[u8]> (the bytes read), not a length.
    match nvs.get_blob(NVS_KEY_SEED, &mut seed_buf)? {
        Some(bytes) if bytes.len() == 32 => {
            // Found a complete seed → reconstruct identity.
            log::info!("identity: loaded existing keypair from NVS");
            Ok(Identity::from_seed(seed_buf))
        }
        Some(bytes) => {
            // Partial / corrupt write — regenerate.
            let n = bytes.len();
            log::warn!("identity: NVS seed truncated ({} bytes), regenerating", n);
            generate_and_persist(&mut nvs)
        }
        None => {
            // First boot.
            log::info!("identity: no seed in NVS, generating new keypair");
            generate_and_persist(&mut nvs)
        }
    }
}

/// Load the persisted device display name from NVS, if one has been set.
///
/// Returns `(name, name_len)`, zero-padded to `MAX_NAME_LEN` — mirrors the
/// `display_name`/`display_name_len` convention used for contacts/channels.
/// `name_len == 0` means no name has been set (first boot, or explicitly
/// cleared via [`set_name`] with an empty name).
pub fn load_name(
    nvs_partition: EspNvsPartition<NvsDefault>,
) -> Result<([u8; MAX_NAME_LEN], u8), EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    let mut buf = [0u8; MAX_NAME_LEN];
    let len = match nvs.get_blob(NVS_KEY_NAME, &mut buf)? {
        Some(bytes) => bytes.len(),
        None => 0,
    };
    Ok((buf, len as u8))
}

/// Persist a new device display name to NVS, overwriting any previous value.
///
/// `name` longer than `MAX_NAME_LEN` bytes is truncated; callers should
/// reject oversized names before calling this (the wire decode in
/// `protocol::provisioning::decode_set_device_name` already enforces the
/// limit, so a truncation here would only ever be defensive). An empty
/// `name` clears the stored name — the next [`load_name`] then returns
/// `name_len == 0`.
pub fn set_name(nvs_partition: EspNvsPartition<NvsDefault>, name: &[u8]) -> Result<(), EspError> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    let n = name.len().min(MAX_NAME_LEN);
    nvs.set_blob(NVS_KEY_NAME, &name[..n])
}

fn generate_and_persist(nvs: &mut EspNvs<NvsDefault>) -> Result<Identity, EspError> {
    let id = Identity::generate(&mut OsRng);
    // Persist seed; public key is always re-derivable from the seed.
    nvs.set_blob(NVS_KEY_SEED, &id.seed)?;
    log::info!(
        "identity: persisted new keypair; pub_hash=0x{:02x}",
        id.pub_hash()
    );
    Ok(id)
}
