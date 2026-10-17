//! Audio capture driving the `metermaid_dsp` loudness/spectrum analyzer.
//!
//! A single dedicated thread (`engine_loop`) owns the cpal stream because
//! `cpal::Stream` is not `Send`. The realtime audio callback does no locking
//! and no heap allocation in steady state: it only copies the incoming frames
//! into a lock-free single-producer/single-consumer ring. The engine thread is
//! the sole owner of the `Analyzer` (no synchronization needed); on a fixed
//! cadence it drains the ring into the analyzer (which de-interleaves the
//! user-selected channels and measures BS.1770 loudness, true peak and the
//! spectrum) and emits metrics to the UI.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use metermaid_dsp::{Analyzer, Metrics};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::HeapRb;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Emit cadence.
const EMIT_INTERVAL: Duration = Duration::from_millis(33);
/// Sample rates offered in the UI when within a device's supported range.
const CANDIDATE_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Upper bound (samples) on the scratch buffer used to drain the SPSC ring on
/// the engine thread. The buffer is sized per stream via [`drain_chunk_len`]
/// so a chunk always holds whole interleaved frames.
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
    Ok(dedup_devices(out))
}

/// Collapse entries that share a device id, keeping the first occurrence.
/// ALSA surfaces one physical card as several PCM aliases (`sysdefault`,
/// `front`, `plughw`, …) that share a description, so cpal lists the same
/// name many times over. Device identity *is* the (host-qualified) id —
/// `find_device` resolves an id to the first name match — so same-id rows
/// are indistinguishable in the picker and collapsing them loses nothing.
fn dedup_devices(devices: Vec<DeviceInfo>) -> Vec<DeviceInfo> {
    let mut seen = BTreeSet::new();
    devices
        .into_iter()
        .filter(|d| seen.insert(d.id.clone()))
        .collect()
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
    /// ASIO only: a silent output stream opened on the same device. Line 6 (and
    /// other) ASIO drivers won't run an input-only `ASIOCreateBuffers` — no
    /// callbacks fire and disposing the stream access-violates — but creating
    /// input and output buffers together works. Kept alive and played/stopped
    /// alongside the input stream. `None` on every non-ASIO path.
    output_stream: Option<cpal::Stream>,
    consumer: ringbuf::HeapCons<f32>,
    /// Samples dropped on ring overrun, tallied lock-free by the realtime
    /// callback and logged off the realtime thread by the engine.
    dropped: Arc<AtomicU64>,
    info: StreamInfo,
    sample_rate: u32,
    sel: Vec<usize>,
    device_channels: usize,
}

/// Whether a device id targets the ASIO host.
#[cfg(all(windows, target_arch = "x86_64"))]
fn is_asio_id(id: &Option<String>) -> bool {
    id.as_deref().is_some_and(|s| s.starts_with(ASIO_ID_PREFIX))
}

/// Build a silent output stream on an ASIO `device` (see
/// `BuiltStream::output_stream` for why it's needed). The realtime output
/// callback only fills its buffer with silence — no allocation or locking.
#[cfg(all(windows, target_arch = "x86_64"))]
fn build_silent_output(
    device: &cpal::Device,
    dev_name: &str,
    rate: u32,
) -> Result<cpal::Stream, String> {
    let default = device
        .default_output_config()
        .map_err(|e| explain_default_config_error(dev_name, e))?;
    let config = cpal::StreamConfig {
        channels: default.channels(),
        sample_rate: rate,
        buffer_size: cpal::BufferSize::Default,
    };
    let on_error = |err: cpal::Error| eprintln!("asio output stream error: {err}");
    let stream = match default.sample_format() {
        cpal::SampleFormat::I24 => device.build_output_stream(
            config,
            |data: &mut [cpal::I24], _: &cpal::OutputCallbackInfo| {
                data.fill(cpal::I24::new(0).unwrap_or_default());
            },
            on_error,
            None,
        ),
        cpal::SampleFormat::F32 => device.build_output_stream(
            config,
            |data: &mut [f32], _: &cpal::OutputCallbackInfo| data.fill(0.0),
            on_error,
            None,
        ),
        cpal::SampleFormat::I32 => device.build_output_stream(
            config,
            |data: &mut [i32], _: &cpal::OutputCallbackInfo| data.fill(0),
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            |data: &mut [i16], _: &cpal::OutputCallbackInfo| data.fill(0),
            on_error,
            None,
        ),
        other => {
            return Err(format!(
                "“{dev_name}” uses an ASIO output format MeterMaid can’t drive ({other:?})."
            ))
        }
    }
    .map_err(|e| explain_build_error(dev_name, e))?;
    Ok(stream)
}

