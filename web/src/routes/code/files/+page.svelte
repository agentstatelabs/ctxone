<script lang="ts">
	import { listFiles } from '$lib/codeApi';
	import type { FileEntry } from '$lib/codeTypes';
	import { selectedRepo } from '$lib/repoStore';

	let files = $state<FileEntry[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let filter = $state('');

	$effect(() => {
		const repo = $selectedRepo;
		loading = true;
		error = null;
		listFiles(repo)
			.then((f) => { files = f; loading = false; })
			.catch((e) => { error = e instanceof Error ? e.message : String(e); loading = false; });
	});

	// Build a tree from flat file paths.
	interface TreeNode {
		name: string;
		path: string;
		isFile: boolean;
		file?: FileEntry;
		children: Map<string, TreeNode>;
		expanded: boolean;
	}

	function buildTree(entries: FileEntry[]): TreeNode {
		const root: TreeNode = { name: '', path: '', isFile: false, children: new Map(), expanded: true };
		for (const f of entries) {
			const parts = f.path.split('/');
			let cur = root;
			for (let i = 0; i < parts.length; i++) {
				const part = parts[i];
				const isLast = i === parts.length - 1;
				if (!cur.children.has(part)) {
					const fullPath = parts.slice(0, i + 1).join('/');
					cur.children.set(part, {
						name: part,
						path: fullPath,
						isFile: isLast,
						file: isLast ? f : undefined,
						children: new Map(),
						expanded: false
					});
				}
				if (!isLast) cur = cur.children.get(part)!;
			}
		}
		return root;
	}

	let filteredFiles = $derived(
		filter.trim()
			? files.filter((f) => f.path.toLowerCase().includes(filter.toLowerCase()))
			: null
	);

	let tree = $derived(buildTree(filteredFiles ?? files));

	// Toggle expand — uses reactivity through a rebuilt tree approach:
	// we track expanded paths in a Set.
	let expanded = $state<Set<string>>(new Set());

	function toggle(path: string) {
		const next = new Set(expanded);
		if (next.has(path)) next.delete(path);
		else next.add(path);
		expanded = next;
	}

	function langColor(lang: string): string {
		const map: Record<string, string> = {
			rust: '#d08770',
			python: '#a3be8c',
			typescript: '#88c0d0',
			javascript: '#ebcb8b',
			swift: '#b48ead'
		};
		return map[lang.toLowerCase()] ?? 'var(--text-3)';
	}
</script>

<div class="page">
	<h2>Files</h2>

	<input
		type="text"
		class="filter"
		placeholder="Filter files…"
		bind:value={filter}
	/>

	{#snippet treeNode(node: { name: string; path: string; isFile: boolean; file?: FileEntry; children: Map<string, any> }, depth: number)}
		{#if node.isFile && node.file}
			<div class="tree-item file-item" style="padding-left: {depth * 16 + 8}px">
				<span class="tree-icon">📄</span>
				<a href="/code/files/{node.file.path}">
					<span class="fname">{node.name}</span>
				</a>
				<span class="file-badges">
					<span class="lang-dot" style="background: {langColor(node.file.language)}" title={node.file.language}></span>
					<span class="sym-count">{node.file.symbol_count}</span>
				</span>
			</div>
		{:else if node.children.size > 0}
			<!-- svelte-ignore a11y_click_events_have_key_events -->
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div
				class="tree-item dir-item"
				style="padding-left: {depth * 16 + 8}px"
				onclick={() => toggle(node.path)}
			>
				<span class="tree-icon">{expanded.has(node.path) ? '▾' : '▸'}</span>
				<span class="dname">{node.name}</span>
				<span class="dir-count">{node.children.size}</span>
			</div>
			{#if expanded.has(node.path)}
				{#each [...node.children.entries()].sort(([a, an], [b, bn]) => (an.isFile ? 1 : -1) - (bn.isFile ? 1 : -1) || a.localeCompare(b)) as [, child]}
					{@render treeNode(child, depth + 1)}
				{/each}
			{/if}
		{/if}
	{/snippet}

	{#if loading}
		<p class="muted">loading…</p>
	{:else if error}
		<p class="error">{error}</p>
	{:else if filteredFiles !== null}
		<!-- Flat list when filtering -->
		<div class="meta">{filteredFiles.length} file{filteredFiles.length === 1 ? '' : 's'}</div>
		<ul class="flat-list">
			{#each filteredFiles as f}
				<li>
					<a href="/code/files/{f.path}">
						<code class="filepath">{f.path}</code>
						<span class="file-badges">
							<span class="lang-dot" style="background: {langColor(f.language)}" title={f.language}></span>
							<span class="sym-count">{f.symbol_count}</span>
						</span>
					</a>
				</li>
			{/each}
		</ul>
	{:else}
		<!-- Tree view -->
		<div class="meta">{files.length} file{files.length === 1 ? '' : 's'}</div>
		<div class="tree">
			{#each [...tree.children.entries()].sort(([a, an], [b, bn]) => (an.isFile ? 1 : -1) - (bn.isFile ? 1 : -1) || a.localeCompare(b)) as [, node]}
				{@render treeNode(node, 0)}
			{/each}
		</div>
	{/if}
</div>

<style>
	.page {
		max-width: 800px;
	}

	h2 {
		margin: 0 0 1.25rem;
	}

	.filter {
		width: 100%;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text-1);
		padding: 0.4rem 0.75rem;
		font-family: inherit;
		font-size: 0.9rem;
		box-sizing: border-box;
		margin-bottom: 0.75rem;
	}

	.filter:focus {
		outline: none;
		border-color: var(--accent);
	}

	.meta {
		font-size: 0.8rem;
		color: var(--text-3);
		margin-bottom: 0.6rem;
	}

	.muted {
		color: var(--text-3);
	}

	.error {
		color: var(--danger);
	}

	.flat-list {
		list-style: none;
		margin: 0;
		padding: 0;
	}

	.flat-list li a {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0.35rem 0.5rem;
		border-radius: 4px;
		color: inherit;
		text-decoration: none;
		font-size: 0.88rem;
	}

	.flat-list li a:hover {
		background: var(--bg-hover);
	}

	.filepath {
		color: var(--text-1);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.file-badges {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		flex-shrink: 0;
		margin-left: 0.5rem;
	}

	.lang-dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
	}

	.sym-count {
		font-size: 0.75rem;
		color: var(--text-3);
		font-family: monospace;
	}

	.tree, .tree-root {
		font-size: 0.88rem;
	}

	.tree-item {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		padding: 0.28rem 0.5rem;
		border-radius: 4px;
		min-height: 1.6rem;
	}

	.dir-item {
		cursor: pointer;
		color: var(--text-2);
	}

	.dir-item:hover {
		background: var(--bg-hover);
	}

	.file-item {
		color: var(--text-1);
	}

	.file-item:hover {
		background: var(--bg-hover);
	}

	.file-item a {
		color: inherit;
		text-decoration: none;
		flex: 1;
	}

	.tree-icon {
		font-size: 0.75rem;
		width: 14px;
		text-align: center;
		flex-shrink: 0;
		color: var(--text-3);
	}

	.dname {
		font-weight: 500;
		color: var(--text-1);
	}

	.fname {
		font-family: monospace;
	}

	.dir-count {
		font-size: 0.72rem;
		color: var(--text-3);
		margin-left: auto;
	}
</style>
