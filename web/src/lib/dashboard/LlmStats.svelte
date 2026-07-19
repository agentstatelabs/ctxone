<!--
	LlmStats — on-system re-presentation of the LLM consumption block for
	the token-economics panel: observed input/output/cache tokens, call
	count, last model, and the pricing.ts cost estimates (with / without
	CTXone, measured savings). Same derivations as the legacy
	LlmConsumptionPanel, restyled on lens tokens.
-->
<script lang="ts">
	import type { TokenStats } from '$lib/api';
	import { formatCompact } from '@agentstate/lens-core';
	import { estimateCost, estimateWithoutCtxone, formatUsd, pricingFor } from '$lib/pricing';

	// Compact (12.4K / 4.3M / 10.5B / 1.2T) so a row cannot blow out the
	// panel width — Codex sessions report input counts in the billions, which
	// rendered as a 13-character comma-grouped number here. The exact value
	// stays one hover away via `title`, which is the only place it is legible
	// anyway.
	const fmt = (n: number) => formatCompact(n ?? 0);
	const exact = (n: number) => (n ?? 0).toLocaleString();

	let { snapshot }: { snapshot: TokenStats } = $props();

	const llmInput = $derived(snapshot.llm_input_tokens ?? 0);
	const llmOutput = $derived(snapshot.llm_output_tokens ?? 0);
	const llmCacheRead = $derived(snapshot.llm_cache_read_tokens ?? 0);
	const llmCacheCreate = $derived(snapshot.llm_cache_create_tokens ?? 0);
	const llmCalls = $derived(snapshot.llm_call_count ?? 0);
	const hasAnyUsage = $derived(llmCalls > 0);
	const lastModel = $derived(snapshot.last_model ?? null);
	const lastProvider = $derived(snapshot.last_provider ?? null);

	/** Prompt-cache hit rate: cache_read / (cache_read + input). */
	const cacheHitRate = $derived.by(() => {
		const denom = llmCacheRead + llmInput;
		if (denom === 0) return null;
		return llmCacheRead / denom;
	});

	const tokenBreakdown = $derived({
		input: llmInput,
		output: llmOutput,
		cache_read: llmCacheRead,
		cache_create: llmCacheCreate
	});

	const estimatedCost = $derived(estimateCost(lastModel, tokenBreakdown));
	const withoutCost = $derived(
		estimateWithoutCtxone(lastModel, tokenBreakdown, snapshot.cumulative_ratio)
	);

	/** Measured savings: estimated-without ÷ estimated-with. */
	const measuredSavings = $derived.by(() => {
		if (estimatedCost === null || withoutCost === null) return null;
		if (estimatedCost <= 0) return null;
		return withoutCost / estimatedCost;
	});

	const pricingTracked = $derived(pricingFor(lastModel) !== null);
</script>

<div class="llm" data-testid="llm-consumption-panel">
	<h4>LLM consumption</h4>

	{#if !hasAnyUsage}
		<p class="llm-empty" data-testid="llm-empty">
			No LLM usage reported yet. Agents can call <code>record_llm_usage</code> (MCP) or
			<code>POST /api/stats/llm_usage</code> to surface real token counts here.
		</p>
	{:else}
		<dl class="rows">
			<div class="row">
				<dt>Input tokens</dt>
				<dd data-testid="llm-input" title={exact(llmInput)}>{fmt(llmInput)}</dd>
			</div>
			<div class="row">
				<dt>Output tokens</dt>
				<dd data-testid="llm-output" title={exact(llmOutput)}>{fmt(llmOutput)}</dd>
			</div>
			<div class="row">
				<dt>
					Cache read
					{#if cacheHitRate !== null && cacheHitRate > 0}
						<span class="muted" data-testid="cache-hit-rate">
							({(cacheHitRate * 100).toFixed(0)}% hit rate)
						</span>
					{/if}
				</dt>
				<dd data-testid="llm-cache-read" title={exact(llmCacheRead)}>{fmt(llmCacheRead)}</dd>
			</div>
			<div class="row">
				<dt>Cache create</dt>
				<dd data-testid="llm-cache-create" title={exact(llmCacheCreate)}>{fmt(llmCacheCreate)}</dd>
			</div>
			<div class="row">
				<dt>LLM calls</dt>
				<dd data-testid="llm-calls" title={exact(llmCalls)}>{fmt(llmCalls)}</dd>
			</div>
			{#if lastModel}
				<div class="row">
					<dt>Last model</dt>
					<dd data-testid="llm-last-model">
						{lastModel}
						{#if lastProvider}
							<span class="muted">({lastProvider})</span>
						{/if}
					</dd>
				</div>
			{/if}
		</dl>

		<div class="cost" data-testid="cost-block">
			{#if pricingTracked && estimatedCost !== null}
				<div class="row">
					<dt>Estimated cost</dt>
					<dd data-testid="cost-estimated">{formatUsd(estimatedCost)}</dd>
				</div>
				{#if withoutCost !== null && snapshot.cumulative_ratio > 0}
					<div class="row">
						<dt>Without CTXone</dt>
						<dd data-testid="cost-without">
							{formatUsd(withoutCost)}
							<span class="muted">(extrapolated)</span>
						</dd>
					</div>
				{/if}
				{#if measuredSavings !== null && snapshot.cumulative_ratio > 0}
					<div class="row">
						<dt>Measured savings</dt>
						<dd class="ratio" data-testid="measured-savings">{measuredSavings.toFixed(1)}×</dd>
					</div>
				{/if}
			{:else}
				<p class="llm-empty" data-testid="cost-missing">
					Cost not tracked for model=<code>{lastModel ?? 'unknown'}</code>. Add pricing in
					<code>web/src/lib/pricing.ts</code>.
				</p>
			{/if}
		</div>
	{/if}
</div>

<style>
	.llm {
		min-width: 0;
	}

	h4 {
		margin: 0 0 var(--lens-space-2);
		font-size: var(--lens-font-size-2xs);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}

	.llm-empty {
		margin: 0;
		font-size: var(--lens-font-size-xs);
		color: var(--lens-muted);
	}

	.llm-empty code {
		font-family: var(--lens-font-mono);
		font-size: 0.9em;
		background: var(--lens-surface-raised);
		border: 1px solid var(--lens-border-subtle, var(--lens-border));
		border-radius: 3px;
		padding: 0 0.25rem;
	}

	.rows {
		margin: 0;
	}

	.row {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		gap: var(--lens-space-3);
		padding: var(--lens-space-1) 0;
		border-bottom: 1px solid var(--lens-border-subtle, var(--lens-border));
		font-size: var(--lens-font-size-xs);
	}

	.rows .row:last-child {
		border-bottom: none;
	}

	.row dt {
		color: var(--lens-text-secondary);
	}

	.row dd {
		margin: 0;
		color: var(--lens-text-strong);
		font-family: var(--lens-font-mono);
		text-align: right;
	}

	.muted {
		color: var(--lens-muted);
		font-size: 0.9em;
	}

	.cost {
		margin-top: var(--lens-space-2);
		padding-top: var(--lens-space-2);
		border-top: 1px dashed var(--lens-border);
	}

	.cost .row:last-child {
		border-bottom: none;
	}

	dd.ratio {
		color: var(--lens-accent);
		font-size: var(--lens-font-size-md);
		font-weight: 700;
	}
</style>
