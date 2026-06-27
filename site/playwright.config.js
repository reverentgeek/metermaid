import { defineConfig, devices } from "@playwright/test";

export default defineConfig( {
	testDir: "tests",
	fullyParallel: true,
	forbidOnly: !!process.env.CI,
	reporter: "list",

	use: {
		baseURL: "http://localhost:3001",
	},

	projects: [
		{
			name: "chromium",
			use: { ...devices[ "Desktop Chrome" ] },
		},
	],

	webServer: {
		command: "node scripts/serve-site.mjs",
		url: "http://localhost:3001",
		reuseExistingServer: false,
	},
} );
