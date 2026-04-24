// Team data is dynamic — disable prerendering so the shell is served as-is
// and all data fetching happens client-side via onMount (same as every other page),
// but we don't emit a static HTML snapshot that would go stale.
export const prerender = false;
export const trailingSlash = 'always';
