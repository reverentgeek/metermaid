# ASIO support on Windows — implementation plan

## Problem

On Windows, multichannel audio interfaces (e.g. the Line 6 Helix Stadium XL) expose only a single channel to MeterMaid: the channel picker lists just "Ch 1 (mono)". On macOS the same device correctly offers Ch 1–2 (stereo) through Ch 8.

### Root cause

The channel dropdown is built entirely from `default.channels()` — cpal's `default_input_config()` for the device (`src-tauri/src/audio.rs` `device_config`, around line 470) feeding `populateChannels` in `src/main.ts` (around line 410).

- **macOS / CoreAudio** reports the device's native channel count (8 for the Helix).
- **Windows / WASAPI** (cpal 0.15's default Windows host) opens devices in **shared mode** only. The format is locked to the endpoint's "Default Format" in the Windows Sound control panel, which reports the Helix capture endpoint as 1 channel. `supported_input_configs()` only returns the shared-mode format, so there is no 8-channel option to surface.

This is a WASAPI-shared-mode limitation, not a MeterMaid bug. The standard fix for multichannel capture on Windows pro-audio devices is **ASIO**, which Line 6 ships a driver for and which exposes all device channels natively.

## Licensing context (changed Nov 2025 — this is now viable)

Steinberg relicensed the ASIO SDK:

- **ASIO SDK → GPLv3** (dual-licensed: GPLv3 *or* the existing Steinberg commercial license). This replaced the old proprietary-only, no-redistribution license that previously made CI builds and SDK vendoring impractical.
- (For reference, the VST3 SDK separately went to MIT — not relevant to ASIO.)
- A maintained copy lives at <https://github.com/audiosdk/asio>.
- "ASIO" is a Steinberg trademark; use of the name/logo is optional under the GPLv3 license but, if used, must follow Steinberg's trademark rules.

### Implication for MeterMaid (currently MIT)

GPLv3 is copyleft and one-directional with MIT: MIT code can be combined into a GPLv3 work, but the **distributed combined binary** then falls under GPLv3.

- Only the **Windows-x64 release binary** links ASIO, so **only that artifact** becomes GPLv3. The MeterMaid *source* stays MIT. macOS, Linux, and Windows-ARM64 builds (no ASIO) remain pure MIT.
- Other bundled components are already GPLv3-compatible: Tauri and cpal are Apache-2.0/MIT; bundled fonts are OFL.
- A non-copyleft Windows binary would require Steinberg's commercial ASIO license, but since MeterMaid is already open source the free GPLv3 route is the natural fit.

**Decision gate:** confirm we are OK shipping the Windows-x64 build under GPLv3 (source stays MIT; that one binary carries a GPLv3 notice). Verify the exact `LICENSE` text and SDK version from the SDK source before relying on vendoring.

## Scope of changes

### What we get for free

Once a device is opened through the ASIO host, cpal's `default_input_config()` reports the driver's native channel count (all 8 on the Helix), so the existing `populateChannels` dropdown works unchanged. **No frontend channel-picker logic changes are needed** — the work is in the build system and host plumbing.

### What is unchanged / not possible

- **Windows ARM64 cannot have ASIO** — there is no ARM64 ASIO SDK. The `aarch64-pc-windows-msvc` release leg stays WASAPI-only.
- macOS and Linux are untouched (CoreAudio / ALSA via the default host).

## Phase 1 — Local spike (validate the premise cheaply)

Goal: prove on the actual Windows 10 box that the Helix enumerates 8 channels through the ASIO host before investing in CI/plumbing.

1. Install build prerequisites on the Windows machine:
   - LLVM (`choco install llvm`) — `asio-sys` uses bindgen and needs `libclang`; set `LIBCLANG_PATH`.
   - The MSVC C++ toolchain (Visual Studio Build Tools) — the SDK's C++ sources are compiled, so the build must run inside the VS dev environment (`vcvars`).
2. Obtain the ASIO SDK (clone <https://github.com/audiosdk/asio> or download the release) and set `CPAL_ASIO_DIR` to the extracted SDK directory.
3. Add a target-scoped cpal feature so ASIO only compiles where it can (Cargo unifies the feature for the x64-Windows target):

   ```toml
   # src-tauri/Cargo.toml — base dependency, all platforms
   cpal = "0.15"

   # x64 Windows only
   [target.'cfg(all(windows, target_arch = "x86_64"))'.dependencies]
   cpal = { version = "0.15", features = ["asio"] }
   ```

4. Add a throwaway debug path (or a temporary test) that opens `cpal::host_from_id(HostId::Asio)`, enumerates input devices, and prints each device's `default_input_config()` channel count. Confirm the Helix reports 8.

**Exit criteria:** `cargo build` is green on Windows x64 with the `asio` feature, and the Helix enumerates 8 channels through the ASIO host. If this fails, stop — the rest of the plan depends on it.

## Phase 2 — Multi-host backend (`src-tauri/src/audio.rs`)

cpal's ASIO support does **not** change `default_host()`; WASAPI stays the default and ASIO is a second host opened explicitly. All ASIO references must be gated `#[cfg(all(windows, target_arch = "x86_64"))]` so non-ASIO builds still compile.

1. **Host abstraction.** Introduce a small notion of "which host a device belongs to." Add an ASIO host accessor (`cpal::host_from_id(HostId::Asio)`) behind the cfg gate; on all other builds it is absent.
2. **Merged enumeration.** Update `list_input_devices` (around line 449) to enumerate the default host *and* (when present) the ASIO host, merging results.
3. **Device disambiguation.** The same Helix appears under both WASAPI and ASIO. Device identity is currently a bare name string (also used as the `settings.json` persistence key). Tag devices with their host so they are distinguishable and the right one is reopened:
   - Extend `DeviceInfo` with a host tag, and present a disambiguated label (e.g. `Helix Stadium XL (ASIO)`).
   - Thread the host tag through `find_device` (around line 429) so lookup targets the correct host instead of always `default_host()`.
   - Update the matching TS `DeviceInfo` interface in `src/main.ts` and the saved-settings re-validation so a persisted ASIO selection is restored to the ASIO device (and falls back gracefully if the driver/device is absent).
4. **Keep `device_config` / `build_stream` host-aware.** Both currently resolve the device via `find_device`; once that is host-aware they work unchanged, and `device_config` will naturally report the full ASIO channel count.

### Runtime behavior / UX notes

- **ASIO is exclusive-access.** If the Helix is open in a DAW, MeterMaid's ASIO open fails (and vice versa). Add a clear "driver in use" error message distinct from the generic build error.
- **Driver-controlled rate/buffer.** ASIO sample rate and buffer size are set in the device's own control panel, not by MeterMaid; the sample-rate picker becomes advisory for ASIO devices. Decide whether to hide/annotate the rate picker when an ASIO device is selected.

## Phase 3 — CI / release workflow

1. **Release (`.github/workflows/release.yml`).** On the `x86_64-pc-windows-msvc` matrix leg only, add steps to: install LLVM, fetch/extract the ASIO SDK (now redistributable under GPLv3 — vendor it or download a pinned release), export `CPAL_ASIO_DIR` and `LIBCLANG_PATH`, and ensure the build runs in the VS dev environment. The `aarch64-pc-windows-msvc` leg is left WASAPI-only (no `asio` feature).
2. **CI (`.github/workflows/ci.yml`).** The matrix is currently ubuntu + macos only (no Windows), so the ASIO build path would go untested. Add a Windows-x64 leg that performs the same LLVM + SDK setup and at least `cargo build`s with the `asio` feature, so the ASIO code path does not silently rot. (Weigh CI minutes vs. coverage — at minimum a build check, not necessarily full bundle.)
3. **Licensing artifacts.** Add the GPLv3 license text and notice scoped to the Windows-x64 binary; document in `README.md` that the Windows-x64 build is distributed under GPLv3 while the project source remains MIT. Respect the ASIO trademark rules if the name/logo is used in UI or docs.

## Phase 4 — Docs & release

- Update `CLAUDE.md` (architecture + release sections) to describe the second host, the cfg-gated `asio` feature, the Windows-x64 build prerequisites, and the per-artifact GPLv3 licensing.
- Update `README.md` platform/build and licensing sections.
- Add a `CHANGELOG.md` entry.

## Effort & risk

- **Dominant cost:** CI/build iteration. The LLVM + ASIO SDK + `vcvars` chain can realistically only be debugged on a Windows runner, so expect slow trial-and-error. The Rust multi-host changes are comparatively small (~1 day).
- **Rough estimate:** a focused multi-day effort, front-loaded by the Phase 1 spike.
- **Primary risks:** (1) the bindgen/libclang/MSVC build chain in CI; (2) device-identity/settings migration across two hosts; (3) ASIO exclusive-access surprising users. None are blockers given a successful Phase 1.

## Decision gates (resolve before Phase 2)

1. Phase 1 spike confirms the Helix enumerates 8 channels via the ASIO host.
2. We accept shipping the Windows-x64 binary under GPLv3 (source stays MIT), confirmed against the SDK's actual `LICENSE`.

## References

- Steinberg press — open-source ASIO / OBS partnership: <https://www.steinberg.net/press/2025/obs-collaboration/>
- ASIO now GPLv3 (VST3 → MIT): <https://www.kvraudio.com/news/steinberg-moves-vst-3-sdk-to-mit-open-source-license-asio-now-gplv3-65179>
- ASIO SDK source copy: <https://github.com/audiosdk/asio>
- cpal ASIO backend (build requirements: `CPAL_ASIO_DIR`, libclang/bindgen, MSVC).
