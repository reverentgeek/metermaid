# Contributing to MeterMaid

Thanks for your interest in improving MeterMaid! This is a small Tauri app (Rust audio engine + TypeScript/Vite UI), so the contribution loop is short.

By participating, you agree to abide by the [Code of Conduct](CODE_OF_CONDUCT.md).

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (stable) with `rustfmt` and `clippy`
- [Node.js](https://nodejs.org) and [pnpm](https://pnpm.io)
- The [Tauri system dependencies](https://tauri.app/start/prerequisites/) for your OS

## Setup

```sh
pnpm install
pnpm tauri dev
```

`pnpm install` also enables a Git pre-commit hook (via `core.hooksPath`) that lints any staged Rust, TypeScript, and Markdown before each commit.

## Linting

TypeScript is linted and formatted with [Biome](https://biomejs.dev), Markdown with [markdownlint](https://github.com/DavidAnson/markdownlint), and Rust with `rustfmt` + `clippy`. The pre-commit hook runs these on staged files automatically; you can also run them by hand:

```sh
pnpm lint      # Biome (TypeScript) + markdownlint (Markdown)
pnpm format    # apply Biome fixes/formatting
```

## Before opening a pull request

Please make sure all of the following pass locally — CI runs the same checks:

```sh
# Frontend
pnpm lint
pnpm build

# Rust (run from src-tauri/)
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

- Run `pnpm format` to fix TypeScript formatting and `cargo fmt` to fix Rust formatting.
- New audio/analysis behavior should come with tests. See the golden-signal suite in `src-tauri/src/audio.rs` (`#[cfg(test)] mod tests`) for the pattern — you can drive the `Analyzer` directly with synthesized frames, no audio device required. If you have `ffmpeg` installed, the ignored cross-check is handy: `cargo test ebur128_matches_ffmpeg -- --ignored --nocapture`.

## Guidelines

- Keep the realtime audio callback allocation- and lock-free (it only writes into the SPSC ring; all analysis happens on the engine thread). See `audio.rs` for the rationale.
- Match the existing code style and keep changes focused.
- For larger changes, open an issue first to discuss the approach.

By contributing you agree that your contributions are licensed under the [MIT License](LICENSE).
