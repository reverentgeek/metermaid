---
name: release
description: Cut a MeterMaid release — version bump across all four lockstep files, tag, watch the build, verify the draft's assets, write the download-table release notes, and publish. Use whenever asked to cut, tag, build, or publish a release, or to write release notes.
---

# Cutting a MeterMaid release

Builds are produced by `.github/workflows/release.yml` (matrix: macOS/Windows/Linux × x64/arm64 via `tauri-action`), triggered by pushing a `v*` tag. The workflow creates a **draft** release; publishing is a deliberate separate step.

Signing posture, the ASIO/GPLv3 per-artifact licensing constraint, and the updater-key rotation footgun are in the root `CLAUDE.md` under "Release & signing" — read those before changing anything about signing.

## 1. Bump the version in all four places

These must stay in lockstep:

- `package.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`
- the `metermaid` entry in `src-tauri/Cargo.lock`

Add a dated `## [x.y.z]` section to `CHANGELOG.md`, and — if the release has anything user-facing — a plain-English entry to [`site/content/whatsnew.md`](../../../site/content/whatsnew.md) for the website. This is normally done in the feature PR, not at tag time.

Verify the lockfile still matches after bumping `Cargo.toml`:

```sh
cd src-tauri && cargo metadata --locked --format-version 1 > /dev/null
```

## 2. Tag and push

Annotated, subject `MeterMaid x.y.z`:

```sh
git tag -a v0.2.0 -m "MeterMaid 0.2.0"
git push origin v0.2.0
```

## 3. Watch the build

```sh
gh run list --workflow=release.yml --limit 3
gh run watch <run-id> --exit-status
```

## 4. Verify before publishing

Confirm the `create-release` job plus all 6 build-matrix jobs succeeded, and that assets are complete.

Base installers (14): macOS `.dmg`×2 + `.app.tar.gz`×2, Windows `-setup.exe`×2 + `.msi`×2, Linux `.AppImage`×2 + `.deb`×2 + `.rpm`×2. With `bundle.createUpdaterArtifacts` on, Tauri emits a detached `.sig` for **every** bundle except the `.dmg` (×12), plus one `latest.json` — so expect **27 assets** total.

```sh
gh release view v0.2.0 --json isDraft,assets --jq '{isDraft, count:(.assets|length), assets:[.assets[].name]}'
```

- `latest.json` is what the in-app updater polls. If it's missing, self-update is silently broken — check the `TAURI_SIGNING_PRIVATE_KEY` secret is set.
- All matrix jobs upload to the single draft `create-release` makes up front (`releaseId`). This replaced letting each job create-or-find its own, which raced into duplicate drafts with assets split across them.
- For macOS, the build log should show `Notarizing ... status Accepted` for both legs. It does **not** log a "Stapling" line — `tauri-action` doesn't emit one, so its absence is normal and not worth investigating.

## 5. Write the release notes

The auto-generated body has no download table. **Always** add a per-OS Download table plus a What's Changed section — this is a standing preference, not a question to ask.

Match the previous release's format: `gh release view <prev-tag> --json body`.

Write the notes to a **`.txt`** file, not `.md`, so the repo's markdownlint hook doesn't flag release-notes-only conventions (leading `## Download`, bare Full Changelog URL). Then:

```sh
gh release edit v0.2.0 --notes-file <file>
```

Setting notes does not publish the draft.

### Asset name patterns

Links are built under `https://github.com/reverentgeek/metermaid/releases/download/v<ver>/`:

- macOS: `MeterMaid_<ver>_aarch64.dmg` (Apple Silicon), `MeterMaid_<ver>_x64.dmg` (Intel)
- Windows: `MeterMaid_<ver>_x64-setup.exe` / `MeterMaid_<ver>_arm64-setup.exe`, `MeterMaid_<ver>_x64_en-US.msi` / `MeterMaid_<ver>_arm64_en-US.msi`
- Linux: `MeterMaid_<ver>_amd64.AppImage` / `MeterMaid_<ver>_aarch64.AppImage`, `MeterMaid_<ver>_amd64.deb` / `MeterMaid_<ver>_arm64.deb`, `MeterMaid-<ver>-1.x86_64.rpm` / `MeterMaid-<ver>-1.aarch64.rpm` (note: rpm uses `-` separators and a `-1` release component)

Follow the table with the signing caveat line, then a What's Changed summary drawn from `CHANGELOG.md`, ending with `**Full Changelog**: https://github.com/reverentgeek/metermaid/compare/v<prev>...v<ver>`.

Cross-check every link in the table against the real asset names before publishing — a typo ships a dead download button.

## 6. Publish

```sh
gh release edit v0.2.0 --draft=false --latest
```

`gh release view --json` has no `isLatest` field — verify with `isDraft`/`publishedAt` instead.

Publishing does two things at once: existing installs start getting the in-app update prompt, and the `release: published` hook fires the `Deploy website` workflow, rebuilding [`site/`](../../../site) so the version badge and download links pick up the new release automatically. Nothing else to do for the site.

Because publishing prompts every existing user, it's worth a beat of judgment for releases with nothing user-facing — parking the draft until the next real change is a legitimate option.
