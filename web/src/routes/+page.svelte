<script lang="ts">
	import { onMount } from 'svelte';
	import { getHealth, getStats, getLog, getTokenStats } from '$lib/api';
	import type { StatsResponse, CommitEntry, TokenStats } from '$lib/api';

	let connected = $state(false);
	let stats: StatsResponse | null = $state(null);
	let tokenStats: TokenStats | null = $state(null);
	let recentCommits: CommitEntry[] = $state([]);
	let error: string | null = $state(null);

	onMount(async () => {
		connected = await getHealth();
		if (connected) {
			try {
				stats = await getStats();
				tokenStats = await getTokenStats();
				recentCommits = await getLog('main', 5);
			} catch (e) {
				error = e instanceof Error ? e.message : 'Unknown error';
			}
		}
	});
</script>

<h2>Dashboard</h2>

<div class="status">
	<div class="indicator" class:connected class:disconnected={!connected}></div>
	<span>{connected ? 'Hub connected' : 'Hub unreachable'}</span>
</div>

{#if error}
	<p class="error">{error}</p>
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

	.connected { background: #22c55e; }
	.disconnected { background: #ef4444; }

	.error { color: #ef4444; }

	.stats-grid {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: 1rem;
		margin-bottom: 2rem;
	}

	.stat-card {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		padding: 1.5rem;
		text-align: center;
	}

	.stat-value {
		font-size: 2rem;
		font-weight: 700;
		color: #fff;
	}

	.stat-label {
		font-size: 0.8rem;
		color: #666;
		text-transform: uppercase;
		margin-top: 0.25rem;
	}

	.commits {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		overflow: hidden;
	}

	.commit {
		padding: 0.75rem 1rem;
		border-bottom: 1px solid #1a1a1a;
		font-size: 0.9rem;
	}

	.commit:last-child { border-bottom: none; }

	.commit-time {
		color: #555;
		font-family: monospace;
		font-size: 0.8rem;
	}

	.commit-category {
		color: #3b82f6;
		margin: 0 0.5rem;
	}

	.commit-desc { color: #ccc; }

	.savings-card {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		padding: 1.25rem 1.5rem;
		margin-bottom: 2rem;
	}

	.savings-row {
		display: flex;
		justify-content: space-between;
		padding: 0.5rem 0;
		border-bottom: 1px solid #1a1a1a;
	}

	.savings-row:last-child { border-bottom: none; }
	.savings-row.big { padding-top: 0.75rem; }

	.savings-label { color: #888; font-size: 0.9rem; }
	.savings-value { color: #fff; font-family: monospace; }
	.savings-value.saved { color: #22c55e; }
	.savings-value.ratio { color: #3b82f6; font-size: 1.4rem; font-weight: 700; }
</style>
