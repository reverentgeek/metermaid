# MeterMaid website

The landing page for [MeterMaid](https://github.com/reverentgeek/metermaid) — a cross-platform desktop LUFS / loudness meter. Built with [Eleventy](https://www.11ty.dev/) + [Edge.js](https://edgejs.dev/) templates and [Tailwind CSS v4](https://tailwindcss.com/), deployed on Netlify. It lives in the metermaid app repo as a monorepo (this `site/` directory); all commands below run from here.

## Develop

```sh
pnpm install
pnpm run dev        # 11ty serve + Tailwind watch
```

Open the URL Eleventy prints (default <http://localhost:8080>).

## Build & test

```sh
pnpm run build      # → _site/
pnpm run test:install   # one-time: install the Playwright browser
pnpm test           # build, then run the Playwright suite against _site/
```

## How releases reach the site

The version badge and download links come from the GitHub Releases API at build time ([`src/_data/release.js`](src/_data/release.js)) — there's nothing to bump by hand. When the app publishes a new release, the [`Deploy website`](../.github/workflows/site-deploy.yml) workflow pings a Netlify build hook, the site rebuilds, and the new version goes live. To move the site to a different domain (e.g. a subdomain), change `url` in [`src/_data/site.json`](src/_data/site.json).

## License

MIT © David Neal
