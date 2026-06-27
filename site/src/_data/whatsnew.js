/**
 * Build-time read of the curated, musician-facing release notes.
 *
 * The site's audience is musicians, not developers, so /updates/ and the
 * home-page teaser render hand-written plain-English summaries from
 * ../../content/whatsnew.md — NOT the technical CHANGELOG.md (which stays the
 * developer record on GitHub). Each release is a "## <version> <date>" heading
 * followed by a short markdown body.
 *
 * Add an entry to content/whatsnew.md at release time and the page refreshes on
 * the next site rebuild. No GitHub API call.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import MarkdownIt from "markdown-it";

const md = new MarkdownIt({ linkify: true, html: false });

// Open external links in a new tab.
const defaultLinkOpen =
	md.renderer.rules.link_open ||
	( ( tokens, idx, options, _env, self ) => self.renderToken( tokens, idx, options ) );
md.renderer.rules.link_open = ( tokens, idx, options, env, self ) => {
	tokens[ idx ].attrSet( "target", "_blank" );
	tokens[ idx ].attrSet( "rel", "noopener" );
	return defaultLinkOpen( tokens, idx, options, env, self );
};

const SOURCE = resolve(
	dirname( fileURLToPath( import.meta.url ) ),
	"../../content/whatsnew.md"
);

const MONTHS = [
	"January", "February", "March", "April", "May", "June",
	"July", "August", "September", "October", "November", "December",
];

function formatDate( iso ) {
	const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec( iso || "" );
	if ( !m ) {
		return iso || "";
	}
	const [ , y, mo, d ] = m;
	return `${MONTHS[ Number( mo ) - 1 ]} ${Number( d )}, ${y}`;
}

export default function () {
	let text;
	try {
		text = readFileSync( SOURCE, "utf8" );
	} catch ( err ) {
		console.warn( `[whatsnew] could not read content/whatsnew.md: ${err.message}` );
		return { releases: [], featured: null };
	}

	const releases = [];
	let cur = null;

	for ( const raw of text.split( "\n" ) ) {
		// "## <semver> <YYYY-MM-DD>" — everything before the first such heading
		// (the file's intro comment and its top-level title) is ignored.
		const head = /^##\s+(\d+\.\d+\.\d+)\s+(\d{4}-\d{2}-\d{2})\s*$/.exec( raw.trimEnd() );
		if ( head ) {
			cur = { version: head[ 1 ], date: formatDate( head[ 2 ] ), body: [] };
			releases.push( cur );
			continue;
		}
		if ( cur ) {
			cur.body.push( raw );
		}
	}

	const out = releases.map( ( r ) => ( {
		version: r.version,
		date: r.date,
		html: md.render( r.body.join( "\n" ).trim() ),
	} ) );

	return { releases: out, featured: out[ 0 ] || null };
}
