import { describe, it, expect } from 'vitest';
import { makeMatcher, hasWildcard } from './glob';

describe('makeMatcher', () => {
	it('empty query matches everything', () => {
		const m = makeMatcher('   ');
		expect(m('anything')).toBe(true);
	});

	it('bare term is a case-insensitive substring match', () => {
		const m = makeMatcher('Budget');
		expect(m('probe budget calc')).toBe(true);
		expect(m('BUDGETING')).toBe(true);
		expect(m('unrelated')).toBe(false);
	});

	it('* matches any run (anchored glob)', () => {
		const m = makeMatcher('asd-m5*');
		expect(m('asd-m52-classification')).toBe(true);
		expect(m('asd-m5')).toBe(true);
		expect(m('asd-m4-foo')).toBe(false);
		// anchored: a leading prefix must match from the start
		expect(m('xx-asd-m52')).toBe(false);
	});

	it('*x* matches anywhere', () => {
		const m = makeMatcher('*release*');
		expect(m('public-release-reorg')).toBe(true);
		expect(m('set up a release pipeline')).toBe(true);
		expect(m('nothing here')).toBe(false);
	});

	it('? matches exactly one character', () => {
		const m = makeMatcher('t-00?');
		expect(m('t-001')).toBe(true);
		expect(m('t-009')).toBe(true);
		expect(m('t-0012')).toBe(false);
		expect(m('t-00')).toBe(false);
	});

	it('regex metacharacters in the query are treated literally', () => {
		const m = makeMatcher('a.b(c)*');
		expect(m('a.b(c)-tail')).toBe(true);
		expect(m('aXbYcZ')).toBe(false); // '.' and '()' are literal, not regex
	});

	it('hasWildcard detects glob chars', () => {
		expect(hasWildcard('asd-m5*')).toBe(true);
		expect(hasWildcard('t-00?')).toBe(true);
		expect(hasWildcard('plain')).toBe(false);
	});
});
