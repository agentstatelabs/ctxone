<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { getCallGraph, searchSymbols } from '$lib/codeApi';
	import type { CallGraphResponse, CallGraphNode, SearchResult } from '$lib/codeTypes';
	import CallGraph from '$lib/CallGraph.svelte';

	let query = $state($page.url.searchParams.get('q') ?? '');
	let hops = $state(1);
	let graph = $state<CallGraphResponse | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);

	// Autocomplete suggestions
	let suggestions = $state<SearchResult[]>([]);
	let showSuggestions = $state(false);
	let suggestDebounce: ReturnType<typeof setTimeout> | null = null;

	// Selected node detail panel
	let selected = $state<CallGraphNode | null>(null);

	// Pinned nodes — kept in graph across navigation
	let pinned = $state<Set<string>>(new Set());

	async function loadGraph(qname: string) {
		if (!qname.trim()) return;
		loading = true;
		error = null;
		selected = null;
		try {
			graph = await getCallGraph(qname.trim(), hops);
			// Sync URL
			const params = new URLSearchParams({ q: qname.trim() });
			history.replaceState(null, '', `?${params}`);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	function handleSubmit(e: SubmitEvent) {
		e.preventDefault();
		showSuggestions = false;
		loadGraph(query);
	}

	function onQueryInput() {
		showSuggestions = true;
		if (suggestDebounce) clearTimeout(suggestDebounce);
		suggestDebounce = setTimeout(async () => {
			if (query.trim().length < 2) { suggestions = []; return; }
			try {
				suggestions = await searchSymbols({ q: query, limit: 8 });
			} catch {
				suggestions = [];
			}
		}, 250);
	}

	function pickSuggestion(s: SearchResult) {
		query = s.qname;
		suggestions = [];
		showSuggestions = false;
		loadGraph(s.qname);
	}

	function handleNodeClick(node: CallGraphNode) {
		selected = node;
	}

	function navigateToSymbol(qname: string) {
		goto(`/code/symbols/${encodeURIComponent(qname)}`);
	}

	function togglePin(id: string) {
		const next = new Set(pinned);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		pinned = next;
	}

	function expandNode(qname: string) {
		query = qname;
		loadGraph(qname);
	}

	// Auto-load if URL has a query param
	$effect(() => {
		const q = $page.url.searchParams.get('q');
		if (q) { query = q; loadGraph(q); }
	});
</script>

<div class="page">
	<h2>Graph Explorer</h2>

	<form class="search-bar" onsubmit={handleSubmit}>
		<div class="input-wrap">
			<input
				type="text"
				placeholder="Enter a symbol qname to explore…"
				bind:value={query}
				oninput={onQueryInput}
				autocomplete="off"
			/>
			{#if showSuggestions && suggestions.length > 0}
				<ul class="suggestions">
					{#each suggestions as s}
						<!-- svelte-ignore a11y_click_events_have_key_events -->
						<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
						<li onclick={() => pickSuggestion(s)}>
							<span class="kind-badge kind-{s.kind}">{s.kind}</span>
							<span class="sugg-qname">{s.qname}</span>
							<span class="sugg-file">{s.file}</span>
						</li>
					{/each}
				</ul>
			{/if}
		</div>

		<div class="hops-control">
			<label>
				Hops
				<select bind:value={hops}>
					<option value={1}>1</option>
					<option value={2}>2</option>
					<option value={3}>3</option>
				</select>
			</label>
		</div>

		<button type="submit" disabled={loading || !query.trim()}>
			{loading ? '…' : 'Load'}
		</button>
	</form>

	{#if error}
		<p class="error">{error}</p>
	{/if}

	{#if graph}
		<div class="graph-area">
			<div class="graph-main">
				<CallGraph
					nodes={graph.nodes}
					edges={graph.edges}
					width={740}
					height={520}
					onNodeClick={handleNodeClick}
				/>
				<div class="graph-meta">
					{graph.nodes.length} nodes · {graph.edges.length} edges · {hops} hop{hops === 1 ? '' : 's'}
				</div>
			</div>

			{#if selected}
				<div class="detail-panel">
					<div class="panel-header">
						<span class="kind-badge kind-{selected.kind}">{selected.kind}</span>
						<button class="close-btn" onclick={() => (selected = null)}>×</button>
					</div>
					<div class="panel-qname">{selected.qname}</div>
					<div class="panel-meta">
						<span>{selected.language}</span>
						<span class="dim">·</span>
						<span class="dim">{selected.file}</span>
					</div>

					<div class="panel-actions">
						<button onclick={() => navigateToSymbol(selected!.qname)}>View detail →</button>
						<button onclick={() => expandNode(selected!.qname)}>Expand graph</button>
						<button
							class="pin-btn"
							class:pinned={pinned.has(selected.id)}
							onclick={() => togglePin(selected!.id)}
						>
							{pinned.has(selected.id) ? '📌 Pinned' : 'Pin'}
						</button>
					</div>
				</div>
			{:else}
				<div class="detail-panel hint">
					<p>Click a node to see details</p>
					{#if pinned.size > 0}
						<div class="pinned-list">
							<div class="pinned-label">Pinned nodes</div>
							{#each [...pinned] as id}
								{@const node = graph?.nodes.find((n) => n.id === id)}
								{#if node}
									<div class="pinned-item">
										<span class="kind-badge kind-{node.kind}">{node.kind}</span>
										<span class="pin-qname">{node.qname}</span>
										<button class="unpin" onclick={() => togglePin(id)}>×</button>
									</div>
								{/if}
							{/each}
						</div>
					{/if}
				</div>
			{/if}
		</div>
	{:else if !loading}
		<div class="empty-state">
			<p>Enter a symbol name above to explore its call graph.</p>
			<p class="dim">Tip: use <a href="/code/search">Code Search</a> to find symbols by concept.</p>
		</div>
	{/if}
</div>

<style>
	.page {
		max-width: 1100px;
	}

	h2 {
		margin: 0 0 1.25rem;
	}

	.search-bar {
		display: flex;
		gap: 0.6rem;
		align-items: flex-start;
		margin-bottom: 1.25rem;
	}

	.input-wrap {
		flex: 1;
		position: relative;
	}

	.input-wrap input {
		width: 100%;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text-0);
		padding: 0.5rem 0.75rem;
		font-family: monospace;
		font-size: 0.92rem;
		box-sizing: border-box;
	}

	.input-wrap input:focus {
		outline: none;
		border-color: var(--accent);
	}

	.suggestions {
		position: absolute;
		top: 100%;
		left: 0;
		right: 0;
		z-index: 10;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 6px;
		margin: 2px 0 0;
		padding: 0;
		list-style: none;
		box-shadow: 0 4px 16px rgba(0,0,0,0.3);
	}

	.suggestions li {
		display: flex;
		align-items: baseline;
		gap: 0.5rem;
		padding: 0.4rem 0.75rem;
		cursor: pointer;
		font-size: 0.88rem;
		overflow: hidden;
	}

	.suggestions li:hover {
		background: var(--bg-hover);
	}

	.sugg-qname {
		font-family: monospace;
		color: var(--text-0);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		flex: 1;
	}

	.sugg-file {
		font-size: 0.75rem;
		color: var(--text-3);
		flex-shrink: 0;
	}

	.hops-control {
		display: flex;
		align-items: center;
	}

	.hops-control label {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		font-size: 0.85rem;
		color: var(--text-2);
	}

	.hops-control select {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--text-1);
		padding: 0.4rem 0.5rem;
		font-size: 0.88rem;
	}

	.search-bar > button {
		background: var(--accent-bg);
		border: 1px solid var(--accent-bg-hi);
		border-radius: 6px;
		color: var(--accent);
		padding: 0.5rem 1rem;
		font-size: 0.9rem;
		cursor: pointer;
		white-space: nowrap;
	}

	.search-bar > button:hover:not(:disabled) {
		background: color-mix(in srgb, var(--accent-bg) 60%, var(--accent));
	}

	.search-bar > button:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.error { color: var(--danger); }

	.graph-area {
		display: flex;
		gap: 1rem;
		align-items: flex-start;
	}

	.graph-main {
		flex: 1;
		min-width: 0;
	}

	.graph-meta {
		font-size: 0.75rem;
		color: var(--text-3);
		margin-top: 0.4rem;
	}

	.detail-panel {
		width: 240px;
		flex-shrink: 0;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 0.85rem 1rem;
	}

	.detail-panel.hint {
		color: var(--text-3);
		font-size: 0.85rem;
	}

	.panel-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		margin-bottom: 0.5rem;
	}

	.close-btn {
		background: none;
		border: none;
		color: var(--text-3);
		cursor: pointer;
		font-size: 1.1rem;
		padding: 0;
		line-height: 1;
	}

	.close-btn:hover { color: var(--text-0); }

	.panel-qname {
		font-family: monospace;
		font-size: 0.85rem;
		color: var(--text-0);
		word-break: break-all;
		margin-bottom: 0.3rem;
	}

	.panel-meta {
		font-size: 0.78rem;
		color: var(--text-3);
		display: flex;
		gap: 0.3rem;
		flex-wrap: wrap;
		margin-bottom: 0.75rem;
	}

	.dim { color: var(--text-3); }

	.panel-actions {
		display: flex;
		flex-direction: column;
		gap: 0.35rem;
	}

	.panel-actions button {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		border-radius: 5px;
		color: var(--text-1);
		padding: 0.35rem 0.6rem;
		font-size: 0.82rem;
		cursor: pointer;
		text-align: left;
	}

	.panel-actions button:hover {
		border-color: var(--accent);
		color: var(--accent);
	}

	.pin-btn.pinned {
		border-color: var(--accent);
		color: var(--accent);
	}

	.pinned-list {
		margin-top: 0.75rem;
	}

	.pinned-label {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.07em;
		color: var(--text-3);
		margin-bottom: 0.4rem;
	}

	.pinned-item {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		font-size: 0.82rem;
		padding: 0.2rem 0;
	}

	.pin-qname {
		font-family: monospace;
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-1);
	}

	.unpin {
		background: none;
		border: none;
		color: var(--text-3);
		cursor: pointer;
		padding: 0;
		font-size: 0.9rem;
	}

	.unpin:hover { color: var(--danger); }

	.empty-state {
		color: var(--text-3);
		font-size: 0.9rem;
	}

	.empty-state a {
		color: var(--accent);
	}

	.kind-badge {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		flex-shrink: 0;
	}

	.kind-function  { color: var(--kind-function,  #88c0d0); }
	.kind-method    { color: var(--kind-method,    #a3be8c); }
	.kind-class     { color: var(--kind-class,     #d08770); }
	.kind-module    { color: var(--kind-module,    #b48ead); }
	.kind-variable  { color: var(--kind-variable,  #ebcb8b); }
</style>
