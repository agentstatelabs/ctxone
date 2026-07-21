<!--
	RecentActivity — the five most-recently-updated branches, plans, and
	sessions, each row a link into its detail page.

	Everything the workspace produces is timestamped: sessions carry the
	last-turn `updated_at`, branches the head-commit time, plans their
	`created_at`. This surfaces "what moved most recently" without the user
	hunting across three pages. Sessions arrive by import; branches and plans
	are updated by the agent as it works, so this is a live pulse of the
	workspace.
-->
<script lang="ts">
	import { getBranches, type BranchInfo, type SessionSnapshot } from '$lib/api';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import type { Plan } from '$lib/plansApi';

	let {
		sessions = [],
		plans = []
	}: { sessions?: SessionSnapshot[]; plans?: Plan[] } = $props();

	const SHOW = 5;

	let branches = $state<BranchInfo[]>([]);
	let branchError = $state(false);

	// Branches aren't loaded by the dashboard's other panels, so fetch them
	// here. Re-runs when the workspace changes (getBranches is namespace-scoped
	// via hubFetch). The `main` branch is dropped — it's the baseline, not a
	// recently-worked branch, and would otherwise pin the top slot forever.
	$effect(() => {
		namespaceStore.current; // dependency: refetch on workspace switch
		branchError = false;
		getBranches()
			.then((b) => (branches = b.filter((x) => x.name !== 'main')))
			.catch(() => {
				branches = [];
				branchError = true;
			});
	});

	function ts(v: string | null | undefined): number {
		if (!v) return 0;
		const t = Date.parse(v);
		return Number.isNaN(t) ? 0 : t;
	}

	/** Newest first, top SHOW, dropping entries with no timestamp to sort by. */
	function recent<T>(items: T[], stamp: (t: T) => number): T[] {
		return items
			.map((it) => ({ it, at: stamp(it) }))
			.filter((x) => x.at > 0)
			.sort((a, b) => b.at - a.at)
			.slice(0, SHOW)
			.map((x) => x.it);
	}

	const recentBranches = $derived(recent(branches, (b) => ts(b.updated_at)));
	const recentPlans = $derived(recent(plans, (p) => ts(p.created_at)));
	const recentSessions = $derived(recent(sessions, (s) => ts(s.updated_at)));

	function ago(ms: number): string {
		const s = Math.max(0, (Date.now() - ms) / 1000);
		if (s < 90) return 'just now';
		if (s < 5400) return `${Math.round(s / 60)}m`;
		if (s < 86400) return `${Math.round(s / 3600)}h`;
		if (s < 86400 * 30) return `${Math.round(s / 86400)}d`;
		return `${Math.round(s / (86400 * 30))}mo`;
	}

	function sessionLabel(s: SessionSnapshot): string {
		const n = (s.name ?? '').trim();
		if (n) return n.length > 40 ? n.slice(0, 39) + '…' : n;
		return s.session_id.slice(0, 8);
	}
</script>

<div class="recent">
	<section>
		<h4>Branches</h4>
		{#if recentBranches.length === 0}
			<p class="none">{branchError ? 'Unavailable on this hub.' : 'No branches yet.'}</p>
		{:else}
			<ul>
				{#each recentBranches as b (b.name)}
					<li>
						<a class="name" href={`/branches?branch=${encodeURIComponent(b.name)}`} title={b.name}>
							{b.name}
						</a>
						<span class="when">{ago(ts(b.updated_at))}</span>
					</li>
				{/each}
			</ul>
		{/if}
	</section>

	<section>
		<h4>Plans</h4>
		{#if recentPlans.length === 0}
			<p class="none">No plans yet.</p>
		{:else}
			<ul>
				{#each recentPlans as p (p.name)}
					<li>
						<a class="name" href={`/plans?plan=${encodeURIComponent(p.name)}`} title={p.name}>
							{p.name}
						</a>
						<span class="when">{ago(ts(p.created_at))}</span>
					</li>
				{/each}
			</ul>
		{/if}
	</section>

	<section>
		<h4>Sessions</h4>
		{#if recentSessions.length === 0}
			<p class="none">No sessions yet.</p>
		{:else}
			<ul>
				{#each recentSessions as s (s.session_id)}
					<li>
						<a
							class="name"
							href={`/sessions?session=${encodeURIComponent(s.session_id)}`}
							title={s.name ?? s.session_id}
						>
							{sessionLabel(s)}
						</a>
						<span class="when">{ago(ts(s.updated_at))}</span>
					</li>
				{/each}
			</ul>
		{/if}
	</section>
</div>

<style>
	.recent {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: var(--lens-space-4);
	}
	@media (max-width: 640px) {
		.recent {
			grid-template-columns: minmax(0, 1fr);
		}
	}

	h4 {
		margin: 0 0 var(--lens-space-2);
		font-size: var(--lens-font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--lens-text-muted);
	}

	ul {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-1, 4px);
	}

	li {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--lens-space-2);
		font-size: var(--lens-font-size-sm);
		min-width: 0;
	}

	.name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--lens-text);
		text-decoration: none;
	}
	.name:hover {
		color: var(--lens-accent);
		text-decoration: underline;
	}

	.when {
		flex: none;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-muted);
	}

	.none {
		margin: 0;
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-muted);
	}
</style>
