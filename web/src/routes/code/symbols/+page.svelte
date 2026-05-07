<script lang="ts">
	import { getSymbols } from '$lib/codeApi';
	import type { SymbolSummary } from '$lib/codeTypes';
	import { selectedRepo } from '$lib/repoStore';

	const KINDS = ['', 'function', 'method', 'class', 'module', 'variable'];
	const PAGE_SIZE = 50;

	let all = $state<SymbolSummary[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let filterName = $state('');
	let filterKind = $state('');
	let filterLang = $state('');
	let page = $state(0);

	$effect(() => {
		const repo = $selectedRepo;
		loading = true;
		error = null;
		getSymbols(repo)
			.then((s) => { all = s; loading = false; })
			.catch((e) => { error = e instanceof Error ? e.message : String(e); loading = false; });
	});

	let filtered = $derived(
		all.filter((s) => {
			if (filterKind && s.kind !== filterKind) return false;
			if (filterLang && s.language !== filterLang) return false;
			if (filterName) {
				const q = filterName.toLowerCase();
				return s.qname.toLowerCase().includes(q) || s.file.toLowerCase().includes(q);
			}
			return true;
		})
	);

	let pageCount = $derived(Math.max(1, Math.ceil(filtered.length / PAGE_SIZE)));
	let visible = $derived(filtered.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE));

	let langs = $derived([...new Set(all.map((s) => s.language))].sort());

	function onFilterChange() {
		page = 0;
	}
</script>

<div class="page">
	<h2>Symbols</h2>

	<div class="filters">
		<input
			type="text"
			placeholder="Filter by name or file…"
			bind:value={filterName}
			oninput={onFilterChange}
		/>
		<select bind:value={filterKind} onchange={onFilterChange}>
			{#each KINDS as k}
				<option value={k}>{k || 'all kinds'}</option>
			{/each}
		</select>
		<select bind:value={filterLang} onchange={onFilterChange}>
			<option value="">all languages</option>
			{#each langs as l}
				<option value={l}>{l}</option>
			{/each}
		</select>
	</div>

	{#if loading}
		<p class="muted">loading…</p>
	{:else if error}
		<p class="error">{error}</p>
	{:else}
		<div class="meta">
			{filtered.length.toLocaleString()} of {all.length.toLocaleString()} symbols
		</div>

		<table>
			<thead>
				<tr>
					<th>Kind</th>
					<th>Name</th>
					<th>Language</th>
					<th>File</th>
					<th>Line</th>
				</tr>
			</thead>
			<tbody>
				{#each visible as s (s.symbol_id)}
					<tr onclick={() => (window.location.href = `/code/symbols/${encodeURIComponent(s.qname)}`)}>
						<td><span class="kind-badge kind-{s.kind}">{s.kind}</span></td>
						<td class="qname-cell">
							<a href="/code/symbols/{encodeURIComponent(s.qname)}">{s.qname}</a>
						</td>
						<td class="lang">{s.language}</td>
						<td class="file"><code>{s.file}</code></td>
						<td class="line">{s.start.line}</td>
					</tr>
				{/each}
			</tbody>
		</table>

		{#if pageCount > 1}
			<div class="pagination">
				<button onclick={() => (page = Math.max(0, page - 1))} disabled={page === 0}>←</button>
				<span>{page + 1} / {pageCount}</span>
				<button onclick={() => (page = Math.min(pageCount - 1, page + 1))} disabled={page === pageCount - 1}>→</button>
			</div>
		{/if}
	{/if}
</div>

<style>
	.page {
		max-width: 1000px;
	}

	h2 {
		margin: 0 0 1.25rem;
	}

	.filters {
		display: flex;
		gap: 0.6rem;
		margin-bottom: 1rem;
	}

	.filters input,
	.filters select {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text-1);
		padding: 0.4rem 0.6rem;
		font-family: inherit;
		font-size: 0.88rem;
	}

	.filters input {
		flex: 1;
	}

	.filters input:focus,
	.filters select:focus {
		outline: none;
		border-color: var(--accent);
	}

	.meta {
		font-size: 0.8rem;
		color: var(--text-3);
		margin-bottom: 0.6rem;
	}

	.muted {
		color: var(--text-3);
	}

	.error {
		color: var(--danger);
	}

	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.88rem;
	}

	thead th {
		text-align: left;
		padding: 0.5rem 0.75rem;
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-3);
		border-bottom: 1px solid var(--border);
	}

	tbody tr {
		cursor: pointer;
		border-bottom: 1px solid var(--bg-hover);
	}

	tbody tr:hover {
		background: var(--bg-hover);
	}

	tbody td {
		padding: 0.45rem 0.75rem;
		vertical-align: middle;
	}

	.qname-cell a {
		font-family: monospace;
		color: var(--text-0);
		text-decoration: none;
		display: block;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 400px;
	}

	.qname-cell a:hover {
		color: var(--accent);
	}

	.lang {
		color: var(--text-2);
		font-size: 0.82rem;
	}

	.file code {
		color: var(--text-3);
		font-size: 0.8rem;
		display: block;
		max-width: 280px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.line {
		color: var(--text-3);
		font-family: monospace;
		font-size: 0.82rem;
		text-align: right;
	}

	.kind-badge {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}

	.kind-function  { color: var(--kind-function,  #88c0d0); }
	.kind-method    { color: var(--kind-method,    #a3be8c); }
	.kind-class     { color: var(--kind-class,     #d08770); }
	.kind-module    { color: var(--kind-module,    #b48ead); }
	.kind-variable  { color: var(--kind-variable,  #ebcb8b); }

	.pagination {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		margin-top: 1rem;
		font-size: 0.88rem;
		color: var(--text-2);
	}

	.pagination button {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--text-1);
		padding: 0.3rem 0.65rem;
		cursor: pointer;
		font-size: 0.9rem;
	}

	.pagination button:hover:not(:disabled) {
		border-color: var(--accent);
		color: var(--accent);
	}

	.pagination button:disabled {
		opacity: 0.4;
		cursor: default;
	}
</style>
