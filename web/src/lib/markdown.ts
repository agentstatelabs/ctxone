// Markdown → sanitized HTML for transcript turns.
//
// Claude's turn text is Markdown (tables, ```fences```, JSON/XML blocks,
// lists). We render it with `marked` (GFM: tables, fenced code, task lists)
// and sanitize the result with DOMPurify before it ever reaches {@html}.
// The data is the user's own transcript, but we sanitize anyway — defense in
// depth costs nothing here and the turn text is ultimately agent-authored.
//
// Lens is a pure client-side SPA (`ssr = false`), so DOMPurify's default
// browser export always has a real `window` to work against.

import { Marked, type Tokens } from 'marked';
import DOMPurify from 'dompurify';

function escapeHtml(s: string): string {
	return s
		.replace(/&/g, '&amp;')
		.replace(/</g, '&lt;')
		.replace(/>/g, '&gt;')
		.replace(/"/g, '&quot;');
}

// Dedicated instance so we don't mutate the global `marked` singleton.
const md = new Marked({ gfm: true, breaks: false });

// Fenced code blocks: keep marked's <pre><code> but wrap it so we can show a
// small language-name label (from the fence info string) and give the block
// its own horizontal-scroll surface. We escape the code ourselves — the token
// text is raw.
md.use({
	renderer: {
		code(token: Tokens.Code): string {
			const lang = (token.lang ?? '').trim().split(/\s+/)[0];
			const body = `<pre><code>${escapeHtml(token.text)}</code></pre>`;
			const label = lang ? `<div class="md-code-lang">${escapeHtml(lang)}</div>` : '';
			return `<div class="md-code">${label}${body}</div>`;
		}
	}
});

// Open links in a new tab, safely. Registered once on the singleton; this
// module is the only DOMPurify consumer in the app.
DOMPurify.addHook('afterSanitizeAttributes', (node) => {
	if (node.tagName === 'A' && node.hasAttribute('href')) {
		node.setAttribute('target', '_blank');
		node.setAttribute('rel', 'noopener noreferrer nofollow');
	}
});

/**
 * Render Markdown to sanitized HTML. Wide tables are wrapped in a
 * horizontal-scroll container so they never blow out the panel width.
 */
export function renderMarkdown(src: string | undefined | null): string {
	if (!src || !src.trim()) return '';
	let html = md.parse(src, { async: false });
	// marked emits bare <table>…</table>; give each one a scroll wrapper.
	html = html
		.replace(/<table>/g, '<div class="md-table-wrap"><table>')
		.replace(/<\/table>/g, '</table></div>');
	return DOMPurify.sanitize(html);
}
