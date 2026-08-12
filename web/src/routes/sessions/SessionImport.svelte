<script lang="ts">
	import { hubFetch, listNamespaces } from '$lib/api';
	import { formatCompact } from '@agentstate/lens-core';

	interface Props {
		/** Two-way: parent opens the panel; the panel closes itself. */
		open: boolean;
		/** Fired after a successful import so the parent can reload its list. */
		onimported?: () => void;
	}
	let { open = $bindable(), onimported }: Props = $props();

	interface Discoverable {
		number: number;
		id: string;
		source: string;
		project: string;
		last_activity: number | null;
		status: 'new' | 'imported' | 'ignored';
	}

	let items = $state<Discoverable[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let msg = $state<string | null>(null);
	let busy = $state(false);

	// Filters
	let sourceFilter = $state('all');
	let hideImported = $state(true);
	let search = $state('');

	// Selection + per-row workspace target ('' = auto/route by repo).
	let selected = $state<Set<string>>(new Set());
	let targets = $state<Record<string, string>>({});
	let bulkTarget = $state('');
	let workspaces = $state<string[]>([]);

	// Autosync (background sweep) toggle.
	let autosyncEnabled = $state(false);
	let autosyncInterval = $state(900);
	let autosyncBusy = $state(false);

	// Load everything the first time the panel opens (and refresh on reopen).
	let lastOpen = false;
	$effect(() => {
		if (open && !lastOpen) {
			void loadAll();
		}
		lastOpen = open;
	});

	async function loadAll() {
		await Promise.all([loadDiscoverable(), loadWorkspaces(), loadAutosync()]);
	}

	async function loadDiscoverable() {
		loading = true;
		error = null;
		try {
			const r = await hubFetch('/api/sessions/discoverable');
			if (r.status === 404) {
				error = 'Import is not available on this Hub version.';
				items = [];
				return;
			}
			if (!r.ok) throw new Error(`${r.status} ${(await r.text()) || r.statusText}`);
			items = await r.json();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			items = [];
		} finally {
			loading = false;
		}
	}

	async function loadWorkspaces() {
		try {
			workspaces = (await listNamespaces()).sort();
		} catch {
			workspaces = [];
		}
	}

	async function loadAutosync() {
		try {
			const r = await hubFetch('/api/sessions/autosync');
			if (!r.ok) return;
			const v = await r.json();
			autosyncEnabled = !!v.enabled;
			if (typeof v.interval_secs === 'number') autosyncInterval = v.interval_secs;
		} catch {
			/* older hub — leave defaults */
		}
	}

	const sources = $derived(['all', ...new Set(items.map((i) => i.source))]);

	const visible = $derived(
		items.filter((i) => {
			if (sourceFilter !== 'all' && i.source !== sourceFilter) return false;
			if (hideImported && i.status === 'imported') return false;
			if (search.trim()) {
				const q = search.toLowerCase();
				if (!i.project.toLowerCase().includes(q) && !i.id.toLowerCase().includes(q))
					return false;
			}
			return true;
		})
	);

	const selectedCount = $derived(selected.size);

	function shortId(id: string): string {
		const [pfx, tail] = id.includes(':') ? id.split(/:(.+)/) : ['', id];
		const t = tail.length > 12 ? tail.slice(0, 12) : tail;
		return pfx ? `${pfx}:${t}` : t;
	}
	function fmtDate(secs: number | null): string {
		if (!secs) return '—';
		return new Date(secs * 1000).toISOString().slice(0, 10);
	}

	function toggle(id: string) {
		const next = new Set(selected);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		selected = next;
	}
	function selectAllVisible() {
		selected = new Set(visible.map((i) => i.id));
	}
	function selectNewVisible() {
		selected = new Set(visible.filter((i) => i.status === 'new').map((i) => i.id));
	}
	function clearSelection() {
		selected = new Set();
	}

	function applyBulkTarget() {
		const next = { ...targets };
		for (const id of selected) next[id] = bulkTarget;
		targets = next;
	}

	/** Group the selected ids by their chosen workspace ('' = auto), so we make
	 *  one import call per distinct target. */
	function groupByTarget(ids: string[]): Map<string, string[]> {
		const groups = new Map<string, string[]>();
		for (const id of ids) {
			const to = targets[id] ?? '';
			const arr = groups.get(to) ?? [];
			arr.push(id);
			groups.set(to, arr);
		}
		return groups;
	}

	async function doImport(ids: string[]) {
		if (ids.length === 0) {
			msg = 'Nothing selected to import.';
			return;
		}
		busy = true;
		msg = null;
		error = null;
		try {
			let sessions = 0;
			for (const [to, group] of groupByTarget(ids)) {
				const r = await hubFetch('/api/sessions/import', {
					method: 'POST',
					headers: { 'Content-Type': 'application/json' },
					body: JSON.stringify({ ids: group, to: to || undefined })
				});
				if (!r.ok) throw new Error(`${r.status} ${(await r.text()) || r.statusText}`);
				const res = await r.json();
				sessions += res.sessions ?? 0;
			}
			msg = `Imported ${sessions} session${sessions === 1 ? '' : 's'}.`;
			clearSelection();
			await loadDiscoverable();
			onimported?.();
		} catch (e) {
			error = `Import failed: ${e instanceof Error ? e.message : String(e)}`;
		} finally {
			busy = false;
		}
	}

	function importSelected() {
		void doImport([...selected]);
	}
	function importAllNew() {
		void doImport(items.filter((i) => i.status === 'new').map((i) => i.id));
	}

	async function skiplist(verb: 'ignore' | 'unignore') {
		const ids = [...selected];
		if (ids.length === 0) return;
		busy = true;
		msg = null;
		try {
			const r = await hubFetch(`/api/sessions/${verb}`, {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ ids })
			});
			if (!r.ok) throw new Error(`${r.status} ${(await r.text()) || r.statusText}`);
			msg = verb === 'ignore' ? `Marked ${ids.length} private.` : `Unmarked ${ids.length}.`;
			clearSelection();
			await loadDiscoverable();
		} catch (e) {
			error = `${verb} failed: ${e instanceof Error ? e.message : String(e)}`;
		} finally {
			busy = false;
		}
	}

	async function saveAutosync(enabled: boolean) {
		autosyncBusy = true;
		try {
			const r = await hubFetch('/api/sessions/autosync', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ enabled, interval_secs: autosyncInterval })
			});
			if (!r.ok) throw new Error(`${r.status}`);
			const v = await r.json();
			autosyncEnabled = !!v.enabled;
			if (typeof v.interval_secs === 'number') autosyncInterval = v.interval_secs;
		} catch (e) {
			error = `Autosync update failed: ${e instanceof Error ? e.message : String(e)}`;
		} finally {
			autosyncBusy = false;
		}
	}

	function close() {
		open = false;
	}
