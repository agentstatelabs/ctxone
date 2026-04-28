<script lang="ts">
	import { onMount } from 'svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	const API_BASE: string = import.meta.env.VITE_CTXONE_API_URL
		?? (import.meta.env.DEV ? 'http://localhost:3001' : '');

	interface Session {
		session_id: string;
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

	$effect(() => {
		if (selected) {
			loadMemories(selected.session_id);
		} else {
			memories = [];
		}
	});

	async function loadMemories(sessionId: string) {
		memoriesLoading = true;
		try {
			const r = await fetch(`${API_BASE}/api/log/main?limit=500`);
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
			const r = await fetch(`${API_BASE}/api/stats/sessions`);
			if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
			sessions = await r.json();
			sessions.sort((a, b) => b.session_tokens_used - a.session_tokens_used);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	onMount(load);

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

	function fmt(n: number): string {
		return n.toLocaleString();
	}
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
						<div class="session-name">{s.session_id}</div>
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
					<h2>{selected.session_id}</h2>

					<div class="stat-grid">
						<div class="stat">
							<div class="stat-value">{fmt(selected.session_tokens_used)}</div>
							<div class="stat-label">Tokens used</div>
						</div>
						<div class="stat">
							<div class="stat-value" style="color: var(--success)">{fmt(selected.session_tokens_saved)}</div>
							<div class="stat-label">Tokens saved</div>
						</div>
						<div class="stat">
							<div class="stat-value">{fmt(selected.total_graph_size_tokens)}</div>
							<div class="stat-label">Graph size (tokens)</div>
						</div>
						<div class="stat">
							<div class="stat-value" style="color: {ratioColor(selected.cumulative_ratio)}; font-size: 2rem">
								{selected.cumulative_ratio.toFixed(1)}x
							</div>
							<div class="stat-label">Savings ratio</div>
						</div>
					</div>

					{#if selected.llm_call_count > 0}
						<h3>LLM Consumption</h3>
						<div class="stat-grid">
							<div class="stat">
								<div class="stat-value">{fmt(selected.llm_call_count)}</div>
								<div class="stat-label">API calls</div>
							</div>
							<div class="stat">
								<div class="stat-value">{fmt(selected.llm_input_tokens)}</div>
								<div class="stat-label">Input tokens</div>
							</div>
							<div class="stat">
								<div class="stat-value">{fmt(selected.llm_output_tokens)}</div>
								<div class="stat-label">Output tokens</div>
							</div>
							<div class="stat">
								<div class="stat-value">{fmt(selected.llm_cache_read_tokens)}</div>
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
</style>
