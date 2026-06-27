//! Audio capture + ITU-R BS.1770 / EBU R128 loudness analysis and FFT spectrum.
//!
//! A single dedicated thread (`engine_loop`) owns the cpal stream because
//! `cpal::Stream` is not `Send`. The realtime audio callback does no locking
//! and no heap allocation in steady state: it only copies the incoming frames
//! into a lock-free single-producer/single-consumer ring. The engine thread is
//! the sole owner of the `Analyzer` (no synchronization needed); on a fixed
//! cadence it drains the ring, de-interleaves the user-selected channels, feeds
//! a pure-Rust `ebur128` analyzer + a mono ring buffer, computes a spectrum,
//! and emits metrics to the UI.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use ebur128::{EbuR128, Mode};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// FFT window size for the spectral analyzer (power of two).
const FFT_SIZE: usize = 4096;
/// Number of log-spaced display bands sent to the UI.
const BANDS: usize = 96;
/// How many mono samples to retain for analysis.
const RING_CAP: usize = FFT_SIZE * 2;
/// Floor for loudness readouts (LUFS) when below the gate / no signal.
const LOUDNESS_FLOOR: f64 = -70.0;
/// Floor for the spectrum (dB).
const SPECTRUM_FLOOR: f32 = -90.0;
/// Floor for peak readouts (dBTP) with no signal.
const PEAK_FLOOR: f64 = -120.0;
/// Emit cadence.
const EMIT_INTERVAL: Duration = Duration::from_millis(33);
/// Sample rates offered in the UI when within a device's supported range.
const CANDIDATE_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Scratch size (samples) used to drain the SPSC ring on the engine thread.
const DRAIN_CHUNK: usize = 8192;

/// Identity prefix marking an ASIO-host device. The default (WASAPI/CoreAudio/
/// ALSA) host uses the bare device name as its id, so settings saved before
/// ASIO existed still resolve.
#[cfg(all(windows, target_arch = "x86_64"))]
const ASIO_ID_PREFIX: &str = "asio:";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// Host-qualified, stable identity: the dropdown value, the `settings.json`
    /// key, and what `start_capture` / `get_device_config` receive. ASIO devices
    /// are prefixed (`asio:`); default-host devices use the bare name.
    pub id: String,
    /// Raw device name, for display.
    pub name: String,
    /// Which host the device belongs to: `"default"` or `"asio"`. Drives the
    /// disambiguating label and the (advisory) sample-rate picker.
    pub host: String,
    pub is_default: bool,
}

