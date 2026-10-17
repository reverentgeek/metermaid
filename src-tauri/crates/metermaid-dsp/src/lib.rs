//! ITU-R BS.1770 / EBU R128 loudness, FFT spectrum and test-signal analysis.
//!
//! Pure analysis: no audio I/O, no threads, no UI. MeterMaid's engine thread
//! owns an [`Analyzer`] and feeds it captured frames; other tools can feed it
//! (or [`analyze_offline`]) decoded files or rendered buffers instead.
//!
//! Everything here allocates freely and is **not** realtime-safe — call it from
//! a worker thread, never from an audio callback.

mod analyzer;
mod ltas;
mod noise;
mod offline;
mod window;

pub use analyzer::{Analyzer, Metrics};
pub use ltas::{ltas, Ltas, LTAS_FFT_SIZE};
pub use noise::{gen_brown, gen_pink, noise_reference_curve, NoiseKind, NoiseReference};
pub use offline::{analyze_offline, OfflineReport};
pub use window::{cycle_window, downmix};

/// FFT window size for the display spectrum (power of two).
pub const FFT_SIZE: usize = 4096;
/// Number of log-spaced display bands in [`Metrics::spectrum`].
pub const BANDS: usize = 96;
/// Floor for loudness readouts (LUFS) when below the gate / no signal.
pub const LOUDNESS_FLOOR: f64 = -70.0;
/// Floor for the spectrum (dB).
pub const SPECTRUM_FLOOR: f32 = -90.0;
/// Floor for peak readouts (dBTP) with no signal.
pub const PEAK_FLOOR: f64 = -120.0;

/// Map an optional/non-finite loudness value to a finite floored value
/// (serde_json cannot serialize NaN / infinity).
pub fn clean(v: Option<f64>, floor: f64) -> f64 {
    match v {
        Some(x) if x.is_finite() => x.max(floor),
        _ => floor,
    }
}

/// Convert a linear amplitude to dB, floored (and always finite for serde).
pub fn lin_to_db(lin: f64) -> f64 {
    if lin > 1e-9 {
        (20.0 * lin.log10()).max(PEAK_FLOOR)
    } else {
        PEAK_FLOOR
    }
}

/// Shared signal builders for the crate's golden-signal tests.
#[cfg(test)]
pub(crate) mod test_signals {
    use std::f32::consts::PI;

    /// Interleaved frames for a single-channel (mono) device: a pure tone.
    pub fn mono_sine(freq: f32, amp: f32, secs: f32, sr: u32) -> Vec<f32> {
        let n = (secs * sr as f32) as usize;
        (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / sr as f32).sin())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lin_to_db_maps_levels_and_floors() {
        assert!((lin_to_db(1.0) - 0.0).abs() < 1e-9);
        assert!((lin_to_db(0.5) - (-6.0206)).abs() < 1e-3);
        // Silence and sub-threshold values clamp to the peak floor.
        assert_eq!(lin_to_db(0.0), PEAK_FLOOR);
        assert_eq!(lin_to_db(1e-12), PEAK_FLOOR);
    }

    #[test]
    fn clean_handles_non_finite_and_floor() {
        assert_eq!(clean(Some(-5.0), LOUDNESS_FLOOR), -5.0);
        assert_eq!(clean(Some(f64::NAN), LOUDNESS_FLOOR), LOUDNESS_FLOOR);
        assert_eq!(
            clean(Some(f64::NEG_INFINITY), LOUDNESS_FLOOR),
            LOUDNESS_FLOOR
        );
        assert_eq!(clean(None, LOUDNESS_FLOOR), LOUDNESS_FLOOR);
        // Finite-but-below-floor values are clamped up to the floor.
        assert_eq!(clean(Some(-100.0), LOUDNESS_FLOOR), LOUDNESS_FLOOR);
    }
}
