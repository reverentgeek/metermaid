# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

MeterMaid is a cross-platform desktop **LUFS / loudness meter** built with **Tauri 2** — a Rust audio engine (`src-tauri/`) plus a vanilla TypeScript + Vite web UI (`src/`). No frontend framework; the UI is a single `index.html` + `src/main.ts` driving a canvas spectrum. Developed primarily on macOS.

## Commands

```sh
pnpm install              # also installs the git pre-commit hook via core.hooksPath
pnpm tauri dev            # run the app (Vite dev server + Rust); HMR applies UI edits live
pnpm tauri build          # production bundle → src-tauri/target/release/bundle/
pnpm build                # frontend type-check + Vite build only (tsc && vite build)
pnpm lint                 # Biome (TS) + markdownlint (Markdown)
pnpm format               # apply Biome fixes/formatting
```

Rust checks run from `src-tauri/`:

```sh
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo test build_error_messages_name_the_device   # single test by name substring
cargo test ebur128_matches_ffmpeg -- --ignored --nocapture   # optional ffmpeg cross-check (needs ffmpeg)
```

The audio tests in `audio.rs` (`#[cfg(test)] mod tests`) drive the `Analyzer` directly with synthesized frames — **no audio device required**. New analysis behavior should come with one of these golden-signal tests.

