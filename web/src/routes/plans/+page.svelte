<script lang="ts">
	import { onMount } from 'svelte';
	import { branchStore } from '$lib/branchStore.svelte';
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
		type Plan,
		type Task,
		type Priority,
		type ProofKind
	} from '$lib/plansApi';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	let plans: Plan[] = $state([]);

	// View controls — Tree groups the sidebar by effective status, Flat
	// is the original single list. Persist so the choice sticks across
	// reloads (mirrors /browse, /pinned).
	type ViewMode = 'tree' | 'flat';
	const VIEW_KEY = 'lens.plans.view';
	function loadView(): ViewMode {
		if (typeof localStorage === 'undefined') return 'tree';
		const v = localStorage.getItem(VIEW_KEY);
		return v === 'flat' ? 'flat' : 'tree';
	}
	let viewMode: ViewMode = $state(loadView());
	function setView(v: ViewMode) {
		viewMode = v;
		if (typeof localStorage !== 'undefined') localStorage.setItem(VIEW_KEY, v);
	}

	// Status filter (t-001). 'all' is the no-op. Filter operates on the
	// *effective* status (in_progress / active / completed / archived)
	// — same buckets the grouped view uses, so the dropdown options
	// match what the user already sees in the sidebar.
	type StatusFilter = 'all' | 'in_progress' | 'active' | 'completed' | 'archived';
	const STATUS_FILTER_KEY = 'lens.plans.statusFilter';
	const STATUS_FILTERS: StatusFilter[] = ['all', 'in_progress', 'active', 'completed', 'archived'];
	function loadStatusFilter(): StatusFilter {
		if (typeof localStorage === 'undefined') return 'all';
		const v = localStorage.getItem(STATUS_FILTER_KEY) as StatusFilter | null;
		return v && STATUS_FILTERS.includes(v) ? v : 'all';
	}
	let statusFilter: StatusFilter = $state(loadStatusFilter());
	$effect(() => {
		if (typeof localStorage !== 'undefined')
			localStorage.setItem(STATUS_FILTER_KEY, statusFilter);
	});
	const STATUS_FILTER_LABELS: Record<StatusFilter, string> = {
		all: 'All statuses',
		in_progress: 'In progress',
		active: 'Active',
		completed: 'Completed',
		archived: 'Archived'
	};
	// Filter input — what the user is typing — and the debounced
	// "applied" value used for actual filtering. 150ms feels instant but
	// avoids re-filtering 800-plan lists on every keystroke.
	let filter = $state('');
	let appliedFilter = $state('');
	let filterTimer: ReturnType<typeof setTimeout> | null = null;
	$effect(() => {
		const v = filter;
		if (filterTimer) clearTimeout(filterTimer);
		filterTimer = setTimeout(() => {
			appliedFilter = v;
			page = 0; // reset to page 1 whenever the search changes
		}, 150);
	});

	// Pagination — Flat view only. Tree view groups by status and
	// would look broken if cut mid-bucket, so we leave it ungated.
	const PAGE_SIZE = 25;
	let page = $state(0);

	// Sort controls (t-014). Default is the "what should I look at
	// next" order — for plans that's status-first; for tasks it's
	// open-first then priority. Persist per-list so the agent's choice
	// sticks.
	type PlanSort = 'status' | 'date-new' | 'date-old' | 'name';
	type TaskSort = 'default' | 'priority' | 'date-new' | 'date-old' | 'name' | 'status';
	const PLAN_SORT_KEY = 'lens.plans.sort';
	const TASK_SORT_KEY = 'lens.plans.tasks.sort';
	function loadPlanSort(): PlanSort {
		if (typeof localStorage === 'undefined') return 'status';
		const v = localStorage.getItem(PLAN_SORT_KEY) as PlanSort | null;
		return v && ['status', 'date-new', 'date-old', 'name'].includes(v) ? v : 'status';
	}
	function loadTaskSort(): TaskSort {
		if (typeof localStorage === 'undefined') return 'default';
		const v = localStorage.getItem(TASK_SORT_KEY) as TaskSort | null;
		return v && ['default', 'priority', 'date-new', 'date-old', 'name', 'status'].includes(v)
			? v
			: 'default';
	}
	let planSort: PlanSort = $state(loadPlanSort());
	let taskSort: TaskSort = $state(loadTaskSort());
	$effect(() => {
		if (typeof localStorage !== 'undefined') localStorage.setItem(PLAN_SORT_KEY, planSort);
	});
	$effect(() => {
		if (typeof localStorage !== 'undefined') localStorage.setItem(TASK_SORT_KEY, taskSort);
	});

	let collapsedGroups: Set<string> = $state(new Set());
	function toggleGroup(g: string) {
		const next = new Set(collapsedGroups);
		if (next.has(g)) next.delete(g);
		else next.add(g);
		collapsedGroups = next;
	}

	// Per-bucket "show all" tracking (t-002 of lens-enhancements-3).
	// Tree view shows the first GROUP_PAGE_SIZE plans per status bucket
	// by default; clicking "Show all" reveals the rest. Buckets stay in
	// their chosen state across filter/sort changes — agents typically
	// expand the bucket they care about and don't want it snapping shut.
	const GROUP_PAGE_SIZE = 25;
	let expandedGroups: Set<string> = $state(new Set());
	function toggleGroupExpanded(g: string) {
		const next = new Set(expandedGroups);
		if (next.has(g)) next.delete(g);
		else next.add(g);
		expandedGroups = next;
	}

	let selectedName: string | null = $state(null);
	let selectedPlan: Plan | null = $state(null);
	let selectedTask: Task | null = $state(null);
	let error: string | null = $state(null);

	// Create-plan form
	let showCreate = $state(false);
	let newPlanName = $state('');
	let newPlanDesc = $state('');

	// Plan-header actions dropdown (Mark complete / Archive). One toggle
	// instead of stacked buttons keeps the header tidy as more actions land.
	let showPlanActions = $state(false);

	// Add-task form
	let showAddTask = $state(false);
	let newTaskTitle = $state('');
	let newTaskPriority: Priority = $state('medium');
	let newTaskAssigned = $state('');

	// Proof modal for Done
	let proofOpen = $state(false);
	let proofKind: ProofKind = $state('commit');
	let proofValue = $state('');
	let proofNote = $state('');

	// Abandon modal
	let abandonOpen = $state(false);
	let abandonReason = $state('');

	// Inline expanded-row tracking — a Set of task ids that are
	// currently showing the details accordion. Multiple rows can be
	// open at once and stay open across sort changes / auto-refresh.
	// Cleared on plan change because task ids are scoped to a plan.
	let expandedTaskIds: Set<string> = $state(new Set());
	function toggleExpanded(id: string) {
		const next = new Set(expandedTaskIds);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		expandedTaskIds = next;
	}
	function isExpanded(id: string): boolean {
		return expandedTaskIds.has(id);
	}
	// Pretty-print a Task for the "raw JSON" footer of the accordion.
	// The Hub may carry fields the typed client doesn't model (e.g.
	// payload, on_complete) — surfacing the JSON is the safety net.
	function rawJson(t: Task | null): string {
		if (!t) return '';
		try {
			return JSON.stringify(t, null, 2);
		} catch {
			return String(t);
		}
	}
	function formatTs(ts: string | null): string {
		if (!ts) return '—';
		try {
			const d = new Date(ts);
			return `${d.toLocaleString()} (${ts})`;
		} catch {
			return ts;
		}
	}

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
		selectedTask = null;
		try {
			selectedPlan = await getPlan(name, branchStore.current);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			selectedPlan = null;
		}
	}

	async function refreshSelected() {
		if (selectedName) await selectPlan(selectedName);
	}

	onMount(loadPlans);

	// Refresh both the list and the open plan on each tick — agents
	// looking at a plan want task-status changes to surface promptly.
	const auto = useAutoRefresh(async () => {
		await loadPlans();
		if (selectedName) await refreshSelected();
	});

	$effect(() => {
		void branchStore.current;
		selectedName = null;
		selectedPlan = null;
		selectedTask = null;
		expandedTaskIds = new Set();
		loadPlans();
	});

	// Clear expanded rows when the user picks a different plan — task
	// ids are plan-scoped so leftover ids would be misleading. Sort
	// changes and auto-refresh deliberately preserve the set.
	$effect(() => {
		void selectedName;
		expandedTaskIds = new Set();
	});

	async function handleCreate(e: Event) {
		e.preventDefault();
		if (!newPlanName.trim()) return;
		try {
			await createPlan(
				newPlanName.trim(),
				newPlanDesc.trim() || null,
				branchStore.current
			);
			newPlanName = '';
			newPlanDesc = '';
			showCreate = false;
			await loadPlans();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		}
	}

	async function handleAddTask(e: Event) {
		e.preventDefault();
		if (!selectedName || !newTaskTitle.trim()) return;
		try {
			await addTask(
				selectedName,
				{
					title: newTaskTitle.trim(),
					priority: newTaskPriority,
					assigned_to: newTaskAssigned.trim() || undefined
				},
				branchStore.current
			);
			newTaskTitle = '';
			newTaskAssigned = '';
			newTaskPriority = 'medium';
			showAddTask = false;
			await refreshSelected();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		}
	}

	async function handleStart(task: Task) {
		if (!selectedName) return;
		try {
			await startTask(selectedName, task.id, branchStore.current);
			await refreshSelected();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		}
	}

	function openProof(task: Task) {
		selectedTask = task;
		proofKind = 'commit';
		proofValue = '';
		proofNote = '';
		proofOpen = true;
	}

	async function submitProof(e: Event) {
		e.preventDefault();
		if (!selectedName || !selectedTask) return;
		if (!proofValue.trim()) return;
		try {
			await completeTask(
				selectedName,
				selectedTask.id,
				{
					kind: proofKind,
					value: proofValue.trim(),
					note: proofNote.trim() || null
				},
				branchStore.current
			);
			proofOpen = false;
			selectedTask = null;
			await refreshSelected();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		}
	}

	function openAbandon(task: Task) {
		selectedTask = task;
		abandonReason = '';
		abandonOpen = true;
	}

	async function submitAbandon(e: Event) {
		e.preventDefault();
		if (!selectedName || !selectedTask) return;
		if (!abandonReason.trim()) return;
		try {
			await abandonTask(
				selectedName,
				selectedTask.id,
				abandonReason.trim(),
				branchStore.current
			);
			abandonOpen = false;
			selectedTask = null;
			await refreshSelected();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		}
	}

	// "Move to branch" — populated lazily on plan-detail mount so the
	// dropdown always reflects the current branch list. We could share
	// the layout's fetch via branchStore, but the layout doesn't
	// expose the list — fetching here is one extra request and keeps
	// the data flow obvious.
	let allBranches: string[] = $state([]);
	let moveTarget = $state('');
	async function refreshBranchList() {
		try {
			const list = await getBranches();
			allBranches = list.map((b) => b.name);
		} catch {
			allBranches = [];
		}
	}
	onMount(refreshBranchList);

	let movableBranches = $derived(allBranches.filter((b) => b !== branchStore.current));

	async function handleMovePlan() {
		if (!selectedName || !moveTarget) return;
		const target = moveTarget;
		const source = branchStore.current;
		if (!confirm(`Move plan "${selectedName}" from ${source} to ${target}?`)) return;
		try {
			await movePlan(selectedName, source, target);
			// Switch to the target branch so the user lands where their
			// plan now lives. The $effect on branchStore.current will
			// reload plans + clear selection.
			branchStore.current = target;
			moveTarget = '';
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		}
	}

	// Force-complete the open plan. Counts open tasks first so the
	// confirmation prompt names the actual fallout. Disabled when the
	// plan is already terminal (Completed / Archived).
	async function handleForceComplete() {
		if (!selectedPlan || !selectedName) return;
		const tc = selectedPlan.task_counts;
		const openCount = tc.pending + tc.in_progress;
		const msg = openCount === 0
			? `Mark plan "${selectedName}" complete?`
			: `Mark plan "${selectedName}" complete? This will abandon ${openCount} open task${openCount === 1 ? '' : 's'} ` +
				`with the reason "Plan force-completed by user".`;
		if (!confirm(msg)) return;
		try {
			await forceCompletePlan(selectedName, branchStore.current);
			await loadPlans();
			await refreshSelected();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		}
	}

	async function handleArchive() {
		if (!selectedName) return;
		if (!confirm(`Archive plan "${selectedName}"? It stays browsable.`)) return;
		try {
			await archivePlan(selectedName, branchStore.current);
			await loadPlans();
			await refreshSelected();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		}
	}

	type PlanLike = { status: string; task_counts: { done: number; in_progress: number; pending: number; abandoned: number; total: number } };

	function effectivePlanStatus(p: PlanLike): string {
		if (p.status === 'archived') return 'archived';
		const tc = p.task_counts;
		if (tc.in_progress > 0) return 'in_progress';
		if (tc.pending > 0) return 'active';
		if (tc.total > 0 && tc.done + tc.abandoned === tc.total) return 'completed';
		return p.status;
	}

	function statusGlyph(status: string): string {
		switch (status) {
			case 'done':
				return '\u2713';
			case 'in_progress':
				return '>';
			case 'abandoned':
				return '!';
			default:
				return ' ';
		}
	}

	function priorityClass(p: string): string {
		switch (p) {
			case 'critical':
				return 'pri-critical';
			case 'high':
				return 'pri-high';
			case 'low':
				return 'pri-low';
			default:
				return 'pri-medium';
		}
	}

	// Apply the filter input (case-insensitive substring) to plan name
	// + description. Empty filter is the identity.
	let filteredPlans = $derived.by(() => {
		const q = appliedFilter.trim().toLowerCase();
		const sf = statusFilter;
		return plans.filter((p) => {
			if (sf !== 'all' && effectivePlanStatus(p) !== sf) return false;
			if (!q) return true;
			return (
				p.name.toLowerCase().includes(q) ||
				(p.description ?? '').toLowerCase().includes(q)
			);
		});
	});

	// Reset to page 1 whenever the status filter changes — same UX as
	// for text-search changes.
	$effect(() => {
		void statusFilter;
		page = 0;
	});

	// Status priority for the "status" sort — lower = sorts first.
	// Mirrors the grouped-view bucket order: in_progress → active →
	// completed → archived.
	const PLAN_STATUS_RANK: Record<string, number> = {
		in_progress: 0,
		active: 1,
		completed: 2,
		archived: 3
	};
	function planActivityTs(p: Plan): number {
		// "Most-recent activity" proxy: archived_at if archived,
		// otherwise created_at. The Plan shape doesn't carry a
		// last-touched timestamp; this is good enough to keep
		// recently-archived plans near the top of their bucket.
		const t = p.archived_at ?? p.created_at;
		return t ? new Date(t).getTime() : 0;
	}
	function comparePlans(a: Plan, b: Plan): number {
		switch (planSort) {
			case 'status': {
				const ea = effectivePlanStatus(a);
				const eb = effectivePlanStatus(b);
				const ra = PLAN_STATUS_RANK[ea] ?? 99;
				const rb = PLAN_STATUS_RANK[eb] ?? 99;
				if (ra !== rb) return ra - rb;
				return planActivityTs(b) - planActivityTs(a);
			}
			case 'date-new':
				return planActivityTs(b) - planActivityTs(a);
			case 'date-old':
				return planActivityTs(a) - planActivityTs(b);
			case 'name':
				return a.name.localeCompare(b.name);
		}
	}
	let sortedPlans = $derived.by(() => [...filteredPlans].sort(comparePlans));

	// Page slice (Flat view only). Clamp page when filter shrinks the list.
	let pageCount = $derived(Math.max(1, Math.ceil(sortedPlans.length / PAGE_SIZE)));
	$effect(() => {
		if (page >= pageCount) page = pageCount - 1;
		if (page < 0) page = 0;
	});
	let pagedPlans = $derived.by(() => {
		const start = page * PAGE_SIZE;
		return sortedPlans.slice(start, start + PAGE_SIZE);
	});

	const PRIORITY_RANK: Record<string, number> = {
		critical: 0,
		high: 1,
		medium: 2,
		low: 3
	};
	const TASK_STATUS_RANK: Record<string, number> = {
		in_progress: 0,
		pending: 1,
		done: 2,
		abandoned: 3
	};
	function taskActivityTs(t: Task): number {
		// Most-recent timestamp on the task (any of the lifecycle stamps).
		const stamps = [t.completed_at, t.abandoned_at, t.started_at, t.created_at]
			.filter((s): s is string => !!s)
			.map((s) => new Date(s).getTime());
		return stamps.length > 0 ? Math.max(...stamps) : 0;
	}
	function compareTasks(a: Task, b: Task): number {
		switch (taskSort) {
			case 'default': {
				// Open-first (in_progress, pending) then priority then
				// created_at ascending — agents almost always want to
				// know "what's the most important open thing on this
				// plan" without thinking about it.
				const ra = TASK_STATUS_RANK[a.status] ?? 99;
				const rb = TASK_STATUS_RANK[b.status] ?? 99;
				if (ra !== rb) return ra - rb;
				const pa = PRIORITY_RANK[a.priority] ?? 99;
				const pb = PRIORITY_RANK[b.priority] ?? 99;
				if (pa !== pb) return pa - pb;
				return (
					(a.created_at ? new Date(a.created_at).getTime() : 0) -
					(b.created_at ? new Date(b.created_at).getTime() : 0)
				);
			}
			case 'priority': {
				const pa = PRIORITY_RANK[a.priority] ?? 99;
				const pb = PRIORITY_RANK[b.priority] ?? 99;
				if (pa !== pb) return pa - pb;
				return a.id.localeCompare(b.id);
			}
			case 'status': {
				const ra = TASK_STATUS_RANK[a.status] ?? 99;
				const rb = TASK_STATUS_RANK[b.status] ?? 99;
				if (ra !== rb) return ra - rb;
				return a.id.localeCompare(b.id);
			}
			case 'date-new':
				return taskActivityTs(b) - taskActivityTs(a);
			case 'date-old':
				return taskActivityTs(a) - taskActivityTs(b);
			case 'name':
				return a.title.localeCompare(b.title);
		}
	}
	let sortedTasks = $derived.by(() =>
		selectedPlan?.tasks ? [...selectedPlan.tasks].sort(compareTasks) : []
	);

	// Task-list pagination (t-001 of lens-enhancements-3). 50/page is
	// roughly one screenful at typical row heights; prev/next is enough
	// because deep-jumping in a single plan's task list is unusual.
	const TASK_PAGE_SIZE = 25;
	let taskPage = $state(0);
	let taskPageCount = $derived(Math.max(1, Math.ceil(sortedTasks.length / TASK_PAGE_SIZE)));
	$effect(() => {
		if (taskPage >= taskPageCount) taskPage = taskPageCount - 1;
		if (taskPage < 0) taskPage = 0;
	});
	// Reset on plan switch and sort change — both invalidate "page 3".
	$effect(() => {
		void selectedName;
		void taskSort;
		taskPage = 0;
	});
	let pagedTasks = $derived.by(() => {
		const start = taskPage * TASK_PAGE_SIZE;
		return sortedTasks.slice(start, start + TASK_PAGE_SIZE);
	});

	// Tree-view: group by effective status. Order matters — agents
	// almost always want "what's in flight right now" first.
	const STATUS_ORDER = ['in_progress', 'active', 'completed', 'archived'] as const;
	const STATUS_LABELS: Record<string, string> = {
		in_progress: 'In progress',
		active: 'Active (pending tasks)',
		completed: 'Completed',
		archived: 'Archived'
	};
	let groupedPlans = $derived.by(() => {
		const buckets: Record<string, Plan[]> = {};
		for (const p of sortedPlans) {
			const eff = effectivePlanStatus(p);
			(buckets[eff] ??= []).push(p);
		}
		return STATUS_ORDER.filter((s) => buckets[s]?.length).map((s) => ({
			key: s,
			label: STATUS_LABELS[s],
			plans: buckets[s]
		}));
	});

	function statusClass(s: string): string {
		switch (s) {
			case 'done':
				return 'task-done';
			case 'in_progress':
				return 'task-progress';
			case 'abandoned':
				return 'task-abandoned';
			default:
				return 'task-pending';
		}
	}
