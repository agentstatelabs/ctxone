<script lang="ts">
	import { getPinned, primeSections, parseMarkdownSections } from '$lib/api';
	import type { PinnedItem } from '$lib/api';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import ScopeBadge from '$lib/ScopeBadge.svelte';
	import EmptyState from '$lib/EmptyState.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	let pinned: PinnedItem[] = $state([]);
	let error: string | null = $state(null);
	let loading = $state(true);

	// View controls (mirror /browse: Tree groups by source, Flat is a
	// single searchable list). Persist the choice across reloads so the
	// agent's "I prefer flat" intent sticks.
	type ViewMode = 'tree' | 'flat';
	const VIEW_KEY = 'lens.pinned.view';
	function loadView(): ViewMode {
		if (typeof localStorage === 'undefined') return 'tree';
		const v = localStorage.getItem(VIEW_KEY);
		return v === 'flat' ? 'flat' : 'tree';
	}
	let viewMode: ViewMode = $state(loadView());
	function setView(v: ViewMode) {
		viewMode = v;
		if (typeof localStorage !== 'undefined') localStorage.setItem(VIEW_KEY, v);
	}
	let filter = $state('');

	// Upload form
	let fileInput: HTMLInputElement;
	let sourceName = $state('');
	let pinMode = $state(true);
	let uploading = $state(false);
	let uploadMessage: string | null = $state(null);

	async function refresh() {
		try {
			loading = true;
			pinned = await getPinned();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load pinned memories';
		} finally {
			loading = false;
		}
	}

	// Load on mount and re-load whenever the active namespace changes.
	$effect(() => {
		void namespaceStore.current;
		refresh();
	});

	const auto = useAutoRefresh(refresh);

	// Group pinned items by source (everything between /memory/pinned/ and the last segment)
	let grouped = $derived.by(() => {
		const bySource = new Map<string, Map<string, { title?: string; body?: string }>>();

		for (const item of pinned) {
			// path looks like /memory/pinned/<source>/<slug>/(title|body)
			const m = item.path.match(/^\/memory\/pinned\/([^/]+)\/([^/]+)\/(title|body)$/);
			if (!m) continue;
			const [, source, slug, field] = m;

			if (!bySource.has(source)) bySource.set(source, new Map());
			const sectionMap = bySource.get(source)!;
			if (!sectionMap.has(slug)) sectionMap.set(slug, {});
			const section = sectionMap.get(slug)!;

			if (field === 'title' && typeof item.value === 'string') section.title = item.value;
			if (field === 'body' && typeof item.value === 'string') section.body = item.value;
		}

		return Array.from(bySource.entries()).map(([source, sections]) => ({
			source,
			sections: Array.from(sections.values()).filter((s) => s.title && s.body) as Array<{
				title: string;
				body: string;
			}>
		}));
	});

	// Apply the filter input (case-insensitive substring) to source +
	// title + body. An empty filter shows everything.
	let filtered = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		if (!q) return grouped;
		return grouped
			.map((g) => ({
				source: g.source,
				sections: g.sections.filter(
					(s) =>
						g.source.toLowerCase().includes(q) ||
						s.title.toLowerCase().includes(q) ||
						s.body.toLowerCase().includes(q)
				)
			}))
			.filter((g) => g.sections.length > 0);
	});

	// Flat view: every section as its own row, with source as a prefix
	// chip. Useful when you know the title and don't want to scroll
	// through groups.
	let flatSections = $derived.by(() =>
		filtered.flatMap((g) =>
			g.sections.map((s) => ({ source: g.source, title: s.title, body: s.body }))
		)
	);

	async function handleUpload(e: SubmitEvent) {
		e.preventDefault();
		const files = fileInput?.files;
		if (!files || files.length === 0) {
			uploadMessage = 'Pick a markdown file first';
			return;
		}

		const file = files[0];
		const content = await file.text();
		const sections = parseMarkdownSections(content);

		if (sections.length === 0) {
			uploadMessage = 'No sections found — add H1 or H2 headings';
			return;
		}

		const source = sourceName.trim() || file.name.replace(/\.md$/, '');

		uploading = true;
		uploadMessage = null;
		try {
			const result = await primeSections(source, pinMode, sections);
			uploadMessage = `${result.pinned ? 'Pinned' : 'Primed'} ${result.sections_written} sections under "${result.source}"`;
			fileInput.value = '';
			sourceName = '';
			await refresh();
		} catch (e) {
			uploadMessage = e instanceof Error ? e.message : 'Upload failed';
		} finally {
			uploading = false;
		}
	}
</script>

<h2>
	Pinned Memory <ScopeBadge />
	<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
</h2>
<p class="intro">
	Pinned memories are always included in every <code>recall</code> response, regardless of the
	topic. Use them for critical project context.
</p>

