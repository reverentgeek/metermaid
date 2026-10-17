//! Whole-buffer loudness analysis (a decoded file or a finished capture).

use ebur128::{EbuR128, Mode};

use crate::{clean, lin_to_db, LOUDNESS_FLOOR};

/// Momentary/short-term loudness are sampled on this hop: BS.1770-4's 100 ms
/// update rate (the 75% overlap of the 400 ms momentary block).
const HOP_SECS: f64 = 0.1;

/// Loudness summary of a whole buffer.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct OfflineReport {
    /// Integrated (gated) loudness, LUFS.
    pub integrated: f64,
    /// Loudness range, LU.
    pub lra: f64,
    /// Highest momentary (400 ms) loudness, sampled every 100 ms, LUFS.
    pub momentary_max: f64,
    /// Highest short-term (3 s) loudness, sampled every 100 ms, LUFS.
    pub short_term_max: f64,
    /// Short-term loudness every 100 ms, LUFS (floored before the first full
    /// 3 s window).
    pub short_term: Vec<f64>,
    /// Per-channel true peak over the whole buffer, dBTP.
    pub true_peak_ch_db: Vec<f64>,
    /// Maximum of `true_peak_ch_db`, dBTP.
    pub true_peak_db: f64,
    /// Analyzed duration, seconds.
    pub seconds: f64,
}

/// Measure an interleaved buffer of `channels` channels at `sample_rate`.
/// Every channel is measured (pick or downmix channels first if needed); a
/// trailing partial frame is ignored.
pub fn analyze_offline(
    interleaved: &[f32],
    channels: u16,
    sample_rate: u32,
) -> Result<OfflineReport, String> {
    if channels == 0 || sample_rate == 0 {
        return Err("Channel count and sample rate must be non-zero".into());
    }
    let mut ebu = EbuR128::new(u32::from(channels), sample_rate, Mode::all())
        .map_err(|e| format!("Couldn’t initialize the loudness analyzer: {e}"))?;

    let ch = usize::from(channels);
    let frames = interleaved.len() / ch;
    let hop = ((sample_rate as f64 * HOP_SECS).round() as usize).max(1) * ch;
    let mut momentary_max = LOUDNESS_FLOOR;
    let mut short_term_max = LOUDNESS_FLOOR;
    let mut short_term = Vec::with_capacity(frames * ch / hop + 1);

    for chunk in interleaved[..frames * ch].chunks(hop) {
        ebu.add_frames_f32(chunk)
            .map_err(|e| format!("Loudness analysis failed: {e}"))?;
        let m = clean(ebu.loudness_momentary().ok(), LOUDNESS_FLOOR);
        let s = clean(ebu.loudness_shortterm().ok(), LOUDNESS_FLOOR);
        momentary_max = momentary_max.max(m);
        short_term_max = short_term_max.max(s);
        short_term.push(s);
    }

    let true_peak_ch_db: Vec<f64> = (0..u32::from(channels))
        .map(|c| lin_to_db(ebu.true_peak(c).unwrap_or(0.0)))
        .collect();
    let true_peak_db = true_peak_ch_db.iter().copied().fold(f64::MIN, f64::max);

    Ok(OfflineReport {
        integrated: clean(ebu.loudness_global().ok(), LOUDNESS_FLOOR),
        lra: clean(ebu.loudness_range().ok(), 0.0),
        momentary_max,
        short_term_max,
        short_term,
        true_peak_ch_db,
        true_peak_db,
        seconds: frames as f64 / f64::from(sample_rate),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::mono_sine;
    use crate::{Analyzer, PEAK_FLOOR};

    #[test]
    fn calibration_tone_reads_minus_23_lufs() {
        // BS.1770's calibration point: a 1 kHz sine at -20 dBFS peak in one
        // channel measures -23.0 LUFS (the K-weighting adds ~+0.69 dB at
        // 1 kHz, cancelling the -0.691 offset; the sine's mean square is
        // -3.01 dB below its peak).
        let amp = 10f32.powf(-20.0 / 20.0);
        let r = analyze_offline(&mono_sine(1000.0, amp, 10.0, 48_000), 1, 48_000).unwrap();
        assert!(
            (r.integrated - (-23.0)).abs() < 0.1,
            "integrated {}",
            r.integrated
        );
        // A steady tone: momentary and short-term maxima agree with integrated.
        assert!((r.momentary_max - r.integrated).abs() < 0.1);
        assert!((r.short_term_max - r.integrated).abs() < 0.1);
        assert!((r.true_peak_db - (-20.0)).abs() < 0.5);
        assert!((r.seconds - 10.0).abs() < 1e-9);
        assert_eq!(r.short_term.len(), 100);
    }

    #[test]
    fn matches_the_live_analyzer() {
        // The offline path and the live path must agree on the same signal.
        let sig = mono_sine(440.0, 0.3, 5.0, 44_100);
        let r = analyze_offline(&sig, 1, 44_100).unwrap();
        let mut a = Analyzer::new();
        a.configure(44_100, vec![0], 1).unwrap();
        a.process(&sig);
        let m = a.metrics();
        assert!((r.integrated - m.integrated).abs() < 1e-9);
        assert!((r.true_peak_db - m.true_peak_max_db).abs() < 1e-9);
    }

    #[test]
    fn short_term_timeline_tracks_a_level_step() {
        // 4 s quiet then 4 s 20 dB louder: the timeline must rise by ~20 LU
        // and the max must come from the loud half.
        let mut sig = mono_sine(1000.0, 0.05, 4.0, 48_000);
        sig.extend(mono_sine(1000.0, 0.5, 4.0, 48_000));
        let r = analyze_offline(&sig, 1, 48_000).unwrap();
        let quiet = r.short_term[35]; // 3.5 s: a full 3 s window of the quiet tone
        let loud = *r.short_term.last().unwrap();
        assert!((loud - quiet - 20.0).abs() < 0.2, "{quiet} → {loud}");
        assert!((r.short_term_max - loud).abs() < 0.05);
    }

    #[test]
    fn reports_per_channel_peaks_and_handles_silence() {
        let tone = mono_sine(997.0, 1.0, 1.0, 48_000);
        let stereo: Vec<f32> = tone.iter().flat_map(|&s| [s * 0.5, 0.0]).collect();
        let r = analyze_offline(&stereo, 2, 48_000).unwrap();
        assert!((r.true_peak_ch_db[0] - (-6.02)).abs() < 1.0);
        assert_eq!(r.true_peak_ch_db[1], PEAK_FLOOR);

        let silent = analyze_offline(&[0.0; 48_000], 1, 48_000).unwrap();
        assert_eq!(silent.integrated, LOUDNESS_FLOOR);
        assert_eq!(silent.true_peak_db, PEAK_FLOOR);
        assert!(analyze_offline(&[], 0, 48_000).is_err());
    }
}
