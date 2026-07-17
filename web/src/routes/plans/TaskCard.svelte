<script lang="ts">
	import type { Task } from '$lib/plansApi';
	import type { TaskGraph } from './model';
	import {
		PROOF_GLYPH,
		STATUS_META,
		canAbandon,
		canComplete,
		canStart,
		openBlockers,
		subtaskProgress
	} from './model';
	import PriorityChip from './PriorityChip.svelte';
	import AssigneeChip from './AssigneeChip.svelte';

	let {
		task,
		graph,
		onOpen,
		onStart,
		onRequestComplete,
		onRequestAbandon,
		onDragStart,
		onDragEnd
	}: {
		task: Task;
		graph: TaskGraph;
		onOpen: (t: Task) => void;
		onStart: (t: Task) => void;
		onRequestComplete: (t: Task) => void;
		onRequestAbandon: (t: Task) => void;
		onDragStart?: (t: Task, e: DragEvent) => void;
		onDragEnd?: () => void;
	} = $props();

	let blockers = $derived(openBlockers(task, graph));
	let progress = $derived(subtaskProgress(task, graph));
	let meta = $derived(STATUS_META[task.status]);

	function blockersTooltip(): string {
		return (
			'Blocked by: ' + blockers.map((b) => `${b.id} ${b.title}`).join(', ')
		);
	}

	function handleDragStart(e: DragEvent) {
		if (e.dataTransfer) {
			e.dataTransfer.effectAllowed = 'move';
			e.dataTransfer.setData('text/plain', task.id);
		}
		onDragStart?.(task, e);
	}

	function key(e: KeyboardEvent) {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			onOpen(task);
		}
	}
</script>

<div
	class="card"
	class:done={task.status === 'done'}
	class:abandoned={task.status === 'abandoned'}
	role="button"
	tabindex="0"
	draggable="true"
	ondragstart={handleDragStart}
	ondragend={() => onDragEnd?.()}
	onclick={() => onOpen(task)}
	onkeydown={key}
	style:--card-accent={meta.color}
>
	<div class="card-top">
		<span class="task-id">{task.id}</span>
		{#if blockers.length > 0}
			<span class="blocked" title={blockersTooltip()}>⛓ {blockers.length}</span>
		{/if}
		{#if task.proof}
			<span class="proof" title="Proof ({task.proof.kind}): {task.proof.value}">
				✓ {PROOF_GLYPH[task.proof.kind]} {task.proof.kind}
			</span>
		{/if}
	</div>
	<div class="title" class:struck={task.status === 'abandoned'}>{task.title}</div>
	<div class="card-bottom">
		<PriorityChip priority={task.priority} compact />
		{#if progress}
			<span
				class="subtasks"
				class:complete={progress.done === progress.total}
				title="{progress.done} of {progress.total} subtasks done"
			>▣ {progress.done}/{progress.total}</span>
		{/if}
		<span class="spacer"></span>
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<!-- svelte-ignore a11y_click_events_have_key_events -->
		<span class="quick" onclick={(e) => e.stopPropagation()}>
			{#if canStart(task)}
				<button type="button" class="qbtn" onclick={() => onStart(task)}>Start</button>
			{/if}
			{#if canComplete(task)}
				<button type="button" class="qbtn ok" onclick={() => onRequestComplete(task)}>Done</button>
			{/if}
			{#if canAbandon(task)}
				<button type="button" class="qbtn warn" onclick={() => onRequestAbandon(task)} title="Abandon">✕</button>
			{/if}
		</span>
		{#if task.assigned_to}
			<AssigneeChip assignee={task.assigned_to} />
		{/if}
	</div>
</div>

<style>
	.card {
		background: var(--lens-surface);
		border: 1px solid var(--lens-border);
		border-left: 2px solid var(--card-accent);
		border-radius: var(--lens-radius-sm);
		padding: 0.5rem 0.6rem;
		cursor: pointer;
		display: flex;
		flex-direction: column;
		gap: 0.35rem;
		transition: border-color var(--lens-dur-fast) var(--lens-ease),
			background var(--lens-dur-fast) var(--lens-ease);
		user-select: none;
	}
	.card:hover {
		border-color: var(--lens-border-strong);
		border-left-color: var(--card-accent);
		background: var(--lens-surface-raised);
	}
	.card.done {
		opacity: 0.8;
	}
	.card.abandoned {
		opacity: 0.65;
	}
	.card-top {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
	}
	.task-id {
		color: var(--lens-muted);
	}
	.blocked {
		color: var(--lens-warn);
		cursor: help;
	}
	.proof {
		color: var(--lens-ok);
		margin-left: auto;
		white-space: nowrap;
	}
	.title {
		font-size: var(--lens-font-size-sm);
		color: var(--lens-text);
		line-height: var(--lens-leading-tight);
		overflow-wrap: anywhere;
	}
	.title.struck {
		text-decoration: line-through;
		color: var(--lens-text-secondary);
	}
	.card-bottom {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		min-height: 1.25rem;
	}
	.subtasks {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-text-secondary);
	}
	.subtasks.complete {
		color: var(--lens-ok);
	}
	.spacer {
		flex: 1;
	}
	.quick {
		display: none;
		gap: 0.25rem;
	}
	.card:hover .quick,
	.card:focus-within .quick {
		display: inline-flex;
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
</style>
