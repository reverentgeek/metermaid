# Changelog

All notable changes to MeterMaid are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2026-06-22

### Changed

- Capture failures now produce plain-language, actionable messages instead of raw backend strings — they name the device, suggest a fix (e.g. reconnect the device or try a different sample rate), and add an OS-specific hint to check microphone permission when a start fails for an opaque reason.
- Errors are now shown in a dismissible banner with the full, selectable message and a **Copy** button (plus a link to the issue tracker) so they can be read and reported, rather than truncated in the toolbar status.
- The default target loudness is now −20 LUFS (was −14). Only affects fresh installs; a previously saved target is still restored.
- While capturing, **Reset** is now the prominent (amber) primary control — the action you take between patch changes — and **Stop** is a quiet secondary button; Reset is hidden when idle. A one-time hint on first capture explains the Reset-between-patches workflow.
- The UI now bundles its fonts (Inter for the interface, JetBrains Mono for the numeric readouts and spectrum labels) so typography is identical on macOS, Windows, and Linux instead of falling back to each OS's default fonts.

### Fixed

- The Target and Ceiling steppers are now custom, theme-styled up/down controls. The native number-input arrows rendered nearly invisibly on the dark theme (and differently on each platform); the replacements are legible and consistent everywhere.

## [0.1.0] - 2026-06-18

### Added

- Initial release: ITU-R BS.1770 / EBU R128 loudness metering (integrated, short-term, momentary, LRA), true-peak with peak-hold, a log-frequency spectrum analyzer, and a target/apply gain helper.
- Persist configuration between sessions: window size/position/monitor (via `tauri-plugin-window-state`), plus the selected audio device, channels, sample rate, target LUFS, and clip ceiling (via `tauri-plugin-store`). Restored selections are re-validated against the current device — a missing device falls back to the system default with a notice, and out-of-range channels or sample rates fall back gracefully.
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
