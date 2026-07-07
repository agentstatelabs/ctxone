<script lang="ts">
	import { getHealth, getStats, getLog, getTokenStats, remember } from '$lib/api';
	import type { StatsResponse, CommitEntry, TokenStats, SessionSnapshot } from '$lib/api';
	import { listPlans, type Plan } from '$lib/plansApi';
	import LlmConsumptionPanel from '$lib/LlmConsumptionPanel.svelte';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	let connected = $state(false);
	let stats: StatsResponse | null = $state(null);
	let tokenStats: TokenStats | null = $state(null);
	let recentCommits: CommitEntry[] = $state([]);
	let plans: Plan[] = $state([]);
	let error: string | null = $state(null);

	// Remember form state
	let factText = $state('');
	let factImportance: 'high' | 'medium' | 'low' = $state('medium');
	let factContext = $state('');
	let saving = $state(false);
	let saveMessage: string | null = $state(null);

	async function refresh() {
		try {
			stats = await getStats();
			tokenStats = await getTokenStats();
			recentCommits = await getLog('main', 5);
			plans = await listPlans('main');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Unknown error';
		}
	}

	function planSummary(all: Plan[]) {
		let active = 0;
		let pending = 0;
		let inProgress = 0;
		for (const p of all) {
			if (p.status === 'active') active += 1;
			pending += p.task_counts.pending;
			inProgress += p.task_counts.in_progress;
		}
		return { active, pending, inProgress };
	}

	// Load on mount and re-load whenever the active namespace changes.
	$effect(() => {
		void namespaceStore.current;
		void (async () => {
			connected = await getHealth();
			if (connected) {
				await refresh();
			}
		})();
	});

	const auto = useAutoRefresh(async () => {
		connected = await getHealth();
		if (connected) await refresh();
	});

	async function handleRemember(e: SubmitEvent) {
		e.preventDefault();
		if (!factText.trim()) return;
		saving = true;
		saveMessage = null;
		try {
			const result = await remember({
				fact: factText,
				importance: factImportance,
				context: factContext.trim() || undefined
			});
			saveMessage = `Saved: ${result.path}`;
			factText = '';
			factContext = '';
			await refresh();
		} catch (e) {
			saveMessage = e instanceof Error ? e.message : 'Save failed';
		} finally {
			saving = false;
		}
	}
</script>

<h2>
	Dashboard
	<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
</h2>

<div class="status">
	<div class="indicator" class:connected class:disconnected={!connected}></div>
	<span>{connected ? 'Hub connected' : 'Hub unreachable'}</span>
</div>

