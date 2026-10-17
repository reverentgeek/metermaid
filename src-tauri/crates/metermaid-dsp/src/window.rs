//! Helpers for picking and shaping the part of a capture that gets analyzed.

use std::ops::Range;

/// The sample range to analyze in a capture of a looped stimulus: drop `skip`
/// samples at each end (capture start/stop transients), then keep the largest
/// whole number of `period`-sample cycles from what remains, starting right
/// after the skip.
///
/// Analyzing whole cycles keeps the reading independent of where in the loop
/// the window happens to start. Returns `None` when no full cycle fits or the
/// period is zero. All values are in frames (or samples, consistently).
pub fn cycle_window(len: usize, period: usize, skip: usize) -> Option<Range<usize>> {
    if period == 0 {
        return None;
    }
    let usable = len.checked_sub(skip.checked_mul(2)?)?;
    let cycles = usable / period;
    if cycles == 0 {
        return None;
    }
    Some(skip..skip + cycles * period)
}

/// Average the given channels of an interleaved buffer into mono. Trailing
/// partial frames are dropped, and an out-of-range channel reads as silence
/// (matching [`crate::Analyzer::process`]).
pub fn downmix(interleaved: &[f32], channels: usize, sel: &[usize]) -> Vec<f32> {
    if channels == 0 || sel.is_empty() {
        return Vec::new();
    }
    let scale = 1.0 / sel.len() as f32;
    interleaved
        .chunks_exact(channels)
        .map(|frame| {
            sel.iter()
                .map(|&c| frame.get(c).copied().unwrap_or(0.0))
                .sum::<f32>()
                * scale
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_window_keeps_whole_cycles_after_the_skip() {
        // 16 s of a 5 s loop with 1 s skips → 14 s usable → two whole cycles.
        assert_eq!(cycle_window(16, 5, 1), Some(1..11));
        // Exactly fits.
        assert_eq!(cycle_window(12, 5, 1), Some(1..11));
        // No skip.
        assert_eq!(cycle_window(10, 5, 0), Some(0..10));
    }

    #[test]
    fn cycle_window_rejects_captures_without_a_full_cycle() {
        assert_eq!(cycle_window(6, 5, 1), None);
        assert_eq!(cycle_window(1, 5, 1), None); // shorter than both skips
        assert_eq!(cycle_window(100, 0, 0), None);
        assert_eq!(cycle_window(10, 5, usize::MAX), None); // no overflow
    }

    #[test]
    fn downmix_averages_selected_channels() {
        // 3-channel frames: [1, 0.5, 0] repeated; average channels 0 and 1.
        let data = [1.0, 0.5, 0.0].repeat(4);
        assert_eq!(downmix(&data, 3, &[0, 1]), vec![0.75; 4]);
        assert_eq!(downmix(&data, 3, &[2]), vec![0.0; 4]);
        // A trailing partial frame is dropped.
        assert_eq!(downmix(&data[..7], 3, &[0]).len(), 2);
        assert!(downmix(&data, 0, &[0]).is_empty());
    }
}
