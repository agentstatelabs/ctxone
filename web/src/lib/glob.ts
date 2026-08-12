/**
 * Build a case-insensitive text matcher from a user query.
 *
 * - A query with **no** wildcard is a plain substring match (unchanged
 *   behavior): `budget` matches anywhere in the text.
 * - A query containing `*` or `?` is treated as an **anchored glob**:
 *   `*` matches any run of characters, `?` matches exactly one. So
 *   `asd-m5*` matches names starting with `asd-m5`, `*budget*` matches
 *   anywhere, `t-00?` matches `t-001`…`t-009`.
 *
 * Anchoring the glob (but not the substring) keeps both intuitions: bare
 * terms are forgiving, wildcards are precise.
 */
export function makeMatcher(query: string): (text: string) => boolean {
	const q = query.trim().toLowerCase();
	if (!q) return () => true;

	if (!/[*?]/.test(q)) {
		return (text) => text.toLowerCase().includes(q);
	}

	// Glob → anchored regex: escape regex metacharacters, then expand the
	// wildcards. `*` and `?` are handled separately so they survive escaping.
	const pattern = q
		.replace(/[.+^${}()|[\]\\]/g, '\\$&')
		.replace(/\*/g, '.*')
		.replace(/\?/g, '.');
	let rx: RegExp;
	try {
		rx = new RegExp(`^${pattern}$`);
	} catch {
		// Should never happen (we escaped everything), but never throw at a
		// keystroke: fall back to substring on the raw query.
		return (text) => text.toLowerCase().includes(q);
	}
	return (text) => rx.test(text.toLowerCase());
}

/** True when the query contains glob wildcards (for UI hints). */
export function hasWildcard(query: string): boolean {
	return /[*?]/.test(query);
}
