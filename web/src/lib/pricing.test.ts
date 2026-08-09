import { describe, expect, it } from 'vitest';
import {
	PRICING,
	estimateCost,
	estimateCostForPlan,
	estimateWithoutCtxone,
	formatSavingsPercent,
	formatUsd,
	pricingFor,
	savingsPercent,
	type PlanCostSession
} from './pricing';

describe('pricingFor', () => {
	it('returns pricing for known models', () => {
		const p = pricingFor('claude-sonnet-4.5');
		expect(p).not.toBeNull();
		expect(p!.provider).toBe('anthropic');
		expect(p!.input).toBeGreaterThan(0);
		expect(p!.output).toBeGreaterThan(p!.input);
	});

	it('returns null for unknown models', () => {
		expect(pricingFor('gpt-9000')).toBeNull();
		expect(pricingFor('')).toBeNull();
		expect(pricingFor(null)).toBeNull();
		expect(pricingFor(undefined)).toBeNull();
	});

	it('covers the v1 list of providers', () => {
		// Spec §8: Anthropic Sonnet/Haiku/Opus + OpenAI 4o/mini + Gemini.
		// Keep this list in sync with PRICING additions.
		expect(PRICING['claude-sonnet-4.5']).toBeDefined();
		expect(PRICING['claude-haiku-4.5']).toBeDefined();
		expect(PRICING['claude-opus-4.5']).toBeDefined();
		expect(PRICING['gpt-4o']).toBeDefined();
		expect(PRICING['gpt-4o-mini']).toBeDefined();
		expect(PRICING['gemini-2.5-pro']).toBeDefined();
		expect(PRICING['gemini-2.5-flash']).toBeDefined();
	});
});

describe('estimateCost', () => {
	it('computes cost using the right multiplier', () => {
		// 1M input + 1M output on claude-sonnet-4.5 = $3 + $15 = $18
		const cost = estimateCost('claude-sonnet-4.5', {
			input: 1_000_000,
			output: 1_000_000,
			cache_read: 0,
			cache_create: 0
		});
		expect(cost).toBeCloseTo(18, 5);
	});

	it('charges cache reads at the discounted rate', () => {
		// claude-sonnet-4.5: cache_read = 0.3/M, input = 3/M → 10× cheaper
		const cached = estimateCost('claude-sonnet-4.5', {
			input: 0,
			output: 0,
			cache_read: 1_000_000,
			cache_create: 0
		});
		const plain = estimateCost('claude-sonnet-4.5', {
			input: 1_000_000,
			output: 0,
			cache_read: 0,
			cache_create: 0
		});
		expect(cached).toBeLessThan(plain!);
		expect(cached! * 10).toBeCloseTo(plain!, 3);
	});

	it('charges cache writes at a premium (Anthropic)', () => {
		// cache_write > input on Anthropic sonnet
		const written = estimateCost('claude-sonnet-4.5', {
			input: 0,
			output: 0,
			cache_read: 0,
			cache_create: 1_000_000
		});
		const plain = estimateCost('claude-sonnet-4.5', {
			input: 1_000_000,
			output: 0,
			cache_read: 0,
			cache_create: 0
		});
		expect(written!).toBeGreaterThan(plain!);
	});

	it('returns null for unknown models', () => {
		const cost = estimateCost('gpt-9000', {
			input: 1000,
			output: 500,
			cache_read: 0,
			cache_create: 0
		});
		expect(cost).toBeNull();
	});

	it('returns null for null/undefined model', () => {
		expect(
			estimateCost(null, { input: 100, output: 50, cache_read: 0, cache_create: 0 })
		).toBeNull();
		expect(
			estimateCost(undefined, { input: 100, output: 50, cache_read: 0, cache_create: 0 })
		).toBeNull();
	});

	it('handles zero tokens without error', () => {
		const cost = estimateCost('claude-sonnet-4.5', {
			input: 0,
			output: 0,
			cache_read: 0,
			cache_create: 0
		});
		expect(cost).toBe(0);
	});
});

