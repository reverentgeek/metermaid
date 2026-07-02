# Changelog

All notable changes to MeterMaid are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.3] - 2026-07-01

### Fixed

- **Channel misalignment when metering multichannel devices.** The engine drained the capture ring in fixed 8192-sample chunks, but the analyzer drops a trailing partial frame — so on devices whose channel count doesn't divide 8192 (6-, 10-, 12-, or 18-channel interfaces, on any platform) every full chunk rotated the channel alignment of everything drained after it, and the meter silently blended the wrong channels. The drain buffer is now sized per stream to a whole number of interleaved frames, covered by a 6-channel golden-signal regression test. Mono, stereo, 4-, and 8-channel devices were unaffected.
- **Channel misalignment after a capture-ring overrun.** The realtime callback's `push_slice` could push a partial frame when the ring was nearly full, permanently rotating channel alignment even after the overrun cleared. The callback now clamps each push to the ring's free space rounded down to a whole frame (still lock- and allocation-free) and tallies the remainder as dropped.
- **Stale spectrum peak-hold across capture sessions.** Starting a new capture no longer shows the previous session's decaying peak-hold line over the fresh spectrum.
- **Clip light could re-latch immediately after Reset.** A `meter-update` already in flight when Reset was pressed could still carry the old held true-peak max and instantly re-latch the clip indicator. `Metrics` now carries a measurement `generation` (bumped by the engine on every stream configure and Reset) and the UI ignores clip latching for frames from before the reset it requested.

### Changed

- **Device listing and capture start/stop no longer run on the webview's main thread.** These Tauri commands block — enumerating devices can load ASIO drivers (seconds) and start/stop waits on the audio engine's reply — which could visibly freeze the UI. They are now declared `#[tauri::command(async)]` and run on a background thread pool.

## [0.4.2] - 2026-06-28

### Fixed

