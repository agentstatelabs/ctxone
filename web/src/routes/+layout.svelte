<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { getBranches, createBranch, listProjects, type Project } from '$lib/api';
	import { getAsdHealth, listAsdRepos, prefetchAsdRepo } from '$lib/codeApi';
	import type { AsdHealth, AsdRepoInfo } from '$lib/codeTypes';
	import { selectedRepo } from '$lib/repoStore';
	import { branchStore } from '$lib/branchStore.svelte';
	import { namespaceStore, DEFAULT_NAMESPACE } from '$lib/namespaceStore.svelte';
	import { themeStore, THEMES, type ThemeId } from '$lib/themeStore.svelte';
	import { refreshStore, REFRESH_INTERVAL_MS } from '$lib/refreshStore.svelte';
	import CmdK from '$lib/CmdK.svelte';
	import '../app.css';

	let { children }: { children: Snippet } = $props();

	// Sidebar layout — top-level by *which backend serves it*:
	// CtxOne (memory + plans + branches; this hub's own data) vs.
	// ASD (code-intelligence proxied to per-repo asd-serve children).
	// The ASD section gets the repo picker; the CtxOne section gets the
	// branch picker. Inside each section, groups stay intent-driven.
	type NavItem = { href: string; label: string };
	type NavGroup = { label: string; items: NavItem[] };
	type NavSection = { label: string; groups: NavGroup[] };

	const NAV_SECTIONS: NavSection[] = [
		{
			label: 'CtxOne',
			groups: [
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
						{ href: '/projects', label: 'Projects' },
						{ href: '/branches', label: 'Branches' },
						{ href: '/taint', label: 'Taint' }
					]
				}
			]
		},
		{
			label: 'ASD',
			groups: [
				{
					label: 'Code',
					items: [
						{ href: '/code', label: 'Overview' },
						{ href: '/code/search', label: 'Search' },
						{ href: '/code/symbols', label: 'Symbols' },
						{ href: '/code/graph', label: 'Graph' },
						{ href: '/code/files', label: 'Files' }
					]
				},
				{
					label: 'Reasoning',
					items: [{ href: '/code/thinking', label: 'Thinking' }]
				}
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

	let asdRepos = $state<AsdRepoInfo[]>([]);
	let asdHealth = $state<AsdHealth | null>(null);

	let branches: string[] = $state(['main']);
	let newBranchName = $state('');
	let showCreate = $state(false);
	let branchError: string | null = $state(null);

	let projects = $state<Project[]>([]);

	async function loadProjects() {
		try {
			projects = await listProjects();
			namespaceStore.hydrate([
				DEFAULT_NAMESPACE,
				...projects.map((p) => p.namespace)
			]);
		} catch {
			projects = [];
		}
	}

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

	async function loadAsdRepos() {
		const repos = await listAsdRepos();
		asdRepos = repos;
		// Auto-select: restore from localStorage, else pick first repo.
		const saved = localStorage.getItem('ctxone_asd_repo');
		const initial = repos.find((r) => r.name === saved) ?? repos[0];
		if (initial) $selectedRepo = initial.name;
		// Load health for the selected repo.
		if ($selectedRepo) getAsdHealth($selectedRepo).then((h) => (asdHealth = h));
	}

	onMount(() => {
		themeStore.hydrate();
		loadProjects();
		loadAsdRepos();
	});

	// (Re)load the branch list on mount and whenever the namespace
	// changes — branch refs are namespace-scoped.
	$effect(() => {
		void namespaceStore.current;
		refreshBranches();
	});

	// Persist selection and reload health whenever the repo changes.
	// Also fire a prefetch so pool-managed repos warm before /code is hit.
	$effect(() => {
		const repo = $selectedRepo;
		if (!repo) return;
		localStorage.setItem('ctxone_asd_repo', repo);
		asdHealth = null;
		prefetchAsdRepo(repo)
			.then(() => listAsdRepos())
			.then((repos) => (asdRepos = repos));
		getAsdHealth(repo).then((h) => (asdHealth = h));
	});

	// Repo currently bound to the picker, with its live status (running/idle).
	let selectedRepoInfo = $derived(asdRepos.find((r) => r.name === $selectedRepo));

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

		<nav class="nav-sections">
			{#each NAV_SECTIONS as section}
				<div class="nav-section nav-section-{section.label.toLowerCase()}">
					<h2 class="nav-section-label">{section.label}</h2>

					{#if section.label === 'CtxOne'}
						<div class="section-picker namespace-switcher">
							<label for="namespace-select">Project</label>
							<select
								id="namespace-select"
								value={namespaceStore.current}
								onchange={(e) =>
									(namespaceStore.current = (e.currentTarget as HTMLSelectElement).value)}
							>
								<option value={DEFAULT_NAMESPACE}>default</option>
								{#each projects as p}
									<option value={p.namespace}>{p.display_name ?? p.id}</option>
								{/each}
							</select>
						</div>
						<div class="section-picker branch-switcher">
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
								{showCreate ? '− Cancel' : '+ New branch'}
							</button>
							{#if showCreate}
								<form
									class="new-branch-form"
									onsubmit={(e) => {
										e.preventDefault();
										handleCreateBranch();
									}}
								>
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
					{/if}

					{#if section.label === 'ASD' && asdRepos.length > 0}
						<div class="section-picker repo-selector">
							<label for="repo-select" class="repo-label">Repo</label>
							<select
								id="repo-select"
								value={$selectedRepo}
								onchange={(e) =>
									($selectedRepo = (e.currentTarget as HTMLSelectElement).value)}
							>
								{#each asdRepos as r}
									<option value={r.name}>
										{r.status === 'idle' ? '○' : '●'} {r.name}
									</option>
								{/each}
							</select>
							<span
								class="asd-dot"
								class:idle={selectedRepoInfo?.status === 'idle'}
								title={selectedRepoInfo?.status === 'idle'
									? 'idle (not yet spawned)'
									: 'running'}
							></span>
							{#if asdHealth}
								<span class="repo-health">
									{asdHealth.symbol_count.toLocaleString()} symbols
								</span>
							{/if}
						</div>
					{/if}

					<div class="nav-groups">
						{#each section.groups as group}
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
					</div>
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

	.repo-selector {
		margin-top: 0.75rem;
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}

	.repo-label {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.07em;
		color: var(--text-3);
	}

	.repo-selector select {
		width: 100%;
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--text-1);
		padding: 0.3rem 0.5rem;
		font-size: 0.82rem;
		font-family: monospace;
	}

	.repo-health {
		display: flex;
		align-items: center;
		gap: 0.35rem;
		font-size: 0.7rem;
		color: var(--accent);
		padding: 0 0.1rem;
	}

	.asd-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--accent);
		flex-shrink: 0;
	}
	.asd-dot.idle {
		background: transparent;
		border: 1px solid var(--border);
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

	.namespace-switcher {
		margin-top: 1.5rem;
	}

	.branch-switcher {
		margin-top: 0.75rem;
		padding-bottom: 1rem;
		border-bottom: 1px solid var(--border);
	}

	.namespace-switcher label,
	.branch-switcher label,
	.theme-picker label {
		display: block;
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-3);
		margin-bottom: 0.35rem;
	}

	.namespace-switcher select,
	.branch-switcher select,
	.theme-picker select {
		width: 100%;
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.35rem 0.5rem;
		border-radius: 4px;
		font-family: monospace;
		font-size: 0.85rem;
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
		display: block;
		width: 100%;
		background: transparent;
		border: 1px dashed var(--border);
		color: var(--text-2);
		padding: 0.35rem 0.5rem;
		border-radius: 4px;
		font-size: 0.78rem;
		line-height: 1;
		cursor: pointer;
		margin-top: 0.5rem;
		text-align: center;
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

	.nav-sections {
		margin-top: 1rem;
		display: flex;
		flex-direction: column;
		gap: 1.25rem;
	}

	.nav-section + .nav-section {
		border-top: 1px solid var(--border);
		padding-top: 1.1rem;
	}

	.nav-section-label {
		display: block;
		font-size: 0.7rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--text-2);
		margin: 0 0 0.6rem;
		padding: 0 0.75rem;
	}

	.section-picker {
		margin: 0 0.5rem 0.85rem;
	}

	.nav-groups {
		margin-top: 0.25rem;
		display: flex;
		flex-direction: column;
		gap: 1rem;
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
