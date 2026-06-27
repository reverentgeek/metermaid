import { test, expect } from "@playwright/test";

// ─── Accessibility ────────────────────────────────────────────────────────────

test.describe( "Accessibility", () => {
	test( "skip link moves keyboard focus to main content", async ( { page } ) => {
		await page.goto( "/" );

		await page.keyboard.press( "Tab" );
		await expect( page.locator( ".skip-link" ) ).toBeFocused();

		// Activating the skip link should move focus to #main-content, not leave
		// it on the link. This requires tabindex="-1" on the target element.
		await page.keyboard.press( "Enter" );
		await expect( page.locator( "#main-content" ) ).toBeFocused();
	} );

	test( "#features anchor moves keyboard focus to the features section", async ( { page } ) => {
		await page.goto( "/" );

		// Clicking the hero secondary CTA should move focus to #features,
		// not just scroll to it. Requires tabindex="-1" on the section.
		await page.click( 'a[href="#features"]' );
		await expect( page.locator( "#features" ) ).toBeFocused();
	} );

	test( "page has exactly one h1", async ( { page } ) => {
		await page.goto( "/" );
		await expect( page.locator( "h1" ) ).toHaveCount( 1 );
	} );

	test( "named page sections are reachable as landmarks", async ( { page } ) => {
		await page.goto( "/" );
		const expectedLabels = [
			"Hero",
			"Screenshot",
			"Features",
			"Download",
			"Getting started",
			"Updates",
			"Feedback",
		];
		for ( const label of expectedLabels ) {
			await expect(
				page.locator( `section[aria-label="${label}"]` ),
				`section[aria-label="${label}"] not found`
			).toBeAttached();
		}
	} );

	test( "every inline SVG has an accessible name or is marked decorative", async ( { page } ) => {
		await page.goto( "/" );
		const svgs = await page.locator( "svg" ).all();
		for ( const svg of svgs ) {
			const isHidden     = await svg.getAttribute( "aria-hidden" ) === "true";
			const ariaLabel    = await svg.getAttribute( "aria-label" );
			const ariaLabelled = await svg.getAttribute( "aria-labelledby" );
			const hasTitle     = await svg.locator( "title" ).count() > 0;
			expect(
				isHidden || ariaLabel || ariaLabelled || hasTitle,
				"SVG must be aria-hidden or carry an accessible name"
			).toBeTruthy();
		}
	} );

	test( "the screenshot image has descriptive alt text", async ( { page } ) => {
		await page.goto( "/" );
		const shot = page.locator( 'img[src="/images/screenshot.png"]' );
		await expect( shot ).toHaveAttribute( "alt", /.{40,}/ );
	} );

	test( "interactive elements have a visible focus outline", async ( { page } ) => {
		await page.goto( "/" );

		await page.keyboard.press( "Tab" ); // skip link
		await page.keyboard.press( "Tab" ); // nav logo link

		const outlineStyle = await page.evaluate( () => {
			const el = document.activeElement;
			return window.getComputedStyle( el ).outlineStyle;
		} );

		expect( outlineStyle, "Focused element has no visible outline" ).not.toBe( "none" );
	} );

	test( "animations are suppressed under prefers-reduced-motion", async ( { page } ) => {
		await page.emulateMedia( { reducedMotion: "reduce" } );
		await page.goto( "/" );

		const animEl = page.locator( ".animate-fade-up" ).first();
		await expect( animEl ).toBeVisible();

		const [ opacity, animationName ] = await animEl.evaluate( ( el ) => {
			const styles = window.getComputedStyle( el );
			return [ styles.opacity, styles.animationName ];
		} );

		expect( opacity ).toBe( "1" );
		expect( animationName ).toBe( "none" );
	} );
} );

// ─── SEO / Metadata ───────────────────────────────────────────────────────────

