//! Tauri audio engine for the MeterMaid analyzer.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

pub use metermaid_capture::{
    device_config, list_input_devices, DeviceConfig, DeviceInfo, StreamInfo,
};
use metermaid_capture::{drain_chunk_len, Capture};
use metermaid_dsp::{Analyzer, Metrics};
use tauri::{AppHandle, Emitter};

/// Emit cadence.
const EMIT_INTERVAL: Duration = Duration::from_millis(33);
/// Commands sent from Tauri command handlers to the audio engine thread.
pub enum Command {
    Start {
        device: Option<String>,
        sample_rate: Option<u32>,
        /// Zero-based source channel indices to meter (1 or 2 entries).
        channels: Vec<u32>,
        reply: SyncSender<Result<StreamInfo, String>>,
    },
    Stop {
        reply: SyncSender<()>,
    },
    Reset,
}

/// Engine thread: owns the (non-Send) cpal stream + the SPSC consumer + the
/// `Analyzer`, services commands, and on a fixed cadence drains the ring and
/// emits `meter-update` events while capturing.
pub fn engine_loop(rx: Receiver<Command>, app: AppHandle) {
    // Emit on a dedicated thread so a slow/blocking UI emit can never stall the
    // realtime drain. If the UI falls behind, frames are dropped (coalesced to
    // the latest) rather than backing up the audio ring — the loudness analyzer
    // still receives every sample.
    let (emit_tx, emit_rx) = mpsc::sync_channel::<Metrics>(1);
    let emit_app = app.clone();
    std::thread::spawn(move || {
        while let Ok(metrics) = emit_rx.recv() {
            let _ = emit_app.emit("meter-update", metrics);
        }
    });

    // The cpal stream is held only to keep capture alive (dropping it stops the
    // device); it is paired with its consumer and dropped-sample counter so all
    // three are torn down together.
    let mut active: Option<Capture> = None;
    let mut analyzer = Analyzer::new();
    let mut drain = Vec::new();

    loop {
        match rx.recv_timeout(EMIT_INTERVAL) {
            Ok(Command::Start {
                device,
                sample_rate,
                channels,
                reply,
            }) => {
                active = None; // stop any existing stream first
                let channels = channels
                    .into_iter()
                    .map(|channel| channel as usize)
                    .collect();
                let err_app = app.clone();
                let on_error = move |message: String, fatal| {
                    eprintln!("audio stream error: {message}");
                    if fatal {
                        let _ = err_app.emit("stream-error", message);
                    }
                };
                let result = (|| {
                    #[cfg(debug_assertions)]
                    if std::env::var_os("METERMAID_SIMULATE_ERROR").is_some() {
                        return Err("Couldn’t open input device for capture: simulated failure (METERMAID_SIMULATE_ERROR).".to_string());
                    }
                    let capture = metermaid_capture::open(
                        device.as_deref(),
                        sample_rate,
                        channels,
                        on_error,
                    )?;
                    analyzer.configure(
                        capture.info().sample_rate,
                        capture.selection().to_vec(),
                        capture.device_channels(),
                    )?;
                    if let Err(err) = capture.play() {
                        analyzer.shutdown();
                        return Err(err);
                    }
                    drain.resize(drain_chunk_len(capture.device_channels()), 0.0);
                    let info = capture.info().clone();
                    active = Some(capture);
                    Ok(info)
                })();
                let _ = reply.send(result);
            }
            Ok(Command::Stop { reply }) => {
                active = None;
                analyzer.shutdown();
                let _ = reply.send(());
            }
            Ok(Command::Reset) => analyzer.reset(),
            Err(RecvTimeoutError::Timeout) => {
                if let Some(active) = active.as_mut() {
                    loop {
                        let got = active.read(&mut drain);
                        if got == 0 {
                            break;
                        }
                        analyzer.process(&drain[..got]);
                    }
                    // Report any ring overruns the realtime callback tallied,
                    // off the realtime thread.
                    let dropped = active.take_dropped_samples();
                    if dropped > 0 {
                        eprintln!("audio ring overrun: dropped {dropped} samples");
                    }
                    // Non-blocking: drop this frame if the UI emit is behind.
                    let _ = emit_tx.try_send(analyzer.metrics());
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a configured analyzer for the common mono-device case.
    fn analyzer(sample_rate: u32, device_channels: usize, sel: Vec<usize>) -> Analyzer {
        let mut a = Analyzer::new();
        a.configure(sample_rate, sel, device_channels).unwrap();
        a
    }

    // Regression test for the multichannel drain-alignment bug: the engine used
    // to drain the ring in fixed 8192-sample chunks, but 8192 isn't a multiple
    // of a 6-channel frame and `process` drops a trailing partial frame — so
    // every full chunk rotated the channel alignment of everything after it.
    #[test]
    fn chunked_draining_preserves_channel_alignment() {
        let stride = 6; // 6 does not divide 8192, so a naive chunk splits a frame
        let mut a = analyzer(48_000, stride, vec![2]);

        // Each channel carries a distinct constant so any rotation is visible.
        let frames = 4000; // more than two drain chunks' worth of samples
        let mut data = Vec::with_capacity(frames * stride);
        for _ in 0..frames {
            for c in 0..stride {
                data.push(c as f32 / 10.0);
            }
        }

        // Feed exactly the way engine_loop drains: fixed-size whole-frame chunks.
        for chunk in data.chunks(drain_chunk_len(stride)) {
            a.process(chunk);
        }

        let history = a.mono_history();
        assert_eq!(history.len(), frames);
        for &s in history {
            assert!(
                (s - 0.2).abs() < 1e-6,
                "channel alignment rotated: expected 0.2 (channel 3), got {s}"
            );
        }
    }
}
