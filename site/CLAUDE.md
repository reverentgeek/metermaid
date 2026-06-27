# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Marketing / landing site for **MeterMaid** (<https://getmetermaid.com>) — a cross-platform desktop LUFS / loudness meter. Single-page site: hero, screenshot, features, download, and feedback sections.

This `site/` directory lives **inside the metermaid app repo as a monorepo**. The app (Tauri/Rust + TS) is at the repo root; the website is self-contained here with its own `package.json`, lockfile, and `pnpm-workspace.yaml` (the workspace file isolates the site so pnpm doesn't merge it with the app's root workspace). The version and download links are **not hardcoded** — they're fetched from the GitHub Releases API at build time (see `src/_data/release.js`), so a new app release updates the site on its next build.

## Tech Stack

- **Static site generator:** Eleventy (11ty) v3 with ESM (`"type": "module"`)
- **Templating:** Edge.js via `eleventy-plugin-edgejs` (`.edge` files, not Nunjucks/Liquid)
- **CSS:** Tailwind CSS v4 (standalone CLI, not PostCSS plugin) with custom theme in `src/css/styles.css`
- **Tests:** Playwright (accessibility, SEO, and content assertions in `tests/site.spec.js`)
- **Package manager:** pnpm (isolated workspace — `pnpm install` / `pnpm run …` are run from `site/`)
- **Hosting:** Netlify, configured from the **repo-root** `../netlify.toml` (`base = "site"`); a `release: published` GitHub Action triggers a deploy (`../.github/workflows/site-deploy.yml`)

## Commands

- `pnpm run dev` — Dev server (11ty serve + Tailwind watch in parallel via `scripts/dev.mjs`)
- `pnpm run build` — Production build (11ty then Tailwind with minification)
- `pnpm run build:11ty` / `pnpm run build:css` — Build one half only
- `pnpm test` — `pnpm build` then Playwright against the built `_site/` (served by `scripts/serve-site.mjs` on :3001)
- `pnpm run test:install` — One-time `playwright install chromium`

## Architecture

```text
src/
  index.edge              # The single page (hero, screenshot, features, download, feedback)
  _includes/layouts/
    base.edge             # Base HTML layout: head/meta/OG, JSON-LD, header, footer
  _data/site.json         # Static config: canonical URL, repo, links (NOT the version)
  _data/release.js        # Build-time GitHub API fetch → latest version + download URLs
  css/styles.css          # Tailwind v4 @theme + custom CSS (animations, cards)
  images/screenshot.png   # App screenshot (copied from the metermaid repo's docs/)
  sitemap.edge            # XML sitemap template
  favicon.svg, robots.txt # Static assets (passthrough copy)
eleventy.config.js        # 11ty config: Edge.js plugin, passthrough copies, dir setup
```

- **Input:** `src/` → **Output:** `_site/`
- Tailwind scans `_site/` for classes via `@source "../../_site"` — so **build 11ty before CSS** (the `build` script already orders them).

## Data: `site.json` (static) and `release.js` (live)

Two global data files feed the templates. Keep version/URL facts here, never hardcoded in `.edge` files.

`src/_data/site.json` — static config, available as `site.*`:

- `url` — canonical site origin (canonical link, OG URLs, sitemap, JSON-LD). **Change this one value to move the site** (e.g. to a `metermaid.reverentgeek.com` subdomain).
- `repo`, `blogPost`, `author`, `name`, `tagline`, `description` — links and copy used across the page and footer.

`src/_data/release.js` — async build-time lookup, available as `release.*`:

- Calls the GitHub Releases API for the **latest published release**, then derives `release.version`, `release.tag`, `release.htmlUrl`, and `release.downloads.{macArm,macX64,winX64,winArm,linX64,linArm}` (exact `browser_download_url`s from the API). The version badges, the 6 download buttons, and the JSON-LD all read from this — so they refresh automatically on every build.
- On API failure (offline, or the unauthenticated 60/hr rate limit) it logs a warning and falls back to a pinned `FALLBACK_VERSION` so the build never breaks. Set `GITHUB_TOKEN` in the build env to raise the limit. Bump `FALLBACK_VERSION` only as a safety net; normal builds ignore it.
- Asset matching uses the names the metermaid release workflow uploads (macOS `…_aarch64.dmg` / `…_x64.dmg`; Windows `…_x64-setup.exe` / `…_arm64-setup.exe`; Linux `…_amd64.AppImage` / `…_aarch64.AppImage`). The `.msi`, `.deb`, and `.rpm` formats are reached via the "all assets" link → `release.htmlUrl`. If those asset-name patterns ever change in the app's release workflow, update `ASSET_MAP` here to match.

The Playwright suite asserts every download link still resolves to a `…/releases/download/vX.Y.Z/…` URL, so a broken fetch or renamed asset fails the build.

## Design System

Custom Tailwind v4 theme tokens in `src/css/styles.css` `@theme`, lifted from the **MeterMaid app's own palette** so the site matches the product:

- **Colors:** `ink-*` (dark blue-grays, `ink-950` #0a0c10 background → `ink-100` #e8ecf4 text), and the meter accent trio `green-400` #54e08a, `amber-400` #f2c14e, `red-400` #ff5d5d.
- **Fonts:** `font-display` (Space Grotesk), `font-body` (Inter), `font-mono` (JetBrains Mono) — Google Fonts.
- Custom classes: `.spectrum-bar` (hero EQ animation), `.feature-card`, `.download-card`, `.grid-overlay`, `.text-gradient-meter`, `.link-underline`, `.skip-link`.
- All animations are disabled under `prefers-reduced-motion` (a Playwright test enforces this).

## Edge.js Templating Notes

- `{{ }}` escaped output, `{{{ }}}` raw/unescaped (used for `{{{ content }}}` in the layout).
- Comments: `{{-- comment --}}`. Frontmatter is YAML between `---`.
- Global data is available as `site.*` (from `_data/site.json`) and `release.*` (from `_data/release.js`); 11ty supplies `page.*`.

## Conventions

- **All changes go through a branch + PR.** Don't open a PR until the site has been checked locally (`pnpm test`).
- Markdown is **not hard-wrapped**: one line per paragraph / list item.
- Keep the canonical URL in `src/_data/site.json` and the version/download URLs in `src/_data/release.js` — never hardcode either in templates.
