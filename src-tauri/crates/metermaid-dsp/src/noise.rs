//! Noise reference curves (guide overlays) and the noise generators behind
//! them.
//!
//! The UI can draw a pink/brown noise curve behind the spectrum as a
//! tone-shaping guide (issue #9). A theoretical −3/−6 dB per octave line would
//! be misleading here: the spectrum shows the *peak* FFT-bin magnitude per log
//! band (not a PSD), so real noise displays with a different tilt. Instead the
//! guide is calibrated: synthesize the noise, run it through the same
//! `Analyzer`/`spectrum()` pipeline as live audio, and average the per-band
//! response. The same analyzer also measures the signal's integrated loudness,
//! which lets the frontend anchor the curve at the user's target LUFS exactly
//! (a gain of g dB shifts loudness and spectrum dB identically).

use crate::{Analyzer, BANDS, FFT_SIZE, SPECTRUM_FLOOR};

/// Seconds of synthetic noise fed through the analyzer per guide curve. Long
/// enough for a stable gated integrated loudness and ~230 FFT windows at
/// 48 kHz.
const NOISE_SECS: f32 = 10.0;
/// Fixed PRNG seed so guide curves are bit-identical run to run.
const NOISE_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
/// Peak normalization for the synthesized noise (comfortably below clipping;
/// the absolute level is irrelevant — the frontend re-anchors to the target).
const NOISE_PEAK: f32 = 0.25;
/// Brown-noise leaky-integrator corner (Hz) — below the 20 Hz display floor,
/// and expressed as a frequency so the shape is sample-rate independent.
const BROWN_CORNER_HZ: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseKind {
    Pink,
    Brown,
}

impl NoiseKind {
    /// Parse the frontend's guide value ("pink" / "brown").
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "pink" => Ok(NoiseKind::Pink),
            "brown" => Ok(NoiseKind::Brown),
            other => Err(format!("Unknown noise kind: {other}")),
        }
    }
}

/// Calibrated guide curve: the per-band spectrum of synthetic noise at the
/// generated level, plus that same signal's integrated loudness. The frontend
/// anchors the curve by adding (target LUFS − loudness).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct NoiseReference {
    pub bands: Vec<f32>,
    pub loudness: f64,
}

/// xorshift64* — a tiny deterministic PRNG so the guide curves need no `rand`
/// dependency and reproduce bit-identically.
struct XorShift64Star(u64);

