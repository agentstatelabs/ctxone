/**
 * Shared writable store for the currently selected ASD repo.
 *
 * Hydrated synchronously from localStorage at module import so the first
 * render of /code already has a repo value — otherwise its $effect fires
 * with an empty string and produces a confusing "ASD server unreachable"
 * card while the layout's async `loadAsdRepos()` is still in flight.
 *
 * The layout still calls `loadAsdRepos()` on mount to reconcile against
 * the live registry (drop saved names that no longer exist, fall back to
 * the first registered repo) and to prefetch / load health.
 */
import { writable } from 'svelte/store';

const LS_KEY = 'ctxone_asd_repo';

function initial(): string {
	if (typeof localStorage === 'undefined') return '';
	try {
		return localStorage.getItem(LS_KEY) ?? '';
	} catch {
		return '';
	}
}

export const selectedRepo = writable<string>(initial());
