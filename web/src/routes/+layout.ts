// Pre-render all pages at build time. Required for adapter-static.
// All data fetching is client-side (onMount), so prerendering just
// produces the HTML shell — data loads in the browser as before.
export const prerender = true;
export const trailingSlash = 'always';
