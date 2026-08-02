<script lang="ts">
	import { getSessionSegments, type SessionSegment } from '$lib/api';
	import { formatCompact } from '@agentstate/lens-core';

	interface Props {
		sessionId: string;
		/** Called when an arc is clicked — lets the page scroll to those turns. */
		onScrollToTurn?: (turnIndex: number) => void;
	}

	let { sessionId, onScrollToTurn }: Props = $props();

	let segments = $state<SessionSegment[]>([]);
	let gapMinutes = $state(30);
	let loading = $state(false);
	let error = $state<string | null>(null);

	async function load(sid: string, gap: number) {
		loading = true;
		error = null;
		try {
			const r = await getSessionSegments(sid, gap);
			segments = r.segments;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			segments = [];
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		if (sessionId) load(sessionId, gapMinutes);
		else segments = [];
	});

	let totalTurns = $derived(segments.reduce((n, s) => n + s.turn_count, 0));

	/** Why an arc split, in words — the "40 turns = 3 topics" story. */
	function reasonLabel(reason: string): string {
		switch (reason) {
			case 'start':
				return 'session start';
			case 'branch':
				return 'branch switch';
			case 'cwd':
				return 'directory change';
			case 'gap':
				return 'idle gap';
			default:
				return reason;
		}
	}

	/** Short cwd tail for the arc subtitle. */
	function shortCwd(cwd: string | null): string | null {
		if (!cwd) return null;
		const parts = cwd.split('/').filter(Boolean);
		return parts.length <= 2 ? cwd : `…/${parts.slice(-2).join('/')}`;
	}
</script>

