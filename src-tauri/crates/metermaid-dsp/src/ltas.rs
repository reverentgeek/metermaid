//! Long-term average spectrum (LTAS) on fractional-octave bands.
//!
//! Unlike the display spectrum ([`crate::Analyzer::spectrum`], the peak bin
//! per band of one 4096-point frame), this is a calibrated power measurement:
//! Welch-averaged over the whole buffer, with each band holding the signal's
//! mean-square power inside it. The bands therefore sum to the signal's total
//! mean square, a sine of amplitude A reads 20·log10(A) − 3.01 dB in its band,
//! and pink noise reads flat — what comparing two tones needs.

use rustfft::{num_complex::Complex, FftPlanner};

/// FFT size for the Welch average: ~2.9 Hz bins at 48 kHz, enough for
/// 1/3-octave bands to hold several bins down to ~25 Hz.
pub const LTAS_FFT_SIZE: usize = 16_384;
/// The covered range, Hz: every band that overlaps it is included (so the
/// third-octave set runs from the 20 Hz band to the 20 kHz band).
const F_MIN: f32 = 20.0;
const F_MAX: f32 = 20_000.0;
/// Floor for band levels (dB re full-scale mean square).
const FLOOR_DB: f32 = -150.0;

/// A long-term average spectrum.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct Ltas {
    /// Band center frequencies, Hz (base-2, anchored at 1 kHz).
    pub centers_hz: Vec<f32>,
    /// Mean-square power in each band, dB re full scale.
    pub db: Vec<f32>,
    pub bands_per_octave: u32,
}