impl XorShift64Star {
    /// Next uniform sample in [-1.0, 1.0).
    fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let r = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((r >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

/// Pink noise via Paul Kellet's "economy" 3-pole pinking filter, peak-
/// normalized to [`NOISE_PEAK`]. The coefficients are tuned for 44.1 kHz, so
/// the filter corners shift a little at higher rates — fine by construction:
/// the guide shows whatever the real pipeline displays for this exact signal.
pub fn gen_pink(n: usize) -> Vec<f32> {
    let mut rng = XorShift64Star(NOISE_SEED);
    let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
    let mut out: Vec<f32> = (0..n)
        .map(|_| {
            let white = rng.next_f32();
            b0 = 0.99765 * b0 + white * 0.099_046;
            b1 = 0.963 * b1 + white * 0.296_516_4;
            b2 = 0.57 * b2 + white * 1.052_691_3;
            b0 + b1 + b2 + white * 0.1848
        })
        .collect();
    normalize_peak(&mut out);
    out
}

/// Brown noise via a leaky integrator over white noise (−6 dB/oct above the
/// [`BROWN_CORNER_HZ`] corner), peak-normalized to [`NOISE_PEAK`].
pub fn gen_brown(n: usize, sample_rate: u32) -> Vec<f32> {
    let mut rng = XorShift64Star(NOISE_SEED);
    let leak = (-2.0 * std::f32::consts::PI * BROWN_CORNER_HZ / sample_rate as f32).exp();
    let mut acc = 0.0f32;
    let mut out: Vec<f32> = (0..n)
        .map(|_| {
            acc = leak * acc + rng.next_f32();
            acc
        })
        .collect();
    normalize_peak(&mut out);
    out
}

fn normalize_peak(buf: &mut [f32]) {
    let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        let g = NOISE_PEAK / peak;
        for s in buf.iter_mut() {
            *s *= g;
        }
    }
}

/// Calibrated guide curve for `kind` at `sample_rate`: synthesizes
/// [`NOISE_SECS`] of noise, runs it through the same `Analyzer` used for live
/// metering, and averages the per-band spectrum over all 50%-overlapped FFT
/// windows.
pub fn noise_reference_curve(kind: NoiseKind, sample_rate: u32) -> Result<NoiseReference, String> {
    if !(8_000..=384_000).contains(&sample_rate) {
        return Err(format!("Unsupported sample rate: {sample_rate}"));
    }
    let n = (NOISE_SECS * sample_rate as f32) as usize;
    let noise = match kind {
        NoiseKind::Pink => gen_pink(n),
        NoiseKind::Brown => gen_brown(n, sample_rate),
    };

    let mut a = Analyzer::new();
    a.configure(sample_rate, vec![0], 1)?; // synthetic mono device, channel 0

    // 50%-overlapped snapshots: `spectrum()` reads the last FFT_SIZE samples
    // of the mono ring, so a half-window hop sees every sample twice.
    const HOP: usize = FFT_SIZE / 2;
    let mut acc = [0.0f64; BANDS];
    let mut count = 0u32;
    let mut fed = 0usize;
    for chunk in noise.chunks(HOP) {
        a.process(chunk);
        fed += chunk.len();
        // Skip until the ring holds a full FFT of noise, so the first
        // snapshots aren't diluted by the short, zero-history ring.
        if fed < FFT_SIZE {
            continue;
        }
        for (slot, db) in acc.iter_mut().zip(a.spectrum()) {
            // Average in linear power — the mean energy across windows.
            // Averaging dB values would bias the curve low; floored bands
            // contribute ~1e-9, i.e. nothing.
            *slot += 10f64.powf(db as f64 / 10.0);
        }
        count += 1;
    }
    if count == 0 {
        return Err("Not enough noise for one FFT window".into());
    }

    let bands = acc
        .iter()
        .map(|p| ((10.0 * (p / f64::from(count)).log10()) as f32).max(SPECTRUM_FLOOR))
        .collect();

    let loudness = a
        .ebu
        .as_ref()
        .ok_or("Loudness analyzer unavailable")?
        .loudness_global()
        .map_err(|e| format!("Couldn’t measure the guide’s loudness: {e}"))?;
    if !loudness.is_finite() {
        return Err("Guide loudness measurement was not finite".into());
    }

    Ok(NoiseReference { bands, loudness })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mean level over a band range. Band index math (48 kHz, f_hi = 20 kHz):
    /// band(f) = 96·ln(f/20)/ln(1000) → 100 Hz ≈ 22, 5 kHz ≈ 77, 10 kHz ≈ 86.
    fn band_avg(bands: &[f32], lo: usize, hi: usize) -> f32 {
        bands[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
    }

    #[test]
    fn noise_reference_is_complete_and_deterministic() {
        let a = noise_reference_curve(NoiseKind::Pink, 48_000).unwrap();
        assert_eq!(a.bands.len(), BANDS);
        assert!(a.bands.iter().all(|v| v.is_finite()));
        // Fixed seed → bit-identical curves run to run.
        let b = noise_reference_curve(NoiseKind::Pink, 48_000).unwrap();
        assert_eq!(a.bands, b.bands);
        assert_eq!(a.loudness, b.loudness);
    }

    #[test]
    fn noise_reference_slopes_fall_and_brown_falls_faster() {
        let pink = noise_reference_curve(NoiseKind::Pink, 48_000).unwrap();
        let brown = noise_reference_curve(NoiseKind::Brown, 48_000).unwrap();
        // Band-averaged drop from ~80–130 Hz to ~5–10 kHz. Pink PSD falls
        // ~18.5 dB over that span and brown ~37; the peak-bin banding offsets
        // that by a few dB, so the margins are deliberately loose — they catch
        // a swapped filter or a flat curve, not calibration drift.
        let drop = |r: &NoiseReference| band_avg(&r.bands, 18, 27) - band_avg(&r.bands, 74, 88);
        assert!(drop(&pink) > 6.0, "pink drop {}", drop(&pink));
        assert!(
            drop(&brown) > drop(&pink) + 6.0,
            "brown {} vs pink {}",
            drop(&brown),
            drop(&pink)
        );
    }

    #[test]
    fn noise_reference_loudness_is_plausible() {
        for kind in [NoiseKind::Pink, NoiseKind::Brown] {
            let r = noise_reference_curve(kind, 48_000).unwrap();
            assert!(r.loudness.is_finite());
            assert!(
                (-60.0..-5.0).contains(&r.loudness),
                "{kind:?} loudness {}",
                r.loudness
            );
        }
    }

    #[test]
    fn noise_reference_covers_candidate_rates() {
        for &sr in &[44_100u32, 96_000] {
            for kind in [NoiseKind::Pink, NoiseKind::Brown] {
                let r = noise_reference_curve(kind, sr).unwrap();
                assert_eq!(r.bands.len(), BANDS);
                assert!(r.bands.iter().all(|v| v.is_finite()));
            }
        }
    }

    #[test]
    fn noise_kind_parses_frontend_values() {
        assert_eq!(NoiseKind::parse("pink").unwrap(), NoiseKind::Pink);
        assert_eq!(NoiseKind::parse("brown").unwrap(), NoiseKind::Brown);
        assert!(NoiseKind::parse("white").is_err());
        assert!(noise_reference_curve(NoiseKind::Pink, 0).is_err());
    }
}
