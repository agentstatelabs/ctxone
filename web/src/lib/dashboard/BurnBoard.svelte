<!--
	BurnBoard — the least efficient sessions, ranked by the burn metric.

	Scans on demand rather than on load. The metric needs per-turn tokens and
	tool calls, which only exist inside each session's turn subtree, and that
	subtree carries full transcript text: ~1MB per session, measured. Scanning
	15 of them is ~15MB, and the dashboard auto-refreshes — so doing this
	eagerly would re-download tens of MB on a timer for a five-row panel.

	The result is cached until the user asks for a rescan. The real fix is a
	server-side burn summary on /api/stats/sessions so the client never pulls
	transcripts to compute a ratio; until that exists, on-demand is the honest
	trade.
-->
<script lang="ts">
	import { hubFetch } from '$lib/api';
	import { computeBurn, type BurnResult, type BurnTurn } from '$lib/sessionBurn';
	import type { SessionSnapshot } from '$lib/api';

	let {
		sessions = [],
		branch = 'main'
	}: { sessions?: SessionSnapshot[]; branch?: string } = $props();

	/** Sessions below this many LLM calls are too small to rank meaningfully. */
	const MIN_CALLS = 40;
	/** Hard cap on transcripts pulled per scan — the cost control. */
	const MAX_CANDIDATES = 15;
	const CONCURRENCY = 4;
	const SHOW = 5;

	interface Row {
		id: string;
		name: string;
		turns: number;
		burn: BurnResult;
	}

	let rows = $state<Row[]>([]);
	let scanning = $state(false);
	let scanned = $state(false);
	let done = $state(0);
	let total = $state(0);
	let skipped = $state(0);
	let failed = $state(0);
	let error = $state<string | null>(null);

	const candidates = $derived(
		[...sessions]
			.filter((s) => (s.llm_call_count ?? 0) >= MIN_CALLS)
			.sort((a, b) => (b.llm_call_count ?? 0) - (a.llm_call_count ?? 0))
	);

	function label(s: SessionSnapshot): string {
		const n = (s.name ?? '').trim();
		if (n) return n.length > 58 ? n.slice(0, 57) + '…' : n;
		return s.session_id.slice(0, 8);
	}

	async function fetchTurns(id: string): Promise<BurnTurn[]> {
		const r = await hubFetch(
			`/api/state/${encodeURIComponent(branch)}?path=/sessions/${encodeURIComponent(id)}/turns`
		);
		if (r.status === 404) return []; // predates turn capture
		if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
		const tree = await r.json();
		if (!tree || typeof tree !== 'object') return [];
		return Object.keys(tree)
			.filter((k) => /^t\d+$/.test(k))
			.sort()
			.map((k) => (tree as Record<string, BurnTurn>)[k]);
	}

	async function scan() {
		if (scanning) return;
		scanning = true;
		error = null;
		done = 0;
		skipped = 0;
		failed = 0;
		const pool = candidates.slice(0, MAX_CANDIDATES);
		total = pool.length;
		const out: Row[] = [];

		try {
			let next = 0;
			const worker = async () => {
				for (;;) {
					const i = next++;
					if (i >= pool.length) return;
					const s = pool[i];
					try {
						const turns = await fetchTurns(s.session_id);
						const burn = computeBurn(turns);
						// `unknown` means the metric declined to judge (too short, or
						// no productive baseline). Ranking those would be inventing a
						// number the metric explicitly refused to give.
						if (burn.level === 'unknown' || burn.ratio === null) skipped++;
						else out.push({ id: s.session_id, name: label(s), turns: turns.length, burn });
					} catch {
						failed++;
					} finally {
						done++;
					}
				}
			};
			await Promise.all(Array.from({ length: Math.min(CONCURRENCY, pool.length) }, worker));
			out.sort((a, b) => (b.burn.ratio ?? 0) - (a.burn.ratio ?? 0));
			rows = out.slice(0, SHOW);
			scanned = true;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			scanning = false;
		}
	}
