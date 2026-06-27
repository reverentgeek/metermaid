/**
 * Build-time lookup of the latest MeterMaid release.
 *
 * Runs once per Eleventy build. It asks the GitHub Releases API for the latest
 * *published* release and derives the version and per-platform download URLs
 * from the actual uploaded assets — so the site's version badge and download
 * buttons update automatically every time the site is rebuilt (e.g. when the
 * `release: published` deploy hook fires; see .github/workflows/site-deploy.yml).
 *
 * If the API is unreachable or rate-limited (unauthenticated requests are
 * capped at 60/hr), it falls back to a pinned version so the build never fails.
 * Set GITHUB_TOKEN in the build environment to raise the rate limit.
 */

const REPO = "reverentgeek/metermaid";
const API = `https://api.github.com/repos/${REPO}/releases/latest`;

// Bump this only as a safety net for offline/rate-limited builds; normal builds
// ignore it entirely and use whatever the API reports as latest.
const FALLBACK_VERSION = "0.4.0";

const assetUrl = ( tag, name ) =>
	`https://github.com/${REPO}/releases/download/${tag}/${name}`;

// Map a friendly key to the asset name pattern (from the release workflow) plus
// a constructed-URL fallback name, in case that specific asset is ever missing.
const ASSET_MAP = {
	macArm: { re: /aarch64\.dmg$/,        name: ( v ) => `MeterMaid_${v}_aarch64.dmg` },
	macX64: { re: /_x64\.dmg$/,           name: ( v ) => `MeterMaid_${v}_x64.dmg` },
	winX64: { re: /_x64-setup\.exe$/,     name: ( v ) => `MeterMaid_${v}_x64-setup.exe` },
	winArm: { re: /_arm64-setup\.exe$/,   name: ( v ) => `MeterMaid_${v}_arm64-setup.exe` },
	linX64: { re: /_amd64\.AppImage$/,    name: ( v ) => `MeterMaid_${v}_amd64.AppImage` },
	linArm: { re: /_aarch64\.AppImage$/,  name: ( v ) => `MeterMaid_${v}_aarch64.AppImage` },
};

function buildDownloads( version, tag, assets ) {
	const downloads = {};
	for ( const [ key, { re, name } ] of Object.entries( ASSET_MAP ) ) {
		const hit = assets.find( ( a ) => re.test( a.name ) );
		downloads[ key ] = hit ? hit.browser_download_url : assetUrl( tag, name( version ) );
	}
	return downloads;
}

function fallback() {
	const version = FALLBACK_VERSION;
	const tag = `v${version}`;
	return {
		version,
		tag,
		htmlUrl: `https://github.com/${REPO}/releases/tag/${tag}`,
		source: "fallback",
		downloads: buildDownloads( version, tag, [] ),
	};
}

export default async function () {
	try {
		const headers = {
			"Accept": "application/vnd.github+json",
			"User-Agent": "metermaid-site-build",
		};
		if ( process.env.GITHUB_TOKEN ) {
			headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
		}

		const res = await fetch( API, { headers } );
		if ( !res.ok ) {
			throw new Error( `GitHub API responded ${res.status} ${res.statusText}` );
		}

		const rel = await res.json();
		const tag = rel.tag_name;                 // e.g. "v0.4.0"
		const version = tag.replace( /^v/, "" );  // e.g. "0.4.0"
		const assets = rel.assets ?? [];

		console.log( `[release] latest = ${tag} (${assets.length} assets) via GitHub API` );

		return {
			version,
			tag,
			htmlUrl: rel.html_url,
			source: "github-api",
			downloads: buildDownloads( version, tag, assets ),
		};
	} catch ( err ) {
		console.warn( `[release] GitHub API lookup failed, using fallback v${FALLBACK_VERSION}: ${err.message}` );
		return fallback();
	}
}
