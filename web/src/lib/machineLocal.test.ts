import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { refreshTenant } from './api';
import { authStore } from './authStore.svelte';
import { isCodeRoute } from './codeApi';
import MachineLocal from './MachineLocal.svelte';

/** A fetch that answers every call with `status` and `body`. */
function answer(status: number, body = '') {
	const fetch = vi.fn(async (_url: string, _init?: RequestInit) => new Response(body, { status }));
	vi.stubGlobal('fetch', fetch);
	return fetch;
}

/** A snippet rendering one element with `testid`. */
function snippet(testid: string) {
	return createRawSnippet(() => ({ render: () => `<button data-testid="${testid}">x</button>` }));
}

afterEach(() => {
	vi.unstubAllGlobals();
});

// Module-level store, so tests share one instance: each test sets the state
// it needs rather than assuming a fresh store.
describe('which kind of Hub Lens is on', () => {
	it('is multi-tenant once the Hub names a tenant, and remembers it', () => {
		authStore.named(null);
		expect(authStore.multiTenant).toBe(false);
		authStore.named('alice');
		expect(authStore.multiTenant).toBe(true);
		expect(localStorage.getItem('ctxone:tenant')).toBe('alice');
		authStore.named(null);
		expect(authStore.multiTenant).toBe(false);
		expect(localStorage.getItem('ctxone:tenant')).toBeNull();
	});

	it('refreshTenant asks the Hub with the token and believes its answer', async () => {
		authStore.accept('token-a', null);
		const fetch = answer(200, '{"tenant":"alice"}');
		await refreshTenant();
		expect(fetch.mock.calls[0][0]).toMatch(/\/api\/tenant$/);
		expect(new Headers(fetch.mock.calls[0][1]?.headers).get('Authorization')).toBe(
			'Bearer token-a'
		);
		expect(authStore.multiTenant).toBe(true);

		answer(200, '{"tenant":null}');
		await refreshTenant();
		expect(authStore.multiTenant).toBe(false);
	});

	it('refreshTenant keeps what Lens knew when the Hub gives no answer', async () => {
		authStore.accept('token-a', 'alice');
		answer(404, '{"error":"not found","path":"/api/tenant"}');
		await refreshTenant();
		expect(authStore.multiTenant).toBe(true);

		vi.stubGlobal('fetch', vi.fn(async () => Promise.reject(new TypeError('offline'))));
		await refreshTenant();
		expect(authStore.multiTenant).toBe(true);
	});

	it('refreshTenant refused sends Lens to token entry', async () => {
		authStore.accept('token-a', 'alice');
		answer(401);
		await refreshTenant();
		expect(authStore.needed).toBe(true);
		expect(authStore.token).toBeNull();
	});
});

describe('isCodeRoute', () => {
	it('matches the Code views and nothing else', () => {
		expect(isCodeRoute('/code')).toBe(true);
		expect(isCodeRoute('/code/search')).toBe(true);
		expect(isCodeRoute('/code/symbols/a%3A%3Ab')).toBe(true);
		expect(isCodeRoute('/codex')).toBe(false);
		expect(isCodeRoute('/sessions')).toBe(false);
		expect(isCodeRoute('/')).toBe(false);
	});
});

describe('MachineLocal', () => {
	it('shows its children on a Hub that serves one tenant', () => {
		authStore.named(null);
		const { queryByTestId } = render(MachineLocal, {
			children: snippet('import'),
			otherwise: snippet('note')
		});
		expect(queryByTestId('import')).not.toBeNull();
		expect(queryByTestId('note')).toBeNull();
	});

	it('hides them on a multi-tenant Hub, showing `otherwise` if given', () => {
		authStore.named('alice');
		const withNote = render(MachineLocal, {
			children: snippet('import'),
			otherwise: snippet('note')
		});
		expect(withNote.queryByTestId('import')).toBeNull();
		expect(withNote.queryByTestId('note')).not.toBeNull();
		withNote.unmount();

		const bare = render(MachineLocal, { children: snippet('import') });
		expect(bare.queryByTestId('import')).toBeNull();
		expect(bare.container.textContent).toBe('');
	});
});
