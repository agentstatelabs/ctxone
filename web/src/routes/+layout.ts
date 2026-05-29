// Lens is a pure client-side SPA. It fetches all data from the same-origin
// /api at runtime; the Hub (axum) serves index.html for any non-/api route
// (adapter-static `fallback: index.html`). We do NOT prerender or SSR —
// dynamic pages (e.g. /code/graph reads url.searchParams) can't be prerendered.
export const prerender = false;
export const ssr = false;
export const trailingSlash = 'always';
