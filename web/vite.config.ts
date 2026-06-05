import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Vitest-specific config (test block) lives in `vitest.config.ts` to
// avoid type conflicts between vite's UserConfig and vitest's extended
// one. Vitest picks up that file automatically when running `vitest`.
// Proxy `/api/*` to a locally-running ctxone-hub so the dev server can hit
// real endpoints without CORS or a separate fetch base. Override the target
// with VITE_HUB_URL=http://localhost:<port> when the hub isn't on 3001.
// (`process` is the Node global Vite runs under — svelte-check doesn't have
// `@types/node` in scope, so cast through globalThis to keep it happy.)
const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {};
const HUB = env.VITE_HUB_URL ?? 'http://localhost:3001';

export default defineConfig({
	plugins: [sveltekit()],
	server: {
		proxy: {
			'/api': { target: HUB, changeOrigin: true }
		}
	}
});
