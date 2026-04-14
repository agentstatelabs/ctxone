<script lang="ts">
	import { onMount } from 'svelte';
	import { listPaths, getState } from '$lib/api';

	let paths: string[] = $state([]);
	let selectedPath: string | null = $state(null);
	let selectedValue: unknown = $state(null);
	let error: string | null = $state(null);

	onMount(async () => {
		try {
			paths = await listPaths();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load paths';
		}
	});

	async function selectPath(path: string) {
		selectedPath = path;
		try {
			selectedValue = await getState('main', path);
		} catch (e) {
			selectedValue = null;
			error = e instanceof Error ? e.message : 'Failed to load value';
		}
	}
</script>

<h2>Browse Memory</h2>

{#if error}
	<p class="error">{error}</p>
{/if}

<div class="browser">
	<div class="path-list">
		{#each paths as path}
			<button
				class="path-item"
				class:selected={selectedPath === path}
				onclick={() => selectPath(path)}
			>
				{path}
			</button>
		{/each}
		{#if paths.length === 0}
			<p class="empty">No paths found. Start by remembering something.</p>
		{/if}
	</div>

	<div class="detail">
		{#if selectedPath}
			<h3>{selectedPath}</h3>
			<pre>{JSON.stringify(selectedValue, null, 2)}</pre>
		{:else}
			<p class="hint">Select a path to view its value</p>
		{/if}
	</div>
</div>

<style>
	.browser {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1rem;
		height: calc(100vh - 10rem);
	}

	.path-list {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		overflow-y: auto;
		padding: 0.5rem;
	}

	.path-item {
		display: block;
		width: 100%;
		text-align: left;
		background: none;
		border: none;
		color: #ccc;
		padding: 0.5rem 0.75rem;
		cursor: pointer;
		border-radius: 4px;
		font-family: monospace;
		font-size: 0.85rem;
	}

	.path-item:hover { background: #1a1a1a; }
	.path-item.selected { background: #1e3a5f; color: #fff; }

	.detail {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		padding: 1rem;
		overflow-y: auto;
	}

	pre {
		color: #a5d6a7;
		font-size: 0.85rem;
		white-space: pre-wrap;
		word-break: break-word;
	}

	.error { color: #ef4444; }
	.empty, .hint { color: #555; }
</style>
