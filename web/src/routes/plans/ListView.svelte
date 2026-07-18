<script lang="ts">
	import type { Priority, Task, TaskStatus } from '$lib/plansApi';
	import type { TaskGraph } from './model';
	import {
		PRIORITY_META,
		PRIORITY_ORDER,
		PROOF_GLYPH,
		STATUS_META,
		TASK_COLUMNS,
		agoShort,
		canAbandon,
		canComplete,
		canStart,
		openBlockers,
		subtaskProgress,
		taskActivityTs
	} from './model';
	import PriorityChip from './PriorityChip.svelte';
	import AssigneeChip from './AssigneeChip.svelte';

	let {
		tasks,
		graph,
		onOpen,
		onStart,
		onRequestComplete,
		onRequestAbandon
	}: {
		tasks: Task[];
		graph: TaskGraph;
		onOpen: (t: Task) => void;
		onStart: (t: Task) => void;
		onRequestComplete: (t: Task) => void;
		onRequestAbandon: (t: Task) => void;
	} = $props();

	/* Group-by selector. 'parent' is the tree mode (subtasks nested
	   under their parent); 'none' is the flat mode — together they carry
	   the old page's tree/flat capability forward. */
	type GroupBy = 'status' | 'priority' | 'assignee' | 'parent' | 'none';
	const GROUP_KEY = 'lens.plans.list.group';
	const GROUPS: GroupBy[] = ['status', 'priority', 'assignee', 'parent', 'none'];
	const GROUP_LABELS: Record<GroupBy, string> = {
		status: 'Status',
		priority: 'Priority',
		assignee: 'Assignee',
		parent: 'Parent (tree)',
		none: 'None (flat)'
	};
	function loadGroup(): GroupBy {
		if (typeof localStorage === 'undefined') return 'status';
		const v = localStorage.getItem(GROUP_KEY) as GroupBy | null;
		return v && GROUPS.includes(v) ? v : 'status';
	}
	let groupBy: GroupBy = $state(loadGroup());
	$effect(() => {
		if (typeof localStorage !== 'undefined') localStorage.setItem(GROUP_KEY, groupBy);
	});

	/* Sortable columns. */
	type SortCol = 'priority' | 'updated' | 'title';
	const SORT_KEY = 'lens.plans.list.sort';
	function loadSort(): { col: SortCol; dir: 1 | -1 } {
		if (typeof localStorage === 'undefined') return { col: 'priority', dir: 1 };
		try {
			const raw = localStorage.getItem(SORT_KEY);
			if (!raw) return { col: 'priority', dir: 1 };
			const v = JSON.parse(raw);
			if (['priority', 'updated', 'title'].includes(v.col) && (v.dir === 1 || v.dir === -1))
				return v;
		} catch {
			/* fall through */
		}
		return { col: 'priority', dir: 1 };
	}
	let sort = $state(loadSort());
	$effect(() => {
		if (typeof localStorage !== 'undefined')
			localStorage.setItem(SORT_KEY, JSON.stringify(sort));
	});
	function toggleSort(col: SortCol) {
		if (sort.col === col) sort = { col, dir: sort.dir === 1 ? -1 : 1 };
		else sort = { col, dir: 1 };
	}
	function sortGlyph(col: SortCol): string {
		if (sort.col !== col) return '';
		return sort.dir === 1 ? '▲' : '▼';
	}

	function compare(a: Task, b: Task): number {
		let n = 0;
		switch (sort.col) {
			case 'priority':
				n = (PRIORITY_META[a.priority]?.rank ?? 99) - (PRIORITY_META[b.priority]?.rank ?? 99);
				break;
			case 'updated':
				// dir 1 = newest first for "updated" (the natural read).
				n = taskActivityTs(b) - taskActivityTs(a);
				break;
			case 'title':
				n = a.title.localeCompare(b.title);
				break;
		}
		if (n === 0) n = a.id.localeCompare(b.id);
		return n * sort.dir;
	}

	interface Row {
		task: Task;
		depth: number;
	}
	interface Group {
		key: string;
		label: string;
		color: string | null;
		rows: Row[];
	}

	/** Nested parent → children rows (tree mode), sorted per sibling set. */
	function treeRows(list: Task[]): Row[] {
		const inSet = new Set(list.map((t) => t.id));
		const roots = list.filter((t) => !t.parent_id || !inSet.has(t.parent_id)).sort(compare);
		const out: Row[] = [];
		const visit = (t: Task, depth: number, seen: Set<string>) => {
			if (seen.has(t.id)) return; // defensive: malformed parent cycles
			seen.add(t.id);
			out.push({ task: t, depth });
			const kids = (graph.children.get(t.id) ?? [])
				.filter((k) => inSet.has(k.id))
				.sort(compare);
			for (const k of kids) visit(k, depth + 1, seen);
		};
		const seen = new Set<string>();
		for (const r of roots) visit(r, 0, seen);
		return out;
	}

	let groups: Group[] = $derived.by(() => {
		const list = tasks;
		if (groupBy === 'none') {
			return [{ key: 'all', label: '', color: null, rows: [...list].sort(compare).map((task) => ({ task, depth: 0 })) }];
		}
		if (groupBy === 'parent') {
			return [{ key: 'tree', label: '', color: null, rows: treeRows(list) }];
		}
		if (groupBy === 'status') {
			return TASK_COLUMNS.map((s: TaskStatus) => ({
				key: s,
				label: STATUS_META[s].label,
				color: STATUS_META[s].color,
				rows: list.filter((t) => t.status === s).sort(compare).map((task) => ({ task, depth: 0 }))
			})).filter((g) => g.rows.length > 0);
		}
		if (groupBy === 'priority') {
			return PRIORITY_ORDER.map((p: Priority) => ({
				key: p,
				label: PRIORITY_META[p].label,
				color: PRIORITY_META[p].color,
				rows: list.filter((t) => t.priority === p).sort(compare).map((task) => ({ task, depth: 0 }))
			})).filter((g) => g.rows.length > 0);
		}
		// assignee
		const names = [...new Set(list.map((t) => t.assigned_to ?? ''))].sort((a, b) => {
			if (a === '') return 1; // Unassigned last
			if (b === '') return -1;
			return a.localeCompare(b);
		});
		return names
			.map((n) => ({
				key: n || '∅',
				label: n || 'Unassigned',
				color: null,
				rows: list
					.filter((t) => (t.assigned_to ?? '') === n)
					.sort(compare)
					.map((task) => ({ task, depth: 0 }))
			}))
			.filter((g) => g.rows.length > 0);
	});

	let collapsed: Set<string> = $state(new Set());
	function toggleGroup(key: string) {
		const next = new Set(collapsed);
		if (next.has(key)) next.delete(key);
		else next.add(key);
		collapsed = next;
	}

	function rowKey(e: KeyboardEvent, t: Task) {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			onOpen(t);
		}
	}
