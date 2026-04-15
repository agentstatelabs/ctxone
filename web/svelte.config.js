// Use adapter-node so Lens builds into a standalone Node.js server.
// This is what the Dockerfile serves, and also what you'd run behind
// a reverse proxy in production. For local dev `npm run dev` still
// works — the adapter only matters at build time.
import adapter from '@sveltejs/adapter-node';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	compilerOptions: {
		// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
		runes: ({ filename }) => (filename.split(/[/\\]/).includes('node_modules') ? undefined : true)
	},
	kit: {
		adapter: adapter()
	}
};

export default config;
