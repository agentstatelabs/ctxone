<script lang="ts">
	import type { SessionSnapshot } from './api';
	import { estimateCost, estimateWithoutCtxone, formatUsd, pricingFor } from './pricing';

	interface Props {
		/** The session snapshot driving the panel. Usually the current
		 * session, but the dashboard may pass the aggregate to show
		 * Hub-wide consumption. */
		snapshot: SessionSnapshot;
	}

	let { snapshot }: Props = $props();

	let llmInput = $derived(snapshot.llm_input_tokens ?? 0);
	let llmOutput = $derived(snapshot.llm_output_tokens ?? 0);
	let llmCacheRead = $derived(snapshot.llm_cache_read_tokens ?? 0);
	let llmCacheCreate = $derived(snapshot.llm_cache_create_tokens ?? 0);
	let llmCalls = $derived(snapshot.llm_call_count ?? 0);
	let hasAnyUsage = $derived(llmCalls > 0);
	let lastModel = $derived(snapshot.last_model ?? null);
	let lastProvider = $derived(snapshot.last_provider ?? null);

	/** Prompt-cache hit rate: cache_read / (cache_read + input). */
	let cacheHitRate = $derived.by(() => {
		const denom = llmCacheRead + llmInput;
		if (denom === 0) return null;
		return llmCacheRead / denom;
	});

	let tokenBreakdown = $derived({
		input: llmInput,
		output: llmOutput,
		cache_read: llmCacheRead,
		cache_create: llmCacheCreate
	});

	let estimatedCost = $derived(estimateCost(lastModel, tokenBreakdown));
	let withoutCost = $derived(
		estimateWithoutCtxone(lastModel, tokenBreakdown, snapshot.cumulative_ratio)
	);

	/** Measured savings: estimated-without ÷ estimated-with. */
	let measuredSavings = $derived.by(() => {
		if (estimatedCost === null || withoutCost === null) return null;
		if (estimatedCost <= 0) return null;
		return withoutCost / estimatedCost;
	});

	let pricingTracked = $derived(pricingFor(lastModel) !== null);
</script>

<section class="llm-panel" data-testid="llm-consumption-panel">
	<header>
		<h3>LLM Consumption</h3>
		<span class="session-id">session: {snapshot.session_id}</span>
	</header>

	{#if !hasAnyUsage}
		<p class="empty" data-testid="llm-empty">
			No LLM usage reported yet. Agents can call
			<code>record_llm_usage</code> (MCP) or
			<code>POST /api/stats/llm_usage</code> to surface real token counts here.
		</p>
	{:else}
		<dl class="rows">
			<div class="row">
				<dt>Input tokens</dt>
				<dd data-testid="llm-input">{llmInput.toLocaleString()}</dd>
			</div>
			<div class="row">
				<dt>Output tokens</dt>
				<dd data-testid="llm-output">{llmOutput.toLocaleString()}</dd>
			</div>
			<div class="row">
				<dt>
					Cache read tokens
					{#if cacheHitRate !== null && cacheHitRate > 0}
						<span class="muted" data-testid="cache-hit-rate">
							({(cacheHitRate * 100).toFixed(0)}% hit rate)
						</span>
					{/if}
				</dt>
				<dd data-testid="llm-cache-read">{llmCacheRead.toLocaleString()}</dd>
			</div>
			<div class="row">
				<dt>Cache create tokens</dt>
				<dd data-testid="llm-cache-create">{llmCacheCreate.toLocaleString()}</dd>
			</div>
			<div class="row">
				<dt>LLM calls</dt>
				<dd data-testid="llm-calls">{llmCalls.toLocaleString()}</dd>
			</div>
			{#if lastModel}
				<div class="row">
					<dt>Last model</dt>
					<dd data-testid="llm-last-model">
						{lastModel}
						{#if lastProvider}
							<span class="muted">(provider: {lastProvider})</span>
						{/if}
					</dd>
				</div>
			{/if}
		</dl>

		<div class="cost-block" data-testid="cost-block">
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
					<div class="row big">
						<dt>Measured savings</dt>
						<dd class="ratio" data-testid="measured-savings">
							{measuredSavings.toFixed(1)}×
						</dd>
					</div>
				{/if}
			{:else}
				<p class="cost-missing" data-testid="cost-missing">
					Cost not tracked for model=<code>{lastModel ?? 'unknown'}</code>.
					Add pricing in <code>web/src/lib/pricing.ts</code>.
				</p>
			{/if}
		</div>
	{/if}
</section>

<style>
	.llm-panel {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		padding: 1.25rem 1.5rem;
		margin-bottom: 2rem;
	}

	header {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		margin-bottom: 1rem;
	}

	header h3 {
		margin: 0;
		color: #fff;
		font-size: 1rem;
		font-weight: 600;
	}

	.session-id {
		color: #555;
		font-family: monospace;
		font-size: 0.8rem;
	}

	.empty {
		color: #888;
		font-size: 0.9rem;
		margin: 0;
	}

	.empty code,
	.cost-missing code {
		background: #0a0a0a;
		border: 1px solid #222;
		border-radius: 3px;
		padding: 0 0.25rem;
		font-size: 0.85em;
	}

	.rows {
		margin: 0;
	}

	.row {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		padding: 0.5rem 0;
		border-bottom: 1px solid #1a1a1a;
	}

	.row:last-child {
		border-bottom: none;
	}

	.row dt {
		color: #888;
		font-size: 0.9rem;
	}

	.row dd {
		margin: 0;
		color: #fff;
		font-family: monospace;
	}

	.muted {
		color: #555;
		font-size: 0.85em;
	}

	.cost-block {
		margin-top: 1rem;
		padding-top: 0.75rem;
		border-top: 1px dashed #222;
	}

	.cost-missing {
		color: #888;
		font-size: 0.85rem;
		margin: 0;
	}

	.row.big dd.ratio {
		color: #3b82f6;
		font-size: 1.4rem;
		font-weight: 700;
	}
</style>
