<script lang="ts">
	import { getPlanCost, type PlanCostSessionRow } from '$lib/plansApi';
	import { estimateCostForPlan, formatUsd, type PlanCostSummary } from '$lib/pricing';
	import { formatCompact } from '@agentstate/lens-core';

	interface Props {
		plan: string;
		/** Optional: link a session row through to the sessions view. */
		onOpenSession?: (sessionId: string) => void;
	}

	let { plan, onOpenSession }: Props = $props();

	let sessions = $state<PlanCostSessionRow[]>([]);
	let summary = $state<PlanCostSummary | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);

	async function load(name: string) {
		loading = true;
		error = null;
		try {
			const r = await getPlanCost(name);
			sessions = r.sessions;
			// pricing.ts prices read-time by each session's own model.
			summary = estimateCostForPlan(r.sessions);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			sessions = [];
			summary = null;
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		if (plan) load(plan);
	});

	function perSessionCost(s: PlanCostSessionRow): PlanCostSummary {
		return estimateCostForPlan([s]);
	}
</script>

<section class="card">
	<header>
		<h3>Cost per feature</h3>
		{#if sessions.length}
			<span class="sub">{sessions.length} session{sessions.length === 1 ? '' : 's'}</span>
		{/if}
	</header>

	{#if loading}
		<p class="muted">Pricing sessions…</p>
	{:else if error}
		<p class="muted hint">Cost unavailable: {error}</p>
	{:else if !summary || sessions.length === 0}
		<p class="muted hint">
			No sessions linked to this plan yet. Link work with
			<code>ctx session link-plan</code> so its LLM cost rolls up here.
		</p>
	{:else}
		<div class="headline">
			<div class="hl-main">
				This feature cost <strong class="cost">{formatUsd(summary.cost)}</strong>
			</div>
			{#if summary.costAvoided > 0}
				<div class="hl-avoided">
					CTXone avoided ~<strong>{formatUsd(summary.costAvoided)}</strong>
					<span class="muted">
						({(summary.costWithoutCtxone > 0
							? summary.costWithoutCtxone / Math.max(summary.cost, 1e-9)
							: 1
						).toFixed(1)}× vs a flat context)
					</span>
				</div>
			{/if}
		</div>

		{#if summary.untrackedSessions > 0}
			<p class="untracked">
				{summary.untrackedSessions} session{summary.untrackedSessions === 1 ? '' : 's'}
				not priced — model has no entry in the pricing table.
			</p>
		{/if}

		<table class="breakdown">
			<thead>
				<tr>
					<th>Session</th>
					<th>Model</th>
					<th class="num">In / Out</th>
					<th class="num">Cost</th>
				</tr>
			</thead>
			<tbody>
				{#each sessions as s (s.session_id)}
					{@const c = perSessionCost(s)}
					<tr>
						<td>
							{#if onOpenSession}
								<button class="link" onclick={() => onOpenSession?.(s.session_id)}>
									{s.name || s.session_id.slice(0, 12)}
								</button>
							{:else}
								<span title={s.session_id}>{s.name || s.session_id.slice(0, 12)}</span>
							{/if}
						</td>
						<td class="model" title={s.models_used?.join(', ')}>
							{s.last_model ?? '—'}
						</td>
						<td class="num" title="{s.llm_input_tokens} in / {s.llm_output_tokens} out">
							{formatCompact(s.llm_input_tokens)}↑ {formatCompact(s.llm_output_tokens)}↓
						</td>
						<td class="num">
							{#if c.trackedSessions > 0}
								{formatUsd(c.cost)}
							{:else}
								<span class="muted" title="model not in pricing table">n/a</span>
							{/if}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</section>

<style>
	.card {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1rem 1.25rem;
		margin-bottom: 1rem;
	}
	header {
		display: flex;
		align-items: baseline;
		gap: 0.6rem;
		margin-bottom: 0.75rem;
	}
	h3 {
		margin: 0;
		font-size: 1rem;
		color: var(--text-0);
	}
	.sub {
		font-size: 0.8rem;
		color: var(--text-2);
	}
	.muted {
		color: var(--text-2);
		font-size: 0.85rem;
	}
	.hint code {
		background: var(--bg-2);
		border: 1px solid var(--border);
		border-radius: 3px;
		padding: 0 0.25rem;
		font-size: 0.85em;
	}

	.headline {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		padding: 0.6rem 0.8rem;
		background: var(--bg-2);
		border-radius: 6px;
		margin-bottom: 0.75rem;
	}
	.hl-main {
		font-size: 0.95rem;
		color: var(--text-1);
	}
	.cost {
		color: var(--text-0);
		font-size: 1.15rem;
	}
	.hl-avoided {
		font-size: 0.9rem;
		color: var(--success);
	}
	.hl-avoided strong {
		font-size: 1rem;
	}

	.untracked {
		color: var(--warning);
		font-size: 0.8rem;
		margin: 0 0 0.75rem;
	}

	.breakdown {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.85rem;
	}
	.breakdown th {
		text-align: left;
		color: var(--text-2);
		font-weight: 500;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.03em;
		padding: 0.3rem 0.5rem;
		border-bottom: 1px solid var(--border);
	}
	.breakdown td {
		padding: 0.35rem 0.5rem;
		border-bottom: 1px solid var(--border);
		color: var(--text-1);
	}
	.breakdown tr:last-child td {
		border-bottom: none;
	}
	.num {
		text-align: right;
		font-variant-numeric: tabular-nums;
		font-family: monospace;
	}
	.model {
		font-family: monospace;
		font-size: 0.8rem;
		color: var(--text-2);
	}
	.link {
		background: none;
		border: none;
		padding: 0;
		color: var(--accent);
		cursor: pointer;
		font: inherit;
		text-align: left;
	}
	.link:hover {
		text-decoration: underline;
	}
</style>
