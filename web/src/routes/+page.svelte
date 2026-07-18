<script lang="ts">
	import {
		getHealth,
		getStats,
		getTokenStats,
		getSessions,
		getLog,
		getDueReminders
	} from '$lib/api';
	import type {
		StatsResponse,
		TokenStats,
		SessionSnapshot,
		CommitEntry,
		Reminder
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
		}
		connected = await getHealth();
		await Promise.all([
			track(() => getTokenStats(), (l) => (tokensL = l)),
			track(() => getSessions(), (l) => (sessionsL = l)),
			track(() => getStats(branch), (l) => (statsL = l)),
			track(() => getLog(branch, 1000), (l) => (logL = l)),
			track(() => loadPlans(branch), (l) => (plansL = l)),
			track(() => getDueReminders(), (l) => (remindersL = l))
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
	const heatCells = $derived.by((): HeatCell[] => {
		const byDay = new Map<string, number>();
		for (const c of logL.data ?? []) {
			const day = c.timestamp.slice(0, 10);
			if (!/^\d{4}-\d{2}-\d{2}$/.test(day)) continue;
			byDay.set(day, (byDay.get(day) ?? 0) + 1);
		}
		return [...byDay.entries()].map(([date, count]) => ({ date, count }));
	});
	const recentCommits = $derived((logL.data ?? []).slice(0, 8));

	// Panel status helpers.
	function statusOf<T>(l: Load<T>, isEmpty: (d: T) => boolean): 'loading' | 'error' | 'empty' | 'ready' {
		if (l.status === 'loading') return 'loading';
		if (l.status === 'error') return 'error';
		return l.data !== null && !isEmpty(l.data) ? 'ready' : 'empty';
	}
	const econStatus = $derived(statusOf(sessionsL, () => !hasEconTraffic));
	const planStatus = $derived(statusOf(plansL, () => activePlans.length === 0));
	const activityStatus = $derived(statusOf(logL, (d) => d.length === 0));
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

	<!-- ── 5 · Quick capture ────────────────────────────────────────────── -->
	<Panel title="Remember a fact">
		<QuickCapture {connected} onSaved={() => refreshAll(false)} />
	</Panel>

	<!-- ── 3 · Plan health ──────────────────────────────────────────────── -->
	<Panel
		title="Plan health"
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

	<!-- ── 4 · Activity ─────────────────────────────────────────────────── -->
	<Panel
		title="Activity"
		links={[
			{ href: '/history', label: 'History' },
			{ href: '/tail', label: 'Live Tail' }
		]}
		status={activityStatus}
		errorText={logL.error}
		emptyTitle="No commits yet"
		emptyText="Memory writes on {branchStore.current} will show up here."
	>
		<CalendarHeatmap data={heatCells} ariaLabel="Memory commits per day" />
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
		align-items: start;
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
</style>
