/**
 * Minimal static file server for _site/ used by Playwright tests.
 * No extra dependencies — uses only Node built-ins.
 */
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join } from "node:path";

const PORT = process.env.PORT ?? 3001;
const ROOT = "_site";

const MIME = {
	".html": "text/html; charset=utf-8",
	".css":  "text/css",
	".js":   "application/javascript",
	".svg":  "image/svg+xml",
	".png":  "image/png",
	".ico":  "image/x-icon",
	".xml":  "application/xml",
	".txt":  "text/plain",
};

createServer( async ( req, res ) => {
	let urlPath = ( req.url ?? "/" ).split( "?" )[ 0 ];
	// Map directory paths (e.g. "/" or "/updates/") to their index.html.
	if ( urlPath.endsWith( "/" ) ) {
		urlPath += "index.html";
	}
	const filePath = join( ROOT, urlPath );

	try {
		const data = await readFile( filePath );
		const mime = MIME[ extname( filePath ) ] ?? "application/octet-stream";
		res.writeHead( 200, { "Content-Type": mime } );
		res.end( data );
	} catch {
		res.writeHead( 404, { "Content-Type": "text/plain" } );
		res.end( "Not found" );
	}
} ).listen( PORT, () => {
	console.log( `Serving _site on http://localhost:${PORT}` );
} );
