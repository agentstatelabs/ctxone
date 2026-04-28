<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { getBranches, createBranch } from '$lib/api';
	import { listPlans } from '$lib/plansApi';
	import { branchStore } from '$lib/branchStore.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	interface BranchRow {
		name: string;
		id: string;
		plan_count: number | null;
	}

	let rows: BranchRow[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);

	let showCreate = $state(false);
	let newName = $state('');
	let newFrom = $state('main');
	let createError: string | null = $state(null);

	async function load() {
		loading = true;
		error = null;
		try {
			const branches = await getBranches();
			rows = branches.map((b) => ({ ...b, plan_count: null }));
			rows.sort((a, b) => (a.name === 'main' ? -1 : b.name === 'main' ? 1 : a.name.localeCompare(b.name)));
			void Promise.all(
				rows.map(async (r) => {
					try {
						const plans = await listPlans(r.name);
						r.plan_count = plans.length;
						rows = rows;
					} catch {
						r.plan_count = -1;
					}
				})
			);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	onMount(load);

	const auto = useAutoRefresh(load);

	function activate(name: string) {
		branchStore.current = name;
	}

	function viewPlans(name: string) {
		branchStore.current = name;
		goto('/plans');
	}

	function viewBrowse(name: string) {
		branchStore.current = name;
		goto('/browse');
	}

	async function handleCreate(e: Event) {
		e.preventDefault();
		const name = newName.trim();
		if (!name) return;
		createError = null;
		try {
			await createBranch({ name, from: newFrom });
			newName = '';
			showCreate = false;
			branchStore.current = name;
			await load();
		} catch (err) {
			createError = err instanceof Error ? err.message : String(err);
		}
	}
</script>

<div class="page">
	<header class="page-header">
		<h1>Branches</h1>
		<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
		<button class="btn" onclick={() => (showCreate = !showCreate)}>
			{showCreate ? 'Cancel' : '+ New branch'}
		</button>
	</header>

	{#if showCreate}
		<form class="create-form" onsubmit={handleCreate}>
			<input type="text" bind:value={newName} placeholder="branch name" required />
			<label class="from-label">
				from
				<select bind:value={newFrom}>
					{#each rows as r}
						<option value={r.name}>{r.name}</option>
					{/each}
				</select>
			</label>
			<button type="submit">Create</button>
			{#if createError}
				<span class="error">{createError}</span>
			{/if}
		</form>
	{/if}

	{#if error}
		<p class="error">{error}</p>
	{:else if loading}
		<p class="muted">Loading branches…</p>
	{:else if rows.length === 0}
		<p class="muted">No branches.</p>
	{:else}
		<table class="branches">
			<thead>
				<tr>
					<th>Name</th>
					<th>Head</th>
					<th class="num">Plans</th>
					<th class="actions">Actions</th>
				</tr>
			</thead>
			<tbody>
				{#each rows as r}
					<tr class:active={r.name === branchStore.current}>
						<td class="name">
							<code>{r.name}</code>
							{#if r.name === branchStore.current}
								<span class="active-tag">current</span>
							{/if}
						</td>
						<td><code class="hash">{r.id}</code></td>
						<td class="num">
							{#if r.plan_count === null}
								<span class="muted">…</span>
							{:else if r.plan_count < 0}
								<span class="muted">—</span>
							{:else}
								{r.plan_count}
							{/if}
						</td>
						<td class="actions">
							<button class="btn-sm" onclick={() => activate(r.name)} disabled={r.name === branchStore.current}>
								Use
							</button>
							<button class="btn-sm" onclick={() => viewPlans(r.name)}>Plans</button>
							<button class="btn-sm" onclick={() => viewBrowse(r.name)}>Browse</button>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</div>

<style>
	.page {
		max-width: 900px;
	}

	.page-header {
		display: flex;
		align-items: center;
		gap: 1rem;
		margin-bottom: 1.5rem;
	}

	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		margin-right: auto;
	}

	h1 {
		margin: 0;
		font-size: 1.6rem;
	}

	.btn,
	.btn-sm {
		background: var(--bg-1);
		border: 1px solid var(--border);
		color: var(--text-2);
		padding: 0.35rem 0.75rem;
		border-radius: 6px;
		cursor: pointer;
		font-size: 0.85rem;
	}
	.btn:hover,
	.btn-sm:hover:not(:disabled) {
		color: var(--text-0);
		border-color: var(--text-3);
	}
	.btn-sm {
		padding: 0.2rem 0.55rem;
		font-size: 0.78rem;
	}
	.btn-sm:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.create-form {
		display: flex;
		gap: 0.5rem;
		align-items: center;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 0.75rem;
		margin-bottom: 1.25rem;
	}

	.create-form input {
		flex: 1;
		min-width: 8rem;
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.4rem 0.6rem;
		border-radius: 4px;
	}

	.from-label {
		font-size: 0.8rem;
		color: var(--text-2);
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}

	.create-form select {
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.35rem 0.5rem;
		border-radius: 4px;
	}

	.create-form button {
		background: var(--accent-bg);
		border: 1px solid var(--accent-bg-hi);
		color: var(--accent);
		padding: 0.4rem 0.9rem;
		border-radius: 4px;
		cursor: pointer;
	}

	.error {
		color: var(--danger);
		font-size: 0.85rem;
	}
	.muted {
		color: var(--text-3);
		font-size: 0.9rem;
	}

	table.branches {
		width: 100%;
		border-collapse: collapse;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow: hidden;
	}

	th,
	td {
		padding: 0.6rem 0.9rem;
		text-align: left;
		border-bottom: 1px solid var(--border);
	}
	tr:last-child td {
		border-bottom: none;
	}
	th {
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-3);
		background: var(--bg-0);
		font-weight: 600;
	}
	tr.active td {
		background: var(--bg-active);
	}
	td.num,
	th.num {
		text-align: right;
		font-variant-numeric: tabular-nums;
	}
	td.actions,
	th.actions {
		text-align: right;
		white-space: nowrap;
	}

	.name code {
		font-family: monospace;
		color: var(--text-0);
	}

	.hash {
		font-family: monospace;
		color: var(--text-2);
		font-size: 0.85rem;
	}

	.active-tag {
		font-size: 0.65rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--accent);
		background: var(--accent-bg);
		padding: 0.05rem 0.4rem;
		border-radius: 3px;
		margin-left: 0.5rem;
		vertical-align: middle;
	}
</style>
