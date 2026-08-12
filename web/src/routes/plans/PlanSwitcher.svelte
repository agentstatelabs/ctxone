<script lang="ts">
	import type { Plan } from '$lib/plansApi';
	import { makeMatcher } from '$lib/glob';
	import {
		EFFECTIVE_STATUS_LABELS,
		EFFECTIVE_STATUS_ORDER,
		effectivePlanStatus
	} from './model';

	let {
		plans,
		selectedName,
		onSelect,
		onCreate
	}: {
		plans: Plan[];
		selectedName: string | null;
		onSelect: (name: string) => void;
		/** Returns true on success (closes the create form). */
		onCreate: (name: string, description: string | null) => Promise<boolean>;
	} = $props();

	let open = $state(false);

	/* Status filter chips — same effective-status buckets the old page
	   filtered on, persisted under the same key. */
	type StatusFilter = 'all' | 'in_progress' | 'active' | 'completed' | 'archived';
	const FILTERS: StatusFilter[] = ['all', 'in_progress', 'active', 'completed', 'archived'];
	const FILTER_LABELS: Record<StatusFilter, string> = {
		all: 'All',
		in_progress: 'In progress',
		active: 'Active',
		completed: 'Completed',
		archived: 'Archived'
	};
	const FILTER_KEY = 'lens.plans.statusFilter';
	function loadFilter(): StatusFilter {
		if (typeof localStorage === 'undefined') return 'all';
		const v = localStorage.getItem(FILTER_KEY) as StatusFilter | null;
		return v && FILTERS.includes(v) ? v : 'all';
	}
	let statusFilter: StatusFilter = $state(loadFilter());
	$effect(() => {
		if (typeof localStorage !== 'undefined') localStorage.setItem(FILTER_KEY, statusFilter);
	});

	/* Grouped vs flat list — carried over from the old sidebar. */
	type ListMode = 'tree' | 'flat';
	const MODE_KEY = 'lens.plans.view';
	function loadMode(): ListMode {
		if (typeof localStorage === 'undefined') return 'tree';
		return localStorage.getItem(MODE_KEY) === 'flat' ? 'flat' : 'tree';
	}
	let listMode: ListMode = $state(loadMode());
	function setMode(m: ListMode) {
		listMode = m;
		if (typeof localStorage !== 'undefined') localStorage.setItem(MODE_KEY, m);
	}

	/* Sort — carried over. */
	type PlanSort = 'status' | 'date-new' | 'date-old' | 'name';
	const SORT_KEY = 'lens.plans.sort';
	function loadSort(): PlanSort {
		if (typeof localStorage === 'undefined') return 'status';
		const v = localStorage.getItem(SORT_KEY) as PlanSort | null;
		return v && ['status', 'date-new', 'date-old', 'name'].includes(v) ? v : 'status';
	}
	let planSort: PlanSort = $state(loadSort());
	$effect(() => {
		if (typeof localStorage !== 'undefined') localStorage.setItem(SORT_KEY, planSort);
	});

	let search = $state('');

	/* Create form */
	let showCreate = $state(false);
	let newName = $state('');
	let newDesc = $state('');
	let creating = $state(false);

	const STATUS_RANK: Record<string, number> = {
		in_progress: 0,
		active: 1,
		completed: 2,
		archived: 3
	};
	function activityTs(p: Plan): number {
		const t = p.archived_at ?? p.created_at;
		return t ? new Date(t).getTime() : 0;
	}
	function compare(a: Plan, b: Plan): number {
		switch (planSort) {
			case 'status': {
				const ra = STATUS_RANK[effectivePlanStatus(a)] ?? 99;
				const rb = STATUS_RANK[effectivePlanStatus(b)] ?? 99;
				if (ra !== rb) return ra - rb;
				return activityTs(b) - activityTs(a);
			}
			case 'date-new':
				return activityTs(b) - activityTs(a);
			case 'date-old':
				return activityTs(a) - activityTs(b);
			case 'name':
				return a.name.localeCompare(b.name);
		}
	}

	let filtered = $derived.by(() => {
		const m = makeMatcher(search);
		return plans
			.filter((p) => {
				if (statusFilter !== 'all' && effectivePlanStatus(p) !== statusFilter) return false;
				return m(p.name) || m(p.description ?? '');
			})
			.sort(compare);
	});

	let grouped = $derived.by(() => {
		const buckets: Record<string, Plan[]> = {};
		for (const p of filtered) {
			const eff = effectivePlanStatus(p);
			(buckets[eff] ??= []).push(p);
		}
		return EFFECTIVE_STATUS_ORDER.filter((s) => buckets[s]?.length).map((s) => ({
			key: s as string,
			label: EFFECTIVE_STATUS_LABELS[s],
			plans: buckets[s]
		}));
	});

	/* Per-bucket show-all + flat incremental reveal — the pagination
	   re-presentation for long plan lists. */
	const PAGE = 25;
	let expandedGroups: Set<string> = $state(new Set());
	function toggleExpanded(key: string) {
		const next = new Set(expandedGroups);
		if (next.has(key)) next.delete(key);
		else next.add(key);
		expandedGroups = next;
	}
	let collapsedGroups: Set<string> = $state(new Set());
	function toggleCollapsed(key: string) {
		const next = new Set(collapsedGroups);
		if (next.has(key)) next.delete(key);
		else next.add(key);
		collapsedGroups = next;
	}
	let flatLimit = $state(PAGE);
	$effect(() => {
		void search;
		void statusFilter;
		flatLimit = PAGE;
	});

	let selected = $derived(plans.find((p) => p.name === selectedName) ?? null);

	function pick(name: string) {
		open = false;
		onSelect(name);
	}

	async function submitCreate(e: Event) {
		e.preventDefault();
		if (!newName.trim() || creating) return;
		creating = true;
		try {
			const ok = await onCreate(newName.trim(), newDesc.trim() || null);
			if (ok) {
				newName = '';
				newDesc = '';
				showCreate = false;
				open = false;
			}
		} finally {
			creating = false;
		}
	}

	function onKeydown(e: KeyboardEvent) {
		if (open && e.key === 'Escape') open = false;
	}
