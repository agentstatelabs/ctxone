<script lang="ts">
	import {
		listReminders,
		getDueReminders,
		createReminder,
		snoozeReminder,
		approveReminder,
		cancelReminder,
		startReminder,
		recordReminder,
		type Reminder,
		type ReminderPriority,
		type ReminderExecution
	} from '$lib/api';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import ScopeBadge from '$lib/ScopeBadge.svelte';
	import EmptyState from '$lib/EmptyState.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	type Filter = 'due' | 'pending' | 'all';
	const FILTERS: Filter[] = ['due', 'pending', 'all'];

	let filter: Filter = $state('due');
	let reminders: Reminder[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);
	let actionError: string | null = $state(null);

	let showCreate = $state(false);
	let newTitle = $state('');
	let newInstructions = $state('');
	let newDueAt = $state('');
	let newPriority: ReminderPriority = $state('medium');
	let newAutonomous = $state(true);
	let createError: string | null = $state(null);

	// Per-row expanders: which reminder has its snooze / record panel open.
	let snoozeOpenId: string | null = $state(null);
	let recordOpenId: string | null = $state(null);
	let recordResult: ReminderExecution['result'] = $state('success');

	async function load() {
		error = null;
		try {
			if (filter === 'due') {
				reminders = await getDueReminders();
			} else if (filter === 'pending') {
				reminders = await listReminders('pending');
			} else {
				reminders = await listReminders();
			}
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			reminders = [];
		} finally {
			loading = false;
		}
	}

	// Reminders are namespace-scoped (not branch-scoped) — re-load on
	// namespace change and whenever the filter tab changes.
	$effect(() => {
		void namespaceStore.current;
		void filter;
		loading = true;
		load();
	});

	const auto = useAutoRefresh(load);

	const TERMINAL = new Set(['completed', 'cancelled']);
	const now = () => new Date();

	function isOverdue(r: Reminder): boolean {
		if (TERMINAL.has(r.status)) return false;
		return new Date(r.due_at).getTime() < now().getTime();
	}

	/** Day-group key: Overdue / Today / Tomorrow / weekday date / Done. */
	function dayKey(r: Reminder): string {
		if (TERMINAL.has(r.status)) return 'Done';
		if (isOverdue(r)) return 'Overdue';
		const due = new Date(r.due_at);
		const today = now();
		const startOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
		const dayDiff = Math.round((startOf(due) - startOf(today)) / 86_400_000);
		if (dayDiff === 0) return 'Today';
		if (dayDiff === 1) return 'Tomorrow';
		return due.toLocaleDateString(undefined, {
			weekday: 'short',
			month: 'short',
			day: 'numeric',
			year: due.getFullYear() === today.getFullYear() ? undefined : 'numeric'
		});
	}

	interface DayGroup {
		label: string;
		items: Reminder[];
	}

	// Group by day, ordering: Overdue first, then chronological, Done last.
	let groups: DayGroup[] = $derived.by(() => {
		const sorted = [...reminders].sort(
			(a, b) => new Date(a.due_at).getTime() - new Date(b.due_at).getTime()
		);
		const map = new Map<string, Reminder[]>();
		for (const r of sorted) {
			const key = dayKey(r);
			const list = map.get(key) ?? [];
			list.push(r);
			map.set(key, list);
		}
		const out: DayGroup[] = [];
		const emit = (key: string) => {
			const items = map.get(key);
			if (items) {
				out.push({ label: key, items });
				map.delete(key);
			}
		};
		emit('Overdue');
		const done = map.get('Done');
		map.delete('Done');
		for (const [label, items] of map) out.push({ label, items });
		if (done) out.push({ label: 'Done', items: done });
		return out;
	});

	async function handleCreate(e: Event) {
		e.preventDefault();
		createError = null;
		const title = newTitle.trim();
		const instructions = newInstructions.trim();
		if (!title || !instructions || !newDueAt) return;
		try {
			// datetime-local is timezone-naive; Date() interprets it as local
			// time and toISOString() converts to the UTC RFC 3339 the Hub wants.
			await createReminder({
				title,
				instructions,
				due_at: new Date(newDueAt).toISOString(),
				priority: newPriority,
				autonomous: newAutonomous
			});
			newTitle = '';
			newInstructions = '';
			newDueAt = '';
			showCreate = false;
			await load();
		} catch (err) {
			createError = err instanceof Error ? err.message : String(err);
		}
	}

	async function doAction(fn: () => Promise<Reminder>) {
		actionError = null;
		try {
			await fn();
			await load();
		} catch (e) {
			actionError = e instanceof Error ? e.message : String(e);
		}
	}

	function handleApprove(r: Reminder) {
		void doAction(() => approveReminder(r.id, 'lens-user'));
	}

	function handleStart(r: Reminder) {
		void doAction(() => startReminder(r.id));
	}

	function handleCancel(r: Reminder) {
		if (!confirm(`Cancel reminder "${r.title}"? This is permanent.`)) return;
		void doAction(() => cancelReminder(r.id));
	}

	function handleSnooze(r: Reminder, ms: number) {
		snoozeOpenId = null;
		void doAction(() => snoozeReminder(r.id, new Date(Date.now() + ms).toISOString()));
	}

	function handleRecord(r: Reminder) {
		recordOpenId = null;
		void doAction(() => recordReminder(r.id, recordResult));
	}

	function toggleSnooze(id: string) {
		snoozeOpenId = snoozeOpenId === id ? null : id;
		recordOpenId = null;
	}

	function toggleRecord(id: string) {
		recordOpenId = recordOpenId === id ? null : id;
		recordResult = 'success';
		snoozeOpenId = null;
	}

	function fmtDue(iso: string): string {
		const d = new Date(iso);
		return d.toLocaleString(undefined, {
			hour: '2-digit',
			minute: '2-digit',
			month: 'short',
			day: 'numeric'
		});
	}

	const HOUR = 3_600_000;
	const DAY = 24 * HOUR;
