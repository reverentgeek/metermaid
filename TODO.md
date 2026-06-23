# TODO / Ideas

Future tasks and ideas for the MeterMaid app. Roughly ordered by value.

## Features

### Persist configuration between sessions

Remember and restore app state on launch so the user doesn't reconfigure every time.

- [x] Persist **window size, position, and monitor** across launches.
  - On startup, if the previously used monitor is **no longer present** (e.g. an external display was unplugged), move the window to the next available monitor instead of restoring it off-screen.
  - Also clamp the restored position so the window is always at least partially on-screen (guards against resolution changes / display rearrangement).
- [x] Persist **selected audio device** and re-select it on launch.
  - If that device is gone, fall back to the system default and surface a notice.
- [x] Persist **selected channels** (e.g. `Ch 1–2`) and **sample rate**.
  - Re-validate against the restored device's capabilities; fall back gracefully if the channel index / rate is no longer valid.
- [x] Persist **target LUFS** and **clip ceiling (dBTP)** values.
- [x] (Optional) Auto-start capture on launch if a valid device + channels restore.
- [x] Add more frequency labels below the spectral graph so that users have a better idea of which ones are spiking

Implementation notes:

- The [`tauri-plugin-window-state`](https://github.com/tauri-apps/plugins-workspace/tree/v2/plugins/window-state) plugin handles window size/position/monitor restore out of the box — evaluate it before hand-rolling. Confirm its multi-monitor / missing-monitor behavior matches the requirement above; add the fallback logic if not.
- For app settings (device, channels, rate, target, ceiling), use [`tauri-plugin-store`](https://github.com/tauri-apps/plugins-workspace/tree/v2/plugins/store) (JSON key/value store) rather than inventing a config format.

### "Main output" preset

- [ ] One-click preset that restores a saved device + channel pair (subset of the persistence work above; could ship as soon as device/channel persistence lands).

## Distribution

- [ ] **Code signing + notarization (macOS)** — Apple Developer account ($99/yr), Developer ID Application cert. Env vars are already documented in the README.
- [ ] Add hardened-runtime entitlements file (`com.apple.security.device.audio-input`).
- [ ] Decide on **Windows / Linux** support (changes the signing budget and the webview-audio consistency story — see notes from initial design discussion).
- [ ] CI to produce signed builds reproducibly for contributors.

## Possible enhancements

- [ ] Numeric **dB grid / target line overlay** on the spectrum.
- [ ] **Loudness history** graph (integrated/short-term over time).
- [ ] Selectable **loudness standard presets** (streaming −14, broadcast −23 EBU R128, etc.).
- [ ] **A/B compare** two captures for matching patch levels.
- [ ] Spectrum options: linear/log toggle, adjustable averaging/smoothing, peak vs RMS.
- [ ] Validate readings against `ffmpeg -af ebur128` in an automated test.
