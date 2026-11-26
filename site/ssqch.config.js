import { defineConfig } from "ssqch";

// Tailwind scans the templates (`@source` in styles.css), so it can run before
// or alongside the page build. $SSQCH_OUTPUT is the output folder.
const tailwind = "tailwindcss -i src/css/styles.css -o $SSQCH_OUTPUT/css/styles.css";

export default defineConfig( ( { serving } ) => ( {
	dirs: { input: "src", output: "_site", includes: "_includes", layouts: "_includes/layouts" },
	templateFormats: [ "edge" ],
	passthrough: [ "src/images", "src/favicon.svg", "src/robots.txt" ],
	// A build runs Tailwind once; `ssqch serve` keeps its watcher running instead.
	beforeBuild: serving ? [] : { run: `${ tailwind } --minify` },
	// Only Tailwind reads styles.css, so saving it needs no page rebuild.
	serve: { run: [ `${ tailwind } --watch` ], ignore: [ "src/css/styles.css" ] }
} ) );