</script>

<div class="page">
	<header class="page-header">
		<h1>Reminders <ScopeBadge /></h1>
		<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
		<button class="btn" onclick={() => (showCreate = !showCreate)}>
			{showCreate ? 'Cancel' : '+ New reminder'}
		</button>
	</header>

	<div class="filters" role="tablist" aria-label="Reminder filter">
		{#each FILTERS as f}
			<button
				class="seg"
				class:active={filter === f}
				role="tab"
				aria-selected={filter === f}
				onclick={() => (filter = f)}
			>
				{f}
			</button>
		{/each}
	</div>

	{#if showCreate}
		<form class="create-form" onsubmit={handleCreate}>
			<div class="form-row">
				<input type="text" bind:value={newTitle} placeholder="title (imperative, one line)" required />
				<input
					type="datetime-local"
					bind:value={newDueAt}
					required
					title="When this reminder becomes due"
				/>
				<label class="inline-label">
					priority
					<select bind:value={newPriority}>
						<option value="critical">critical</option>
						<option value="high">high</option>
						<option value="medium">medium</option>
						<option value="low">low</option>
						<option value="minimal">minimal</option>
					</select>
				</label>
				<label class="inline-label check">
					<input type="checkbox" bind:checked={newAutonomous} />
					autonomous
				</label>
			</div>
			<div class="form-row">
				<textarea
					bind:value={newInstructions}
					placeholder="instructions the agent should follow at execution time"
					rows="2"
					required
				></textarea>
				<button type="submit">Create</button>
			</div>
			{#if createError}
				<span class="error">{createError}</span>
			{/if}
		</form>
	{/if}

	{#if actionError}
		<p class="error">{actionError}</p>
	{/if}

	{#if error}
		<p class="error">{error}</p>
	{:else if loading && reminders.length === 0}
		<p class="muted">Loading reminders…</p>
	{:else if reminders.length === 0}
		<EmptyState
			icon="⏰"
			title={filter === 'due'
				? 'Nothing actionable right now'
				: filter === 'pending'
					? 'No pending reminders'
					: 'No reminders yet'}
			description={filter === 'all'
				? 'Reminders let agents schedule future work in this workspace (e.g. `ctx remind`). None have been created.'
				: undefined}
		/>
	{:else}
		{#each groups as group}
			<h3 class="day-label" class:overdue={group.label === 'Overdue'}>{group.label}</h3>
			<table class="reminders">
				<tbody>
					{#each group.items as r (r.id)}
						<tr class:overdue={isOverdue(r)}>
							<td class="due">
								{fmtDue(r.due_at)}
								{#if r.status === 'snoozed' && r.snoozed_until}
									<span class="snoozed-note">→ {fmtDue(r.snoozed_until)}</span>
								{/if}
							</td>
							<td class="title-cell">
								<span class="title">{r.title}</span>
								<span class="instructions">{r.instructions}</span>
							</td>
							<td class="badges">
								<span class="badge priority-{r.priority}">{r.priority}</span>
								<span class="badge status-{r.status}">{r.status.replace('_', ' ')}</span>
								{#if r.schedule && r.schedule.kind !== 'once'}
									<span class="badge schedule" title="recurring">↻ {r.schedule.kind}</span>
								{/if}
								{#if !r.autonomous}
									<span class="badge manual" title="needs approval before an agent may execute">
										needs approval
									</span>
								{/if}
							</td>
							<td class="actions">
								{#if r.status === 'awaiting_permission'}
									<button class="btn-sm accent" onclick={() => handleApprove(r)}>Approve</button>
								{/if}
								{#if r.status === 'due' || r.status === 'awaiting_permission'}
									<button class="btn-sm" onclick={() => handleStart(r)}>Start</button>
								{/if}
								{#if r.status === 'in_progress'}
									<button class="btn-sm" onclick={() => toggleRecord(r.id)}>Record…</button>
								{/if}
								{#if !TERMINAL.has(r.status) && r.status !== 'in_progress'}
									<button class="btn-sm" onclick={() => toggleSnooze(r.id)}>Snooze…</button>
								{/if}
								{#if !TERMINAL.has(r.status)}
									<button class="btn-sm danger" onclick={() => handleCancel(r)}>Cancel</button>
								{/if}
							</td>
						</tr>
						{#if snoozeOpenId === r.id}
							<tr class="expander">
								<td colspan="4">
									snooze until:
									<button class="btn-sm" onclick={() => handleSnooze(r, HOUR)}>+1 hour</button>
									<button class="btn-sm" onclick={() => handleSnooze(r, DAY)}>+1 day</button>
									<button class="btn-sm" onclick={() => handleSnooze(r, 7 * DAY)}>+1 week</button>
									<button class="btn-sm" onclick={() => (snoozeOpenId = null)}>never mind</button>
								</td>
							</tr>
						{/if}
						{#if recordOpenId === r.id}
							<tr class="expander">
								<td colspan="4">
									record outcome:
									<select bind:value={recordResult}>
										<option value="success">success</option>
										<option value="failed">failed</option>
										<option value="deferred">deferred</option>
										<option value="cancelled">cancelled</option>
									</select>
									<button class="btn-sm accent" onclick={() => handleRecord(r)}>Record</button>
									<button class="btn-sm" onclick={() => (recordOpenId = null)}>never mind</button>
								</td>
							</tr>
						{/if}
					{/each}
				</tbody>
			</table>
		{/each}
	{/if}
</div>

<style>
	.page {
		max-width: 1000px;
	}

	.page-header {
		display: flex;
		align-items: center;
		gap: 1rem;
		margin-bottom: 1rem;
	}

	h1 {
		margin: 0;
		font-size: 1.6rem;
	}

	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		margin-right: auto;
	}

	.filters {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: 6px;
		overflow: hidden;
		margin-bottom: 1.25rem;
	}

	.seg {
		background: var(--bg-1);
		border: none;
		color: var(--text-2);
		padding: 0.35rem 0.9rem;
		cursor: pointer;
		font-size: 0.85rem;
		text-transform: capitalize;
	}
	.seg + .seg {
		border-left: 1px solid var(--border);
	}
	.seg.active {
		background: var(--accent-bg);
		color: var(--accent);
	}

	.btn,
	.btn-sm {
		background: var(--bg-1);
		border: 1px solid var(--border);
		color: var(--text-2);
		padding: 0.35rem 0.75rem;
		border-radius: 6px;
		cursor: pointer;
		font-size: 0.85rem;
	}
	.btn:hover,
	.btn-sm:hover {
		color: var(--text-0);
		border-color: var(--text-3);
	}
	.btn-sm {
		padding: 0.2rem 0.55rem;
		font-size: 0.78rem;
	}
	.btn-sm.danger:hover {
		color: var(--danger);
		border-color: var(--danger);
	}
	.btn-sm.accent {
		background: var(--accent-bg);
		border-color: var(--accent-bg-hi);
		color: var(--accent);
	}

	.create-form {
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 0.75rem;
		margin-bottom: 1.25rem;
	}

	.form-row {
		display: flex;
		gap: 0.5rem;
		align-items: center;
	}

	.create-form input[type='text'],
	.create-form textarea {
		flex: 1;
		min-width: 8rem;
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.4rem 0.6rem;
		border-radius: 4px;
		font-family: inherit;
	}

	.create-form input[type='datetime-local'],
	.create-form select,
	.expander select {
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.35rem 0.5rem;
		border-radius: 4px;
	}

	.inline-label {
		font-size: 0.8rem;
		color: var(--text-2);
		display: flex;
		gap: 0.4rem;
		align-items: center;
		white-space: nowrap;
	}
	.inline-label.check input {
		accent-color: var(--accent);
	}

	.create-form button[type='submit'] {
		background: var(--accent-bg);
		border: 1px solid var(--accent-bg-hi);
		color: var(--accent);
		padding: 0.4rem 0.9rem;
		border-radius: 4px;
		cursor: pointer;
	}

	.error {
		color: var(--danger);
		font-size: 0.85rem;
	}
	.muted {
		color: var(--text-3);
		font-size: 0.9rem;
	}

	.day-label {
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-3);
		margin: 1.25rem 0 0.4rem;
	}
	.day-label.overdue {
		color: var(--danger);
	}

	table.reminders {
		width: 100%;
		border-collapse: collapse;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow: hidden;
	}

	td {
		padding: 0.6rem 0.9rem;
		text-align: left;
		border-bottom: 1px solid var(--border);
		vertical-align: top;
	}
	tr:last-child td {
		border-bottom: none;
	}

	tr.overdue td.due {
		color: var(--danger);
		font-weight: 600;
	}

	td.due {
		font-family: monospace;
		font-size: 0.8rem;
		color: var(--text-3);
		white-space: nowrap;
		width: 8.5rem;
	}

	.snoozed-note {
		display: block;
		color: var(--text-3);
		font-weight: normal;
		font-size: 0.72rem;
	}

	.title-cell .title {
		display: block;
		color: var(--text-0);
		font-size: 0.92rem;
	}
	.title-cell .instructions {
		display: block;
		color: var(--text-3);
		font-size: 0.8rem;
		margin-top: 0.15rem;
	}

	td.badges {
		white-space: nowrap;
		width: 1%;
	}

	.badge {
		display: inline-block;
		font-size: 0.68rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		padding: 0.08rem 0.45rem;
		border-radius: 3px;
		background: var(--bg-0);
		color: var(--text-2);
		border: 1px solid var(--border);
		margin-left: 0.3rem;
	}
	.badge:first-child {
		margin-left: 0;
	}

	.badge.priority-critical,
	.badge.priority-high {
		color: var(--danger);
		border-color: var(--danger);
	}
	.badge.status-due,
	.badge.status-awaiting_permission {
		color: var(--accent);
		background: var(--accent-bg);
		border-color: var(--accent-bg-hi);
	}
	.badge.status-completed {
		opacity: 0.6;
	}
	.badge.status-cancelled {
		opacity: 0.5;
		text-decoration: line-through;
	}
	.badge.manual {
		color: var(--danger);
	}

	td.actions {
		text-align: right;
		white-space: nowrap;
		width: 1%;
	}

	tr.expander td {
		background: var(--bg-0);
		font-size: 0.82rem;
		color: var(--text-2);
	}
</style>