/// Number of samples the realtime callback may push for one buffer: bounded by
/// the free ring space (and the conversion scratch, folded into `available` by
/// the caller) and rounded down to a whole interleaved frame. On ring overrun
/// a plain `push_slice` could push a partial frame, permanently rotating the
/// channel alignment of everything after it even once the overrun clears.
/// Clamping to the vacant space first is safe: this is the only producer, so
/// the vacant space can only grow concurrently. Lock- and allocation-free.
fn push_len(available: usize, vacant: usize, stride: usize) -> usize {
    let n = available.min(vacant);
    n - n % stride.max(1)
}

/// Whether a stream error reported by cpal's error callback means the stream
/// is dead and capture must be torn down. Some kinds are advisories on a
/// stream that keeps running: after an `Xrun` the ALSA worker re-prepares and
/// restarts the stream itself (common on machines without realtime
/// scheduling, e.g. VMs), `DeviceChanged` means the OS already rerouted the
/// stream, and `RealtimeDenied` only downgrades the audio thread's scheduling.
/// Unknown kinds (`ErrorKind` is non-exhaustive) stay fatal so a genuinely
/// dead stream is never left looking healthy.
fn is_fatal_stream_error(kind: cpal::ErrorKind) -> bool {
    !matches!(
        kind,
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied
    )
}

