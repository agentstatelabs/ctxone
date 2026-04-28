<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { getBranches, createBranch } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';
	import { themeStore, THEMES, type ThemeId } from '$lib/themeStore.svelte';
	import { refreshStore, REFRESH_INTERVAL_MS } from '$lib/refreshStore.svelte';
	import CmdK from '$lib/CmdK.svelte';
	import '../app.css';

	let { children }: { children: Snippet } = $props();

	// Sidebar groups — order is intent-driven, not alphabetical:
	// "Now" = current work, "Memory" = stored knowledge,
	// "Changes" = audit, "Governance" = control surfaces.
	const NAV_GROUPS: { label: string; items: { href: string; label: string }[] }[] = [
		{
			label: 'Now',
			items: [
				{ href: '/', label: 'Dashboard' },
				{ href: '/plans', label: 'Plans' },
				{ href: '/sessions', label: 'Sessions' }
			]
		},
		{
			label: 'Memory',
			items: [
				{ href: '/pinned', label: 'Pinned' },
				{ href: '/browse', label: 'Browse' },
				{ href: '/search', label: 'Search' }
			]
		},
		{
			label: 'Changes',
			items: [
				{ href: '/history', label: 'History' },
				{ href: '/diff', label: 'Diff' }
			]
		},
		{
			label: 'Governance',
			items: [
				{ href: '/branches', label: 'Branches' },
				{ href: '/taint', label: 'Taint' }
			]
		}
	];

	function isActive(href: string, pathname: string): boolean {
		if (href === '/') return pathname === '/';
		return pathname === href || pathname.startsWith(href + '/');
	}

	let cmdkOpen = $state(false);

	// Global Cmd/Ctrl-K: open the palette. Ignore when an input/textarea
	// already has focus and the user is typing a real K — except when
	// they're holding the meta/ctrl modifier, which is unambiguously the
	// shortcut.
	function onGlobalKey(e: KeyboardEvent) {
		if ((e.metaKey || e.ctrlKey) && (e.key === 'k' || e.key === 'K')) {
			e.preventDefault();
			cmdkOpen = !cmdkOpen;
		}
	}

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

<svelte:window onkeydown={onGlobalKey} />

<CmdK bind:open={cmdkOpen} />

<div class="app">
	<nav class="sidebar">
		<div class="logo">
			<h1>CtxOne</h1>
			<span class="subtitle">Lens</span>
			<button
				type="button"
				class="cmdk-hint"
				onclick={() => (cmdkOpen = true)}
				title="Open command palette"
			>
				<kbd>⌘K</kbd>
			</button>
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

		<nav class="nav-groups">
			{#each NAV_GROUPS as group}
				<div class="nav-group">
					<span class="nav-group-label">{group.label}</span>
					<ul>
						{#each group.items as item}
							{@const active = isActive(item.href, $page.url.pathname)}
							<li>
								<a
									href={item.href}
									class:active
									aria-current={active ? 'page' : undefined}
								>
									{item.label}
								</a>
							</li>
						{/each}
					</ul>
				</div>
			{/each}
		</nav>

		<div class="refresh-toggle">
			<label class="refresh-row">
				<input
					type="checkbox"
					checked={refreshStore.enabled}
					onchange={(e) => (refreshStore.enabled = (e.currentTarget as HTMLInputElement).checked)}
				/>
				<span>Auto-refresh</span>
			</label>
			<span class="refresh-hint">every {Math.round(REFRESH_INTERVAL_MS / 1000)}s</span>
		</div>

		<div class="theme-picker">
			<label for="theme-select">Theme</label>
			<select
				id="theme-select"
				value={themeStore.current}
				onchange={(e) => themeStore.set((e.currentTarget as HTMLSelectElement).value as ThemeId)}
			>
				<optgroup label="Dark">
					{#each THEMES.filter((t) => t.group === 'dark') as t}
						<option value={t.id}>{t.label}</option>
					{/each}
				</optgroup>
				<optgroup label="Light">
					{#each THEMES.filter((t) => t.group === 'light') as t}
						<option value={t.id}>{t.label}</option>
					{/each}
				</optgroup>
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

	.cmdk-hint {
		float: right;
		background: transparent;
		border: 1px solid var(--border);
		border-radius: 4px;
		padding: 0.1rem 0.4rem;
		cursor: pointer;
	}
	.cmdk-hint kbd {
		font-family: monospace;
		font-size: 0.7rem;
		color: var(--text-2);
	}
	.cmdk-hint:hover {
		border-color: var(--accent);
	}
	.cmdk-hint:hover kbd {
		color: var(--accent);
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
		padding-top: 1rem;
		border-top: 1px solid var(--border);
	}

	.refresh-toggle {
		margin-top: auto;
		padding-top: 1rem;
		border-top: 1px solid var(--border);
	}

	.refresh-row {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		color: var(--text-2);
		font-size: 0.85rem;
		cursor: pointer;
	}

	.refresh-row input {
		accent-color: var(--accent);
	}

	.refresh-hint {
		display: block;
		font-size: 0.7rem;
		color: var(--text-3);
		margin-top: 0.2rem;
		padding-left: 1.4rem;
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

	.nav-groups {
		margin-top: 1.5rem;
		display: flex;
		flex-direction: column;
		gap: 1.1rem;
	}

	.nav-group-label {
		display: block;
		font-size: 0.65rem;
		text-transform: uppercase;
		letter-spacing: 0.1em;
		color: var(--text-3);
		padding: 0 0.75rem 0.3rem;
	}

	ul {
		list-style: none;
		padding: 0;
		margin: 0;
	}

	li {
		margin-bottom: 0.15rem;
	}

	a {
		color: var(--text-2);
		text-decoration: none;
		padding: 0.4rem 0.75rem;
		display: block;
		border-radius: 6px;
		transition: background 0.12s, color 0.12s;
		font-size: 0.92rem;
		border-left: 2px solid transparent;
	}

	a:hover {
		color: var(--text-0);
		background: var(--bg-hover);
	}

	a.active {
		color: var(--text-0);
		background: var(--accent-bg);
		border-left-color: var(--accent);
	}

	main {
		flex: 1;
		padding: 2rem;
		overflow-y: auto;
	}
</style>
