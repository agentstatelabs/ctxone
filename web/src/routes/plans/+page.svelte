<script lang="ts">
	import { onMount } from 'svelte';
	import { branchStore } from '$lib/branchStore.svelte';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import { getBranches } from '$lib/api';
	import {
		listPlans,
		getPlan,
		createPlan,
		addTask,
		startTask,
		completeTask,
		abandonTask,
		archivePlan,
		forceCompletePlan,
		movePlan,
		nextTask,
		type AddTaskRequest,
		type Plan,
		type Proof,
		type Task
	} from '$lib/plansApi';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';
	import { buildGraph, effectivePlanStatus as effStatus, taskMatches } from './model';
	import PlanSwitcher from './PlanSwitcher.svelte';
	import BoardView from './BoardView.svelte';
	import ListView from './ListView.svelte';
	import TimelineView from './TimelineView.svelte';
	import TaskDetailPanel from './TaskDetailPanel.svelte';
	import AddTaskModal from './AddTaskModal.svelte';
	import ConfirmButton from './ConfirmButton.svelte';
	import CostPerFeature from './CostPerFeature.svelte';
	import ProvenanceCard from './ProvenanceCard.svelte';
	import { goto } from '$app/navigation';

	/** Chain the trust story: a plan's session → the sessions view. */
	function openSession(sessionId: string) {
		goto(`/sessions?session=${encodeURIComponent(sessionId)}`);
	}

	/* ---------------------------------------------------------------- *
	 *  Data                                                            *
	 * ---------------------------------------------------------------- */
	let plans: Plan[] = $state([]);
	let selectedName: string | null = $state(null);
	let selectedPlan = $state<Plan | null>(null);
	let error: string | null = $state(null);

	async function loadPlans() {
		error = null;
		try {
			plans = await listPlans(branchStore.current);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			plans = [];
		}
	}

	async function selectPlan(name: string) {
		selectedName = name;
		panelTaskId = null;
		panelIntent = null;
		try {
			selectedPlan = await getPlan(name, branchStore.current);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			selectedPlan = null;
		}
	}

	async function refreshSelected() {
		if (!selectedName) return;
		try {
			selectedPlan = await getPlan(selectedName, branchStore.current);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	onMount(async () => {
		await loadPlans();
		// Deep-link: /plans?plan=<name>&task=<t-id> opens that plan (and task).
		// Used by the session Commits section to jump from a commit to its task.
		if (typeof window !== 'undefined') {
			const p = new URLSearchParams(window.location.search);
			const plan = p.get('plan');
			const task = p.get('task');
			if (plan && plans.some((x) => x.name === plan)) {
				await selectPlan(plan);
				if (task) panelTaskId = task;
			}
		}
	});

	// Refresh both the list and the open plan on each tick — agents
	// looking at a plan want task-status changes to surface promptly.
	const auto = useAutoRefresh(async () => {
		await loadPlans();
		await refreshSelected();
	});

	// Branch / namespace switches invalidate everything (plan names and
	// task ids are ref-scoped) — reset selection and reload.
	//
	// `plans` MUST be cleared too, not just the selection: the auto-select
	// effect below runs the moment `selectedName` becomes null, and if the old
	// workspace's `plans` were still present it would pick one of those and
	// `getPlan(oldPlan, newRef)` would 404. Clearing `plans` makes auto-select
	// wait for `loadPlans()` to bring in the new workspace's list, then land on
	// the first plan that actually exists there.
	$effect(() => {
		void branchStore.current;
		void namespaceStore.current;
		selectedName = null;
		selectedPlan = null;
		plans = [];
		panelTaskId = null;
		panelIntent = null;
		error = null;
		loadPlans();
	});

	// Land somewhere useful: when nothing is selected, auto-open the
	// most relevant plan (in-progress first — same rank the switcher uses).
	const AUTO_RANK: Record<string, number> = { in_progress: 0, active: 1, completed: 2, archived: 3 };
	$effect(() => {
		if (selectedName !== null || plans.length === 0) return;
		const best = [...plans].sort((a, b) => {
			const ra = AUTO_RANK[effStatus(a)] ?? 99;
			const rb = AUTO_RANK[effStatus(b)] ?? 99;
			return ra - rb || a.name.localeCompare(b.name);
		})[0];
		if (best) selectPlan(best.name);
	});

	/* ---------------------------------------------------------------- *
	 *  View switcher (Board | List | Timeline) — persisted             *
	 * ---------------------------------------------------------------- */
	type ViewMode = 'board' | 'list' | 'timeline';
	const VIEW_KEY = 'lens.plans.viewMode';
	const VIEWS: ViewMode[] = ['board', 'list', 'timeline'];
	const VIEW_LABELS: Record<ViewMode, string> = { board: 'Board', list: 'List', timeline: 'Timeline' };
	function loadView(): ViewMode {
		if (typeof localStorage === 'undefined') return 'board';
		const v = localStorage.getItem(VIEW_KEY) as ViewMode | null;
		return v && VIEWS.includes(v) ? v : 'board';
	}
	let viewMode: ViewMode = $state(loadView());
	function setView(v: ViewMode) {
		viewMode = v;
		if (typeof localStorage !== 'undefined') localStorage.setItem(VIEW_KEY, v);
	}

	/* ---------------------------------------------------------------- *
	 *  Task search (debounced, like the old plan filter)               *
	 * ---------------------------------------------------------------- */
	let search = $state('');
	let appliedSearch = $state('');
	let searchTimer: ReturnType<typeof setTimeout> | null = null;
	$effect(() => {
		const v = search;
		if (searchTimer) clearTimeout(searchTimer);
		searchTimer = setTimeout(() => (appliedSearch = v), 150);
	});

	let allTasks = $derived(selectedPlan?.tasks ?? []);
	let graph = $derived(buildGraph(allTasks));
	let visibleTasks = $derived(allTasks.filter((t) => taskMatches(t, appliedSearch.trim())));

	/* ---------------------------------------------------------------- *
	 *  Detail panel                                                    *
	 * ---------------------------------------------------------------- */
	let panelTaskId: string | null = $state(null);
	let panelIntent: 'complete' | 'abandon' | null = $state(null);
	// Live task lookup so auto-refresh flows straight into the panel.
	let panelTask = $derived(
		panelTaskId ? (allTasks.find((t) => t.id === panelTaskId) ?? null) : null
	);

	function openTask(t: Task) {
		panelTaskId = t.id;
		panelIntent = null;
	}
	function openWithIntent(t: Task, intent: 'complete' | 'abandon') {
		panelTaskId = t.id;
		panelIntent = intent;
	}
	function closePanel() {
		panelTaskId = null;
		panelIntent = null;
	}

	/* ---------------------------------------------------------------- *
	 *  Toasts                                                          *
	 * ---------------------------------------------------------------- */
	interface Toast {
		id: number;
		msg: string;
		tone: 'info' | 'error';
	}
	let toasts: Toast[] = $state([]);
	let toastSeq = 0;
	function toast(msg: string, tone: 'info' | 'error' = 'info') {
		const id = ++toastSeq;
		toasts = [...toasts, { id, msg, tone }];
		setTimeout(() => {
			toasts = toasts.filter((t) => t.id !== id);
		}, 4200);
	}

	function errMsg(e: unknown): string {
		return e instanceof Error ? e.message : String(e);
	}

	/* ---------------------------------------------------------------- *
	 *  Task actions                                                    *
	 * ---------------------------------------------------------------- */
	async function doStart(t: Task) {
		if (!selectedName) return;
		try {
			await startTask(selectedName, t.id, branchStore.current);
			await refreshSelected();
			await loadPlans();
			toast(`Started ${t.id}`);
		} catch (e) {
			toast(errMsg(e), 'error');
		}
	}

	async function doComplete(t: Task, proof: Proof): Promise<boolean> {
		if (!selectedName) return false;
		try {
			await completeTask(selectedName, t.id, proof, branchStore.current);
			await refreshSelected();
			await loadPlans();
			toast(`${t.id} done — proof recorded (${proof.kind})`);
			return true;
		} catch (e) {
			toast(errMsg(e), 'error');
			return false;
		}
	}

	async function doAbandon(t: Task, reason: string): Promise<boolean> {
		if (!selectedName) return false;
		try {
			await abandonTask(selectedName, t.id, reason, branchStore.current);
			await refreshSelected();
			await loadPlans();
			toast(`${t.id} abandoned`);
			return true;
		} catch (e) {
			toast(errMsg(e), 'error');
			return false;
		}
	}

	/* ---------------------------------------------------------------- *
	 *  Add task / create plan                                          *
	 * ---------------------------------------------------------------- */
	let showAddTask = $state(false);

	async function handleAddTask(req: AddTaskRequest): Promise<boolean> {
		if (!selectedName) return false;
		try {
			const t = await addTask(selectedName, req, branchStore.current);
			showAddTask = false;
			await refreshSelected();
			await loadPlans();
			toast(`Added ${t.id} — ${t.title}`);
			return true;
		} catch (e) {
			toast(errMsg(e), 'error');
			return false;
		}
	}

	async function handleCreatePlan(name: string, description: string | null): Promise<boolean> {
		try {
			await createPlan(name, description, branchStore.current);
			await loadPlans();
			await selectPlan(name);
			toast(`Plan ${name} created`);
			return true;
		} catch (e) {
			toast(errMsg(e), 'error');
			return false;
		}
	}

	/* ---------------------------------------------------------------- *
	 *  Plan-level actions (overflow menu)                              *
	 * ---------------------------------------------------------------- */
	let showOverflow = $state(false);

	// Branch list for "Move to branch" — fetched lazily on mount, same
	// trade-off as the old page (layout doesn't expose its list).
	let allBranches: string[] = $state([]);
	let moveTarget = $state('');
	onMount(async () => {
		try {
			const list = await getBranches();
			allBranches = list.map((b) => b.name);
		} catch {
			allBranches = [];
		}
	});
	let movableBranches = $derived(allBranches.filter((b) => b !== branchStore.current));

	let openTaskCount = $derived(
		selectedPlan ? selectedPlan.task_counts.pending + selectedPlan.task_counts.in_progress : 0
	);

	async function handleArchive() {
		if (!selectedName) return;
		showOverflow = false;
		try {
			await archivePlan(selectedName, branchStore.current);
			await loadPlans();
			await refreshSelected();
			toast(`Plan ${selectedName} archived (stays browsable)`);
		} catch (e) {
			toast(errMsg(e), 'error');
		}
	}

	async function handleForceComplete() {
		if (!selectedName) return;
		showOverflow = false;
		try {
			const res = await forceCompletePlan(selectedName, branchStore.current);
			await loadPlans();
			await refreshSelected();
			const n = res.abandoned_task_ids.length;
			toast(
				n > 0
					? `Plan completed — ${n} open task${n === 1 ? '' : 's'} abandoned`
					: 'Plan marked complete'
			);
		} catch (e) {
			toast(errMsg(e), 'error');
		}
	}

	async function handleMovePlan() {
		if (!selectedName || !moveTarget) return;
		const target = moveTarget;
		showOverflow = false;
		try {
			await movePlan(selectedName, branchStore.current, target);
			toast(`Plan moved to ${target}`);
			// Follow the plan: the branchStore $effect reloads + clears selection.
			branchStore.current = target;
			moveTarget = '';
		} catch (e) {
			toast(errMsg(e), 'error');
		}
	}

	/* ---------------------------------------------------------------- *
	 *  "Next up" — the engine's own pick for what to work on           *
	 * ---------------------------------------------------------------- */
	async function handleNextUp() {
		if (!selectedName) return;
		try {
			const t = await nextTask(selectedName, { branch: branchStore.current });
			if (t) openTask(t);
			else toast('No eligible next task — everything is blocked, running, or closed.');
		} catch (e) {
			toast(errMsg(e), 'error');
		}
	}
</script>

<div class="page">
	<header class="topbar">
		<h2>Plans</h2>
		<PlanSwitcher {plans} {selectedName} onSelect={selectPlan} onCreate={handleCreatePlan} />
		<span class="branch-label">on {branchStore.current}</span>
		<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
		<span class="top-spacer"></span>
		{#if selectedPlan}
			<button type="button" class="tbtn" onclick={handleNextUp} title="Open the engine's next eligible task">
				▶ Next up
			</button>
			<button type="button" class="tbtn primary" onclick={() => (showAddTask = true)}>
				+ Add task
			</button>
			<div class="overflow">
				<button
					type="button"
					class="tbtn"
					aria-haspopup="menu"
					aria-expanded={showOverflow}
					aria-label="Plan actions"
					onclick={() => (showOverflow = !showOverflow)}
				>⋯</button>
				{#if showOverflow}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<!-- svelte-ignore a11y_click_events_have_key_events -->
					<div class="menu-backdrop" onclick={() => (showOverflow = false)}></div>
					<div class="menu" role="menu">
						<div class="menu-title">{selectedPlan.name}</div>
						<ConfirmButton
							menuItem
							label={openTaskCount > 0
								? `Mark complete (abandons ${openTaskCount} open)`
								: 'Mark complete'}
							confirmLabel="Confirm — mark complete"
							disabled={selectedPlan.status !== 'active'}
							onconfirm={handleForceComplete}
						/>
						<ConfirmButton
							menuItem
							label="Archive plan"
							confirmLabel="Confirm — archive"
							disabled={selectedPlan.status === 'archived'}
							onconfirm={handleArchive}
						/>
						{#if movableBranches.length > 0}
							<div class="menu-move">
								<span class="menu-move-label">Move to branch</span>
								<select bind:value={moveTarget} aria-label="Target branch">
									<option value="">— pick branch —</option>
									{#each movableBranches as b (b)}
										<option value={b}>{b}</option>
									{/each}
								</select>
								<ConfirmButton
									label="Move"
									confirmLabel="Confirm move"
									disabled={!moveTarget}
									onconfirm={handleMovePlan}
								/>
							</div>
						{/if}
					</div>
				{/if}
			</div>
		{/if}
	</header>

	{#if error}
		<p class="error">{error}</p>
	{/if}

	{#if selectedPlan}
		<div class="toolbar">
			<div class="seg-group" role="tablist" aria-label="View">
				{#each VIEWS as v (v)}
					<button
						type="button"
						class="seg"
						class:active={viewMode === v}
						role="tab"
						aria-selected={viewMode === v}
						onclick={() => setView(v)}
					>{VIEW_LABELS[v]}</button>
				{/each}
			</div>
			<input
				type="search"
				class="task-search"
				placeholder="Search tasks…"
				bind:value={search}
				aria-label="Search tasks"
			/>
			{#if selectedPlan.description}
				<span class="plan-desc" title={selectedPlan.description}>{selectedPlan.description}</span>
			{/if}
			<span class="tool-spacer"></span>
			{#if selectedPlan.task_counts.total > 0}
				{@const tc = selectedPlan.task_counts}
				<span
					class="progress"
					title="{tc.done} done · {tc.in_progress} in progress · {tc.pending} pending · {tc.abandoned} abandoned"
				>
					<span class="progress-bar" aria-hidden="true">
						<span class="p-done" style:flex-grow={tc.done}></span>
						<span class="p-prog" style:flex-grow={tc.in_progress}></span>
						<span class="p-pend" style:flex-grow={tc.pending}></span>
						<span class="p-aband" style:flex-grow={tc.abandoned}></span>
					</span>
					<span class="progress-text">{tc.done}/{tc.total}</span>
				</span>
			{/if}
		</div>

		{#if viewMode === 'board'}
			<BoardView
				tasks={visibleTasks}
				{graph}
				onOpen={openTask}
				onStart={doStart}
				onRequestComplete={(t) => openWithIntent(t, 'complete')}
				onRequestAbandon={(t) => openWithIntent(t, 'abandon')}
				onIllegal={(reason) => toast(reason, 'error')}
			/>
		{:else if viewMode === 'list'}
			<ListView
				tasks={visibleTasks}
				{graph}
				onOpen={openTask}
				onStart={doStart}
				onRequestComplete={(t) => openWithIntent(t, 'complete')}
				onRequestAbandon={(t) => openWithIntent(t, 'abandon')}
			/>
		{:else}
			<TimelineView tasks={visibleTasks} onOpen={openTask} />
		{/if}

		<!-- Trust & cost views (t-002 / t-004). Keyed on plan so they reload
		     when the selection changes; branch flows through to provenance. -->
		{#key selectedPlan.name + '@' + branchStore.current}
			<div class="trust-grid">
				<ProvenanceCard
					plan={selectedPlan.name}
					branch={branchStore.current}
					onOpenSession={openSession}
				/>
				<CostPerFeature plan={selectedPlan.name} onOpenSession={openSession} />
			</div>
		{/key}
	{:else if !error}
		<p class="empty">
			{plans.length === 0
				? 'No plans on this branch yet — create one from the switcher above.'
				: 'Select a plan from the switcher above.'}
		</p>
	{/if}
</div>

{#if panelTask && selectedPlan}
	<TaskDetailPanel
		task={panelTask}
		{graph}
		planName={selectedPlan.name}
		branch={branchStore.current}
		intent={panelIntent}
		onClose={closePanel}
		onNavigate={(id) => {
			panelTaskId = id;
			panelIntent = null;
		}}
		onStart={doStart}
		onComplete={doComplete}
		onAbandon={doAbandon}
	/>
{/if}

{#if showAddTask && selectedPlan}
	<AddTaskModal
		tasks={allTasks}
		planName={selectedPlan.name}
		onSubmit={handleAddTask}
		onClose={() => (showAddTask = false)}
	/>
{/if}

<div class="toasts" role="status" aria-live="polite">
	{#each toasts as t (t.id)}
		<div class="toast" class:err={t.tone === 'error'}>{t.msg}</div>
	{/each}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-4);
	}
	.topbar {
		display: flex;
		align-items: center;
		gap: var(--lens-space-3);
		flex-wrap: wrap;
	}
	.topbar h2 {
		margin: 0;
		font-size: var(--lens-font-size-lg);
		color: var(--lens-text-strong);
	}
	.branch-label {
		color: var(--lens-muted);
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
	}
	.ago {
		font-size: var(--lens-font-size-2xs);
		font-family: var(--lens-font-mono);
		color: var(--lens-text-faint);
	}
	.top-spacer,
	.tool-spacer {
		flex: 1;
	}
	.tbtn {
		background: var(--lens-surface);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		font-size: var(--lens-font-size-xs);
		padding: 0.32rem 0.7rem;
		cursor: pointer;
		white-space: nowrap;
	}
	.tbtn:hover {
		border-color: var(--lens-border-strong);
	}
	.tbtn.primary {
		background: var(--lens-accent-surface);
		border-color: var(--lens-accent-border);
		color: var(--lens-accent-hover);
	}
	.tbtn.primary:hover {
		background: var(--lens-accent-surface-hi);
	}
	.overflow {
		position: relative;
		display: inline-block;
	}
	.menu-backdrop {
		position: fixed;
		inset: 0;
		z-index: 60;
	}
	.menu {
		position: absolute;
		top: calc(100% + 4px);
		right: 0;
		z-index: 70;
		min-width: 15rem;
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
		background: var(--lens-overlay);
		border: 1px solid var(--lens-border-strong);
		border-radius: var(--lens-radius-md);
		box-shadow: var(--lens-shadow-md);
		padding: var(--lens-space-2);
	}
	.menu-title {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
		padding: 0.15rem 0.6rem 0.35rem;
		border-bottom: 1px solid var(--lens-border-subtle);
		margin-bottom: 0.2rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.menu-move {
		display: flex;
		flex-direction: column;
		gap: 0.35rem;
		border-top: 1px solid var(--lens-border-subtle);
		margin-top: 0.2rem;
		padding: 0.45rem 0.6rem 0.2rem;
	}
	.menu-move-label {
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}
	.menu-move select {
		background: var(--lens-bg);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: 0.25rem 0.4rem;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
	}
	.error {
		background: var(--lens-danger-tint);
		border: 1px solid var(--lens-danger-border);
		color: var(--lens-danger);
		padding: var(--lens-space-2) var(--lens-space-3);
		border-radius: var(--lens-radius-sm);
		font-size: var(--lens-font-size-sm);
		margin: 0;
	}
	.toolbar {
		display: flex;
		align-items: center;
		gap: var(--lens-space-3);
		flex-wrap: wrap;
	}
	.seg-group {
		display: inline-flex;
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		overflow: hidden;
	}
	.seg {
		background: var(--lens-surface);
		border: none;
		color: var(--lens-text-secondary);
		padding: 0.3rem 0.8rem;
		font-size: var(--lens-font-size-xs);
		cursor: pointer;
	}
	.seg:not(:last-child) {
		border-right: 1px solid var(--lens-border);
	}
	.seg.active {
		background: var(--lens-accent-tint);
		color: var(--lens-accent);
	}
	.task-search {
		background: var(--lens-surface);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: 0.32rem 0.55rem;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
		width: 14rem;
	}
	.plan-desc {
		color: var(--lens-muted);
		font-size: var(--lens-font-size-xs);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 24rem;
	}
	.progress {
		display: inline-flex;
		align-items: center;
		gap: var(--lens-space-2);
	}
	.progress-bar {
		display: inline-flex;
		width: 9rem;
		height: 0.4rem;
		border-radius: var(--lens-radius-full);
		overflow: hidden;
		background: var(--lens-surface-raised);
	}
	.progress-bar > span {
		flex-basis: 0;
	}
	.p-done {
		background: var(--lens-ok);
	}
	.p-prog {
		background: var(--lens-accent);
	}
	.p-pend {
		background: var(--lens-border-strong);
	}
	.p-aband {
		background: var(--lens-warn);
		opacity: 0.55;
	}
	.progress-text {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-text-secondary);
	}
	.empty {
		color: var(--lens-text-faint);
		font-style: italic;
		text-align: center;
		padding: var(--lens-space-12) 0;
	}
	/* Trust & cost cards sit side by side on wide viewports, stack on narrow. */
	.trust-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
		gap: var(--lens-space-4);
		margin-top: var(--lens-space-4);
	}
	.toasts {
		position: fixed;
		bottom: var(--lens-space-4);
		right: var(--lens-space-4);
		z-index: 120;
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-2);
		max-width: 24rem;
	}
	.toast {
		background: var(--lens-overlay);
		border: 1px solid var(--lens-border-strong);
		border-left: 3px solid var(--lens-accent);
		border-radius: var(--lens-radius-sm);
		box-shadow: var(--lens-shadow-md);
		color: var(--lens-text);
		font-size: var(--lens-font-size-xs);
		padding: var(--lens-space-2) var(--lens-space-3);
		animation: toast-in var(--lens-dur) var(--lens-ease-out);
	}
	.toast.err {
		border-left-color: var(--lens-danger);
		color: var(--lens-danger);
	}
	@keyframes toast-in {
		from {
			transform: translateY(8px);
			opacity: 0;
		}
		to {
			transform: translateY(0);
			opacity: 1;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.toast {
			animation: none;
		}
	}
</style>
