<script lang="ts">
	import { searchValues } from '$lib/api';
	import type { SearchResult } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';
	import ScopeBadge from '$lib/ScopeBadge.svelte';

	let query = $state('');
	let results: SearchResult[] = $state([]);
	let searched = $state(false);
	let error: string | null = $state(null);

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

{#if searched}
	<p class="count">{results.length} result{results.length !== 1 ? 's' : ''}</p>

	<div class="results">
		{#each results as result}
			<div class="result">
				<div class="result-path">{result.path}</div>
				<div class="result-value">{result.value}</div>
			</div>
		{/each}
	</div>
{/if}

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

	.count { color: var(--text-3); margin-bottom: 1rem; }
	.error { color: var(--danger); }


	.results {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow: hidden;
	}

	.result {
		padding: 0.75rem 1rem;
		border-bottom: 1px solid var(--bg-hover);
	}

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
