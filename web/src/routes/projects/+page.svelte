<script lang="ts">
	import { listProjects, registerProject, type Project } from '$lib/api';
	import { namespaceStore, DEFAULT_NAMESPACE } from '$lib/namespaceStore.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	let projects: Project[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);

	let showCreate = $state(false);
	let newId = $state('');
	let newDisplayName = $state('');
	let newRemoteUrl = $state('');
	let newLocalPath = $state('');
	let createError: string | null = $state(null);

	async function load() {
		loading = true;
		error = null;
		try {
			const list = await listProjects();
			list.sort((a, b) => a.id.localeCompare(b.id));
			projects = list;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	// Load on mount; the project list itself is global (not
	// namespace-scoped), but re-running on switch keeps it fresh.
	$effect(() => {
		void namespaceStore.current;
		load();
	});

	const auto = useAutoRefresh(load);

	function activate(namespace: string) {
		namespaceStore.current = namespace;
	}

	async function handleCreate(e: Event) {
		e.preventDefault();
		const id = newId.trim();
		if (!id) return;
		createError = null;
		try {
			const project = await registerProject({
				id,
				display_name: newDisplayName.trim() || undefined,
				remote_url: newRemoteUrl.trim() || undefined,
				local_path: newLocalPath.trim() || undefined
			});
			newId = '';
			newDisplayName = '';
			newRemoteUrl = '';
			newLocalPath = '';
			showCreate = false;
			await load();
			namespaceStore.current = project.namespace;
		} catch (err) {
			createError = err instanceof Error ? err.message : String(err);
		}
	}
</script>

<div class="page">
	<header class="page-header">
		<h1>Projects</h1>
		<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
		<button class="btn" onclick={() => (showCreate = !showCreate)}>
			{showCreate ? 'Cancel' : '+ Register project'}
		</button>
	</header>

	{#if showCreate}
		<form class="create-form" onsubmit={handleCreate}>
			<input type="text" bind:value={newId} placeholder="project id" required />
			<input type="text" bind:value={newDisplayName} placeholder="display name (optional)" />
			<input type="text" bind:value={newRemoteUrl} placeholder="remote url (optional)" />
			<input type="text" bind:value={newLocalPath} placeholder="local path (optional)" />
			<button type="submit">Register</button>
			{#if createError}
				<span class="error">{createError}</span>
			{/if}
		</form>
	{/if}

	{#if error}
		<p class="error">{error}</p>
	{:else if loading && projects.length === 0}
		<p class="muted">Loading projects…</p>
	{:else}
		<table class="projects">
			<thead>
				<tr>
					<th>Project</th>
					<th>Namespace</th>
					<th>Remote</th>
					<th>Local paths</th>
					<th>ASD repos</th>
					<th>Created</th>
					<th class="actions">Actions</th>
				</tr>
			</thead>
			<tbody>
				<tr class:active={namespaceStore.current === DEFAULT_NAMESPACE}>
					<td class="name">
						default
						{#if namespaceStore.current === DEFAULT_NAMESPACE}
							<span class="active-tag">current</span>
						{/if}
					</td>
					<td><code>{DEFAULT_NAMESPACE}</code></td>
					<td><span class="muted">—</span></td>
					<td><span class="muted">—</span></td>
					<td><span class="muted">—</span></td>
					<td><span class="muted">pre-namespace data</span></td>
					<td class="actions">
						<button
							class="btn-sm"
							onclick={() => activate(DEFAULT_NAMESPACE)}
							disabled={namespaceStore.current === DEFAULT_NAMESPACE}
						>
							Switch to
						</button>
					</td>
				</tr>
				{#each projects as p}
					<tr class:active={p.namespace === namespaceStore.current}>
						<td class="name">
							{p.display_name ?? p.id}
							{#if p.display_name && p.display_name !== p.id}
								<code class="proj-id">{p.id}</code>
							{/if}
							{#if p.namespace === namespaceStore.current}
								<span class="active-tag">current</span>
							{/if}
						</td>
						<td><code>{p.namespace}</code></td>
						<td>
							{#if p.remote_url}
								<code class="remote">{p.remote_url}</code>
							{:else}
								<span class="muted">—</span>
							{/if}
						</td>
						<td>
							{#if p.local_paths.length > 0}
								<ul class="path-list">
									{#each p.local_paths as path}
										<li><code>{path}</code></li>
									{/each}
								</ul>
							{:else}
								<span class="muted">—</span>
							{/if}
						</td>
						<td>
							{#if p.asd_repos.length > 0}
								{p.asd_repos.join(', ')}
							{:else}
								<span class="muted">—</span>
							{/if}
						</td>
						<td class="created">{new Date(p.created_at).toLocaleDateString()}</td>
						<td class="actions">
							<button
								class="btn-sm"
								onclick={() => activate(p.namespace)}
								disabled={p.namespace === namespaceStore.current}
							>
								Switch to
							</button>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
		{#if projects.length === 0}
			<p class="muted hint">
				No projects registered yet — everything lives in the default namespace.
				Register a project to give a repo its own branches, plans, and memory.
			</p>
		{/if}
	{/if}
</div>

<style>
	.page {
		max-width: 1100px;
	}

	.page-header {
		display: flex;
		align-items: center;
		gap: 1rem;
		margin-bottom: 1.5rem;
	}

	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		margin-right: auto;
	}

	h1 {
		margin: 0;
		font-size: 1.6rem;
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
	.btn-sm:hover:not(:disabled) {
		color: var(--text-0);
		border-color: var(--text-3);
	}
	.btn-sm {
		padding: 0.2rem 0.55rem;
		font-size: 0.78rem;
	}
	.btn-sm:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.create-form {
		display: flex;
		gap: 0.5rem;
		align-items: center;
		flex-wrap: wrap;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 0.75rem;
		margin-bottom: 1.25rem;
	}

	.create-form input {
		flex: 1;
		min-width: 10rem;
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.4rem 0.6rem;
		border-radius: 4px;
	}

	.create-form button {
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
	.hint {
		margin-top: 1rem;
	}

	table.projects {
		width: 100%;
		border-collapse: collapse;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow: hidden;
	}

	th,
	td {
		padding: 0.6rem 0.9rem;
		text-align: left;
		border-bottom: 1px solid var(--border);
		vertical-align: top;
	}
	tr:last-child td {
		border-bottom: none;
	}
	th {
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-3);
		background: var(--bg-0);
		font-weight: 600;
	}
	tr.active td {
		background: var(--bg-active);
	}
	td.actions,
	th.actions {
		text-align: right;
		white-space: nowrap;
	}

	.name {
		color: var(--text-0);
	}

	.proj-id {
		display: block;
		font-family: monospace;
		color: var(--text-3);
		font-size: 0.75rem;
	}

	code {
		font-family: monospace;
		color: var(--text-2);
		font-size: 0.85rem;
	}

	.remote {
		word-break: break-all;
	}

	.path-list {
		list-style: none;
		padding: 0;
		margin: 0;
	}
	.path-list li {
		margin-bottom: 0.15rem;
	}

	.created {
		white-space: nowrap;
		color: var(--text-2);
		font-size: 0.85rem;
	}

	.active-tag {
		font-size: 0.65rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--accent);
		background: var(--accent-bg);
		padding: 0.05rem 0.4rem;
		border-radius: 3px;
		margin-left: 0.5rem;
		vertical-align: middle;
	}
</style>
