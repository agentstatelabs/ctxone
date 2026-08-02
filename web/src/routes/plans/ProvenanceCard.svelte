<script lang="ts">
	import { getPlanProvenance, type ProvenanceResponse } from '$lib/plansApi';
	import { formatUsd } from '$lib/pricing';
	import { formatCompact } from '@agentstate/lens-core';

	interface Props {
		plan: string;
		branch?: string;
		onOpenSession?: (sessionId: string) => void;
	}

	let { plan, branch = 'main', onOpenSession }: Props = $props();

	let data = $state<ProvenanceResponse | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);

	async function load(name: string, ref: string) {
		loading = true;
		error = null;
		try {
			data = await getPlanProvenance(name, ref);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			data = null;
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		if (plan) load(plan, branch);
	});

	let summary = $derived(data?.proof_summary ?? null);
	let allDone = $derived(!!summary && summary.tasks_total > 0 && summary.tasks_done === summary.tasks_total);

	/** Flatten the raw `/memory/{plan}` decisions subtree into readable rows. */
	interface DecisionRow {
		key: string;
		text: string;
	}
	let decisions = $derived.by<DecisionRow[]>(() => {
		const tree = data?.decisions;
		if (!tree || typeof tree !== 'object') return [];
		const rows: DecisionRow[] = [];
		const walk = (node: unknown, prefix: string) => {
			if (node && typeof node === 'object' && !Array.isArray(node)) {
				for (const [k, v] of Object.entries(node as Record<string, unknown>)) {
					walk(v, prefix ? `${prefix}/${k}` : k);
				}
			} else {
				const text =
					typeof node === 'string' ? node : JSON.stringify(node);
				rows.push({ key: prefix || '(root)', text });
			}
		};
		walk(tree, '');
		return rows;
	});

	let cost = $derived(data?.cost ?? null);
</script>

<section class="card">
	<header>
		<h3>Provenance</h3>
		<span class="sub">what was done · and why</span>
	</header>

	{#if loading}
		<p class="muted">Loading provenance…</p>
	{:else if error}
		<p class="muted hint">Provenance unavailable: {error}</p>
	{:else if data && summary}
		<!-- Proof badge: the trust headline. -->
		<div class="badges">
			<span class="badge" class:ok={allDone}>
				{summary.tasks_done}/{summary.tasks_total} task{summary.tasks_total === 1 ? '' : 's'} done
			</span>
			<span
				class="badge"
				class:ok={summary.tasks_with_commit_proof > 0}
				title="Tasks whose proof is a commit"
			>
				{summary.tasks_with_commit_proof} commit-proofed
			</span>
			{#if cost}
				<span class="badge cost" title="Total LLM tokens across linked sessions">
					{formatCompact((cost.llm_input_tokens ?? 0) + (cost.llm_output_tokens ?? 0))} tok
				</span>
				{#if cost.ctx_tokens_saved > 0}
					<span class="badge saved" title="Context tokens CTXone kept out of the prompt">
						{formatCompact(cost.ctx_tokens_saved)} saved
					</span>
				{/if}
			{/if}
		</div>

		<!-- Who did the work. -->
		<div class="block">
			<h4>Sessions <span class="count">{data.sessions.length}</span></h4>
			{#if data.sessions.length === 0}
				<p class="muted hint">No sessions linked to this plan.</p>
			{:else}
				<ul class="sessions">
					{#each data.sessions as s (s.session_id)}
						<li>
							{#if onOpenSession}
								<button class="link" onclick={() => onOpenSession?.(s.session_id)}>
									{s.name || s.session_id.slice(0, 12)}
								</button>
							{:else}
								<span title={s.session_id}>{s.name || s.session_id.slice(0, 12)}</span>
							{/if}
							<span class="s-meta" title="{s.llm_input_tokens} in / {s.llm_output_tokens} out">
								{formatCompact(s.llm_input_tokens + s.llm_output_tokens)} tok
								{#if s.last_model}· {s.last_model}{/if}
							</span>
						</li>
					{/each}
				</ul>
			{/if}
		</div>

		<!-- The why. -->
		<div class="block">
			<h4>Decisions {#if decisions.length}<span class="count">{decisions.length}</span>{/if}</h4>
			{#if decisions.length === 0}
				<p class="muted hint">
					No decisions recorded under this plan. Capture the "why" with
					<code>remember</code> tagged to the plan.
				</p>
			{:else}
				<ul class="decisions">
					{#each decisions as d (d.key)}
						<li>
							<code class="d-key">{d.key}</code>
							<span class="d-text">{d.text}</span>
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	{:else}
		<p class="muted hint">No provenance for this plan.</p>
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
	.hint code,
	.decisions code {
		background: var(--bg-2);
		border: 1px solid var(--border);
		border-radius: 3px;
		padding: 0 0.25rem;
		font-size: 0.85em;
	}

	.badges {
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem;
		margin-bottom: 0.9rem;
	}
	.badge {
		font-size: 0.8rem;
		font-weight: 600;
		color: var(--text-1);
		background: var(--bg-2);
		border: 1px solid var(--border);
		border-radius: 999px;
		padding: 0.15rem 0.6rem;
	}
	.badge.ok {
		color: var(--success);
		border-color: color-mix(in srgb, var(--success) 40%, var(--border));
		background: color-mix(in srgb, var(--success) 12%, var(--bg-2));
	}
	.badge.saved {
		color: var(--info);
	}

	.block {
		margin-top: 0.9rem;
	}
	h4 {
		margin: 0 0 0.4rem;
		font-size: 0.8rem;
		text-transform: uppercase;
		letter-spacing: 0.03em;
		color: var(--text-2);
		font-weight: 600;
	}
	.count {
		font-size: 0.7rem;
		color: var(--text-2);
		background: var(--bg-2);
		border-radius: 999px;
		padding: 0.03rem 0.4rem;
		margin-left: 0.3rem;
	}

	.sessions,
	.decisions {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.35rem;
	}
	.sessions li {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 0.5rem;
		font-size: 0.85rem;
	}
	.s-meta {
		flex: none;
		font-size: 0.75rem;
		color: var(--text-2);
		font-family: monospace;
	}
	.decisions li {
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
		font-size: 0.85rem;
		padding: 0.35rem 0.5rem;
		background: var(--bg-2);
		border-radius: 5px;
	}
	.d-key {
		font-size: 0.72rem;
		color: var(--text-3);
		align-self: flex-start;
	}
	.d-text {
		color: var(--text-1);
		white-space: pre-wrap;
		word-break: break-word;
	}
	.link {
		background: none;
		border: none;
		padding: 0;
		color: var(--accent);
		cursor: pointer;
		font: inherit;
	}
	.link:hover {
		text-decoration: underline;
	}
</style>