/// Capabilities of a device, used to populate the channel / sample-rate pickers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConfig {
    /// Total channels the device exposes (the de-interleave stride).
    pub channels: u16,
    pub default_sample_rate: u32,
    pub sample_rates: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfo {
    pub device_name: String,
    pub sample_rate: u32,
    /// Number of channels actually being metered (1 = mono, 2 = stereo pair).
    pub channels: u16,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
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
    /// Log-spaced spectrum magnitudes, dB.
    pub spectrum: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

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

/// All loudness/spectrum analysis state. Owned solely by the engine thread, so
/// it needs no synchronization — the realtime callback never touches it.
struct Analyzer {
    ebu: Option<EbuR128>,
    /// Mono downmix ring buffer feeding the FFT spectrum.
    mono_ring: VecDeque<f32>,
    /// Source channel indices to extract from each interleaved frame.
    sel: Vec<usize>,
    /// Total channels in the incoming stream (de-interleave stride).
    device_channels: usize,
    sample_rate: u32,
    /// Number of channels fed to the analyzer (selection length).
    channels: u16,
    /// Max true peak (linear) seen since the last emit; reset each emit.
    live_peak: f64,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// Reused scratch for the de-interleaved analyzer-channel samples.
    inter: Vec<f32>,
}

impl Analyzer {
    fn new() -> Self {
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
            live_peak: 0.0,
            fft,
            window,
            inter: Vec::new(),
        }
    }

    /// (Re)initialize for a new stream. Fails only on invalid analyzer params.
    fn configure(
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
        self.live_peak = 0.0;
        Ok(())
    }

    /// Tear down the active measurement (called on Stop).
    fn shutdown(&mut self) {
        self.ebu = None;
        self.mono_ring.clear();
        self.live_peak = 0.0;
    }

    /// Process a chunk of interleaved frames: de-interleave the selected
    /// channels, accumulate a mono downmix, feed the loudness analyzer, and
    /// track the live true peak. Runs on the engine thread (no locks).
    fn process(&mut self, data: &[f32]) {
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
                let mut p = 0.0f64;
                for c in 0..n as u32 {
                    if let Ok(v) = ebu.prev_true_peak(c) {
                        p = p.max(v);
                    }
                }
                if p > self.live_peak {
                    self.live_peak = p;
                }
            }
        }
    }

    /// Re-initialize the integrated/LRA/peak measurement (keeps device running).
    fn reset(&mut self) {
        if self.sample_rate > 0 && self.channels > 0 {
            self.ebu = EbuR128::new(self.channels as u32, self.sample_rate, Mode::all()).ok();
        }
        self.mono_ring.clear();
        self.live_peak = 0.0;
    }

    fn spectrum(&self) -> Vec<f32> {
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

    fn metrics(&mut self) -> Metrics {
        let sr = self.sample_rate;
        let ch = self.channels;

        // Live peak: max since last emit, then reset the accumulator.
        let live_lin = self.live_peak;
        self.live_peak = 0.0;

        let (momentary, short_term, integrated, lra, max_lin) = match self.ebu.as_ref() {
            Some(e) => {
                let mut peak = 0.0f64;
                for c in 0..ch as u32 {
                    if let Ok(p) = e.true_peak(c) {
                        peak = peak.max(p);
                    }
                }
                (
                    clean(e.loudness_momentary().ok(), LOUDNESS_FLOOR),
                    clean(e.loudness_shortterm().ok(), LOUDNESS_FLOOR),
                    clean(e.loudness_global().ok(), LOUDNESS_FLOOR),
                    clean(e.loudness_range().ok(), 0.0),
                    peak,
                )
            }
            None => (LOUDNESS_FLOOR, LOUDNESS_FLOOR, LOUDNESS_FLOOR, 0.0, 0.0),
        };

        Metrics {
            momentary,
            short_term,
            integrated,
            lra,
            true_peak_db: lin_to_db(live_lin),
            true_peak_max_db: lin_to_db(max_lin),
            spectrum: self.spectrum(),
            sample_rate: sr,
            channels: ch,
        }
    }
}

/// Map an optional/non-finite loudness value to a finite floored value
/// (serde_json cannot serialize NaN / infinity).
fn clean(v: Option<f64>, floor: f64) -> f64 {
    match v {
        Some(x) if x.is_finite() => x.max(floor),
        _ => floor,
    }
}

/// Convert a linear amplitude to dB, floored (and always finite for serde).
fn lin_to_db(lin: f64) -> f64 {
    if lin > 1e-9 {
        (20.0 * lin.log10()).max(PEAK_FLOOR)
    } else {
        PEAK_FLOOR
    }
}

/// Validate that the requested source channels exist on the device and form a
/// valid 1- or 2-channel selection (mono or a stereo pair).
fn validate_selection(sel: &[usize], device_channels: usize) -> Result<(), String> {
    if sel.is_empty() {
        return Err("no channels selected".into());
    }
    if sel.len() > 2 {
        return Err(format!(
            "too many channels selected ({}); meter 1 (mono) or 2 (stereo)",
            sel.len()
        ));
    }
    if sel.len() == 2 && sel[0] == sel[1] {
        return Err("the two selected channels must be different".into());
    }
    if let Some(&mx) = sel.iter().max() {
        if mx >= device_channels {
            return Err(format!(
                "channel {} out of range (device exposes {} channels)",
                mx + 1,
                device_channels
            ));
        }
    }
    Ok(())
}

/// Platform-specific guidance appended to capture failures that are commonly
/// caused by the OS withholding microphone access (the usual reason a build or
/// start fails with an opaque backend error). Kept actionable: it names the
/// exact place to grant access so the user can fix it without guessing.
fn mic_permission_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        " If this is the first time starting capture, macOS may be blocking \
microphone access — open System Settings → Privacy & Security → Microphone, \
enable MeterMaid, then try again."
    } else if cfg!(target_os = "windows") {
        " Windows may be blocking microphone access — open Settings → Privacy & \
security → Microphone, allow desktop apps to access the microphone, then try \
again."
    } else {
        " Your system may be blocking microphone access, or another application \
may be using the device exclusively."
    }
}

