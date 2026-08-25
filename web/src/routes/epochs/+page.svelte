<script lang="ts">
	import { getEpochs, epochExportUrl, type Epoch } from '$lib/api';
	import { formatCompact } from '@agentstate/lens-core';
	import { useAutoRefresh } from '$lib/refreshStore.svelte';

	let epochs = $state<Epoch[]>([]);
	let loading = $state(true);
	let error = $state('');

	async function load() {
		try {
			epochs = await getEpochs(true);
			error = '';
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void load();
	});
	useAutoRefresh(() => void load());

	function shortDate(iso: string | null): string {
		if (!iso) return '—';
		const d = new Date(iso);
		return Number.isNaN(d.getTime()) ? '—' : d.toISOString().slice(0, 10);
	}

	const workspaceCount = $derived(new Set(epochs.map((e) => e.namespace)).size);
</script>

<div class="epochs">
	<header class="ep-head">
		<div>
			<h1>Sealed checkpoints</h1>
			<p class="sub">
				Every completed plan seals a tamper-evident, exportable <strong>epoch</strong> — a
				verifiable snapshot of that workspace's memory graph at plan close. Download any bundle to
				archive or independently verify it.
			</p>
		</div>
		<div class="count">
			<span class="n">{formatCompact(epochs.length)}</span>
			<span class="l">epochs · {workspaceCount} workspaces</span>
		</div>
	</header>

	{#if loading}
		<p class="state">Loading…</p>
	{:else if error}
		<p class="state err">{error}</p>
	{:else if epochs.length === 0}
		<p class="state">No sealed checkpoints yet. Close a plan in any workspace to create one.</p>
	{:else}
		<div class="table-wrap">
			<table>
				<thead>
					<tr>
						<th>Workspace</th>
						<th>Plan</th>
						<th>Sealed</th>
						<th class="num">Commits sealed</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each epochs as e (e.id)}
						<tr>
							<td class="ws">{e.namespace}</td>
							<td class="plan">{e.plan}</td>
							<td>{shortDate(e.sealed_at)}</td>
							<td class="num">{formatCompact(e.commit_count)}</td>
							<td class="dl-cell">
								<a
									class="dl"
									href={epochExportUrl(e.id, e.namespace)}
									download
									title="Download audit bundle (JSON)">Download</a
								>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>

<style>
	.epochs {
		max-width: var(--lens-maxw, 1100px);
		margin: 0 auto;
		padding: var(--lens-space-5, 2rem);
	}
	.ep-head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--lens-space-4);
		margin-bottom: var(--lens-space-4);
	}
	h1 {
		margin: 0 0 var(--lens-space-2);
		font-size: var(--lens-font-size-xl, 1.5rem);
	}
	.sub {
		margin: 0;
		max-width: 60ch;
		color: var(--lens-muted);
		line-height: 1.55;
		font-size: var(--lens-font-size-sm);
	}
	.count {
		text-align: right;
		flex: none;
	}
	.count .n {
		display: block;
		font-size: var(--lens-font-size-xl, 1.5rem);
		font-weight: 700;
		font-variant-numeric: tabular-nums;
	}
	.count .l {
		font-size: var(--lens-font-size-2xs, 0.72rem);
		color: var(--lens-muted);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
	}
	.state {
		color: var(--lens-muted);
	}
	.state.err {
		color: var(--lens-error, #f87171);
	}
	.table-wrap {
		overflow-x: auto;
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-md, 8px);
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--lens-font-size-xs);
	}
	th,
	td {
		text-align: left;
		padding: var(--lens-space-2) var(--lens-space-3);
		border-bottom: 1px solid var(--lens-border-subtle, var(--lens-border));
	}
	th {
		color: var(--lens-muted);
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		font-size: var(--lens-font-size-2xs, 0.72rem);
	}
	tbody tr:last-child td {
		border-bottom: none;
	}
	.ws {
		color: var(--lens-text-secondary, var(--lens-muted));
	}
	.plan {
		font-family: var(--lens-font-mono, monospace);
		font-weight: 600;
	}
	.num {
		text-align: right;
		font-variant-numeric: tabular-nums;
	}
	code {
		font-family: var(--lens-font-mono, monospace);
		color: var(--lens-text-secondary, var(--lens-muted));
	}
	.dl-cell {
		text-align: right;
	}
	.dl {
		font-size: var(--lens-font-size-2xs, 0.72rem);
		font-weight: 600;
		text-decoration: none;
		color: var(--lens-accent, #6ea8fe);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm, 6px);
		padding: 3px 10px;
		white-space: nowrap;
	}
	.dl:hover {
		background: var(--lens-surface-raised, rgba(255, 255, 255, 0.04));
	}
</style>
