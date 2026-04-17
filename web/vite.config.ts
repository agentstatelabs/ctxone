import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Vitest-specific config (test block) lives in `vitest.config.ts` to
// avoid type conflicts between vite's UserConfig and vitest's extended
// one. Vitest picks up that file automatically when running `vitest`.
export default defineConfig({
	plugins: [sveltekit()]
});
