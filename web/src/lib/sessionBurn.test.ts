import { describe, it, expect } from 'vitest';
import {
	computeBurn,
	toolName,
	WINDOW,
	MIN_TURNS,
	T_BURNING,
	T_DIMINISHING,
	type BurnTurn
} from './sessionBurn';

/**
 * Build a turn with `edits` mutating calls, `reads` read calls, and a given
 * per-call context size. Context is stored pre-multiplied by call count
 * because computeBurn divides it back out.
 */
function turn(edits: number, reads: number, perCallContext: number): BurnTurn {
	const calls = [
		...Array.from({ length: edits }, (_, i) => `Edit: /src/f${i}.rs`),
		...Array.from({ length: reads }, (_, i) => `Read: /src/r${i}.rs`)
	];
	const n = Math.max(1, calls.length);
	return { tool_calls: calls, tokens: { cache_read: perCallContext * n } };
}

const steady = (n: number) => Array.from({ length: n }, () => turn(3, 3, 100_000));

describe('toolName', () => {
	it('parses the stored "Name: arg" form and bare names', () => {
		expect(toolName('Read: /src/main.rs')).toBe('Read');
		expect(toolName('Bash: ls -la /tmp')).toBe('Bash');
		expect(toolName('ToolSearch')).toBe('ToolSearch');
	});
});

describe('computeBurn — guards', () => {
	it('reports unknown below the turn floor rather than guessing', () => {
		const r = computeBurn(steady(MIN_TURNS - 1));
		expect(r.level).toBe('unknown');
		expect(r.ratio).toBeNull();
	});

	it('reports unknown when the baseline is too edit-sparse to divide by', () => {
		// The exploration-first shape that produced a nonsense 0.21x in the
		// field sample: heavy reading, no edits, then edits arrive late.
		const turns = [
			...Array.from({ length: 20 }, () => turn(0, 6, 100_000)),
			...Array.from({ length: 10 }, () => turn(4, 2, 100_000))
		];
		const r = computeBurn(turns);
		expect(r.level).toBe('unknown');
		expect(r.headline).toMatch(/baseline/i);
	});
});

describe('computeBurn — direction (must not invert)', () => {
	/**
	 * The axis this metric is most likely to get flipped on. `productive` is
	 * the GOOD bucket and must sit at the LOW end of the ratio; `burning` is
	 * the BAD bucket at the HIGH end. See CLAUDE.md — the ASD calibration
	 * table shipped inverted for nine releases because every test encoded the
	 * same wrong assumption. This test encodes the direction independently:
	 * it asserts against cost, not against the implementation's own labels.
	 */
	it('higher cost-per-edit is worse, not better', () => {
		const cheap = computeBurn(steady(40));
		// Same edits, 10x the context per call = strictly worse.
		const expensive = computeBurn([
			...steady(20),
			...Array.from({ length: 20 }, () => turn(3, 3, 1_000_000))
		]);

		expect(expensive.ratio!).toBeGreaterThan(cheap.ratio!);
		expect(cheap.level).toBe('productive');
		expect(expensive.level).toBe('burning');

		const rank = { productive: 0, diminishing: 1, burning: 2, unknown: -1 };
		expect(rank[expensive.level]).toBeGreaterThan(rank[cheap.level]);
	});

	it('orders the ladder thresholds low=good to high=bad', () => {
		expect(T_DIMINISHING).toBeLessThan(T_BURNING);
	});
});

describe('computeBurn — levels', () => {
	it('a steady session stays productive', () => {
		const r = computeBurn(steady(40));
		expect(r.level).toBe('productive');
		expect(r.ratio!).toBeLessThan(T_DIMINISHING);
	});

	it('reads the typical late-session shape as diminishing, not burning', () => {
		// 2x context for half the edits => ~4x cost per edit. This is the
		// MEDIAN degradation of a real session's last third, so it must not
		// trip the top-20% alarm — that was the miscalibration the rolling
		// window distribution exposed.
		const turns = [
			...Array.from({ length: 20 }, () => turn(4, 4, 100_000)),
			...Array.from({ length: 20 }, () => turn(2, 6, 200_000))
		];
		const r = computeBurn(turns);
		expect(r.ratio!).toBeGreaterThanOrEqual(T_DIMINISHING);
		expect(r.ratio!).toBeLessThan(T_BURNING);
		expect(r.level).toBe('diminishing');
	});

	it('reads a severe collapse as burning', () => {
		// 4x context for a quarter of the edits => ~16x, comfortably past p85.
		const turns = [
			...Array.from({ length: 20 }, () => turn(4, 4, 100_000)),
			...Array.from({ length: 20 }, () => turn(1, 7, 400_000))
		];
		const r = computeBurn(turns);
		expect(r.ratio!).toBeGreaterThanOrEqual(T_BURNING);
		expect(r.level).toBe('burning');
		expect(r.detail).toMatch(/new session/i);
	});

	it('treats a zero-edit trailing window as burning and says so', () => {
		const turns = [
			...Array.from({ length: 20 }, () => turn(4, 2, 100_000)),
			...Array.from({ length: WINDOW }, () => turn(0, 8, 300_000))
		];
		const r = computeBurn(turns);
		expect(r.level).toBe('burning');
		expect(r.detail).toMatch(/no edits/i);
	});

	it('keeps a mild slowdown on the productive side of the ladder', () => {
		// ~2.7x — below the rolling-window median, so not worth flagging.
		const turns = [
			...Array.from({ length: 20 }, () => turn(4, 4, 100_000)),
			...Array.from({ length: 20 }, () => turn(3, 5, 200_000))
		];
		const r = computeBurn(turns);
		expect(r.ratio!).toBeLessThan(T_DIMINISHING);
		expect(r.level).toBe('productive');
	});
});

describe('computeBurn — knee', () => {
	it('locates a sustained crossover and ignores a lone spike', () => {
		const good = () => turn(4, 2, 100_000);
		const bad = () => turn(1, 8, 800_000);

		const spike = computeBurn([...Array.from({ length: 30 }, good), bad(), ...Array.from({ length: 10 }, good)]);
		expect(spike.knee).toBeNull();

		const shelf = computeBurn([...Array.from({ length: 20 }, good), ...Array.from({ length: 20 }, bad)]);
		expect(shelf.knee).not.toBeNull();
		expect(shelf.knee!).toBeGreaterThanOrEqual(20 - WINDOW);
		expect(shelf.sinceKneeTokens).toBeGreaterThan(0);
	});

	it('emits one series point per turn', () => {
		const r = computeBurn(steady(40));
		expect(r.series).toHaveLength(40);
	});
});
