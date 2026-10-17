//! The live loudness/spectrum analyzer MeterMaid's engine thread drives.

use std::collections::VecDeque;
use std::sync::Arc;

use ebur128::{EbuR128, Mode};
use rustfft::{num_complex::Complex, Fft, FftPlanner};

use crate::{clean, lin_to_db, BANDS, FFT_SIZE, LOUDNESS_FLOOR, SPECTRUM_FLOOR};

/// How many mono samples to retain for analysis.
pub(crate) const RING_CAP: usize = FFT_SIZE * 2;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct Metrics {
    /// Momentary loudness (400 ms window), LUFS.
    pub momentary: f64,
    /// Short-term loudness (3 s window), LUFS.
    pub short_term: f64,
    /// Integrated (gated) loudness, LUFS.
    pub integrated: f64,
    /// Loudness range, LU.
    pub lra: f64,
    /// True peak over the most recent window (live meter), dBTP.
    pub true_peak_db: f64,
    /// Maximum true peak held since the last reset (peak-hold), dBTP.
    pub true_peak_max_db: f64,
    /// Per-channel live true peak, dBTP, in metered-channel order (one entry
    /// for mono, L then R for a stereo pair). `true_peak_db` is their maximum.
    pub true_peak_ch_db: Vec<f64>,
    /// Per-channel held true-peak maximum, dBTP, in the same order.
    pub true_peak_ch_max_db: Vec<f64>,
    /// Log-spaced spectrum magnitudes, dB.
    pub spectrum: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    /// Measurement generation: bumped on (re)configure and on Reset. Lets the
    /// UI discard metrics computed before a reset it requested — e.g. so a
    /// stale held true-peak still in flight can't re-latch the clip light the
    /// user just cleared.
    pub generation: u64,
}

/// All loudness/spectrum analysis state. Owned solely by the engine thread, so
/// it needs no synchronization — the realtime callback never touches it.
pub struct Analyzer {
    pub(crate) ebu: Option<EbuR128>,
    /// Mono downmix ring buffer feeding the FFT spectrum.
    pub(crate) mono_ring: VecDeque<f32>,
    /// Source channel indices to extract from each interleaved frame.
    sel: Vec<usize>,
    /// Total channels in the incoming stream (de-interleave stride).
    device_channels: usize,
    sample_rate: u32,
    /// Number of channels fed to the analyzer (selection length).
    channels: u16,
    /// Per-channel max true peak (linear) seen since the last emit; reset
    /// each emit. One entry per analyzer channel.
    live_peaks: Vec<f64>,
    /// Measurement generation (see [`Metrics::generation`]).
    generation: u64,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// Reused scratch for the de-interleaved analyzer-channel samples.
    inter: Vec<f32>,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    pub fn new() -> Self {
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        // Hann window.
        let window = (0..FFT_SIZE)
            .map(|i| {
                let x = std::f32::consts::PI * i as f32 / (FFT_SIZE as f32 - 1.0);
                x.sin().powi(2)
            })
            .collect();
        Analyzer {
            ebu: None,
            mono_ring: VecDeque::with_capacity(RING_CAP),
            sel: Vec::new(),
            device_channels: 0,
            sample_rate: 0,
            channels: 0,
            live_peaks: Vec::new(),
            generation: 0,
            fft,
            window,
            inter: Vec::new(),
        }
    }

    /// (Re)initialize for a new stream. Fails only on invalid analyzer params.
    pub fn configure(
        &mut self,
        sample_rate: u32,
        sel: Vec<usize>,
        device_channels: usize,
    ) -> Result<(), String> {
        let ch = sel.len() as u32;
        self.ebu = Some(
            EbuR128::new(ch, sample_rate, Mode::all())
                .map_err(|e| format!("Couldn’t initialize the loudness analyzer: {e}"))?,
        );
        self.sample_rate = sample_rate;
        self.channels = ch as u16;
        self.sel = sel;
        self.device_channels = device_channels;
        self.mono_ring.clear();
        self.live_peaks.clear();
        self.live_peaks.resize(ch as usize, 0.0);
        self.generation += 1;
        Ok(())
    }

