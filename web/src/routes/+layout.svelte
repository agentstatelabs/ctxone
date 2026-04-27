<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { getBranches, createBranch } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';
	import { themeStore, THEMES, type ThemeId } from '$lib/themeStore.svelte';
	import '../app.css';

	let { children }: { children: Snippet } = $props();

	let branches: string[] = $state(['main']);
	let newBranchName = $state('');
	let showCreate = $state(false);
	let branchError: string | null = $state(null);

	async function refreshBranches() {
		try {
			const list = await getBranches();
			const names = list.map((b) => b.name);
			names.sort((a, b) => (a === 'main' ? -1 : b === 'main' ? 1 : a.localeCompare(b)));
			branches = names;
			branchStore.hydrate(names);
		} catch {
			branches = ['main'];
		}
	}

	onMount(() => {
		themeStore.hydrate();
		refreshBranches();
	});

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
				<form
					class="new-branch-form"
					onsubmit={(e) => {
						e.preventDefault();
						handleCreateBranch();
					}}
				>
					<input type="text" bind:value={newBranchName} placeholder="new branch name" />
					<button type="submit">Create</button>
				</form>
				{#if branchError}
					<p class="branch-error">{branchError}</p>
				{/if}
			{/if}
		</div>

		<ul>
			<li><a href="/">Dashboard</a></li>
			<li><a href="/sessions">Sessions</a></li>
			<li><a href="/branches">Branches</a></li>
			<li><a href="/plans">Plans</a></li>
			<li><a href="/pinned">Pinned</a></li>
			<li><a href="/browse">Browse</a></li>
			<li><a href="/search">Search</a></li>
			<li><a href="/history">History</a></li>
			<li><a href="/diff">Diff</a></li>
			<li><a href="/taint">Taint</a></li>
		</ul>

		<div class="theme-picker">
			<label for="theme-select">Theme</label>
			<select
				id="theme-select"
				value={themeStore.current}
				onchange={(e) => themeStore.set((e.currentTarget as HTMLSelectElement).value as ThemeId)}
			>
				{#each THEMES as t}
					<option value={t.id}>{t.label}</option>
				{/each}
			</select>
		</div>
	</nav>
	<main>
		{@render children()}
	</main>
</div>

<style>
	.app {
		display: flex;
		min-height: 100vh;
	}

	.sidebar {
		width: 220px;
		background: var(--bg-1);
		border-right: 1px solid var(--border);
		padding: 1.5rem 1rem;
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
	}

	.logo h1 {
		margin: 0;
		font-size: 1.4rem;
		color: var(--text-0);
	}

	.subtitle {
		font-size: 0.75rem;
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.1em;
	}

	.branch-switcher {
		margin-top: 1.5rem;
		padding-bottom: 1rem;
		border-bottom: 1px solid var(--border);
	}

	.branch-switcher label,
	.theme-picker label {
		display: block;
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-3);
		margin-bottom: 0.35rem;
	}

	.branch-switcher select,
	.theme-picker select {
		width: calc(100% - 2.2rem);
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.35rem 0.5rem;
		border-radius: 4px;
		font-family: monospace;
		font-size: 0.85rem;
	}

	.theme-picker select {
		width: 100%;
	}

	.theme-picker {
		margin-top: auto;
		padding-top: 1rem;
		border-top: 1px solid var(--border);
	}

	.new-branch-btn {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		color: var(--text-2);
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
		color: var(--text-0);
		border-color: var(--text-3);
	}

	.new-branch-form {
		display: flex;
		gap: 0.25rem;
		margin-top: 0.5rem;
	}

	.new-branch-form input {
		flex: 1;
		min-width: 0;
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.3rem 0.5rem;
		border-radius: 4px;
		font-size: 0.8rem;
	}

	.new-branch-form button {
		background: var(--accent-bg);
		border: 1px solid var(--accent-bg-hi);
		color: var(--accent);
		padding: 0.3rem 0.6rem;
		border-radius: 4px;
		font-size: 0.8rem;
		cursor: pointer;
	}

	.branch-error {
		color: var(--danger);
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
		color: var(--text-2);
		text-decoration: none;
		padding: 0.5rem 0.75rem;
		display: block;
		border-radius: 6px;
		transition: all 0.15s;
	}

	a:hover {
		color: var(--text-0);
		background: var(--bg-hover);
	}

	main {
		flex: 1;
		padding: 2rem;
		overflow-y: auto;
	}
</style>
