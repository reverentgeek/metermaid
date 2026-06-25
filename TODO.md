# TODO / Ideas

Future tasks and ideas for the MeterMaid app. Roughly ordered by value.

## Features

### "Main output" preset

- [ ] One-click preset that restores a saved device + channel pair (subset of the persistence work above; could ship as soon as device/channel persistence lands).

### Spectrum peak-hold & reference curves ([#9](https://github.com/reverentgeek/metermaid/issues/9))

- [ ] **Persistent max peak-hold** — a per-band maximum that holds (no decay) until cleared, drawn as a distinct line over the live bars. Give it its **own** toggle + clear control rather than overloading the existing Reset, which resets integrated loudness and the decaying peak-hold together.
- [ ] **Calibrated pink/brown noise reference curves** — optional background guide curves for EQ/tone-shaping (a visual target, not a precise measurement). Don't draw idealized −3 dB/oct (pink) / −6 dB/oct (brown) lines: MeterMaid shows the peak FFT-bin magnitude per log band, not a PSD, so a theoretical slope won't match what real noise displays as. Instead calibrate to the analyzer — generate the noise, run it through the same 96-band `spectrum()` pipeline, and store the resulting per-band response as the reference shape. Precompute for the common 20 Hz–20 kHz case (44.1/48 kHz); handle lower sample rates where nyquist < 20 kHz shifts the top band edge. Needs a vertical anchor/offset (e.g. pin at 1 kHz) so the shape is comparable regardless of level.

Implementation note: a "freeze current spectrum as a background reference" feature would complement these — it reuses the max-hold machinery and lets the user freeze pink/brown noise *or* a reference tone they're matching. See the [#9](https://github.com/reverentgeek/metermaid/issues/9) discussion.

## Distribution

- [x] **Code signing + notarization (macOS)** — Apple Developer account ($99/yr), Developer ID Application cert. Env vars are already documented in the README.
- [x] Add hardened-runtime entitlements file (`com.apple.security.device.audio-input`).
- [ ] Decide on **Windows / Linux** support (changes the signing budget and the webview-audio consistency story — see notes from initial design discussion).
- [ ] CI to produce signed builds reproducibly for contributors.
- [x] **Auto-updater** — `tauri-plugin-updater` ships in 0.3.0: silent launch + manual ("Check for updates") checks, an Install & Restart banner with release notes, and minisign-verified downloads. `release.yml` signs the update artifacts and publishes `latest.json`. Requires the `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repo secrets.
- [ ] **Windows code signing** — builds are currently unsigned, so SmartScreen warns on first run. Configure `bundle.windows.signCommand` (Azure Trusted Signing or a `.pfx`) plus the matching secrets.
- [ ] **Automate the release download table** — generate the per-OS asset-link table in `release.yml` from the uploaded artifacts instead of hand-building the release notes each time (error-prone — asset names must match exactly).

## Possible enhancements

- [ ] Numeric **dB grid / target line overlay** on the spectrum.
- [ ] **Loudness history** graph (integrated/short-term over time).
- [ ] Selectable **loudness standard presets** (streaming −14, broadcast −23 EBU R128, etc.).
- [ ] **A/B compare** two captures for matching patch levels.
- [ ] Spectrum options: linear/log toggle, adjustable averaging/smoothing, peak vs RMS.
- [ ] Validate readings against `ffmpeg -af ebur128` in an automated test.
- [ ] **Spectrum hover readout** — show frequency + dB at the cursor (pairs with the dB grid / target-line overlay above; handy for EQ and the reference-curve work).
- [x] **Keyboard shortcut for Reset** (Space) — the leveling loop hits Reset constantly between patches, so a shortcut tightens the core workflow. Gated on capture being active and focus not in an input/select/button.

## Tooling

- [ ] **Frontend UI** - investigate the usefulness of migrating the UI to React, Vue, Solidjs, or some other UI framework. Same for CSS, like TailwindCSS.
- [x] **Project cSpell dictionary** — committed `cspell.json` at the repo root with the shared domain word list (`nyquist`, `ebur`, `LUFS`, `clippy`, `serde`, `SPSC`, `rustfft`, `hotplug`, `cpal`, `ringbuf`, `minisign`, …), so the editor stops flagging domain terms on every Markdown/source edit. (Editor-agnostic; the per-machine `.vscode/settings.json` is gitignored.)