{#if error}
	<p class="error">{error}</p>
{/if}

{#if connected}
	<form class="remember-form" onsubmit={handleRemember}>
		<h3>Remember a fact</h3>
		<textarea
			bind:value={factText}
			placeholder="e.g., We use BSL-1.1 for all projects"
			rows="2"
			disabled={saving}
		></textarea>
		<div class="remember-row">
			<select bind:value={factImportance} disabled={saving}>
				<option value="high">High</option>
				<option value="medium">Medium</option>
				<option value="low">Low</option>
			</select>
			<input
				type="text"
				bind:value={factContext}
				placeholder="context (e.g., licensing)"
				disabled={saving}
			/>
			<button type="submit" disabled={saving || !factText.trim()}>
				{saving ? 'Saving...' : 'Remember'}
			</button>
		</div>
		{#if saveMessage}
			<p class="save-message">{saveMessage}</p>
		{/if}
	</form>
{/if}

{#if stats}
	<div class="stats-grid">
		<div class="stat-card">
			<div class="stat-value">{stats.path_count}</div>
			<div class="stat-label">Paths</div>
		</div>
		<div class="stat-card">
			<div class="stat-value">{stats.commit_count}</div>
			<div class="stat-label">Commits</div>
		</div>
		<div class="stat-card">
			<div class="stat-value">{stats.branch_count}</div>
			<div class="stat-label">Branches</div>
		</div>
		<div class="stat-card">
			<div class="stat-value">{stats.epoch_count}</div>
			<div class="stat-label">Epochs</div>
		</div>
	</div>
{/if}

{#if tokenStats}
	<h3>Token Savings</h3>
	<div class="savings-card">
		<div class="savings-row">
			<span class="savings-label">Tokens sent this session</span>
			<span class="savings-value">{tokenStats.session_tokens_used.toLocaleString()}</span>
		</div>
		<div class="savings-row">
			<span class="savings-label">Tokens saved vs flat memory</span>
			<span class="savings-value saved">{tokenStats.session_tokens_saved.toLocaleString()}</span>
		</div>
		<div class="savings-row">
			<span class="savings-label">Graph size (flat equivalent)</span>
			<span class="savings-value">{tokenStats.total_graph_size_tokens.toLocaleString()}</span>
		</div>
		<div class="savings-row big">
			<span class="savings-label">Savings ratio</span>
			<span class="savings-value ratio">{tokenStats.cumulative_ratio.toFixed(1)}x</span>
		</div>
	</div>
{/if}

{#if tokenStats}
	<LlmConsumptionPanel snapshot={{ session_id: '_aggregate', ...tokenStats } as SessionSnapshot} />
{/if}

{#if connected}
	<h3>Plans <a class="see-all" href="/plans">see all</a></h3>
	{@const summary = planSummary(plans)}
	<div class="stats-grid plans-grid">
		<div class="stat-card">
			<div class="stat-value">{summary.active}</div>
			<div class="stat-label">Active plans</div>
		</div>
		<div class="stat-card">
			<div class="stat-value">{summary.pending}</div>
			<div class="stat-label">Pending tasks</div>
		</div>
		<div class="stat-card">
			<div class="stat-value">{summary.inProgress}</div>
			<div class="stat-label">In-progress tasks</div>
		</div>
	</div>
{/if}

{#if recentCommits.length > 0}
	<h3>Recent Activity</h3>
	<div class="commits">
		{#each recentCommits as commit}
			<div class="commit">
				<span class="commit-time">{commit.timestamp.slice(0, 19)}</span>
				<span class="commit-category">[{commit.intent.category}]</span>
				<span class="commit-desc">{commit.intent.description}</span>
			</div>
		{/each}
	</div>
{/if}

<style>
	.status {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin-bottom: 2rem;
	}

	.indicator {
		width: 10px;
		height: 10px;
		border-radius: 50%;
	}

	.connected { background: var(--success); }
	.disconnected { background: var(--danger); }

	.error { color: var(--danger); }

	.stats-grid {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: 1rem;
		margin-bottom: 2rem;
	}

	.plans-grid {
		grid-template-columns: repeat(3, 1fr);
	}

	.see-all {
		font-size: 0.75rem;
		color: var(--text-3);
		font-weight: normal;
		margin-left: 0.5rem;
	}
	.see-all:hover {
		color: var(--accent);
	}

	.stat-card {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1.5rem;
		text-align: center;
	}

	.stat-value {
		font-size: 2rem;
		font-weight: 700;
		color: var(--text-0);
	}

	.stat-label {
		font-size: 0.8rem;
		color: var(--text-3);
		text-transform: uppercase;
		margin-top: 0.25rem;
	}

	.commits {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow: hidden;
	}

	.commit {
		padding: 0.75rem 1rem;
		border-bottom: 1px solid var(--bg-hover);
		font-size: 0.9rem;
	}

	.commit:last-child { border-bottom: none; }

	.commit-time {
		color: var(--text-3);
		font-family: monospace;
		font-size: 0.8rem;
	}

	.commit-category {
		color: var(--accent);
		margin: 0 0.5rem;
	}

	.commit-desc { color: var(--text-1); }

	.savings-card {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1.25rem 1.5rem;
		margin-bottom: 2rem;
	}

	.savings-row {
		display: flex;
		justify-content: space-between;
		padding: 0.5rem 0;
		border-bottom: 1px solid var(--bg-hover);
	}

	.savings-row:last-child { border-bottom: none; }
	.savings-row.big { padding-top: 0.75rem; }

	.savings-label { color: var(--text-2); font-size: 0.9rem; }
	.savings-value { color: var(--text-0); font-family: monospace; }
	.savings-value.saved { color: var(--success); }
	.savings-value.ratio { color: var(--accent); font-size: 1.4rem; font-weight: 700; }

	.remember-form {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1.25rem 1.5rem;
		margin-bottom: 2rem;
	}

	.remember-form h3 {
		margin: 0 0 0.75rem 0;
		color: var(--text-0);
		font-size: 1rem;
		font-weight: 600;
	}

	.remember-form textarea {
		width: 100%;
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text-1);
		padding: 0.625rem 0.75rem;
		font-size: 0.95rem;
		font-family: inherit;
		box-sizing: border-box;
		resize: vertical;
	}

	.remember-form textarea:focus,
	.remember-form input:focus,
	.remember-form select:focus {
		outline: none;
		border-color: var(--border-hi);
	}

	.remember-row {
		display: flex;
		gap: 0.5rem;
		margin-top: 0.5rem;
	}

	.remember-form select,
	.remember-form input {
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text-1);
		padding: 0.5rem 0.75rem;
		font-size: 0.9rem;
		font-family: inherit;
	}

	.remember-form input {
		flex: 1;
	}

	.remember-form button {
		background: var(--accent);
		border: none;
		border-radius: 6px;
		color: var(--text-0);
		padding: 0.5rem 1rem;
		cursor: pointer;
		font-size: 0.9rem;
		font-weight: 500;
	}

	.remember-form button:hover:not(:disabled) {
		background: color-mix(in srgb, var(--accent) 80%, black);
	}

	.remember-form button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.save-message {
		color: var(--success);
		font-size: 0.85rem;
		margin: 0.5rem 0 0 0;
		font-family: monospace;
	}

	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		font-weight: normal;
		margin-left: 0.75rem;
	}
</style>
