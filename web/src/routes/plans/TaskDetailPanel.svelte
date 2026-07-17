<script lang="ts">
	import type { Proof, ProofKind, Task } from '$lib/plansApi';
	import type { TaskGraph } from './model';
	import {
		PROOF_GLYPH,
		STATUS_META,
		canAbandon,
		canComplete,
		canStart,
		formatTs,
		isOpen
	} from './model';
	import PriorityChip from './PriorityChip.svelte';
	import AssigneeChip from './AssigneeChip.svelte';
	import ConfirmButton from './ConfirmButton.svelte';

	let {
		task,
		graph,
		planName,
		branch,
		intent = null,
		onClose,
		onNavigate,
		onStart,
		onComplete,
		onAbandon
	}: {
		task: Task;
		graph: TaskGraph;
		planName: string;
		branch: string;
		/** Pre-open a proof/reason form (drag-to-Done routes through here). */
		intent?: 'complete' | 'abandon' | null;
		onClose: () => void;
		onNavigate: (taskId: string) => void;
		onStart: (t: Task) => void;
		onComplete: (t: Task, proof: Proof) => Promise<boolean>;
		onAbandon: (t: Task, reason: string) => Promise<boolean>;
	} = $props();

	type Mode = 'view' | 'complete' | 'abandon';
	let mode: Mode = $state('view');

	// Proof form
	let proofKind: ProofKind = $state('commit');
	let proofValue = $state('');
	let proofNote = $state('');
	// Abandon form
	let reason = $state('');
	let submitting = $state(false);

	// Re-sync when the panel is retargeted (new task or an explicit
	// intent from a board drag / quick action).
	$effect(() => {
		void task.id;
		mode = intent ?? 'view';
		proofKind = 'commit';
		proofValue = '';
		proofNote = '';
		reason = '';
	});

	let meta = $derived(STATUS_META[task.status]);
	let parent = $derived(task.parent_id ? (graph.byId.get(task.parent_id) ?? null) : null);
	let children = $derived(graph.children.get(task.id) ?? []);
	let blocks = $derived(graph.blocks.get(task.id) ?? []);
	let blockedBy = $derived(
		task.blocked_by.map((id) => ({ id, task: graph.byId.get(id) ?? null }))
	);

	let panelEl: HTMLElement | undefined = $state();
	$effect(() => {
		void task.id;
		panelEl?.focus();
	});

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			if (mode !== 'view') mode = 'view';
			else onClose();
		}
	}

	async function submitComplete(e: Event) {
		e.preventDefault();
		if (!proofValue.trim() || submitting) return;
		submitting = true;
		try {
			const ok = await onComplete(task, {
				kind: proofKind,
				value: proofValue.trim(),
				note: proofNote.trim() || null
			});
			if (ok) mode = 'view';
		} finally {
			submitting = false;
		}
	}

	async function submitAbandon() {
		if (!reason.trim() || submitting) return;
		submitting = true;
		try {
			const ok = await onAbandon(task, reason.trim());
			if (ok) mode = 'view';
		} finally {
			submitting = false;
		}
	}

	const PROOF_PLACEHOLDER: Record<ProofKind, string> = {
		commit: 'git SHA (e.g. ef6ce63)',
		file: 'path/to/file',
		test: 'test_function_name',
		text: 'free-text description'
	};

	function rawJson(t: Task): string {
		try {
			return JSON.stringify(t, null, 2);
		} catch {
			return String(t);
		}
	}
</script>

