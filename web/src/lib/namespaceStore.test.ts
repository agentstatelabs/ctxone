import { describe, it, expect } from 'vitest';
import { namespaceStore, DEFAULT_NAMESPACE } from './namespaceStore.svelte';
import { branchStore } from './branchStore.svelte';

// Module-level store, so tests share one instance — each test sets the
// state it needs rather than assuming a fresh store.
describe('namespaceStore', () => {
	it('starts on the default namespace', () => {
		expect(namespaceStore.current).toBe(DEFAULT_NAMESPACE);
	});

	it('persists the selection to localStorage', () => {
		namespaceStore.current = 'exampleproj';
		expect(namespaceStore.current).toBe('exampleproj');
		expect(localStorage.getItem('ctxone:namespace')).toBe('exampleproj');
	});

	it('resets the branch to main when the namespace changes', () => {
		namespaceStore.current = 'ns-a';
		branchStore.current = 'feature-x';
		namespaceStore.current = 'ns-b';
		expect(branchStore.current).toBe('main');
	});

	it('leaves the branch alone when re-setting the same namespace', () => {
		namespaceStore.current = 'ns-same';
		branchStore.current = 'feature-y';
		namespaceStore.current = 'ns-same';
		expect(branchStore.current).toBe('feature-y');
	});

	it('hydrate falls back to default when the namespace is unknown', () => {
		namespaceStore.current = 'deleted-project';
		namespaceStore.hydrate([DEFAULT_NAMESPACE, 'other']);
		expect(namespaceStore.current).toBe(DEFAULT_NAMESPACE);
		expect(localStorage.getItem('ctxone:namespace')).toBe(DEFAULT_NAMESPACE);
	});

	it('hydrate keeps a known namespace', () => {
		namespaceStore.current = 'known';
		namespaceStore.hydrate([DEFAULT_NAMESPACE, 'known']);
		expect(namespaceStore.current).toBe('known');
	});
});
