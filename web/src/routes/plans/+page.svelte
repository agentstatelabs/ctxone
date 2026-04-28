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
		color: #666;
		font-family: monospace;
		font-weight: normal;
		font-size: 0.9rem;
	}
	.error {
		background: #2a1515;
		border: 1px solid #663030;
		color: #ef9999;
		padding: 0.6rem;
		border-radius: 4px;
	}
	.empty {
		color: #666;
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
		background: #0a0a0a;
		border: 1px solid #333;
		color: #e0e0e0;
		padding: 0.4rem 0.6rem;
		border-radius: 4px;
		flex: 1 1 12rem;
		font-family: monospace;
	}
	button {
		background: #1e3a5f;
		border: 1px solid #2a4a7a;
		color: #93c5fd;
		padding: 0.4rem 0.8rem;
		border-radius: 4px;
		cursor: pointer;
		font-size: 0.9rem;
	}
	button:hover {
		background: #264e80;
	}
	.btn-secondary {
		background: #222;
		border-color: #444;
		color: #aaa;
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
		border: 1px solid #222;
		border-radius: 6px;
		padding: 0.4rem;
		background: #0d0d0d;
	}
	.sidebar-controls {
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		padding: 0.3rem 0.2rem 0.6rem;
		border-bottom: 1px solid #1a1a1a;
		margin-bottom: 0.4rem;
	}
	.seg-group {
		display: inline-flex;
		border: 1px solid #2a2a2a;
		border-radius: 4px;
		overflow: hidden;
		align-self: stretch;
	}
	.seg {
		flex: 1;
		background: #0a0a0a;
		border: 0;
		color: #888;
		padding: 0.3rem 0.6rem;
		font-size: 0.78rem;
		font-family: monospace;
		cursor: pointer;
	}
	.seg:not(:last-child) {
		border-right: 1px solid #2a2a2a;
	}
	.seg.active {
		background: #1e3a5f;
		color: #93c5fd;
	}
	.filter-input {
		background: #0a0a0a;
		border: 1px solid #333;
		border-radius: 4px;
		color: #e0e0e0;
		padding: 0.35rem 0.55rem;
		font-family: monospace;
		font-size: 0.8rem;
	}
	.sort-row {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		font-size: 0.7rem;
		color: #666;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.sort-row select {
		flex: 1;
		background: #0a0a0a;
		border: 1px solid #333;
		border-radius: 4px;
		color: #e0e0e0;
		padding: 0.3rem 0.45rem;
		font-family: monospace;
		font-size: 0.78rem;
	}
	.task-sort-row {
		display: flex;
		flex-direction: column;
		gap: 0.2rem;
		font-size: 0.65rem;
		color: #666;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.task-sort-row select {
		background: #0a0a0a;
		border: 1px solid #333;
		border-radius: 4px;
		color: #e0e0e0;
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
		color: #888;
		cursor: pointer;
		font-family: monospace;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		text-align: left;
	}
	.status-header:hover {
		background: #131313;
		border-radius: 4px;
	}
	.caret {
		display: inline-block;
		width: 0.8rem;
		color: #555;
	}
	.status-label {
		flex: 1;
	}
	.status-count {
		color: #555;
		font-size: 0.7rem;
	}
	.plan-row {
		width: 100%;
		text-align: left;
		background: transparent;
		border: 0;
		padding: 0.5rem;
		border-radius: 4px;
		color: #e0e0e0;
		cursor: pointer;
		display: block;
	}
	.plan-row:hover {
		background: #1a1a1a;
	}
	.plan-row.selected {
		background: #1e3a5f;
	}
	.plan-name {
		font-family: monospace;
		font-weight: 600;
	}
	.plan-meta {
		display: flex;
		gap: 0.6rem;
		font-size: 0.75rem;
		color: #888;
		margin-top: 0.2rem;
	}
	.plan-status {
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.plan-status-active {
		color: #7fd484;
	}
	.plan-status-in_progress {
		color: #fbbf24;
	}
	.plan-status-completed {
		color: #93c5fd;
	}
	.plan-status-archived {
		color: #888;
	}
	.plan-detail {
		border: 1px solid #222;
		border-radius: 6px;
		padding: 1rem;
		background: #0d0d0d;
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
		color: #aaa;
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
		background: #131313;
		border: 1px solid #1f1f1f;
		border-radius: 4px;
		font-family: monospace;
	}
	.task-row.task-done {
		color: #7fd484;
	}
	.task-row.task-progress {
		border-left: 3px solid #3b82f6;
	}
	.task-row.task-abandoned {
		color: #b08040;
		text-decoration: line-through;
	}
	.task-glyph {
		color: #888;
	}
	.task-id {
		color: #888;
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
		background: #1f1f1f;
	}
	.pri-critical {
		background: #5a1515;
		color: #ff8888;
	}
	.pri-high {
		background: #4d3a15;
		color: #f0b060;
	}
	.pri-low {
		background: #1a1a1a;
		color: #888;
	}
	.pri-medium {
		background: #15304d;
		color: #93c5fd;
	}
	.assigned {
		color: #b0a8e6;
		font-size: 0.8rem;
	}
	.task-actions {
		display: flex;
		gap: 0.25rem;
	}
	.task-proof {
		grid-column: 1 / -1;
		color: #888;
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
		background: #0d0d0d;
		border: 1px solid #333;
		border-radius: 8px;
		padding: 1.5rem;
		min-width: 360px;
		max-width: 90vw;
	}
	.modal h3 {
		margin-top: 0;
	}
	.modal .hint {
		color: #888;
		font-size: 0.85rem;
	}
	.modal label {
		display: block;
		font-size: 0.85rem;
		color: #aaa;
		margin-top: 0.7rem;
	}
	.modal input,
	.modal select {
		width: 100%;
		box-sizing: border-box;
		margin-top: 0.2rem;
		background: #0a0a0a;
		border: 1px solid #333;
		color: #e0e0e0;
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
</style>
