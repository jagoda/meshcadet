// site/upgrade-gate.js — DOM-free validation for the Upgrade path's
// update-meta.json gate (ADR-0008 D2/D4, ADR-0009 D2). Pulled out of
// flash.js so this one piece of decision logic — whether a given
// update-meta.json both marks a release upgrade_safe AND has the exact
// shape the Upgrade flash flow needs before it writes a single byte — gets
// plain-node test coverage independent of a browser/DOM. Same pattern
// site/provisioner/validation.js and
// firmware/release-container/generate-update-meta.sh already follow for
// their own safety-critical decision logic.

// The `factory` app partition's fixed flash offset (firmware/partitions.csv,
// 0x10000/65536) — update-meta.json's own app_offset field is expected to
// always equal this (frozen by ADR-0008 D4). A mismatch means either a
// release-pipeline bug or a corrupted/tampered mirror, and app_offset feeds
// directly into a flash write address — refused rather than blindly
// trusted.
export const EXPECTED_APP_OFFSET = 0x10000;

// release.yml's own app-asset naming convention
// (meshcadet-vX.Y.Z-app.bin). app_asset feeds directly into a same-origin
// fetch URL (`firmware/<tag>/<app_asset>`, flash.js's runUpgradeFlash);
// constraining its shape rules out a malformed/tampered value escaping the
// intended firmware/<tag>/ directory (e.g. via "../").
const APP_ASSET_RE = /^meshcadet-v\d+\.\d+\.\d+-app\.bin$/;

/**
 * Whether `meta` (a parsed update-meta.json, or null if none was mirrored /
 * the fetch failed) both marks the release upgrade_safe and has the exact
 * shape the Upgrade flash flow requires. Any other shape — missing fields,
 * wrong types, an app_offset that doesn't match the frozen factory-partition
 * offset, an app_asset that doesn't match the expected naming convention —
 * is treated as "Upgrade not available", the same outward behavior as
 * upgrade_safe:false. A malformed metadata file fails closed (Fresh install
 * only), never open.
 */
export function isValidUpdateMeta(meta) {
  return (
    meta !== null &&
    typeof meta === "object" &&
    meta.upgrade_safe === true &&
    typeof meta.app_asset === "string" &&
    APP_ASSET_RE.test(meta.app_asset) &&
    meta.app_offset === EXPECTED_APP_OFFSET
  );
}