/// Map a cpal `Error` raised while reading a device's capabilities
/// (`default_input_config`) to an actionable message naming the device.
fn explain_default_config_error(device: &str, err: cpal::Error) -> String {
    use cpal::ErrorKind::*;
    match err.kind() {
        DeviceNotAvailable | DeviceChanged => {
            format!("“{device}” is no longer available. Reconnect it or pick another input device.")
        }
        UnsupportedConfig | UnsupportedOperation => {
            format!("“{device}” doesn’t expose a capture format MeterMaid can read.")
        }
        _ => format!(
            "Couldn’t read the audio settings for “{device}”: {err}.{}",
            mic_permission_hint()
        ),
    }
}

/// Map a cpal `Error` raised while opening the capture stream
/// (`build_input_stream`) to an actionable message. Backend failures — where a
/// denied microphone permission usually lands — carry the permission hint.
fn explain_build_error(device: &str, err: cpal::Error) -> String {
    use cpal::ErrorKind::*;
    match err.kind() {
        DeviceNotAvailable | DeviceChanged => {
            format!("“{device}” is no longer available. Reconnect it or pick another input device.")
        }
        // ASIO drivers are exclusive-access: only one app may hold the device.
        DeviceBusy => format!(
            "“{device}” is in use by another application. ASIO devices allow only one app at a \
time — close the other app (such as a DAW) and try again."
        ),
        UnsupportedConfig => format!(
            "“{device}” doesn’t support the selected sample rate or channels. \
Try a different sample rate."
        ),
        InvalidInput => format!(
            "MeterMaid requested invalid capture settings for “{device}”. \
Try a different channel or sample-rate selection."
        ),
        _ => format!(
            "Couldn’t open “{device}” for capture: {err}.{}",
            mic_permission_hint()
        ),
    }
}

/// Map a cpal `Error` raised while starting the built stream (`play`) to an
/// actionable message.
fn explain_play_error(device: &str, err: cpal::Error) -> String {
    use cpal::ErrorKind::*;
    match err.kind() {
        DeviceNotAvailable | DeviceChanged => {
            format!("“{device}” is no longer available. Reconnect it or pick another input device.")
        }
        _ => format!(
            "Couldn’t start capture on “{device}”: {err}.{}",
            mic_permission_hint()
        ),
    }
}

/// The ASIO host (x64 Windows only); absent on every other build. cpal keeps
/// WASAPI as the default host, so ASIO is opened explicitly as a second host.
#[cfg(all(windows, target_arch = "x86_64"))]
fn asio_host() -> Result<cpal::Host, String> {
    cpal::host_from_id(cpal::HostId::Asio).map_err(|e| format!("The ASIO host is unavailable: {e}"))
}

/// Find an input device by its raw name within a specific host.
fn find_in_host(host: &cpal::Host, name: &str) -> Result<cpal::Device, String> {
    host.input_devices()
        .map_err(|e| format!("Couldn’t list input devices: {e}"))?
        .find(|d| d.to_string() == name)
        .ok_or_else(|| {
            format!(
                "Input device “{name}” wasn’t found. It may have been disconnected — \
pick another device from the list."
            )
        })
}

/// Resolve a device id (as produced by `list_input_devices`) to a cpal device,
/// routing to the host the id names. `None` selects the default host's default
/// device. An `asio:`-prefixed id targets the ASIO host (x64 Windows); any other
/// id is a default-host device name (so settings saved before ASIO still work).
fn find_device(id: &Option<String>) -> Result<cpal::Device, String> {
    let Some(id) = id else {
        return cpal::default_host().default_input_device().ok_or_else(|| {
            "No input device found. Connect a microphone or audio interface and try again."
                .to_string()
        });
    };
    #[cfg(all(windows, target_arch = "x86_64"))]
    if let Some(name) = id.strip_prefix(ASIO_ID_PREFIX) {
        return find_in_host(&asio_host()?, name);
    }
    find_in_host(&cpal::default_host(), id)
}