- **macOS: input devices not detected and no microphone prompt.** MeterMaid now explicitly requests microphone access via AVFoundation (`AVCaptureDevice`) at launch. cpal's CoreAudio HAL never triggers the OS permission prompt on its own, and on older macOS (e.g. Catalina) the HAL returns an empty input-device list until access is granted — leaving the picker stuck with no devices and no way to grant permission. Requesting access up front raises the prompt; once granted, the device picker repopulates. ([#31](https://github.com/reverentgeek/metermaid/issues/31))
- **Older macOS (Catalina): blank/non-functional UI.** Set the Vite build target to `safari13` so the frontend bundle is transpiled for the older system WebKit shipped on macOS 10.15. Previously the bundle used syntax too new for that WebKit to parse, so the JavaScript silently never executed — the window rendered but every control (device/channel/rate pickers, buttons) was dead. ([#31](https://github.com/reverentgeek/metermaid/issues/31))

## [0.4.1] - 2026-06-27

### Internal

- Updated Tauri to 2.11.3 / tauri-build 2.6.3 (patch releases; pulls in tauri-utils 2.9.3 and tray-icon 0.24.1). Lockfile-only dependency maintenance with no behavior change.

## [0.4.0] - 2026-06-27

### Added

- **Multichannel capture on Windows via ASIO.** Multichannel interfaces such as the Line 6 Helix Stadium XL now expose all their channels on Windows (e.g. Ch 1–8) through the **ASIO** host, where WASAPI shared mode reported only a single channel. ASIO devices appear in the input picker tagged `(ASIO)` next to their WASAPI endpoint; pick the `(ASIO)` entry for multichannel. ASIO is exclusive-access and its sample rate is set in the device's own control panel. This is x64-Windows only (no ARM64 ASIO SDK); macOS, Linux, and Windows-ARM64 are unaffected. See the README "ASIO multichannel capture" section.

### Changed

- Upgraded **cpal 0.15 → 0.18**, which adds the 24-bit ASIO sample support the Helix driver requires (plus newer audio-backend improvements across platforms).

### Internal

- The distributed **Windows-x64 binary is now licensed GPLv3** because it links the (GPLv3-or-Steinberg) ASIO SDK; MeterMaid's source and all other platform binaries remain MIT. The SDK is vendored at `third-party/asio` and linked only on the `x86_64-pc-windows-msvc` target. CI gained a Windows-x64 leg that build-tests the ASIO path.

## [0.3.3] - 2026-06-25

### Added

- **Space resets the measurement** while capturing. The leveling loop hits Reset constantly between patches, so the shortcut removes a mouse round-trip from the most-repeated action. It's ignored when focus is in an input, select, or button so Space still types and activates normally there, and the Reset button tooltip advertises it.

### Changed

- The About dialog now puts **Submit feedback** on its own line so it no longer wraps mid-phrase.

### Internal

- Added a committed `cspell.json` project dictionary with the shared domain word list (`cpal`, `ebur128`, `lufs`, `rustfft`, `ringbuf`, `minisign`, …) so editors stop flagging domain terms.

## [0.3.2] - 2026-06-25

### Fixed

- The spectrum plot could occasionally start blank when metering began, correcting itself only after the window was resized. The canvas backing store now re-syncs on any layout change (such as bundled fonts finishing loading and reflowing the header), not just on window resizes.

## [0.3.1] - 2026-06-25

### Added

- An application menu with **About MeterMaid** and **Check for Updates…** items: on macOS in the app menu, and on Windows/Linux under a new **Help** menu. Check for Updates runs the same check as the in-app button.
- A redesigned **About MeterMaid** dialog: a centered in-app panel showing the app version, description, author, and copyright, with clickable links to the author's website, the GitHub repository, and the issue tracker (opened in the default browser).

## [0.3.0] - 2026-06-25

### Added

- MeterMaid can now update itself. It checks GitHub Releases for a newer signed build on launch (quietly, nothing appears unless an update exists) and via a **Check for updates** button in the config row. When a newer version is available, a banner shows the version and release notes with a one-click **Install & Restart**; the download is signature-verified before it is applied. Self-update covers the macOS, Windows, and Linux AppImage builds (Linux `.deb`/`.rpm` installs continue to update through their package manager).

## [0.2.0] - 2026-06-23

### Added

- The input-device list now updates automatically while the app is idle: plug in or remove a USB/audio device and it appears in (or disappears from) the picker within a couple of seconds, without reopening the dropdown. The current selection is preserved, and if the selected device is unplugged the picker falls back to the system default with a notice that clears once a device is reselected or reconnected.

### Fixed

- The spectrum canvas no longer redraws at the display refresh rate while idle. The render loop now runs only while capturing and otherwise repaints once per state change, eliminating several percent of idle CPU/GPU usage when metering is stopped.
- The sample-rate picker no longer offers rates that the device only supports under a different channel count or sample format than the one capture actually uses. Such rates would appear selectable and then fail on Start; the picker is now filtered to the configuration the capture stream is built with.
- Auto-start now begins capture only after the meter and stream-error listeners are registered, so an immediate stream fault on launch is surfaced instead of leaving the UI with controls disabled and no visible error.

## [0.1.1] - 2026-06-22

### Changed

- Capture failures now produce plain-language, actionable messages instead of raw backend strings: they name the device, suggest a fix (e.g. reconnect the device or try a different sample rate), and add an OS-specific hint to check microphone permission when a start fails for an opaque reason.
- Errors are now shown in a dismissible banner with the full, selectable message and a **Copy** button (plus a link to the issue tracker) so they can be read and reported, rather than truncated in the toolbar status.
- The default target loudness is now −20 LUFS (was −14). Only affects fresh installs; a previously saved target is still restored.
- While capturing, **Reset** is now the prominent (amber) primary control for the action you take between patch changes, and **Stop** is a quiet secondary button; Reset is hidden when idle. A one-time hint on first capture explains the Reset-between-patches workflow.
- The UI now bundles its fonts (Inter for the interface, JetBrains Mono for the numeric readouts and spectrum labels) so typography is identical on macOS, Windows, and Linux instead of falling back to each OS's default fonts.

### Fixed

- The Target and Ceiling steppers are now custom, theme-styled up/down controls. The native number-input arrows rendered nearly invisibly on the dark theme (and differently on each platform); the replacements are legible and consistent everywhere.

## [0.1.0] - 2026-06-18

### Added

- Initial release: ITU-R BS.1770 / EBU R128 loudness metering (integrated, short-term, momentary, LRA), true-peak with peak-hold, a log-frequency spectrum analyzer, and a target/apply gain helper.
- Persist configuration between sessions: window size/position/monitor (via `tauri-plugin-window-state`), plus the selected audio device, channels, sample rate, target LUFS, and clip ceiling (via `tauri-plugin-store`). Restored selections are re-validated against the current device: a missing device falls back to the system default with a notice, and out-of-range channels or sample rates fall back gracefully.
- If the monitor the window was last shown on is gone at launch (e.g. an external display was unplugged), the window is recentered on an available monitor instead of restoring off-screen.
- Optional **Auto-start** toggle that begins capture on launch when a valid saved device and channels are restored.
- Surface OS audio stream faults (e.g. the device being unplugged mid-capture) in the UI instead of silently freezing the meter.

### Changed

- True-peak and spectrum peak-hold ballistics now fall at a fixed dB/second rate, independent of display refresh rate and the engine's emit cadence.
- The realtime audio callback no longer logs on ring overrun; it tallies dropped samples lock-free and the engine thread reports them, keeping the callback allocation- and lock-free even under overrun.
- Tightened the webview Content-Security-Policy (`style-src` no longer allows `'unsafe-inline'`).
- CI now also builds and tests on macOS and runs `cargo audit`.

### Fixed

- Reject invalid channel selections (more than two channels, or a stereo pair pointing at the same channel) in the audio engine.
