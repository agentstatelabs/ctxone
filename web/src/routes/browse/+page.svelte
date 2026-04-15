<script lang="ts">
	import { onMount } from 'svelte';
	import { listPaths, getState, getBlame, forget } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';

	let paths: string[] = $state([]);
	let selectedPath: string | null = $state(null);
	let selectedValue: unknown = $state(null);
	let blame: unknown = $state(null);
	let error: string | null = $state(null);
	let forgetting = $state(false);
	let forgetMessage: string | null = $state(null);

	async function loadPaths() {
		error = null;
		try {
			paths = await listPaths(branchStore.current);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load paths';
			paths = [];
		}
	}

	onMount(loadPaths);

	// Reload when the branch changes
	$effect(() => {
		// Track branch reactively
		void branchStore.current;
		selectedPath = null;
		selectedValue = null;
		blame = null;
		forgetMessage = null;
		loadPaths();
	});

	async function selectPath(path: string) {
		selectedPath = path;
		forgetMessage = null;
		try {
			selectedValue = await getState(branchStore.current, path);
		} catch (e) {
			selectedValue = null;
			error = e instanceof Error ? e.message : 'Failed to load value';
		}
		try {
			blame = await getBlame(branchStore.current, path);
		} catch {
			blame = null;
		}
	}

	async function handleForget() {
		if (!selectedPath) return;
		if (!confirm(`Forget ${selectedPath}? This writes a rollback commit you can see in history.`)) {
			return;
		}
		forgetting = true;
		forgetMessage = null;
		try {
			await forget({
				path: selectedPath,
				reason: 'forgotten via Lens browse',
				ref: branchStore.current
			});
			forgetMessage = `Forgot ${selectedPath}`;
			const forgotten = selectedPath;
			selectedPath = null;
			selectedValue = null;
			blame = null;
			await loadPaths();
			// Keep the message after paths reload
			forgetMessage = `Forgot ${forgotten}`;
		} catch (e) {
			forgetMessage = e instanceof Error ? e.message : 'Forget failed';
		} finally {
			forgetting = false;
		}
	}

	// Pretty-print blame which may be an array, object, or null
	function formatBlame(b: unknown): string {
		if (b === null || b === undefined) return 'No provenance available';
		return JSON.stringify(b, null, 2);
	}
</script>

<h2>Browse Memory <span class="branch-label">on {branchStore.current}</span></h2>

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
			<p class="empty">No paths found on {branchStore.current}. Start by remembering something.</p>
		{/if}
	</div>

	<div class="detail">
		{#if selectedPath}
			<div class="detail-header">
				<h3>{selectedPath}</h3>
				<button
					type="button"
					class="forget-btn"
					onclick={handleForget}
					disabled={forgetting}
					title="Forget this path (writes a rollback commit)"
				>
					{forgetting ? 'Forgetting…' : 'Forget'}
				</button>
			</div>
			{#if forgetMessage}
				<p class="forget-message">{forgetMessage}</p>
			{/if}
			<h4>Value</h4>
			<pre>{JSON.stringify(selectedValue, null, 2)}</pre>
			<h4>Provenance</h4>
			<pre class="blame">{formatBlame(blame)}</pre>
		{:else}
			<p class="hint">Select a path to view its value and history</p>
		{/if}
	</div>
</div>

<style>
	.branch-label {
		font-size: 0.85rem;
		font-family: monospace;
		color: #3b82f6;
		font-weight: normal;
		margin-left: 0.5rem;
	}

	.browser {
		display: grid;
		grid-template-columns: 1fr 1.3fr;
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
		padding: 1rem 1.25rem;
		overflow-y: auto;
	}

	.detail-header {
		display: flex;
		justify-content: space-between;
		align-items: flex-start;
		gap: 1rem;
	}

	.detail-header h3 {
		margin: 0;
		font-family: monospace;
		font-size: 0.95rem;
		color: #fff;
		word-break: break-all;
	}

	.detail h4 {
		color: #666;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		margin-top: 1rem;
		margin-bottom: 0.4rem;
	}

	.forget-btn {
		background: #3a1a1a;
		border: 1px solid #5a2a2a;
		color: #ef4444;
		padding: 0.3rem 0.75rem;
		border-radius: 4px;
		font-size: 0.8rem;
		cursor: pointer;
		flex-shrink: 0;
	}

	.forget-btn:hover:not(:disabled) {
		background: #4a2020;
		border-color: #7a3030;
	}

	.forget-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.forget-message {
		color: #22c55e;
		font-size: 0.8rem;
		margin: 0.5rem 0 0 0;
		font-family: monospace;
	}

	pre {
		color: #a5d6a7;
		font-size: 0.85rem;
		white-space: pre-wrap;
		word-break: break-word;
		margin: 0;
	}

	pre.blame {
		color: #8b9ab0;
		font-size: 0.78rem;
	}

	.error { color: #ef4444; }
	.empty, .hint { color: #555; }
</style>
