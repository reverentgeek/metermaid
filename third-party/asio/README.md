# Vendored Steinberg ASIO SDK (2.3)

This is a trimmed copy of the [Steinberg ASIO SDK](https://github.com/audiosdk/asio), vendored so the Windows-x64 build is hermetic and reproducible in CI.

## Why it's here

cpal's `asio` feature compiles the SDK's C++ host sources at build time and locates them via the `CPAL_ASIO_DIR` environment variable. Vendoring removes the need to download the SDK on every build machine. Only the files cpal's `asio-sys` build script actually uses are included (`common/`, `host/`, `host/pc/`); the example host app, driver-author scaffolding, PDFs, and logo artwork are omitted. See [LICENSE.txt](LICENSE.txt) for the SDK license and [changes.txt](changes.txt) for its version history.

## Build scope

Only the `x86_64-pc-windows-msvc` target links ASIO (there is no ARM64 ASIO SDK, and the other platforms use their native hosts). See the repository `README.md` "Platform support" and "Licensing" sections.

## Licensing

The ASIO SDK is dual-licensed: the Steinberg ASIO License **or** GPLv3. MeterMaid uses the GPLv3 option. Because GPLv3 is copyleft, the **Windows-x64 release binary** — the only artifact that links ASIO — is distributed under GPLv3. MeterMaid's own source remains MIT, as do the macOS, Linux, and Windows-ARM64 binaries (none of which link ASIO). "ASIO" is a trademark of Steinberg Media Technologies GmbH.