/// Enumerate input devices. `include_asio` controls whether the (x64-Windows)
/// ASIO host is also enumerated: cpal must *load* each ASIO driver to list it,
/// which is slow and disturbs other ASIO apps, so the idle hotplug poll passes
/// `false` and only refreshes ASIO when the default-host topology changes.
pub fn list_input_devices(include_asio: bool) -> Result<Vec<DeviceInfo>, String> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().map(|d| d.to_string());
    let mut out = Vec::new();
    for device in host.input_devices().map_err(|e| e.to_string())? {
        let name = device.to_string();
        let is_default = Some(&name) == default_name.as_ref();
        out.push(DeviceInfo {
            id: name.clone(),
            name,
            host: "default".to_string(),
            is_default,
        });
    }
    if include_asio {
        #[cfg(all(windows, target_arch = "x86_64"))]
        append_asio_devices(&mut out);
    }
    Ok(out)
}

/// Append ASIO-host input devices (x64 Windows). Best-effort: a missing or flaky
/// ASIO driver must never break the default-host list, so errors are swallowed.
/// cpal loads each driver to enumerate it, so drivers for absent hardware (e.g.
/// the Helix / HX-Stomp interposers) simply don't appear.
#[cfg(all(windows, target_arch = "x86_64"))]
fn append_asio_devices(out: &mut Vec<DeviceInfo>) {
    let Ok(host) = asio_host() else {
        return;
    };
    let Ok(devices) = host.input_devices() else {
        return;
    };
    for device in devices {
        let name = device.to_string();
        out.push(DeviceInfo {
            id: format!("{ASIO_ID_PREFIX}{name}"),
            name,
            host: "asio".to_string(),
            is_default: false,
        });
    }
}

pub fn device_config(name: Option<String>) -> Result<DeviceConfig, String> {
    let device = find_device(&name)?;
    let dev_name = device.to_string();
    let default = device
        .default_input_config()
        .map_err(|e| explain_default_config_error(&dev_name, e))?;
    let channels = default.channels();
    let default_sample_rate = default.sample_rate();

    let mut rates = BTreeSet::new();
    rates.insert(default_sample_rate);
    if let Ok(ranges) = device.supported_input_configs() {
        for range in ranges {
            // Only offer rates from ranges that match the channel count and
            // sample format `build_stream` will actually use (the device's
            // default config). A rate valid only for some other format/channel
            // count would otherwise appear in the picker and then fail Start
            // with StreamConfigNotSupported.
            if range.channels() != channels || range.sample_format() != default.sample_format() {
                continue;
            }
            let min = range.min_sample_rate();
            let max = range.max_sample_rate();
            for &cand in CANDIDATE_RATES.iter() {
                if cand >= min && cand <= max {
                    rates.insert(cand);
                }
            }
        }
    }

    Ok(DeviceConfig {
        channels,
        default_sample_rate,
        sample_rates: rates.into_iter().collect(),
    })
}

/// A built-but-not-yet-playing capture stream plus everything the engine thread
/// needs to drain and analyze it.
struct BuiltStream {
    stream: cpal::Stream,
    consumer: ringbuf::HeapCons<f32>,
    /// Samples dropped on ring overrun, tallied lock-free by the realtime
    /// callback and logged off the realtime thread by the engine.
    dropped: Arc<AtomicU64>,
    info: StreamInfo,
    sample_rate: u32,
    sel: Vec<usize>,
    device_channels: usize,
}

