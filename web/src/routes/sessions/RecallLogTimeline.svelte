<script lang="ts">
	import { getSessionRecallLog, type RecallLogEntry } from '$lib/api';
	import { formatCompact } from '@agentstate/lens-core';

	interface Props {
		sessionId: string;
	}

	let { sessionId }: Props = $props();

	let entries = $state<RecallLogEntry[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);

	async function load(sid: string) {
		loading = true;
		error = null;
		try {
			const r = await getSessionRecallLog(sid);
			// Newest first — the log is capped and appended chronologically.
			entries = [...r.recall_log].reverse();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			entries = [];
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		if (sessionId) load(sessionId);
		else entries = [];
	});

	function shortPath(p: string): string {
		const parts = p.split('/').filter(Boolean);
		return parts.length <= 2 ? p : `…/${parts.slice(-2).join('/')}`;
	}

	function when(at: string): string {
		const d = new Date(at);
		return isNaN(d.getTime()) ? at : d.toLocaleString();
	}
</script>

<h3>
	Recall log
	{#if entries.length}<span class="count">{entries.length}</span>{/if}
</h3>

{#if loading}
	<p class="muted">Loading recall log…</p>
{:else if error}
	<p class="muted hint">Recall log unavailable: {error}</p>
{:else if entries.length === 0}
	<p class="muted hint">
		No recalls recorded for this session. The recall log is live and
		non-durable — it captures <code>recall</code> injections since the Hub
		last started, so a resumed or ingested session may show none.
	</p>
{:else}
	<ol class="recall-list">
		{#each entries as e, i (i)}
			<li class="recall-item">
				<div class="recall-head">
					<span class="recall-topic" title={e.topic}>{e.topic}</span>
					<span class="recall-time">{when(e.at)}</span>
				</div>
				<div class="recall-meta">
					<span class="recall-stat" title="memory paths injected into context">
						{e.paths.length} path{e.paths.length === 1 ? '' : 's'}
					</span>
					<span class="recall-dot">·</span>
					<span class="recall-stat" title="{e.tokens_sent} tokens sent">
						{formatCompact(e.tokens_sent)} tok sent
					</span>
					{#if e.savings_ratio > 1}
						<span class="recall-dot">·</span>
						<span class="recall-savings" title="tokens a flat dump would have cost ÷ tokens sent">
							{e.savings_ratio.toFixed(1)}× saved
						</span>
					{/if}
				</div>
				{#if e.paths.length}
					<ul class="recall-paths">
						{#each e.paths as p (p)}
							<li><code title={p}>{shortPath(p)}</code></li>
						{/each}
					</ul>
				{/if}
			</li>
		{/each}
	</ol>
	<p class="muted recall-foot">What memory shaped each answer, most recent first.</p>
{/if}

<style>
	.count {
		font-size: 0.75rem;
		font-weight: 500;
		color: var(--text-2);
		background: var(--bg-2);
		border-radius: 999px;
		padding: 0.05rem 0.5rem;
		margin-left: 0.4rem;
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

	.recall-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		/* Timeline rail. */
		border-left: 2px solid var(--border);
		padding-left: 0.9rem;
	}
	.recall-item {
		position: relative;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 0.5rem 0.7rem;
	}
	.recall-item::before {
		content: '';
		position: absolute;
		left: -1.05rem;
		top: 0.85rem;
		width: 8px;
		height: 8px;
		border-radius: 999px;
		background: var(--accent);
		box-shadow: 0 0 0 3px var(--bg-0);
	}
	.recall-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 0.5rem;
	}
	.recall-topic {
		font-weight: 600;
		color: var(--text-0);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.recall-time {
		flex: none;
		font-size: 0.75rem;
		color: var(--text-3);
		font-variant-numeric: tabular-nums;
	}
	.recall-meta {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.3rem;
		margin-top: 0.3rem;
		font-size: 0.8rem;
		color: var(--text-2);
	}
	.recall-dot {
		opacity: 0.5;
	}
	.recall-savings {
		color: var(--success);
		font-weight: 600;
	}
	.recall-paths {
		list-style: none;
		margin: 0.4rem 0 0;
		padding: 0;
		display: flex;
		flex-wrap: wrap;
		gap: 0.3rem;
	}
	.recall-paths code {
		background: var(--bg-2);
		border: 1px solid var(--border);
		border-radius: 4px;
		padding: 0.05rem 0.35rem;
		font-size: 0.75rem;
		color: var(--text-1);
	}
	.recall-foot {
		margin-top: 0.5rem;
	}
</style>