</script>

<div class="list-view">
	<div class="list-controls">
		<label class="ctl">
			<span>Group by</span>
			<select bind:value={groupBy} aria-label="Group tasks by">
				{#each GROUPS as g (g)}
					<option value={g}>{GROUP_LABELS[g]}</option>
				{/each}
			</select>
		</label>
		<span class="count">{tasks.length} task{tasks.length === 1 ? '' : 's'}</span>
	</div>

	<div class="table" role="table" aria-label="Tasks">
		<div class="thead" role="row">
			<button type="button" class="th th-title" onclick={() => toggleSort('title')}>
				Task <span class="dir">{sortGlyph('title')}</span>
			</button>
			<button type="button" class="th" onclick={() => toggleSort('priority')}>
				Priority <span class="dir">{sortGlyph('priority')}</span>
			</button>
			<span class="th th-static">Assignee</span>
			<button type="button" class="th" onclick={() => toggleSort('updated')}>
				Updated <span class="dir">{sortGlyph('updated')}</span>
			</button>
			<span class="th th-static th-status">Status</span>
			<span class="th th-static th-actions"></span>
		</div>

		{#each groups as group (group.key)}
			{#if group.label}
				<button
					type="button"
					class="group-header"
					onclick={() => toggleGroup(group.key)}
					aria-expanded={!collapsed.has(group.key)}
				>
					<span class="caret">{collapsed.has(group.key) ? '▸' : '▾'}</span>
					<span class="group-label" style:color={group.color ?? 'var(--lens-text-secondary)'}>
						{group.label}
					</span>
					<span class="group-count">{group.rows.length}</span>
				</button>
			{/if}
			{#if !collapsed.has(group.key)}
				{#each group.rows as row (row.task.id)}
					{@const t = row.task}
					{@const blockers = openBlockers(t, graph)}
					{@const progress = subtaskProgress(t, graph)}
					<div
						class="row"
						role="row"
						tabindex="0"
						onclick={() => onOpen(t)}
						onkeydown={(e) => rowKey(e, t)}
					>
						<span class="cell cell-title" style:padding-left="{0.5 + row.depth * 1.1}rem">
							{#if row.depth > 0}<span class="tree-tick">└</span>{/if}
							<span class="tid">{t.id}</span>
							<span class="ttl" class:struck={t.status === 'abandoned'}>{t.title}</span>
							{#if blockers.length > 0}
								<span
									class="blocked"
									title={'Blocked by: ' + blockers.map((b) => `${b.id} ${b.title}`).join(', ')}
								>⛓ {blockers.length}</span>
							{/if}
							{#if progress}
								<span class="subtasks" title="{progress.done} of {progress.total} subtasks done">
									▣ {progress.done}/{progress.total}
								</span>
							{/if}
							{#if t.proof}
								<span class="proof" title="Proof ({t.proof.kind}): {t.proof.value}">
									✓ {PROOF_GLYPH[t.proof.kind]}
								</span>
							{/if}
						</span>
						<span class="cell"><PriorityChip priority={t.priority} compact /></span>
						<span class="cell cell-assignee">
							{#if t.assigned_to}<AssigneeChip assignee={t.assigned_to} showName />{:else}<span class="dash">—</span>{/if}
						</span>
						<span class="cell cell-updated" title={new Date(taskActivityTs(t)).toLocaleString()}>
							{agoShort(taskActivityTs(t))}
						</span>
						<span class="cell">
							<span
								class="status-pill"
								style:color={STATUS_META[t.status].color}
								style:background={STATUS_META[t.status].tint}
								style:border-color={STATUS_META[t.status].border}
							>{STATUS_META[t.status].label}</span>
						</span>
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<!-- svelte-ignore a11y_click_events_have_key_events -->
						<span class="cell cell-actions" onclick={(e) => e.stopPropagation()}>
							{#if canStart(t)}
								<button type="button" class="qbtn" onclick={() => onStart(t)}>Start</button>
							{/if}
							{#if canComplete(t)}
								<button type="button" class="qbtn ok" onclick={() => onRequestComplete(t)}>Done</button>
							{/if}
							{#if canAbandon(t)}
								<button type="button" class="qbtn warn" title="Abandon" onclick={() => onRequestAbandon(t)}>✕</button>
							{/if}
						</span>
					</div>
				{/each}
			{/if}
		{/each}
		{#if tasks.length === 0}
			<p class="empty">No tasks match.</p>
		{/if}
	</div>
</div>

<style>
	.list-view {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-2);
	}
	.list-controls {
		display: flex;
		align-items: center;
		gap: var(--lens-space-3);
	}
	.ctl {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}
	.ctl select {
		background: var(--lens-surface);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: 0.25rem 0.4rem;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
	}
	.count {
		margin-left: auto;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
	}
	.table {
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-md);
		background: var(--lens-surface);
		overflow: hidden;
	}
	.thead,
	.row {
		display: grid;
		grid-template-columns: minmax(16rem, 1fr) 5.2rem 9rem 4.2rem 6.6rem 8.2rem;
		align-items: center;
		column-gap: var(--lens-space-2);
	}
	.thead {
		border-bottom: 1px solid var(--lens-border);
		background: color-mix(in srgb, var(--lens-surface-raised) 55%, transparent);
	}
	.th {
		background: transparent;
		border: none;
		text-align: left;
		padding: 0.4rem 0.5rem;
		font-size: var(--lens-font-size-2xs);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
		cursor: pointer;
		white-space: nowrap;
	}
	.th:hover:not(.th-static) {
		color: var(--lens-text);
	}
	.th-static {
		cursor: default;
	}
	.dir {
		color: var(--lens-accent);
		font-size: 0.55rem;
	}
	.group-header {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		width: 100%;
		background: color-mix(in srgb, var(--lens-surface-raised) 40%, transparent);
		border: none;
		border-top: 1px solid var(--lens-border-subtle);
		padding: 0.3rem 0.5rem;
		cursor: pointer;
		text-align: left;
	}
	.caret {
		color: var(--lens-muted);
		font-size: var(--lens-font-size-2xs);
		width: 0.8rem;
	}
	.group-label {
		font-size: var(--lens-font-size-2xs);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
	}
	.group-count {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
	}
	.row {
		border-top: 1px solid var(--lens-border-subtle);
		cursor: pointer;
		transition: background var(--lens-dur-fast) var(--lens-ease);
	}
	.row:hover {
		background: var(--lens-surface-raised);
	}
	.row:focus-visible {
		outline: 2px solid var(--lens-focus);
		outline-offset: -2px;
	}
	.cell {
		padding: 0.35rem 0.5rem;
		font-size: var(--lens-font-size-sm);
		min-width: 0;
	}
	.cell-title {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		overflow: hidden;
	}
	.tree-tick {
		color: var(--lens-text-faint);
		font-family: var(--lens-font-mono);
	}
	.tid {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
		flex: none;
	}
	.ttl {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--lens-text);
	}
	.ttl.struck {
		text-decoration: line-through;
		color: var(--lens-text-secondary);
	}
	.blocked {
		color: var(--lens-warn);
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		cursor: help;
		flex: none;
	}
	.subtasks {
		color: var(--lens-text-secondary);
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		flex: none;
	}
	.proof {
		color: var(--lens-ok);
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		flex: none;
	}
	.cell-assignee {
		overflow: hidden;
	}
	.dash {
		color: var(--lens-text-faint);
	}
	.cell-updated {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-secondary);
	}
	.status-pill {
		display: inline-block;
		font-size: var(--lens-font-size-2xs);
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-wide);
		padding: 0.05rem 0.45rem;
		border: 1px solid transparent;
		border-radius: var(--lens-radius-full);
		white-space: nowrap;
	}
	.cell-actions {
		display: flex;
		gap: 0.25rem;
		justify-content: flex-end;
		visibility: hidden;
	}
	.row:hover .cell-actions,
	.row:focus-within .cell-actions {
		visibility: visible;
	}
	.qbtn {
		background: var(--lens-surface-raised);
		border: 1px solid var(--lens-border);
		color: var(--lens-text-secondary);
		font-size: var(--lens-font-size-2xs);
		padding: 0.05rem 0.4rem;
		border-radius: var(--lens-radius-sm);
		cursor: pointer;
	}
	.qbtn:hover {
		border-color: var(--lens-border-strong);
		color: var(--lens-text);
	}
	.qbtn.ok:hover {
		color: var(--lens-ok);
		border-color: var(--lens-ok-border);
	}
	.qbtn.warn:hover {
		color: var(--lens-warn);
		border-color: var(--lens-warn-border);
	}
	.empty {
		color: var(--lens-text-faint);
		font-style: italic;
		text-align: center;
		padding: var(--lens-space-6);
		margin: 0;
	}
</style>
