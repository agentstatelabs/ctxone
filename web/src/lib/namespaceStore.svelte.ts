/**
 * Global current-namespace (project) store. Mirrors selection to
 * localStorage so a page refresh lands on the same project. Every Hub
 * request carries the selection via the `X-CTXone-Namespace` header
 * (see hubFetch in api.ts).
 *
 * Branches are per-namespace, so switching namespaces resets the
 * branch store to `main` — a stale branch from another namespace
 * would 404.
 */

import { branchStore } from './branchStore.svelte';

const KEY = 'ctxone:namespace';
export const DEFAULT_NAMESPACE = 'default';

function load(): string {
	if (typeof localStorage === 'undefined') return DEFAULT_NAMESPACE;
	return localStorage.getItem(KEY) ?? DEFAULT_NAMESPACE;
}

function save(name: string) {
	if (typeof localStorage !== 'undefined') localStorage.setItem(KEY, name);
}

function createNamespaceStore() {
	let current = $state(load());

	return {
		get current() {
			return current;
		},
		set current(value: string) {
			if (value === current) return;
			current = value;
			save(value);
			// Branch refs are namespace-scoped; fall back to the one
			// branch guaranteed to exist everywhere.
			branchStore.current = 'main';
		},
		/** Reconcile the persisted namespace against the known list —
		 * if its project was deleted, fall back to default. */
		hydrate(known: string[]) {
			if (!known.includes(current) && current !== DEFAULT_NAMESPACE) {
				this.current = DEFAULT_NAMESPACE;
			}
		}
	};
}

export const namespaceStore = createNamespaceStore();