test.describe( "SEO and metadata", () => {
	test( "page title includes the product name", async ( { page } ) => {
		await page.goto( "/" );
		await expect( page ).toHaveTitle( /MeterMaid/ );
	} );

	test( "canonical link is present and absolute", async ( { page } ) => {
		await page.goto( "/" );
		const canonical = page.locator( 'link[rel="canonical"]' );
		await expect( canonical ).toHaveAttribute( "href", /^https:\/\// );
	} );

	test( "Open Graph meta tags are present and populated", async ( { page } ) => {
		await page.goto( "/" );

		const ogTitle = page.locator( 'meta[property="og:title"]' );
		const ogImage = page.locator( 'meta[property="og:image"]' );
		const ogImageAlt = page.locator( 'meta[property="og:image:alt"]' );

		await expect( ogTitle ).toHaveAttribute( "content", /.+/ );
		await expect( ogImage ).toHaveAttribute( "content", /social-preview\.png/ );
		await expect( ogImageAlt ).toHaveAttribute( "content", /.+/ );
	} );

	test( "Twitter card meta tags are present and use large image format", async ( { page } ) => {
		await page.goto( "/" );

		const twitterCard  = page.locator( 'meta[name="twitter:card"]' );
		const twitterImage = page.locator( 'meta[name="twitter:image"]' );
		const twitterAlt   = page.locator( 'meta[name="twitter:image:alt"]' );

		await expect( twitterCard ).toHaveAttribute( "content", "summary_large_image" );
		await expect( twitterImage ).toHaveAttribute( "content", /social-preview\.png/ );
		await expect( twitterAlt ).toHaveAttribute( "content", /.+/ );
	} );

	test( "structured data is valid JSON-LD describing the application", async ( { page } ) => {
		await page.goto( "/" );

		const raw = await page.locator( 'script[type="application/ld+json"]' ).textContent();

		let data;
		expect( () => { data = JSON.parse( raw ); }, "JSON-LD is not valid JSON" ).not.toThrow();

		expect( data[ "@type" ] ).toBe( "SoftwareApplication" );
		expect( data.name ).toBe( "MeterMaid" );
		expect( data.operatingSystem ).toMatch( /macOS/ );
		expect( data.offers?.price ).toBe( "0" );
	} );
} );

// ─── Content ──────────────────────────────────────────────────────────────────

test.describe( "Content", () => {
	test( "download links point at GitHub release assets", async ( { page } ) => {
		await page.goto( "/" );

		const downloadLinks = page.locator(
			'#download a[href*="/releases/download/"]'
		);
		// macOS (2) + Windows (2) + Linux (2) primary asset buttons
		await expect( downloadLinks ).toHaveCount( 6 );

		// Every direct-download href must target the reverentgeek/metermaid repo
		for ( const link of await downloadLinks.all() ) {
			const href = await link.getAttribute( "href" );
			expect( href ).toMatch(
				/^https:\/\/github\.com\/reverentgeek\/metermaid\/releases\/download\/v/
			);
		}
	} );

	test( "every download asset link carries a version number", async ( { page } ) => {
		await page.goto( "/" );
		// Guards against a stale build that drops the version interpolation,
		// which would produce broken (404) asset URLs.
		for ( const link of await page.locator( '#download a[href*="/releases/download/"]' ).all() ) {
			const href = await link.getAttribute( "href" );
			expect( href, `asset link missing version: ${href}` ).toMatch( /\/v\d+\.\d+\.\d+\// );
		}
	} );

	test( "primary download CTA reaches the download section", async ( { page } ) => {
		await page.goto( "/" );
		await page.click( 'a[href="#download"]' );
		await expect( page.locator( "#download" ) ).toBeFocused();
	} );

	test( "feedback section links to GitHub issues", async ( { page } ) => {
		await page.goto( "/" );
		const issues = page.locator( '#feedback a[href*="/issues/"]' );
		await expect( issues.first() ).toBeVisible();
	} );

	test( "heading order is logical (no skipped levels)", async ( { page } ) => {
		await page.goto( "/" );

		const headings = await page.evaluate( () =>
			[ ...document.querySelectorAll( "h1,h2,h3,h4,h5,h6" ) ].map( h => ( {
				level: parseInt( h.tagName[ 1 ] ),
				text: h.textContent.trim().slice( 0, 60 ),
			} ) )
		);

		let prev = 0;
		for ( const h of headings ) {
			expect(
				h.level - prev,
				`Heading "${h.text}" skips from h${prev} to h${h.level}`
			).toBeLessThanOrEqual( 1 );
			prev = h.level;
		}
	} );
} );

// ─── Updates / changelog page ──────────────────────────────────────────────────

test.describe( "Updates page", () => {
	test( "loads with exactly one h1", async ( { page } ) => {
		await page.goto( "/updates/" );
		await expect( page.locator( "h1" ) ).toHaveCount( 1 );
	} );

	test( "renders multiple release versions from the changelog", async ( { page } ) => {
		await page.goto( "/updates/" );
		const versions = page.locator( "article h2" );
		// The changelog has many releases; guard against a parser/render regression
		// that would drop them to zero or one.
		expect( await versions.count() ).toBeGreaterThan( 3 );
		await expect( versions.filter( { hasText: "v0.1.0" } ) ).toHaveCount( 1 );
	} );

	test( "links to the full release notes on GitHub", async ( { page } ) => {
		await page.goto( "/updates/" );
		await expect(
			page.locator( 'a[href$="/releases"]' ).first()
		).toBeVisible();
	} );

	test( "heading order is logical (h1 → h2, no skipped levels)", async ( { page } ) => {
		await page.goto( "/updates/" );
		const headings = await page.evaluate( () =>
			[ ...document.querySelectorAll( "h1,h2,h3,h4,h5,h6" ) ].map( h => parseInt( h.tagName[ 1 ] ) )
		);
		let prev = 0;
		for ( const level of headings ) {
			expect( level - prev ).toBeLessThanOrEqual( 1 );
			prev = level;
		}
	} );
} );

test.describe( "Homepage updates teaser", () => {
	test( "links to the full updates page", async ( { page } ) => {
		await page.goto( "/" );
		await expect( page.locator( '#updates a[href="/updates/"]' ) ).toHaveCount( 1 );
	} );
} );