<h3>
	Session arcs
	{#if segments.length}<span class="count">{segments.length}</span>{/if}
</h3>

{#if loading}
	<p class="muted">Segmenting session…</p>
{:else if error}
	<p class="muted hint">Arcs unavailable: {error}</p>
{:else if segments.length === 0}
	<p class="muted hint">
		No arcs for this session. Arcs are derived from captured turns — they
		appear once an agent posts to <code>/api/sessions/{'{sid}'}/turns</code>.
	</p>
{:else}
	<div class="arcs-gap">
		<span class="muted">Split on idle gaps over</span>
		<div class="seg-group" role="radiogroup" aria-label="Idle-gap threshold">
			{#each [15, 30, 60, 0] as g (g)}
				<button
					type="button"
					class="seg"
					class:active={gapMinutes === g}
					role="radio"
					aria-checked={gapMinutes === g}
					onclick={() => (gapMinutes = g)}
				>{g === 0 ? 'off' : `${g}m`}</button>
			{/each}
		</div>
	</div>

	<!-- Proportional band: each arc's width tracks its turn share. -->
	<div class="band" aria-label="Session arcs">
		{#each segments as s, i (s.start)}
			<button
				type="button"
				class="band-arc reason-{s.reason}"
				style:flex-grow={s.turn_count}
				title={`${s.label} · turns ${s.start + 1}–${s.end + 1} · ${s.turn_count} turns · ${formatCompact(s.tokens)} tokens · split: ${reasonLabel(s.reason)}`}
				onclick={() => onScrollToTurn?.(s.start)}
			>
				<span class="band-idx">{i + 1}</span>
			</button>
		{/each}
	</div>

	<ol class="arc-list">
		{#each segments as s, i (s.start)}
			<li>
				<button
					type="button"
					class="arc-item"
					onclick={() => onScrollToTurn?.(s.start)}
					title="Scroll to turn #{s.start + 1}"
				>
					<div class="arc-head">
						<span class="arc-idx">{i + 1}</span>
						<span class="arc-label">{s.label}</span>
						<span class="arc-reason reason-{s.reason}">{reasonLabel(s.reason)}</span>
					</div>
					<div class="arc-meta">
						<span class="arc-range">turns {s.start + 1}–{s.end + 1}</span>
						<span class="arc-dot">·</span>
						<span>{s.turn_count} turn{s.turn_count === 1 ? '' : 's'}</span>
						<span class="arc-dot">·</span>
						<span title="{s.tokens} tokens">{formatCompact(s.tokens)} tok</span>
						{#if s.branch}
							<span class="arc-dot">·</span>
							<span class="arc-branch" title="branch">⎇ {s.branch}</span>
						{/if}
						{#if shortCwd(s.cwd)}
							<span class="arc-dot">·</span>
							<span class="arc-cwd" title={s.cwd}>{shortCwd(s.cwd)}</span>
						{/if}
					</div>
				</button>
			</li>
		{/each}
	</ol>
	<p class="muted arc-foot">
		{totalTurns} turns across {segments.length} arc{segments.length === 1 ? '' : 's'}.
		Click an arc to jump to its turns.
	</p>
{/if}

<style>
	.count {
		font-size: 0.75rem;
		font-weight: 500;
		color: var(--text-2);
		background: var(--bg-2);
		border-radius: 999px;
		padding: 0.05rem 0.5rem;
		margin-left: 0.4rem;
	}

	.muted {
		color: var(--text-2);
		font-size: 0.85rem;
	}
	.hint code {
		background: var(--bg-2);
		border: 1px solid var(--border);
		border-radius: 3px;
		padding: 0 0.25rem;
		font-size: 0.85em;
	}

	.arcs-gap {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin: 0.25rem 0 0.75rem;
	}
	.seg-group {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: 6px;
		overflow: hidden;
	}
	.seg {
		background: var(--bg-1);
		color: var(--text-2);
		border: none;
		border-left: 1px solid var(--border);
		padding: 0.15rem 0.55rem;
		font-size: 0.8rem;
		cursor: pointer;
	}
	.seg:first-child {
		border-left: none;
	}
	.seg.active {
		background: var(--accent-bg);
		color: var(--text-0);
	}

	.band {
		display: flex;
		gap: 3px;
		height: 34px;
		margin-bottom: 0.75rem;
	}
	.band-arc {
		flex-basis: 0;
		min-width: 22px;
		border: 1px solid var(--border);
		border-radius: 5px;
		background: var(--accent-bg);
		color: var(--text-0);
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 0.75rem;
		font-weight: 600;
		transition: filter 0.1s;
	}
	.band-arc:hover {
		filter: brightness(1.2);
		border-color: var(--border-hi);
	}
	/* Tint by why the arc split, so the visual carries the reason. */
	.band-arc.reason-branch {
		background: var(--accent-bg-hi);
	}
	.band-arc.reason-cwd {
		background: color-mix(in srgb, var(--warning) 22%, var(--bg-2));
	}
	.band-arc.reason-gap {
		background: var(--bg-2);
	}

	.arc-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
	}
	.arc-item {
		width: 100%;
		text-align: left;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 0.5rem 0.7rem;
		cursor: pointer;
	}
	.arc-item:hover {
		border-color: var(--border-hi);
		background: var(--bg-hover);
	}
	.arc-head {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}
	.arc-idx {
		flex: none;
		width: 1.4rem;
		height: 1.4rem;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		background: var(--bg-2);
		border-radius: 999px;
		font-size: 0.75rem;
		font-weight: 600;
		color: var(--text-1);
	}
	.arc-label {
		font-weight: 600;
		color: var(--text-0);
		flex: 1 1 auto;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.arc-reason {
		flex: none;
		font-size: 0.7rem;
		color: var(--text-2);
		background: var(--bg-2);
		border-radius: 4px;
		padding: 0.1rem 0.4rem;
	}
	.arc-reason.reason-branch {
		color: var(--accent);
		background: var(--accent-bg);
	}
	.arc-reason.reason-cwd {
		color: var(--warning);
	}
	.arc-meta {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.3rem;
		margin-top: 0.3rem;
		margin-left: 1.9rem;
		font-size: 0.8rem;
		color: var(--text-2);
	}
	.arc-dot {
		opacity: 0.5;
	}
	.arc-branch,
	.arc-cwd {
		font-family: monospace;
		font-size: 0.75rem;
	}
	.arc-foot {
		margin-top: 0.5rem;
	}
</style>