fn build_stream(
    app: &AppHandle,
    device_name: Option<String>,
    sample_rate: Option<u32>,
    sel: Vec<u32>,
) -> Result<BuiltStream, String> {
    let device = find_device(&device_name)?;
    let dev_name = device.to_string();

    // Debug-only: force a representative capture failure so the error UI can be
    // exercised without unplugging hardware or revoking permissions. Run the dev
    // app with `METERMAID_SIMULATE_ERROR=1` and press Start. Compiled out of
    // release builds.
    #[cfg(debug_assertions)]
    if std::env::var_os("METERMAID_SIMULATE_ERROR").is_some() {
        return Err(explain_build_error(
            &dev_name,
            cpal::Error::with_message(
                cpal::ErrorKind::BackendError,
                "simulated failure (METERMAID_SIMULATE_ERROR)",
            ),
        ));
    }
    let default = device
        .default_input_config()
        .map_err(|e| explain_default_config_error(&dev_name, e))?;
    let device_channels = default.channels();
    let sample_format = default.sample_format();
    let rate = sample_rate.unwrap_or_else(|| default.sample_rate());

    let sel_idx: Vec<usize> = sel.iter().map(|&c| c as usize).collect();
    validate_selection(&sel_idx, device_channels as usize)?;
    let analyzer_ch = sel_idx.len() as u16;

    let config = cpal::StreamConfig {
        channels: device_channels,
        sample_rate: rate,
        buffer_size: cpal::BufferSize::Default,
    };

    // Lock-free handoff from the realtime callback to the engine thread, sized
    // to roughly one second of audio so it never overflows between drains.
    let cap = (rate.max(48_000) as usize) * device_channels as usize;
    let (mut producer, consumer) = HeapRb::<f32>::new(cap).split();

    // Dropped-sample tally. The realtime callback only does a relaxed atomic
    // add (no lock, no allocation); the engine thread logs and clears it.
    let dropped = Arc::new(AtomicU64::new(0));

    // cpal invokes this on its own thread when the device faults (e.g. it is
    // unplugged mid-capture). Forward it to the UI so the user sees a reason
    // rather than a silently frozen meter.
    let err_app = app.clone();
    let on_error = move |err: cpal::Error| {
        eprintln!("audio stream error: {err}");
        let _ = err_app.emit("stream-error", err.to_string());
    };

    let cb_dropped = Arc::clone(&dropped);
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let pushed = producer.push_slice(data);
                if pushed < data.len() {
                    cb_dropped.fetch_add((data.len() - pushed) as u64, Ordering::Relaxed);
                }
            },
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => {
            // Pre-sized to the ring capacity so the realtime callback converts
            // in place and never reallocates (one callback can't exceed a full
            // ring's worth of samples).
            let mut scratch = vec![0.0f32; cap];
            device.build_input_stream(
                config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let n = data.len().min(scratch.len());
                    for (dst, &s) in scratch[..n].iter_mut().zip(data) {
                        *dst = s as f32 / 32768.0;
                    }
                    let pushed = producer.push_slice(&scratch[..n]);
                    if pushed < data.len() {
                        cb_dropped.fetch_add((data.len() - pushed) as u64, Ordering::Relaxed);
                    }
                },
                on_error,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let mut scratch = vec![0.0f32; cap];
            device.build_input_stream(
                config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    let n = data.len().min(scratch.len());
                    for (dst, &s) in scratch[..n].iter_mut().zip(data) {
                        *dst = (s as f32 - 32768.0) / 32768.0;
                    }
                    let pushed = producer.push_slice(&scratch[..n]);
                    if pushed < data.len() {
                        cb_dropped.fetch_add((data.len() - pushed) as u64, Ordering::Relaxed);
                    }
                },
                on_error,
                None,
            )
        }
        // 24-bit packed PCM — what pro-audio ASIO drivers (e.g. the Line 6
        // Helix) report. cpal stores I24 in a 4-byte container; `from_sample`
        // does the scaled conversion to f32.
        cpal::SampleFormat::I24 => {
            let mut scratch = vec![0.0f32; cap];
            device.build_input_stream(
                config,
                move |data: &[cpal::I24], _: &cpal::InputCallbackInfo| {
                    let n = data.len().min(scratch.len());
                    for (dst, &s) in scratch[..n].iter_mut().zip(data) {
                        *dst = f32::from_sample(s);
                    }
                    let pushed = producer.push_slice(&scratch[..n]);
                    if pushed < data.len() {
                        cb_dropped.fetch_add((data.len() - pushed) as u64, Ordering::Relaxed);
                    }
                },
                on_error,
                None,
            )
        }
        other => {
            return Err(format!(
                "“{dev_name}” uses an audio format MeterMaid can’t read ({other:?})."
            ))
        }
    }
    .map_err(|e| explain_build_error(&dev_name, e))?;

    Ok(BuiltStream {
        stream,
        consumer,
        dropped,
        info: StreamInfo {
            device_name: dev_name,
            sample_rate: rate,
            channels: analyzer_ch,
        },
        sample_rate: rate,
        sel: sel_idx,
        device_channels: device_channels as usize,
    })
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
    let mut active: Option<ActiveStream> = None;
    let mut analyzer = Analyzer::new();
    let mut drain = vec![0.0f32; DRAIN_CHUNK];

    loop {
        match rx.recv_timeout(EMIT_INTERVAL) {
            Ok(Command::Start {
                device,
                sample_rate,
                channels,
                reply,
            }) => {
                active = None; // stop any existing stream first
                match build_stream(&app, device, sample_rate, channels) {
                    Ok(built) => {
                        let dev_name = built.info.device_name.clone();
                        if let Err(e) =
                            analyzer.configure(built.sample_rate, built.sel, built.device_channels)
                        {
                            let _ = reply.send(Err(e));
                            continue;
                        }
                        match built.stream.play() {
                            Ok(()) => {
                                active = Some(ActiveStream {
                                    stream: built.stream,
                                    consumer: built.consumer,
                                    dropped: built.dropped,
                                });
                                let _ = reply.send(Ok(built.info));
                            }
                            Err(e) => {
                                analyzer.shutdown();
                                let _ = reply.send(Err(explain_play_error(&dev_name, e)));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
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
                        let got = active.consumer.pop_slice(&mut drain);
                        if got == 0 {
                            break;
                        }
                        analyzer.process(&drain[..got]);
                    }
                    // Report any ring overruns the realtime callback tallied,
                    // off the realtime thread.
                    let dropped = active.dropped.swap(0, Ordering::Relaxed);
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

/// An active capture: the cpal stream (kept alive to keep the device running),
/// its SPSC consumer, and the lock-free dropped-sample counter.
struct ActiveStream {
    /// Held only for its `Drop`: dropping the stream stops the device.
    #[allow(dead_code)]
    stream: cpal::Stream,
    consumer: ringbuf::HeapCons<f32>,
    dropped: Arc<AtomicU64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    // ASIO drivers are single-instance: two tests loading the driver at once
    // (as `cargo test -- --include-ignored` does, since it runs ignored tests
    // in parallel) collide and one sees zero channels. Serialize the ASIO
    // hardware tests on this lock. `into_inner` recovers from a poisoned lock
    // so one failing ASIO test doesn't cascade into the other.
    #[cfg(all(windows, target_arch = "x86_64"))]
    static ASIO_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Build a configured analyzer for the common mono-device case.
    fn analyzer(sample_rate: u32, device_channels: usize, sel: Vec<usize>) -> Analyzer {
        let mut a = Analyzer::new();
        a.configure(sample_rate, sel, device_channels).unwrap();
        a
    }

    /// Interleaved frames for a single-channel (mono) device: a pure tone.
    fn mono_sine(freq: f32, amp: f32, secs: f32, sr: u32) -> Vec<f32> {
        let n = (secs * sr as f32) as usize;
        (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / sr as f32).sin())
            .collect()
    }

    // --- Pure helpers -------------------------------------------------------

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

    // --- Error messages -----------------------------------------------------

    #[test]
    fn build_error_messages_name_the_device_and_are_actionable() {
        // A vanished device tells the user to reconnect or pick another.
        let msg = explain_build_error(
            "Scarlett 2i2",
            cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable),
        );
        assert!(msg.contains("Scarlett 2i2"), "got: {msg}");
        assert!(msg.contains("no longer available"), "got: {msg}");

        // An unsupported config points at the sample rate.
        let msg = explain_build_error(
            "Built-in Mic",
            cpal::Error::new(cpal::ErrorKind::UnsupportedConfig),
        );
        assert!(msg.contains("sample rate"), "got: {msg}");

        // A device held exclusively (ASIO) names the conflict.
        let msg = explain_build_error("Helix", cpal::Error::new(cpal::ErrorKind::DeviceBusy));
        assert!(msg.contains("Helix"), "got: {msg}");
        assert!(
            msg.to_lowercase().contains("another application"),
            "got: {msg}"
        );

        // Opaque backend failures (where denied mic permission lands) carry the
        // permission hint.
        let backend =
            cpal::Error::with_message(cpal::ErrorKind::BackendError, "kAudioUnitErr_NoConnection");
        let msg = explain_build_error("Built-in Mic", backend);
        assert!(msg.contains("Built-in Mic"), "got: {msg}");
        assert!(msg.to_lowercase().contains("microphone"), "got: {msg}");
    }

    #[test]
    fn play_error_carries_permission_hint() {
        let backend = cpal::Error::with_message(cpal::ErrorKind::BackendError, "denied");
        let msg = explain_play_error("Mic", backend);
        assert!(msg.contains("Mic"), "got: {msg}");
        assert!(msg.to_lowercase().contains("microphone"), "got: {msg}");
    }

    // --- Channel selection / de-interleave ---------------------------------

    #[test]
    fn validate_selection_rejects_empty_and_out_of_range() {
        assert!(validate_selection(&[], 2)
            .unwrap_err()
            .contains("no channels"));
        let err = validate_selection(&[2], 2).unwrap_err();
        assert!(err.contains("out of range"), "got: {err}");
        assert!(err.contains("channel 3"), "1-based label, got: {err}");
        assert!(validate_selection(&[0, 1], 2).is_ok());
    }

    #[test]
    fn validate_selection_rejects_too_many_and_duplicates() {
        // More than a stereo pair is not a valid selection.
        assert!(validate_selection(&[0, 1, 2], 4)
            .unwrap_err()
            .contains("too many"));
        // A "stereo" pair pointing at the same channel is rejected.
        assert!(validate_selection(&[1, 1], 2)
            .unwrap_err()
            .contains("different"));
    }

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

    // --- ASIO enumeration spike (Phase 1, manual) ---------------------------
    //
    // Validates the premise behind Windows multichannel support: that a
    // multichannel interface (e.g. the Line 6 Helix) reports its full native
    // channel count through the ASIO host, where WASAPI shared mode reports
    // only the endpoint default (often mono). Requires the `asio` cpal feature
    // (x64 Windows), the ASIO SDK build chain, and the device plugged in and
    // not held by another app. Ignored by default; run with:
    //   cargo test asio_enumerates_devices -- --ignored --nocapture
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    #[ignore]
    fn asio_enumerates_devices() {
        use cpal::HostId;

        let _asio = ASIO_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let host = cpal::host_from_id(HostId::Asio).expect(
            "ASIO host unavailable — is the `asio` feature enabled and a driver installed?",
        );

        // Iterate ALL devices (not input_devices(), which silently drops any
        // driver whose config query fails) so a failed driver load is visible
        // rather than vanishing. Two of the registered drivers are interposers
        // for hardware that may be absent — we expect those to error.
        let devices: Vec<_> = host.devices().expect("enumerate ASIO devices").collect();

        eprintln!("ASIO devices (all): {}", devices.len());
        let mut max_channels = 0u16;
        for device in &devices {
            let name = device.to_string();
            match device.default_input_config() {
                Ok(cfg) => {
                    max_channels = max_channels.max(cfg.channels());
                    eprintln!(
                        "  [ok]  {name}: {} in-ch @ {} Hz ({:?})",
                        cfg.channels(),
                        cfg.sample_rate(),
                        cfg.sample_format()
                    );
                }
                Err(e) => eprintln!("  [err] {name}: default_input_config: {e}"),
            }
        }

        assert!(!devices.is_empty(), "no ASIO drivers registered/visible");
        assert!(
            max_channels > 1,
            "no ASIO driver yielded a multichannel input config (max was {max_channels}); \
see per-device errors above"
        );
    }

    // Exercises the Phase 2 merged enumeration: `list_input_devices` should
    // surface ASIO devices alongside the default host, tagged with host "asio"
    // and an `asio:`-prefixed id that round-trips back through `find_device`.
    // Requires an ASIO device plugged in. Run with:
    //   cargo test list_devices_includes_asio -- --ignored --nocapture
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    #[ignore]
    fn list_devices_includes_asio() {
        let _asio = ASIO_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let devices = list_input_devices(true).expect("list devices");
        for d in &devices {
            eprintln!("  host={} id={:?} name={:?}", d.host, d.id, d.name);
        }
        let asio: Vec<_> = devices.iter().filter(|d| d.host == "asio").collect();
        assert!(
            !asio.is_empty(),
            "no ASIO devices surfaced by list_input_devices"
        );
        for d in asio {
            assert!(
                d.id.starts_with(ASIO_ID_PREFIX),
                "ASIO device id should be prefixed: {:?}",
                d.id
            );
            // The id must resolve back to a real device through find_device.
            find_device(&Some(d.id.clone()))
                .unwrap_or_else(|e| panic!("find_device failed for {:?}: {e}", d.id));
        }
    }
}
