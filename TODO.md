# TODO / Ideas

Future tasks and ideas for the MeterMaid app. Roughly ordered by value.

## Bugs / correctness

- [x] **Multichannel drain alignment** — `engine_loop` drained the SPSC ring in fixed 8192-sample chunks, but `Analyzer::process` drops a trailing partial frame, so on devices whose channel count doesn't divide 8192 (6-, 10-, 12-, 18-channel interfaces — exactly the multichannel ASIO hardware) every full chunk rotated the channel alignment of everything after it and the meter silently blended the wrong channels. Fixed by sizing the drain buffer per stream to a whole number of frames (`drain_chunk_len`), with a 6-channel golden-signal regression test.
- [ ] **Ring-overrun frame alignment** — on ring overrun the realtime callback's `push_slice` can push a partial frame, permanently rotating channel alignment even after the overrun clears. Round the push count down to a frame boundary (`n -= n % stride`, still lock- and allocation-free) and tally the remainder as dropped. Rare (the ring holds ~1 s, drained every 33 ms) but the failure mode is "wrong channels until Stop".
- [ ] **Make blocking commands `async`** — in Tauri 2, non-`async` commands run on the main thread. `start_capture` blocks on the engine's reply and `list_devices` with `includeAsio: true` loads ASIO drivers (can take seconds), freezing the webview meanwhile. Declaring them `async fn` moves them to the async thread pool with no other changes.
- [ ] **Stale spectrum peak-hold across capture sessions** — `start()` resets `displayedPeak`, `lastPeakTs`, and `clipLatched` but not `peaks`, so after Stop → Start the previous session's spectrum peak-hold line reappears over the new capture and decays from there. Clear `peaks` in `start()`.
- [ ] **Clip light can re-latch right after Reset** (minor) — `resetMeasurement` clears `clipLatched` and fire-and-forgets `reset_integrated`, but a `meter-update` already in flight can still carry the old `truePeakMaxDb` and re-latch for one frame cycle. Ignore latching until the reported max first drops back below the ceiling, or add an engine-side reset generation counter.

## Features

### "Main output" preset

- [ ] One-click preset that restores a saved device + channel pair. Device/channel/rate persistence has shipped (`settings.json`), so the groundwork is done — this is now just a save/recall UI over the existing settings keys.

### Spectrum peak-hold & reference curves ([#9](https://github.com/reverentgeek/metermaid/issues/9))

- [ ] **Persistent max peak-hold** — a per-band maximum that holds (no decay) until cleared, drawn as a distinct line over the live bars. Give it its **own** toggle + clear control rather than overloading the existing Reset, which resets integrated loudness and the decaying peak-hold together.
- [ ] **Calibrated pink/brown noise reference curves** — optional background guide curves for EQ/tone-shaping (a visual target, not a precise measurement). Don't draw idealized −3 dB/oct (pink) / −6 dB/oct (brown) lines: MeterMaid shows the peak FFT-bin magnitude per log band, not a PSD, so a theoretical slope won't match what real noise displays as. Instead calibrate to the analyzer — generate the noise, run it through the same 96-band `spectrum()` pipeline, and store the resulting per-band response as the reference shape. Precompute for the common 20 Hz–20 kHz case (44.1/48 kHz); handle lower sample rates where nyquist < 20 kHz shifts the top band edge. Needs a vertical anchor/offset (e.g. pin at 1 kHz) so the shape is comparable regardless of level.

Implementation note: a "freeze current spectrum as a background reference" feature would complement these — it reuses the max-hold machinery and lets the user freeze pink/brown noise *or* a reference tone they're matching. See the [#9](https://github.com/reverentgeek/metermaid/issues/9) discussion.

Work-package note: **persistent max peak-hold**, the **ceiling reference line**, and the **spectrum hover readout** (below) all touch `drawSpectrum`/`toY` and the canvas interaction layer — each is ~30–60 lines, and doing them together is much cheaper than doing them months apart. Together they also give the reference-curve work a richer canvas foundation to land on.

## Distribution