</script>

<svelte:window onkeydown={onKeydown} />

<div class="switcher">
	<button
		type="button"
		class="trigger"
		aria-haspopup="listbox"
		aria-expanded={open}
		onclick={() => (open = !open)}
	>
		<span class="trigger-name">{selected?.name ?? 'Select a plan'}</span>
		{#if selected}
			{@const eff = effectivePlanStatus(selected)}
			<span class="trigger-status status-{eff}">{eff.replace('_', ' ')}</span>
		{/if}
		<span class="caret" aria-hidden="true">▾</span>
	</button>

	{#if open}
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<!-- svelte-ignore a11y_click_events_have_key_events -->
		<div class="backdrop" onclick={() => (open = false)}></div>
		<div class="popover" role="listbox" aria-label="Plans">
			<div class="pop-controls">
				<input
					type="search"
					class="search"
					placeholder="Search plans…"
					bind:value={search}
					aria-label="Search plans"
				/>
				<div class="chips" role="radiogroup" aria-label="Filter plans by status">
					{#each FILTERS as f (f)}
						<button
							type="button"
							class="chip"
							class:active={statusFilter === f}
							role="radio"
							aria-checked={statusFilter === f}
							onclick={() => (statusFilter = f)}
						>{FILTER_LABELS[f]}</button>
					{/each}
				</div>
				<div class="row2">
					<div class="seg-group" role="tablist" aria-label="List mode">
						<button
							type="button"
							class="seg"
							class:active={listMode === 'tree'}
							onclick={() => setMode('tree')}
						>Grouped</button>
						<button
							type="button"
							class="seg"
							class:active={listMode === 'flat'}
							onclick={() => setMode('flat')}
						>Flat</button>
					</div>
					<select bind:value={planSort} aria-label="Sort plans">
						<option value="status">Status</option>
						<option value="date-new">Newest</option>
						<option value="date-old">Oldest</option>
						<option value="name">Name</option>
					</select>
					<button type="button" class="new-plan" onclick={() => (showCreate = !showCreate)}>
						{showCreate ? 'Cancel' : '+ New plan'}
					</button>
				</div>
				{#if showCreate}
					<form class="create-form" onsubmit={submitCreate}>
						<input
							type="text"
							bind:value={newName}
							placeholder="plan-name (kebab-case)"
							required
						/>
						<input type="text" bind:value={newDesc} placeholder="description (optional)" />
						<button type="submit" disabled={creating || !newName.trim()}>
							{creating ? 'Creating…' : 'Create'}
						</button>
					</form>
				{/if}
			</div>

			<div class="pop-list">
				{#if plans.length === 0}
					<p class="empty">No plans yet — create one above.</p>
				{:else if filtered.length === 0}
					<p class="empty">No plans match.</p>
				{:else if listMode === 'tree'}
					{#each grouped as group (group.key)}
						{@const collapsed = collapsedGroups.has(group.key)}
						{@const expanded = expandedGroups.has(group.key)}
						{@const hidden = Math.max(0, group.plans.length - PAGE)}
						{@const visible = expanded || hidden === 0 ? group.plans : group.plans.slice(0, PAGE)}
						<button
							type="button"
							class="group-header"
							onclick={() => toggleCollapsed(group.key)}
							aria-expanded={!collapsed}
						>
							<span class="gcaret">{collapsed ? '▸' : '▾'}</span>
							<span class="glabel status-{group.key}">{group.label}</span>
							<span class="gcount">{group.plans.length}</span>
						</button>
						{#if !collapsed}
							{#each visible as plan (plan.name)}
								{@render planRow(plan)}
							{/each}
							{#if hidden > 0}
								<button type="button" class="show-more" onclick={() => toggleExpanded(group.key)}>
									{expanded ? `Show less (hide ${hidden})` : `Show all ${group.plans.length}`}
								</button>
							{/if}
						{/if}
					{/each}
				{:else}
					{#each filtered.slice(0, flatLimit) as plan (plan.name)}
						{@render planRow(plan)}
					{/each}
					{#if filtered.length > flatLimit}
						<button type="button" class="show-more" onclick={() => (flatLimit += PAGE)}>
							Show more ({filtered.length - flatLimit} remaining)
						</button>
					{:else}
						<p class="total">{filtered.length} plans</p>
					{/if}
				{/if}
			</div>
		</div>
	{/if}
</div>

{#snippet planRow(plan: Plan)}
	{@const eff = effectivePlanStatus(plan)}
	<button
		type="button"
		class="plan-row"
		class:selected={plan.name === selectedName}
		role="option"
		aria-selected={plan.name === selectedName}
		title={plan.description ?? plan.name}
		onclick={() => pick(plan.name)}
	>
		<span class="plan-name">{plan.name}</span>
		<span class="plan-meta">
			<span class="plan-status status-{eff}">{eff.replace('_', ' ')}</span>
			<span class="plan-counts" title="{plan.task_counts.done} done · {plan.task_counts.in_progress} in progress · {plan.task_counts.pending} pending">
				{plan.task_counts.done}✓ {plan.task_counts.in_progress}▶ {plan.task_counts.pending}·
			</span>
		</span>
	</button>
{/snippet}

<style>
	.switcher {
		position: relative;
		display: inline-block;
	}
	.trigger {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
		background: var(--lens-surface);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text-strong);
		padding: 0.35rem 0.7rem;
		font-size: var(--lens-font-size-sm);
		cursor: pointer;
		max-width: 24rem;
	}
	.trigger:hover {
		border-color: var(--lens-border-strong);
	}
	.trigger-name {
		font-family: var(--lens-font-mono);
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.trigger-status {
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-wide);
		flex: none;
	}
	.caret {
		color: var(--lens-muted);
		font-size: 0.7em;
		flex: none;
	}
	.backdrop {
		position: fixed;
		inset: 0;
		z-index: 60;
	}
	.popover {
		position: absolute;
		top: calc(100% + 6px);
		left: 0;
		z-index: 70;
		width: min(24rem, 90vw);
		max-height: 34rem;
		display: flex;
		flex-direction: column;
		background: var(--lens-overlay);
		border: 1px solid var(--lens-border-strong);
		border-radius: var(--lens-radius-md);
		box-shadow: var(--lens-shadow-md);
	}
	.pop-controls {
		padding: var(--lens-space-2);
		border-bottom: 1px solid var(--lens-border);
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-2);
		flex: none;
	}
	.search {
		background: var(--lens-bg);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: 0.35rem 0.55rem;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 0.3rem;
	}
	.chip {
		background: var(--lens-surface);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-full);
		color: var(--lens-text-secondary);
		font-size: var(--lens-font-size-2xs);
		padding: 0.1rem 0.55rem;
		cursor: pointer;
	}
	.chip:hover {
		border-color: var(--lens-border-strong);
	}
	.chip.active {
		background: var(--lens-accent-tint);
		border-color: var(--lens-accent-border);
		color: var(--lens-accent);
	}
	.row2 {
		display: flex;
		align-items: center;
		gap: var(--lens-space-2);
	}
	.seg-group {
		display: inline-flex;
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		overflow: hidden;
	}
	.seg {
		background: var(--lens-surface);
		border: none;
		color: var(--lens-text-secondary);
		padding: 0.25rem 0.55rem;
		font-size: var(--lens-font-size-2xs);
		cursor: pointer;
	}
	.seg:not(:last-child) {
		border-right: 1px solid var(--lens-border);
	}
	.seg.active {
		background: var(--lens-accent-tint);
		color: var(--lens-accent);
	}
	.row2 select {
		background: var(--lens-surface);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: 0.25rem 0.4rem;
		font-size: var(--lens-font-size-2xs);
		font-family: var(--lens-font-mono);
	}
	.new-plan {
		margin-left: auto;
		background: var(--lens-accent-surface);
		border: 1px solid var(--lens-accent-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-accent-hover);
		font-size: var(--lens-font-size-2xs);
		padding: 0.25rem 0.55rem;
		cursor: pointer;
	}
	.new-plan:hover {
		background: var(--lens-accent-surface-hi);
	}
	.create-form {
		display: flex;
		flex-direction: column;
		gap: 0.35rem;
	}
	.create-form input {
		background: var(--lens-bg);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: 0.3rem 0.5rem;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
	}
	.create-form button {
		align-self: flex-start;
		background: var(--lens-accent-surface);
		border: 1px solid var(--lens-accent-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-accent-hover);
		font-size: var(--lens-font-size-xs);
		padding: 0.25rem 0.7rem;
		cursor: pointer;
	}
	.create-form button:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}
	.pop-list {
		overflow-y: auto;
		padding: var(--lens-space-1);
	}
	.group-header {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		width: 100%;
		background: transparent;
		border: none;
		padding: 0.35rem 0.45rem;
		cursor: pointer;
		text-align: left;
	}
	.group-header:hover {
		background: var(--lens-surface-raised);
		border-radius: var(--lens-radius-sm);
	}
	.gcaret {
		color: var(--lens-muted);
		font-size: var(--lens-font-size-2xs);
		width: 0.8rem;
	}
	.glabel {
		flex: 1;
		font-size: var(--lens-font-size-2xs);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
	}
	.gcount {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
	}
	.plan-row {
		display: block;
		width: 100%;
		background: transparent;
		border: none;
		border-radius: var(--lens-radius-sm);
		padding: 0.4rem 0.5rem;
		cursor: pointer;
		text-align: left;
		color: var(--lens-text);
	}
	.plan-row:hover {
		background: var(--lens-surface-raised);
	}
	.plan-row.selected {
		background: var(--lens-accent-tint);
	}
	.plan-name {
		display: block;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.plan-meta {
		display: flex;
		gap: 0.6rem;
		margin-top: 0.15rem;
	}
	.plan-status {
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-wide);
	}
	.plan-counts {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
	}
	.status-in_progress {
		color: var(--lens-accent);
	}
	.status-active {
		color: var(--lens-ok);
	}
	.status-completed {
		color: var(--lens-text-secondary);
	}
	.status-archived {
		color: var(--lens-text-faint);
	}
	.show-more {
		display: block;
		width: 100%;
		background: transparent;
		border: 1px dashed var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-muted);
		font-size: var(--lens-font-size-2xs);
		padding: 0.3rem;
		margin: 0.2rem 0;
		cursor: pointer;
	}
	.show-more:hover {
		background: var(--lens-surface-raised);
		color: var(--lens-text);
	}
	.empty {
		color: var(--lens-text-faint);
		font-style: italic;
		font-size: var(--lens-font-size-xs);
		text-align: center;
		padding: var(--lens-space-4);
		margin: 0;
	}
	.total {
		text-align: center;
		color: var(--lens-text-faint);
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		margin: 0.3rem 0;
	}
</style>
