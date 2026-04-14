<script lang="ts">
	import { searchValues } from '$lib/api';
	import type { SearchResult } from '$lib/api';

	let query = $state('');
	let results: SearchResult[] = $state([]);
	let searched = $state(false);
	let error: string | null = $state(null);

	async function handleSearch() {
		if (!query.trim()) return;
		searched = true;
		error = null;
		try {
			results = await searchValues('main', query);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Search failed';
			results = [];
		}
	}
</script>

<h2>Search Memory</h2>

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
		background: #111;
		border: 1px solid #333;
		border-radius: 6px;
		color: #fff;
		font-size: 1rem;
	}

	input:focus {
		outline: none;
		border-color: #3b82f6;
	}

	button {
		padding: 0.75rem 1.5rem;
		background: #3b82f6;
		border: none;
		border-radius: 6px;
		color: #fff;
		cursor: pointer;
		font-size: 1rem;
	}

	button:hover { background: #2563eb; }

	.count { color: #666; margin-bottom: 1rem; }
	.error { color: #ef4444; }

	.results {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		overflow: hidden;
	}

	.result {
		padding: 0.75rem 1rem;
		border-bottom: 1px solid #1a1a1a;
	}

	.result:last-child { border-bottom: none; }

	.result-path {
		font-family: monospace;
		font-size: 0.8rem;
		color: #3b82f6;
		margin-bottom: 0.25rem;
	}

	.result-value {
		color: #ccc;
		font-size: 0.9rem;
	}
</style>
