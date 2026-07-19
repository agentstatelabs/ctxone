/**
 * Global auto-refresh control for Lens pages.
 *
 * Each data-bearing page (Plans, Sessions, Browse, Pinned, History,
 * Branches, Taint, Diff) calls `useAutoRefresh(refresh)` from its
 * onMount; the helper sets up a `setInterval` poll, pauses on
 * `document.visibilitychange` when the tab goes hidden, and exposes
 * a `lastRefreshed` timestamp the page can render.
 *
 * The user-facing on/off toggle lives in the layout sidebar and
 * persists to localStorage so the choice sticks across reloads. When
 * disabled, mounted pages stop polling but stay subscribed — flipping
 * the toggle back on resumes immediately.
 */

import { onMount } from 'svelte';

const KEY = 'ctxone:autorefresh';
/** Polling cadence, ms. Was 15s (t-021 spec default); widened to 30s to
 * cut background load now that refreshes update data in place. */
export const REFRESH_INTERVAL_MS = 30_000;

function loadEnabled(): boolean {
	if (typeof localStorage === 'undefined') return true;
	const v = localStorage.getItem(KEY);
	// Default ON. Only an explicit "0" disables.
	return v !== '0';
}

function createRefreshStore() {
	let enabled = $state(loadEnabled());

	return {
		get enabled() {
			return enabled;
		},
		set enabled(v: boolean) {
			enabled = v;
			if (typeof localStorage !== 'undefined') {
				localStorage.setItem(KEY, v ? '1' : '0');
			}
		},
		toggle() {
			this.enabled = !enabled;
		},
		intervalMs: REFRESH_INTERVAL_MS
	};
}

export const refreshStore = createRefreshStore();

/**
 * Hook a page's `refresh` function into the global polling loop.
 *
 * Contract:
 * - Caller is expected to do its own initial load on mount; this hook
 *   does **not** fire `refresh` immediately, so there's no double-fetch.
 * - The hook returns reactive `{ lastRefreshed, refreshing }` getters
 *   the caller can render in a "refreshed Xs ago" indicator.
 * - Polling pauses while `document.hidden` is true and resumes on
 *   visibility change.
 * - Polling is gated on `refreshStore.enabled`; flipping the global
 *   toggle takes effect on the next interval tick (no restart needed).
 *
 * Must be called from a component setup (uses `onMount` for cleanup).
 */
export function useAutoRefresh(refresh: () => void | Promise<void>) {
	let lastRefreshed: Date | null = $state(null);
	let refreshing = $state(false);

	async function tick() {
		if (refreshing) return; // skip if a previous tick is still in flight
		if (!refreshStore.enabled) return;
		if (typeof document !== 'undefined' && document.hidden) return;
		refreshing = true;
		try {
			await refresh();
			lastRefreshed = new Date();
		} finally {
			refreshing = false;
		}
	}

	onMount(() => {
		const id = setInterval(tick, refreshStore.intervalMs);
		// On visibility *gain*, do an immediate refresh so a user
		// returning to the tab sees fresh data right away rather than
		// waiting up to 30s for the next interval.
		const onVisibility = () => {
			if (typeof document !== 'undefined' && !document.hidden) {
				void tick();
			}
		};
		if (typeof document !== 'undefined') {
			document.addEventListener('visibilitychange', onVisibility);
		}
		return () => {
			clearInterval(id);
			if (typeof document !== 'undefined') {
				document.removeEventListener('visibilitychange', onVisibility);
			}
		};
	});

	return {
		get lastRefreshed() {
			return lastRefreshed;
		},
		get refreshing() {
			return refreshing;
		}
	};
}

/**
 * Format a "X seconds ago" string for the per-page indicator.
 * Returns "—" for null / never.
 */
export function formatAgo(ts: Date | null, now: Date = new Date()): string {
	if (!ts) return '—';
	const ms = now.getTime() - ts.getTime();
	if (ms < 0) return 'just now';
	const sec = Math.round(ms / 1000);
	if (sec < 5) return 'just now';
	if (sec < 60) return `${sec}s ago`;
	const min = Math.round(sec / 60);
	if (min < 60) return `${min}m ago`;
	const hr = Math.round(min / 60);
	return `${hr}h ago`;
}
