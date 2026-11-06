<script lang="ts">
	import { tick, untrack } from 'svelte';
	import { editTask, getTask, type Proof, type ProofKind, type Task } from '$lib/plansApi';
	import type { TaskGraph } from './model';
	import {
		MAX_NOTES_BYTES,
		PROOF_GLYPH,
		STATUS_META,
		canAbandon,
		canComplete,
		canStart,
		formatBytes,
		formatTs,
		hasNotes,
		isOpen,
		notesBytes,
		packedTitle
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
		onAbandon,
		onNotesSaved
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
		/** After notes were saved or cleared, so the page can reload the plan. */
		onNotesSaved?: (taskId: string, cleared: boolean) => void | Promise<void>;
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

	// Notes. The plan listing marks a task `has_notes` and leaves the notes
	// out, so the panel reads the task on its own for them.
	let full = $state<Task | null>(null);
	let notesLoadError = $state<string | null>(null);
	let editingNotes = $state(false);
	let draft = $state('');
	let draftBase = $state('');
	let savingNotes = $state(false);
	let notesError = $state<string | null>(null);
	let notesEl: HTMLTextAreaElement | undefined = $state();

	// Re-sync when the panel is retargeted: a new task, or an explicit
	// intent from a board drag / quick action. A plan refresh hands the
	// panel a fresh copy of the same task every tick, so this compares ids
	// rather than running on every copy, which would close an open form.
	let shownId: string | null = null;
	let shownIntent: typeof intent = null;
	$effect(() => {
		const id = task.id;
		const want = intent;
		if (id === shownId && want === shownIntent) return;
		const retargeted = id !== shownId;
		shownId = id;
		shownIntent = want;
		mode = want ?? 'view';
		proofKind = 'commit';
		proofValue = '';
		proofNote = '';
		reason = '';
		if (retargeted) {
			editingNotes = false;
			draft = '';
			draftBase = '';
			notesError = null;
			notesLoadError = null;
		}
	});

	let loadSeq = 0;
	async function loadNotes(id: string) {
		const seq = ++loadSeq;
		try {
			const t = await getTask(planName, id, branch);
			if (seq !== loadSeq) return;
			full = t;
			notesLoadError = null;
		} catch (e) {
			if (seq !== loadSeq) return;
			notesLoadError = e instanceof Error ? e.message : String(e);
		}
	}

	// Read the task again on every fresh copy, not just on a retarget: an
	// agent may have changed the notes since the last plan refresh.
	$effect(() => {
		const id = task.id;
		void task;
		void planName;
		void branch;
		untrack(() => void loadNotes(id));
	});

	/** The task as read on its own, once it matches the one shown. */
	let fullHere = $derived(full && full.id === task.id ? full : null);
	let notes = $derived(fullHere?.notes || null);
	/** A task from before notes existed, its brief still in its title. */
	let packed = $derived(hasNotes(fullHere ?? task) ? null : packedTitle(task.title));
	/** The Hub refuses notes edits on a finished task unless forced. */
	let notesEditable = $derived(isOpen(task.status));
	let notesPending = $derived(!fullHere && !notesLoadError && hasNotes(task));
	let draftBytes = $derived(notesBytes(draft.trim()));
	let draftDirty = $derived(draft.trim() !== draftBase.trim());

	async function startEditNotes() {
		const base = notes ?? packed?.brief ?? '';
		draftBase = base;
		draft = base;
		notesError = null;
		editingNotes = true;
		await tick();
		notesEl?.focus();
	}

	function cancelEditNotes() {
		editingNotes = false;
		notesError = null;
	}

	/** PATCH the notes (`""` clears them), keeping the editor open on error. */
	async function saveNotes(value: string) {
		if (savingNotes) return;
		const id = task.id;
		savingNotes = true;
		notesError = null;
		try {
			await editTask(planName, id, { notes: value }, branch);
		} catch (e) {
			if (task.id === id) notesError = e instanceof Error ? e.message : String(e);
			return;
		} finally {
			savingNotes = false;
		}
		if (task.id === id) {
			// Show what was saved now; the re-read confirms it. The reply
			// carries `has_notes`, not the notes.
			if (full?.id === id) full = { ...full, notes: value || null };
			editingNotes = false;
			void loadNotes(id);
		}
		await onNotesSaved?.(id, value === '');
	}

	function submitNotes(e: Event) {
		e.preventDefault();
		if (!draftDirty) return;
		void saveNotes(draft.trim());
	}

	function notesKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) submitNotes(e);
	}

	let meta = $derived(STATUS_META[task.status]);
	let parent = $derived(task.parent_id ? (graph.byId.get(task.parent_id) ?? null) : null);
	let children = $derived(graph.children.get(task.id) ?? []);
	let blocks = $derived(graph.blocks.get(task.id) ?? []);
	let blockedBy = $derived(
		task.blocked_by.map((id) => ({ id, task: graph.byId.get(id) ?? null }))
	);

	// Focus the panel when it opens or is retargeted, but not on a plan
	// refresh: that would pull focus out of a field being typed in.
	let panelEl: HTMLElement | undefined = $state();
	let focusedId: string | null = null;
	$effect(() => {
		const id = task.id;
		if (!panelEl || id === focusedId) return;
		focusedId = id;
		panelEl.focus();
	});

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			// Esc never throws away an edited draft: Cancel does that.
			if (editingNotes) {
				if (!draftDirty) cancelEditNotes();
			} else if (mode !== 'view') mode = 'view';
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

		{#if notesEditable || hasNotes(fullHere ?? task)}
			<section class="notes" aria-label="Notes">
				<div class="notes-head">
					<span class="micro-label">Notes</span>
					{#if notesEditable && !editingNotes}
						<button
							type="button"
							class="notes-edit"
							disabled={!fullHere}
							onclick={startEditNotes}
						>{notes || packed ? 'Edit' : 'Add'}</button>
					{/if}
				</div>
				{#if editingNotes}
					<form class="subform" onsubmit={submitNotes}>
						{#if packed}
							<p class="hint">
								This brief is still part of the task's title, from before tasks had
								notes. Saving takes it out of the title: what you save here becomes
								the notes.
							</p>
						{/if}
						<textarea
							bind:this={notesEl}
							bind:value={draft}
							onkeydown={notesKeydown}
							rows="10"
							aria-label="Task notes"
							aria-describedby="notes-bytes"
							placeholder="why it matters, what to check, what counts as proof, links…"
							readonly={savingNotes}
						></textarea>
						<span
							id="notes-bytes"
							class="notes-bytes"
							class:over={draftBytes > MAX_NOTES_BYTES}
						>{formatBytes(draftBytes)} of {formatBytes(MAX_NOTES_BYTES)}</span>
						{#if notesError}
							<p class="notes-error" role="alert">{notesError}</p>
						{/if}
						<div class="subform-actions">
							<button type="submit" class="act primary" disabled={!draftDirty || savingNotes}>
								{savingNotes ? 'Saving…' : 'Save notes'}
							</button>
							<button type="button" class="act" disabled={savingNotes} onclick={cancelEditNotes}>
								Cancel
							</button>
							{#if notes}
								<span class="spacer"></span>
								<ConfirmButton
									label="Clear notes"
									confirmLabel="Confirm clear"
									danger
									disabled={savingNotes}
									onconfirm={() => void saveNotes('')}
								/>
							{/if}
						</div>
					</form>
				{:else if notes}
					<!-- Plain text: interpolation escapes it, and pre-wrap keeps its line breaks. -->
					<div class="notes-text" data-testid="task-notes">{notes}</div>
				{:else if notesLoadError && !fullHere}
					<p class="notes-error" role="alert">
						Couldn't load the notes: {notesLoadError}
						<button type="button" class="notes-edit" onclick={() => void loadNotes(task.id)}>
							Retry
						</button>
					</p>
				{:else if notesPending}
					<p class="notes-empty">Loading notes…</p>
				{:else}
					<p class="notes-empty">
						{packed ? "No notes yet: this task's brief is still in its title." : 'No notes.'}
					</p>
				{/if}
			</section>
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
			<pre>{rawJson(fullHere ?? task)}</pre>
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
	.notes {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-2);
	}
	.notes-head {
		display: flex;
		align-items: center;
		gap: var(--lens-space-2);
	}
	.notes-edit {
		background: transparent;
		border: none;
		padding: 0;
		color: var(--lens-accent);
		font-size: var(--lens-font-size-2xs);
		cursor: pointer;
	}
	.notes-edit:hover:not(:disabled) {
		color: var(--lens-accent-hover);
		text-decoration: underline;
	}
	.notes-edit:disabled {
		color: var(--lens-text-faint);
		cursor: default;
	}
	.notes-text {
		color: var(--lens-text);
		font-size: var(--lens-font-size-sm);
		line-height: var(--lens-leading);
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
	.notes-empty {
		margin: 0;
		color: var(--lens-text-faint);
		font-size: var(--lens-font-size-xs);
		font-style: italic;
	}
	.notes-error {
		margin: 0;
		color: var(--lens-danger);
		font-size: var(--lens-font-size-xs);
		line-height: var(--lens-leading);
		overflow-wrap: anywhere;
	}
	.notes-bytes {
		align-self: flex-end;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
	}
	.notes-bytes.over {
		color: var(--lens-danger);
	}
	.subform textarea {
		background: var(--lens-bg);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: 0.4rem 0.55rem;
		font-family: var(--lens-font-sans);
		font-size: var(--lens-font-size-sm);
		line-height: var(--lens-leading);
		resize: vertical;
		min-height: 8rem;
	}
	.spacer {
		flex: 1;
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
