---
name: beta-release
description: Cut a MeterMaid beta (prerelease) build from a feature branch so testers can try it before the real release — numeric prerelease version bump, tag, watch the build, verify assets, write beta release notes, and publish as a GitHub prerelease that existing installs and the website ignore. Use whenever asked for a beta, prerelease, preview, or test build to share.
---

# Cutting a MeterMaid beta release

A beta reuses the normal release pipeline (`.github/workflows/release.yml`, triggered by any `v*` tag) and differs from a real release in four places. Read the [`release`](../release/SKILL.md) skill first: everything not called out here (asset count, asset name patterns, notarization log expectations) is the same.

Why a beta is safe to publish: the in-app updater polls `releases/latest/download/latest.json` and `site/src/_data/release.js` reads `/releases/latest`. GitHub's "latest" skips prereleases, so **as long as the release is flagged prerelease** neither existing installs nor the website see it. A beta install compares higher than the previous stable (no downgrade prompt) and lower than the final `x.y.z`, so testers roll forward on their own once the real release is published.

## 1. Bump to a numeric prerelease version

Use `x.y.z-N` (`0.7.0-1`, then `0.7.0-2`, …), **not** `x.y.z-beta.N`. The MSI bundler only accepts a numeric prerelease identifier, so `-beta.1` risks failing both Windows legs. The release title and notes can still say "beta 1".

Same four lockstep files as a real release:

- `package.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`
- the `metermaid` entry in `src-tauri/Cargo.lock`

```sh
cd src-tauri && cargo metadata --locked --format-version 1 > /dev/null
```

Do **not** add a `CHANGELOG.md` section or a `site/content/whatsnew.md` entry. Both belong to the final release; the site would show a whatsnew entry immediately.

Commit the bump on the feature branch (`Bump version to x.y.z-N for beta N`). The final release PR bumps again to plain `x.y.z`.

## 2. Tag and push

Annotated, subject `MeterMaid x.y.z beta N`. The branch does not need to be merged or even pushed; pushing the tag uploads its commits.

```sh
git tag -a v0.7.0-1 -m "MeterMaid 0.7.0 beta 1"
git push origin v0.7.0-1
```

## 3. Watch and verify

Identical to the `release` skill, steps 3 and 4: `create-release` plus all 6 matrix jobs green, **27 assets**, `latest.json` present.

```sh
gh run list --workflow=release.yml --limit 3
gh run watch <run-id> --exit-status
gh release view v0.7.0-1 --json isDraft,isPrerelease,assets --jq '{isDraft, isPrerelease, count:(.assets|length), assets:[.assets[].name]}'
```

Asset names carry the full version, so the patterns from the `release` skill hold with `<ver>` = `0.7.0-1` (for example `MeterMaid_0.7.0-1_aarch64.dmg`). The rpm and msi names are the ones most likely to deviate with a prerelease version, so build the download table from the real asset list, not from the pattern.

## 4. Write the beta notes

Write to a `.txt` file in the scratchpad, then `gh release edit <tag> --title "MeterMaid x.y.z beta N" --notes-file <file>`. Structure:

1. A short "this is a beta" callout: what it is for, that stable users are not auto-updated to it, that beta installs will update to the final release automatically, and where to send feedback (the GitHub discussion or issue that prompted the work, else Issues).
2. The standard `## Download` table and the signing and GPLv3 caveat lines, copied in format from the previous release (`gh release view <prev-tag> --json body`).
3. `## What's new to try`, drawn from the branch's commits (`git log main..HEAD`) since there is no changelog section yet. Written for musicians: what changed and what to poke at. Call out changed defaults and anything that moved.
4. `**Full Changelog**: https://github.com/reverentgeek/metermaid/compare/v<prev-stable>...v<beta-tag>`

Cross-check every table link against the real asset names.

## 5. Publish as a prerelease

The workflow creates the draft with `prerelease: false`, so the flag must be set here. This is the one step that keeps the beta away from existing users and the website:

```sh
gh release edit v0.7.0-1 --draft=false --prerelease --latest=false
```

Never pass `--latest`. Afterwards confirm stable is untouched:

```sh
gh release view v0.7.0-1 --json isDraft,isPrerelease
gh api repos/reverentgeek/metermaid/releases/latest --jq .tag_name   # must still be the previous stable
```

Publishing fires the `Deploy website` workflow (`release: published` includes prereleases). That is harmless: the site rebuilds against `/releases/latest`, which is still the stable release.

## Follow-up betas and the final release

- Another beta: bump to `x.y.z-(N+1)`, repeat. Beta installs update only when the final release ships, not from beta to beta, so tell testers to download the new build.
- Final release: bump to plain `x.y.z` in the feature PR along with the `CHANGELOG.md` and `whatsnew.md` entries, then follow the `release` skill. The MSI for a beta is versioned `x.y.z.N`, which is numerically above the final `x.y.z.0`; Tauri's MSI allows downgrades by default so the final still installs over it, but mention it if a tester reports an MSI upgrade problem.
- Old beta releases can be deleted (release and tag) once the final ships.
