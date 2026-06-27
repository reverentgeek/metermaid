import edgePlugin from "eleventy-plugin-edgejs";

export default function( eleventyConfig ) {
	eleventyConfig.addPlugin( edgePlugin );

	eleventyConfig.addPassthroughCopy( "src/images" );
	eleventyConfig.addPassthroughCopy( "src/favicon.svg" );
	eleventyConfig.addPassthroughCopy( "src/robots.txt" );

	return {
		dir: {
			input: "src",
			output: "_site",
			includes: "_includes",
			layouts: "_includes/layouts"
		}
	};
}
