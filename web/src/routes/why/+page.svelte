<script lang="ts">
	import { untrack } from 'svelte';
	import { whyDidWe, type WhyResponse, type WhyBlame } from '$lib/api';
	import { namespaceStore } from '$lib/namespaceStore.svelte';

	let question = $state('');
	let response: WhyResponse | null = $state(null);
	let searched = $state(false);
	let error: string | null = $state(null);

	async function ask() {
		const q = question.trim();
		if (!q) return;
		searched = true;
		error = null;
		try {
			response = await whyDidWe(q);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Lookup failed';
			response = null;
		}
	}

	function handleSubmit(e: Event) {
		e.preventDefault();
		void ask();
	}

	// The Hub resolves why_did_we against the namespace (always on main),
	// so re-ask when the namespace changes. `question`/`searched` are read
	// untracked so typing doesn't re-trigger the effect.
	$effect(() => {
		void namespaceStore.current;
		untrack(() => {
			if (searched && question.trim()) void ask();
		});
	});

	/** The Hub returns one blame entry per trace (despite docs showing an
	 * array). Normalize both shapes into a flat list. */
	function blameEntries(blame: WhyBlame | WhyBlame[] | null): WhyBlame[] {
		if (!blame) return [];
		return Array.isArray(blame) ? blame : [blame];
	}

	interface TimelineEvent {
		blame: WhyBlame;
		tracePath: string;
	}

	// Flatten every trace's blame chain into one chronological story,
	// oldest first — "first we decided X, then Y".
	let timeline: TimelineEvent[] = $derived.by(() => {
		if (!response) return [];
		const events: TimelineEvent[] = [];
		for (const t of response.traces) {
			for (const b of blameEntries(t.blame)) {
				events.push({ blame: b, tracePath: t.path });
			}
		}
		events.sort(
			(a, b) => new Date(a.blame.timestamp).getTime() - new Date(b.blame.timestamp).getTime()
		);
		return events;
	});

	function fmtWhen(iso: string): string {
		return new Date(iso).toLocaleString(undefined, {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}
</script>

<h2>Why did we…</h2>
<p class="hint">
	Trace a decision back to the commits that made it. Searches decision text on
	<code>main</code> and follows each hit's blame chain.
</p>

<form onsubmit={handleSubmit} class="search-form">
	<input
		type="text"
		bind:value={question}
		placeholder="why did we… (e.g., use BSL-1.1, pick SQLite)"
	/>
	<button type="submit">Trace</button>
</form>

{#if error}
	<p class="error">{error}</p>
{/if}

{#if response}
	{#if timeline.length === 0}
		<p class="muted">
			No decision matching “{response.decision}” found in memory on <code>main</code>.
		</p>
	{:else}
		<p class="count">
			{response.traces.length} matching path{response.traces.length !== 1 ? 's' : ''}, oldest
			first:
		</p>

		<div class="timeline">
			{#each timeline as ev, i}
				<div class="event">
					<div class="event-marker">
						<span class="dot"></span>
						{#if i < timeline.length - 1}<span class="line"></span>{/if}
					</div>
					<div class="event-body">
						<div class="event-when">
							{fmtWhen(ev.blame.timestamp)}
							<span class="agent">by {ev.blame.agent_id}</span>
							<a
								class="commit-link"
								href={`/history?commit=${encodeURIComponent(ev.blame.commit_id)}`}
								title="Open this commit in History"
							>
								{ev.blame.commit_id.slice(0, 8)}
							</a>
							{#if ev.blame.timestamp_anomaly}
								<span class="anomaly" title="Commit timestamp ≤ a parent's — possible clock rewind">
									⚠ timestamp anomaly
								</span>
							{/if}
						</div>
						<div class="event-desc">
							<span class="category">{ev.blame.intent_category}</span>
							{ev.blame.intent_description}
						</div>
						{#if ev.blame.reasoning}
							<div class="event-reasoning">{ev.blame.reasoning}</div>
						{/if}
						<a class="event-path" href={`/browse?path=${encodeURIComponent(ev.tracePath)}`}>
							{ev.tracePath}
						</a>
					</div>
				</div>
			{/each}
		</div>
	{/if}
{:else if searched && !error}
	<p class="muted">Tracing…</p>
{/if}

<style>
	.hint {
		color: var(--text-3);
		font-size: 0.85rem;
		margin: 0 0 1rem;
	}
	.hint code,
	.muted code {
		font-family: monospace;
		color: var(--text-2);
	}

	.search-form {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 1.5rem;
	}

	input {
		flex: 1;
		padding: 0.75rem 1rem;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text-0);
		font-size: 1rem;
	}

	input:focus {
		outline: none;
		border-color: var(--border-hi);
	}

	button {
		padding: 0.75rem 1.5rem;
		background: var(--accent);
		border: none;
		border-radius: 6px;
		color: var(--text-0);
		cursor: pointer;
		font-size: 1rem;
	}

	button:hover {
		background: color-mix(in srgb, var(--accent) 80%, black);
	}

	.count {
		color: var(--text-3);
		margin-bottom: 1rem;
	}
	.error {
		color: var(--danger);
	}
	.muted {
		color: var(--text-3);
	}

	.timeline {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1rem 1.25rem;
	}

	.event {
		display: flex;
		gap: 0.9rem;
	}

	.event-marker {
		display: flex;
		flex-direction: column;
		align-items: center;
		width: 10px;
		flex-shrink: 0;
	}

	.dot {
		width: 10px;
		height: 10px;
		border-radius: 50%;
		background: var(--accent);
		margin-top: 0.35rem;
		flex-shrink: 0;
	}

	.line {
		flex: 1;
		width: 2px;
		background: var(--border);
		margin: 0.2rem 0;
	}

	.event-body {
		padding-bottom: 1.25rem;
		min-width: 0;
	}
	.event:last-child .event-body {
		padding-bottom: 0;
	}

	.event-when {
		font-family: monospace;
		font-size: 0.78rem;
		color: var(--text-3);
		display: flex;
		gap: 0.6rem;
		align-items: baseline;
		flex-wrap: wrap;
	}

	.agent {
		color: var(--text-2);
	}

	.commit-link {
		color: var(--accent);
		text-decoration: none;
	}
	.commit-link:hover {
		text-decoration: underline;
	}

	.anomaly {
		color: var(--danger);
	}

	.event-desc {
		color: var(--text-1);
		font-size: 0.92rem;
		margin-top: 0.25rem;
	}

	.category {
		background: var(--accent-bg);
		color: var(--accent);
		padding: 0.1rem 0.5rem;
		border-radius: 3px;
		font-size: 0.75rem;
		margin-right: 0.5rem;
	}

	.event-reasoning {
		margin-top: 0.4rem;
		padding: 0.5rem;
		background: var(--bg-0);
		border-radius: 4px;
		font-size: 0.85rem;
		color: var(--text-2);
	}

	.event-path {
		display: inline-block;
		margin-top: 0.4rem;
		font-family: monospace;
		font-size: 0.78rem;
		color: var(--text-3);
		text-decoration: none;
	}
	.event-path:hover {
		color: var(--accent);
		text-decoration: underline;
	}
</style>
