<script lang="ts">
	import { onMount } from 'svelte';
	import { getAsdHealth, getSymbols, listFiles } from '$lib/codeApi';
	import type { AsdHealth, FileEntry, SymbolSummary } from '$lib/codeTypes';

	let health = $state<AsdHealth | null>(null);
	let symbols = $state<SymbolSummary[]>([]);
	let files = $state<FileEntry[]>([]);
	let error = $state<string | null>(null);
	let loading = $state(true);

	onMount(async () => {
		try {
			[health, symbols, files] = await Promise.all([
				getAsdHealth(),
				getSymbols(),
				listFiles()
			]);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	});

	type KindCounts = Record<string, number>;
	type LangCounts = Record<string, number>;

	let byKind = $derived<KindCounts>(
		symbols.reduce((acc, s) => {
			acc[s.kind] = (acc[s.kind] ?? 0) + 1;
			return acc;
		}, {} as KindCounts)
	);

	let byLang = $derived<LangCounts>(
		symbols.reduce((acc, s) => {
			acc[s.language] = (acc[s.language] ?? 0) + 1;
			return acc;
		}, {} as LangCounts)
	);

	let topFiles = $derived(
		[...files].sort((a, b) => b.symbol_count - a.symbol_count).slice(0, 8)
	);

	const KIND_ORDER = ['function', 'method', 'class', 'module', 'variable'];
	let kindRows = $derived(
		KIND_ORDER.filter((k) => byKind[k] > 0).map((k) => ({ kind: k, count: byKind[k] }))
	);
	let langRows = $derived(
		Object.entries(byLang)
			.sort((a, b) => b[1] - a[1])
			.slice(0, 8)
	);
</script>

<div class="page">
	<h2>Code Overview</h2>

	{#if loading}
		<p class="muted">loading…</p>
	{:else if error || !health}
		<div class="offline-card">
			<div class="offline-title">ASD server unreachable</div>
			<p class="muted">
				Start it with <code>asd-serve</code> (default port 8787), then refresh.
			</p>
		</div>
	{:else}
		<div class="status-row">
			<span class="dot connected"></span>
			<span class="status-text">Connected · <code>{health.db_path}</code></span>
		</div>

		<div class="stats-grid">
			<div class="stat-card">
				<div class="stat-value">{health.symbol_count.toLocaleString()}</div>
				<div class="stat-label">Symbols</div>
			</div>
			<div class="stat-card">
				<div class="stat-value">{files.length.toLocaleString()}</div>
				<div class="stat-label">Files</div>
			</div>
			<div class="stat-card">
				<div class="stat-value">{Object.keys(byLang).length}</div>
				<div class="stat-label">Languages</div>
			</div>
			<div class="stat-card">
				<div class="stat-value">{Object.keys(byKind).length}</div>
				<div class="stat-label">Symbol kinds</div>
			</div>
		</div>

		<div class="two-col">
			<div class="breakdown-card">
				<h3>By kind</h3>
				<table>
					<tbody>
						{#each kindRows as row}
							<tr>
								<td><span class="kind-badge kind-{row.kind}">{row.kind}</span></td>
								<td class="count">{row.count.toLocaleString()}</td>
								<td class="bar-cell">
									<div
										class="bar"
										style="width: {Math.round((row.count / health.symbol_count) * 100)}%"
									></div>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>

			<div class="breakdown-card">
				<h3>By language</h3>
				<table>
					<tbody>
						{#each langRows as [lang, count]}
							<tr>
								<td class="lang">{lang}</td>
								<td class="count">{count.toLocaleString()}</td>
								<td class="bar-cell">
									<div
										class="bar"
										style="width: {Math.round((count / health.symbol_count) * 100)}%"
									></div>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</div>

		{#if topFiles.length > 0}
			<div class="files-card">
				<h3>Top files by symbol count <a class="see-all" href="/code/files">see all</a></h3>
				<ul class="file-list">
					{#each topFiles as f}
						<li>
							<a href="/code/files/{f.path}">
								<span class="file-path">{f.path}</span>
								<span class="file-meta">{f.language} · {f.symbol_count} symbols</span>
							</a>
						</li>
					{/each}
				</ul>
			</div>
		{/if}
	{/if}
</div>

<style>
	.page {
		max-width: 900px;
	}

	h2 {
		margin: 0 0 1.5rem;
	}

	.muted {
		color: var(--text-3);
	}

	.offline-card {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1.25rem 1.5rem;
	}

	.offline-title {
		color: var(--danger);
		font-weight: 600;
		margin-bottom: 0.5rem;
	}

	.status-row {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin-bottom: 1.5rem;
		font-size: 0.88rem;
		color: var(--text-2);
	}

	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex-shrink: 0;
	}

	.dot.connected {
		background: var(--success, #6fcf97);
	}

	.stats-grid {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: 1rem;
		margin-bottom: 1.5rem;
	}

	.stat-card {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1.25rem;
		text-align: center;
	}

	.stat-value {
		font-size: 2rem;
		font-weight: 700;
		color: var(--text-0);
	}

	.stat-label {
		font-size: 0.75rem;
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		margin-top: 0.2rem;
	}

	.two-col {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1rem;
		margin-bottom: 1.5rem;
	}

	.breakdown-card {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1rem 1.25rem;
	}

	.breakdown-card h3 {
		margin: 0 0 0.75rem;
		font-size: 0.8rem;
		text-transform: uppercase;
		letter-spacing: 0.07em;
		color: var(--text-3);
	}

	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.88rem;
	}

	td {
		padding: 0.3rem 0.4rem 0.3rem 0;
		vertical-align: middle;
	}

	.count {
		font-family: monospace;
		color: var(--text-1);
		text-align: right;
		padding-right: 0.75rem;
		white-space: nowrap;
	}

	.lang {
		color: var(--text-1);
	}

	.bar-cell {
		width: 100%;
	}

	.bar {
		height: 6px;
		background: var(--accent);
		border-radius: 3px;
		min-width: 2px;
		opacity: 0.6;
	}

	.kind-badge {
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		padding: 1px 5px;
		border-radius: 3px;
	}

	.kind-function  { color: var(--kind-function,  #88c0d0); }
	.kind-method    { color: var(--kind-method,    #a3be8c); }
	.kind-class     { color: var(--kind-class,     #d08770); }
	.kind-module    { color: var(--kind-module,    #b48ead); }
	.kind-variable  { color: var(--kind-variable,  #ebcb8b); }

	.files-card {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1rem 1.25rem;
	}

	.files-card h3 {
		margin: 0 0 0.75rem;
		font-size: 0.8rem;
		text-transform: uppercase;
		letter-spacing: 0.07em;
		color: var(--text-3);
	}

	.see-all {
		font-size: 0.75rem;
		color: var(--text-3);
		font-weight: normal;
		margin-left: 0.5rem;
	}

	.see-all:hover {
		color: var(--accent);
	}

	.file-list {
		list-style: none;
		margin: 0;
		padding: 0;
	}

	.file-list li a {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		padding: 0.4rem 0;
		border-bottom: 1px solid var(--bg-hover);
		color: inherit;
		text-decoration: none;
		font-size: 0.88rem;
		gap: 1rem;
	}

	.file-list li:last-child a {
		border-bottom: none;
	}

	.file-list a:hover .file-path {
		color: var(--accent);
	}

	.file-path {
		font-family: monospace;
		color: var(--text-1);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.file-meta {
		color: var(--text-3);
		font-size: 0.8rem;
		white-space: nowrap;
	}
</style>
