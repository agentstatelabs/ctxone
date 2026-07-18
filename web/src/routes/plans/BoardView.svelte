<script lang="ts">
	import type { Task, TaskStatus } from '$lib/plansApi';
	import type { TaskGraph } from './model';
	import {
		STATUS_META,
		TASK_COLUMNS,
		compareBoardOrder,
		dropAction
	} from './model';
	import TaskCard from './TaskCard.svelte';

	let {
		tasks,
		graph,
		onOpen,
		onStart,
		onRequestComplete,
		onRequestAbandon,
		onIllegal
	}: {
		tasks: Task[];
		graph: TaskGraph;
		onOpen: (t: Task) => void;
		onStart: (t: Task) => void;
		onRequestComplete: (t: Task) => void;
		onRequestAbandon: (t: Task) => void;
		/** Called with an explanation when a drag lands on an illegal column. */
		onIllegal: (reason: string) => void;
	} = $props();

	let columns = $derived.by(() => {
		const buckets: Record<TaskStatus, Task[]> = {
			pending: [],
			in_progress: [],
			done: [],
			abandoned: []
		};
		for (const t of tasks) buckets[t.status]?.push(t);
		for (const s of TASK_COLUMNS) buckets[s].sort(compareBoardOrder);
		return TASK_COLUMNS.map((s) => ({ status: s, meta: STATUS_META[s], tasks: buckets[s] }));
	});

	// HTML5 DnD: dataTransfer payloads aren't readable during dragover, so
	// the dragged task rides in component state for live legality hints.
	let dragging: Task | null = $state(null);
	let overColumn: TaskStatus | null = $state(null);

	function legality(to: TaskStatus): 'legal' | 'illegal' | null {
		if (!dragging) return null;
		const a = dropAction(dragging.status, to);
		if (a.kind === 'noop') return null;
		return a.kind === 'illegal' ? 'illegal' : 'legal';
	}

	function handleDragOver(e: DragEvent, to: TaskStatus) {
		// Always accept the drop so we control the outcome (and can toast
		// on illegal transitions instead of relying on the browser's
		// silent snap-back).
		e.preventDefault();
		if (e.dataTransfer) {
			e.dataTransfer.dropEffect = legality(to) === 'illegal' ? 'none' : 'move';
		}
		overColumn = to;
	}

	function handleDrop(e: DragEvent, to: TaskStatus) {
		e.preventDefault();
		const t = dragging;
		clearDrag();
		if (!t) return;
		const action = dropAction(t.status, to);
		switch (action.kind) {
			case 'start':
				onStart(t);
				break;
			case 'complete':
				onRequestComplete(t);
				break;
			case 'abandon':
				onRequestAbandon(t);
				break;
			case 'illegal':
				onIllegal(action.reason);
				break;
			case 'noop':
				break;
		}
	}

	function clearDrag() {
		dragging = null;
		overColumn = null;
	}
</script>

<div class="board" role="list" aria-label="Task board">
	{#each columns as col (col.status)}
		{@const hint = overColumn === col.status ? legality(col.status) : null}
		<section
			class="column"
			class:drop-legal={hint === 'legal'}
			class:drop-illegal={hint === 'illegal'}
			role="listitem"
			aria-label="{col.meta.label} column"
			ondragover={(e) => handleDragOver(e, col.status)}
			ondragleave={() => {
				if (overColumn === col.status) overColumn = null;
			}}
			ondrop={(e) => handleDrop(e, col.status)}
		>
			<header class="col-header" style:--col-accent={col.meta.color}>
				<span class="col-dot"></span>
				<span class="col-label">{col.meta.label}</span>
				<span class="col-count">{col.tasks.length}</span>
			</header>
			<div class="col-body">
				{#each col.tasks as task (task.id)}
					<TaskCard
						{task}
						{graph}
						{onOpen}
						{onStart}
						{onRequestComplete}
						{onRequestAbandon}
						onDragStart={(t) => (dragging = t)}
						onDragEnd={clearDrag}
					/>
				{/each}
				{#if col.tasks.length === 0}
					<p class="col-empty">No tasks</p>
				{/if}
			</div>
		</section>
	{/each}
</div>

<style>
	.board {
		display: grid;
		grid-template-columns: repeat(4, minmax(220px, 1fr));
		gap: var(--lens-space-3);
		align-items: start;
		overflow-x: auto;
	}
	.column {
		background: color-mix(in srgb, var(--lens-surface) 55%, transparent);
		border: 1px solid var(--lens-border-subtle);
		border-radius: var(--lens-radius-md);
		padding: var(--lens-space-2);
		min-height: 12rem;
		transition: border-color var(--lens-dur-fast) var(--lens-ease),
			background var(--lens-dur-fast) var(--lens-ease);
	}
	.column.drop-legal {
		border-color: var(--lens-accent-border);
		background: var(--lens-accent-tint);
	}
	.column.drop-illegal {
		border-color: var(--lens-danger-border);
		background: var(--lens-danger-tint);
	}
	.col-header {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		padding: 0.15rem 0.25rem 0.5rem;
	}
	.col-dot {
		width: 0.5rem;
		height: 0.5rem;
		border-radius: var(--lens-radius-full);
		background: var(--col-accent);
		flex: none;
	}
	.col-label {
		font-size: var(--lens-font-size-2xs);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-text-secondary);
	}
	.col-count {
		margin-left: auto;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
	}
	.col-body {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-2);
	}
	.col-empty {
		color: var(--lens-text-faint);
		font-size: var(--lens-font-size-xs);
		font-style: italic;
		text-align: center;
		margin: var(--lens-space-4) 0;
	}
</style>
