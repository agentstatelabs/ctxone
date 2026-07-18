<script lang="ts">
	import { hubFetch } from '$lib/api';
	import { formatCompact } from '@agentstate/lens-core';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	interface Session {
		session_id: string;
		/** Human-readable title (server-derived from the first user turn).
		 * Optional — absent on older Hubs; we fall back to the id. */
		name?: string | null;
		session_tokens_used: number;
		session_tokens_saved: number;
		total_graph_size_tokens: number;
		cumulative_ratio: number;
		llm_input_tokens: number;
		llm_output_tokens: number;
		llm_cache_read_tokens: number;
		llm_call_count: number;
		last_model: string | null;
		last_provider: string | null;
	}

	interface MemoryCommit {
		id: string;
		timestamp: string;
		agent_id: string;
		intent: { description: string; tags: string[] };
	}

	let sessions: Session[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);
	let selected: Session | null = $state(null);
	let memories: MemoryCommit[] = $state([]);
	let memoriesLoading = $state(false);

	interface Turn {
		key: string;
		turn_index?: number;
		timestamp?: string;
		model?: string;
		user_text?: string;
		assistant_text?: string;
		tool_calls?: string[];
		tool_calls_raw?: unknown[];
		tokens?: { input?: number; output?: number; cache_read?: number; cache_creation?: number };
	}
	let turns: Turn[] = $state([]);
	let turnsLoading = $state(false);
	let turnsError: string | null = $state(null);
	let expandedTools: Record<string, boolean> = $state({});

	$effect(() => {
		if (selected) {
			loadMemories(selected.session_id);
			loadTurns(selected.session_id);
		} else {
			memories = [];
			turns = [];
		}
	});

	async function loadTurns(sessionId: string) {
		turnsLoading = true;
		turnsError = null;
		turns = [];
		expandedTools = {};
		try {
			// One subtree fetch returns every turn for the session.
			const r = await hubFetch(
				`/api/state/main?path=/sessions/${encodeURIComponent(sessionId)}/turns`
			);
			if (r.status === 404) {
				turns = []; // session predates turn capture
				return;
			}
			if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
			const tree = await r.json();
			if (tree && typeof tree === 'object') {
				turns = Object.entries(tree as Record<string, Turn>)
					.map(([key, v]) => ({ ...v, key }))
					.sort((a, b) => (a.turn_index ?? 0) - (b.turn_index ?? 0));
			}
		} catch (e) {
			turnsError = e instanceof Error ? e.message : String(e);
		} finally {
			turnsLoading = false;
		}
	}

	async function loadMemories(sessionId: string) {
		memoriesLoading = true;
		try {
			const r = await hubFetch('/api/log/main?limit=500');
			if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
			const all: MemoryCommit[] = await r.json();
			const tag = `session:${sessionId}`;
			memories = all.filter((c) => c.intent.tags?.includes(tag));
		} catch {
			memories = [];
		} finally {
			memoriesLoading = false;
		}
	}

	async function load() {
		loading = true;
		error = null;
		try {
			const r = await hubFetch('/api/stats/sessions');
			if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
			sessions = await r.json();
			sessions.sort((a, b) => b.session_tokens_used - a.session_tokens_used);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		// Re-load whenever the active namespace changes
		void namespaceStore.current;
		selected = null;
		load();
	});

	const auto = useAutoRefresh(async () => {
		await load();
		if (selected) await loadMemories(selected.session_id);
	});

	function ratioColor(r: number): string {
		if (r >= 5) return 'var(--success)';
		if (r >= 2) return 'var(--success)';
		if (r >= 1.2) return 'var(--accent)';
		return 'var(--text-2)';
	}

	// Compact display (12.4K / 17.5M / 1.2B) — exact value goes in the
	// title attribute so precision is a hover away.
	function fmt(n: number): string {
		return formatCompact(n ?? 0);
	}
	function exact(n: number): string {
		return (n ?? 0).toLocaleString();
	}

	// A session that carries a name (or, in the detail, whose first turn
	// we've loaded) gets a human title; otherwise the id stands in.
	function truncate(s: string, n: number): string {
		return s.length > n ? s.slice(0, n - 1) + '…' : s;
	}
	function listName(s: Session): string {
		return s.name?.trim() || s.session_id;
	}
	// Detail title: server name > first user message (turns already loaded) > id.
	const detailTitle: string = $derived.by(() => {
		const sel = selected;
		if (!sel) return '';
		if (sel.name?.trim()) return sel.name.trim();
		const firstUser = turns.find((t) => t.user_text?.trim())?.user_text?.trim();
		if (firstUser) return truncate(firstUser, 80);
		return sel.session_id;
	});
