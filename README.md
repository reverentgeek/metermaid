# MeterMaid

A cross-platform desktop **LUFS / loudness meter** built with [Tauri](https://tauri.app) (Rust audio engine + web UI). It measures loudness according to the **ITU-R BS.1770 / EBU R128** standard and displays a real-time frequency spectrum, handy for normalizing the levels of guitar amp/effects patches (e.g., Line 6 Helix Stadium XL, Neural DSP Quad Cortex, Fractal Axe-FX) or any audio source.

![MeterMaid metering a stereo input: loudness readouts, true-peak, target/apply helper, and a log-frequency spectrum analyzer](docs/screenshot.png)

> **Why level patches?** A louder patch almost always *sounds* better, even when it isn't, so honest tone comparisons need matched loudness. The companion blog post, [Level Guitar Patches with MeterMaid](https://reverentgeek.com/leveling-guitar-patches-with-tauri/), explains the problem and walks through a full leveling session with the app.

## Download

Grab the installer for your platform below, or see [all releases](https://github.com/reverentgeek/metermaid/releases/latest) for every format and the latest version.

| Platform | Installer | Other formats |
| --- | --- | --- |
| **macOS** (Apple Silicon) | [`.dmg`](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid_0.1.0_aarch64.dmg) | — |
| **macOS** (Intel) | [`.dmg`](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid_0.1.0_x64.dmg) | — |
| **Windows** (x64) | [`.exe` installer](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid_0.1.0_x64-setup.exe) | [`.msi`](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid_0.1.0_x64_en-US.msi) |
| **Windows** (ARM64) | [`.exe` installer](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid_0.1.0_arm64-setup.exe) | [`.msi`](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid_0.1.0_arm64_en-US.msi) |
| **Linux** (x64) | [`.AppImage`](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid_0.1.0_amd64.AppImage) | [`.deb`](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid_0.1.0_amd64.deb) · [`.rpm`](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid-0.1.0-1.x86_64.rpm) |
| **Linux** (ARM64) | [`.AppImage`](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid_0.1.0_aarch64.AppImage) | [`.deb`](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid_0.1.0_arm64.deb) · [`.rpm`](https://github.com/reverentgeek/metermaid/releases/download/v0.1.0/MeterMaid-0.1.0-1.aarch64.rpm) |

macOS builds are signed with an Apple Developer ID and **notarized** by Apple, so they open without Gatekeeper warnings. Windows builds are currently **unsigned**, and SmartScreen may warn on first run (choose *More info → Run anyway*). On first launch, MeterMaid asks for **microphone access**, which it needs to read audio from any input device.

## Readouts

- **Integrated** loudness (gated), LUFS — the overall "how loud is this patch" number
- **Short-term** (3 s) and **Momentary** (400 ms) loudness, LUFS
- **Loudness Range (LRA)**, LU
- **True Peak**, dBTP
- **Spectrum analyzer** — log-frequency, peak-hold, with dB and frequency grid
- **Target / apply** helper — set a target LUFS, and it shows the gain to apply

The loudness math uses the [`ebur128`](https://crates.io/crates/ebur128) crate (a pure-Rust implementation of BS.1770, the same algorithm behind ffmpeg's `ebur128` filter). Audio is captured with [`cpal`](https://crates.io/crates/cpal), and the spectrum is computed using [`rustfft`](https://crates.io/crates/rustfft).

## Using the meter

**Integrated** loudness, and therefore the **Apply** value, is a *long-term average* of the entire take since you started (or last reset) the measurement, per the EBU R128 standard. It describes the overall loudness of a whole performance rather than the moment-to-moment level.

Because it averages everything heard so far, **Apply** won't jump when you change your volume mid-measurement. The earlier (louder or quieter) signal is still part of the average. The live **Short-term** and **Momentary** readouts react instantly, but the Integrated and Apply deliberately lag.

So to dial in a level:

1. Make your change (e.g., adjust the patch or guitar volume).
2. Click **Reset**. Reset restarts the integrated measurement while keeping the audio device running.
3. Play for a few representative seconds, and **Apply** will settle on the new suggested gain.

## Audio source

The app meters any **input device** the OS exposes. To meter a hardware unit’s USB output (like a Helix), connect it over USB and select it in the device dropdown. Its output serves as input to the computer. To meter software/playback, route it through a virtual device, such as [BlackHole](https://github.com/ExistentialAudio/BlackHole), and select it.

## Settings

MeterMaid remembers your setup between launches: the window size, position, and monitor, as well as the selected device, channels, sample rate, target LUFS, and clip ceiling. Restored selections are re-validated against the hardware actually present. If the saved device is gone, it falls back to the system default with a notice, and invalid channels or sample rates fall back gracefully. If the monitor the window was last on has been disconnected, the window is recentered on an available display rather than restored off-screen.

Enable **Auto-start** to begin capturing on launch whenever a valid saved device and channels are restored.

## Develop

```sh
pnpm install
pnpm tauri dev
```

On first launch, macOS will prompt for **microphone access** (required to read any audio input device). The usage string lives in `src-tauri/Info.plist`.

## Build

```sh
pnpm tauri build
```

The bundle is written to `src-tauri/target/release/bundle/`.

## Code signing & notarization (macOS)

To distribute without Gatekeeper warnings, you need an Apple Developer account ($99/yr) and a "Developer ID Application" certificate. Tauri reads these from environment variables at build time:

```sh
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID="you@example.com"
export APPLE_PASSWORD="app-specific-password"   # appleid.apple.com app-specific password
export APPLE_TEAM_ID="TEAMID"
pnpm tauri build
```

With those set, `tauri build` signs the app, submits it to Apple's notary service, and staples the ticket. Notarization enforces the hardened runtime, under which microphone access requires the `com.apple.security.device.audio-input` entitlement — declared in [`src-tauri/Entitlements.plist`](src-tauri/Entitlements.plist) and wired in via `bundle.macOS.entitlements`. Without it, a signed build launches but is silently denied audio input.

Release builds produced by [`.github/workflows/release.yml`](.github/workflows/release.yml) sign and notarize automatically when the matching `APPLE_*` repository secrets are present (see [Building releases](#building-releases)).

## Building releases

Distributable installers for all platforms are built in CI by [`.github/workflows/release.yml`](.github/workflows/release.yml), which runs a matrix across **macOS, Windows, and Linux** for both **x64 and arm64** using [`tauri-action`](https://github.com/tauri-apps/tauri-action). Each job emits every bundle type for its OS:

| Platform | Arches | Installers |
| --- | --- | --- |
| macOS | x64, arm64 | `.dmg`, `.app` |
| Windows | x64, arm64 | `.msi`, `.exe` |
| Linux | x64, arm64 | `.deb`, `.rpm`, `.AppImage` |

To cut a release, push a version tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The workflow builds every target and uploads the artifacts to a **draft** GitHub Release named after the tag. Review the draft and publish it manually once all jobs have finished. (You can also trigger a build from the **Actions** tab via *workflow_dispatch* to smoke-test without tagging.)

### Signing secrets

Builds run **unsigned** by default — usable for testing, but they trip Gatekeeper (macOS) and SmartScreen (Windows) on other machines. To produce signed builds, add these repository secrets (Settings → Secrets and variables → Actions); the workflow wires them in automatically when present:

- **macOS** — `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`
- **Windows** — `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD`
- **Updater (optional)** — `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

## Platform support

MeterMaid is built on Tauri 2 and is developed primarily on **macOS**. Windows and Linux should work, but are less exercised.

On **Windows**, the Rust MSVC toolchain needs the Visual C++ linker (`link.exe`). Installing the Visual Studio Build Tools installer is not enough on its own — you must select the C++ components. In the **Visual Studio Installer → Modify → Individual components**, install:

- **MSVC v143 - VS 2022 C++ build tools** for your architecture
- **Windows 11 SDK** (or the Windows 10 SDK)

On **Windows on ARM (ARM64)**, this is the usual cause of `error: linker 'link.exe' not found`: the default x64 component does not include the ARM64 linker. Make sure you have:

- The **aarch64-pc-windows-msvc** Rust toolchain (`rustup default stable-aarch64-pc-windows-msvc`; confirm `rustc -vV` reports `host: aarch64-pc-windows-msvc`)
- The **MSVC v143 - VS 2022 C++ ARM64/ARM64EC build tools** component
- A build run from the **ARM64 Native Tools Command Prompt for VS 2022**, whose environment puts the matching `link.exe` on `PATH`

Mixing an x64 Rust toolchain (running under emulation) with only the ARM64 toolset installed — or vice versa — is the most common reason the linker isn't found; keep the Rust host triple and the installed C++ toolset on the same architecture.

On **Linux**, the Tauri stack still depends on GTK3 / WebKitGTK. `cargo audit` therefore reports several "unmaintained" advisories for transitive GTK3 (`gtk`, `gdk`, `atk`, …) and `glib` crates, plus `unic-*` crates pulled in via Tauri's URL handling. These are not exploitable vulnerabilities for this app and are tracked upstream by the Tauri project; they are acknowledged in [`src-tauri/.cargo/audit.toml`](src-tauri/.cargo/audit.toml) and will clear as Tauri migrates off GTK3.

## License

Released under the [MIT License](LICENSE). © 2026 David Neal.
