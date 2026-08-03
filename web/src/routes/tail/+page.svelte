<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { getLog, type CommitEntry } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import ScopeBadge from '$lib/ScopeBadge.svelte';

	/** Tail polls fast (3s) — this page IS the auto-refresh, so it runs its
	 * own interval instead of the global 30s useAutoRefresh loop. */
	const POLL_MS = 3_000;
	const LIMIT = 20;

	let commits: CommitEntry[] = $state([]);
	let error: string | null = $state(null);
	let loaded = $state(false);
	let lastPolled: Date | null = $state(null);
	/** Hovering the feed pauses updates so a row can't move under the cursor. */
	let hoverPaused = $state(false);
	/** Ids that arrived in the most recent poll — flashed briefly. */
	let freshIds: Set<string> = $state(new Set());

	let seenIds = new Set<string>();
	let feedEl: HTMLElement | undefined = $state();

	async function poll(reset = false) {
		if (typeof document !== 'undefined' && document.hidden) return;
		if (hoverPaused && !reset) return;
		try {
			const latest = await getLog(branchStore.current, LIMIT);
			if (reset) {
				seenIds = new Set(latest.map((c) => c.id));
				freshIds = new Set();
			} else {
				const fresh = latest.filter((c) => !seenIds.has(c.id)).map((c) => c.id);
				if (fresh.length > 0) {
					for (const id of fresh) seenIds.add(id);
					freshIds = new Set(fresh);
					// New rows land at the top — keep the feed pinned there so
					// they're visible (unless the user is hovering/reading).
					feedEl?.scrollTo({ top: 0, behavior: 'smooth' });
				}
			}
			commits = latest;
			error = null;
			lastPolled = new Date();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load commits';
		} finally {
			loaded = true;
		}
	}

	onMount(() => {
		const id = setInterval(() => void poll(), POLL_MS);
		// Poll immediately when the tab becomes visible again (the interval
		// skips ticks while document.hidden).
		const onVisibility = () => {
			if (!document.hidden) void poll();
		};
		document.addEventListener('visibilitychange', onVisibility);
		return () => {
			clearInterval(id);
			document.removeEventListener('visibilitychange', onVisibility);
		};
	});

	// Reset the feed whenever the branch or namespace changes. `poll`
	// reads other state (hoverPaused) — untracked so it can't re-trigger.
	$effect(() => {
		void branchStore.current;
		void namespaceStore.current;
		loaded = false;
		commits = [];
		untrack(() => void poll(true));
	});

	function fmtTime(iso: string): string {
		return iso.slice(11, 19);
	}
	function fmtDay(iso: string): string {
		return iso.slice(0, 10);
	}
</script>

<h2>
	Tail <ScopeBadge branch />
	{#if hoverPaused}
		<span class="live-pill paused">⏸ paused</span>
	{:else}
		<span class="live-pill"><span class="pulse"></span> Live</span>
	{/if}
	<span class="ago">
		polling every {POLL_MS / 1000}s{lastPolled
			? ` · last ${lastPolled.toLocaleTimeString()}`
			: ''}
	</span>
</h2>
<p class="hint">
	Newest commit first. Updates pause while you hover the feed (and while the tab is hidden).
</p>

{#if error}
	<p class="error">{error}</p>
{/if}

<div
	class="feed"
	bind:this={feedEl}
	onmouseenter={() => (hoverPaused = true)}
	onmouseleave={() => {
		hoverPaused = false;
		freshIds = new Set();
	}}
	role="log"
	aria-label="Live commit feed"
>
	{#each commits as commit (commit.id)}
		<div class="commit" class:fresh={freshIds.has(commit.id)}>
			<div class="commit-header">
				<span class="commit-time" title={commit.timestamp}>
					{fmtDay(commit.timestamp)} {fmtTime(commit.timestamp)}
				</span>
				<span class="commit-agent">{commit.agent_id}</span>
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
		</div>
	{/each}

	{#if commits.length === 0 && loaded && !error}
		<p class="empty">No commits yet on {branchStore.current}. They'll appear here live.</p>
	{:else if !loaded && !error}
		<p class="empty">Connecting…</p>
	{/if}
</div>

<style>

	.live-pill {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		font-size: 0.7rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--accent);
		background: var(--accent-bg);
		border: 1px solid var(--accent-bg-hi);
		padding: 0.15rem 0.55rem;
		border-radius: 999px;
		margin-left: 0.75rem;
		vertical-align: middle;
	}

	.live-pill.paused {
		color: var(--text-2);
		background: var(--bg-1);
		border-color: var(--border);
	}

	.pulse {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--accent);
		animation: pulse 1.6s ease-in-out infinite;
	}

	@keyframes pulse {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.25;
		}
	}

	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		font-weight: normal;
		margin-left: 0.75rem;
	}

	.hint {
		color: var(--text-3);
		font-size: 0.85rem;
		margin: 0 0 1rem;
	}

	.error {
		color: var(--danger);
	}

	.feed {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow-y: auto;
		max-height: calc(100vh - 12rem);
	}

	.commit {
		padding: 0.75rem 1rem;
		border-bottom: 1px solid var(--bg-hover);
	}

	.commit:last-child {
		border-bottom: none;
	}

	.commit.fresh {
		animation: flash 2.5s ease-out;
	}

	@keyframes flash {
		0% {
			background: var(--accent-bg);
		}
		100% {
			background: transparent;
		}
	}

	.commit-header {
		display: flex;
		gap: 0.75rem;
		margin-bottom: 0.25rem;
	}

	.commit-time {
		font-family: monospace;
		font-size: 0.8rem;
		color: var(--text-3);
	}

	.commit-agent {
		font-family: monospace;
		font-size: 0.8rem;
		color: var(--text-2);
		margin-right: auto;
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

	.commit-desc {
		color: var(--text-1);
		font-size: 0.9rem;
	}

	.commit-meta {
		font-size: 0.75rem;
		color: var(--text-3);
		margin-top: 0.25rem;
	}

	.empty {
		color: var(--text-3);
		padding: 2rem;
		text-align: center;
	}
</style>
