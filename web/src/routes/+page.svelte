<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import {
		getHealth,
		getTokenStats,
		getNamespacesSummary,
		listProjects,
		type TokenStats,
		type WorkspaceSummary,
		type Project
	} from '$lib/api';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';
	import { StatTile, formatCompact, trimFloat } from '@agentstate/lens-core';
	import Panel from '$lib/dashboard/Panel.svelte';
	import EmptyState from '$lib/EmptyState.svelte';

	// Hub Home is the ONE cross-workspace view: it does not scope by branch and
	// treats the namespace header as irrelevant (every endpoint it calls is
	// hub-global). Drill into a workspace to get branch/plan/epoch detail.

	let healthy = $state(true);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let tokens = $state<TokenStats | null>(null);
	let workspaces = $state<WorkspaceSummary[]>([]);
	let projects = $state<Project[]>([]);

	/** namespace -> project metadata (display name, remote), joined client-side. */
	let projectByNs = $derived.by(() => {
		const m = new Map<string, Project>();
		for (const p of projects) m.set(p.namespace, p);
		return m;
	});

	let workspaceCount = $derived(workspaces.length);
	let totalSessions = $derived(workspaces.reduce((n, w) => n + w.session_count, 0));
	// Fresh install: no registered projects and nothing ingested anywhere.
	let firstRun = $derived(!loading && projects.length === 0 && totalSessions === 0);

	let panelStatus = $derived<'loading' | 'error' | 'empty' | 'ready'>(
		loading ? 'loading' : error ? 'error' : workspaces.length === 0 ? 'empty' : 'ready'
	);

	async function load() {
		error = null;
		try {
			const [ok, tok, ws, projs] = await Promise.all([
				getHealth(),
				getTokenStats(),
				getNamespacesSummary(),
				listProjects().catch(() => [] as Project[])
			]);
			healthy = ok;
			tokens = tok;
			// Busiest workspaces first — most sessions is the useful default.
			workspaces = [...ws].sort((a, b) => b.session_count - a.session_count);
			projects = projs;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	const auto = useAutoRefresh(() => load());
	onMount(load); // initial paint — useAutoRefresh only sets the interval

	function displayName(ns: string): string {
		return projectByNs.get(ns)?.display_name || ns;
	}
	function remoteOf(ns: string): string | null {
		return projectByNs.get(ns)?.remote_url ?? null;
	}
	/** Strip the scheme/host down to owner/repo for a compact subtitle. */
	function shortRemote(url: string | null): string | null {
		if (!url) return null;
		return url.replace(/^https?:\/\/[^/]+\//, '').replace(/\.git$/, '');
	}

	function openWorkspace(ns: string) {
		namespaceStore.current = ns; // resets branch to main (namespace-scoped refs)
		goto('/workspace');
	}
</script>

<div class="hub">
	<header class="hub-head">
		<div class="titles">
			<h1>CTXone</h1>
			<span class="subtitle">Hub overview — all workspaces</span>
		</div>
		<div class="head-right">
			<span class="health" class:ok={healthy} class:bad={!healthy}>
				{healthy ? 'Hub connected' : 'Hub unreachable'}
			</span>
			{#if auto.lastRefreshed}<span class="refreshed">updated {formatAgo(auto.lastRefreshed)}</span>{/if}
		</div>
	</header>

	{#if error}
		<p class="error">Couldn’t load the hub: {error}</p>
	{/if}

	<!-- Grand totals: truly hub-wide (getTokenStats aggregates every session). -->
	<div class="tiles">
		<StatTile label="Workspaces" value={loading ? '—' : String(workspaceCount)} />
		<StatTile label="Sessions" value={loading ? '—' : String(totalSessions)} />
		<StatTile
			label="Tokens used"
			value={tokens ? formatCompact(tokens.session_tokens_used) : '—'}
			unit="tok · all workspaces"
			title="Total context tokens sent across every session in the hub"
		/>
		<StatTile
			label="Tokens saved"
			value={tokens ? formatCompact(tokens.session_tokens_saved) : '—'}
			unit={tokens ? `tok · ${trimFloat(tokens.cumulative_ratio, 1)}× ratio` : undefined}
			accent
			title="Total tokens CTXone kept out of prompts vs a flat context"
		/>
	</div>

	{#if firstRun}
		<EmptyState
			icon="👋"
			title="Welcome to CTXone"
			description="No workspaces are populated yet. Register a project to map a code repo to its own workspace, or just start using ctx — recall/remember populate the default workspace automatically."
			actionLabel="Register a workspace"
			actionHref="/projects"
		/>
	{:else}
	<Panel
		title="Workspaces"
		scope="hub-wide"
		status={panelStatus}
		errorText={error ?? ''}
		emptyTitle="No workspaces yet"
		emptyText="Register one under Settings → Workspaces, or run ctx recall / ctx remember to populate the default workspace."
	>
		<div class="grid" aria-label="Workspaces">
			{#each workspaces as w (w.namespace)}
					{@const remote = shortRemote(remoteOf(w.namespace))}
					<button
						type="button"
						class="card"
						class:current={namespaceStore.current === w.namespace}
						onclick={() => openWorkspace(w.namespace)}
						title={`Open ${displayName(w.namespace)}`}
					>
						<div class="card-head">
							<span class="card-name">{displayName(w.namespace)}</span>
							<code class="card-ns">{w.namespace}</code>
						</div>
						{#if remote}<div class="card-remote" title={remoteOf(w.namespace)}>{remote}</div>{/if}

						<div class="card-stats">
							<span class="stat"><strong>{w.session_count}</strong> sessions</span>
							<span class="stat" title="{w.tokens.used} used / {w.tokens.saved} saved">
								<strong>{formatCompact(w.tokens.used)}</strong> tok
								{#if w.tokens.saved > 0}
									<span class="saved">· {formatCompact(w.tokens.saved)} saved</span>
								{/if}
							</span>
						</div>

						{#if w.graph}
							<div class="card-counts">
								<span title="commits on main">{formatCompact(w.graph.commit_count)} commits</span>
								<span class="dot">·</span>
								<span title="memory paths">{formatCompact(w.graph.path_count)} paths</span>
								<span class="dot">·</span>
								<span title="branches">{w.graph.branch_count} branches</span>
								<span class="dot">·</span>
								<span title="epochs">{w.graph.epoch_count} epochs</span>
							</div>
						{/if}
					</button>
				{/each}
			</div>
	</Panel>
	{/if}
</div>

<style>
	.hub {
		max-width: 1400px;
		margin: 0 auto;
		padding: var(--lens-space-4, 1rem);
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-4, 1rem);
	}
	.hub-head {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: 1rem;
		flex-wrap: wrap;
	}
	.titles h1 {
		margin: 0;
		font-size: 1.5rem;
		color: var(--lens-text-strong);
	}
	.subtitle {
		color: var(--lens-text-secondary);
		font-size: 0.9rem;
	}
	.head-right {
		display: flex;
		align-items: center;
		gap: 0.75rem;
	}
	.health {
		font-size: 0.8rem;
		font-weight: 600;
		border-radius: 999px;
		padding: 0.15rem 0.6rem;
		border: 1px solid var(--lens-border);
	}
	.health.ok {
		color: var(--lens-ok);
		border-color: color-mix(in srgb, var(--lens-ok) 40%, var(--lens-border));
	}
	.health.bad {
		color: var(--lens-danger);
		border-color: color-mix(in srgb, var(--lens-danger) 40%, var(--lens-border));
	}
	.refreshed {
		color: var(--lens-text-faint);
		font-size: 0.8rem;
		font-variant-numeric: tabular-nums;
	}
	.error {
		color: var(--lens-danger);
		font-size: 0.9rem;
	}

	.tiles {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
		gap: var(--lens-space-3, 0.75rem);
	}

	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
		gap: var(--lens-space-3, 0.75rem);
	}
	.card {
		text-align: left;
		background: var(--lens-surface);
		border: 1px solid var(--lens-border);
		border-radius: 8px;
		padding: 0.8rem 0.9rem;
		cursor: pointer;
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		transition:
			border-color 0.1s,
			background 0.1s;
	}
	.card:hover {
		border-color: var(--lens-border-strong);
		background: var(--lens-surface-raised);
	}
	.card.current {
		border-color: var(--lens-accent);
	}
	.card-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 0.5rem;
	}
	.card-name {
		font-weight: 600;
		color: var(--lens-text-strong);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.card-ns {
		flex: none;
		font-size: 0.7rem;
		color: var(--lens-text-faint);
		font-family: var(--lens-font-mono, monospace);
	}
	.card-remote {
		font-size: 0.75rem;
		color: var(--lens-text-secondary);
		font-family: var(--lens-font-mono, monospace);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.card-stats {
		display: flex;
		flex-wrap: wrap;
		gap: 0.75rem;
		font-size: 0.85rem;
		color: var(--lens-text);
		margin-top: 0.1rem;
	}
	.card-stats strong {
		color: var(--lens-text-strong);
	}
	.saved {
		color: var(--lens-ok);
	}
	.card-counts {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.35rem;
		font-size: 0.78rem;
		color: var(--lens-text-secondary);
	}
	.card-counts .dot {
		opacity: 0.5;
	}
</style>
