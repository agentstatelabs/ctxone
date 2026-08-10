/**
 * Per-model pricing table for the LLM consumption panel in Lens.
 *
 * Prices are in USD per 1M tokens, sourced from each provider's
 * public pricing page as of 2026-04-16. Maintenance note: provider
 * prices change (usually down); the spec picks hardcoded-per-model
 * as the v1 approach and flags dynamic pricing as deferred. When
 * prices change, edit this file and ship a new Lens build.
 *
 * The panel falls back to "cost not tracked for model=X" when a
 * reported model isn't present here — we'd rather not guess than
 * show a wrong number.
 *
 * `cache_read` is the Anthropic prompt-caching discount (~10% of
 * input cost). `cache_write` is the premium charged when writing
 * to the cache (~125% of input). Providers that don't offer prompt
 * caching set these equal to `input`.
 */

export interface ModelPricing {
	/** Provider identifier — matches what agents report. */
	provider: string;
	/** USD per 1M input tokens (non-cached). */
	input: number;
	/** USD per 1M output tokens. */
	output: number;
	/** USD per 1M tokens served from the prompt cache. */
	cache_read: number;
	/** USD per 1M tokens written to the prompt cache. */
	cache_write: number;
}

/**
 * Canonical pricing. Keys are the exact model strings agents should
 * pass in the `model` field. Unknown models render a "cost not
 * tracked" note in the panel.
 */
export const PRICING: Record<string, ModelPricing> = {
	// -- Anthropic --
	// Sonnet 4.5 / Haiku 4.5 / Opus 4.5 — spec's v1 list.
	'claude-sonnet-4.5': {
		provider: 'anthropic',
		input: 3.0,
		output: 15.0,
		cache_read: 0.3,
		cache_write: 3.75
	},
	'claude-haiku-4.5': {
		provider: 'anthropic',
		input: 0.8,
		output: 4.0,
		cache_read: 0.08,
		cache_write: 1.0
	},
	'claude-opus-4.5': {
		provider: 'anthropic',
		input: 15.0,
		output: 75.0,
		cache_read: 1.5,
		cache_write: 18.75
	},
	// Current-gen Anthropic models report HYPHENATED ids (claude-opus-4-8),
	// not the dotted form above — pricingFor() does an exact key match, so the
	// key must be spelled exactly as agents report it. Opus 4.8: $5/$25 per 1M
	// (cache_read = 0.1x input, cache_write = 1.25x input, the Anthropic
	// prompt-cache ratios).
	'claude-opus-4-8': {
		provider: 'anthropic',
		input: 5.0,
		output: 25.0,
		cache_read: 0.5,
		cache_write: 6.25
	},
	// Rest of the current-gen Anthropic line, hyphenated as agents report them.
	// Opus 4.6–5 share $5/$25; Sonnet 4.5–5 share $3/$15; Haiku 4.5 $0.80/$4;
	// Fable 5 $10/$50. cache_read ≈ 0.1× input, cache_write ≈ 1.25× input.
	'claude-opus-5': { provider: 'anthropic', input: 5.0, output: 25.0, cache_read: 0.5, cache_write: 6.25 },
	'claude-opus-4-7': { provider: 'anthropic', input: 5.0, output: 25.0, cache_read: 0.5, cache_write: 6.25 },
	'claude-opus-4-6': { provider: 'anthropic', input: 5.0, output: 25.0, cache_read: 0.5, cache_write: 6.25 },
	'claude-sonnet-5': { provider: 'anthropic', input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 },
	'claude-sonnet-4-6': { provider: 'anthropic', input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 },
	'claude-sonnet-4-5': { provider: 'anthropic', input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 },
	'claude-haiku-4-5': { provider: 'anthropic', input: 0.8, output: 4.0, cache_read: 0.08, cache_write: 1.0 },
	'claude-fable-5': { provider: 'anthropic', input: 10.0, output: 50.0, cache_read: 1.0, cache_write: 12.5 },

	// -- OpenAI --
	'gpt-4o': {
		provider: 'openai',
		input: 2.5,
		output: 10.0,
		// OpenAI prompt caching: cached input tokens charged at 50%.
		cache_read: 1.25,
		cache_write: 2.5
	},
	'gpt-4o-mini': {
		provider: 'openai',
		input: 0.15,
		output: 0.6,
		cache_read: 0.075,
		cache_write: 0.15
	},

	// -- Google --
	'gemini-2.5-pro': {
		provider: 'gemini',
		input: 1.25,
		output: 5.0,
		cache_read: 0.3125,
		cache_write: 1.25
	},
	'gemini-2.5-flash': {
		provider: 'gemini',
		input: 0.075,
		output: 0.3,
		cache_read: 0.01875,
		cache_write: 0.075
	}
};

