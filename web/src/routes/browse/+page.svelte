<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { listPaths, getState, getBlame, forget } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import ScopeBadge from '$lib/ScopeBadge.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	type ViewMode = 'tree' | 'flat';
	const VIEW_KEY = 'ctxone:browseView';

	let paths: string[] = $state([]);
	let selectedPath: string | null = $state(null);
	let selectedValue: unknown = $state(null);
	let blame: unknown = $state(null);
	let error: string | null = $state(null);
	let forgetting = $state(false);
	let forgetMessage: string | null = $state(null);
	let viewMode: ViewMode = $state(loadView());
	let expanded: Set<string> = $state(new Set(['']));

	function loadView(): ViewMode {
		if (typeof localStorage === 'undefined') return 'tree';
		const v = localStorage.getItem(VIEW_KEY);
		return v === 'flat' ? 'flat' : 'tree';
	}

	function setView(v: ViewMode) {
		viewMode = v;
		if (typeof localStorage !== 'undefined') localStorage.setItem(VIEW_KEY, v);
	}

	interface TreeNode {
		name: string;
		fullPath: string;
		children: Map<string, TreeNode>;
		leafPath: string | null;
	}

	function buildTree(list: string[]): TreeNode {
		const root: TreeNode = { name: '', fullPath: '', children: new Map(), leafPath: null };
		for (const p of list) {
			const segments = p.split('/').filter((s) => s.length > 0);
			let node = root;
			let acc = '';
			for (let i = 0; i < segments.length; i++) {
				const seg = segments[i];
				acc = `${acc}/${seg}`;
				let child = node.children.get(seg);
				if (!child) {
					child = { name: seg, fullPath: acc, children: new Map(), leafPath: null };
					node.children.set(seg, child);
				}
				if (i === segments.length - 1) child.leafPath = p;
				node = child;
			}
		}
		return root;
	}

	let tree = $derived(buildTree(paths));

	function toggleFolder(path: string) {
		const next = new Set(expanded);
		if (next.has(path)) next.delete(path);
		else next.add(path);
		expanded = next;
	}

	function expandAll() {
		const all = new Set<string>(['']);
		const walk = (n: TreeNode) => {
			if (n.children.size > 0) all.add(n.fullPath);
			for (const c of n.children.values()) walk(c);
		};
		walk(tree);
		expanded = all;
	}

	function collapseAll() {
		expanded = new Set(['']);
	}

	async function loadPaths() {
		error = null;
		try {
			paths = await listPaths(branchStore.current);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load paths';
			paths = [];
		}
	}

	// Initial load + deep link (?path=…) from /recall and /why: select the
	// linked path and expand its ancestors so it's visible in tree view.
	onMount(async () => {
		await loadPaths();
		const deepLink = $page.url.searchParams.get('path');
		if (!deepLink) return;
		// Recall/why hand out *section* paths for pinned memories, while
		// the path list holds the stored leaves (…/body, …/title) — fall
		// back to a child leaf when there's no exact match.
		const target = paths.includes(deepLink)
			? deepLink
			: (paths.find((p) => p === `${deepLink}/body`) ??
				paths.find((p) => p.startsWith(`${deepLink}/`)));
		if (!target) return;
		const next = new Set(expanded);
		const segments = target.split('/').filter((s) => s.length > 0);
		let acc = '';
		for (const seg of segments.slice(0, -1)) {
			acc = `${acc}/${seg}`;
			next.add(acc);
		}
		expanded = next;
		void selectPath(target);
	});

	const auto = useAutoRefresh(loadPaths);

	$effect(() => {
		void branchStore.current;
		void namespaceStore.current;
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
			const forgotten = selectedPath;
			selectedPath = null;
			selectedValue = null;
			blame = null;
			await loadPaths();
			forgetMessage = `Forgot ${forgotten}`;
		} catch (e) {
			forgetMessage = e instanceof Error ? e.message : 'Forget failed';
		} finally {
			forgetting = false;
		}
	}

	function formatBlame(b: unknown): string {
		if (b === null || b === undefined) return 'No provenance available';
		return JSON.stringify(b, null, 2);
	}
</script>

