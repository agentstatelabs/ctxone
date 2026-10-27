/**
 * The bearer token Lens sends with every Hub call (see hubFetch in api.ts),
 * kept in localStorage, so only pages from the Hub's own origin can read it.
 *
 * A Hub on this machine never asks for one: loopback needs no token. A
 * multi-tenant Hub, or a token-guarded Hub reached from another machine,
 * answers 401, and Lens shows token entry (TokenEntry.svelte) until the Hub
 * accepts a token. A 401 to a call that sent the stored token forgets it, so
 * a rotated or disabled token goes back to token entry too.
 *
 * localStorage is readable by any script on the Hub's origin, which serves
 * nothing but Lens; a real sign-in comes with an account service.
 */

const TOKEN_KEY = 'ctxone:token';
const TENANT_KEY = 'ctxone:tenant';

function load(key: string): string | null {
	if (typeof localStorage === 'undefined') return null;
	return localStorage.getItem(key);
}

function save(key: string, value: string | null) {
	if (typeof localStorage === 'undefined') return;
	if (value === null) localStorage.removeItem(key);
	else localStorage.setItem(key, value);
}

function createAuthStore() {
	let token = $state(load(TOKEN_KEY));
	let tenant = $state(load(TENANT_KEY));
	let needed = $state(false);

	function forget() {
		token = null;
		tenant = null;
		save(TOKEN_KEY, null);
		save(TENANT_KEY, null);
	}

	return {
		/** The token to send, or null to send none. */
		get token() {
			return token;
		},
		/** The tenant the token belongs to; null on a single-tenant Hub. */
		get tenant() {
			return tenant;
		},
		/** True once the Hub has refused a call: Lens shows token entry. */
		get needed() {
			return needed;
		},
		/** Keep a token the Hub accepted, with the tenant it named. */
		accept(newToken: string, newTenant: string | null) {
			token = newToken;
			tenant = newTenant;
			needed = false;
			save(TOKEN_KEY, newToken);
			save(TENANT_KEY, newTenant);
		},
		/** The Hub answered 401 to a call that sent `sent` (null for none):
		 * forget that token and ask for another. A late answer to a call
		 * made before the current token was entered changes nothing. */
		refused(sent: string | null) {
			if (sent !== token) return;
			forget();
			needed = true;
		},
		/** Forget the stored token, as when signing out. */
		forget
	};
}

export const authStore = createAuthStore();