/// Long-term average spectrum of a mono buffer on `bands_per_octave`
/// fractional-octave bands (3 = third-octave) between 20 Hz and 20 kHz (or
/// Nyquist). Needs at least [`LTAS_FFT_SIZE`] samples.
pub fn ltas(mono: &[f32], sample_rate: u32, bands_per_octave: u32) -> Result<Ltas, String> {
    if !(1..=24).contains(&bands_per_octave) {
        return Err(format!(
            "bands per octave must be 1–24, got {bands_per_octave}"
        ));
    }
    if sample_rate == 0 {
        return Err("Sample rate must be non-zero".into());
    }
    const N: usize = LTAS_FFT_SIZE;
    if mono.len() < N {
        return Err(format!(
            "Need at least {N} samples for a spectrum, got {}",
            mono.len()
        ));
    }

    // Welch: Hann window, 50% overlap, averaged |X|².
    let window: Vec<f32> = (0..N)
        .map(|i| {
            let x = std::f32::consts::PI * i as f32 / N as f32;
            x.sin().powi(2)
        })
        .collect();
    let w2: f64 = window.iter().map(|&w| f64::from(w * w)).sum();
    let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
    let half = N / 2;
    let mut acc = vec![0.0f64; half + 1];
    let mut buf = vec![Complex::default(); N];
    let mut segments = 0u32;
    let mut start = 0;
    while start + N <= mono.len() {
        for (b, (&s, &w)) in buf
            .iter_mut()
            .zip(mono[start..start + N].iter().zip(&window))
        {
            *b = Complex { re: s * w, im: 0.0 };
        }
        fft.process(&mut buf);
        for (a, c) in acc.iter_mut().zip(&buf[..=half]) {
            *a += f64::from(c.norm_sqr());
        }
        segments += 1;
        start += N / 2;
    }

    // One-sided mean-square per bin (Parseval): the bins sum to the signal's
    // mean square. DC and Nyquist appear once in the two-sided spectrum.
    let scale = 1.0 / (N as f64 * w2 * f64::from(segments));
    let power: Vec<f64> = acc
        .iter()
        .enumerate()
        .map(|(k, &p)| p * scale * if k == 0 || k == half { 1.0 } else { 2.0 })
        .collect();

    let bin_hz = sample_rate as f32 / N as f32;
    let nyquist = sample_rate as f32 / 2.0;
    let b = bands_per_octave as f32;
    let edge = 2f32.powf(1.0 / (2.0 * b));
    // Band i spans centers ±half a band in log2 units, so it overlaps
    // [F_MIN, F_MAX] when its index is within half a band of the range.
    let i_lo = (b * (F_MIN / 1000.0).log2() - 0.5).ceil() as i32;
    let i_hi = (b * (F_MAX / 1000.0).log2() + 0.5).floor() as i32;

    let mut centers_hz = Vec::new();
    let mut db = Vec::new();
    for i in i_lo..=i_hi {
        let fc = 1000.0 * 2f32.powf(i as f32 / b);
        let (lo, hi) = (fc / edge, fc * edge);
        if hi > nyquist {
            break;
        }
        let k_lo = (lo / bin_hz).ceil() as usize;
        let k_hi = ((hi / bin_hz).ceil() as usize).min(half + 1);
        let band_power = if k_lo < k_hi {
            power[k_lo..k_hi].iter().sum()
        } else {
            // Narrower than one bin (low bands at high rates): take the
            // density of the nearest bin across the band's width.
            let k = ((fc / bin_hz).round() as usize).min(half);
            power[k] * f64::from((hi - lo) / bin_hz)
        };
        centers_hz.push(fc);
        db.push(((10.0 * (band_power + 1e-30).log10()) as f32).max(FLOOR_DB));
    }

    Ok(Ltas {
        centers_hz,
        db,
        bands_per_octave,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::mono_sine;
    use crate::{gen_brown, gen_pink};

    /// Least-squares slope of band level against log2(frequency), dB/octave,
    /// over bands whose centers fall in [lo, hi] Hz.
    fn slope_db_per_octave(l: &Ltas, lo: f32, hi: f32) -> f32 {
        let pts: Vec<(f32, f32)> = l
            .centers_hz
            .iter()
            .zip(&l.db)
            .filter(|(f, _)| (lo..=hi).contains(*f))
            .map(|(f, d)| (f.log2(), *d))
            .collect();
        let n = pts.len() as f32;
        let mx = pts.iter().map(|p| p.0).sum::<f32>() / n;
        let my = pts.iter().map(|p| p.1).sum::<f32>() / n;
        let sxy: f32 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        let sxx: f32 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
        sxy / sxx
    }

    /// Deterministic white noise (LCG), independent of the crate's generators.
    fn white(n: usize) -> Vec<f32> {
        let mut s: u32 = 1;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn third_octave_bands_are_standard_centers() {
        let l = ltas(&vec![0.0; LTAS_FFT_SIZE], 48_000, 3).unwrap();
        // Base-2 third-octave centers from ~20 Hz to ~20 kHz: 31 bands.
        assert_eq!(l.centers_hz.len(), 31);
        assert!((l.centers_hz[0] - 19.69).abs() < 0.1);
        assert!(l.centers_hz.iter().any(|&f| (f - 1000.0).abs() < 1e-3));
        assert!(l.db.iter().all(|&d| d == FLOOR_DB));
    }

    #[test]
    fn sine_reads_its_mean_square_in_its_band() {
        // Amplitude 0.5 → mean square 0.125 → -9.03 dB, all in the 1 kHz band.
        let l = ltas(&mono_sine(1000.0, 0.5, 5.0, 48_000), 48_000, 3).unwrap();
        let i = l
            .centers_hz
            .iter()
            .position(|&f| (f - 1000.0).abs() < 1e-3)
            .unwrap();
        assert!((l.db[i] - (-9.03)).abs() < 0.1, "1 kHz band {}", l.db[i]);
        // Hann leakage stays local: two bands away is far down.
        assert!(l.db[i + 2] < l.db[i] - 60.0, "{}", l.db[i + 2]);
        assert!(l.db[i - 2] < l.db[i] - 60.0, "{}", l.db[i - 2]);
    }

    #[test]
    fn noise_colors_have_their_textbook_slopes() {
        // Power per fractional-octave band: white rises 3 dB/oct, pink is flat,
        // brown falls 3 dB/oct.
        let sr = 48_000;
        let n = 30 * sr as usize;
        let w = slope_db_per_octave(&ltas(&white(n), sr, 3).unwrap(), 50.0, 10_000.0);
        let p = slope_db_per_octave(&ltas(&gen_pink(n), sr, 3).unwrap(), 50.0, 10_000.0);
        let b = slope_db_per_octave(&ltas(&gen_brown(n, sr), sr, 3).unwrap(), 50.0, 10_000.0);
        assert!((w - 3.01).abs() < 0.2, "white {w}");
        assert!(p.abs() < 0.3, "pink {p}");
        assert!((b + 3.01).abs() < 0.3, "brown {b}");
    }

    #[test]
    fn bands_sum_to_the_signal_mean_square() {
        // A multitone spread across the range, all inside 20 Hz–20 kHz: the
        // bands must account for its mean square, Σ A²/2.
        let tones = [(63.0, 0.3), (440.0, 0.2), (2500.0, 0.1), (12_000.0, 0.05)];
        let mut sig = vec![0.0f32; 5 * 48_000];
        for &(f, a) in &tones {
            for (s, t) in sig.iter_mut().zip(mono_sine(f, a, 5.0, 48_000)) {
                *s += t;
            }
        }
        let ms: f64 = tones.iter().map(|&(_, a)| f64::from(a * a) / 2.0).sum();
        let l = ltas(&sig, 48_000, 3).unwrap();
        let total: f64 = l.db.iter().map(|&d| 10f64.powf(f64::from(d) / 10.0)).sum();
        let err_db = 10.0 * (total / ms).log10();
        assert!(err_db.abs() < 0.05, "band total off by {err_db} dB");
    }

    #[test]
    fn level_change_shifts_every_band_equally() {
        let sig = gen_pink(5 * 48_000);
        let loud: Vec<f32> = sig.iter().map(|&s| s * 2.0).collect();
        let a = ltas(&sig, 48_000, 6).unwrap();
        let b = ltas(&loud, 48_000, 6).unwrap();
        for (x, y) in a.db.iter().zip(&b.db) {
            assert!((y - x - 6.02).abs() < 0.01, "{x} → {y}");
        }
    }

    #[test]
    fn handles_rates_and_rejects_bad_input() {
        // 44.1 kHz: Nyquist cuts the top band; low bands still finite.
        let l = ltas(&gen_pink(5 * 44_100), 44_100, 3).unwrap();
        assert!(*l.centers_hz.last().unwrap() < 22_050.0 / 2f32.powf(1.0 / 6.0));
        assert!(l.db.iter().all(|d| d.is_finite() && *d > FLOOR_DB));
        // 96 kHz with 1/12-octave bands narrower than a bin at the bottom.
        let l = ltas(&gen_pink(5 * 96_000), 96_000, 12).unwrap();
        assert!(l.db.iter().all(|d| d.is_finite() && *d > FLOOR_DB));

        assert!(ltas(&[0.0; 100], 48_000, 3).is_err());
        assert!(ltas(&vec![0.0; LTAS_FFT_SIZE], 48_000, 0).is_err());
        assert!(ltas(&vec![0.0; LTAS_FFT_SIZE], 0, 3).is_err());
    }
}
