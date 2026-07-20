import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

/*
	`svelte()`, NOT `sveltekit()`. The kit plugin expects a dev/build server it
	never gets under vitest, and on vite 8 it crashed the runner at startup
	("Cannot convert undefined or null to object" in hot-update.js) — which
	made the entire web unit suite unrunnable, not just the component tests.

	`conditions: ['browser']` matters just as much: svelte ships separate
	client and server builds, and the server one throws
	`lifecycle_function_unavailable` as soon as a test calls `mount()`. The
	browser condition plus the jsdom environment gets component tests the
	client build and a DOM to mount into.
*/
export default defineConfig({
	plugins: [svelte()],
	resolve: {
		conditions: ['browser']
	},
	test: {
		environment: 'jsdom',
		globals: true,
		include: ['src/**/*.{test,spec}.{ts,js}']
	}
});