</script>

<div class="burnboard">
	{#if !scanned && !scanning}
		<p class="bb-intro">
			Ranks sessions by context tokens spent per edit landed, against each
			session's own baseline. Reads up to {MAX_CANDIDATES} transcripts, so it runs
			on request rather than on every refresh.
		</p>
		<button class="bb-run" onclick={scan} disabled={candidates.length === 0}>
			{candidates.length ? `Scan ${Math.min(candidates.length, MAX_CANDIDATES)} sessions` : 'No sessions large enough'}
		</button>
	{:else if scanning}
		<p class="bb-progress">Scanning transcripts… {done}/{total}</p>
	{:else}
		{#if rows.length === 0}
			<p class="bb-intro">
				No session scored high enough to rank. {skipped} of {total} had no usable
				baseline — common when a session is short or read-heavy.
			</p>
		{:else}
			<ol class="bb-rows">
				{#each rows as r, i (r.id)}
					<li class="bb-row">
						<span class="bb-rank">{i + 1}</span>
						<a class="bb-name" href={`/sessions?session=${encodeURIComponent(r.id)}`} title={r.name}>
							{r.name}
						</a>
						<span class="bb-ratio bb-{r.burn.level}">{r.burn.ratio?.toFixed(1)}×</span>
						<span class="bb-turns">{r.turns} turns</span>
					</li>
				{/each}
			</ol>
		{/if}
		<div class="bb-foot">
			<span>
				scanned {total}{skipped ? ` · ${skipped} unrankable` : ''}{failed
					? ` · ${failed} failed`
					: ''}
			</span>
			<button class="bb-rescan" onclick={scan} disabled={scanning}>Rescan</button>
		</div>
	{/if}
	{#if error}<p class="bb-error">{error}</p>{/if}
</div>

<style>
	.burnboard {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-3);
	}

	.bb-intro,
	.bb-progress {
		margin: 0;
		font-size: var(--lens-font-size-sm);
		color: var(--lens-text-muted);
	}

	.bb-run,
	.bb-rescan {
		align-self: flex-start;
		background: var(--lens-accent-surface);
		border: 1px solid var(--lens-accent-border, var(--lens-border));
		border-radius: var(--lens-radius-sm);
		color: var(--lens-accent-hover, var(--lens-accent));
		padding: var(--lens-space-2) var(--lens-space-4);
		font-size: var(--lens-font-size-sm);
		font-weight: 600;
		cursor: pointer;
	}
	.bb-rescan {
		padding: 0 var(--lens-space-2);
		font-size: var(--lens-font-size-xs);
		font-weight: 500;
	}
	.bb-run:disabled,
	.bb-rescan:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.bb-rows {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-2);
	}

	.bb-row {
		display: grid;
		grid-template-columns: 1.2rem 1fr auto auto;
		align-items: baseline;
		gap: var(--lens-space-2);
		font-size: var(--lens-font-size-sm);
	}

	.bb-rank {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-muted);
	}

	.bb-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--lens-text);
		text-decoration: none;
	}
	.bb-name:hover {
		color: var(--lens-accent);
		text-decoration: underline;
	}

	.bb-ratio {
		font-family: var(--lens-font-mono);
		font-weight: 600;
	}
	.bb-burning { color: var(--lens-danger, #f87171); }
	.bb-diminishing { color: var(--lens-warn, #fbbf24); }
	.bb-productive { color: var(--lens-ok, #4ade80); }

	.bb-turns {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-muted);
	}

	.bb-foot {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: var(--lens-space-2);
		font-size: var(--lens-font-size-xs);
		font-family: var(--lens-font-mono);
		color: var(--lens-text-muted);
	}

	.bb-error {
		margin: 0;
		font-size: var(--lens-font-size-xs);
		color: var(--lens-danger);
	}
</style>
