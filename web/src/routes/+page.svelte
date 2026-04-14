<script lang="ts">
	import { onMount } from 'svelte';
	import { getHealth, getStats, getLog } from '$lib/api';
	import type { StatsResponse, CommitEntry } from '$lib/api';

	let connected = $state(false);
	let stats: StatsResponse | null = $state(null);
	let recentCommits: CommitEntry[] = $state([]);
	let error: string | null = $state(null);

	onMount(async () => {
		connected = await getHealth();
		if (connected) {
			try {
				stats = await getStats();
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
			<div class="stat-value">{stats.total_paths}</div>
			<div class="stat-label">Paths</div>
		</div>
		<div class="stat-card">
			<div class="stat-value">{stats.total_commits}</div>
			<div class="stat-label">Commits</div>
		</div>
		<div class="stat-card">
			<div class="stat-value">{stats.branches}</div>
			<div class="stat-label">Branches</div>
		</div>
		<div class="stat-card">
			<div class="stat-value">{stats.epochs}</div>
			<div class="stat-label">Epochs</div>
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
</style>