/**
 * Model-FAMILY pricing — best-effort rates applied when an exact key is
 * missing. Model names churn weekly (gpt-5.2 → 5.3-codex → 5.4 → 5.6-sol), and
 * maintaining an exact row per variant is a losing game. Instead we match a
 * family by pattern and lend it a representative base rate, clearly flagged as a
 * FAMILY ESTIMATE so the UI can mark it "est."
 *
 * Provenance: rates below are approximate list prices for each family's flagship
 * tier as of 2026-07 (USD per 1M tokens). They are deliberately conservative and
 * meant for relative comparison, not billing. When a variant's real price is
 * known, add an exact `PRICING` entry — exact always wins over family.
 *
 * ORDER MATTERS: more specific patterns (mini/nano, codex-mini) must precede the
 * broad family catch so a cheaper sub-tier isn't priced at the flagship rate.
 * The first matching entry wins.
 *
 * NOTE: this table is the seam for a future shared pricing SERVICE — the same
 * "resolve a model name to a rate, with a freshness/estimate flag" contract that
 * ThreadWeaver and other apps will need. Keep `resolvePricing` the single entry
 * point so swapping this static table for a service call is a one-function change.
 */
interface PricingFamily {
	/** Human-readable family label, surfaced in tooltips. */
	family: string;
	/** Matches the reported model id (case-insensitive). */
	test: RegExp;
	pricing: ModelPricing;
}

export const PRICING_FAMILIES: PricingFamily[] = [
	// -- OpenAI GPT-5 line (incl. Codex + "-sol"/"-max" variants) --
	// Mini/nano sub-tiers are much cheaper — match before the broad gpt-5 catch.
	{
		family: 'GPT-5 mini',
		test: /^(gpt-5.*(mini|nano)|.*codex-mini)/i,
		pricing: { provider: 'openai', input: 0.25, output: 2.0, cache_read: 0.025, cache_write: 0.25 }
	},
	{
		family: 'GPT-5',
		test: /^gpt-5/i,
		pricing: { provider: 'openai', input: 1.25, output: 10.0, cache_read: 0.125, cache_write: 1.25 }
	},
	// Bare "codex-*" pseudo-models (e.g. codex-auto-review) → GPT-5 family.
	{
		family: 'Codex',
		test: /^codex/i,
		pricing: { provider: 'openai', input: 1.25, output: 10.0, cache_read: 0.125, cache_write: 1.25 }
	},
	// -- OpenAI reasoning (o-series) --
	{
		family: 'OpenAI o-series',
		test: /^o[134](-|$)/i,
		pricing: { provider: 'openai', input: 15.0, output: 60.0, cache_read: 7.5, cache_write: 15.0 }
	},
	// -- OpenAI GPT-4 line --
	{
		family: 'GPT-4o mini',
		test: /^gpt-4.*mini/i,
		pricing: { provider: 'openai', input: 0.15, output: 0.6, cache_read: 0.075, cache_write: 0.15 }
	},
	{
		family: 'GPT-4',
		test: /^gpt-4/i,
		pricing: { provider: 'openai', input: 2.5, output: 10.0, cache_read: 1.25, cache_write: 2.5 }
	},
	// -- Anthropic Claude line (new versions auto-price at family base) --
	{
		family: 'Claude Opus',
		test: /^claude-opus/i,
		pricing: { provider: 'anthropic', input: 5.0, output: 25.0, cache_read: 0.5, cache_write: 6.25 }
	},
	{
		family: 'Claude Sonnet',
		test: /^claude-sonnet/i,
		pricing: { provider: 'anthropic', input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 }
	},
	{
		family: 'Claude Haiku',
		test: /^claude-haiku/i,
		pricing: { provider: 'anthropic', input: 0.8, output: 4.0, cache_read: 0.08, cache_write: 1.0 }
	},
	{
		family: 'Claude Fable',
		test: /^claude-fable/i,
		pricing: { provider: 'anthropic', input: 10.0, output: 50.0, cache_read: 1.0, cache_write: 12.5 }
	},
	// -- Google Gemini line --
	{
		family: 'Gemini Flash',
		test: /^gemini.*flash/i,
		pricing: { provider: 'gemini', input: 0.075, output: 0.3, cache_read: 0.01875, cache_write: 0.075 }
	},
	{
		family: 'Gemini',
		test: /^gemini/i,
		pricing: { provider: 'gemini', input: 1.25, output: 5.0, cache_read: 0.3125, cache_write: 1.25 }
	}
];