describe('estimateWithoutCtxone', () => {
	it('scales input by the cumulative ratio', () => {
		const tokens = { input: 1000, output: 500, cache_read: 0, cache_create: 0 };
		const withCtxone = estimateCost('claude-sonnet-4.5', tokens)!;
		const without = estimateWithoutCtxone('claude-sonnet-4.5', tokens, 5)!;

		// Without-CTXone cost should reflect 5× scaled input
		// but unchanged output.
		const expectedInputCost = (5000 * 3) / 1_000_000;
		const expectedOutputCost = (500 * 15) / 1_000_000;
		expect(without).toBeCloseTo(expectedInputCost + expectedOutputCost, 6);
		expect(without).toBeGreaterThan(withCtxone);
	});

	it('falls back to plain cost when ratio is zero', () => {
		const tokens = { input: 100, output: 50, cache_read: 0, cache_create: 0 };
		const without = estimateWithoutCtxone('claude-sonnet-4.5', tokens, 0);
		const plain = estimateCost('claude-sonnet-4.5', tokens);
		expect(without).toBe(plain);
	});

	it('returns null for unknown models', () => {
		expect(
			estimateWithoutCtxone(
				'gpt-9000',
				{ input: 100, output: 50, cache_read: 0, cache_create: 0 },
				5
			)
		).toBeNull();
	});
});

describe('formatUsd', () => {
	it('shows 4 decimals below $0.1', () => {
		expect(formatUsd(0.00123)).toBe('$0.0012');
		expect(formatUsd(0.09)).toBe('$0.0900');
	});

	it('shows 3 decimals between $0.1 and $1', () => {
		expect(formatUsd(0.12345)).toBe('$0.123');
		expect(formatUsd(0.5)).toBe('$0.500');
	});

	it('shows 2 decimals above $1', () => {
		expect(formatUsd(1.2345)).toBe('$1.23');
		expect(formatUsd(100)).toBe('$100.00');
	});
});

describe('savingsPercent', () => {
	it('converts a ratio to a percentage reduction', () => {
		expect(savingsPercent(4)).toBeCloseTo(75, 5); // 1 − 1/4
		expect(savingsPercent(5)).toBeCloseTo(80, 5); // 1 − 1/5
		expect(savingsPercent(2)).toBeCloseTo(50, 5);
	});

	it('returns null when there is no measurable saving', () => {
		expect(savingsPercent(1)).toBeNull(); // exactly break-even
		expect(savingsPercent(0)).toBeNull();
		expect(savingsPercent(0.5)).toBeNull(); // ratio < 1 → spent more
		expect(savingsPercent(null)).toBeNull();
		expect(savingsPercent(undefined)).toBeNull();
		expect(savingsPercent(Infinity)).toBeNull();
	});

	it('formats as a whole-percent string, or null', () => {
		expect(formatSavingsPercent(4)).toBe('75%');
		expect(formatSavingsPercent(5)).toBe('80%');
		expect(formatSavingsPercent(1)).toBeNull();
	});
});

describe('estimateCostForPlan', () => {
	const session = (
		input: number,
		output: number,
		model: string | null,
		ratio = 1
	): PlanCostSession => ({
		llm_input_tokens: input,
		llm_output_tokens: output,
		llm_cache_read_tokens: 0,
		llm_cache_create_tokens: 0,
		last_model: model,
		cumulative_ratio: ratio
	});

	it('sums cost across tracked sessions and counts untracked ones', () => {
		const out = estimateCostForPlan([
			session(1_000_000, 100_000, 'claude-sonnet-4.5'),
			session(500_000, 50_000, 'claude-sonnet-4.5'),
			session(999, 999, 'gpt-9000') // untracked model
		]);
		expect(out.trackedSessions).toBe(2);
		expect(out.untrackedSessions).toBe(1);
		expect(out.cost).toBeGreaterThan(0);
		// Two tracked sessions cost more than one alone.
		const single = estimateCostForPlan([session(1_000_000, 100_000, 'claude-sonnet-4.5')]);
		expect(out.cost).toBeGreaterThan(single.cost);
	});

	it('reports cost avoided from the savings ratio (>= 0)', () => {
		const out = estimateCostForPlan([session(1_000_000, 100_000, 'claude-sonnet-4.5', 5)]);
		expect(out.costWithoutCtxone).toBeGreaterThan(out.cost);
		expect(out.costAvoided).toBeGreaterThan(0);
		expect(out.costAvoided).toBeCloseTo(out.costWithoutCtxone - out.cost, 10);
	});

	it('is all-zero for an empty plan', () => {
		const out = estimateCostForPlan([]);
		expect(out).toEqual({
			cost: 0,
			costWithoutCtxone: 0,
			costAvoided: 0,
			trackedSessions: 0,
			untrackedSessions: 0
		});
	});
});