CI (`.github/workflows/ci.yml`) runs `pnpm build`, `cargo fmt --check`, clippy, and `cargo test` on macOS + Linux (+ Windows for the ASIO path), plus a `cargo audit`. It is **path-aware**: a `changes` job (`dorny/paths-filter`) classifies each push/PR as touching the app (anything outside `site/`) and/or the site (`site/**`). The app matrix + `audit` run only on app changes; a separate `build-site` job (Playwright suite on Node 22) runs only on `site/**` changes. So a website-only PR skips the whole Rust matrix and an app-only PR skips the site build. The required checks (`build-and-test (macos-latest)`, `build-and-test (ubuntu-latest)`, `audit`) are *skipped* — not absent — on site-only PRs, which GitHub counts as passing, so the PR stays mergeable. (Don't switch this to a workflow-level `paths` filter: that leaves required checks pending forever and blocks the merge. `build-site` is **not** itself a required check yet — add its context to the `main` ruleset if site CI should gate site PRs.) The pre-commit hook lints only *staged* `.rs`/`.ts`/`.md`; bypass with `git commit --no-verify`, skip just clippy with `SKIP_CLIPPY=1 git commit`.

## Architecture

**The realtime-safety contract is the central design constraint.** `src-tauri/src/audio.rs` is built around keeping the audio callback allocation- and lock-free:

- A single dedicated thread, `engine_loop`, owns the `cpal::Stream` (cpal streams are `!Send`) and is the **sole owner** of the `Analyzer` — so the analyzer needs no synchronization.
- The realtime cpal callback does no locking/allocation in steady state: it only pushes incoming frames into a lock-free SPSC ring (`ringbuf::HeapRb`) and tallies dropped samples with a relaxed atomic on overrun. **Never add allocation, locking, or logging to this callback.**
- `engine_loop` blocks on `rx.recv_timeout(EMIT_INTERVAL)` (33 ms). On each timeout it drains the ring, de-interleaves the user-selected channel(s), feeds an `ebur128` analyzer (BS.1770 loudness) and a mono ring for the `rustfft` spectrum, then emits a `meter-update` event with a `Metrics` struct. A separate emit thread (bounded `sync_channel(1)`, coalescing) does the Tauri `emit` so a slow UI can never stall the drain.

**Frontend ↔ backend IPC** has two directions:

- **Commands** (`src/main.ts` `invoke(...)` → `lib.rs` `#[tauri::command]`): `list_devices`, `get_device_config`, `start_capture`, `stop_capture`, `reset_integrated`. The capture commands forward a `Command` enum over an `mpsc::Sender` to `engine_loop` and wait on a reply `SyncSender` — i.e. command handlers are thin shims; the engine thread does the work.
- **Events** (`lib.rs`/`audio.rs` `emit` → `src/main.ts` `listen`): `meter-update` (per-frame metrics) and `stream-error` (OS stream fault, e.g. device unplugged mid-capture → UI tears down and surfaces the reason).

Rust `Metrics`/`DeviceConfig`/`StreamInfo` use `#[serde(rename_all = "camelCase")]`; the matching TS `interface`s in `main.ts` must stay in sync (snake_case Rust field ↔ camelCase TS field).

**Frontend rendering** is a `requestAnimationFrame` loop (`frame`/`requestFrame` in `main.ts`) that draws the spectrum canvas. It is **gated on capture state**: it self-sustains only while `running`, and otherwise repaints exactly once per state change (start, stop/teardown, resize, load). Keep that gating intact — an always-on rAF loop redraws a static plot at the display refresh rate and burns several percent of idle CPU/GPU. Any new state that changes the canvas while idle must call `requestFrame()`.

**Device handling.** `list_input_devices` enumerates via cpal, which exposes only a name (no transport/type) — so the picker deliberately shows *all* inputs and cannot reliably filter microphones. cpal has no hotplug callback, so `main.ts` polls `list_devices` every 2 s while idle (`pollDevices`), rebuilding the dropdown only when the device set changes and preserving the selection. `device_config` only offers candidate sample rates from supported-config ranges that match the **default config's channel count and sample format** (what `build_stream` actually uses); offering others would let Start fail with `UnsupportedConfig`.

**Multi-host (ASIO) device identity.** On x64 Windows, `list_input_devices` also enumerates the **ASIO host** (a second cpal host opened explicitly via `host_from_id(HostId::Asio)`) so multichannel interfaces expose all their channels — WASAPI shared mode reports only the endpoint default (often mono). All ASIO code is gated `#[cfg(all(windows, target_arch = "x86_64"))]` (there is no ARM64 ASIO SDK), so every other target compiles unchanged. Device identity is therefore **host-qualified**: `DeviceInfo` carries a stable `id` (ASIO devices prefixed `asio:`, default-host devices keep the bare name — so settings saved before ASIO still resolve) plus a `host` tag for the label. `find_device` routes an id to the right host. Because cpal must **load** an ASIO driver to enumerate it (slow, and disruptive to other ASIO apps), `list_devices` takes an `include_asio` flag: the 2 s idle poll passes `false` (default host only) and only triggers a full ASIO-inclusive refresh when the default-host topology changes. **ASIO capture requires opening an output stream alongside the input:** the Line 6 (and other) drivers won't run an input-only `ASIOCreateBuffers` — no callbacks fire and disposing the stream access-violates — so `build_stream` opens a silent output stream on the same device (both go into one `ASIOCreateBuffers`) and `BuiltStream`/`ActiveStream` keep it alive, played and dropped with the input. The two hardware-dependent ASIO tests in `audio.rs` are `#[ignore]`d and serialized on a mutex (ASIO drivers are single-instance). Building the ASIO path requires `CPAL_ASIO_DIR` (the vendored [`third-party/asio`](third-party/asio) SDK), `LIBCLANG_PATH` (bindgen), and the MSVC C++ toolchain on `PATH` — see README "ASIO multichannel capture".

**Settings persistence** uses `@tauri-apps/plugin-store` (`settings.json`): device, channels, sample rate, target LUFS, clip ceiling — all re-validated against present hardware on load (missing device → system default with a notice). Window geometry persists via `tauri-plugin-window-state`; a small `window_guard_plugin` in `lib.rs` recenters the window if its last monitor is gone.

**Self-update** uses `tauri-plugin-updater` (+ `tauri-plugin-process` for the post-install relaunch). Both are registered in `lib.rs::run`; the updater is gated behind `#[cfg(desktop)]` and declared as a non-mobile target dependency in `Cargo.toml`. `main.ts` calls `check()` on launch (silent) and from the **Check for updates** button (`checkForUpdates(manual)` — `manual` controls whether "up to date"/errors surface). A found update raises the green update banner; **Install & Restart** runs `downloadAndInstall()` (Rust does the HTTP fetch + minisign verify — note this bypasses the webview CSP, so no `connect-src` change is needed) then `relaunch()`. The capability permissions `updater:default` and `process:allow-restart` are in `capabilities/default.json`. The plugin checks `plugins.updater.endpoints` (the GitHub `latest.json`) and only fires once that release is **published** (the workflow drafts releases), which dovetails with the manual publish step.

## macOS entitlement gotcha

Capturing from **any** input device on macOS requires the microphone permission — this is an OS rule, not a MeterMaid choice, and applies even to USB interfaces. Under the notarized hardened runtime, that means the `com.apple.security.device.audio-input` entitlement in `src-tauri/Entitlements.plist` (wired via `bundle.macOS.entitlements`) is mandatory: without it a *signed* build launches but is silently denied audio. The usage string lives in `src-tauri/Info.plist`.

## Release & signing process

Builds are produced by `.github/workflows/release.yml` (matrix: macOS/Windows/Linux × x64/arm64 via `tauri-action`), triggered by pushing a `v*` tag. **macOS builds are signed with a Developer ID and notarized/stapled** (the `APPLE_*` repo secrets are configured); **Windows builds are currently unsigned** (no `signCommand` in `tauri.conf.json`); Linux packages are unsigned by nature.

**ASIO on the Windows-x64 leg.** Only `x86_64-pc-windows-msvc` links ASIO. Its matrix steps set up the MSVC env (`ilammy/msvc-dev-cmd`), point `CPAL_ASIO_DIR` at the vendored [`third-party/asio`](third-party/asio) SDK, and set `LIBCLANG_PATH`; the ARM64 Windows leg has no ASIO SDK and builds WASAPI-only. **Licensing consequence:** the vendored SDK is GPLv3, so the **distributed Windows-x64 installer is GPLv3** ([`LICENSE-GPL-3.0.txt`](LICENSE-GPL-3.0.txt)) while MeterMaid's source and every other binary (macOS, Linux, Windows-ARM64) stay MIT. This is a per-artifact distinction; don't relicense the project. CI (`ci.yml`) build-tests the ASIO path on a `windows-latest` runner so it can't silently rot.

**Updater signing is separate from OS code signing.** The in-app self-updater (`tauri-plugin-updater`, see "Self-update" below) requires its own minisign keypair: the public key lives in `tauri.conf.json` (`plugins.updater.pubkey`) and the private key is the `TAURI_SIGNING_PRIVATE_KEY` repo secret (generated with `pnpm tauri signer generate`, empty password → `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` may be empty). This secret is **mandatory** now that `bundle.createUpdaterArtifacts` is on — without it the release build fails. Rotating the key requires shipping a build with the new pubkey *before* any release signed by it, or existing installs will reject the update.

To cut a release after the version PR is merged to `main`:

1. **Bump the version in all four places** (must stay in lockstep): `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and the `metermaid` entry in `src-tauri/Cargo.lock`. Add a dated `## [x.y.z]` section to `CHANGELOG.md`. (This is normally done in the feature PR, not at tag time.)
2. **Tag and push** an annotated tag matching the existing style (subject `MeterMaid x.y.z`):

   ```sh
   git tag -a v0.2.0 -m "MeterMaid 0.2.0"
   git push origin v0.2.0
   ```

3. The workflow builds every target and uploads installers to a **draft** GitHub Release named `MeterMaid v0.2.0`. Watch it:

   ```sh
   gh run list --workflow=release.yml --limit 3
   gh run watch <run-id> --exit-status
   ```

4. **Verify before publishing** — confirm the `create-release` job plus all 6 build-matrix jobs succeeded and assets are complete. Base installers (14): macOS `.dmg`×2 + `.app.tar.gz`×2, Windows `-setup.exe`×2 + `.msi`×2, Linux `.AppImage`×2 + `.deb`×2 + `.rpm`×2. With `bundle.createUpdaterArtifacts` on, Tauri emits a detached `.sig` for **every** bundle except the `.dmg` (×12), plus one `latest.json` — so expect **27 assets** total. All matrix jobs upload to the single draft the `create-release` job creates up front (`releaseId`); this replaced letting each job create-or-find its own, which raced into duplicate drafts with assets split across them. `latest.json` is what the in-app updater polls; if it's missing, self-update is silently broken (check the `TAURI_SIGNING_PRIVATE_KEY` secret is set). For macOS, the build log should show `Notarizing ... status Accepted` + `Stapling`:

   ```sh
   gh release view v0.2.0 --json isDraft,assets --jq '{isDraft, assets:[.assets[].name]}'
   ```

5. **Publish:**

   ```sh
   gh release edit v0.2.0 --draft=false --latest
   ```

   (`gh release view --json` has no `isLatest` field — verify with `isDraft`/`publishedAt` instead.)

   Publishing also fires the `Deploy website` workflow (`release: published`), which rebuilds [`site/`](site) so the website's version badge and download links pick up this release automatically — see "Website" below. Nothing else to do for the site.

### Release notes / download table

The auto-generated body has no download table — add one matching prior releases (see `gh release view v0.1.1 --json body`). Set it with `gh release edit v0.2.0 --notes-file <file>`. The table links are built from the asset names, which follow these patterns under `https://github.com/reverentgeek/metermaid/releases/download/v<ver>/`:

- macOS: `MeterMaid_<ver>_aarch64.dmg` (Apple Silicon), `MeterMaid_<ver>_x64.dmg` (Intel)
- Windows: `MeterMaid_<ver>_x64-setup.exe` / `MeterMaid_<ver>_arm64-setup.exe`, `MeterMaid_<ver>_x64_en-US.msi` / `MeterMaid_<ver>_arm64_en-US.msi`
- Linux: `MeterMaid_<ver>_amd64.AppImage` / `MeterMaid_<ver>_aarch64.AppImage`, `MeterMaid_<ver>_amd64.deb` / `MeterMaid_<ver>_arm64.deb`, `MeterMaid-<ver>-1.x86_64.rpm` / `MeterMaid-<ver>-1.aarch64.rpm` (note: rpm uses `-` separators and a `-1` release component)

Conclude the notes with a What's Changed summary (from the changelog) and `**Full Changelog**: https://github.com/reverentgeek/metermaid/compare/v<prev>...v<ver>`.

Signing secrets and the full signed-build env are documented in `README.md` ("Code signing & notarization", "Signing secrets"); Windows toolchain/cross-compile setup is in README "Platform support".

## Website (`site/`)

The marketing/landing site (<https://getmetermaid.com>) lives in [`site/`](site) as a monorepo alongside the app — an Eleventy + Edge.js + Tailwind v4 static site, separate from the Tauri toolchain. It is **self-contained**: its own `package.json`, lockfile, and `pnpm-workspace.yaml` (the last isolates it so `pnpm` from `site/` doesn't merge with the app's root workspace). Work on it from inside `site/` (`cd site && pnpm install && pnpm run dev`); see [`site/CLAUDE.md`](site/CLAUDE.md).

- **No manual version bumps.** The site's version badge, the 6 download buttons, and its JSON-LD are derived from the GitHub Releases API at build time ([`site/src/_data/release.js`](site/src/_data/release.js)) — not hardcoded. A rebuild always reflects whatever is currently the latest published release (with a pinned offline fallback so builds never break).
- **Deploy is release-driven.** Hosting is Netlify, configured from the repo-root [`netlify.toml`](netlify.toml) (`base = "site"`). Pushes to `main` deploy normally; on top of that, [`.github/workflows/site-deploy.yml`](.github/workflows/site-deploy.yml) pings a Netlify build hook on `release: published`, so cutting a release rebuilds the site at the moment "latest" changes. **This needs a `NETLIFY_BUILD_HOOK` repo secret** (create the hook in Netlify → Site configuration → Build & deploy → Build hooks).
- If the app's release **asset names** ever change, update `ASSET_MAP` in `site/src/_data/release.js` to match (the same patterns listed under "Release notes / download table" above).

## Workflow conventions

- **All changes go through a branch + PR — never push directly to `main`.** Don't push/open a PR until the user has tested locally.
- Markdown is **not hard-wrapped**: one line per paragraph / list item (enforced by `.markdownlint.json` via markdownlint-cli2).
- Commit messages end with the project's Co-Authored-By trailer.
