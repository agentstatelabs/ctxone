<!--
	SyncStatus — whether this workspace syncs with a remote Hub, and where each
	of its branches stands against it (ctx-hub-sync t-006).

	Reads GET /api/sync/status, which reports what the last sync left: it
	doesn't ask the remote. A workspace that doesn't sync shows how to start.
	Only a single-tenant Hub syncs with a remote, so the workspace page shows
	this inside <MachineLocal>.
-->
<script lang="ts">
	import { hubFetch } from '$lib/api';
	import { namespaceStore } from '$lib/namespaceStore.svelte';

	type BranchState = {
		branch: string;
		state: string;
		local: string | null;
		remote: string | null;
	};
	type WorkspaceSync = {
		namespace: string;
		remote: string;
		branches: BranchState[];
		last: { at: string; ok: boolean } | null;
	};

	let all = $state<WorkspaceSync[]>([]);
	let failed = $state(false);

	$effect(() => {
		namespaceStore.current; // dependency: refetch on workspace switch
		failed = false;
		hubFetch('/api/sync/status')
			.then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
			.then((v: { workspaces?: WorkspaceSync[] }) => (all = v.workspaces ?? []))
			.catch(() => {
				all = [];
				failed = true;
			});
	});

	const here = $derived(all.find((w) => w.namespace === namespaceStore.current) ?? null);

	const LABELS: Record<string, string> = {
		up_to_date: 'up to date',
		ahead: 'ahead: push to send',
		behind: 'behind: pull to take',
		diverged: 'diverged: sync to merge',
		not_pushed: 'not pushed yet',
		not_pulled: 'only on the remote',
		unrelated: 'not joined yet: sync to join'
	};

	function label(state: string): string {
		return LABELS[state] ?? state.replaceAll('_', ' ');
	}

	function when(at: string): string {
		const t = Date.parse(at);
		if (Number.isNaN(t)) return at;
		const s = Math.max(0, (Date.now() - t) / 1000);
		if (s < 90) return 'just now';
		if (s < 5400) return `${Math.round(s / 60)} min ago`;
		if (s < 86400) return `${Math.round(s / 3600)} h ago`;
		return `${Math.round(s / 86400)} d ago`;
	}
</script>

<div class="sync">
	{#if failed}
		<p class="muted">Couldn't read the sync status.</p>
	{:else if !here}
		<p class="muted">
			This workspace doesn't sync. To keep it on your other machines:
			<code>ctx remote add hub &lt;url&gt; --token-stdin</code>, then
			<code>ctx remote set {namespaceStore.current} hub</code>. Raw session turns stay on
			this machine.
		</p>
	{:else}
		<p>
			Syncs with <strong>{here.remote}</strong>
			{#if here.last}
				· {here.last.ok ? 'last synced' : 'last sync had problems'}
				{when(here.last.at)}
			{:else}
				· not synced yet
			{/if}
		</p>
		{#if here.branches.length}
			<ul>
				{#each here.branches as b (b.branch)}
					<li class:warn={b.state !== 'up_to_date'}>
						<span class="mono">{b.branch}</span>
						<span>{label(b.state)}</span>
					</li>
				{/each}
			</ul>
		{/if}
		{#if here.last && !here.last.ok}
			<p class="muted">Run <code>ctx sync</code> to see what didn't go.</p>
		{/if}
	{/if}
</div>

<style>
	.sync p {
		margin: 0 0 0.5rem;
	}
	.muted {
		color: var(--text-muted, #888);
	}
	ul {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	li {
		display: flex;
		justify-content: space-between;
		gap: 1rem;
		padding: 0.15rem 0;
	}
	li.warn span:last-child {
		color: var(--warn, #b26b00);
	}
	.mono,
	code {
		font-family: var(--font-mono, ui-monospace, monospace);
	}
</style>
