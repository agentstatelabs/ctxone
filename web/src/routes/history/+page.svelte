<script lang="ts">
	import { onMount } from 'svelte';
	import { getLog, getBlame } from '$lib/api';
	import type { CommitEntry, BlameEntry } from '$lib/api';

	let commits: CommitEntry[] = $state([]);
	let selectedCommit: CommitEntry | null = $state(null);
	let error: string | null = $state(null);

	onMount(async () => {
		try {
			commits = await getLog('main', 50);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load history';
		}
	});
</script>

<h2>Commit History</h2>

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
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		overflow: hidden;
	}

	.commit {
		padding: 0.75rem 1rem;
		border-bottom: 1px solid #1a1a1a;
		cursor: pointer;
		transition: background 0.1s;
	}

	.commit:hover { background: #151515; }
	.commit.selected { background: #1a1a2e; }
	.commit:last-child { border-bottom: none; }

	.commit-header {
		display: flex;
		justify-content: space-between;
		margin-bottom: 0.25rem;
	}

	.commit-time {
		font-family: monospace;
		font-size: 0.8rem;
		color: #555;
	}

	.commit-id {
		font-family: monospace;
		font-size: 0.75rem;
		color: #444;
	}

	.commit-category {
		background: #1e3a5f;
		color: #93c5fd;
		padding: 0.1rem 0.5rem;
		border-radius: 3px;
		font-size: 0.75rem;
		margin-right: 0.5rem;
	}

	.commit-desc { color: #ccc; font-size: 0.9rem; }

	.commit-meta {
		font-size: 0.75rem;
		color: #555;
		margin-top: 0.25rem;
	}

	.commit-reasoning {
		margin-top: 0.5rem;
		padding: 0.5rem;
		background: #0a0a0a;
		border-radius: 4px;
		font-size: 0.85rem;
		color: #999;
	}

	.error { color: #ef4444; }
	.empty { color: #555; padding: 2rem; text-align: center; }
</style>
