# Changelog

All notable changes to MeterMaid are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Surface OS audio stream faults (e.g. the device being unplugged mid-capture)
  in the UI instead of silently freezing the meter.

### Changed
- True-peak and spectrum peak-hold ballistics now fall at a fixed dB/second rate,
  independent of display refresh rate and the engine's emit cadence.
- The realtime audio callback no longer logs on ring overrun; it tallies dropped
  samples lock-free and the engine thread reports them, keeping the callback
  allocation- and lock-free even under overrun.
- Tightened the webview Content-Security-Policy (`style-src` no longer allows
  `'unsafe-inline'`).
- CI now also builds and tests on macOS and runs `cargo audit`.

### Fixed
- Reject invalid channel selections (more than two channels, or a stereo pair
  pointing at the same channel) in the audio engine.

## [0.1.0]

- Initial release: ITU-R BS.1770 / EBU R128 loudness metering (integrated,
  short-term, momentary, LRA), true-peak with peak-hold, a log-frequency
  spectrum analyzer, and a target/apply gain helper.
