<script lang="ts">
	import { onMount } from 'svelte';
	import { branchStore } from '$lib/branchStore.svelte';
	import {
		listPlans,
		getPlan,
		createPlan,
		addTask,
		startTask,
		completeTask,
		abandonTask,
		archivePlan,
		type Plan,
		type Task,
		type Priority,
		type ProofKind
	} from '$lib/plansApi';

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
	let filter = $state('');

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

	let selectedName: string | null = $state(null);
	let selectedPlan: Plan | null = $state(null);
	let selectedTask: Task | null = $state(null);
	let error: string | null = $state(null);

	// Create-plan form
	let showCreate = $state(false);
	let newPlanName = $state('');
	let newPlanDesc = $state('');

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

	// Details modal — read-only "everything we know about this task"
	// view. Opened from the per-row Details button. Pulls from the
	// already-loaded selectedPlan.tasks (no extra fetch required).
	let detailsOpen = $state(false);
	let detailsTask: Task | null = $state(null);
	function openDetails(task: Task) {
		detailsTask = task;
		detailsOpen = true;
	}
	function closeDetails() {
		detailsOpen = false;
		detailsTask = null;
	}
	// Pretty-print a Task for the "raw JSON" footer of the modal.
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

	$effect(() => {
		void branchStore.current;
		selectedName = null;
		selectedPlan = null;
		selectedTask = null;
		loadPlans();
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
		const q = filter.trim().toLowerCase();
		if (!q) return plans;
		return plans.filter(
			(p) =>
				p.name.toLowerCase().includes(q) ||
				(p.description ?? '').toLowerCase().includes(q)
		);
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
				placeholder="Filter plans…"
				bind:value={filter}
				aria-label="Filter plans"
			/>
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
			<p class="empty">No plans match "{filter}".</p>
		{:else if viewMode === 'tree'}
			{#each groupedPlans as group}
				{@const collapsed = collapsedGroups.has(group.key)}
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
						{#each group.plans as plan}
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
					{/if}
				</div>
			{/each}
		{:else}
			{#each sortedPlans as plan}
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
					{#if selectedPlan.status !== 'archived'}
						<button class="btn-secondary" onclick={handleArchive}>Archive</button>
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
					{#each sortedTasks as task}
						<li class="task-row {statusClass(task.status)}">
							<span class="task-glyph">[{statusGlyph(task.status)}]</span>
							<span class="task-id">{task.id}</span>
							<span class="pri-tag {priorityClass(task.priority)}">
								{task.priority.slice(0, 2).toUpperCase()}
							</span>
							<span class="task-title">{task.title}</span>
							{#if task.assigned_to}
								<span class="assigned">@{task.assigned_to}</span>
							{/if}
							<span class="task-actions">
								<button
									class="btn-xs btn-secondary"
									onclick={() => openDetails(task)}
									title="Show every field for this task"
								>
									Details
								</button>
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
						</li>
					{/each}
				</ul>
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

{#if detailsOpen && detailsTask}
	<div class="modal-backdrop" onclick={closeDetails}>
		<div class="modal modal-wide" onclick={(e) => e.stopPropagation()}>
			<header class="details-header">
				<div>
					<h3>
						<span class="task-id-mono">{detailsTask.id}</span>
						<span class="pri-tag {priorityClass(detailsTask.priority)}">
							{detailsTask.priority.slice(0, 2).toUpperCase()}
						</span>
						<span class="task-status-pill task-status-{detailsTask.status}">
							{detailsTask.status.replace('_', ' ')}
						</span>
					</h3>
					<p class="details-title">{detailsTask.title}</p>
				</div>
				<button type="button" class="btn-xs btn-secondary" onclick={closeDetails}>Close</button>
			</header>

			<dl class="details-grid">
				<dt>Priority</dt><dd>{detailsTask.priority}</dd>
				<dt>Status</dt><dd>{detailsTask.status}</dd>
				<dt>Assigned to</dt><dd>{detailsTask.assigned_to ?? '—'}</dd>
				<dt>Parent</dt><dd>{detailsTask.parent_id ?? '—'}</dd>
				<dt>Blocked by</dt>
				<dd>
					{#if detailsTask.blocked_by.length > 0}
						{detailsTask.blocked_by.join(', ')}
					{:else}
						—
					{/if}
				</dd>

				<dt>Created</dt>
				<dd>
					{formatTs(detailsTask.created_at)}
					{#if detailsTask.created_by}<span class="by">by {detailsTask.created_by}</span>{/if}
				</dd>
				<dt>Started</dt>
				<dd>
					{formatTs(detailsTask.started_at)}
					{#if detailsTask.started_by}<span class="by">by {detailsTask.started_by}</span>{/if}
				</dd>
				<dt>Completed</dt>
				<dd>
					{formatTs(detailsTask.completed_at)}
					{#if detailsTask.completed_by}<span class="by">by {detailsTask.completed_by}</span>{/if}
				</dd>
				<dt>Abandoned</dt>
				<dd>
					{formatTs(detailsTask.abandoned_at)}
					{#if detailsTask.abandoned_reason}
						<div class="reason">{detailsTask.abandoned_reason}</div>
					{/if}
				</dd>

				<dt>Proof</dt>
				<dd>
					{#if detailsTask.proof}
						<span class="proof-kind">{detailsTask.proof.kind}</span>
						<code class="proof-value">{detailsTask.proof.value}</code>
						{#if detailsTask.proof.note}
							<div class="reason">{detailsTask.proof.note}</div>
						{/if}
					{:else}
						—
					{/if}
				</dd>
			</dl>

			<details class="raw-json">
				<summary>Raw JSON</summary>
				<pre>{rawJson(detailsTask)}</pre>
			</details>
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
	.task-row {
		display: grid;
		grid-template-columns: 2.2rem 4rem 2.4rem 1fr auto auto;
		gap: 0.4rem;
		align-items: center;
		padding: 0.45rem 0.6rem;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 4px;
		font-family: monospace;
	}
	.task-row.task-done {
		color: var(--success);
	}
	.task-row.task-progress {
		border-left: 3px solid var(--accent);
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
