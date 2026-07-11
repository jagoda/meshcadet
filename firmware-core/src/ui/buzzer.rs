// SPDX-License-Identifier: GPL-3.0-only
//! I2S "buzzer" — pure square-wave sample generator.
//!
//! `BuzzerDriver` (I2S TX channel ownership, real hardware streaming) stays
//! in `firmware/src/ui/mod.rs` — it owns the `I2sDriver<I2sTx>` handle; only
//! the duty-cycle arithmetic below moves here so its tests execute under
//! `cargo test --workspace` (this crate is a detached, cross-compiled
//! workspace — see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block
//! written there would type-check but never run). See
//! `docs/adr/0005-firmware-core-extraction.md`.

/// Pure square-wave sample generator for `sample_index` of a `freq_hz` tone
/// at `sample_rate_hz`, returning `+amplitude`/`-amplitude` (or `0` for
/// `freq_hz == 0`, used for the silence gap between bursts).
///
/// Extracted as a standalone function (no I2S/hardware dependency) so the
/// part of `BuzzerDriver` most likely to carry a subtle bug — the duty-cycle
/// arithmetic — has a host-checkable unit test independent of the
/// esp-idf-hal I2S stack.
///
/// `.max(1)` guards the `sample_rate_hz / freq_hz` division against a
/// `freq_hz == 0` (silence) caller; `.max(2)` guards the `% samples_per_cycle`
/// below against a division/modulo by zero if `freq_hz` ever exceeded the
/// Nyquist limit (`sample_rate_hz`) — not reachable from the current
/// `notification::tone_sequence()` table (max 1320 Hz vs. an 8 kHz sample
/// rate), but cheap to make unconditionally safe.
pub fn square_wave_sample(
    sample_index: u32,
    freq_hz: u32,
    sample_rate_hz: u32,
    amplitude: i16,
) -> i16 {
    if freq_hz == 0 {
        return 0;
    }
    let samples_per_cycle = (sample_rate_hz / freq_hz.max(1)).max(2);
    if (sample_index % samples_per_cycle) < samples_per_cycle / 2 {
        amplitude
    } else {
        -amplitude
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_wave_sample_silence_is_always_zero() {
        for i in [0u32, 1, 2, 100] {
            assert_eq!(square_wave_sample(i, 0, 8_000, 16_384), 0);
        }
    }

    #[test]
    fn square_wave_sample_alternates_high_then_low_per_cycle() {
        // freq_hz=1000 @ sample_rate=8000 => samples_per_cycle = 8: first
        // half of each 8-sample cycle is +amplitude, second half -amplitude.
        let amplitude = 16_384i16;
        let expected = [
            amplitude, amplitude, amplitude, amplitude, -amplitude, -amplitude, -amplitude,
            -amplitude,
        ];
        for (i, &want) in expected.iter().enumerate() {
            assert_eq!(
                square_wave_sample(i as u32, 1_000, 8_000, amplitude),
                want,
                "sample {i} of an 8-sample cycle",
            );
        }
        // Cycle repeats: sample 8 matches sample 0.
        assert_eq!(square_wave_sample(8, 1_000, 8_000, amplitude), amplitude);
    }

    #[test]
    fn square_wave_sample_never_panics_above_nyquist() {
        // freq_hz > sample_rate_hz would drive samples_per_cycle to 0 without
        // the `.max(2)` guard (mod-by-zero panic). Not reachable from the
        // current tone_sequence() table, but must stay safe regardless.
        let _ = square_wave_sample(0, 50_000, 8_000, 16_384);
    }
}
