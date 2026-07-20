<script lang="ts">
	import {
		getHealth,
		getStats,
		getTokenStats,
		getSessions,
		getLog,
		getActivity,
		getDueReminders
	} from '$lib/api';
	import type {
		StatsResponse,
		TokenStats,
		SessionSnapshot,
		CommitEntry,
		Reminder,
		ActivityResponse
	} from '$lib/api';
	import { listPlans, listPlanTasks, type Plan, type Task } from '$lib/plansApi';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import { branchStore } from '$lib/branchStore.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';
	import {
		StatTile,
		AreaLine,
		BarChart,
		Donut,
		CalendarHeatmap,
		formatCompact,
		trimFloat
	} from '@agentstate/lens-core';
	import type { AreaPoint, SeriesDef, ChartDatum, HeatCell } from '@agentstate/lens-core';
	import Panel from '$lib/dashboard/Panel.svelte';
	import QuickCapture from '$lib/dashboard/QuickCapture.svelte';
	import LlmStats from '$lib/dashboard/LlmStats.svelte';
	import BurnBoard from '$lib/dashboard/BurnBoard.svelte';

	/**
	 * Bridge for StatTile's `spark` snippet prop: lens-core is a symlinked
	 * file: dependency, so its dist .d.ts brands `Snippet` with a different
	 * unique symbol than this app's svelte copy. The runtime is one svelte
	 * instance (vite compiles the dist .svelte sources with the app's
	 * svelte), so the cast is safe — it only reconciles the two nominal
	 * `Snippet` types.
	 */
	function asSpark(s: unknown): number[] | undefined {
		return s as number[] | undefined;
	}

	// ── Per-endpoint load state ──────────────────────────────────────────────
	// Every panel loads independently: one failing endpoint degrades its own
	// panel to an honest error state and the rest of the page stays useful.
	interface Load<T> {
		status: 'loading' | 'error' | 'ready';
		data: T | null;
		error: string;
	}
	function pending<T>(): Load<T> {
		return { status: 'loading', data: null, error: '' };
	}

	interface PlansData {
		plans: Plan[];
		/** Tasks for active plans (best-effort; used for blocked counts). */
		tasksByPlan: Record<string, Task[]>;
	}

	let connected = $state(true);
	let tokensL = $state<Load<TokenStats>>(pending());
	let sessionsL = $state<Load<SessionSnapshot[]>>(pending());
	let statsL = $state<Load<StatsResponse>>(pending());
	let logL = $state<Load<CommitEntry[]>>(pending());
	let plansL = $state<Load<PlansData>>(pending());
	let remindersL = $state<Load<Reminder[]>>(pending());
	let activityL = $state<Load<ActivityResponse>>(pending());

	function friendly(e: unknown): string {
		const msg = e instanceof Error ? e.message : String(e);
		if (msg.includes('404')) return 'Not available on this Hub version.';
		return msg;
	}

	async function track<T>(fn: () => Promise<T>, assign: (l: Load<T>) => void): Promise<void> {
		try {
			assign({ status: 'ready', data: await fn(), error: '' });
		} catch (e) {
			assign({ status: 'error', data: null, error: friendly(e) });
		}
	}

	async function loadPlans(branch: string): Promise<PlansData> {
		const plans = await listPlans(branch);
		const tasksByPlan: Record<string, Task[]> = {};
		const active = plans.filter((p) => p.status === 'active').slice(0, 12);
		await Promise.all(
			active.map(async (p) => {
				try {
					tasksByPlan[p.name] = await listPlanTasks(p.name, branch);
				} catch {
					// Blocked counts degrade to unknown for this plan.
				}
			})
		);
		return { plans, tasksByPlan };
	}

	async function refreshAll(initial: boolean) {
		const branch = branchStore.current;
		if (initial) {
			tokensL = pending();
			sessionsL = pending();
			statsL = pending();
			logL = pending();
			plansL = pending();
			remindersL = pending();
			activityL = pending();
		}
		connected = await getHealth();
		await Promise.all([
			track(() => getTokenStats(), (l) => (tokensL = l)),
			track(() => getSessions(), (l) => (sessionsL = l)),
			track(() => getStats(branch), (l) => (statsL = l)),
			track(() => getLog(branch, 1000), (l) => (logL = l)),
			track(() => loadPlans(branch), (l) => (plansL = l)),
			track(() => getDueReminders(), (l) => (remindersL = l)),
			track(() => getActivity(branch, 120), (l) => (activityL = l))
		]);
	}

	// Load on mount and re-load whenever the workspace or branch changes.
	$effect(() => {
		void namespaceStore.current;
		void branchStore.current;
		void refreshAll(true);
	});

	const auto = useAutoRefresh(() => refreshAll(false));

	// ── Derivations ──────────────────────────────────────────────────────────
	const sessionList = $derived(sessionsL.data ?? []);
	const usedSeries = $derived(sessionList.map((s) => s.session_tokens_used));
	const savedSeries = $derived(sessionList.map((s) => s.session_tokens_saved));

	const activePlans = $derived(
		(plansL.data?.plans ?? []).filter((p) => p.status === 'active')
	);

	const blockedByPlan = $derived.by(() => {
		const out: Record<string, number> = {};
		const byPlan = plansL.data?.tasksByPlan ?? {};
		for (const [name, tasks] of Object.entries(byPlan)) {
			out[name] = tasks.filter(
				(t) => (t.status === 'pending' || t.status === 'in_progress') && t.blocked_by.length > 0
			).length;
		}
		return out;
	});
	const blockedTotal = $derived(Object.values(blockedByPlan).reduce((s, n) => s + n, 0));

	const dueReminders = $derived(remindersL.data ?? []);
	const overdueCount = $derived(
		dueReminders.filter((r) => Date.parse(r.due_at) < Date.now()).length
	);

	// Token economics: one x-slice per session, used vs saved.
	const econPoints = $derived<AreaPoint[]>(
		sessionList.map((s) => ({
			t: s.session_id,
			used: s.session_tokens_used,
			saved: s.session_tokens_saved
		}))
	);
	const econSeries: SeriesDef[] = [
		{ key: 'used', label: 'Tokens used', color: 'var(--lens-accent, #6ea8ff)' },
		{ key: 'saved', label: 'Tokens saved', color: 'var(--lens-ok, #4ade80)' }
	];
	const hasEconTraffic = $derived(
		sessionList.some((s) => s.session_tokens_used + s.session_tokens_saved > 0)
	);
	function shortId(t: number | string): string {
		const s = String(t);
		return s.length > 12 ? s.slice(0, 11) + '…' : s;
	}

	// ── Token usage over time ────────────────────────────────────────────
	//
	// Built from the sessions list rather than a new endpoint: each session
	// carries started_at plus its LLM totals, which is enough for a
	// per-period view. The cost is granularity — a session lands entirely in
	// the bucket it *started* in, so a long session is not spread across the
	// days it actually ran. Per-turn precision would need one request per
	// session; not worth it for a dashboard overview.

	/** Sessions with both a timestamp and reported usage. */
	const timedSessions = $derived(
		sessionList.filter(
			(s) => s.started_at && (s.llm_input_tokens ?? 0) + (s.llm_output_tokens ?? 0) > 0
		)
	);

	/** Sessions excluded for want of a timestamp — surfaced, never silent. */
	const untimedCount = $derived(
		sessionList.filter(
			(s) => !s.started_at && (s.llm_input_tokens ?? 0) + (s.llm_output_tokens ?? 0) > 0
		).length
	);

	const DAY_MS = 86_400_000;

	/** Day buckets while the span is short; weeks once a daily axis would be
	 * unreadable. Returns the bucket key (an epoch ms) for a timestamp. */
	const bucketMs = $derived.by(() => {
		const times = timedSessions.map((s) => Date.parse(s.started_at!)).filter((n) => !isNaN(n));
		if (times.length === 0) return DAY_MS;
		const span = Math.max(...times) - Math.min(...times);
		return span > 60 * DAY_MS ? 7 * DAY_MS : DAY_MS;
	});
	function bucketOf(iso: string): number {
		return Math.floor(Date.parse(iso) / bucketMs) * bucketMs;
	}

	/** Total tokens per bucket — one series, so no legend is needed. */
	const usageOverTime = $derived.by((): AreaPoint[] => {
		const acc = new Map<number, number>();
		for (const s of timedSessions) {
			const b = bucketOf(s.started_at!);
			if (isNaN(b)) continue;
			acc.set(b, (acc.get(b) ?? 0) + (s.llm_input_tokens ?? 0) + (s.llm_output_tokens ?? 0));
		}
		return [...acc.entries()].sort((a, b) => a[0] - b[0]).map(([t, tokens]) => ({ t, tokens }));
	});
	const usageSeries: SeriesDef[] = [
		{ key: 'tokens', label: 'Tokens', color: 'var(--lens-accent, #6ea8ff)' }
	];

	/** Categorical hues for the by-model chart.
	 *
	 * Deliberately NOT lens-core's SERIES_COLORS: its first two slots are
	 * --lens-accent and --lens-info, and inside `.app` this theme aliases
	 * both onto --accent/--info, which are the same #93c5fd. Two series
	 * rendered identically before this was pinned down. These four resolve
	 * distinctly under the app theme and clear CVD separation comfortably
	 * (worst adjacent ΔE 33.2 protan / 22.5 tritan against the dark surface).
	 */
	const SERIES_HUES = [
		'var(--accent, #93c5fd)',
		'var(--success, #4ade80)',
		'var(--lens-kind-module, #b48ead)',
		'var(--lens-kind-class, #d08770)'
	];

	/** Models charted individually — the rest fold into one "Other" band.
	 * A model is attributed by last_model; only 5-in-87 sessions here touch
	 * more than one, and splitting a session's single total across several
	 * models would invent data rather than measure it. */
	const CHARTED_MODELS = 4;
	const modelSplit = $derived.by(() => {
		const totals = new Map<string, number>();
		for (const s of timedSessions) {
			const m = s.last_model ?? 'unknown';
			totals.set(m, (totals.get(m) ?? 0) + (s.llm_input_tokens ?? 0) + (s.llm_output_tokens ?? 0));
		}
		const ranked = [...totals.entries()].sort((a, b) => b[1] - a[1]);
		return {
			charted: ranked.slice(0, CHARTED_MODELS).map(([m]) => m),
			otherCount: Math.max(0, ranked.length - CHARTED_MODELS)
		};
	});

	const usageByModel = $derived.by((): AreaPoint[] => {
		const charted = new Set(modelSplit.charted);
		const rows = new Map<number, Record<string, number>>();
		for (const s of timedSessions) {
			const b = bucketOf(s.started_at!);
			if (isNaN(b)) continue;
			const tok = (s.llm_input_tokens ?? 0) + (s.llm_output_tokens ?? 0);
			const m = s.last_model ?? 'unknown';
			const key = charted.has(m) ? m : 'Other';
			const row = rows.get(b) ?? {};
			row[key] = (row[key] ?? 0) + tok;
			rows.set(b, row);
		}
		return [...rows.entries()]
			.sort((a, b) => a[0] - b[0])
			.map(([t, row]) => ({ t, ...row }) as AreaPoint);
	});

	/** Hue per charted model.
	 *
	 * Assigned over the charted set only, never over every model seen: with
	 * more models than hues, indexing into the full list wraps and two
	 * charted series silently land on the same colour (gpt-5.6-sol and
	 * gpt-5.2-codex both did). Sorted by name so the mapping is
	 * deterministic rather than following token rank, which would repaint
	 * every series whenever one model overtook another.
	 *
	 * The honest limit: with 11 models and 5 hues, a colour cannot be
	 * permanently bound to a model. Stability holds while the charted set
	 * does, and the legend carries identity regardless.
	 */
	const modelColors = $derived.by((): Map<string, string> => {
		const named = [...modelSplit.charted].sort();
		return new Map(named.map((m, i) => [m, SERIES_HUES[i]]));
	});

	const modelSeries = $derived.by((): SeriesDef[] => {
		const out: SeriesDef[] = modelSplit.charted.map((m) => ({
			key: m,
			label: m,
			color: modelColors.get(m) ?? 'var(--lens-muted, #667089)'
		}));
		if (modelSplit.otherCount > 0) {
			out.push({
				key: 'Other',
				label: `Other (${modelSplit.otherCount})`,
				color: 'var(--lens-muted, #667089)'
			});
		}
		return out;
	});

	/** Axis label: numeric "4/15", with a 2-digit year only when it is not
	 * the current one ("12/3/25").
	 *
	 * Deliberately terse. These panels are ~320px wide and the chart draws a
	 * tick every few buckets, so "Apr 15" / "May 13" collided into an
	 * unreadable run at week granularity. Numeric survives the width. */
	function formatBucket(t: number | string): string {
		const d = new Date(Number(t));
		if (isNaN(d.getTime())) return String(t);
		const md = `${d.getMonth() + 1}/${d.getDate()}`;
		return d.getFullYear() === new Date().getFullYear()
			? md
			: `${md}/${String(d.getFullYear()).slice(2)}`;
	}

	// Per-model LLM token totals across sessions (only when agents reported).
	const modelBars = $derived.by((): ChartDatum[] => {
		const acc = new Map<string, number>();
		for (const s of sessionList) {
			const tok = (s.llm_input_tokens ?? 0) + (s.llm_output_tokens ?? 0);
			if (tok <= 0) continue;
			const label = s.last_model ?? s.last_provider ?? 'unknown';
			acc.set(label, (acc.get(label) ?? 0) + tok);
		}
		return [...acc.entries()]
			.map(([label, value]) => ({ label, value }))
			.sort((a, b) => b.value - a.value)
			.slice(0, 6);
	});

	// Plan health: task-status distribution across active plans.
	const taskDonut = $derived.by((): ChartDatum[] => {
		let pendingN = 0;
		let inProgress = 0;
		let done = 0;
		let abandoned = 0;
		for (const p of activePlans) {
			pendingN += p.task_counts.pending;
			inProgress += p.task_counts.in_progress;
			done += p.task_counts.done;
			abandoned += p.task_counts.abandoned;
		}
		const out: ChartDatum[] = [];
		if (inProgress > 0)
			out.push({ label: 'in progress', value: inProgress, color: 'var(--lens-accent, #6ea8ff)' });
		if (pendingN > 0)
			out.push({ label: 'pending', value: pendingN, color: 'var(--lens-warn, #ebcb8b)' });
		if (done > 0) out.push({ label: 'done', value: done, color: 'var(--lens-ok, #4ade80)' });
		if (abandoned > 0)
			out.push({ label: 'abandoned', value: abandoned, color: 'var(--lens-muted, #8a93a5)' });
		return out;
	});

	// Top active plans by open (pending + in-progress) tasks; blocked → warn.
	const planBars = $derived.by((): ChartDatum[] =>
		activePlans
			.map((p) => ({ p, open: p.task_counts.pending + p.task_counts.in_progress }))
			.filter((x) => x.open > 0)
			.sort((a, b) => b.open - a.open)
			.slice(0, 6)
			.map(({ p, open }) => ({
				label: p.name,
				value: open,
				color:
					(blockedByPlan[p.name] ?? 0) > 0
						? 'var(--lens-warn, #ebcb8b)'
						: 'var(--lens-accent, #6ea8ff)'
			}))
	);
	const blockedNotes = $derived(
		activePlans
			.map((p) => ({ name: p.name, blocked: blockedByPlan[p.name] ?? 0 }))
			.filter((x) => x.blocked > 0)
	);

	// Activity: commits bucketed per UTC day (timestamps are RFC 3339 Z).
	// Per-day counts come from the server. Counting getLog(ref, 1000)
	// client-side charted a commit-count window, not a time window: the
	// busier the machine, the less history appeared — 1000 commits covered
	// 80 minutes mid-import, which read as "no activity since April".
	const heatCells = $derived.by((): HeatCell[] =>
		(activityL.data?.days ?? []).map((d) => ({ date: d.date, count: d.count }))
	);
	/** Set when the server's walk was capped before reaching the cutoff. */
	const activityTruncated = $derived(activityL.data?.truncated === true);
	const recentCommits = $derived((logL.data ?? []).slice(0, 8));

	// Panel status helpers.
	function statusOf<T>(l: Load<T>, isEmpty: (d: T) => boolean): 'loading' | 'error' | 'empty' | 'ready' {
		if (l.status === 'loading') return 'loading';
		if (l.status === 'error') return 'error';
		return l.data !== null && !isEmpty(l.data) ? 'ready' : 'empty';
	}
	const econStatus = $derived(statusOf(sessionsL, () => !hasEconTraffic));
	const planStatus = $derived(statusOf(plansL, () => activePlans.length === 0));
	// Follows the activity load: the heatmap is the panel's primary content
	// now, so an activity failure must surface rather than being masked by a
	// healthy log fetch.
	const activityStatus = $derived(
		statusOf(activityL, (d) => d.days.length === 0 && (logL.data ?? []).length === 0)
	);
