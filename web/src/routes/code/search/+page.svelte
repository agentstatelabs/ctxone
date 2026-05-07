<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { searchSymbols } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';
	import type { SearchResult } from '$lib/codeTypes';

	const KINDS = ['', 'function', 'method', 'class', 'module', 'variable'];

	// Sync state with URL params.
	let q = $state($page.url.searchParams.get('q') ?? '');
	let kind = $state($page.url.searchParams.get('kind') ?? '');
	let lang = $state($page.url.searchParams.get('lang') ?? '');

	let results = $state<SearchResult[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let searched = $state(false);

	let debounceTimer: ReturnType<typeof setTimeout> | null = null;

	async function runSearch() {
		const query = q.trim();
		if (!query) {
			results = [];
			searched = false;
			return;
		}
		loading = true;
		error = null;
		searched = true;
		try {
			results = await searchSymbols($selectedRepo, {
				q: query,
				kind: kind || undefined,
				language: lang || undefined,
				limit: 50
			});
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	function handleInput() {
		// Sync URL without navigation.
		const params = new URLSearchParams();
		if (q.trim()) params.set('q', q.trim());
		if (kind) params.set('kind', kind);
		if (lang) params.set('lang', lang);
		const qs = params.toString();
		history.replaceState(null, '', qs ? `?${qs}` : location.pathname);

		if (debounceTimer) clearTimeout(debounceTimer);
		debounceTimer = setTimeout(runSearch, 280);
	}

	// Run on initial load if URL has a query.
	$effect(() => {
		if (q.trim()) runSearch();
	});
</script>

<div class="page">
	<h2>Code Search</h2>

	<div class="search-bar">
		<input
			type="text"
			placeholder="Search symbols — concepts, names, signatures…"
			bind:value={q}
			oninput={handleInput}
			autofocus
		/>
	</div>

	<div class="filters">
		<label>
			Kind
			<select bind:value={kind} onchange={handleInput}>
				{#each KINDS as k}
					<option value={k}>{k || 'all'}</option>
				{/each}
			</select>
		</label>
		<label>
			Language
			<input
				type="text"
				placeholder="e.g. rust, python"
				bind:value={lang}
				oninput={handleInput}
			/>
		</label>
	</div>

	{#if loading}
		<p class="muted">searching…</p>
	{:else if error}
		<p class="error">{error}</p>
	{:else if searched && results.length === 0}
		<p class="muted">No results for <em>{q}</em>.</p>
	{:else if results.length > 0}
		<div class="result-count">{results.length} result{results.length === 1 ? '' : 's'}</div>
		<ul class="results">
			{#each results as r (r.qname)}
				<li>
					<a href="/code/symbols/{encodeURIComponent(r.qname)}">
						<div class="result-header">
							<span class="kind-badge kind-{r.kind}">{r.kind}</span>
							<span class="qname">{r.qname}</span>
							<span class="score" title="relevance score">{r.score}</span>
						</div>
						<div class="result-meta">
							<code class="file">{r.file}:{r.start.line}</code>
							<span class="lang-tag">{r.language}</span>
						</div>
						{#if r.signature}
							<div class="sig"><code>{r.signature}</code></div>
						{/if}
					</a>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.page {
		max-width: 800px;
	}

	h2 {
		margin: 0 0 1.25rem;
	}

	.search-bar input {
		width: 100%;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		color: var(--text-0);
		padding: 0.65rem 1rem;
		font-size: 1rem;
		font-family: inherit;
		box-sizing: border-box;
	}

	.search-bar input:focus {
		outline: none;
		border-color: var(--accent);
	}

	.filters {
		display: flex;
		gap: 1rem;
		margin: 0.75rem 0 1.25rem;
		font-size: 0.85rem;
	}

	.filters label {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		color: var(--text-2);
	}

	.filters select,
	.filters input[type='text'] {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--text-1);
		padding: 0.3rem 0.5rem;
		font-family: inherit;
		font-size: 0.85rem;
	}

	.filters select:focus,
	.filters input:focus {
		outline: none;
		border-color: var(--accent);
	}

	.muted {
		color: var(--text-3);
	}

	.error {
		color: var(--danger);
	}

	.result-count {
		font-size: 0.8rem;
		color: var(--text-3);
		margin-bottom: 0.75rem;
	}

	.results {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}

	.results li a {
		display: block;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 0.75rem 1rem;
		color: inherit;
		text-decoration: none;
		transition: border-color 0.1s;
	}

	.results li a:hover {
		border-color: var(--accent);
		background: var(--bg-hover);
	}

	.result-header {
		display: flex;
		align-items: baseline;
		gap: 0.5rem;
		margin-bottom: 0.3rem;
	}

	.qname {
		font-family: monospace;
		font-size: 0.95rem;
		color: var(--text-0);
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.score {
		font-size: 0.72rem;
		color: var(--text-3);
		font-family: monospace;
		background: var(--bg-hover);
		padding: 1px 5px;
		border-radius: 3px;
	}

	.result-meta {
		display: flex;
		gap: 0.75rem;
		align-items: center;
		font-size: 0.8rem;
		margin-bottom: 0.25rem;
	}

	.file {
		color: var(--text-3);
		font-size: 0.8rem;
	}

	.lang-tag {
		color: var(--text-3);
		font-size: 0.75rem;
		padding: 1px 5px;
		background: var(--bg-hover);
		border-radius: 3px;
	}

	.sig code {
		font-size: 0.8rem;
		color: var(--text-2);
	}

	.kind-badge {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		flex-shrink: 0;
	}

	.kind-function  { color: var(--kind-function,  #88c0d0); }
	.kind-method    { color: var(--kind-method,    #a3be8c); }
	.kind-class     { color: var(--kind-class,     #d08770); }
	.kind-module    { color: var(--kind-module,    #b48ead); }
	.kind-variable  { color: var(--kind-variable,  #ebcb8b); }
</style>
