<script lang="ts">
	import { getThinking } from '$lib/codeApi';
	import type { PriorThinking, ThinkingKind } from '$lib/codeTypes';
	import { selectedRepo } from '$lib/repoStore';

	let data = $state<PriorThinking | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);

	// The default floor (0.3) matches asd's DEFAULT_CONFIDENCE_FLOOR. Slider
	// lets the user dip below it to see suppressed Hypotheses without having
	// to flip a CLI flag.
	let floor = $state(0.3);

	$effect(() => {
		const repo = $selectedRepo;
		if (!repo) {
			loading = false;
			data = null;
			error = null;
			return;
		}
		const f = floor;
		loading = true;
		error = null;
		getThinking(repo, undefined, f)
			.then((d) => {
				data = d;
				loading = false;
			})
			.catch((e) => {
				error = e instanceof Error ? e.message : String(e);
				loading = false;
			});
	});

	type SectionDef = { kind: ThinkingKind; label: string; field: keyof NonNullable<PriorThinking['entries']> };
	const SECTIONS: SectionDef[] = [
		{ kind: 'hypothesis', label: 'Hypotheses', field: 'hypotheses' },
		{ kind: 'mental_model', label: 'Mental models', field: 'mental_models' },
		{ kind: 'open_question', label: 'Open questions', field: 'open_questions' },
		{ kind: 'failed_attempt', label: 'Failed attempts', field: 'failed_attempts' }
	];

	function count(d: PriorThinking | null, kind: ThinkingKind): number {
		return d?.summary?.by_kind?.[kind] ?? 0;
	}
	function dropped(d: PriorThinking | null, kind: ThinkingKind): number {
		return d?.summary?.by_kind_dropped?.[kind] ?? 0;
	}
</script>

<div class="page">
	<header class="page-head">
		<h2>Thinking</h2>
		<p class="muted">
			Hypotheses, mental models, open questions, and failed attempts captured
			by <code>asd think</code>. Below the confidence floor, Hypotheses are
			suppressed — slide it down to inspect them.
		</p>
	</header>

	{#if !$selectedRepo}
		<div class="card">
			<strong>No repo selected.</strong> Pick one from the sidebar's
			<strong>ASD</strong> section.
		</div>
	{:else if loading}
		<p class="muted">loading…</p>
	{:else if error}
		<div class="card error">{error}</div>
	{:else if data}
		<div class="floor-row">
			<label for="floor">Confidence floor: <strong>{floor.toFixed(2)}</strong></label>
			<input
				id="floor"
				type="range"
				min="0"
				max="1"
				step="0.05"
				bind:value={floor}
			/>
		</div>

		<div class="summary">
			<span class="chip">Scanned: <strong>{data.summary.scanned_qnames ?? 0}</strong></span>
			<span class="chip">Surfaced: <strong>{data.summary.surfaced ?? 0}</strong></span>
			<span class="chip">Matched: <strong>{data.summary.matched_for_query ?? 0}</strong></span>
			{#if data.summary.entries_in_workspace !== undefined && data.summary.entries_in_workspace > 0 && (data.summary.surfaced ?? 0) === 0}
				<span class="chip warn"
					>{data.summary.entries_in_workspace} entries exist elsewhere in the workspace</span
				>
			{/if}
		</div>

		<div class="kind-grid">
			{#each SECTIONS as s}
				{@const n = count(data, s.kind)}
				{@const d = dropped(data, s.kind)}
				{@const entries = data.entries?.[s.field] ?? []}
				<section class="kind-card kind-{s.kind}">
					<header>
						<h3>{s.label}</h3>
						<span class="badge">{n}</span>
					</header>

					{#if d > 0 && n === 0}
						<p class="dropped-hint">
							{d}
							{d === 1 ? 'entry exists' : 'entries exist'} below the floor — lower
							the slider to inspect.
						</p>
					{:else if d > 0}
						<p class="dropped-hint subtle">
							{d} more below floor (slider to see)
						</p>
					{/if}

					{#if entries.length === 0}
						<p class="muted small">No surfaced entries.</p>
					{:else}
						<ul class="entry-list">
							{#each entries as e}
								<li class="entry">
									<div class="entry-head">
										{#if 'qname' in e && e.qname}
											<code class="qname">{e.qname}</code>
										{:else if 'name' in e && e.name}
											<code class="qname">{e.name}</code>
										{/if}
										{#if s.kind === 'hypothesis' && 'confidence' in e}
											<span class="conf"
												>conf {(e as { confidence: number }).confidence.toFixed(2)}</span
											>
										{/if}
									</div>
									<div class="entry-summary">{e.summary}</div>
									{#if 'symbols' in e && e.symbols && e.symbols.length}
										<div class="entry-symbols">
											{#each e.symbols as q}
												<code>{q}</code>
											{/each}
										</div>
									{/if}
								</li>
							{/each}
						</ul>
					{/if}
				</section>
			{/each}
		</div>
	{:else}
		<p class="muted">no data</p>
	{/if}
</div>

<style>
	.page {
		max-width: 1100px;
	}
	.page-head h2 {
		margin: 0 0 0.25rem;
	}
	.muted {
		color: var(--text-2);
	}
	.muted.small {
		font-size: 0.85rem;
	}
	.card {
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 1rem;
		background: var(--bg-1);
	}
	.card.error {
		border-color: var(--danger, #b34);
		color: var(--danger, #b34);
	}
	.floor-row {
		display: flex;
		align-items: center;
		gap: 1rem;
		margin: 1rem 0 0.5rem;
	}
	.floor-row input[type='range'] {
		flex: 1;
		max-width: 320px;
	}
	.summary {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		margin-bottom: 1rem;
	}
	.chip {
		font-size: 0.8rem;
		padding: 0.2rem 0.55rem;
		border-radius: 999px;
		background: var(--bg-2, var(--bg-1));
		border: 1px solid var(--border);
		color: var(--text-2);
	}
	.chip.warn {
		border-color: var(--accent);
		color: var(--accent);
	}
	.kind-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(360px, 1fr));
		gap: 1rem;
	}
	.kind-card {
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 1rem;
		background: var(--bg-1);
	}
	.kind-card header {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		margin-bottom: 0.5rem;
	}
	.kind-card h3 {
		margin: 0;
		font-size: 1rem;
	}
	.badge {
		font-family: monospace;
		font-size: 0.85rem;
		color: var(--text-2);
	}
	.dropped-hint {
		font-size: 0.8rem;
		color: var(--accent);
		margin: 0.25rem 0 0.5rem;
	}
	.dropped-hint.subtle {
		color: var(--text-3);
	}
	.entry-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: 0.6rem;
	}
	.entry {
		padding: 0.5rem 0.6rem;
		border-left: 2px solid var(--border);
		background: var(--bg-0, transparent);
	}
	.entry-head {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		gap: 0.5rem;
		margin-bottom: 0.2rem;
	}
	.qname {
		font-size: 0.8rem;
		color: var(--text-2);
	}
	.conf {
		font-size: 0.75rem;
		color: var(--text-3);
		font-family: monospace;
	}
	.entry-summary {
		font-size: 0.9rem;
	}
	.entry-symbols {
		margin-top: 0.25rem;
		display: flex;
		flex-wrap: wrap;
		gap: 0.35rem;
		font-size: 0.75rem;
	}
</style>
