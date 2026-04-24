// adapter-static: builds Lens as pure static HTML/JS/CSS with no Node.js
// runtime. The Hub embeds these files at compile time (via rust-embed) and
// serves them from the same port when started with --lens. For local dev,
// `npm run dev` still works — the adapter only matters at build time.
import adapter from '@sveltejs/adapter-static';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	compilerOptions: {
		runes: ({ filename }) => (filename.split(/[/\\]/).includes('node_modules') ? undefined : true)
	},
	kit: {
		adapter: adapter({
			// Serve index.html for unknown routes (SPA fallback — axum
			// mirrors this by serving index.html for non-/api/ paths).
			fallback: 'index.html'
		})
	}
};

export default config;