/// Build an input stream whose samples arrive in a non-f32 PCM format. The
/// realtime callback converts each sample to f32 into a scratch buffer
/// pre-sized to the ring capacity (one callback can't exceed a full ring's
/// worth, so it never reallocates) and pushes whole frames into the SPSC ring.
/// One generic implementation covers every integer/f64 format a backend may
/// report, so an unusual driver default (e.g. ALSA's S32_LE) doesn't need its
/// own hand-written arm.
fn build_converting_input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut producer: ringbuf::HeapProd<f32>,
    dropped: Arc<AtomicU64>,
    stride: usize,
    cap: usize,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    let mut scratch = vec![0.0f32; cap];
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let n = push_len(data.len().min(scratch.len()), producer.vacant_len(), stride);
            for (dst, &s) in scratch[..n].iter_mut().zip(data) {
                *dst = f32::from_sample(s);
            }
            let _ = producer.push_slice(&scratch[..n]);
            if n < data.len() {
                dropped.fetch_add((data.len() - n) as u64, Ordering::Relaxed);
            }
        },
        on_error,
        None,
    )
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
    // rather than a silently frozen meter — but only for faults that actually
    // end the stream; advisory errors are logged and capture continues.
    let err_app = app.clone();
    let on_error = move |err: cpal::Error| {
        eprintln!("audio stream error: {err}");
        if is_fatal_stream_error(err.kind()) {
            let _ = err_app.emit("stream-error", err.to_string());
        }
    };

    // ASIO needs input and output buffers created together (see
    // BuiltStream::output_stream). Build the silent output stream first so it's
    // part of the same ASIOCreateBuffers as the input stream below.
    #[cfg(all(windows, target_arch = "x86_64"))]
    let output_stream = if is_asio_id(&device_name) {
        Some(build_silent_output(&device, &dev_name, rate)?)
    } else {
        None
    };
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    let output_stream: Option<cpal::Stream> = None;

    let cb_dropped = Arc::clone(&dropped);
    // Frame size for the whole-frame push clamp in the callbacks (see push_len).
    let stride = device_channels as usize;
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let n = push_len(data.len(), producer.vacant_len(), stride);
                let _ = producer.push_slice(&data[..n]);
                if n < data.len() {
                    cb_dropped.fetch_add((data.len() - n) as u64, Ordering::Relaxed);
                }
            },
            on_error,
            None,
        ),
        // Every other PCM format converts to f32 through the shared scratch
        // path. Which one arrives is the backend's choice, not the user's:
        // ASIO drivers (e.g. the Line 6 Helix) report I24, ALSA typically
        // reports S32_LE (I32) for class-compliant USB interfaces, older
        // hardware I16/U16.
        cpal::SampleFormat::I8 => build_converting_input_stream::<i8>(
            &device, config, producer, cb_dropped, stride, cap, on_error,
        ),
        cpal::SampleFormat::I16 => build_converting_input_stream::<i16>(
            &device, config, producer, cb_dropped, stride, cap, on_error,
        ),
        cpal::SampleFormat::I24 => build_converting_input_stream::<cpal::I24>(
            &device, config, producer, cb_dropped, stride, cap, on_error,
        ),
        cpal::SampleFormat::I32 => build_converting_input_stream::<i32>(
            &device, config, producer, cb_dropped, stride, cap, on_error,
        ),
        cpal::SampleFormat::I64 => build_converting_input_stream::<i64>(
            &device, config, producer, cb_dropped, stride, cap, on_error,
        ),
        cpal::SampleFormat::U8 => build_converting_input_stream::<u8>(
            &device, config, producer, cb_dropped, stride, cap, on_error,
        ),
        cpal::SampleFormat::U16 => build_converting_input_stream::<u16>(
            &device, config, producer, cb_dropped, stride, cap, on_error,
        ),
        cpal::SampleFormat::U24 => build_converting_input_stream::<cpal::U24>(
            &device, config, producer, cb_dropped, stride, cap, on_error,
        ),
        cpal::SampleFormat::U32 => build_converting_input_stream::<u32>(
            &device, config, producer, cb_dropped, stride, cap, on_error,
        ),
        cpal::SampleFormat::U64 => build_converting_input_stream::<u64>(
            &device, config, producer, cb_dropped, stride, cap, on_error,
        ),
        cpal::SampleFormat::F64 => build_converting_input_stream::<f64>(
            &device, config, producer, cb_dropped, stride, cap, on_error,
        ),
        // DSD bitstreams (DsdU8/U16/U32) aren't PCM samples; there is no
        // meaningful per-sample conversion.
        other => {
            return Err(format!(
                "“{dev_name}” uses an audio format MeterMaid can’t read ({other:?})."
            ))
        }
    }
    .map_err(|e| explain_build_error(&dev_name, e))?;

    Ok(BuiltStream {
        stream,
        output_stream,
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

/// Largest whole-frame multiple of `device_channels` that fits in
/// [`DRAIN_CHUNK`]. The drain buffer must hold whole interleaved frames:
/// `Analyzer::process` ignores a trailing partial frame, so a chunk boundary
/// that split a frame would rotate the channel alignment of every sample
/// drained after it — 8192 is not a multiple of e.g. a 6- or 10-channel
/// interface's frame size, and those devices fill more than one chunk per
/// 33 ms tick. A degenerate channel count larger than `DRAIN_CHUNK` still
/// gets one whole frame.
fn drain_chunk_len(device_channels: usize) -> usize {
    let ch = device_channels.max(1);
    if ch >= DRAIN_CHUNK {
        ch
    } else {
        DRAIN_CHUNK - DRAIN_CHUNK % ch
    }
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
                        // Start the paired ASIO output stream (if any) before the
                        // input, mirroring the order the driver tolerates.
                        if let Some(out) = &built.output_stream {
                            if let Err(e) = out.play() {
                                analyzer.shutdown();
                                let _ = reply.send(Err(explain_play_error(&dev_name, e)));
                                continue;
                            }
                        }
                        match built.stream.play() {
                            Ok(()) => {
                                // Size the drain buffer to whole frames for
                                // this stream's channel count.
                                drain.resize(drain_chunk_len(built.device_channels), 0.0);
                                active = Some(ActiveStream {
                                    stream: built.stream,
                                    output_stream: built.output_stream,
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
    /// Held only for its `Drop`: dropping the stream stops the device. The input
    /// stream is dropped before the output stream (field order) to match the
    /// teardown order ASIO tolerates.
    #[allow(dead_code)]
    stream: cpal::Stream,
    /// ASIO only: the paired silent output stream (see `BuiltStream`). Held for
    /// its `Drop`; `None` on non-ASIO paths.
    #[allow(dead_code)]
    output_stream: Option<cpal::Stream>,
    consumer: ringbuf::HeapCons<f32>,
    dropped: Arc<AtomicU64>,
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // --- Channel selection --------------------------------------------------

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

    // --- Drain chunking ------------------------------------------------------

    #[test]
    fn drain_chunk_len_is_whole_frames() {
        for ch in 1..=32 {
            let len = drain_chunk_len(ch);
            assert_eq!(len % ch, 0, "chunk not frame-aligned for {ch} channels");
            assert!(len > 0 && len <= DRAIN_CHUNK);
        }
        // A zero channel count must not panic or return an empty chunk.
        assert_eq!(drain_chunk_len(0), DRAIN_CHUNK);
        // Degenerate: more channels than DRAIN_CHUNK still yields one whole frame.
        assert_eq!(drain_chunk_len(DRAIN_CHUNK + 1), DRAIN_CHUNK + 1);
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

    // --- Realtime-callback push clamp ----------------------------------------

    #[test]
    fn push_len_clamps_to_whole_frames() {
        // Plenty of room: the whole buffer goes through untouched.
        assert_eq!(push_len(960, 10_000, 6), 960);
        // Overrun: clamped to the vacant space, rounded down to a whole frame.
        assert_eq!(push_len(960, 100, 6), 96);
        assert_eq!(push_len(960, 5, 6), 0); // less than one frame free
                                            // Already-aligned clamp stays aligned; zero stride must not panic.
        assert_eq!(push_len(960, 96, 6), 96);
        assert_eq!(push_len(960, 100, 0), 100);
    }

    // --- Stream-error classification ------------------------------------------

    #[test]
    fn advisory_stream_errors_do_not_tear_down_capture() {
        // cpal recovers from these itself; the stream keeps running.
        assert!(!is_fatal_stream_error(cpal::ErrorKind::Xrun));
        assert!(!is_fatal_stream_error(cpal::ErrorKind::DeviceChanged));
        assert!(!is_fatal_stream_error(cpal::ErrorKind::RealtimeDenied));
        // These end the stream (or leave it unusable) and must reach the UI.
        assert!(is_fatal_stream_error(cpal::ErrorKind::DeviceNotAvailable));
        assert!(is_fatal_stream_error(cpal::ErrorKind::StreamInvalidated));
        assert!(is_fatal_stream_error(cpal::ErrorKind::BackendError));
        assert!(is_fatal_stream_error(cpal::ErrorKind::Other));
    }

    // --- Device listing -------------------------------------------------------

    #[test]
    fn duplicate_device_ids_are_collapsed() {
        let mk = |id: &str, host: &str, is_default: bool| DeviceInfo {
            id: id.to_string(),
            name: id.trim_start_matches("asio:").to_string(),
            host: host.to_string(),
            is_default,
        };
        let out = dedup_devices(vec![
            // ALSA-style aliases: one card listed once per PCM alias.
            mk("Helix Stadium, USB Audio", "default", false),
            mk("Helix Stadium, USB Audio", "default", false),
            mk("HDA Intel PCH, ALC887-VD Analog", "default", true),
            mk("Helix Stadium, USB Audio", "default", false),
            // Same bare name under another host keeps its distinct id.
            mk("asio:Helix Stadium, USB Audio", "asio", false),
        ]);
        assert_eq!(
            out.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            vec![
                "Helix Stadium, USB Audio",
                "HDA Intel PCH, ALC887-VD Analog",
                "asio:Helix Stadium, USB Audio",
            ]
        );
        // Order is preserved and the default flag survives on the kept row.
        assert!(out[1].is_default);
    }

    // --- Sample-format conversion --------------------------------------------

    #[test]
    fn converted_sample_formats_map_full_scale_to_unity() {
        // The converting input path relies on cpal's `from_sample` mapping
        // each PCM format's range onto ±1.0 with the format's origin at 0.0.
        // Pin those semantics for the formats real backends report: I32 is
        // ALSA's S32_LE (issue #33, Helix Stadium on Linux), I24 is what ASIO
        // drivers deliver, I16/U16 come from older hardware.
        assert_eq!(f32::from_sample(i32::MIN), -1.0);
        assert_eq!(f32::from_sample(0i32), 0.0);
        assert_eq!(f32::from_sample(1i32 << 30), 0.5);
        assert_eq!(f32::from_sample(i16::MIN), -1.0);
        assert_eq!(f32::from_sample(1i16 << 14), 0.5);
        assert_eq!(f32::from_sample(u16::MIN), -1.0);
        assert_eq!(f32::from_sample(1u16 << 15), 0.0);
        assert_eq!(f32::from_sample(cpal::I24::new(-(1 << 23)).unwrap()), -1.0);
        assert_eq!(f32::from_sample(cpal::I24::new(1 << 22).unwrap()), 0.5);
        assert_eq!(f32::from_sample(cpal::U24::new(1 << 23).unwrap()), 0.0);
        assert_eq!(f32::from_sample(1u32 << 31), 0.0);
        assert_eq!(f32::from_sample(3u32 << 30), 0.5);
        assert_eq!(f32::from_sample(i8::MIN), -1.0);
        assert_eq!(f32::from_sample(u8::MIN), -1.0);
        assert_eq!(f32::from_sample(0.25f64), 0.25);
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

    // Regression test for the ASIO input-only failure: this driver delivers no
    // callbacks and access-violates on dispose unless an output stream is created
    // alongside the input (one ASIOCreateBuffers for both directions) — which is
    // exactly what build_stream does for ASIO devices. Verifies the input+output
    // pairing actually captures and tears down cleanly. Requires the device. Run:
    //   cargo test asio_input_output_captures -- --ignored --nocapture
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    #[ignore]
    fn asio_input_output_captures() {
        let _asio = ASIO_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let host = asio_host().expect("asio host");
        let device = host
            .input_devices()
            .expect("input devices")
            .next()
            .expect("an asio input device");
        let in_default = device.default_input_config().expect("default input config");
        let out_default = device
            .default_output_config()
            .expect("default output config");
        let rate = in_default.sample_rate();
        let in_config = cpal::StreamConfig {
            channels: in_default.channels(),
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let out_config = cpal::StreamConfig {
            channels: out_default.channels(),
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        };

        // Silent output stream first, then the input — same order build_stream uses.
        let out_stream = device
            .build_output_stream(
                out_config,
                |data: &mut [cpal::I24], _: &cpal::OutputCallbackInfo| {
                    data.fill(cpal::I24::new(0).unwrap_or_default());
                },
                |e| eprintln!("output stream error: {e}"),
                None,
            )
            .expect("build_output_stream");

        let count = Arc::new(AtomicU64::new(0));
        let cb_count = Arc::clone(&count);
        let in_stream = device
            .build_input_stream(
                in_config,
                move |data: &[cpal::I24], _: &cpal::InputCallbackInfo| {
                    cb_count.fetch_add(data.len() as u64, Ordering::Relaxed);
                },
                |e| eprintln!("input stream error: {e}"),
                None,
            )
            .expect("build_input_stream");

        out_stream.play().expect("play output");
        in_stream.play().expect("play input");
        std::thread::sleep(Duration::from_millis(500));
        drop(in_stream);
        drop(out_stream);

        let n = count.load(Ordering::Relaxed);
        eprintln!("captured {n} samples across callbacks");
        assert!(n > 0, "ASIO input+output stream produced no callbacks");
    }
}
