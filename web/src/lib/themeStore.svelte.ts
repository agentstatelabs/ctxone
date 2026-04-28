/**
 * Theme picker. Persists to localStorage and reflects to
 * `document.documentElement.dataset.theme` so CSS can use
 * `:root[data-theme="..."]` selectors. Tokens themselves live in
 * `app.css` — this file just owns the active selection.
 */

/**
 * Each theme declares the same token contract in app.css; this list
 * just drives the picker. `group` is what the layout dropdown uses
 * to split dark from light so the user can scan their half quickly.
 */
export const THEMES = [
	// Dark
	{ id: 'tw-dark', label: 'TW Dark', group: 'dark' },
	{ id: 'pure-black', label: 'Pure Black', group: 'dark' },
	{ id: 'slate', label: 'Slate', group: 'dark' },
	{ id: 'solarized-dark', label: 'Solarized Dark', group: 'dark' },
	{ id: 'nord', label: 'Nord', group: 'dark' },
	{ id: 'dracula', label: 'Dracula', group: 'dark' },
	{ id: 'gruvbox-dark', label: 'Gruvbox Dark', group: 'dark' },
	// Light
	{ id: 'tw-light', label: 'TW Light', group: 'light' },
	{ id: 'solarized-light', label: 'Solarized Light', group: 'light' },
	{ id: 'gruvbox-light', label: 'Gruvbox Light', group: 'light' },
	{ id: 'paper', label: 'Paper', group: 'light' },
	{ id: 'high-contrast-light', label: 'High Contrast Light', group: 'light' }
] as const;

export type ThemeId = (typeof THEMES)[number]['id'];
export type ThemeGroup = (typeof THEMES)[number]['group'];

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