    /// Tear down the active measurement (called on Stop).
    pub fn shutdown(&mut self) {
        self.ebu = None;
        self.mono_ring.clear();
        self.live_peaks.fill(0.0);
    }

    /// Process a chunk of interleaved frames: de-interleave the selected
    /// channels, accumulate a mono downmix, feed the loudness analyzer, and
    /// track the live true peak. Runs on the engine thread (no locks).
    pub fn process(&mut self, data: &[f32]) {
        let stride = self.device_channels;
        let n = self.sel.len();
        if stride == 0 || n == 0 {
            return;
        }

        let frames = data.len() / stride;
        self.inter.clear();
        self.inter.reserve(frames * n);
        for f in 0..frames {
            let base = f * stride;
            let mut m = 0.0f32;
            for &ci in &self.sel {
                let s = data.get(base + ci).copied().unwrap_or(0.0);
                self.inter.push(s);
                m += s;
            }
            self.mono_ring.push_back(m / n as f32);
            if self.mono_ring.len() > RING_CAP {
                self.mono_ring.pop_front();
            }
        }

        if let Some(ebu) = self.ebu.as_mut() {
            if ebu.add_frames_f32(&self.inter).is_ok() {
                for (c, live) in self.live_peaks.iter_mut().enumerate() {
                    if let Ok(v) = ebu.prev_true_peak(c as u32) {
                        if v > *live {
                            *live = v;
                        }
                    }
                }
            }
        }
    }

    /// Re-initialize the integrated/LRA/peak measurement (keeps device running).
    pub fn reset(&mut self) {
        if self.sample_rate > 0 && self.channels > 0 {
            self.ebu = EbuR128::new(self.channels as u32, self.sample_rate, Mode::all()).ok();
        }
        self.mono_ring.clear();
        self.live_peaks.fill(0.0);
        self.generation += 1;
    }

    /// The mono downmix history feeding the spectrum, oldest first (at most
    /// two FFT windows).
    pub fn mono_history(&self) -> &VecDeque<f32> {
        &self.mono_ring
    }

    pub fn spectrum(&self) -> Vec<f32> {
        let sr = self.sample_rate;
        if self.mono_ring.len() < FFT_SIZE || sr == 0 {
            return vec![SPECTRUM_FLOOR; BANDS];
        }
        let start = self.mono_ring.len() - FFT_SIZE;
        let mut buf: Vec<Complex<f32>> = (0..FFT_SIZE)
            .map(|i| Complex {
                re: self.mono_ring[start + i] * self.window[i],
                im: 0.0,
            })
            .collect();

        self.fft.process(&mut buf);

        let half = FFT_SIZE / 2;
        // Coherent gain of a Hann window is 0.5, so scale by 2.
        let norm = 2.0 / (FFT_SIZE as f32);
        let mag: Vec<f32> = buf[..half].iter().map(|c| c.norm() * norm).collect();

        let f_lo = 20.0f32;
        let f_hi = (sr as f32 / 2.0).min(20_000.0);
        let bin_hz = sr as f32 / FFT_SIZE as f32;

        (0..BANDS)
            .map(|b| {
                let lo = f_lo * (f_hi / f_lo).powf(b as f32 / BANDS as f32);
                let hi = f_lo * (f_hi / f_lo).powf((b + 1) as f32 / BANDS as f32);
                let bin_lo = ((lo / bin_hz).floor() as usize).max(1);
                let bin_hi = ((hi / bin_hz).ceil() as usize).clamp(bin_lo + 1, half);
                let slice = &mag[bin_lo..bin_hi];
                let peak = slice.iter().copied().fold(0.0f32, f32::max);
                (20.0 * (peak + 1e-9).log10()).max(SPECTRUM_FLOOR)
            })
            .collect()
    }

