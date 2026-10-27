import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, waitFor } from '@testing-library/svelte';
import { checkToken, hubFetch } from './api';
import { authStore } from './authStore.svelte';
import { codeClient, listAsdRepos } from './codeApi';
import TokenEntry from './TokenEntry.svelte';

/** A fetch that answers every call with `status` and `body`. */
function answer(status: number, body = '') {
	const fetch = vi.fn(async (_url: string, _init?: RequestInit) => new Response(body, { status }));
	vi.stubGlobal('fetch', fetch);
	return fetch;
}

/** The Authorization header of the `n`th call to `fetch`. */
function authorization(fetch: ReturnType<typeof answer>, n = 0): string | null {
	return new Headers(fetch.mock.calls[n][1]?.headers).get('Authorization');
}

afterEach(() => {
	vi.unstubAllGlobals();
});

// Module-level store, so tests share one instance: each test sets the state
// it needs rather than assuming a fresh store.
describe('hubFetch and the token', () => {
	it('sends no token before one is accepted', async () => {
		authStore.forget();
		const fetch = answer(200, '{}');
		await hubFetch('/api/stats');
		expect(authorization(fetch)).toBeNull();
		expect(new Headers(fetch.mock.calls[0][1]?.headers).get('X-CTXone-Namespace')).toBeTruthy();
	});

	it('sends an accepted token as a bearer token and keeps it for the origin', async () => {
		authStore.accept('token-a', 'alice');
		const fetch = answer(200, '{}');
		await hubFetch('/api/stats');
		expect(authorization(fetch)).toBe('Bearer token-a');
		expect(authStore.needed).toBe(false);
		expect(localStorage.getItem('ctxone:token')).toBe('token-a');
		expect(localStorage.getItem('ctxone:tenant')).toBe('alice');
	});

	it('a 401 forgets the token and asks for another', async () => {
		authStore.accept('token-a', 'alice');
		answer(401);
		const resp = await hubFetch('/api/stats');
		expect(resp.status).toBe(401);
		expect(authStore.needed).toBe(true);
		expect(authStore.token).toBeNull();
		expect(authStore.tenant).toBeNull();
		expect(localStorage.getItem('ctxone:token')).toBeNull();
		expect(localStorage.getItem('ctxone:tenant')).toBeNull();
	});

	it('a 401 with no token asks for one', async () => {
		authStore.accept('token-a', 'alice');
		authStore.forget();
		answer(401);
		await hubFetch('/api/stats');
		expect(authStore.needed).toBe(true);
	});

	it('calls to the ASD proxy send the token too', async () => {
		authStore.accept('token-a', 'alice');
		const fetch = answer(200, '[]');
		await listAsdRepos();
		await codeClient('app').health();
		expect(fetch.mock.calls.map((c) => c[0])).toEqual(['/api/code', '/api/code/app/health']);
		expect(authorization(fetch, 0)).toBe('Bearer token-a');
		expect(authorization(fetch, 1)).toBe('Bearer token-a');
	});

	it('a late 401 to a call made before a token was entered changes nothing', async () => {
		authStore.accept('old-token', 'alice');
		let refuse: (r: Response) => void = () => {};
		vi.stubGlobal(
			'fetch',
			vi.fn(() => new Promise<Response>((resolve) => (refuse = resolve)))
		);
		const call = hubFetch('/api/stats');
		authStore.accept('new-token', 'alice');
		refuse(new Response('', { status: 401 }));
		await call;
		expect(authStore.token).toBe('new-token');
		expect(authStore.needed).toBe(false);
	});
});

describe('checkToken', () => {
	it("names the token's tenant without keeping the token", async () => {
		authStore.forget();
		const fetch = answer(200, '{"tenant":"alice"}');
		expect(await checkToken('token-a')).toEqual({ ok: true, tenant: 'alice' });
		expect(fetch.mock.calls[0][0]).toMatch(/\/api\/tenant$/);
		expect(authorization(fetch)).toBe('Bearer token-a');
		expect(authStore.token).toBeNull();
	});

	it('is null-tenant on a single-tenant Hub', async () => {
		answer(200, '{"tenant":null}');
		expect(await checkToken('token-a')).toEqual({ ok: true, tenant: null });
	});

	it('says why a token is refused', async () => {
		answer(401);
		expect(await checkToken('wrong')).toEqual({
			ok: false,
			error: "The Hub doesn't accept this token."
		});
		answer(503, "can't look up the token's tenant right now; try again");
		expect(await checkToken('token-a')).toEqual({
			ok: false,
			error: "can't look up the token's tenant right now; try again"
		});
		vi.stubGlobal('fetch', vi.fn(async () => Promise.reject(new TypeError('offline'))));
		expect(await checkToken('token-a')).toEqual({ ok: false, error: "Can't reach the Hub." });
	});
});

describe('TokenEntry', () => {
	it('keeps a token the Hub accepts, then reloads', async () => {
		authStore.forget();
		const fetch = answer(200, '{"tenant":"alice"}');
		const onaccepted = vi.fn();
		const { getByTestId, getByRole } = render(TokenEntry, { onaccepted });
		await fireEvent.input(getByTestId('token-input'), { target: { value: ' token-a\n' } });
		await fireEvent.submit(getByRole('button', { name: 'Continue' }).closest('form')!);
		await waitFor(() => expect(onaccepted).toHaveBeenCalledOnce());
		expect(authorization(fetch)).toBe('Bearer token-a');
		expect(authStore.token).toBe('token-a');
		expect(authStore.tenant).toBe('alice');
		expect(authStore.needed).toBe(false);
	});

	it('shows why a token is refused and keeps nothing', async () => {
		authStore.forget();
		answer(401);
		const onaccepted = vi.fn();
		const { getByTestId, getByRole } = render(TokenEntry, { onaccepted });
		await fireEvent.input(getByTestId('token-input'), { target: { value: 'wrong' } });
		await fireEvent.submit(getByRole('button', { name: 'Continue' }).closest('form')!);
		await waitFor(() =>
			expect(getByTestId('token-error').textContent).toBe("The Hub doesn't accept this token.")
		);
		expect(onaccepted).not.toHaveBeenCalled();
		expect(authStore.token).toBeNull();
	});
});
