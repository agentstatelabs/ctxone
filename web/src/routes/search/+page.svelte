<script lang="ts">
	import { searchValues } from '$lib/api';
	import type { SearchResult } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';
	import ScopeBadge from '$lib/ScopeBadge.svelte';
	import BrowsePane from '$lib/BrowsePane.svelte';

	let query = $state('');
	let results: SearchResult[] = $state([]);
	let searched = $state(false);
	let error: string | null = $state(null);

	// Selecting a result drives the embedded browse pane instead of navigating
	// away. `target` is the path we hand the pane; `selected` mirrors the pane's
	// own selection so the active result stays highlighted.
	let target: string | null = $state(null);
	let selected: string | null = $state(null);

	async function handleSearch() {
		if (!query.trim()) return;
		searched = true;
		error = null;
		try {
			results = await searchValues(branchStore.current, query);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Search failed';
			results = [];
		}
	}

	function openInBrowser(path: string) {
		// Reassign even when the path is unchanged so re-clicking re-selects.
		target = null;
		target = path;
	}
</script>

<h2>Search Memory <ScopeBadge branch /></h2>

<form onsubmit={handleSearch} class="search-form">
	<input
		type="text"
		bind:value={query}
		placeholder="Search memories... (e.g., licensing, architecture)"
	/>
	<button type="submit">Search</button>
</form>

{#if error}
	<p class="error">{error}</p>
{/if}

<div class="split">
	<div class="results-col">
		{#if searched}
			<p class="count">{results.length} result{results.length !== 1 ? 's' : ''}</p>
			<div class="results">
				{#each results as result}
					<button
						class="result"
						class:selected={selected === result.path}
						onclick={() => openInBrowser(result.path)}
					>
						<div class="result-path">{result.path}</div>
						<div class="result-value">{result.value}</div>
					</button>
				{/each}
			</div>
		{:else}
			<p class="hint">Search memories, then click a result to open it in the browser →</p>
		{/if}
	</div>

	<div class="browse-col">
		<BrowsePane bind:target onselect={(p) => (selected = p)} />
	</div>
</div>

<style>
	.search-form {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 1.5rem;
	}

	input {
		flex: 1;
		padding: 0.75rem 1rem;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text-0);
		font-size: 1rem;
	}

	input:focus {
		outline: none;
		border-color: var(--border-hi);
	}

	button {
		padding: 0.75rem 1.5rem;
		background: var(--accent);
		border: none;
		border-radius: 6px;
		color: var(--text-0);
		cursor: pointer;
		font-size: 1rem;
	}

	button:hover { background: color-mix(in srgb, var(--accent) 80%, black); }

	.split {
		display: grid;
		grid-template-columns: minmax(240px, 340px) 1fr;
		gap: 1.25rem;
		align-items: start;
	}

	@media (max-width: 900px) {
		.split { grid-template-columns: 1fr; }
	}

	.count { color: var(--text-3); margin-bottom: 1rem; }
	.hint { color: var(--text-3); font-size: 0.9rem; }
	.error { color: var(--danger); }

	.results {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow: hidden;
	}

	.result {
		display: block;
		width: 100%;
		text-align: left;
		background: none;
		border: none;
		border-bottom: 1px solid var(--bg-hover);
		padding: 0.75rem 1rem;
		cursor: pointer;
		border-radius: 0;
	}

	.result:hover { background: var(--bg-hover); }
	.result.selected { background: var(--bg-active); }
	.result:last-child { border-bottom: none; }

	.result-path {
		font-family: monospace;
		font-size: 0.8rem;
		color: var(--accent);
		margin-bottom: 0.25rem;
	}

	.result-value {
		color: var(--text-1);
		font-size: 0.9rem;
	}
</style>