    pub fn metrics(&mut self) -> Metrics {
        let sr = self.sample_rate;
        let ch = self.channels;

        // Live peaks: max since last emit per channel, then reset the
        // accumulators.
        let true_peak_ch_db: Vec<f64> = self.live_peaks.iter().map(|&p| lin_to_db(p)).collect();
        let live_lin = self.live_peaks.iter().copied().fold(0.0f64, f64::max);
        self.live_peaks.fill(0.0);

        let (momentary, short_term, integrated, lra, max_ch_lin) = match self.ebu.as_ref() {
            Some(e) => (
                clean(e.loudness_momentary().ok(), LOUDNESS_FLOOR),
                clean(e.loudness_shortterm().ok(), LOUDNESS_FLOOR),
                clean(e.loudness_global().ok(), LOUDNESS_FLOOR),
                clean(e.loudness_range().ok(), 0.0),
                (0..ch as u32)
                    .map(|c| e.true_peak(c).unwrap_or(0.0))
                    .collect::<Vec<f64>>(),
            ),
            None => (
                LOUDNESS_FLOOR,
                LOUDNESS_FLOOR,
                LOUDNESS_FLOOR,
                0.0,
                vec![0.0; ch as usize],
            ),
        };
        let max_lin = max_ch_lin.iter().copied().fold(0.0f64, f64::max);

        Metrics {
            momentary,
            short_term,
            integrated,
            lra,
            true_peak_db: lin_to_db(live_lin),
            true_peak_max_db: lin_to_db(max_lin),
            true_peak_ch_db,
            true_peak_ch_max_db: max_ch_lin.iter().map(|&p| lin_to_db(p)).collect(),
            spectrum: self.spectrum(),
            sample_rate: sr,
            channels: ch,
            generation: self.generation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::mono_sine;
    use crate::PEAK_FLOOR;

    /// Build a configured analyzer for the common mono-device case.
    fn analyzer(sample_rate: u32, device_channels: usize, sel: Vec<usize>) -> Analyzer {
        let mut a = Analyzer::new();
        a.configure(sample_rate, sel, device_channels).unwrap();
        a
    }

    // --- Channel selection / de-interleave ---------------------------------

    #[test]
    fn deinterleave_picks_selected_channel() {
        // 2-channel device, meter the right channel only (index 1).
        let mut a = analyzer(48_000, 2, vec![1]);
        // Frames: L=1.0, R=0.5.
        let data: Vec<f32> = [1.0, 0.5].repeat(8);
        a.process(&data);
        assert_eq!(a.mono_ring.len(), 8);
        for &s in &a.mono_ring {
            assert!((s - 0.5).abs() < 1e-6, "expected right channel, got {s}");
        }
    }

    #[test]
    fn deinterleave_downmixes_selected_channels() {
        // Meter both channels: mono = (L + R) / 2.
        let mut a = analyzer(48_000, 2, vec![0, 1]);
        let data: Vec<f32> = [1.0, 0.0].repeat(8);
        a.process(&data);
        assert_eq!(a.mono_ring.len(), 8);
        for &s in &a.mono_ring {
            assert!((s - 0.5).abs() < 1e-6, "expected 0.5 downmix, got {s}");
        }
    }

    // --- Loudness (golden) --------------------------------------------------

    #[test]
    fn integrated_lufs_scales_with_level() {
        // Two 1 kHz tones 10 dB apart should differ by ~10 LU regardless of the
        // exact K-weighting gain at 1 kHz — a robust correctness anchor.
        let mut hi = analyzer(48_000, 1, vec![0]);
        hi.process(&mono_sine(1000.0, 0.5, 4.0, 48_000));
        let l_hi = hi.metrics().integrated;

        let lo_amp = 0.5 / 10f32.powf(0.5); // -10 dB
        let mut lo = analyzer(48_000, 1, vec![0]);
        lo.process(&mono_sine(1000.0, lo_amp, 4.0, 48_000));
        let l_lo = lo.metrics().integrated;

        assert!(
            (l_hi - l_lo - 10.0).abs() < 0.5,
            "expected ~10 LU difference, got {l_hi} vs {l_lo}"
        );
    }

    #[test]
    fn integrated_lufs_absolute_anchor() {
        // A -6 dBFS 1 kHz mono sine: L ≈ dBFS - 3.7 + K-weighting(1kHz).
        // Assert a generous band around the expected ~-9.7 LUFS.
        let mut a = analyzer(48_000, 1, vec![0]);
        a.process(&mono_sine(1000.0, 0.5, 4.0, 48_000));
        let l = a.metrics().integrated;
        assert!(
            (-12.0..-6.5).contains(&l),
            "integrated LUFS out of expected band: {l}"
        );
    }

    #[test]
    fn true_peak_tracks_signal_level() {
        // -6 dBFS sine → true peak near -6 dBTP (a little higher for ISPs).
        let mut a = analyzer(48_000, 1, vec![0]);
        a.process(&mono_sine(997.0, 0.5, 1.0, 48_000));
        let m = a.metrics();
        assert!(
            (m.true_peak_max_db - (-6.02)).abs() < 1.0,
            "true peak max out of range: {}",
            m.true_peak_max_db
        );
        // The live peak (max since last emit) should also have registered.
        assert!(
            m.true_peak_db > -10.0,
            "live peak too low: {}",
            m.true_peak_db
        );
    }

    #[test]
    fn reset_clears_integrated_measurement() {
        let mut a = analyzer(48_000, 1, vec![0]);
        a.process(&mono_sine(1000.0, 0.5, 3.0, 48_000));
        assert!(a.metrics().integrated > LOUDNESS_FLOOR);
        a.reset();
        assert_eq!(a.metrics().integrated, LOUDNESS_FLOOR);
        assert!(a.mono_ring.is_empty());
    }

    // The generation lets the UI discard metrics computed before a reset it
    // requested (a stale held true-peak could otherwise re-latch the clip
    // light) — so it must bump on every configure and reset.
    #[test]
    fn generation_increments_on_configure_and_reset() {
        let mut a = Analyzer::new();
        assert_eq!(a.metrics().generation, 0);
        a.configure(48_000, vec![0], 1).unwrap();
        assert_eq!(a.metrics().generation, 1);
        a.reset();
        assert_eq!(a.metrics().generation, 2);
        a.configure(48_000, vec![0], 1).unwrap();
        assert_eq!(a.metrics().generation, 3);
    }

    // --- Spectrum -----------------------------------------------------------

    #[test]
    fn spectrum_floors_without_enough_samples() {
        let mut a = analyzer(48_000, 1, vec![0]);
        a.process(&mono_sine(1000.0, 0.5, 0.01, 48_000)); // < FFT_SIZE samples
        let s = a.metrics().spectrum;
        assert_eq!(s.len(), BANDS);
        assert!(s.iter().all(|&v| v == SPECTRUM_FLOOR));
    }

    #[test]
    fn spectrum_peaks_at_tone_frequency() {
        let mut a = analyzer(48_000, 1, vec![0]);
        a.process(&mono_sine(1000.0, 0.5, 0.5, 48_000));
        let s = a.spectrum();
        assert_eq!(s.len(), BANDS);
        let argmax = s
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.partial_cmp(y.1).unwrap())
            .unwrap()
            .0;
        // The 1 kHz band sits near index ~54 in the 20 Hz–20 kHz log scale.
        assert!(
            (50..=58).contains(&argmax),
            "spectrum peak band {argmax} not near 1 kHz"
        );
        assert!(s[argmax] > SPECTRUM_FLOOR);
    }

    // --- Sample-rate handling ----------------------------------------------

    #[test]
    fn handles_multiple_sample_rates() {
        for &sr in &[44_100u32, 96_000] {
            let mut a = analyzer(sr, 1, vec![0]);
            a.process(&mono_sine(1000.0, 0.5, 1.0, sr));
            let m = a.metrics();
            assert_eq!(m.sample_rate, sr);
            assert_eq!(m.channels, 1);
            assert_eq!(m.spectrum.len(), BANDS);
            assert!(m.integrated > LOUDNESS_FLOOR);
        }
    }

    #[test]
    fn stereo_selection_is_supported() {
        let mut a = analyzer(48_000, 2, vec![0, 1]);
        // Identical L/R 1 kHz tone.
        let mono = mono_sine(1000.0, 0.5, 3.0, 48_000);
        let mut interleaved = Vec::with_capacity(mono.len() * 2);
        for &s in &mono {
            interleaved.push(s);
            interleaved.push(s);
        }
        a.process(&interleaved);
        let m = a.metrics();
        assert_eq!(m.channels, 2);
        assert!(m.integrated > LOUDNESS_FLOOR);
    }

    #[test]
    fn true_peak_is_reported_per_channel() {
        // L at -6 dBFS, R at -20 dBFS: each channel reports its own peak, and
        // the overall figures are the louder (left) channel's.
        let mut a = analyzer(48_000, 2, vec![0, 1]);
        let mono = mono_sine(997.0, 1.0, 1.0, 48_000);
        let mut interleaved = Vec::with_capacity(mono.len() * 2);
        for &s in &mono {
            interleaved.push(s * 0.5);
            interleaved.push(s * 0.1);
        }
        a.process(&interleaved);
        let m = a.metrics();
        assert_eq!(m.true_peak_ch_db.len(), 2);
        assert_eq!(m.true_peak_ch_max_db.len(), 2);
        for (name, ch) in [
            ("live", &m.true_peak_ch_db),
            ("max", &m.true_peak_ch_max_db),
        ] {
            assert!(
                (ch[0] - (-6.02)).abs() < 1.0,
                "{name} L out of range: {}",
                ch[0]
            );
            assert!(
                (ch[1] - (-20.0)).abs() < 1.0,
                "{name} R out of range: {}",
                ch[1]
            );
        }
        assert_eq!(m.true_peak_db, m.true_peak_ch_db[0]);
        assert_eq!(m.true_peak_max_db, m.true_peak_ch_max_db[0]);

        // The live peaks are per-emit accumulators; the held maxima persist.
        let m2 = a.metrics();
        assert_eq!(m2.true_peak_ch_db, vec![PEAK_FLOOR; 2]);
        assert_eq!(m2.true_peak_ch_max_db, m.true_peak_ch_max_db);

        // A mono selection reports a single channel.
        let mut mono_a = analyzer(48_000, 1, vec![0]);
        mono_a.process(&mono_sine(997.0, 0.5, 1.0, 48_000));
        assert_eq!(mono_a.metrics().true_peak_ch_db.len(), 1);
    }

    // --- Optional ffmpeg cross-check (manual) -------------------------------
    //
    // Validates our integrated LUFS against ffmpeg's `ebur128` filter on the
    // same signal. Ignored by default (requires ffmpeg + writes a temp WAV);
    // run with: `cargo test ebur128_matches_ffmpeg -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ebur128_matches_ffmpeg() {
        use std::io::Write;
        use std::process::Command as Proc;

        let sr = 48_000u32;
        let samples = mono_sine(1000.0, 0.5, 5.0, sr);

        let mut a = analyzer(sr, 1, vec![0]);
        a.process(&samples);
        let ours = a.metrics().integrated;

        // Write a minimal 16-bit mono WAV.
        let path = std::env::temp_dir().join("metermaid_ebur128_check.wav");
        let mut bytes: Vec<u8> = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&sr.to_le_bytes());
        bytes.extend_from_slice(&(sr * 2).to_le_bytes()); // byte rate
        bytes.extend_from_slice(&2u16.to_le_bytes()); // block align
        bytes.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for &s in &samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::File::create(&path)
            .and_then(|mut f| f.write_all(&bytes))
            .expect("write wav");

        let out = match Proc::new("ffmpeg")
            .args(["-hide_banner", "-nostats", "-i"])
            .arg(&path)
            .args(["-af", "ebur128", "-f", "null", "-"])
            .output()
        {
            Ok(o) => o,
            Err(_) => {
                eprintln!("ffmpeg not found — skipping cross-check");
                return;
            }
        };
        let stderr = String::from_utf8_lossy(&out.stderr);
        // The summary block ends with a line like "    I:  -9.7 LUFS".
        let ff = stderr
            .lines()
            .filter_map(|l| {
                let t = l.trim();
                t.strip_prefix("I:")
                    .and_then(|r| r.trim().strip_suffix("LUFS"))
                    .and_then(|r| r.trim().parse::<f64>().ok())
            })
            .next_back()
            .expect("parse ffmpeg integrated LUFS");

        eprintln!("ours={ours:.2} LUFS, ffmpeg={ff:.2} LUFS");
        assert!(
            (ours - ff).abs() < 1.0,
            "integrated LUFS disagrees with ffmpeg: ours={ours}, ffmpeg={ff}"
        );
    }
}