/** Whether a resolved price is an exact per-model rate or a family estimate. */
export type PricingSource = 'exact' | 'family';

export interface ResolvedPricing {
	pricing: ModelPricing;
	source: PricingSource;
	/** Family label when `source === 'family'`. */
	family?: string;
}

/**
 * Resolve a model to a rate. Exact `PRICING` entries win; otherwise the first
 * matching family lends a best-effort estimate. `null` only when nothing —
 * exact or family — matches. This is the single lookup seam a future pricing
 * service would replace.
 */
export function resolvePricing(model: string | null | undefined): ResolvedPricing | null {
	if (!model) return null;
	const exact = PRICING[model];
	if (exact) return { pricing: exact, source: 'exact' };
	for (const f of PRICING_FAMILIES) {
		if (f.test.test(model)) return { pricing: f.pricing, source: 'family', family: f.family };
	}
	return null;
}

/**
 * Look up pricing for a model, or `null` if neither an exact entry nor a family
 * pattern matches. Family matches are best-effort estimates — use
 * `resolvePricing` when you need to know which.
 */
export function pricingFor(model: string | null | undefined): ModelPricing | null {
	return resolvePricing(model)?.pricing ?? null;
}

export interface TokenBreakdown {
	input: number;
	output: number;
	cache_read: number;
	cache_create: number;
}

/**
 * Estimate USD cost for the given token breakdown using the model's
 * pricing. Returns `null` when the model isn't tracked — callers
 * should render a "cost not tracked" indicator rather than $0.
 *
 * Formula: (input + cache_create) billed at the respective rates, plus
 * output × output_rate, plus cache_read × cache_read_rate. Note that
 * Anthropic bills cache writes at a premium and cache reads at a
 * discount; this function honors both with the per-model rates.
 */
export function estimateCost(
	model: string | null | undefined,
	tokens: TokenBreakdown
): number | null {
	const p = pricingFor(model);
	if (!p) return null;
	const per = 1_000_000;
	return (
		(tokens.input * p.input) / per +
		(tokens.output * p.output) / per +
		(tokens.cache_read * p.cache_read) / per +
		(tokens.cache_create * p.cache_write) / per
	);
}

/**
 * The "without CTXone" extrapolation shown in the consumption panel.
 *
 * The spec (§8): `llm_input_tokens_total × cumulative_ratio`. The
 * idea is that without CTXone's budgeted recall, the agent would
 * have flat-loaded memory and sent proportionally more tokens on
 * every turn. Multiplying the LLM-measured input by the CTXone-side
 * savings ratio gives the counterfactual "what the agent would have
 * paid without us."
 *
 * Output tokens aren't scaled — the model's response length doesn't
 * depend on how much context we loaded. Cache tokens aren't scaled
 * either because they're a property of the input side too.
 */
