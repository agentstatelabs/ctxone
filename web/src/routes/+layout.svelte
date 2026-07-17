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
	import '@agentstate/lens-core/tokens.css';
	import '../app.css';

	let { children }: { children: Snippet } = $props();

	// ── Information architecture ─────────────────────────────────────────────
	// The namespace (workspace) is the spine: a workspace switcher sits at the
	// very top of the sidebar, a branch context pill directly under it, and
	// everything below is implicitly scoped to workspace → branch. Nav is a
	// single flat set of intent groups (no more CtxOne/ASD split). The Code
	// group carries the ASD repo picker in its header, since code intelligence
	// is proxied per-repo and is orthogonal to the CtxOne namespace/branch.
	type NavItem = { href: string; label: string };
	type NavGroup = { label: string; items: NavItem[]; key?: 'code' };

	const NAV_GROUPS: NavGroup[] = [
		{ label: 'Home', items: [{ href: '/', label: 'Dashboard' }] },
		{
			label: 'Work',
			items: [
				{ href: '/plans', label: 'Plans' },
				{ href: '/reminders', label: 'Reminders' },
				{ href: '/sessions', label: 'Sessions' }
			]
		},
		{
			label: 'Memory',
			items: [
				{ href: '/browse', label: 'Browse' },
				{ href: '/pinned', label: 'Pinned' },
				{ href: '/search', label: 'Search' },
				{ href: '/recall', label: 'Recall' },
				{ href: '/why', label: 'Why' }
			]
		},
		{
			label: 'Activity',
			items: [
				{ href: '/history', label: 'History' },
				{ href: '/tail', label: 'Live Tail' },
				{ href: '/diff', label: 'Diff' }
			]
		},
		{
			label: 'Code',
			key: 'code',
			items: [
				{ href: '/code', label: 'Overview' },
				{ href: '/code/search', label: 'Search' },
				{ href: '/code/symbols', label: 'Symbols' },
				{ href: '/code/graph', label: 'Graph' },
				{ href: '/code/files', label: 'Files' },
				{ href: '/code/thinking', label: 'Thinking' }
			]
		},
		{
			label: 'Settings',
			items: [
				{ href: '/projects', label: 'Workspaces' },
				{ href: '/branches', label: 'Branches' },
				{ href: '/taint', label: 'Taint' }
			]
		}
	];

	// Active nav = the single item whose href is the longest prefix of the
	// current path. Longest-prefix resolves the /code vs /code/search overlap
	// (plain startsWith would light up "Overview" on every code subpage).
	const ALL_HREFS = NAV_GROUPS.flatMap((g) => g.items.map((i) => i.href));
	function computeActive(pathname: string): string | null {
		let best: string | null = null;
		for (const href of ALL_HREFS) {
			const matches =
				href === '/' ? pathname === '/' : pathname === href || pathname.startsWith(href + '/');
			if (matches && (best === null || href.length > best.length)) best = href;
		}
		return best;
	}
	let activeHref = $derived(computeActive($page.url.pathname));

	let cmdkOpen = $state(false);

	// Global Cmd/Ctrl-K: open the palette. Escape also dismisses the open
	// workspace menu.
	function onGlobalKey(e: KeyboardEvent) {
		if ((e.metaKey || e.ctrlKey) && (e.key === 'k' || e.key === 'K')) {
			e.preventDefault();
			cmdkOpen = !cmdkOpen;
		} else if (e.key === 'Escape' && wsOpen) {
			wsOpen = false;
		}
	}

	let asdRepos = $state<AsdRepoInfo[]>([]);
	let asdHealth = $state<AsdHealth | null>(null);

	let branches: string[] = $state(['main']);
	let newBranchName = $state('');
	let showCreate = $state(false);
	let branchError: string | null = $state(null);

	let projects = $state<Project[]>([]);

	// ── Workspace (namespace) switcher ───────────────────────────────────────
	let wsOpen = $state(false);
	type WorkspaceOption = { namespace: string; label: string };
	let workspaceOptions = $derived<WorkspaceOption[]>([
		{ namespace: DEFAULT_NAMESPACE, label: 'default' },
		...projects.map((p) => ({ namespace: p.namespace, label: p.display_name ?? p.id }))
	]);
	let currentWorkspace = $derived(
		workspaceOptions.find((o) => o.namespace === namespaceStore.current)?.label ??
			namespaceStore.current
	);
	let workspaceGlyph = $derived((currentWorkspace[0] ?? '·').toUpperCase());

	function selectWorkspace(ns: string) {
		namespaceStore.current = ns; // resets branch → main (see namespaceStore)
		wsOpen = false;
	}

	async function loadProjects() {
		try {
			projects = await listProjects();
			namespaceStore.hydrate([DEFAULT_NAMESPACE, ...projects.map((p) => p.namespace)]);
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
		<div class="brand">
			<div class="brand-mark">
				<span class="brand-name">CtxOne</span>
				<span class="brand-sub">Lens</span>
			</div>
			<button
				type="button"
				class="cmdk-hint"
				onclick={() => (cmdkOpen = true)}
				title="Open command palette"
			>
				<kbd>⌘K</kbd>
			</button>
		</div>

		<!-- ── Workspace spine: namespace switcher + branch context ──────────── -->
		<div class="spine">
			<div class="workspace">
				<button
					type="button"
					class="ws-trigger"
					class:open={wsOpen}
					onclick={() => (wsOpen = !wsOpen)}
					aria-haspopup="menu"
					aria-expanded={wsOpen}
					title="Switch workspace"
				>
					<span class="ws-glyph">{workspaceGlyph}</span>
					<span class="ws-meta">
						<span class="ws-eyebrow">Workspace</span>
						<span class="ws-name">{currentWorkspace}</span>
					</span>
					<span class="ws-caret" aria-hidden="true">⌄</span>
				</button>

				{#if wsOpen}
					<div
						class="ws-backdrop"
						role="presentation"
						onclick={() => (wsOpen = false)}
					></div>
					<div class="ws-menu" role="menu">
						<p class="ws-menu-label">Switch workspace</p>
						{#each workspaceOptions as opt}
							<button
								type="button"
								role="menuitemradio"
								aria-checked={opt.namespace === namespaceStore.current}
								class="ws-option"
								class:selected={opt.namespace === namespaceStore.current}
								onclick={() => selectWorkspace(opt.namespace)}
							>
								<span class="ws-option-glyph">{(opt.label[0] ?? '·').toUpperCase()}</span>
								<span class="ws-option-name">{opt.label}</span>
								{#if opt.namespace === namespaceStore.current}
									<span class="ws-check" aria-hidden="true">✓</span>
								{/if}
							</button>
						{/each}
					</div>
				{/if}
			</div>

			<div class="branch">
				<span class="branch-eyebrow">Branch</span>
				<div class="branch-row">
					<span class="branch-glyph" aria-hidden="true">⑂</span>
					<select
						class="branch-select"
						id="branch-select"
						bind:value={branchStore.current}
						title="Active branch (scoped to workspace)"
					>
						{#each branches as name}
							<option value={name}>{name}</option>
						{/each}
					</select>
					<button
						type="button"
						class="branch-new"
						onclick={() => (showCreate = !showCreate)}
						title="Create a new branch"
						aria-label="Create a new branch"
					>
						{showCreate ? '×' : '+'}
					</button>
				</div>
				{#if showCreate}
					<form
						class="branch-form"
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
		</div>

		<!-- ── Flat nav groups, scoped to workspace → branch ─────────────────── -->
		<nav class="nav-groups">
			{#each NAV_GROUPS as group}
				<div class="nav-group">
					<div class="nav-group-head">
						<span class="nav-group-label">{group.label}</span>
					</div>

					{#if group.key === 'code' && asdRepos.length > 0}
						<div class="repo-picker">
							<select
								class="repo-select"
								id="repo-select"
								value={$selectedRepo}
								onchange={(e) =>
									($selectedRepo = (e.currentTarget as HTMLSelectElement).value)}
								title="ASD code-intelligence repo"
							>
								{#each asdRepos as r}
									<option value={r.name}>
										{r.status === 'idle' ? '○' : '●'} {r.name}
									</option>
								{/each}
							</select>
							<div class="repo-meta">
								<span
									class="repo-dot"
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
						</div>
					{/if}

					<ul>
						{#each group.items as item}
							{@const active = item.href === activeHref}
							<li>
								<a href={item.href} class:active aria-current={active ? 'page' : undefined}>
									{item.label}
								</a>
							</li>
						{/each}
					</ul>
				</div>
			{/each}
		</nav>

		<div class="sidebar-footer">
			<div class="refresh-toggle">
				<label class="refresh-row">
					<input
						type="checkbox"
						checked={refreshStore.enabled}
						onchange={(e) =>
							(refreshStore.enabled = (e.currentTarget as HTMLInputElement).checked)}
					/>
					<span>Auto-refresh</span>
					<span class="refresh-hint">{Math.round(REFRESH_INTERVAL_MS / 1000)}s</span>
				</label>
			</div>

			<div class="theme-picker">
				<label for="theme-select">Theme</label>
				<select
					id="theme-select"
					value={themeStore.current}
					onchange={(e) =>
						themeStore.set((e.currentTarget as HTMLSelectElement).value as ThemeId)}
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
		background: var(--lens-bg);
	}

	.sidebar {
		width: 240px;
		background: var(--lens-surface);
		border-right: 1px solid var(--lens-border);
		padding: var(--lens-space-5) var(--lens-space-3);
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-5);
	}

	/* ── Brand ─────────────────────────────────────────────────────────────── */
	.brand {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0 var(--lens-space-2);
	}

	.brand-mark {
		display: flex;
		align-items: baseline;
		gap: var(--lens-space-2);
	}

	.brand-name {
		font-size: var(--lens-font-size-lg);
		font-weight: 700;
		color: var(--lens-text-strong);
		letter-spacing: -0.01em;
	}

	.brand-sub {
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}

	.cmdk-hint {
		background: transparent;
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		padding: 0.1rem 0.4rem;
		cursor: pointer;
		line-height: 1;
		transition: border-color var(--lens-dur-fast) var(--lens-ease);
	}
	.cmdk-hint kbd {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-text-secondary);
	}
	.cmdk-hint:hover {
		border-color: var(--lens-accent-border);
	}
	.cmdk-hint:hover kbd {
		color: var(--lens-accent);
	}

	/* ── Workspace spine ───────────────────────────────────────────────────── */
	.spine {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-2);
		padding-bottom: var(--lens-space-4);
		border-bottom: 1px solid var(--lens-border-subtle);
	}

	.workspace {
		position: relative;
	}

	.ws-trigger {
		display: flex;
		align-items: center;
		gap: var(--lens-space-3);
		width: 100%;
		background: var(--lens-surface-raised);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-md);
		padding: var(--lens-space-2) var(--lens-space-3);
		cursor: pointer;
		text-align: left;
		transition:
			border-color var(--lens-dur-fast) var(--lens-ease),
			background var(--lens-dur-fast) var(--lens-ease);
	}
	.ws-trigger:hover,
	.ws-trigger.open {
		border-color: var(--lens-border-strong);
	}

	.ws-glyph {
		display: grid;
		place-items: center;
		width: 28px;
		height: 28px;
		flex-shrink: 0;
		border-radius: var(--lens-radius-sm);
		background: var(--lens-accent-surface);
		color: var(--lens-accent-hover);
		font-weight: 700;
		font-size: var(--lens-font-size-sm);
	}

	.ws-meta {
		display: flex;
		flex-direction: column;
		min-width: 0;
		flex: 1;
	}

	.ws-eyebrow {
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
		line-height: 1.2;
	}

	.ws-name {
		font-size: var(--lens-font-size-sm);
		font-weight: 600;
		color: var(--lens-text-strong);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.ws-caret {
		color: var(--lens-muted);
		font-size: var(--lens-font-size-md);
		line-height: 1;
		flex-shrink: 0;
	}

	.ws-backdrop {
		position: fixed;
		inset: 0;
		z-index: 40;
	}

	.ws-menu {
		position: absolute;
		top: calc(100% + var(--lens-space-2));
		left: 0;
		right: 0;
		z-index: 50;
		background: var(--lens-overlay);
		border: 1px solid var(--lens-border-strong);
		border-radius: var(--lens-radius-md);
		box-shadow: var(--lens-shadow-lg);
		padding: var(--lens-space-1);
		display: flex;
		flex-direction: column;
		gap: 1px;
	}

	.ws-menu-label {
		margin: 0;
		padding: var(--lens-space-2) var(--lens-space-2) var(--lens-space-1);
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}

	.ws-option {
		display: flex;
		align-items: center;
		gap: var(--lens-space-2);
		width: 100%;
		background: transparent;
		border: 0;
		border-radius: var(--lens-radius-sm);
		padding: var(--lens-space-2);
		cursor: pointer;
		text-align: left;
		color: var(--lens-text);
		font-size: var(--lens-font-size-sm);
		transition: background var(--lens-dur-fast) var(--lens-ease);
	}
	.ws-option:hover {
		background: var(--lens-surface-raised);
		color: var(--lens-text-strong);
	}
	.ws-option.selected {
		color: var(--lens-text-strong);
	}

	.ws-option-glyph {
		display: grid;
		place-items: center;
		width: 22px;
		height: 22px;
		flex-shrink: 0;
		border-radius: var(--lens-radius-sm);
		background: var(--lens-surface-raised);
		color: var(--lens-text-secondary);
		font-weight: 700;
		font-size: var(--lens-font-size-2xs);
	}
	.ws-option.selected .ws-option-glyph {
		background: var(--lens-accent-surface);
		color: var(--lens-accent-hover);
	}

	.ws-option-name {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.ws-check {
		color: var(--lens-accent);
		font-size: var(--lens-font-size-xs);
	}

	/* ── Branch context ────────────────────────────────────────────────────── */
	.branch {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-1);
		padding: 0 var(--lens-space-1);
	}

	.branch-eyebrow {
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}

	.branch-row {
		display: flex;
		align-items: center;
		gap: var(--lens-space-2);
	}

	.branch-glyph {
		color: var(--lens-text-secondary);
		font-size: var(--lens-font-size-sm);
	}

	.branch-select {
		flex: 1;
		min-width: 0;
		background: var(--lens-surface-raised);
		border: 1px solid var(--lens-border);
		color: var(--lens-text);
		padding: 0.3rem var(--lens-space-2);
		border-radius: var(--lens-radius-sm);
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
	}

	.branch-new {
		display: grid;
		place-items: center;
		width: 26px;
		height: 26px;
		flex-shrink: 0;
		background: transparent;
		border: 1px solid var(--lens-border);
		color: var(--lens-text-secondary);
		border-radius: var(--lens-radius-sm);
		font-size: var(--lens-font-size-md);
		line-height: 1;
		cursor: pointer;
		transition:
			border-color var(--lens-dur-fast) var(--lens-ease),
			color var(--lens-dur-fast) var(--lens-ease);
	}
	.branch-new:hover {
		color: var(--lens-text-strong);
		border-color: var(--lens-border-strong);
	}

	.branch-form {
		display: flex;
		gap: var(--lens-space-1);
		margin-top: var(--lens-space-1);
	}
	.branch-form input {
		flex: 1;
		min-width: 0;
		background: var(--lens-surface-raised);
		border: 1px solid var(--lens-border);
		color: var(--lens-text);
		padding: 0.3rem var(--lens-space-2);
		border-radius: var(--lens-radius-sm);
		font-size: var(--lens-font-size-xs);
	}
	.branch-form button {
		background: var(--lens-accent-surface);
		border: 1px solid var(--lens-accent-border);
		color: var(--lens-accent-hover);
		padding: 0.3rem var(--lens-space-3);
		border-radius: var(--lens-radius-sm);
		font-size: var(--lens-font-size-xs);
		cursor: pointer;
	}
	.branch-error {
		color: var(--lens-danger);
		font-size: var(--lens-font-size-xs);
		margin: var(--lens-space-1) 0 0;
	}

	/* ── Nav groups ────────────────────────────────────────────────────────── */
	.nav-groups {
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-5);
		overflow-y: auto;
	}

	.nav-group-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0 var(--lens-space-2) var(--lens-space-1);
	}

	.nav-group-label {
		font-size: var(--lens-font-size-2xs);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}
	/* Per-group hue tints (source order: Home, Work, Memory, Activity,
	   Code, Settings) — a little wayfinding color against the dark chrome. */
	.nav-group:nth-of-type(1) .nav-group-label {
		color: color-mix(in srgb, var(--lens-accent) 75%, var(--lens-muted));
	}
	.nav-group:nth-of-type(2) .nav-group-label {
		color: color-mix(in srgb, var(--lens-ok) 70%, var(--lens-muted));
	}
	.nav-group:nth-of-type(3) .nav-group-label {
		color: color-mix(in srgb, var(--lens-info) 70%, var(--lens-muted));
	}
	.nav-group:nth-of-type(4) .nav-group-label {
		color: color-mix(in srgb, var(--lens-warn) 65%, var(--lens-muted));
	}
	.nav-group:nth-of-type(5) .nav-group-label {
		color: color-mix(in srgb, var(--lens-danger) 55%, var(--lens-muted));
	}

	.repo-picker {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-1);
		margin: 0 var(--lens-space-1) var(--lens-space-2);
	}

	.repo-select {
		width: 100%;
		background: var(--lens-surface-raised);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: 0.3rem var(--lens-space-2);
		font-size: var(--lens-font-size-xs);
		font-family: var(--lens-font-mono);
	}

	.repo-meta {
		display: flex;
		align-items: center;
		gap: var(--lens-space-2);
		padding: 0 var(--lens-space-1);
	}

	.repo-dot {
		width: 6px;
		height: 6px;
		border-radius: var(--lens-radius-full);
		background: var(--lens-ok);
		flex-shrink: 0;
	}
	.repo-dot.idle {
		background: transparent;
		border: 1px solid var(--lens-border-strong);
	}

	.repo-health {
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-text-secondary);
		font-family: var(--lens-font-mono);
	}

	ul {
		list-style: none;
		padding: 0;
		margin: 0;
	}

	li {
		margin-bottom: 1px;
	}

	a {
		color: var(--lens-text-secondary);
		text-decoration: none;
		padding: 0.4rem var(--lens-space-3);
		display: block;
		border-radius: var(--lens-radius-sm);
		transition:
			background var(--lens-dur-fast) var(--lens-ease),
			color var(--lens-dur-fast) var(--lens-ease);
		font-size: var(--lens-font-size-sm);
		border-left: 2px solid transparent;
	}
	a:hover {
		color: var(--lens-text-strong);
		background: var(--lens-surface-raised);
	}
	a.active {
		color: var(--lens-text-strong);
		background: var(--lens-accent-tint);
		border-left-color: var(--lens-accent);
	}

	/* ── Footer ────────────────────────────────────────────────────────────── */
	.sidebar-footer {
		margin-top: auto;
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-3);
		padding-top: var(--lens-space-3);
		border-top: 1px solid var(--lens-border-subtle);
	}

	.refresh-row {
		display: flex;
		align-items: center;
		gap: var(--lens-space-2);
		color: var(--lens-text-secondary);
		font-size: var(--lens-font-size-sm);
		cursor: pointer;
	}
	.refresh-row input {
		accent-color: var(--lens-accent);
	}
	.refresh-hint {
		margin-left: auto;
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
		font-family: var(--lens-font-mono);
	}

	.theme-picker label {
		display: block;
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
		margin-bottom: var(--lens-space-1);
	}
	.theme-picker select {
		width: 100%;
		background: var(--lens-surface-raised);
		border: 1px solid var(--lens-border);
		color: var(--lens-text);
		padding: 0.35rem var(--lens-space-2);
		border-radius: var(--lens-radius-sm);
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
	}

	main {
		flex: 1;
		padding: var(--lens-space-8);
		overflow-y: auto;
		background: var(--lens-bg);
	}
</style>