</script>

<h2>
	Plans <span class="branch-label">on {branchStore.current}</span>
	<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
	<button class="btn-sm" onclick={() => (showCreate = !showCreate)}>
		{showCreate ? 'Cancel' : '+ New plan'}
	</button>
</h2>

{#if error}
	<p class="error">{error}</p>
{/if}

{#if showCreate}
	<form class="create-form" onsubmit={handleCreate}>
		<input
			type="text"
			bind:value={newPlanName}
			placeholder="plan-name (kebab-case)"
			required
		/>
		<input
			type="text"
			bind:value={newPlanDesc}
			placeholder="description (optional)"
		/>
		<button type="submit">Create</button>
	</form>
{/if}

<div class="layout">
	<aside class="plan-list">
		<div class="sidebar-controls">
			<div class="seg-group" role="tablist" aria-label="View mode">
				<button
					class="seg"
					class:active={viewMode === 'tree'}
					onclick={() => setView('tree')}
					type="button"
				>Grouped</button>
				<button
					class="seg"
					class:active={viewMode === 'flat'}
					onclick={() => setView('flat')}
					type="button"
				>Flat</button>
			</div>
			<input
				type="search"
				class="filter-input"
				placeholder="Search plans…"
				bind:value={filter}
				aria-label="Search plans"
			/>
			<label class="sort-row">
				<span>Status</span>
				<select bind:value={statusFilter} aria-label="Filter plans by status">
					{#each STATUS_FILTERS as s}
						<option value={s}>{STATUS_FILTER_LABELS[s]}</option>
					{/each}
				</select>
			</label>
			<label class="sort-row">
				<span>Sort</span>
				<select bind:value={planSort} aria-label="Sort plans">
					<option value="status">Status (default)</option>
					<option value="date-new">Date — newest</option>
					<option value="date-old">Date — oldest</option>
					<option value="name">Name (A→Z)</option>
				</select>
			</label>
		</div>

		{#if plans.length === 0}
			<p class="empty">No plans yet.</p>
		{:else if filteredPlans.length === 0}
			<p class="empty">No plans match "{appliedFilter}".</p>
		{:else if viewMode === 'tree'}
			{#each groupedPlans as group}
				{@const collapsed = collapsedGroups.has(group.key)}
				{@const expanded = expandedGroups.has(group.key)}
				{@const hidden = Math.max(0, group.plans.length - GROUP_PAGE_SIZE)}
				{@const visible = expanded || hidden === 0
					? group.plans
					: group.plans.slice(0, GROUP_PAGE_SIZE)}
				<div class="status-group">
					<button
						class="status-header"
						type="button"
						onclick={() => toggleGroup(group.key)}
						aria-expanded={!collapsed}
					>
						<span class="caret">{collapsed ? '▸' : '▾'}</span>
						<span class="status-label plan-status-{group.key}">{group.label}</span>
						<span class="status-count">{group.plans.length}</span>
					</button>
					{#if !collapsed}
						{#each visible as plan}
							{@const eff = effectivePlanStatus(plan)}
							<button
								class="plan-row"
								class:selected={plan.name === selectedName}
								onclick={() => selectPlan(plan.name)}
							>
								<div class="plan-name">{plan.name}</div>
								<div class="plan-meta">
									<span class="plan-status plan-status-{eff}">{eff.replace('_', ' ')}</span>
									<span class="plan-counts">
										{plan.task_counts.done}&check;
										{plan.task_counts.in_progress}&gt;
										{plan.task_counts.pending}&middot;
									</span>
								</div>
							</button>
						{/each}
						{#if hidden > 0}
							<button
								type="button"
								class="show-more-btn"
								onclick={() => toggleGroupExpanded(group.key)}
							>
								{expanded
									? `Show less (hide ${hidden})`
									: `Show all ${group.plans.length} (${hidden} more)`}
							</button>
						{/if}
					{/if}
				</div>
			{/each}
		{:else}
			{#each pagedPlans as plan}
				{@const eff = effectivePlanStatus(plan)}
				<button
					class="plan-row"
					class:selected={plan.name === selectedName}
					onclick={() => selectPlan(plan.name)}
				>
					<div class="plan-name">{plan.name}</div>
					<div class="plan-meta">
						<span class="plan-status plan-status-{eff}">{eff.replace('_', ' ')}</span>
						<span class="plan-counts">
							{plan.task_counts.done}&check;
							{plan.task_counts.in_progress}&gt;
							{plan.task_counts.pending}&middot;
						</span>
					</div>
				</button>
			{/each}
			{#if pageCount > 1}
				<nav class="paginator" aria-label="Plans pagination">
					<button
						type="button"
						class="page-btn"
						onclick={() => (page = Math.max(0, page - 1))}
						disabled={page === 0}
					>‹ Prev</button>
					<span class="page-info">
						Page {page + 1} of {pageCount}
						<span class="page-total">({sortedPlans.length} plans)</span>
					</span>
					<button
						type="button"
						class="page-btn"
						onclick={() => (page = Math.min(pageCount - 1, page + 1))}
						disabled={page >= pageCount - 1}
					>Next ›</button>
				</nav>
			{:else if sortedPlans.length > 0}
				<p class="page-total page-info-static">{sortedPlans.length} plans</p>
			{/if}
		{/if}
	</aside>

	<section class="plan-detail">
		{#if !selectedPlan}
			<p class="empty">Select a plan to see its tasks.</p>
		{:else}
			<header class="plan-header">
				<div>
					<h3>{selectedPlan.name}</h3>
					{#if selectedPlan.description}
						<p class="plan-desc">{selectedPlan.description}</p>
					{/if}
				</div>
				<div class="plan-actions">
					<label class="task-sort-row">
						<span>Sort tasks</span>
						<select bind:value={taskSort} aria-label="Sort tasks">
							<option value="default">Open + priority (default)</option>
							<option value="status">Status</option>
							<option value="priority">Priority</option>
							<option value="date-new">Date — newest</option>
							<option value="date-old">Date — oldest</option>
							<option value="name">Title (A→Z)</option>
						</select>
					</label>
					<button onclick={() => (showAddTask = !showAddTask)}>
						{showAddTask ? 'Cancel' : '+ Add task'}
					</button>
					{#if movableBranches.length > 0}
						<label class="move-row">
							<span>Move to</span>
							<select bind:value={moveTarget} aria-label="Target branch">
								<option value="">— pick branch —</option>
								{#each movableBranches as b}
									<option value={b}>{b}</option>
								{/each}
							</select>
							<button
								class="btn-secondary btn-xs"
								onclick={handleMovePlan}
								disabled={!moveTarget}
							>
								Go
							</button>
						</label>
					{/if}
					{#if selectedPlan.status === 'active'}
						<div class="actions-menu">
							<button
								class="btn-secondary actions-toggle"
								type="button"
								aria-haspopup="menu"
								aria-expanded={showPlanActions}
								onclick={() => (showPlanActions = !showPlanActions)}
							>
								Actions <span class="caret" aria-hidden="true">▾</span>
							</button>
							{#if showPlanActions}
								<!-- svelte-ignore a11y_click_events_have_key_events -->
								<!-- svelte-ignore a11y_no_static_element_interactions -->
								<div
									class="actions-backdrop"
									onclick={() => (showPlanActions = false)}
								></div>
								<div class="actions-menu-list" role="menu">
									<button
										role="menuitem"
										type="button"
										class="actions-menu-item complete"
										onclick={() => {
											showPlanActions = false;
											handleForceComplete();
										}}
									>
										Mark complete
									</button>
									<button
										role="menuitem"
										type="button"
										class="actions-menu-item"
										onclick={() => {
											showPlanActions = false;
											handleArchive();
										}}
									>
										Archive
									</button>
								</div>
							{/if}
						</div>
					{/if}
				</div>
			</header>

			{#if showAddTask}
				<form class="add-task-form" onsubmit={handleAddTask}>
					<input
						type="text"
						bind:value={newTaskTitle}
						placeholder="task title (imperative)"
						required
					/>
					<select bind:value={newTaskPriority}>
						<option value="low">Low</option>
						<option value="medium">Medium</option>
						<option value="high">High</option>
						<option value="critical">Critical</option>
					</select>
					<input
						type="text"
						bind:value={newTaskAssigned}
						placeholder="assign to (e.g. claude-code) — optional"
					/>
					<button type="submit">Add</button>
				</form>
			{/if}

			{#if sortedTasks.length > 0}
				<ul class="task-list">
					{#each pagedTasks as task}
						{@const expanded = isExpanded(task.id)}
						<li class="task-item">
							<div
								class="task-row {statusClass(task.status)} task-clickable"
								class:expanded
								role="button"
								tabindex="0"
								aria-expanded={expanded}
								title={expanded ? 'Click to collapse details' : 'Click to expand details'}
								onclick={() => toggleExpanded(task.id)}
								onkeydown={(e) => {
									if (e.key === 'Enter' || e.key === ' ') {
										e.preventDefault();
										toggleExpanded(task.id);
									}
								}}
							>
								<span class="task-caret" aria-hidden="true">{expanded ? '▾' : '▸'}</span>
								<span class="task-glyph">[{statusGlyph(task.status)}]</span>
								<span class="task-id">{task.id}</span>
								<span class="pri-tag {priorityClass(task.priority)}">
									{task.priority.slice(0, 2).toUpperCase()}
								</span>
								<span class="task-title">{task.title}</span>
								{#if task.assigned_to}
									<span class="assigned">@{task.assigned_to}</span>
								{/if}
								<span class="task-actions" onclick={(e) => e.stopPropagation()}>
									{#if task.status === 'pending'}
										<button class="btn-xs" onclick={() => handleStart(task)}>
											Start
										</button>
										<button class="btn-xs btn-secondary" onclick={() => openAbandon(task)}>
											Abandon
										</button>
									{:else if task.status === 'in_progress'}
										<button class="btn-xs" onclick={() => openProof(task)}>
											Done
										</button>
										<button class="btn-xs btn-secondary" onclick={() => openAbandon(task)}>
											Abandon
										</button>
									{/if}
								</span>
								{#if task.proof}
									<div class="task-proof">
										proof: {task.proof.kind} {task.proof.value}
										{#if task.proof.note}
											&mdash; {task.proof.note}
										{/if}
									</div>
								{/if}
								{#if task.status === 'abandoned' && task.abandoned_reason}
									<div class="task-proof">reason: {task.abandoned_reason}</div>
								{/if}
								{#if task.blocked_by.length > 0}
									<div class="task-proof">
										blocked by: {task.blocked_by.join(', ')}
									</div>
								{/if}
							</div>
							{#if expanded}
								<div class="task-details">
									<dl class="details-grid">
										<dt>Priority</dt><dd>{task.priority}</dd>
										<dt>Status</dt><dd>{task.status}</dd>
										<dt>Assigned to</dt><dd>{task.assigned_to ?? '—'}</dd>
										<dt>Parent</dt><dd>{task.parent_id ?? '—'}</dd>
										<dt>Blocked by</dt>
										<dd>
											{#if task.blocked_by.length > 0}
												{task.blocked_by.join(', ')}
											{:else}
												—
											{/if}
										</dd>
										<dt>Created</dt>
										<dd>
											{formatTs(task.created_at)}
											{#if task.created_by}<span class="by">by {task.created_by}</span>{/if}
										</dd>
										<dt>Started</dt>
										<dd>
											{formatTs(task.started_at)}
											{#if task.started_by}<span class="by">by {task.started_by}</span>{/if}
										</dd>
										<dt>Completed</dt>
										<dd>
											{formatTs(task.completed_at)}
											{#if task.completed_by}<span class="by">by {task.completed_by}</span>{/if}
										</dd>
										<dt>Abandoned</dt>
										<dd>
											{formatTs(task.abandoned_at)}
											{#if task.abandoned_reason}
												<div class="reason">{task.abandoned_reason}</div>
											{/if}
										</dd>
										<dt>Proof</dt>
										<dd>
											{#if task.proof}
												<span class="proof-kind">{task.proof.kind}</span>
												<code class="proof-value">{task.proof.value}</code>
												{#if task.proof.note}
													<div class="reason">{task.proof.note}</div>
												{/if}
											{:else}
												—
											{/if}
										</dd>
									</dl>
									<details class="raw-json">
										<summary>Raw JSON</summary>
										<pre>{rawJson(task)}</pre>
									</details>
								</div>
							{/if}
						</li>
					{/each}
				</ul>
				{#if taskPageCount > 1}
					<nav class="paginator" aria-label="Tasks pagination">
						<button
							type="button"
							class="page-btn"
							onclick={() => (taskPage = Math.max(0, taskPage - 1))}
							disabled={taskPage === 0}
						>‹ Prev</button>
						<span class="page-info">
							Page {taskPage + 1} of {taskPageCount}
							<span class="page-total">({sortedTasks.length} tasks)</span>
						</span>
						<button
							type="button"
							class="page-btn"
							onclick={() => (taskPage = Math.min(taskPageCount - 1, taskPage + 1))}
							disabled={taskPage >= taskPageCount - 1}
						>Next ›</button>
					</nav>
				{:else if sortedTasks.length > TASK_PAGE_SIZE / 2}
					<p class="page-total page-info-static">{sortedTasks.length} tasks</p>
				{/if}
			{:else}
				<p class="empty">No tasks in this plan yet.</p>
			{/if}
		{/if}
	</section>
</div>

{#if proofOpen}
	<div class="modal-backdrop" onclick={() => (proofOpen = false)}>
		<div class="modal" onclick={(e) => e.stopPropagation()}>
			<h3>Mark task {selectedTask?.id} done</h3>
			<p class="hint">
				A proof is required — a commit SHA is strongest, then a file path,
				then a test name, then free text.
			</p>
			<form onsubmit={submitProof}>
				<label>
					Kind
					<select bind:value={proofKind}>
						<option value="commit">commit</option>
						<option value="file">file</option>
						<option value="test">test</option>
						<option value="text">text</option>
					</select>
				</label>
				<label>
					Value
					<input
						type="text"
						bind:value={proofValue}
						placeholder={
							proofKind === 'commit'
								? 'git SHA (e.g. ef6ce63)'
								: proofKind === 'file'
									? 'path/to/file'
									: proofKind === 'test'
										? 'test_function_name'
										: 'free-text description'
						}
						required
					/>
				</label>
				<label>
					Note (optional)
					<input type="text" bind:value={proofNote} />
				</label>
				<div class="modal-actions">
					<button type="submit">Mark done</button>
					<button type="button" class="btn-secondary" onclick={() => (proofOpen = false)}>
						Cancel
					</button>
				</div>
			</form>
		</div>
	</div>
{/if}

{#if abandonOpen}
	<div class="modal-backdrop" onclick={() => (abandonOpen = false)}>
		<div class="modal" onclick={(e) => e.stopPropagation()}>
			<h3>Abandon task {selectedTask?.id}</h3>
			<form onsubmit={submitAbandon}>
				<label>
					Reason
					<input
						type="text"
						bind:value={abandonReason}
						placeholder="why is this task being dropped?"
						required
					/>
				</label>
				<div class="modal-actions">
					<button type="submit">Abandon</button>
					<button type="button" class="btn-secondary" onclick={() => (abandonOpen = false)}>
						Cancel
					</button>
				</div>
			</form>
		</div>
	</div>
{/if}

<style>
	h2 {
		display: flex;
		align-items: center;
		gap: 1rem;
		margin-bottom: 1rem;
	}
	.branch-label {
		color: var(--text-3);
		font-family: monospace;
		font-weight: normal;
		font-size: 0.9rem;
	}
	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		font-weight: normal;
		margin-left: 0.75rem;
	}
	.error {
		background: color-mix(in srgb, var(--danger) 18%, transparent);
		border: 1px solid color-mix(in srgb, var(--danger) 40%, transparent);
		color: var(--danger);
		padding: 0.6rem;
		border-radius: 4px;
	}
	.empty {
		color: var(--text-3);
		font-style: italic;
	}
	.create-form,
	.add-task-form {
		display: flex;
		gap: 0.5rem;
		flex-wrap: wrap;
		margin-bottom: 1rem;
	}
	.create-form input,
	.add-task-form input,
	.add-task-form select {
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.4rem 0.6rem;
		border-radius: 4px;
		flex: 1 1 12rem;
		font-family: monospace;
	}
	button {
		background: var(--accent-bg);
		border: 1px solid var(--accent-bg-hi);
		color: var(--accent);
		padding: 0.4rem 0.8rem;
		border-radius: 4px;
		cursor: pointer;
		font-size: 0.9rem;
	}
	button:hover {
		background: var(--accent-bg-hi);
	}
	.btn-secondary {
		background: var(--border);
		border-color: var(--text-3);
		color: var(--text-2);
	}
	.move-row {
		display: flex;
		align-items: center;
		gap: 0.3rem;
		font-size: 0.7rem;
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.move-row select {
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--text-1);
		padding: 0.25rem 0.4rem;
		font-family: monospace;
		font-size: 0.78rem;
	}
	.btn-complete {
		color: var(--success);
		border-color: color-mix(in srgb, var(--success) 50%, transparent);
		background: color-mix(in srgb, var(--success) 14%, transparent);
	}
	.btn-complete:hover {
		background: color-mix(in srgb, var(--success) 24%, transparent);
	}
	.actions-menu {
		position: relative;
		display: inline-block;
	}
	.actions-toggle .caret {
		margin-left: 0.25rem;
		font-size: 0.75em;
		opacity: 0.75;
	}
	/* Invisible full-viewport layer so a click anywhere outside the menu
	   closes it. Sits behind the popover but above the rest of the page. */
	.actions-backdrop {
		position: fixed;
		inset: 0;
		z-index: 40;
		background: transparent;
	}
	.actions-menu-list {
		position: absolute;
		top: calc(100% + 4px);
		right: 0;
		z-index: 50;
		min-width: 11rem;
		display: flex;
		flex-direction: column;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 6px;
		box-shadow: 0 6px 18px rgba(0, 0, 0, 0.35);
		padding: 0.25rem;
	}
	.actions-menu-item {
		text-align: left;
		background: transparent;
		border: none;
		border-radius: 4px;
		padding: 0.4rem 0.6rem;
		color: var(--text-1);
		font-size: 0.85rem;
		cursor: pointer;
	}
	.actions-menu-item:hover {
		background: var(--accent-bg);
	}
	.actions-menu-item.complete {
		color: var(--success);
	}
	.actions-menu-item.complete:hover {
		background: color-mix(in srgb, var(--success) 18%, transparent);
	}
	.btn-sm {
		padding: 0.25rem 0.6rem;
		font-size: 0.8rem;
	}
	.btn-xs {
		padding: 0.15rem 0.5rem;
		font-size: 0.75rem;
	}
	.layout {
		display: grid;
		grid-template-columns: 280px 1fr;
		gap: 1rem;
		align-items: start;
	}
	.plan-list {
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 0.4rem;
		background: var(--bg-1);
	}
	.sidebar-controls {
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		padding: 0.3rem 0.2rem 0.6rem;
		border-bottom: 1px solid var(--bg-hover);
		margin-bottom: 0.4rem;
	}
	.seg-group {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: 4px;
		overflow: hidden;
		align-self: stretch;
	}
	.seg {
		flex: 1;
		background: var(--bg-0);
		border: 0;
		color: var(--text-2);
		padding: 0.3rem 0.6rem;
		font-size: 0.78rem;
		font-family: monospace;
		cursor: pointer;
	}
	.seg:not(:last-child) {
		border-right: 1px solid var(--border);
	}
	.seg.active {
		background: var(--accent-bg);
		color: var(--accent);
	}
	.filter-input {
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--text-1);
		padding: 0.35rem 0.55rem;
		font-family: monospace;
		font-size: 0.8rem;
	}
	.sort-row {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		font-size: 0.7rem;
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.sort-row select {
		flex: 1;
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--text-1);
		padding: 0.3rem 0.45rem;
		font-family: monospace;
		font-size: 0.78rem;
	}
	.task-sort-row {
		display: flex;
		flex-direction: column;
		gap: 0.2rem;
		font-size: 0.65rem;
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.task-sort-row select {
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--text-1);
		padding: 0.3rem 0.5rem;
		font-family: monospace;
		font-size: 0.78rem;
	}
	.status-group {
		margin-bottom: 0.4rem;
	}
	.status-header {
		width: 100%;
		display: flex;
		align-items: center;
		gap: 0.4rem;
		background: transparent;
		border: 0;
		padding: 0.4rem 0.45rem;
		color: var(--text-2);
		cursor: pointer;
		font-family: monospace;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		text-align: left;
	}
	.status-header:hover {
		background: var(--bg-1);
		border-radius: 4px;
	}
	.caret {
		display: inline-block;
		width: 0.8rem;
		color: var(--text-3);
	}
	.status-label {
		flex: 1;
	}
	.status-count {
		color: var(--text-3);
		font-size: 0.7rem;
	}
	.show-more-btn {
		display: block;
		width: 100%;
		text-align: center;
		background: transparent;
		border: 1px dashed var(--border);
		color: var(--text-3);
		padding: 0.3rem 0.5rem;
		margin-top: 0.2rem;
		border-radius: 4px;
		font-family: monospace;
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		cursor: pointer;
	}
	.show-more-btn:hover {
		background: var(--bg-hover);
		color: var(--text-1);
	}
	.plan-row {
		width: 100%;
		text-align: left;
		background: transparent;
		border: 0;
		padding: 0.5rem;
		border-radius: 4px;
		color: var(--text-1);
		cursor: pointer;
		display: block;
	}
	.plan-row:hover {
		background: var(--bg-hover);
	}
	.plan-row.selected {
		background: var(--accent-bg);
	}
	.paginator {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.5rem;
		padding: 0.5rem 0.4rem 0.2rem;
		border-top: 1px solid var(--bg-hover);
		margin-top: 0.4rem;
	}
	.page-btn {
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-2);
		padding: 0.25rem 0.55rem;
		border-radius: 4px;
		font-size: 0.75rem;
		font-family: monospace;
		cursor: pointer;
	}
	.page-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}
	.page-btn:not(:disabled):hover {
		background: var(--bg-hover);
		color: var(--text-0);
	}
	.page-info {
		font-size: 0.72rem;
		font-family: monospace;
		color: var(--text-2);
		text-align: center;
	}
	.page-total {
		color: var(--text-3);
	}
	.page-info-static {
		text-align: center;
		font-size: 0.72rem;
		font-family: monospace;
		margin: 0.4rem 0 0;
	}
	.plan-name {
		font-family: monospace;
		font-weight: 600;
	}
	.plan-meta {
		display: flex;
		gap: 0.6rem;
		font-size: 0.75rem;
		color: var(--text-2);
		margin-top: 0.2rem;
	}
	.plan-status {
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.plan-status-active {
		color: var(--success);
	}
	.plan-status-in_progress {
		color: var(--warning);
	}
	.plan-status-completed {
		color: var(--accent);
	}
	.plan-status-archived {
		color: var(--text-2);
	}
	.plan-detail {
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 1rem;
		background: var(--bg-1);
	}
	.plan-header {
		display: flex;
		justify-content: space-between;
		align-items: flex-start;
		gap: 1rem;
		margin-bottom: 1rem;
	}
	.plan-header h3 {
		margin: 0;
		font-family: monospace;
	}
	.plan-desc {
		color: var(--text-2);
		margin: 0.25rem 0 0 0;
		font-size: 0.9rem;
	}
	.plan-actions {
		display: flex;
		gap: 0.4rem;
	}
	.task-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
	}
	.task-item {
		display: flex;
		flex-direction: column;
	}
	.task-row {
		display: grid;
		grid-template-columns: 1.1rem 2.2rem 4rem 2.4rem 1fr auto auto;
		gap: 0.4rem;
		align-items: center;
		padding: 0.45rem 0.6rem;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 4px;
		font-family: monospace;
	}
	.task-caret {
		color: var(--text-3);
		font-size: 0.85rem;
		text-align: center;
	}
	.task-row.expanded {
		border-bottom-left-radius: 0;
		border-bottom-right-radius: 0;
		border-bottom-color: transparent;
	}
	.task-details {
		background: var(--bg-2, var(--bg-0));
		border: 1px solid var(--border);
		border-top: 0;
		border-radius: 0 0 4px 4px;
		padding: 0.75rem 1rem;
	}
	.task-details .raw-json {
		margin-top: 0.75rem;
		border-top: 1px solid var(--border);
		padding-top: 0.5rem;
	}
	.task-row.task-done {
		color: var(--success);
	}
	.task-row.task-progress {
		border-left: 3px solid var(--accent);
	}
	.task-clickable {
		cursor: pointer;
		transition: background 0.1s, border-color 0.1s;
	}
	.task-clickable:hover {
		background: var(--bg-hover);
		border-color: var(--text-3);
	}
	.task-clickable:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: -2px;
	}
	.task-row.task-abandoned {
		color: var(--warning);
		text-decoration: line-through;
	}
	.task-glyph {
		color: var(--text-2);
	}
	.task-id {
		color: var(--text-2);
		font-size: 0.85rem;
	}
	.task-title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.pri-tag {
		font-size: 0.7rem;
		padding: 0.1rem 0.35rem;
		border-radius: 3px;
		background: var(--border);
	}
	.pri-critical {
		background: #5a1515;
		color: var(--danger);
	}
	.pri-high {
		background: #4d3a15;
		color: var(--warning);
	}
	.pri-low {
		background: var(--bg-hover);
		color: var(--text-2);
	}
	.pri-medium {
		background: var(--accent-bg);
		color: var(--accent);
	}
	.assigned {
		color: var(--text-2);
		font-size: 0.8rem;
	}
	.task-actions {
		display: flex;
		gap: 0.25rem;
	}
	.task-proof {
		grid-column: 1 / -1;
		color: var(--text-2);
		font-size: 0.75rem;
		padding-left: 2.5rem;
	}
	.modal-backdrop {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.7);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 100;
	}
	.modal {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1.5rem;
		min-width: 360px;
		max-width: 90vw;
	}
	.modal h3 {
		margin-top: 0;
	}
	.modal .hint {
		color: var(--text-2);
		font-size: 0.85rem;
	}
	.modal label {
		display: block;
		font-size: 0.85rem;
		color: var(--text-2);
		margin-top: 0.7rem;
	}
	.modal input,
	.modal select {
		width: 100%;
		box-sizing: border-box;
		margin-top: 0.2rem;
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.4rem 0.6rem;
		border-radius: 4px;
		font-family: monospace;
	}
	.modal-actions {
		display: flex;
		justify-content: flex-end;
		gap: 0.4rem;
		margin-top: 1rem;
	}

	/* Details modal — read-only "everything we know" view (t-020). */
	.modal-wide {
		min-width: 540px;
		max-width: 720px;
		max-height: 85vh;
		overflow-y: auto;
	}
	.details-header {
		display: flex;
		justify-content: space-between;
		align-items: flex-start;
		gap: 1rem;
		margin-bottom: 0.75rem;
	}
	.details-header h3 {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin: 0;
	}
	.task-id-mono {
		font-family: monospace;
		color: var(--text-2);
	}
	.task-status-pill {
		font-size: 0.7rem;
		padding: 0.1rem 0.45rem;
		border-radius: 3px;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		background: var(--bg-hover);
	}
	.task-status-pending { color: var(--text-2); }
	.task-status-in_progress {
		color: var(--accent);
		background: var(--accent-bg);
	}
	.task-status-done {
		color: var(--success);
		background: color-mix(in srgb, var(--success) 18%, transparent);
	}
	.task-status-abandoned {
		color: var(--warning);
		background: color-mix(in srgb, var(--warning) 18%, transparent);
	}
	.details-title {
		margin: 0.4rem 0 0 0;
		color: var(--text-1);
		font-size: 0.95rem;
	}
	.details-grid {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 0.4rem 1rem;
		margin: 0;
		font-size: 0.85rem;
	}
	.details-grid dt {
		color: var(--text-3);
		text-transform: uppercase;
		font-size: 0.7rem;
		letter-spacing: 0.05em;
		font-family: monospace;
		padding-top: 0.15rem;
	}
	.details-grid dd {
		margin: 0;
		color: var(--text-1);
		font-family: monospace;
		font-size: 0.82rem;
		word-break: break-word;
	}
	.by {
		color: var(--text-3);
		margin-left: 0.4rem;
	}
	.reason {
		color: var(--text-2);
		margin-top: 0.2rem;
		font-style: italic;
	}
	.proof-kind {
		display: inline-block;
		background: var(--accent-bg);
		color: var(--accent);
		padding: 0.05rem 0.4rem;
		border-radius: 3px;
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		margin-right: 0.4rem;
	}
	.proof-value {
		background: var(--bg-0);
		color: var(--success);
		padding: 0.05rem 0.4rem;
		border-radius: 3px;
	}
	.raw-json {
		margin-top: 1rem;
		border-top: 1px solid var(--border);
		padding-top: 0.6rem;
	}
	.raw-json summary {
		cursor: pointer;
		color: var(--text-3);
		font-size: 0.78rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		font-family: monospace;
	}
	.raw-json pre {
		margin-top: 0.5rem;
		padding: 0.6rem 0.8rem;
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 4px;
		font-size: 0.75rem;
		color: var(--text-1);
		white-space: pre-wrap;
		word-break: break-word;
		max-height: 18rem;
		overflow-y: auto;
	}
</style>