export function estimateWithoutCtxone(
	model: string | null | undefined,
	tokens: TokenBreakdown,
	cumulativeRatio: number
): number | null {
	if (cumulativeRatio <= 0) return estimateCost(model, tokens);
	return estimateCost(model, {
		input: Math.round(tokens.input * cumulativeRatio),
		output: tokens.output,
		cache_read: Math.round(tokens.cache_read * cumulativeRatio),
		cache_create: tokens.cache_create
	});
}

/**
 * One session's usage as returned by `GET /api/stats/plan/{plan}/cost`
 * (t-002, cost-per-feature). Priced read-time here rather than in the
 * server, so the per-model rate table lives in exactly one place.
 */
export interface PlanCostSession {
	llm_input_tokens: number;
	llm_output_tokens: number;
	llm_cache_read_tokens: number;
	llm_cache_create_tokens: number;
	last_model: string | null;
	cumulative_ratio: number;
}

export interface PlanCostSummary {
	/** Actual USD across sessions whose model is priced. */
	cost: number;
	/** Counterfactual USD without CTXone's budgeted recall. */
	costWithoutCtxone: number;
	/** costWithoutCtxone − cost: what CTXone saved on this feature. */
	costAvoided: number;
	trackedSessions: number;
	/** Sessions whose model isn't in the price table (excluded from cost). */
	untrackedSessions: number;
}

/**
 * Cost-per-feature: price every session linked to a plan by its own model
 * and sum. Sessions on an untracked model are counted, not guessed at, so
 * the UI can show "N sessions not priced" rather than an understated total.
 */
export function estimateCostForPlan(sessions: PlanCostSession[]): PlanCostSummary {
	let cost = 0;
	let without = 0;
	let tracked = 0;
	let untracked = 0;
	for (const s of sessions) {
		const tokens: TokenBreakdown = {
			input: s.llm_input_tokens,
			output: s.llm_output_tokens,
			cache_read: s.llm_cache_read_tokens,
			cache_create: s.llm_cache_create_tokens
		};
		const c = estimateCost(s.last_model, tokens);
		if (c === null) {
			untracked++;
			continue;
		}
		tracked++;
		cost += c;
		without += estimateWithoutCtxone(s.last_model, tokens, s.cumulative_ratio) ?? c;
	}
	return {
		cost,
		costWithoutCtxone: without,
		costAvoided: Math.max(0, without - cost),
		trackedSessions: tracked,
		untrackedSessions: untracked
	};
}

/**
 * Convert a savings *ratio* (counterfactual ÷ actual, e.g. 4× means CTXone
 * spend was a quarter of the flat-context spend) into a percentage reduction:
 * the share of the counterfactual bill that CTXone avoided.
 *
 *   ratio 4 → 75%, ratio 5 → 80%, ratio 2 → 50%, ratio 1 → 0%.
 *
 * We show a percent rather than an "Nx" multiplier because a truthful 4× reads
 * as unimpressive next to competitors' inflated "1000×" claims, while the same
 * number stated as "75% saved" is both honest and clearer. Formula:
 * `1 − 1/ratio`. Returns null when nothing was measured or there was no saving
 * (ratio ≤ 1), so callers can hide the row instead of showing "0%".
 */
export function savingsPercent(ratio: number | null | undefined): number | null {
	if (ratio == null || !Number.isFinite(ratio) || ratio <= 1) return null;
	return (1 - 1 / ratio) * 100;
}

/** `savingsPercent` rendered as a whole-percent string (e.g. "75%"), or null. */
export function formatSavingsPercent(ratio: number | null | undefined): string | null {
	const p = savingsPercent(ratio);
	if (p === null) return null;
	return `${Math.round(p)}%`;
}

/**
 * Format a USD amount for display. Uses 4 decimals below $0.1,
 * 3 decimals below $1, 2 decimals otherwise. Keeps small numbers
 * useful to read while keeping big numbers tidy.
 */
export function formatUsd(amount: number): string {
	if (amount < 0.1) return `$${amount.toFixed(4)}`;
	if (amount < 1) return `$${amount.toFixed(3)}`;
	return `$${amount.toFixed(2)}`;
}
