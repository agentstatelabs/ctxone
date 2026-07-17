<script lang="ts">
	import type { AddTaskRequest, Priority, Task } from '$lib/plansApi';
	import { STATUS_META, isOpen } from './model';

	let {
		tasks,
		planName,
		onSubmit,
		onClose
	}: {
		tasks: Task[];
		planName: string;
		/** Returns true on success (closes the modal). */
		onSubmit: (req: AddTaskRequest) => Promise<boolean>;
		onClose: () => void;
	} = $props();

	let title = $state('');
	let description = $state('');
	let priority: Priority = $state('medium');
	let assignedTo = $state('');
	let parentId = $state('');
	let blockedBy: Set<string> = $state(new Set());
	let submitting = $state(false);

	/** Only open tasks make sense as blockers of a brand-new task. */
	let blockerOptions = $derived(tasks.filter((t) => isOpen(t.status)));

	function toggleBlocker(id: string) {
		const next = new Set(blockedBy);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		blockedBy = next;
	}

	async function submit(e: Event) {
		e.preventDefault();
		if (!title.trim() || submitting) return;
		submitting = true;
		try {
			const req: AddTaskRequest = { title: title.trim(), priority };
			if (description.trim()) req.description = description.trim();
			if (assignedTo.trim()) req.assigned_to = assignedTo.trim();
			if (parentId) req.parent_id = parentId;
			if (blockedBy.size > 0) req.blocked_by = [...blockedBy];
			await onSubmit(req);
		} finally {
			submitting = false;
		}
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') onClose();
	}
</script>

<svelte:window onkeydown={onKeydown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="backdrop" onclick={onClose}>
	<div
		class="modal"
		role="dialog"
		aria-label="Add task to {planName}"
		tabindex="-1"
		onclick={(e) => e.stopPropagation()}
	>
		<header class="head">
			<h3>Add task <span class="plan">to {planName}</span></h3>
			<button type="button" class="close" onclick={onClose} aria-label="Close">×</button>
		</header>
		<form onsubmit={submit}>
			<label>
				Title
				<input type="text" bind:value={title} placeholder="task title (imperative)" required />
			</label>
			<label>
				Description (optional)
				<textarea bind:value={description} rows="2" placeholder="details, acceptance criteria…"></textarea>
			</label>
			<div class="row">
				<label>
					Priority
					<select bind:value={priority}>
						<option value="low">Low</option>
						<option value="medium">Medium</option>
						<option value="high">High</option>
						<option value="critical">Critical</option>
					</select>
				</label>
				<label>
					Assign to (optional)
					<input type="text" bind:value={assignedTo} placeholder="e.g. claude-code" />
				</label>
			</div>
			<label>
				Parent task (optional — makes this a subtask)
				<select bind:value={parentId}>
					<option value="">— none —</option>
					{#each tasks as t (t.id)}
						<option value={t.id}>{t.id} · {t.title}</option>
					{/each}
				</select>
			</label>
			{#if blockerOptions.length > 0}
				<fieldset class="blockers">
					<legend>Blocked by (optional)</legend>
					<div class="blocker-list">
						{#each blockerOptions as t (t.id)}
							<label class="blocker">
								<input
									type="checkbox"
									checked={blockedBy.has(t.id)}
									onchange={() => toggleBlocker(t.id)}
								/>
								<span class="dot" style:background={STATUS_META[t.status].color}></span>
								<span class="bid">{t.id}</span>
								<span class="btitle">{t.title}</span>
							</label>
						{/each}
					</div>
				</fieldset>
			{/if}
			<div class="actions">
				<button type="submit" class="primary" disabled={!title.trim() || submitting}>
					{submitting ? 'Adding…' : 'Add task'}
				</button>
				<button type="button" onclick={onClose}>Cancel</button>
			</div>
		</form>
	</div>
</div>

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		z-index: 100;
		background: rgba(0, 0, 0, 0.55);
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.modal {
		background: var(--lens-overlay);
		border: 1px solid var(--lens-border-strong);
		border-radius: var(--lens-radius-md);
		box-shadow: var(--lens-shadow-lg);
		width: min(480px, 92vw);
		max-height: 88vh;
		overflow-y: auto;
		padding: var(--lens-space-4);
	}
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: var(--lens-space-3);
	}
	.head h3 {
		margin: 0;
		font-size: var(--lens-font-size-md);
		color: var(--lens-text-strong);
	}
	.plan {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
		color: var(--lens-muted);
		font-weight: normal;
	}
	.close {
		background: transparent;
		border: none;
		color: var(--lens-muted);
		font-size: 1.1rem;
		cursor: pointer;
		padding: 0.1rem 0.35rem;
		border-radius: var(--lens-radius-sm);
	}
	.close:hover {
		color: var(--lens-text-strong);
		background: var(--lens-surface-raised);
	}
	form {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-3);
	}
	label {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
		flex: 1;
	}
	input,
	select,
	textarea {
		background: var(--lens-bg);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: 0.4rem 0.55rem;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-sm);
		text-transform: none;
		letter-spacing: normal;
	}
	textarea {
		resize: vertical;
		font-family: var(--lens-font-sans);
	}
	.row {
		display: flex;
		gap: var(--lens-space-3);
	}
	.blockers {
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		padding: var(--lens-space-2);
		margin: 0;
	}
	.blockers legend {
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
		padding: 0 0.3rem;
	}
	.blocker-list {
		display: flex;
		flex-direction: column;
		gap: 0.2rem;
		max-height: 9rem;
		overflow-y: auto;
	}
	.blocker {
		flex-direction: row;
		align-items: center;
		gap: 0.45rem;
		text-transform: none;
		letter-spacing: normal;
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text);
		cursor: pointer;
		padding: 0.15rem 0.2rem;
		border-radius: var(--lens-radius-sm);
	}
	.blocker:hover {
		background: var(--lens-surface-raised);
	}
	.blocker input {
		padding: 0;
	}
	.dot {
		width: 0.4rem;
		height: 0.4rem;
		border-radius: var(--lens-radius-full);
		flex: none;
	}
	.bid {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
		flex: none;
	}
	.btitle {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.actions {
		display: flex;
		gap: var(--lens-space-2);
		justify-content: flex-end;
	}
	.actions button {
		background: var(--lens-surface-raised);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		font-size: var(--lens-font-size-xs);
		padding: 0.35rem 0.8rem;
		cursor: pointer;
	}
	.actions button:hover:not(:disabled) {
		border-color: var(--lens-border-strong);
	}
	.actions .primary {
		background: var(--lens-accent-surface);
		border-color: var(--lens-accent-border);
		color: var(--lens-accent-hover);
	}
	.actions .primary:hover:not(:disabled) {
		background: var(--lens-accent-surface-hi);
	}
	.actions button:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}
</style>