</script>

{#if open}
	<div
		class="backdrop"
		role="button"
		tabindex="0"
		aria-label="Close import panel"
		onclick={close}
		onkeydown={(e) => e.key === 'Escape' && close()}
	></div>
	<div class="panel" role="dialog" aria-modal="true" aria-label="Import sessions">
		<header>
			<h2>Import sessions</h2>
			<button class="x" onclick={close} aria-label="Close">✕</button>
		</header>

		<p class="note">
			Sessions found on this machine. Choose which to import — nothing is imported automatically.
			Extraction of topics &amp; memories runs only when an extraction key is configured on the Hub
			host; otherwise turns &amp; token counts are imported (still fully browsable).
		</p>

		<div class="autosync">
			<label>
				<input
					type="checkbox"
					checked={autosyncEnabled}
					disabled={autosyncBusy}
					onchange={(e) => saveAutosync(e.currentTarget.checked)}
				/>
				Auto-sync new sessions in the background
			</label>
			<span class="every">
				every
				<input
					type="number"
					min="30"
					step="30"
					bind:value={autosyncInterval}
					onchange={() => autosyncEnabled && saveAutosync(true)}
					disabled={autosyncBusy}
				/> s
			</span>
		</div>

		<div class="toolbar">
			<select bind:value={sourceFilter}>
				{#each sources as s (s)}
					<option value={s}>{s}</option>
				{/each}
			</select>
			<label class="chk">
				<input type="checkbox" bind:checked={hideImported} /> hide imported
			</label>
			<input class="search" placeholder="filter project / id…" bind:value={search} />
			<span class="spacer"></span>
			<button onclick={selectNewVisible}>Select new</button>
			<button onclick={selectAllVisible}>Select all</button>
			<button onclick={clearSelection} disabled={selectedCount === 0}>Clear</button>
		</div>

		<div class="bulk">
			<span>{selectedCount} selected</span>
			<span class="spacer"></span>
			<label>
				Assign selected to
				<select bind:value={bulkTarget}>
					<option value="">auto (by repo)</option>
					{#each workspaces as w (w)}
						<option value={w}>{w}</option>
					{/each}
				</select>
			</label>
			<button onclick={applyBulkTarget} disabled={selectedCount === 0}>Apply</button>
		</div>

		{#if error}<p class="error">{error}</p>{/if}
		{#if msg}<p class="ok">{msg}</p>{/if}

		<div class="tablewrap">
			{#if loading}
				<p class="muted">Scanning this machine…</p>
			{:else if visible.length === 0}
				<p class="muted">No sessions match.</p>
			{:else}
				<table>
					<thead>
						<tr>
							<th></th>
							<th>Date</th>
							<th>Source</th>
							<th>Project</th>
							<th>Status</th>
							<th>Workspace</th>
						</tr>
					</thead>
					<tbody>
						{#each visible as it (it.id)}
							<tr class:sel={selected.has(it.id)}>
								<td>
									<input
										type="checkbox"
										checked={selected.has(it.id)}
										onchange={() => toggle(it.id)}
									/>
								</td>
								<td class="date">{fmtDate(it.last_activity)}</td>
								<td>{it.source}</td>
								<td class="proj" title={it.id}>{it.project || shortId(it.id)}</td>
								<td><span class="status {it.status}">{it.status}</span></td>
								<td>
									<select
										value={targets[it.id] ?? ''}
										onchange={(e) =>
											(targets = { ...targets, [it.id]: e.currentTarget.value })}
									>
										<option value="">auto</option>
										{#each workspaces as w (w)}
											<option value={w}>{w}</option>
										{/each}
									</select>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</div>

		<footer>
			<button class="ghost" onclick={() => skiplist('ignore')} disabled={busy || selectedCount === 0}>
				Mark private
			</button>
			<button class="ghost" onclick={() => skiplist('unignore')} disabled={busy || selectedCount === 0}>
				Unmark
			</button>
			<span class="spacer"></span>
			<button onclick={importAllNew} disabled={busy}>Import all new</button>
			<button class="primary" onclick={importSelected} disabled={busy || selectedCount === 0}>
				{busy ? 'Importing…' : `Import ${selectedCount || ''} selected`}
			</button>
		</footer>
	</div>
{/if}

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.45);
		z-index: 40;
	}
	.panel {
		position: fixed;
		top: 0;
		right: 0;
		bottom: 0;
		width: min(760px, 96vw);
		background: var(--lens-bg, #14161a);
		border-left: 1px solid var(--lens-border, #2a2e37);
		z-index: 41;
		display: flex;
		flex-direction: column;
		padding: 1rem 1.25rem;
		box-shadow: -8px 0 24px rgba(0, 0, 0, 0.4);
	}
	header {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}
	header h2 {
		margin: 0;
		font-size: 1.1rem;
	}
	.x {
		margin-left: auto;
		background: none;
		border: none;
		color: var(--lens-muted, #8b93a1);
		font-size: 1rem;
		cursor: pointer;
	}
	.note {
		font-size: 0.8rem;
		color: var(--lens-muted, #8b93a1);
		margin: 0.5rem 0;
		line-height: 1.4;
	}
	.autosync {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		font-size: 0.82rem;
		padding: 0.5rem 0.6rem;
		border: 1px solid var(--lens-border, #2a2e37);
		border-radius: 6px;
		margin-bottom: 0.6rem;
	}
	.autosync .every {
		margin-left: auto;
		color: var(--lens-muted, #8b93a1);
	}
	.autosync input[type='number'] {
		width: 5rem;
	}
	.toolbar,
	.bulk {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin-bottom: 0.5rem;
		flex-wrap: wrap;
	}
	.toolbar .search {
		flex: 1;
		min-width: 8rem;
	}
	.spacer {
		flex: 1;
	}
	.chk {
		display: flex;
		align-items: center;
		gap: 0.25rem;
		font-size: 0.8rem;
		white-space: nowrap;
	}
	.tablewrap {
		flex: 1;
		overflow: auto;
		border: 1px solid var(--lens-border, #2a2e37);
		border-radius: 6px;
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.82rem;
	}
	th,
	td {
		text-align: left;
		padding: 0.35rem 0.5rem;
		border-bottom: 1px solid var(--lens-border, #23262e);
	}
	thead th {
		position: sticky;
		top: 0;
		background: var(--lens-bg-alt, #1a1d23);
		z-index: 1;
	}
	tr.sel {
		background: rgba(90, 130, 255, 0.12);
	}
	.date {
		color: var(--lens-muted, #8b93a1);
		white-space: nowrap;
	}
	.proj {
		max-width: 18rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.status {
		font-size: 0.72rem;
		padding: 0.05rem 0.4rem;
		border-radius: 999px;
		border: 1px solid var(--lens-border, #2a2e37);
	}
	.status.new {
		color: #7ee081;
		border-color: #2f5c33;
	}
	.status.imported {
		color: var(--lens-muted, #8b93a1);
	}
	.status.ignored {
		color: #ffb454;
		border-color: #5c4626;
	}
	footer {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin-top: 0.6rem;
	}
	button {
		background: var(--lens-bg-alt, #1a1d23);
		color: var(--lens-fg, #e6e9ef);
		border: 1px solid var(--lens-border, #2a2e37);
		border-radius: 6px;
		padding: 0.35rem 0.7rem;
		cursor: pointer;
		font-size: 0.82rem;
	}
	button:disabled {
		opacity: 0.5;
		cursor: default;
	}
	button.primary {
		background: var(--lens-accent, #3b6ef5);
		border-color: var(--lens-accent, #3b6ef5);
		color: #fff;
	}
	button.ghost {
		background: none;
	}
	.error {
		color: var(--lens-danger, #ff6b6b);
		font-size: 0.82rem;
		margin: 0.3rem 0;
	}
	.ok {
		color: #7ee081;
		font-size: 0.82rem;
		margin: 0.3rem 0;
	}
	.muted {
		color: var(--lens-muted, #8b93a1);
		padding: 1rem;
	}
</style>