<svelte:window onkeydown={onKeydown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="scrim" onclick={onClose}></div>

<div
	class="panel"
	role="dialog"
	aria-modal="false"
	aria-label="Task {task.id} details"
	tabindex="-1"
	bind:this={panelEl}
>
	<header class="panel-header">
		<span class="tid">{task.id}</span>
		<span
			class="status-pill"
			style:color={meta.color}
			style:background={meta.tint}
			style:border-color={meta.border}
		>{meta.label}</span>
		<span class="plan-ctx" title="Plan {planName} on branch {branch}">
			{planName} <span class="on">on</span> {branch}
		</span>
		<button type="button" class="close" onclick={onClose} aria-label="Close panel (Esc)">×</button>
	</header>

	<div class="panel-body">
		<h3 class="title" class:struck={task.status === 'abandoned'}>{task.title}</h3>
		{#if task.description}
			<p class="description">{task.description}</p>
		{/if}

		<!-- Legal transition actions -->
		{#if isOpen(task.status)}
			<div class="actions">
				{#if canStart(task)}
					<button type="button" class="act primary" onclick={() => onStart(task)}>▶ Start</button>
				{/if}
				{#if canComplete(task)}
					<button
						type="button"
						class="act ok"
						class:active={mode === 'complete'}
						onclick={() => (mode = mode === 'complete' ? 'view' : 'complete')}
					>✓ Complete…</button>
				{/if}
				{#if canAbandon(task)}
					<button
						type="button"
						class="act warn"
						class:active={mode === 'abandon'}
						onclick={() => (mode = mode === 'abandon' ? 'view' : 'abandon')}
					>✕ Abandon…</button>
				{/if}
			</div>
		{/if}

		{#if mode === 'complete'}
			<form class="subform" onsubmit={submitComplete}>
				<p class="hint">
					Proof is required by the engine — a commit SHA is strongest, then a file
					path, then a test name, then free text.
				</p>
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
					<input type="text" bind:value={proofValue} placeholder={PROOF_PLACEHOLDER[proofKind]} required />
				</label>
				<label>
					Note (optional)
					<input type="text" bind:value={proofNote} />
				</label>
				<div class="subform-actions">
					<button type="submit" class="act ok" disabled={!proofValue.trim() || submitting}>
						{submitting ? 'Completing…' : 'Mark done'}
					</button>
					<button type="button" class="act" onclick={() => (mode = 'view')}>Cancel</button>
				</div>
			</form>
		{:else if mode === 'abandon'}
			<div class="subform">
				<label>
					Reason (required)
					<input
						type="text"
						bind:value={reason}
						placeholder="why is this task being dropped?"
					/>
				</label>
				<div class="subform-actions">
					<ConfirmButton
						label={submitting ? 'Abandoning…' : 'Abandon task'}
						confirmLabel="Confirm abandon"
						danger
						disabled={!reason.trim() || submitting}
						onconfirm={submitAbandon}
					/>
					<button type="button" class="act" onclick={() => (mode = 'view')}>Cancel</button>
				</div>
			</div>
		{/if}

		{#if task.proof}
			<div class="proof-box">
				<span class="micro-label ok-text">Proof · {task.proof.kind} {PROOF_GLYPH[task.proof.kind]}</span>
				<code class="proof-value">{task.proof.value}</code>
				{#if task.proof.note}
					<p class="proof-note">{task.proof.note}</p>
				{/if}
			</div>
		{/if}
		{#if task.status === 'abandoned' && task.abandoned_reason}
			<div class="reason-box">
				<span class="micro-label warn-text">Abandon reason</span>
				<p class="proof-note">{task.abandoned_reason}</p>
			</div>
		{/if}

		<dl class="meta">
			<dt>Priority</dt>
			<dd><PriorityChip priority={task.priority} /></dd>
			<dt>Assignee</dt>
			<dd>
				{#if task.assigned_to}<AssigneeChip assignee={task.assigned_to} showName />{:else}—{/if}
			</dd>
			<dt>Created</dt>
			<dd>{formatTs(task.created_at)}{#if task.created_by}<span class="by">by {task.created_by}</span>{/if}</dd>
			<dt>Started</dt>
			<dd>{formatTs(task.started_at)}{#if task.started_by}<span class="by">by {task.started_by}</span>{/if}</dd>
			<dt>Completed</dt>
			<dd>{formatTs(task.completed_at)}{#if task.completed_by}<span class="by">by {task.completed_by}</span>{/if}</dd>
			<dt>Abandoned</dt>
			<dd>{formatTs(task.abandoned_at)}</dd>
		</dl>

		{#if parent}
			<section class="rel">
				<span class="micro-label">Parent</span>
				<button type="button" class="rel-row" onclick={() => onNavigate(parent!.id)}>
					<span class="dot" style:background={STATUS_META[parent.status].color}></span>
					<span class="rel-id">{parent.id}</span>
					<span class="rel-title">{parent.title}</span>
				</button>
			</section>
		{/if}

		{#if children.length > 0}
			<section class="rel">
				<span class="micro-label">
					Subtasks · {children.filter((c) => c.status === 'done').length}/{children.length} done
				</span>
				{#each children as c (c.id)}
					<button type="button" class="rel-row" onclick={() => onNavigate(c.id)}>
						<span class="dot" style:background={STATUS_META[c.status].color}></span>
						<span class="rel-id">{c.id}</span>
						<span class="rel-title" class:struck={c.status === 'abandoned'}>{c.title}</span>
						<span class="rel-status" style:color={STATUS_META[c.status].color}>
							{STATUS_META[c.status].label}
						</span>
					</button>
				{/each}
			</section>
		{/if}

		{#if blockedBy.length > 0}
			<section class="rel">
				<span class="micro-label">⛓ Blocked by</span>
				{#each blockedBy as b (b.id)}
					{#if b.task}
						<button type="button" class="rel-row" onclick={() => onNavigate(b.id)}>
							<span class="dot" style:background={STATUS_META[b.task.status].color}></span>
							<span class="rel-id">{b.id}</span>
							<span class="rel-title">{b.task.title}</span>
							<span
								class="rel-status"
								style:color={isOpen(b.task.status) ? 'var(--lens-warn)' : 'var(--lens-ok)'}
							>{isOpen(b.task.status) ? 'open' : 'resolved'}</span>
						</button>
					{:else}
						<span class="rel-row missing">
							<span class="rel-id">{b.id}</span>
							<span class="rel-title">(unknown task)</span>
						</span>
					{/if}
				{/each}
			</section>
		{/if}

		{#if blocks.length > 0}
			<section class="rel">
				<span class="micro-label">Blocks</span>
				{#each blocks as b (b.id)}
					<button type="button" class="rel-row" onclick={() => onNavigate(b.id)}>
						<span class="dot" style:background={STATUS_META[b.status].color}></span>
						<span class="rel-id">{b.id}</span>
						<span class="rel-title">{b.title}</span>
						<span class="rel-status" style:color={isOpen(b.status) ? 'var(--lens-warn)' : 'var(--lens-ok)'}>
							{isOpen(b.status) ? 'waiting' : 'closed'}
						</span>
					</button>
				{/each}
			</section>
		{/if}

		<details class="raw-json">
			<summary>Raw JSON</summary>
			<pre>{rawJson(task)}</pre>
		</details>
	</div>
</div>

<style>
	.scrim {
		position: fixed;
		inset: 0;
		z-index: 90;
		background: rgba(0, 0, 0, 0.25);
	}
	.panel {
		position: fixed;
		top: 0;
		right: 0;
		bottom: 0;
		width: min(430px, 92vw);
		z-index: 95;
		background: var(--lens-overlay);
		border-left: 1px solid var(--lens-border-strong);
		box-shadow: var(--lens-shadow-lg);
		display: flex;
		flex-direction: column;
		animation: slide-in var(--lens-dur-slow) var(--lens-ease-out);
		outline: none;
	}
	@keyframes slide-in {
		from {
			transform: translateX(40px);
			opacity: 0;
		}
		to {
			transform: translateX(0);
			opacity: 1;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.panel {
			animation: none;
		}
	}
	.panel-header {
		display: flex;
		align-items: center;
		gap: var(--lens-space-2);
		padding: var(--lens-space-3) var(--lens-space-4);
		border-bottom: 1px solid var(--lens-border);
		flex: none;
	}
	.tid {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
		color: var(--lens-muted);
	}
	.status-pill {
		font-size: var(--lens-font-size-2xs);
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-wide);
		padding: 0.05rem 0.45rem;
		border: 1px solid transparent;
		border-radius: var(--lens-radius-full);
	}
	.plan-ctx {
		margin-left: auto;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-text-secondary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 12rem;
	}
	.plan-ctx .on {
		color: var(--lens-text-faint);
	}
	.close {
		background: transparent;
		border: none;
		color: var(--lens-muted);
		font-size: 1.15rem;
		line-height: 1;
		cursor: pointer;
		padding: 0.1rem 0.35rem;
		border-radius: var(--lens-radius-sm);
		flex: none;
	}
	.close:hover {
		color: var(--lens-text-strong);
		background: var(--lens-surface-raised);
	}
	.panel-body {
		overflow-y: auto;
		padding: var(--lens-space-4);
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-4);
	}
	.title {
		margin: 0;
		font-size: var(--lens-font-size-md);
		line-height: var(--lens-leading-tight);
		color: var(--lens-text-strong);
	}
	.title.struck {
		text-decoration: line-through;
		color: var(--lens-text-secondary);
	}
	.description {
		margin: 0;
		color: var(--lens-text-secondary);
		font-size: var(--lens-font-size-sm);
		line-height: var(--lens-leading);
		white-space: pre-wrap;
	}
	.actions {
		display: flex;
		gap: var(--lens-space-2);
		flex-wrap: wrap;
	}
	.act {
		background: var(--lens-surface-raised);
		border: 1px solid var(--lens-border);
		color: var(--lens-text);
		padding: 0.3rem 0.7rem;
		border-radius: var(--lens-radius-sm);
		font-size: var(--lens-font-size-xs);
		cursor: pointer;
	}
	.act:hover:not(:disabled) {
		border-color: var(--lens-border-strong);
	}
	.act:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}
	.act.primary {
		background: var(--lens-accent-surface);
		border-color: var(--lens-accent-border);
		color: var(--lens-accent-hover);
	}
	.act.primary:hover {
		background: var(--lens-accent-surface-hi);
	}
	.act.ok {
		color: var(--lens-ok);
	}
	.act.ok.active,
	.act.ok:hover:not(:disabled) {
		background: var(--lens-ok-tint);
		border-color: var(--lens-ok-border);
	}
	.act.warn {
		color: var(--lens-warn);
	}
	.act.warn.active,
	.act.warn:hover {
		background: var(--lens-warn-tint);
		border-color: var(--lens-warn-border);
	}
	.subform {
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-md);
		background: var(--lens-surface);
		padding: var(--lens-space-3);
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-2);
	}
	.hint {
		margin: 0;
		color: var(--lens-text-secondary);
		font-size: var(--lens-font-size-xs);
		line-height: var(--lens-leading);
	}
	.subform label {
		display: flex;
		flex-direction: column;
		gap: 0.2rem;
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}
	.subform input,
	.subform select {
		background: var(--lens-bg);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: 0.35rem 0.5rem;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-sm);
	}
	.subform-actions {
		display: flex;
		gap: var(--lens-space-2);
		margin-top: var(--lens-space-1);
	}
	.proof-box,
	.reason-box {
		border: 1px solid var(--lens-ok-border);
		background: var(--lens-ok-tint);
		border-radius: var(--lens-radius-md);
		padding: var(--lens-space-3);
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-1);
	}
	.reason-box {
		border-color: var(--lens-warn-border);
		background: var(--lens-warn-tint);
	}
	.micro-label {
		font-size: var(--lens-font-size-2xs);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}
	.ok-text {
		color: var(--lens-ok);
	}
	.warn-text {
		color: var(--lens-warn);
	}
	.proof-value {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-sm);
		color: var(--lens-text-strong);
		word-break: break-all;
	}
	.proof-note {
		margin: 0;
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-secondary);
		font-style: italic;
	}
	.meta {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 0.4rem 1rem;
		margin: 0;
	}
	.meta dt {
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
		font-family: var(--lens-font-mono);
		padding-top: 0.1rem;
	}
	.meta dd {
		margin: 0;
		font-size: var(--lens-font-size-xs);
		font-family: var(--lens-font-mono);
		color: var(--lens-text);
		word-break: break-word;
	}
	.by {
		color: var(--lens-muted);
		margin-left: 0.4rem;
	}
	.rel {
		display: flex;
		flex-direction: column;
		gap: 0.3rem;
	}
	.rel-row {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		background: var(--lens-surface);
		border: 1px solid var(--lens-border-subtle);
		border-radius: var(--lens-radius-sm);
		padding: 0.3rem 0.5rem;
		cursor: pointer;
		text-align: left;
		color: var(--lens-text);
		font-size: var(--lens-font-size-xs);
		min-width: 0;
	}
	.rel-row:hover {
		background: var(--lens-surface-raised);
		border-color: var(--lens-border);
	}
	.rel-row.missing {
		cursor: default;
		color: var(--lens-text-faint);
	}
	.dot {
		width: 0.45rem;
		height: 0.45rem;
		border-radius: var(--lens-radius-full);
		flex: none;
	}
	.rel-id {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
		flex: none;
	}
	.rel-title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		flex: 1;
	}
	.rel-title.struck {
		text-decoration: line-through;
	}
	.rel-status {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		flex: none;
	}
	.raw-json summary {
		cursor: pointer;
		color: var(--lens-muted);
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		font-family: var(--lens-font-mono);
	}
	.raw-json pre {
		margin-top: var(--lens-space-2);
		padding: var(--lens-space-3);
		background: var(--lens-bg);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-text);
		white-space: pre-wrap;
		word-break: break-word;
		max-height: 18rem;
		overflow-y: auto;
	}
</style>