<form class="upload" onsubmit={handleUpload}>
	<h3>Prime from markdown</h3>
	<div class="upload-row">
		<input type="file" accept=".md,.markdown,text/markdown" bind:this={fileInput} disabled={uploading} />
		<input
			type="text"
			placeholder="source name (optional)"
			bind:value={sourceName}
			disabled={uploading}
		/>
	</div>
	<label class="pin-toggle">
		<input type="checkbox" bind:checked={pinMode} disabled={uploading} />
		Pin (always include in recall)
	</label>
	<button type="submit" disabled={uploading}>
		{uploading ? 'Uploading...' : 'Upload'}
	</button>
	{#if uploadMessage}
		<p class="message">{uploadMessage}</p>
	{/if}
</form>

{#if error}
	<p class="error">{error}</p>
{/if}

{#if loading && pinned.length === 0}
	<p class="muted">Loading...</p>
{:else if grouped.length === 0}
	<EmptyState
		icon="📌"
		title="No pinned memories yet"
		description="Pinned memories ride every recall, regardless of topic — use them for critical context. Upload a markdown file above, or run `ctx prime ./docs/VISION.md --pin`."
	/>
{:else}
	<div class="controls-bar">
		<div class="seg-group" role="tablist" aria-label="View mode">
			<button
				class="seg"
				class:active={viewMode === 'tree'}
				onclick={() => setView('tree')}
				type="button"
			>Grouped</button>
			<button
				class="seg"
				class:active={viewMode === 'flat'}
				onclick={() => setView('flat')}
				type="button"
			>Flat</button>
		</div>
		<input
			type="search"
			class="filter-input"
			placeholder="Filter source / title / body…"
			bind:value={filter}
			aria-label="Filter pinned memories"
		/>
		<span class="result-count">
			{flatSections.length} / {grouped.reduce((n, g) => n + g.sections.length, 0)}
		</span>
	</div>

	{#if filtered.length === 0}
		<p class="muted">No pinned memories match "{filter}".</p>
	{:else if viewMode === 'tree'}
		{#each filtered as group}
			<div class="source-group">
				<h3 class="source-name">{group.source}</h3>
				<div class="sections">
					{#each group.sections as section}
						<div class="section">
							<div class="section-title">{section.title}</div>
							<div class="section-body">{section.body}</div>
						</div>
					{/each}
				</div>
			</div>
		{/each}
	{:else}
		<div class="source-group flat-list">
			{#each flatSections as s}
				<div class="section flat-section">
					<div class="section-title">
						<span class="source-chip">{s.source}</span>
						{s.title}
					</div>
					<div class="section-body">{s.body}</div>
				</div>
			{/each}
		</div>
	{/if}
{/if}

<style>
	.intro {
		color: var(--text-2);
		margin-bottom: 2rem;
		max-width: 60ch;
	}

	code {
		background: var(--bg-hover);
		color: var(--success);
		padding: 0.1rem 0.4rem;
		border-radius: 3px;
		font-size: 0.85em;
	}

	.upload {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1.25rem 1.5rem;
		margin-bottom: 2rem;
	}

	.upload h3 {
		margin: 0 0 0.75rem 0;
		font-size: 1rem;
		color: var(--text-0);
	}

	.upload-row {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 0.75rem;
	}

	.upload input[type='text'] {
		flex: 1;
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text-1);
		padding: 0.5rem 0.75rem;
	}

	.upload input[type='file'] {
		color: var(--text-2);
		flex: 1;
	}

	.pin-toggle {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		color: var(--text-2);
		margin-bottom: 0.75rem;
		cursor: pointer;
	}

	.upload button {
		background: var(--accent);
		border: none;
		border-radius: 6px;
		color: var(--text-0);
		padding: 0.5rem 1.25rem;
		cursor: pointer;
	}

	.upload button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.message {
		color: var(--success);
		font-size: 0.85rem;
		margin: 0.75rem 0 0 0;
		font-family: monospace;
	}

	.source-group {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1rem 1.5rem;
		margin-bottom: 1.5rem;
	}

	.source-name {
		margin: 0 0 1rem 0;
		color: var(--accent);
		font-size: 1rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		font-family: monospace;
	}

	.section {
		padding: 0.75rem 0;
		border-bottom: 1px solid var(--bg-hover);
	}

	.section:last-child {
		border-bottom: none;
	}

	.section-title {
		color: var(--text-0);
		font-weight: 600;
		margin-bottom: 0.25rem;
	}

	.section-body {
		color: var(--text-2);
		font-size: 0.9rem;
		white-space: pre-wrap;
		line-height: 1.5;
	}

	.muted {
		color: var(--text-3);
	}

	.error {
		color: var(--danger);
	}

	/* View controls bar — mirrors /browse for consistency. */
	.controls-bar {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		margin-bottom: 1rem;
		flex-wrap: wrap;
	}
	.seg-group {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: 6px;
		overflow: hidden;
	}
	.seg {
		background: var(--bg-1);
		border: 0;
		color: var(--text-2);
		padding: 0.35rem 0.85rem;
		font-size: 0.85rem;
		font-family: monospace;
		cursor: pointer;
	}
	.seg:not(:last-child) {
		border-right: 1px solid var(--border);
	}
	.seg.active {
		background: var(--accent-bg);
		color: var(--accent);
	}
	.filter-input {
		flex: 1 1 16rem;
		min-width: 12rem;
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text-1);
		padding: 0.4rem 0.7rem;
		font-family: monospace;
		font-size: 0.85rem;
	}
	.result-count {
		color: var(--text-3);
		font-family: monospace;
		font-size: 0.78rem;
	}
	/* Flat-mode chip identifying the source for each row. */
	.source-chip {
		display: inline-block;
		background: var(--accent-bg);
		color: var(--accent);
		font-family: monospace;
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		padding: 0.05rem 0.45rem;
		border-radius: 3px;
		margin-right: 0.5rem;
		vertical-align: middle;
	}
	.flat-section {
		padding: 0.6rem 0.25rem;
	}
	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		font-weight: normal;
		margin-left: 0.75rem;
	}
</style>
