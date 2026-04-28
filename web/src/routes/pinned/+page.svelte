<script lang="ts">
	import { onMount } from 'svelte';
	import { getPinned, primeSections, parseMarkdownSections } from '$lib/api';
	import type { PinnedItem } from '$lib/api';

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

	onMount(refresh);

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

<h2>Pinned Memory</h2>
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

{#if loading}
	<p class="muted">Loading...</p>
{:else if grouped.length === 0}
	<p class="muted">No pinned memories yet. Upload a markdown file above or run <code>ctx prime ./docs/VISION.md --pin</code>.</p>
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
		color: #888;
		margin-bottom: 2rem;
		max-width: 60ch;
	}

	code {
		background: #1a1a1a;
		color: #a5d6a7;
		padding: 0.1rem 0.4rem;
		border-radius: 3px;
		font-size: 0.85em;
	}

	.upload {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		padding: 1.25rem 1.5rem;
		margin-bottom: 2rem;
	}

	.upload h3 {
		margin: 0 0 0.75rem 0;
		font-size: 1rem;
		color: #fff;
	}

	.upload-row {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 0.75rem;
	}

	.upload input[type='text'] {
		flex: 1;
		background: #0a0a0a;
		border: 1px solid #333;
		border-radius: 6px;
		color: #e0e0e0;
		padding: 0.5rem 0.75rem;
	}

	.upload input[type='file'] {
		color: #888;
		flex: 1;
	}

	.pin-toggle {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		color: #aaa;
		margin-bottom: 0.75rem;
		cursor: pointer;
	}

	.upload button {
		background: #3b82f6;
		border: none;
		border-radius: 6px;
		color: #fff;
		padding: 0.5rem 1.25rem;
		cursor: pointer;
	}

	.upload button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.message {
		color: #22c55e;
		font-size: 0.85rem;
		margin: 0.75rem 0 0 0;
		font-family: monospace;
	}

	.source-group {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		padding: 1rem 1.5rem;
		margin-bottom: 1.5rem;
	}

	.source-name {
		margin: 0 0 1rem 0;
		color: #3b82f6;
		font-size: 1rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		font-family: monospace;
	}

	.section {
		padding: 0.75rem 0;
		border-bottom: 1px solid #1a1a1a;
	}

	.section:last-child {
		border-bottom: none;
	}

	.section-title {
		color: #fff;
		font-weight: 600;
		margin-bottom: 0.25rem;
	}

	.section-body {
		color: #aaa;
		font-size: 0.9rem;
		white-space: pre-wrap;
		line-height: 1.5;
	}

	.muted {
		color: #555;
	}

	.error {
		color: #ef4444;
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
		border: 1px solid #2a2a2a;
		border-radius: 6px;
		overflow: hidden;
	}
	.seg {
		background: #0d0d0d;
		border: 0;
		color: #888;
		padding: 0.35rem 0.85rem;
		font-size: 0.85rem;
		font-family: monospace;
		cursor: pointer;
	}
	.seg:not(:last-child) {
		border-right: 1px solid #2a2a2a;
	}
	.seg.active {
		background: #1e3a5f;
		color: #93c5fd;
	}
	.filter-input {
		flex: 1 1 16rem;
		min-width: 12rem;
		background: #0a0a0a;
		border: 1px solid #333;
		border-radius: 6px;
		color: #e0e0e0;
		padding: 0.4rem 0.7rem;
		font-family: monospace;
		font-size: 0.85rem;
	}
	.result-count {
		color: #555;
		font-family: monospace;
		font-size: 0.78rem;
	}
	/* Flat-mode chip identifying the source for each row. */
	.source-chip {
		display: inline-block;
		background: #1a2a3a;
		color: #93c5fd;
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
</style>
