/**
 * Theme picker. Persists to localStorage and reflects to
 * `document.documentElement.dataset.theme` so CSS can use
 * `:root[data-theme="..."]` selectors. Tokens themselves live in
 * `app.css` — this file just owns the active selection.
 */

export const THEMES = [
	{ id: 'tw-dark', label: 'TW Dark' },
	{ id: 'pure-black', label: 'Pure Black' },
	{ id: 'slate', label: 'Slate' }
] as const;

export type ThemeId = (typeof THEMES)[number]['id'];

const KEY = 'ctxone:theme';
const DEFAULT: ThemeId = 'tw-dark';

function load(): ThemeId {
	if (typeof localStorage === 'undefined') return DEFAULT;
	const v = localStorage.getItem(KEY);
	return THEMES.some((t) => t.id === v) ? (v as ThemeId) : DEFAULT;
}

function apply(id: ThemeId) {
	if (typeof document === 'undefined') return;
	document.documentElement.dataset.theme = id;
}

class ThemeStore {
	current: ThemeId = $state(DEFAULT);

	constructor() {
		this.current = load();
	}

	set(id: ThemeId) {
		this.current = id;
		if (typeof localStorage !== 'undefined') localStorage.setItem(KEY, id);
		apply(id);
	}

	hydrate() {
		apply(this.current);
	}
}

export const themeStore = new ThemeStore();