<h2>
	Browse Memory <ScopeBadge branch />
	<span class="view-toggle">
		<button class="seg" class:active={viewMode === 'tree'} onclick={() => setView('tree')}>Tree</button>
		<button class="seg" class:active={viewMode === 'flat'} onclick={() => setView('flat')}>Flat</button>
	</span>
	{#if viewMode === 'tree'}
		<button class="link-btn" onclick={expandAll}>expand all</button>
		<button class="link-btn" onclick={collapseAll}>collapse all</button>
	{/if}
	<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
</h2>

{#if error}
	<p class="error">{error}</p>
{/if}

<div class="browser">
	<div class="path-list">
		{#if paths.length === 0}
			<p class="empty">No paths found on {branchStore.current}. Start by remembering something.</p>
		{:else if viewMode === 'flat'}
			{#each paths as path}
				<button
					class="path-item"
					class:selected={selectedPath === path}
					onclick={() => selectPath(path)}
				>
					{path}
				</button>
			{/each}
		{:else}
			{@render renderChildren(tree, 0)}
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

{#snippet renderChildren(node: TreeNode, depth: number)}
	{#each [...node.children.values()].sort((a, b) => a.name.localeCompare(b.name)) as child}
		{@const isFolder = child.children.size > 0}
		{@const isExpanded = expanded.has(child.fullPath)}
		<div class="row" style="padding-left: {depth * 0.9 + 0.25}rem">
			{#if isFolder}
				<button class="folder" onclick={() => toggleFolder(child.fullPath)}>
					<span class="caret">{isExpanded ? '▾' : '▸'}</span>
					<span class="folder-name">{child.name}/</span>
					<span class="count">{countLeaves(child)}</span>
				</button>
			{:else if child.leafPath}
				<button
					class="path-item leaf"
					class:selected={selectedPath === child.leafPath}
					onclick={() => selectPath(child.leafPath!)}
				>
					{child.name}
				</button>
			{/if}
		</div>
		{#if isFolder && isExpanded}
			{@render renderChildren(child, depth + 1)}
		{/if}
		{#if isFolder && isExpanded && child.leafPath}
			<div class="row" style="padding-left: {(depth + 1) * 0.9 + 0.25}rem">
				<button
					class="path-item leaf"
					class:selected={selectedPath === child.leafPath}
					onclick={() => selectPath(child.leafPath!)}
				>
					(self)
				</button>
			</div>
		{/if}
	{/each}
{/snippet}

<script lang="ts" module>
	function countLeaves(node: { children: Map<string, any>; leafPath: string | null }): number {
		let n = node.leafPath ? 1 : 0;
		for (const c of node.children.values()) n += countLeaves(c);
		return n;
	}
</script>

<style>

	.view-toggle {
		display: inline-flex;
		gap: 0;
		margin-left: 1rem;
		border: 1px solid var(--border);
		border-radius: 6px;
		overflow: hidden;
		vertical-align: middle;
	}

	.seg {
		background: var(--bg-1);
		border: none;
		color: var(--text-2);
		padding: 0.25rem 0.7rem;
		font-size: 0.78rem;
		cursor: pointer;
	}
	.seg:not(:last-child) {
		border-right: 1px solid var(--border);
	}
	.seg.active {
		background: var(--accent-bg);
		color: var(--accent);
	}

	.link-btn {
		background: none;
		border: none;
		color: var(--text-2);
		font-size: 0.78rem;
		cursor: pointer;
		margin-left: 0.5rem;
		text-decoration: underline;
	}
	.link-btn:hover {
		color: var(--text-0);
	}

	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		font-weight: normal;
		margin-left: 0.75rem;
	}

	.browser {
		display: grid;
		grid-template-columns: 1fr 1.3fr;
		gap: 1rem;
		height: calc(100vh - 10rem);
	}

	.path-list {
		background: var(--bg-1);
		border: 1px solid var(--border);
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
		color: var(--text-1);
		padding: 0.35rem 0.6rem;
		cursor: pointer;
		border-radius: 4px;
		font-family: monospace;
		font-size: 0.82rem;
	}

	.path-item:hover {
		background: var(--bg-hover);
	}
	.path-item.selected {
		background: var(--bg-active);
		color: var(--text-0);
	}

	.row {
		display: block;
	}

	.folder {
		display: flex;
		align-items: center;
		gap: 0.35rem;
		width: 100%;
		background: none;
		border: none;
		color: var(--text-2);
		padding: 0.3rem 0.6rem;
		cursor: pointer;
		border-radius: 4px;
		font-family: monospace;
		font-size: 0.82rem;
		text-align: left;
	}
	.folder:hover {
		background: var(--bg-hover);
		color: var(--text-0);
	}
	.caret {
		width: 0.8rem;
		display: inline-block;
		color: var(--text-3);
	}
	.folder-name {
		flex: 1;
	}
	.count {
		color: var(--text-3);
		font-size: 0.72rem;
	}

	.detail {
		background: var(--bg-1);
		border: 1px solid var(--border);
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
		color: var(--text-0);
		word-break: break-all;
	}

	.detail h4 {
		color: var(--text-3);
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		margin-top: 1rem;
		margin-bottom: 0.4rem;
	}

	.forget-btn {
		background: color-mix(in srgb, var(--danger) 18%, transparent);
		border: 1px solid color-mix(in srgb, var(--danger) 35%, transparent);
		color: var(--danger);
		padding: 0.3rem 0.75rem;
		border-radius: 4px;
		font-size: 0.8rem;
		cursor: pointer;
		flex-shrink: 0;
	}
	.forget-btn:hover:not(:disabled) {
		background: color-mix(in srgb, var(--danger) 28%, transparent);
	}
	.forget-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.forget-message {
		color: var(--success);
		font-size: 0.8rem;
		margin: 0.5rem 0 0 0;
		font-family: monospace;
	}

	pre {
		color: var(--success);
		font-size: 0.85rem;
		white-space: pre-wrap;
		word-break: break-word;
		margin: 0;
	}
	pre.blame {
		color: var(--text-2);
		font-size: 0.78rem;
	}

	.error {
		color: var(--danger);
	}
	.empty,
	.hint {
		color: var(--text-3);
	}
</style>
