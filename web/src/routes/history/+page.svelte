<script lang="ts">
	import { page } from '$app/stores';
	import { getLog } from '$lib/api';
	import type { CommitEntry } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import ScopeBadge from '$lib/ScopeBadge.svelte';
	import EmptyState from '$lib/EmptyState.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	let commits: CommitEntry[] = $state([]);
	let selectedCommit: CommitEntry | null = $state(null);
	let error: string | null = $state(null);

	// Deep link (?commit=<id>) from /why et al: pre-select the commit on
	// first load. Consumed once so later refreshes don't re-select.
	let pendingCommit: string | null = $page.url.searchParams.get('commit');

	async function loadLog() {
		error = null;
		selectedCommit = null;
		try {
			commits = await getLog(branchStore.current, 50);
			if (pendingCommit) {
				const want = pendingCommit;
				const matches = (c: CommitEntry) => c.id.startsWith(want) || want.startsWith(c.id);
				let hit = commits.find(matches) ?? null;
				if (!hit) {
					// Older than the default window — dig deeper once (the
					// engine walks at most 1000 commits anyway).
					const deeper = await getLog(branchStore.current, 1000);
					const deepHit = deeper.find(matches);
					if (deepHit) {
						commits = deeper;
						hit = deepHit;
					}
				}
				selectedCommit = hit;
				pendingCommit = null;
				if (hit) {
					// Bring the pre-selected commit into view once rendered.
					setTimeout(() => {
						document
							.querySelector('.commit.selected')
							?.scrollIntoView({ block: 'center', behavior: 'smooth' });
					}, 50);
				}
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load history';
			commits = [];
		}
	}

	$effect(() => {
		// Re-load whenever the active branch or namespace changes
		void branchStore.current;
		void namespaceStore.current;
		loadLog();
	});

	const auto = useAutoRefresh(loadLog);
</script>

<h2>
	Commit History <ScopeBadge branch />
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
		<EmptyState
			icon="🕓"
			title="No commits on this branch yet"
			description="Every remember, plan change, and ingest writes a commit here. Nothing has been recorded on this branch."
		/>
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


	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		font-weight: normal;
		margin-left: 0.75rem;
	}
</style>
