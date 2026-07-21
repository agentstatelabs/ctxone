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
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import type { SessionSnapshot } from '$lib/api';

	/**
	 * Scoped to the current workspace AND branch.
	 *
	 * Workspace comes free: `hubFetch` sends `X-CTXone-Namespace`, so both the
	 * session list and every turn lookup are already namespace-scoped.
	 *
	 * Branch is this `branch` prop. Transcripts are written per ref (`ctx
	 * ingest-session --ref`), so a session only has turns on the branch it was
	 * captured under, and "efficiency on this branch" is a real question.
	 *
	 * The subtlety that made the first attempt at this misleading: the
	 * candidate list from /api/stats/sessions is NOT branch-filtered — it is a
	 * namespace-wide registry whose titles are read from `main`. Checking it
	 * against a branch therefore turns most candidates into misses. Those
	 * misses are counted as "not on this branch", NOT as "unrankable"; the
	 * earlier version lumped them together, so 12 sessions with no transcript
	 * on `homesite-ios` looked like 12 sessions the metric had judged and
	 * rejected.
	 */
	let {
		sessions = [],
		branch = 'main'
	}: { sessions?: SessionSnapshot[]; branch?: string } = $props();

	/**
	 * Every session in the workspace gets scanned — no activity threshold and
	 * no candidate cap. Measured against the live hub: 98 sessions, 92 with a
	 * transcript, averaging 0.32MB, so a full scan is ~29MB. That is fine for
	 * an explicit on-demand action (and far cheaper than the per-session
	 * worst case suggested — the big transcripts are outliers, not typical).
	 *
	 * Sessions with no transcript cost a 404 and are counted, not ranked.
	 */
	const CONCURRENCY = 6;
	/** Rows kept after ranking. The scan is exhaustive; the list is not. */
	const SHOW = 15;

	interface Row {
		id: string;
		name: string;
		turns: number;
		/** Ratio and level only — a whole BurnResult carries a per-turn series
		 *  that would bloat the cache for data the row never renders. */
		ratio: number;
		level: BurnResult['level'];
		/** Epoch ms of the first and last turn, or null when unstamped. */
		from: number | null;
		to: number | null;
	}

	interface Cached {
		v: 1;
		scannedAt: number;
		rows: Row[];
		total: number;
		absent: number;
		skipped: number;
		failed: number;
		ranked: number;
	}

	/**
	 * Results persist per workspace+branch, so switching away and back — or
	 * reloading the dashboard — does not force a fresh ~29MB scan. The key
	 * carries both scopes because a cache shown under the wrong branch is the
	 * mislabeling bug this panel has already shipped once.
	 */
	const CACHE_V = 1;
	function cacheKey(ns: string, br: string): string {
		return `ctxone.burnboard.v${CACHE_V}.${ns}.${br}`;
	}

	let rows = $state<Row[]>([]);
	let scanning = $state(false);
	let scanned = $state(false);
	let done = $state(0);
	let total = $state(0);
	let skipped = $state(0);
	let absent = $state(0);
	let failed = $state(0);
	/** How many ranked in total, so a capped list can say what it is hiding. */
	let ranked = $state(0);
	let scannedAt = $state<number | null>(null);
	let error = $state<string | null>(null);

	function reset() {
		rows = [];
		scanned = false;
		skipped = 0;
		absent = 0;
		failed = 0;
		ranked = 0;
		scannedAt = null;
		error = null;
	}

	// Results belong to the workspace+branch they were scanned on, so a switch
	// loads THAT scope's cache rather than leaving the previous one on screen
	// under a new name — the mislabeling bug this panel already shipped once.
	$effect(() => {
		const key = cacheKey(namespaceStore.current, branch);
		reset();
		if (typeof localStorage === 'undefined') return;
		try {
			const raw = localStorage.getItem(key);
			if (!raw) return;
			const c = JSON.parse(raw) as Cached;
			if (c?.v !== CACHE_V || !Array.isArray(c.rows)) return;
			rows = c.rows;
			total = c.total;
			absent = c.absent;
			skipped = c.skipped;
			failed = c.failed;
			ranked = c.ranked;
			scannedAt = c.scannedAt;
			scanned = true;
		} catch {
			// A corrupt or unreadable entry just means "not scanned yet".
		}
	});

	function persist() {
		if (typeof localStorage === 'undefined') return;
		const payload: Cached = {
			v: CACHE_V,
			scannedAt: scannedAt ?? Date.now(),
			rows,
			total,
			absent,
			skipped,
			failed,
			ranked
		};
		try {
			localStorage.setItem(cacheKey(namespaceStore.current, branch), JSON.stringify(payload));
		} catch {
			// Quota or private-mode failure — the scan still stands for this
			// session, it just will not survive a reload.
		}
	}

	// Busiest first, so the sessions most likely to rank resolve early and the
	// progress counter is useful rather than back-loaded.
	const candidates = $derived(
		[...sessions].sort((a, b) => (b.llm_call_count ?? 0) - (a.llm_call_count ?? 0))
	);

	/**
	 * When the transcript actually ran, taken from the turns themselves rather
	 * than the session's `started_at`/`updated_at`. Session meta is written on
	 * `main` and covers the session as a whole; the turns are what this branch
	 * holds, so they are the honest answer to "what period does this ranking
	 * cover". Falls back to session meta when turns carry no timestamps.
	 */
	function turnSpan(
		turns: BurnTurn[],
		s: SessionSnapshot
	): { from: number | null; to: number | null } {
		let lo = Infinity;
		let hi = -Infinity;
		for (const t of turns) {
			const raw = (t as { timestamp?: string }).timestamp;
			if (!raw) continue;
			const ms = Date.parse(raw);
			if (Number.isNaN(ms)) continue;
			if (ms < lo) lo = ms;
			if (ms > hi) hi = ms;
		}
		if (lo !== Infinity) return { from: lo, to: hi };
		const a = s.started_at ? Date.parse(s.started_at) : NaN;
		const b = s.updated_at ? Date.parse(s.updated_at) : NaN;
		return { from: Number.isNaN(a) ? null : a, to: Number.isNaN(b) ? null : b };
	}

	const DAY = 86_400_000;

	/** "Jun 1", or "May 28 – Jun 1" when it spans days. */
	function rangeLabel(from: number | null, to: number | null): string {
		if (from === null) return '—';
		const d = (ms: number) =>
			new Date(ms).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
		const start = d(from);
		if (to === null) return start;
		const end = d(to);
		return start === end ? start : `${start} – ${end}`;
	}

	/** Full timestamps plus elapsed, for the hover title. */
	function rangeTitle(from: number | null, to: number | null): string {
		if (from === null) return 'No timestamps on these turns';
		const f = new Date(from).toLocaleString();
		if (to === null) return f;
		const ms = to - from;
		const span =
			ms >= DAY
				? `${(ms / DAY).toFixed(1)}d`
				: ms >= 3_600_000
					? `${(ms / 3_600_000).toFixed(1)}h`
					: `${Math.max(1, Math.round(ms / 60_000))}m`;
		return `${f} → ${new Date(to).toLocaleString()} (${span})`;
	}

	function agoLabel(ms: number): string {
		const s = Math.max(0, Date.now() - ms) / 1000;
		if (s < 90) return 'just now';
		if (s < 5400) return `${Math.round(s / 60)}m ago`;
		if (s < DAY / 1000) return `${Math.round(s / 3600)}h ago`;
		return `${Math.round(s / 86400)}d ago`;
	}

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

	interface StoredBurn {
		level: string;
		ratio?: number;
		turns?: number;
		updated_at?: string;
	}

	/**
	 * The burn score the CLI stored at ingest, if it is still current.
	 *
	 * Current means its `updated_at` matches the session's — a re-ingest that
	 * added turns bumps both, so a stale stored score is never trusted. When it
	 * matches, this is the whole win: one small JSON read instead of pulling a
	 * ~1MB transcript and re-scanning it. Returns null to signal "fall back to
	 * the transcript scan" for sessions ingested before this existed.
	 */
	async function fetchStoredBurn(s: SessionSnapshot): Promise<StoredBurn | null> {
		try {
			const r = await hubFetch(
				`/api/state/${encodeURIComponent(branch)}?path=/sessions/${encodeURIComponent(
					s.session_id
				)}/burn`
			);
			if (!r.ok) return null;
			const b = (await r.json()) as StoredBurn;
			// Stale or shape-less → recompute.
			if (!b?.level) return null;
			if (b.updated_at && s.updated_at && b.updated_at !== s.updated_at) return null;
			return b;
		} catch {
			return null;
		}
	}

	async function scan() {
		if (scanning) return;
		scanning = true;
		error = null;
		done = 0;
		skipped = 0;
		absent = 0;
		failed = 0;
		const pool = candidates;
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
						// Fast path: a current stored score avoids the transcript
						// fetch entirely — the ingest-persistence payoff. Most
						// sessions never change, so most rows cost one small read
						// instead of a ~1MB scan.
						const stored = await fetchStoredBurn(s);
						if (stored) {
							if (stored.level === 'unknown' || typeof stored.ratio !== 'number') {
								skipped++;
							} else {
								out.push({
									id: s.session_id,
									name: label(s),
									turns: stored.turns ?? 0,
									ratio: stored.ratio,
									level: stored.level as Row['level'],
									// The stored score carries no per-turn timestamps; show
									// the session's meta span for the range column instead.
									from: s.started_at ? Date.parse(s.started_at) : null,
									to: s.updated_at ? Date.parse(s.updated_at) : null
								});
							}
							continue;
						}

						// Fallback: no current stored score, scan the transcript.
						const turns = await fetchTurns(s.session_id);
						if (turns.length === 0) {
							// No transcript on THIS branch — the session exists in the
							// workspace but was captured elsewhere. Not a judgement.
							absent++;
							continue;
						}
						const burn = computeBurn(turns);
						// `unknown` means the metric declined to judge (too short, or
						// no productive baseline). Ranking those would be inventing a
						// number the metric explicitly refused to give.
						if (burn.level === 'unknown' || burn.ratio === null) skipped++;
						else {
							const span = turnSpan(turns, s);
							out.push({
								id: s.session_id,
								name: label(s),
								turns: turns.length,
								ratio: burn.ratio,
								level: burn.level,
								from: span.from,
								to: span.to
							});
						}
					} catch {
						failed++;
					} finally {
						done++;
					}
				}
			};
			await Promise.all(Array.from({ length: Math.min(CONCURRENCY, pool.length) }, worker));
			// Worst first, top SHOW kept. The scan covers every session; the list
			// is capped because past ~15 rows this stops being a dashboard panel.
			out.sort((a, b) => b.ratio - a.ratio);
			ranked = out.length;
			rows = out.slice(0, SHOW);
			scannedAt = Date.now();
			scanned = true;
			persist();
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
			Ranks this workspace's sessions on <code>{branch}</code> by context tokens
			spent per edit landed, against each session's own baseline. Reads every
			transcript on the branch, so it runs on request rather than on every
			refresh.
		</p>
		<button class="bb-run" onclick={scan} disabled={candidates.length === 0}>
			{candidates.length ? `Scan all ${candidates.length} sessions` : 'No sessions yet'}
		</button>
	{:else if scanning}
		<p class="bb-progress">Scanning transcripts… {done}/{total}</p>
	{:else}
		{#if rows.length === 0}
			<p class="bb-intro">
				{#if absent === total}
					None of the {total} sessions scanned have a transcript on
					<code>{branch}</code>. Transcripts are captured per branch, so a session
					only appears on the branch it ran under.
				{:else}
					Nothing rankable on <code>{branch}</code>. {skipped} of {total} had no
					usable baseline (short or read-heavy){absent
						? `, and ${absent} have no transcript on this branch`
						: ''}.
				{/if}
			</p>
		{:else}
			<ol class="bb-rows">
				{#each rows as r, i (r.id)}
					<li class="bb-row">
						<span class="bb-rank">{i + 1}</span>
						<a class="bb-name" href={`/sessions?session=${encodeURIComponent(r.id)}`} title={r.name}>
							{r.name}
						</a>
						<span class="bb-range" title={rangeTitle(r.from, r.to)}>
							{rangeLabel(r.from, r.to)}
						</span>
						<span class="bb-ratio bb-{r.level}">{r.ratio.toFixed(1)}×</span>
						<span class="bb-turns">{r.turns} turns</span>
					</li>
				{/each}
			</ol>
		{/if}
		<div class="bb-foot">
			<span>
				{ranked > rows.length ? `top ${rows.length} of ${ranked} ranked · ` : ''}scanned
				{total}{absent ? ` · ${absent} not on ${branch}` : ''}{skipped
					? ` · ${skipped} unrankable`
					: ''}{failed ? ` · ${failed} failed` : ''}{scannedAt
					? ` · ${agoLabel(scannedAt)}`
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
		/* Scrolls instead of truncating, so every rankable session is reachable
		   without the panel dictating the height of its dashboard row. */
		max-height: 15rem;
		overflow-y: auto;
		scrollbar-gutter: stable;
		padding-right: var(--lens-space-1, 4px);
	}

	.bb-row {
		display: grid;
		grid-template-columns: 1.2rem 1fr auto auto auto;
		align-items: baseline;
		gap: var(--lens-space-2);
		font-size: var(--lens-font-size-sm);
	}

	.bb-range {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-muted);
		white-space: nowrap;
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