- [x] **Code signing + notarization (macOS)** — Apple Developer account ($99/yr), Developer ID Application cert. Env vars are already documented in the README.
- [x] Add hardened-runtime entitlements file (`com.apple.security.device.audio-input`).
- [x] Decide on **Windows / Linux** support — decided and shipped: the release matrix builds macOS/Windows/Linux (x64 + arm64), with ASIO multichannel capture on Windows x64. What remains is the Windows-signing item below.
- [ ] CI to produce signed builds reproducibly for contributors.
- [x] **Auto-updater** — `tauri-plugin-updater` ships in 0.3.0: silent launch + manual ("Check for updates") checks, an Install & Restart banner with release notes, and minisign-verified downloads. `release.yml` signs the update artifacts and publishes `latest.json`. Requires the `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repo secrets.
- [ ] **Windows code signing** — builds are currently unsigned, so SmartScreen warns on first run. Configure `bundle.windows.signCommand` (Azure Trusted Signing or a `.pfx`) plus the matching secrets.
- [ ] **Automate the release download table** — generate the per-OS asset-link table in `release.yml` from the uploaded artifacts instead of hand-building the release notes each time (error-prone — asset names must match exactly). The website already maps asset names to per-OS links in `site/src/_data/release.js` (`ASSET_MAP`); extract or reuse that logic so both consumers share one mapping and the "names must match exactly" failure mode dies in both places at once.

## Possible enhancements

- [ ] **Spectrum reference lines.** Note: the spectrum already draws a numeric dB grid (horizontal lines + labels every 20 dB) and a frequency grid in `drawSpectrum` — so the "dB grid" is largely done. Two distinct follow-ups remain, which the original "dB grid / target line overlay" item conflated:
  - **Ceiling reference line** (easy, dimensionally correct) — draw a horizontal line at the −1 dBTP ceiling (and optionally 0 dBFS) over the bars, tinting any band that crosses it. The spectrum Y-axis is per-band FFT peak magnitude (dBFS), which is the same family as the ceiling, so this is meaningful. Reuses the existing `toY(db)` helper (~20–30 lines).
  - **dB grid polish** (trivial) — optionally finer ticks (every 10 dB) and/or a toggle.
  - **Do NOT draw the LUFS target on the spectrum.** The Target control is integrated LUFS (BS.1770-weighted, time-averaged) — a different quantity from per-band FFT magnitude, so a horizontal target line on the spectrum would be dimensionally wrong and misleading. The LUFS target line belongs on the loudness history graph below, where the Y-axis *is* LUFS.
- [ ] **Loudness history** graph (integrated/short-term over time). Natural home for a **−20 LUFS target reference line** (and standard-preset lines), since its Y-axis is loudness — unlike the spectrum.
- [ ] **Overrun indicator in the UI** — ring overruns are currently only `eprintln!`'d on the engine thread; surface the dropped-sample tally as a `Metrics` field plus a small warning glyph so the user knows a reading may be suspect. Pro-audio users expect this.
- [ ] Selectable **loudness standard presets** (streaming −14, broadcast −23 EBU R128, etc.).
- [ ] **A/B compare** two captures for matching patch levels.
- [ ] Spectrum options: linear/log toggle, adjustable averaging/smoothing, peak vs RMS.
- [ ] **Run the ffmpeg cross-check in CI** — the `ebur128_matches_ffmpeg` golden test already exists (`#[ignore]`d in `audio.rs`); the remaining work is a ~10-line CI change: install ffmpeg on the macOS/Linux app-matrix legs and run `cargo test ebur128_matches_ffmpeg -- --ignored`.
- [ ] **Spectrum hover readout** — show frequency + dB at the cursor (pairs with the spectrum reference lines above; handy for EQ and the reference-curve work). Part of the canvas work package noted under Features.
- [x] **Keyboard shortcut for Reset** (Space) — the leveling loop hits Reset constantly between patches, so a shortcut tightens the core workflow. Gated on capture being active and focus not in an input/select/button.

## Tooling

- [ ] **Generated TS bindings (tauri-specta)** — the Rust `Metrics`/`DeviceInfo`/`DeviceConfig`/`StreamInfo` structs and the hand-written TS interfaces in `main.ts` are kept in sync by convention only. [tauri-specta](https://github.com/specta-rs/tauri-specta) (or plain `ts-rs`) would generate the TS types and command bindings from the Rust definitions, eliminating that class of drift bugs for a one-time setup cost.
- [ ] **Split `main.ts` into plain ES modules** — it's at ~1,000 lines and the queued canvas features will grow it. Extracting `updater.ts`, `devices.ts`, `spectrum.ts` costs nothing at runtime and keeps the no-framework decision intact.
- [ ] **Small cleanups** — batch `persist()` into a single `store.set` of one settings object instead of six sequential awaited writes; extend the Space-shortcut focus guard to `TEXTAREA`/contenteditable if such elements ever appear.
- [x] **Frontend UI framework** — investigated; not worth it. Hot path is canvas (a framework doesn't help raw 2D drawing), the DOM-sync surface is small and stable, and a runtime dep fights the lean-bundle / low-idle-CPU goals. Revisit *only* if interactive stateful UI grows substantially — and then reach for signals (Preact/Solid), not React/Vue. Skip Tailwind: hand-written CSS is fine for a finished single-window design.
- [x] **Project cSpell dictionary** — committed `cspell.json` at the repo root with the shared domain word list (`nyquist`, `ebur`, `LUFS`, `clippy`, `serde`, `SPSC`, `rustfft`, `hotplug`, `cpal`, `ringbuf`, `minisign`, …), so the editor stops flagging domain terms on every Markdown/source edit. (Editor-agnostic; the per-machine `.vscode/settings.json` is gitignored.)
