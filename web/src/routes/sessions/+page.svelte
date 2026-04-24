<script lang="ts">
	import { onMount } from 'svelte';

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

	let sessions: Session[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);
	let selected: Session | null = $state(null);

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

	function ratioColor(r: number): string {
		if (r >= 5) return '#4ade80';
		if (r >= 2) return '#86efac';
		if (r >= 1.2) return '#93c5fd';
		return '#888';
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
							<div class="stat-value" style="color: #4ade80">{fmt(selected.session_tokens_saved)}</div>
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
		background: #1a1a1a;
		border: 1px solid #333;
		color: #888;
		padding: 0.35rem 0.75rem;
		border-radius: 6px;
		cursor: pointer;
		font-size: 0.85rem;
	}

	.refresh-btn:hover:not(:disabled) { color: #fff; border-color: #555; }
	.refresh-btn:disabled { opacity: 0.5; cursor: default; }

	.error { color: #ef4444; }
	.muted { color: #555; font-size: 0.9rem; }

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
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		padding: 0.75rem 1rem;
		text-align: left;
		cursor: pointer;
		color: #e0e0e0;
		transition: all 0.15s;
	}

	.session-row:hover { border-color: #444; background: #161616; }
	.session-row.active { border-color: #3b82f6; background: #0f1f3d; }

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
		color: #666;
	}

	.ratio { font-weight: 600; }

	.detail {
		background: #111;
		border: 1px solid #222;
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
		color: #666;
	}

	.stat-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
		gap: 1rem;
	}

	.stat {
		background: #0a0a0a;
		border: 1px solid #1e1e1e;
		border-radius: 8px;
		padding: 1rem;
	}

	.stat-value {
		font-size: 1.5rem;
		font-weight: 700;
		color: #fff;
	}

	.stat-label {
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: #555;
		margin-top: 0.25rem;
	}

	.model-info {
		margin-top: 1rem;
		font-size: 0.85rem;
		color: #666;
	}

	.hint { margin-top: 1rem; }

	code {
		background: #1a1a1a;
		border: 1px solid #2a2a2a;
		padding: 0.1em 0.35em;
		border-radius: 3px;
		font-size: 0.85em;
	}
</style>
