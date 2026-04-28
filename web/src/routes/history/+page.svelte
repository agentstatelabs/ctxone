<script lang="ts">
	import { getLog } from '$lib/api';
	import type { CommitEntry } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	let commits: CommitEntry[] = $state([]);
	let selectedCommit: CommitEntry | null = $state(null);
	let error: string | null = $state(null);

	async function loadLog() {
		error = null;
		selectedCommit = null;
		try {
			commits = await getLog(branchStore.current, 50);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load history';
			commits = [];
		}
	}

	$effect(() => {
		// Re-load whenever the active branch changes
		void branchStore.current;
		loadLog();
	});

	const auto = useAutoRefresh(loadLog);
</script>

<h2>
	Commit History <span class="branch-label">on {branchStore.current}</span>
	<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
</h2>

{#if error}
	<p class="error">{error}</p>
{/if}

<div class="history">
	{#each commits as commit}
		<div
			class="commit"
			class:selected={selectedCommit?.id === commit.id}
			onclick={() => selectedCommit = commit}
			role="button"
			tabindex="0"
			onkeydown={(e) => e.key === 'Enter' && (selectedCommit = commit)}
		>
			<div class="commit-header">
				<span class="commit-time">{commit.timestamp.slice(0, 19)}</span>
				<span class="commit-id">{commit.id.slice(0, 8)}</span>
			</div>
			<div class="commit-body">
				<span class="commit-category">{commit.intent.category}</span>
				<span class="commit-desc">{commit.intent.description}</span>
			</div>
			{#if commit.intent.confidence}
				<div class="commit-meta">
					confidence: {(commit.intent.confidence * 100).toFixed(0)}%
				</div>
			{/if}
			{#if selectedCommit?.id === commit.id && commit.intent.reasoning}
				<div class="commit-reasoning">
					<strong>Reasoning:</strong> {commit.intent.reasoning}
				</div>
			{/if}
		</div>
	{/each}

	{#if commits.length === 0 && !error}
		<p class="empty">No commits yet.</p>
	{/if}
</div>

<style>
	.history {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow: hidden;
	}

	.commit {
		padding: 0.75rem 1rem;
		border-bottom: 1px solid var(--bg-hover);
		cursor: pointer;
		transition: background 0.1s;
	}

	.commit:hover { background: var(--bg-hover); }
	.commit.selected { background: var(--accent-bg); }
	.commit:last-child { border-bottom: none; }

	.commit-header {
		display: flex;
		justify-content: space-between;
		margin-bottom: 0.25rem;
	}

	.commit-time {
		font-family: monospace;
		font-size: 0.8rem;
		color: var(--text-3);
	}

	.commit-id {
		font-family: monospace;
		font-size: 0.75rem;
		color: var(--text-3);
	}

	.commit-category {
		background: var(--accent-bg);
		color: var(--accent);
		padding: 0.1rem 0.5rem;
		border-radius: 3px;
		font-size: 0.75rem;
		margin-right: 0.5rem;
	}

	.commit-desc { color: var(--text-1); font-size: 0.9rem; }

	.commit-meta {
		font-size: 0.75rem;
		color: var(--text-3);
		margin-top: 0.25rem;
	}

	.commit-reasoning {
		margin-top: 0.5rem;
		padding: 0.5rem;
		background: var(--bg-0);
		border-radius: 4px;
		font-size: 0.85rem;
		color: var(--text-2);
	}

	.error { color: var(--danger); }
	.empty { color: var(--text-3); padding: 2rem; text-align: center; }

	.branch-label {
		font-size: 0.85rem;
		font-family: monospace;
		color: var(--accent);
		font-weight: normal;
		margin-left: 0.5rem;
	}

	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		font-weight: normal;
		margin-left: 0.75rem;
	}
</style>
