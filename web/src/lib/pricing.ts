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

/** Look up pricing for a model, or `null` if not tracked. */
export function pricingFor(model: string | null | undefined): ModelPricing | null {
	if (!model) return null;
	return PRICING[model] ?? null;
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
 * Format a USD amount for display. Uses 4 decimals below $0.1,
 * 3 decimals below $1, 2 decimals otherwise. Keeps small numbers
 * useful to read while keeping big numbers tidy.
 */
export function formatUsd(amount: number): string {
	if (amount < 0.1) return `$${amount.toFixed(4)}`;
	if (amount < 1) return `$${amount.toFixed(3)}`;
	return `$${amount.toFixed(2)}`;
}