</script>

<div class="page">
	<div class="header">
		<h1>Sessions</h1>
		<button class="refresh-btn" onclick={load} disabled={loading}>
			{loading ? 'Loading…' : 'Refresh'}
		</button>
		<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
	</div>

	{#if error}
		<p class="error">{error}</p>
	{:else if loading}
		<p class="muted">Loading sessions…</p>
	{:else if sessions.length === 0}
		<p class="muted">No sessions yet. Run <code>ctx recall</code> or <code>ctx remember</code> to start one.</p>
	{:else}
		<div class="layout">
			<div class="list">
				{#each sessions as s}
					<button
						class="session-row"
						class:active={selected?.session_id === s.session_id}
						onclick={() => selected = s}
					>
						<div class="session-name">{listName(s)}</div>
						{#if s.name?.trim()}
							<div class="session-id" title={s.session_id}>{s.session_id}</div>
						{/if}
						<div class="session-meta">
							<span>{fmt(s.session_tokens_used)} tokens used</span>
							<span class="ratio" style="color: {ratioColor(s.cumulative_ratio)}">
								{s.cumulative_ratio.toFixed(1)}x
							</span>
						</div>
					</button>
				{/each}
			</div>

			{#if selected}
				<div class="detail">
					<h2>{detailTitle}</h2>
					{#if detailTitle !== selected.session_id}
						<div class="detail-id" title="session id">{selected.session_id}</div>
					{/if}

					<div class="stat-grid">
						<div class="stat">
							<div class="stat-value" title={exact(selected.session_tokens_used)}>{fmt(selected.session_tokens_used)}</div>
							<div class="stat-label">Tokens used</div>
						</div>
						<div class="stat">
							<div class="stat-value" style="color: var(--success)" title={exact(selected.session_tokens_saved)}>{fmt(selected.session_tokens_saved)}</div>
							<div class="stat-label">Tokens saved</div>
						</div>
						<div class="stat">
							<div class="stat-value" title={exact(selected.total_graph_size_tokens)}>{fmt(selected.total_graph_size_tokens)}</div>
							<div class="stat-label">Graph size (tokens)</div>
						</div>
						<div class="stat">
							<div class="stat-value" style="color: {ratioColor(selected.cumulative_ratio)}; font-size: 2rem">
								{selected.cumulative_ratio.toFixed(1)}x
							</div>
							<div class="stat-label">Savings ratio</div>
						</div>
					</div>

					{#if selected.session_tokens_used === 0 && selected.llm_call_count > 0}
						<p class="zero-hint">
							This session reported LLM usage but no memory operations carry its
							session id — its recalls likely ran under the
							<code>default</code> session (the agent isn't sending
							<code>X-CTXone-Session</code> on memory calls). Used/saved
							accrue there instead.
						</p>
					{/if}

					{#if selected.llm_call_count > 0}
						<h3>LLM Consumption</h3>
						<div class="stat-grid">
							<div class="stat">
								<div class="stat-value" title={exact(selected.llm_call_count)}>{fmt(selected.llm_call_count)}</div>
								<div class="stat-label">API calls</div>
							</div>
							<div class="stat">
								<div class="stat-value" title={exact(selected.llm_input_tokens)}>{fmt(selected.llm_input_tokens)}</div>
								<div class="stat-label">Input tokens</div>
							</div>
							<div class="stat">
								<div class="stat-value" title={exact(selected.llm_output_tokens)}>{fmt(selected.llm_output_tokens)}</div>
								<div class="stat-label">Output tokens</div>
							</div>
							<div class="stat">
								<div class="stat-value" title={exact(selected.llm_cache_read_tokens)}>{fmt(selected.llm_cache_read_tokens)}</div>
								<div class="stat-label">Cache read tokens</div>
							</div>
						</div>
						{#if selected.last_model}
							<p class="model-info">
								Last model: <code>{selected.last_model}</code>
								{#if selected.last_provider}via {selected.last_provider}{/if}
							</p>
						{/if}
					{:else}
						<p class="muted hint">
							No LLM usage reported. Agents can call <code>record_llm_usage</code> (MCP)
							or <code>POST /api/stats/llm_usage</code> to surface real token counts.
						</p>
					{/if}

					<h3>Conversation {#if turns.length}<span class="count">{turns.length} turns</span>{/if}</h3>
					{#if turnsLoading}
						<p class="muted">Loading transcript…</p>
					{:else if turnsError}
						<p class="muted hint">Transcript unavailable: {turnsError}</p>
					{:else if turns.length === 0}
						<p class="muted hint">
							No captured turns for this session. Turn content is recorded when an
							agent posts to <code>/api/sessions/{'{sid}'}/turns</code> (e.g. via the
							session-ingest tooling).
						</p>
					{:else}
						<ol class="turns">
							{#each turns as t (t.key)}
								<li class="turn">
									<div class="turn-head">
										<span class="turn-idx">#{(t.turn_index ?? 0) + 1}</span>
										{#if t.model}<span class="turn-model">{t.model}</span>{/if}
										{#if t.timestamp}<span class="turn-time"
												>{new Date(t.timestamp).toLocaleString()}</span
											>{/if}
										{#if t.tokens}
											<span class="turn-tok" title="input / output tokens"
												>{fmt(t.tokens.input ?? 0)}↑ {fmt(t.tokens.output ?? 0)}↓</span
											>
										{/if}
									</div>
									{#if t.user_text?.trim()}
										<div class="msg user">
											<span class="msg-role">User</span>
											<div class="msg-body">{t.user_text}</div>
										</div>
									{/if}
									{#if t.assistant_text?.trim()}
										<div class="msg assistant">
											<span class="msg-role">Assistant</span>
											<div class="msg-body">{t.assistant_text}</div>
										</div>
									{/if}
									{#if t.tool_calls?.length}
										<div class="msg tool">
											<button
												class="msg-role tool-toggle"
												onclick={() => (expandedTools[t.key] = !expandedTools[t.key])}
											>
												{expandedTools[t.key] ? '▾' : '▸'} {t.tool_calls.length} tool call{t
													.tool_calls.length > 1
													? 's'
													: ''}
											</button>
											<div class="msg-body">
												{#each t.tool_calls as tc}
													<div class="tool-summary">{tc}</div>
												{/each}
												{#if expandedTools[t.key] && t.tool_calls_raw?.length}
													<pre class="tool-raw">{JSON.stringify(t.tool_calls_raw, null, 2)}</pre>
												{/if}
											</div>
										</div>
									{/if}
								</li>
							{/each}
						</ol>
					{/if}

					<h3>Memories</h3>
					{#if memoriesLoading}
						<p class="muted">Loading memories…</p>
					{:else if memories.length === 0}
						<p class="muted hint">
							No memories tagged <code>session:{selected.session_id}</code>.
							New <code>remember</code> calls from this session are auto-tagged.
						</p>
					{:else}
						<ul class="memory-list">
							{#each memories as m}
								<li class="memory-item">
									<div class="memory-head">
										<code class="memory-id">{m.id.slice(0, 12)}</code>
										<span class="memory-agent">{m.agent_id}</span>
										<span class="memory-time">{new Date(m.timestamp).toLocaleString()}</span>
									</div>
									<div class="memory-desc">{m.intent.description}</div>
									{#if m.intent.tags?.length}
										<div class="memory-tags">
											{#each m.intent.tags as t}
												<span class="tag">{t}</span>
											{/each}
										</div>
									{/if}
								</li>
							{/each}
						</ul>
					{/if}
				</div>
			{:else}
				<div class="detail placeholder">
					<p class="muted">Select a session to see details.</p>
				</div>
			{/if}
		</div>
	{/if}
</div>

<style>
	.page { max-width: 1100px; }

	.header {
		display: flex;
		align-items: center;
		gap: 1rem;
		margin-bottom: 1.5rem;
	}

	h1 { margin: 0; font-size: 1.8rem; }

	.refresh-btn {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		color: var(--text-2);
		padding: 0.35rem 0.75rem;
		border-radius: 6px;
		cursor: pointer;
		font-size: 0.85rem;
	}

	.refresh-btn:hover:not(:disabled) { color: var(--text-0); border-color: var(--text-3); }
	.refresh-btn:disabled { opacity: 0.5; cursor: default; }

	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		margin-left: auto;
	}

	.error { color: var(--danger); }
	.muted { color: var(--text-3); font-size: 0.9rem; }

	.layout {
		display: grid;
		grid-template-columns: 300px 1fr;
		gap: 1.5rem;
		align-items: start;
	}

	.list {
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}

	.session-row {
		width: 100%;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 0.75rem 1rem;
		text-align: left;
		cursor: pointer;
		color: var(--text-1);
		transition: all 0.15s;
	}

	.session-row:hover { border-color: var(--text-3); background: var(--bg-1); }
	.session-row.active { border-color: var(--border-hi); background: var(--accent-bg); }

	.session-name {
		font-family: monospace;
		font-size: 0.9rem;
		margin-bottom: 0.35rem;
		word-break: break-all;
	}

	.session-meta {
		display: flex;
		justify-content: space-between;
		font-size: 0.78rem;
		color: var(--text-3);
	}

	.ratio { font-weight: 600; }

	.detail {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1.5rem;
	}

	.detail.placeholder {
		display: flex;
		align-items: center;
		justify-content: center;
		min-height: 200px;
	}

	.detail h2 {
		margin: 0 0 1.25rem 0;
		font-family: monospace;
		font-size: 1.1rem;
		word-break: break-all;
	}

	.detail h3 {
		margin: 1.5rem 0 0.75rem;
		font-size: 0.9rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--text-3);
	}

	.stat-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
		gap: 1rem;
	}

	.stat {
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1rem;
	}

	.stat-value {
		font-size: 1.5rem;
		font-weight: 700;
		color: var(--text-0);
	}

	.stat-label {
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-3);
		margin-top: 0.25rem;
	}

	.model-info {
		margin-top: 1rem;
		font-size: 0.85rem;
		color: var(--text-3);
	}

	.hint { margin-top: 1rem; }

	.memory-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}

	.memory-item {
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 0.6rem 0.8rem;
	}

	.memory-head {
		display: flex;
		gap: 0.75rem;
		align-items: center;
		font-size: 0.75rem;
		color: var(--text-3);
		margin-bottom: 0.3rem;
	}

	.memory-id { color: var(--text-2); }
	.memory-agent { color: var(--accent); }
	.memory-time { margin-left: auto; }

	.memory-desc {
		font-size: 0.88rem;
		color: var(--text-1);
		line-height: 1.4;
	}

	.memory-tags {
		display: flex;
		flex-wrap: wrap;
		gap: 0.3rem;
		margin-top: 0.4rem;
	}

	.tag {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		color: var(--text-2);
		font-size: 0.7rem;
		padding: 0.1rem 0.4rem;
		border-radius: 3px;
	}

	code {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		padding: 0.1em 0.35em;
		border-radius: 3px;
		font-size: 0.85em;
	}

	.zero-hint {
		margin: 0.6rem 0 0;
		padding: 0.5rem 0.7rem;
		font-size: var(--lens-font-size-xs);
		color: var(--lens-muted);
		background: color-mix(in srgb, var(--lens-info, #67c7e6) 8%, var(--lens-surface));
		border: 1px solid color-mix(in srgb, var(--lens-info, #67c7e6) 25%, var(--lens-border));
		border-radius: var(--lens-radius-sm);
	}
	.zero-hint code {
		font-family: var(--lens-font-mono);
		color: var(--lens-text);
	}

	h3 .count {
		font-size: var(--lens-font-size-xs);
		font-weight: 400;
		color: var(--lens-muted);
		margin-left: 0.4rem;
	}
	.turns {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.6rem;
		max-height: 460px;
		overflow-y: auto;
	}
	.turn {
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		background: var(--lens-surface);
		padding: 0.5rem 0.6rem;
	}
	.turn-head {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
		margin-bottom: 0.4rem;
	}
	.turn-idx {
		font-weight: 700;
		color: var(--lens-text);
	}
	.turn-tok {
		margin-left: auto;
	}
	.msg {
		display: grid;
		grid-template-columns: 68px 1fr;
		gap: 0.5rem;
		padding: 0.25rem 0;
	}
	.msg-role {
		font-size: var(--lens-font-size-2xs);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		padding-top: 0.15rem;
	}
	.msg.user .msg-role {
		color: var(--lens-accent);
	}
	.msg.assistant .msg-role {
		color: var(--lens-ok);
	}
	.msg.tool .msg-role {
		color: var(--lens-info, #67c7e6);
	}
	.msg-body {
		font-size: var(--lens-font-size-xs);
		line-height: 1.5;
		color: var(--lens-text);
		white-space: pre-wrap;
		word-break: break-word;
		min-width: 0;
	}
	.tool-toggle {
		background: none;
		border: none;
		cursor: pointer;
		text-align: left;
		padding: 0.15rem 0;
		font-family: inherit;
	}
	.tool-summary {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
	}
	.tool-raw {
		margin: 0.35rem 0 0;
		padding: 0.4rem 0.5rem;
		background: color-mix(in srgb, var(--lens-info, #67c7e6) 6%, var(--lens-surface-raised));
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		font-size: var(--lens-font-size-2xs);
		overflow-x: auto;
		max-height: 260px;
	}

	.session-id {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		margin-top: 0.1rem;
	}
	.detail-id {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
		margin: -0.2rem 0 0.6rem;
	}
</style>
