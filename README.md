# MeterMaid

A cross-platform desktop **LUFS / loudness meter** built with [Tauri](https://tauri.app)
(Rust audio engine + web UI). It measures loudness to the **ITU-R BS.1770 / EBU R128**
standard and shows a real-time frequency spectrum — handy for normalizing the levels of
guitar amp/effects patches (e.g. Line 6 Helix) or any audio source.

## Readouts

- **Integrated** loudness (gated), LUFS — the overall "how loud is this patch" number
- **Short-term** (3 s) and **Momentary** (400 ms) loudness, LUFS
- **Loudness Range (LRA)**, LU
- **True Peak**, dBTP
- **Spectrum analyzer** — log-frequency, peak-hold, with dB and frequency grid
- **Target / apply** helper — set a target LUFS and it shows the gain to apply

The loudness math uses the [`ebur128`](https://crates.io/crates/ebur128) crate (a pure-Rust
implementation of BS.1770, the same algorithm behind ffmpeg's `ebur128` filter). Audio is
captured with [`cpal`](https://crates.io/crates/cpal) and the spectrum uses
[`rustfft`](https://crates.io/crates/rustfft).

## Audio source

The app meters any **input device** the OS exposes. To meter a hardware unit's USB output
(like a Helix), connect it over USB and select it in the device dropdown — its output
arrives at the computer as an input. To meter software/playback, route it through a virtual
device such as [BlackHole](https://github.com/ExistentialAudio/BlackHole) and select that.

## Develop

```sh
pnpm install
pnpm tauri dev
```

On first launch macOS will prompt for **microphone access** (required to read any audio
input device). The usage string lives in `src-tauri/Info.plist`.

## Build

```sh
pnpm tauri build
```

The bundle is written to `src-tauri/target/release/bundle/`.

## Code signing & notarization (macOS)

To distribute without Gatekeeper warnings you need an Apple Developer account ($99/yr) and a
"Developer ID Application" certificate. Tauri reads these from environment variables at build
time:

```sh
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID="you@example.com"
export APPLE_PASSWORD="app-specific-password"   # appleid.apple.com app-specific password
export APPLE_TEAM_ID="TEAMID"
pnpm tauri build
```

With those set, `tauri build` signs the app, submits it to Apple's notary service, and
staples the ticket. See the Tauri macOS signing docs for the hardened-runtime entitlements
(this app needs `com.apple.security.device.audio-input`).

## Platform support

MeterMaid is built on Tauri 2 and is developed primarily on **macOS**. Windows and Linux
should work but are less exercised.

On **Linux**, the Tauri stack still depends on GTK3 / WebKitGTK. `cargo audit` therefore
reports several "unmaintained" advisories for transitive GTK3 (`gtk`, `gdk`, `atk`, …) and
`glib` crates, plus `unic-*` crates pulled in via Tauri's URL handling. These are not
exploitable vulnerabilities for this app and are tracked upstream by the Tauri project; they
are acknowledged in [`src-tauri/.cargo/audit.toml`](src-tauri/.cargo/audit.toml) and will
clear as Tauri migrates off GTK3.

## License

Released under the [MIT License](LICENSE). © 2026 David Neal.
