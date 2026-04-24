<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { getBranches, createBranch } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';

	let { children }: { children: Snippet } = $props();

	let branches: string[] = $state(['main']);
	let newBranchName = $state('');
	let showCreate = $state(false);
	let branchError: string | null = $state(null);

	async function refreshBranches() {
		try {
			const list = await getBranches();
			branches = list.map((b) => b.name);
			if (!branches.includes(branchStore.current)) {
				branchStore.current = 'main';
			}
		} catch (e) {
			// Hub may not be up yet — fail silently, the dashboard will show it
			branches = ['main'];
		}
	}

	onMount(refreshBranches);

	async function handleCreateBranch() {
		const name = newBranchName.trim();
		if (!name) return;
		branchError = null;
		try {
			await createBranch({ name, from: branchStore.current });
			newBranchName = '';
			showCreate = false;
			await refreshBranches();
			branchStore.current = name;
		} catch (e) {
			branchError = e instanceof Error ? e.message : 'Failed to create branch';
		}
	}
</script>

<svelte:head>
	<title>CtxOne Lens</title>
</svelte:head>

<div class="app">
	<nav class="sidebar">
		<div class="logo">
			<h1>CtxOne</h1>
			<span class="subtitle">Lens</span>
		</div>

		<div class="branch-switcher">
			<label for="branch-select">Branch</label>
			<select id="branch-select" bind:value={branchStore.current}>
				{#each branches as name}
					<option value={name}>{name}</option>
				{/each}
			</select>
			<button
				type="button"
				class="new-branch-btn"
				onclick={() => (showCreate = !showCreate)}
				title="Create a new branch"
			>
				+
			</button>
			{#if showCreate}
				<form class="new-branch-form" onsubmit={(e) => { e.preventDefault(); handleCreateBranch(); }}>
					<input
						type="text"
						bind:value={newBranchName}
						placeholder="new branch name"
					/>
					<button type="submit">Create</button>
				</form>
				{#if branchError}
					<p class="branch-error">{branchError}</p>
				{/if}
			{/if}
		</div>

		<ul>
			<li><a href="/">Dashboard</a></li>
			<li><a href="/plans">Plans</a></li>
			<li><a href="/pinned">Pinned</a></li>
			<li><a href="/browse">Browse</a></li>
			<li><a href="/search">Search</a></li>
			<li><a href="/history">History</a></li>
			<li><a href="/diff">Diff</a></li>
			<li><a href="/team">Team</a></li>
		</ul>
	</nav>
	<main>
		{@render children()}
	</main>
</div>

<style>
	:global(body) {
		margin: 0;
		font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
		background: #0a0a0a;
		color: #e0e0e0;
	}

	.app {
		display: flex;
		min-height: 100vh;
	}

	.sidebar {
		width: 220px;
		background: #111;
		border-right: 1px solid #222;
		padding: 1.5rem 1rem;
		flex-shrink: 0;
	}

	.logo h1 {
		margin: 0;
		font-size: 1.4rem;
		color: #fff;
	}

	.subtitle {
		font-size: 0.75rem;
		color: #666;
		text-transform: uppercase;
		letter-spacing: 0.1em;
	}

	.branch-switcher {
		margin-top: 1.5rem;
		padding-bottom: 1rem;
		border-bottom: 1px solid #222;
	}

	.branch-switcher label {
		display: block;
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: #555;
		margin-bottom: 0.35rem;
	}

	.branch-switcher select {
		width: calc(100% - 2.2rem);
		background: #0a0a0a;
		border: 1px solid #333;
		color: #e0e0e0;
		padding: 0.35rem 0.5rem;
		border-radius: 4px;
		font-family: monospace;
		font-size: 0.85rem;
	}

	.new-branch-btn {
		background: #1a1a1a;
		border: 1px solid #333;
		color: #888;
		width: 1.7rem;
		height: 1.7rem;
		border-radius: 4px;
		font-size: 1.1rem;
		line-height: 1;
		cursor: pointer;
		margin-left: 0.2rem;
		vertical-align: middle;
	}

	.new-branch-btn:hover {
		color: #fff;
		border-color: #555;
	}

	.new-branch-form {
		display: flex;
		gap: 0.25rem;
		margin-top: 0.5rem;
	}

	.new-branch-form input {
		flex: 1;
		min-width: 0;
		background: #0a0a0a;
		border: 1px solid #333;
		color: #e0e0e0;
		padding: 0.3rem 0.5rem;
		border-radius: 4px;
		font-size: 0.8rem;
	}

	.new-branch-form button {
		background: #1e3a5f;
		border: 1px solid #2a4a7a;
		color: #93c5fd;
		padding: 0.3rem 0.6rem;
		border-radius: 4px;
		font-size: 0.8rem;
		cursor: pointer;
	}

	.branch-error {
		color: #ef4444;
		font-size: 0.75rem;
		margin: 0.35rem 0 0 0;
	}

	ul {
		list-style: none;
		padding: 0;
		margin-top: 2rem;
	}

	li {
		margin-bottom: 0.5rem;
	}

	a {
		color: #888;
		text-decoration: none;
		padding: 0.5rem 0.75rem;
		display: block;
		border-radius: 6px;
		transition: all 0.15s;
	}

	a:hover {
		color: #fff;
		background: #1a1a1a;
	}

	main {
		flex: 1;
		padding: 2rem;
		overflow-y: auto;
	}
</style>