</script>

<header class="dash-head">
	<h2>Dashboard</h2>
	<div class="head-meta">
		<span class="hub" class:down={!connected}>
			<span class="hub-dot"></span>
			{connected ? 'Hub connected' : 'Hub unreachable'}
		</span>
		<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
	</div>
</header>

<!-- ── 1 · Headline stat row ─────────────────────────────────────────────── -->
{#snippet plansSub()}
	<span class="tile-sub" class:warn={blockedTotal > 0}>
		{plansL.status === 'ready' ? `${blockedTotal} blocked` : '…'}
	</span>
{/snippet}
{#snippet remindersSub()}
	<span class="tile-sub" class:danger={overdueCount > 0}>
		{remindersL.status === 'ready' ? `${overdueCount} overdue` : '…'}
	</span>
{/snippet}
{#snippet memorySub()}
	{#if statsL.data}
		<span class="tile-sub">
			{statsL.data.commit_count} commits · {statsL.data.branch_count} branches · {statsL.data
				.epoch_count} epochs
		</span>
	{/if}
{/snippet}

<div class="stat-row">
	<StatTile
		label="Tokens saved"
		value={tokensL.data ? formatCompact(tokensL.data.session_tokens_saved) : '—'}
		unit={tokensL.data ? `tok · ${trimFloat(tokensL.data.cumulative_ratio, 1)}× ratio` : undefined}
		spark={savedSeries.length > 1 ? savedSeries : undefined}
		sparkColor="var(--lens-ok, #4ade80)"
		accent
		title="Cumulative tokens saved vs flat memory, with the savings ratio"
	/>
	<StatTile
		label="Session tokens used"
		value={tokensL.data ? formatCompact(tokensL.data.session_tokens_used) : '—'}
		unit={tokensL.data ? 'tok' : undefined}
		spark={usedSeries.length > 1 ? usedSeries : undefined}
		title="Tokens actually sent across all sessions"
	/>
	<StatTile
		label="Plans in flight"
		value={plansL.status === 'ready' ? activePlans.length : '—'}
		spark={asSpark(plansSub)}
		title="Active plans on {branchStore.current}"
	/>
	<StatTile
		label="Reminders due"
		value={remindersL.status === 'ready' ? dueReminders.length : '—'}
		spark={asSpark(remindersSub)}
		title="Actionable reminders (due or awaiting permission)"
	/>
	<StatTile
		label="Memory size"
		value={statsL.data ? statsL.data.path_count : '—'}
		unit={statsL.data ? 'paths' : undefined}
		spark={asSpark(memorySub)}
		title="Structural size of the memory graph on {branchStore.current}"
	/>
</div>

<div class="grid">
	<!-- ── 2 · Token economics ──────────────────────────────────────────── -->
	<Panel
		title="Token economics"
		scope="all branches"
		links={[{ href: '/sessions', label: 'Sessions' }]}
		status={econStatus}
		errorText={sessionsL.error}
		emptyTitle="No token traffic yet"
		emptyText="Run an agent recall or context call to start measuring savings."
	>
		{#if tokensL.data}
			<div class="econ-strip">
				<span class="econ-ratio">{trimFloat(tokensL.data.cumulative_ratio, 1)}×</span>
				<span class="econ-ratio-label">savings vs flat memory</span>
				<span class="econ-fig">
					graph size (flat equivalent)
					<strong>{tokensL.data.total_graph_size_tokens.toLocaleString()} tok</strong>
				</span>
			</div>
		{/if}
		<AreaLine
			data={econPoints}
			series={econSeries}
			area
			height={240}
			formatX={shortId}
			ariaLabel="Tokens used vs saved per session"
		/>
		<div class="econ-side">
			{#if modelBars.length > 0}
				<div class="econ-models">
					<h4>LLM tokens by model</h4>
					<BarChart data={modelBars} orientation="horizontal" labelWidth={150} />
				</div>
			{/if}
			{#if tokensL.data}
				<LlmStats snapshot={tokensL.data} />
			{/if}
		</div>
	</Panel>

	<!-- ── 4b · Token usage over time ───────────────────────────────────── -->
	<Panel
		title="Token usage over time"
		scope="all branches"
		links={[{ href: '/sessions', label: 'Sessions' }]}
		status={econStatus}
		errorText={sessionsL.error}
		emptyTitle="No dated usage yet"
		emptyText="Sessions need a start time and reported LLM usage to chart."
	>
		{#if usageOverTime.length > 0}
			<div class="usage-block">
				<h4>All models</h4>
				<AreaLine
					data={usageOverTime}
					series={usageSeries}
					area
					height={200}
					formatX={formatBucket}
					ariaLabel="Total LLM tokens per {bucketMs === 86_400_000 ? 'day' : 'week'}"
				/>
			</div>

			<div class="usage-block">
				<h4>By model</h4>
				<!-- Legend, not colour alone: the palette's worst-case tritan
				     separation sits in the floor band, so identity has to be
				     carried by a label as well as a hue. -->
				<ul class="usage-legend">
					{#each modelSeries as s}
						<li><span class="swatch" style="background: {s.color}"></span>{s.label}</li>
					{/each}
				</ul>
				<AreaLine
					data={usageByModel}
					series={modelSeries}
					area
					stacked
					height={220}
					formatX={formatBucket}
					ariaLabel="LLM tokens per {bucketMs === 86_400_000
						? 'day'
						: 'week'}, split by model"
				/>
			</div>

			<p class="usage-note">
				Bucketed by {bucketMs === 86_400_000 ? 'day' : 'week'}; each session counts in the period
				it started. Attributed by last model used.
				{#if untimedCount > 0}
					{untimedCount} session{untimedCount === 1 ? '' : 's'} with usage but no start time
					{untimedCount === 1 ? 'is' : 'are'} excluded.
				{/if}
			</p>
		{/if}
	</Panel>

	<!-- ── 4 · Activity ─────────────────────────────────────────────────── -->
	<Panel
		title="Activity"
		scope={branchStore.current}
		links={[
			{ href: '/history', label: 'History' },
			{ href: '/tail', label: 'Live Tail' }
		]}
		status={activityStatus}
		errorText={activityL.error || logL.error}
		emptyTitle="No commits yet"
		emptyText="Memory writes on {branchStore.current} will show up here."
	>
		<CalendarHeatmap data={heatCells} ariaLabel="Memory commits per day" />
		{#if activityTruncated}
			<!-- A capped walk must never read as a quiet period. -->
			<p class="activity-note">
				History is partial — the scan reached its limit before covering the full window.
			</p>
		{/if}
		<ul class="commits">
			{#each recentCommits as commit (commit.id)}
				<li class="commit">
					<span class="commit-time">{commit.timestamp.slice(0, 19).replace('T', ' ')}</span>
					<span class="commit-category">[{commit.intent.category}]</span>
					<span class="commit-desc">{commit.intent.description}</span>
				</li>
			{/each}
		</ul>
	</Panel>

	<!-- ── 3 · Plan health ──────────────────────────────────────────────── -->
	<Panel
		title="Plan health"
		scope={branchStore.current}
		links={[{ href: '/plans', label: 'Plans' }]}
		status={planStatus}
		errorText={plansL.error}
		emptyTitle="No active plans"
		emptyText="Nothing in flight on {branchStore.current}."
	>
		<div class="plan-body">
			{#if taskDonut.length > 0}
				<Donut data={taskDonut} size={150} centerLabel="tasks" />
			{:else}
				<p class="plan-note">Active plans have no tasks yet.</p>
			{/if}
			{#if planBars.length > 0}
				<div class="plan-bars">
					<h4>Open tasks by plan</h4>
					<BarChart data={planBars} orientation="horizontal" labelWidth={140} />
					{#if blockedNotes.length > 0}
						<p class="plan-blocked">
							blocked: {blockedNotes.map((b) => `${b.name} (${b.blocked})`).join(', ')}
						</p>
					{/if}
				</div>
			{/if}
		</div>
	</Panel>

	<!-- ── 6 · Least efficient sessions ─────────────────────────────────── -->
	<!--
		Genuinely branch-scoped, unlike the token/session panels above:
		transcripts are captured per ref, so the ranking is per branch. The
		workspace half is implicit — hubFetch sends X-CTXone-Namespace.
	-->
	<Panel
		title="Least efficient sessions"
		scope={branchStore.current}
		links={[{ href: '/sessions', label: 'Sessions' }]}
		status={sessionsL.status}
		errorText={sessionsL.error}
		emptyTitle="No sessions"
		emptyText="No sessions recorded in this workspace yet."
	>
		<BurnBoard sessions={sessionsL.data ?? []} branch={branchStore.current} />
	</Panel>

	<!-- ── 5 · Quick capture ────────────────────────────────────────────── -->
	<!--
		The grid stretches row-mates itself now (`align-items: stretch`), so
		this wrapper no longer exists to equalise height. It survives only to
		hand the reclaimed space to the textarea — a taller capture box is
		useful, an empty one is not.
	-->
	<div class="fill-row">
		<Panel title="Remember a fact" scope={branchStore.current}>
			<QuickCapture {connected} onSaved={() => refreshAll(false)} />
		</Panel>
	</div>
</div>

<style>
	.dash-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--lens-space-4);
		margin-bottom: var(--lens-space-5);
	}

	.dash-head h2 {
		margin: 0;
		font-size: var(--lens-font-size-lg);
		font-weight: 700;
		color: var(--lens-text-strong);
	}

	.head-meta {
		display: flex;
		align-items: center;
		gap: var(--lens-space-4);
	}

	.hub {
		display: inline-flex;
		align-items: center;
		gap: var(--lens-space-2);
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-secondary);
	}

	.hub-dot {
		width: 8px;
		height: 8px;
		border-radius: var(--lens-radius-full);
		background: var(--lens-ok);
	}

	.hub.down {
		color: var(--lens-danger);
	}

	.hub.down .hub-dot {
		background: var(--lens-danger);
	}

	.ago {
		font-size: var(--lens-font-size-2xs);
		font-family: var(--lens-font-mono);
		color: var(--lens-muted);
	}

	/* ── Headline stat row ─────────────────────────────────────────────── */
	.stat-row {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
		gap: var(--lens-space-3);
		margin-bottom: var(--lens-space-4);
	}

	.tile-sub {
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
	}

	.tile-sub.warn {
		color: var(--lens-warn, #ebcb8b);
	}

	.tile-sub.danger {
		color: var(--lens-danger);
	}

	/* ── Panel grid ────────────────────────────────────────────────────── */
	.grid {
		display: grid;
		grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
		gap: var(--lens-space-4);
		/*
			`stretch` (the default), not `start`: panels sharing a row match
			their row's height, so the two columns line up instead of each
			panel ending wherever its content happens to stop. This is also
			what `.fill-row` was hand-rolling for the capture box — that
			wrapper now only handles growing the textarea into the space.
		*/
		align-items: stretch;
	}

	/*
		No `height: 100%` on grid children. A percentage height resolves
		against the grid area, which for an auto-sized row is indefinite, so it
		falls back to content height — and worse, giving an item a non-`auto`
		height DISABLES `align-self: stretch`, defeating the one mechanism that
		does work here. `align-items: stretch` above equalises the row on its
		own, provided the children keep `height: auto`.
	*/

	/*
		Stretch this grid item to its row height and let the chain of
		containers pass that height down to the textarea. Scoped here rather
		than changed in Panel/QuickCapture, which every other panel shares and
		which should stay intrinsically sized.
	*/
	.fill-row {
		align-self: stretch;
		display: flex;
		min-width: 0;
	}
	.fill-row :global(.panel) {
		flex: 1;
		display: flex;
		flex-direction: column;
	}
	.fill-row :global(.capture) {
		flex: 1;
	}
	.fill-row :global(.capture textarea) {
		/* Grows into the reclaimed space; the floor keeps it usable when the
		   neighbouring panel is short. */
		flex: 1;
		min-height: 5rem;
	}

	@media (max-width: 1100px) {
		.grid {
			grid-template-columns: minmax(0, 1fr);
		}
	}

	/* ── Token economics ───────────────────────────────────────────────── */
	.econ-strip {
		display: flex;
		align-items: baseline;
		gap: var(--lens-space-3);
		flex-wrap: wrap;
		margin-bottom: var(--lens-space-3);
	}

	.econ-ratio {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xl);
		font-weight: 700;
		color: var(--lens-accent);
	}

	.econ-ratio-label {
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-secondary);
	}

	.econ-fig {
		margin-left: auto;
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
	}

	.econ-fig strong {
		font-family: var(--lens-font-mono);
		color: var(--lens-text-secondary);
		font-weight: 600;
	}

	.econ-side {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
		gap: var(--lens-space-5);
		margin-top: var(--lens-space-4);
		padding-top: var(--lens-space-4);
		border-top: 1px solid var(--lens-border-subtle, var(--lens-border));
	}

	.econ-models h4,
	.plan-bars h4 {
		margin: 0 0 var(--lens-space-2);
		font-size: var(--lens-font-size-2xs);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}

	/* ── Plan health ───────────────────────────────────────────────────── */
	.plan-body {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-5);
	}

	.plan-note {
		margin: 0;
		font-size: var(--lens-font-size-xs);
		color: var(--lens-muted);
	}

	.plan-blocked {
		margin: var(--lens-space-2) 0 0;
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-warn, #ebcb8b);
		overflow-wrap: anywhere;
	}

	/* ── Activity ──────────────────────────────────────────────────────── */
	.commits {
		list-style: none;
		margin: var(--lens-space-4) 0 0;
		padding: 0;
		border-top: 1px solid var(--lens-border-subtle, var(--lens-border));
	}

	.commit {
		display: flex;
		align-items: baseline;
		gap: var(--lens-space-2);
		padding: var(--lens-space-2) 0;
		border-bottom: 1px solid var(--lens-border-subtle, var(--lens-border));
		font-size: var(--lens-font-size-xs);
		min-width: 0;
	}

	.commit:last-child {
		border-bottom: none;
	}

	.commit-time {
		color: var(--lens-muted);
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		flex-shrink: 0;
	}

	.commit-category {
		color: var(--lens-accent);
		flex-shrink: 0;
	}

	.commit-desc {
		color: var(--lens-text);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.usage-block + .usage-block {
		margin-top: var(--lens-space-4, 1rem);
	}
	.usage-block h4 {
		margin: 0 0 var(--lens-space-2, 0.5rem);
		font-size: var(--lens-font-size-2xs, 0.7rem);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps, 0.06em);
		color: var(--lens-muted, #667089);
	}
	.usage-legend {
		list-style: none;
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 0.85rem;
		margin: 0 0 var(--lens-space-2, 0.5rem);
		padding: 0;
		font-size: var(--lens-font-size-2xs, 0.7rem);
		/* Text keeps text ink; the swatch beside it carries identity. */
		color: var(--lens-text-secondary, var(--text-2));
	}
	.usage-legend li {
		display: flex;
		align-items: center;
		gap: 0.35rem;
	}
	.usage-legend .swatch {
		width: 10px;
		height: 10px;
		border-radius: 2px;
		flex: none;
	}
	.usage-note {
		margin: var(--lens-space-2, 0.5rem) 0 0;
		font-size: var(--lens-font-size-2xs, 0.7rem);
		color: var(--lens-muted, #667089);
	}

	.activity-note {
		margin: var(--lens-space-2, 0.5rem) 0 0;
		font-size: var(--lens-font-size-2xs, 0.7rem);
		color: var(--lens-muted, #667089);
	}
</style>
