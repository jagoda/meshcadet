// SPDX-License-Identifier: GPL-3.0-only
//! GT911 touch controller — pure release-by-silence debounce decision.
//!
//! The GT911 I2C driver (`TouchDriver`, plus the `TouchPoint`/`TouchEvent`/
//! `TouchKind` types it hands back) stays in `firmware/src/ui/touch.rs` —
//! it owns real hardware I/O; only the plain-data debounce predicate below
//! moves here so its tests execute under `cargo test --workspace` (this
//! crate is a detached, cross-compiled workspace — see `Cargo.toml`'s doc
//! comment — so a `#[cfg(test)]` block written there would type-check but
//! never run). See `docs/adr/0005-firmware-core-extraction.md`.

/// Pure decision function for the release-by-silence debounce in
/// `TouchDriver::poll_event`'s "buffer not ready" arm — extracted so this
/// regression-causing logic is covered by a host-runnable unit test
/// independent of the I2C/hardware stack, same rationale as
/// `touch_wake_transition` in `firmware/src/ui/mod.rs`.
///
/// Returns `true` once `now_ms` is at least `debounce_ms` past
/// `last_update_ms` (saturating, so a clock that hasn't advanced — or has
/// wrapped — never spuriously asserts a release).
pub fn silence_implies_release(now_ms: u64, last_update_ms: u64, debounce_ms: u64) -> bool {
    now_ms.saturating_sub(last_update_ms) >= debounce_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── silence_implies_release ─────────────────────────────────────────────
    // Pure debounce arithmetic — see the function's doc for why isolating it
    // from the I2C/hardware stack matters here.

    #[test]
    fn silence_implies_release_false_within_debounce_window() {
        // This is the exact regression scenario: back-to-back polls within
        // the same `step()`'s drain loop are microseconds apart, i.e.
        // `now_ms == last_update_ms` — must NOT infer a release.
        assert!(!silence_implies_release(1_000, 1_000, 40));
        assert!(!silence_implies_release(1_020, 1_000, 40));
    }

    #[test]
    fn silence_implies_release_true_once_debounce_elapsed() {
        assert!(silence_implies_release(1_040, 1_000, 40));
        assert!(silence_implies_release(5_000, 1_000, 40));
    }

    #[test]
    fn silence_implies_release_never_fires_on_backwards_clock() {
        // `saturating_sub` must not wrap into a huge elapsed value if
        // `now_ms` is somehow behind `last_update_ms`.
        assert!(!silence_implies_release(500, 1_000, 40));
    }
}
