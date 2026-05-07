<script lang="ts">
	import { page } from '$app/stores';
	import { readFile, getSymbolsByFile } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';
	import type { SymbolSummary } from '$lib/codeTypes';
	import { goto } from '$app/navigation';

	let filePath = $derived($page.params.path ?? '');
	let source = $state<string | null>(null);
	let symbols = $state<SymbolSummary[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	$effect(() => {
		const p = filePath;
		if (!p) return;
		loading = true;
		error = null;
		source = null;
		symbols = [];

		Promise.all([readFile($selectedRepo, p), getSymbolsByFile($selectedRepo, p)])
			.then(([src, syms]) => {
				source = src;
				symbols = syms.sort((a, b) => a.start.line - b.start.line);
				loading = false;
			})
			.catch((e) => {
				error = e instanceof Error ? e.message : String(e);
				loading = false;
			});
	});

	// Map line number → symbol for annotation
	let lineStartSymbols = $derived(
		new Map(symbols.map((s) => [s.start.line, s]))
	);

	let lines = $derived(source?.split('\n') ?? []);

	const KIND_BORDER: Record<string, string> = {
		function: '#88c0d0',
		method:   '#a3be8c',
		class:    '#d08770',
		module:   '#b48ead',
		variable: '#ebcb8b'
	};

	// Which line ranges are highlighted (symbol spans)
	function lineInSymbol(lineNo: number): SymbolSummary | null {
		for (const s of symbols) {
			if (lineNo >= s.start.line && lineNo <= s.end.line) return s;
		}
		return null;
	}

	let lang = $derived(symbols[0]?.language ?? filePath.split('.').pop() ?? '');
</script>

<div class="page">
	<div class="file-header">
		<div class="breadcrumb">
			<a href="/code/files">Files</a>
			<span class="sep">/</span>
			{#each filePath.split('/') as part, i}
				{#if i < filePath.split('/').length - 1}
					<a href="/code/files/{filePath.split('/').slice(0, i + 1).join('/')}">{part}</a>
					<span class="sep">/</span>
				{:else}
					<span class="fname">{part}</span>
				{/if}
			{/each}
		</div>
		<div class="file-meta">
			{#if lang}<span class="lang-tag">{lang}</span>{/if}
			{#if symbols.length > 0}<span class="sym-count">{symbols.length} symbols</span>{/if}
		</div>
	</div>

	{#if loading}
		<p class="muted">loading…</p>
	{:else if error}
		<p class="error">{error}</p>
		<p class="muted">Make sure the file path is accessible from the ASD server process.</p>
	{:else if source !== null}
		<div class="source-view">
			{#each lines as line, i}
				{@const lineNo = i + 1}
				{@const sym = lineStartSymbols.get(lineNo)}
				{@const inSym = lineInSymbol(lineNo)}
				<div
					class="source-line"
					class:sym-start={sym != null}
					class:in-sym={inSym != null && sym == null}
					style={inSym ? `border-left-color: ${KIND_BORDER[inSym.kind] ?? 'transparent'}` : ''}
				>
					<span class="line-no">{lineNo}</span>
					{#if sym}
						<button
							class="sym-anchor"
							style="color: {KIND_BORDER[sym.kind] ?? 'var(--text-3)'}"
							onclick={() => goto(`/code/symbols/${encodeURIComponent(sym.qname)}`)}
							title={sym.qname}
						>
							{sym.kind}
						</button>
					{:else}
						<span class="sym-placeholder"></span>
					{/if}
					<code class="line-code">{line || ' '}</code>
				</div>
			{/each}
		</div>
	{/if}
</div>

<style>
	.page {
		max-width: 1000px;
	}

	.file-header {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		margin-bottom: 1rem;
		gap: 1rem;
	}

	.breadcrumb {
		display: flex;
		align-items: center;
		gap: 0.2rem;
		font-size: 0.9rem;
		flex-wrap: wrap;
	}

	.breadcrumb a {
		color: var(--accent);
		text-decoration: none;
	}

	.breadcrumb a:hover {
		text-decoration: underline;
	}

	.sep { color: var(--text-3); }

	.fname {
		font-family: monospace;
		color: var(--text-0);
		font-weight: 600;
	}

	.file-meta {
		display: flex;
		gap: 0.5rem;
		align-items: center;
		flex-shrink: 0;
	}

	.lang-tag, .sym-count {
		font-size: 0.75rem;
		padding: 1px 6px;
		background: var(--bg-hover);
		border-radius: 3px;
		color: var(--text-2);
	}

	.muted { color: var(--text-3); }
	.error { color: var(--danger); }

	.source-view {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow-x: auto;
		font-size: 0.82rem;
	}

	.source-line {
		display: flex;
		align-items: stretch;
		border-left: 3px solid transparent;
		min-height: 1.4rem;
	}

	.source-line:hover {
		background: var(--bg-hover);
	}

	.source-line.in-sym {
		border-left-width: 3px;
		border-left-style: solid;
		background: rgba(255,255,255,0.02);
	}

	.source-line.sym-start {
		border-left-width: 3px;
		border-left-style: solid;
	}

	.line-no {
		display: inline-block;
		min-width: 44px;
		text-align: right;
		padding: 0 0.6rem 0 0.5rem;
		color: var(--text-3);
		font-family: monospace;
		user-select: none;
		flex-shrink: 0;
		line-height: 1.5rem;
	}

	.sym-anchor {
		display: inline-block;
		width: 52px;
		flex-shrink: 0;
		background: none;
		border: none;
		padding: 0 0.3rem;
		font-size: 0.68rem;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		cursor: pointer;
		text-align: left;
		line-height: 1.5rem;
	}

	.sym-anchor:hover {
		text-decoration: underline;
	}

	.sym-placeholder {
		display: inline-block;
		width: 52px;
		flex-shrink: 0;
	}

	.line-code {
		display: block;
		white-space: pre;
		flex: 1;
		padding: 0 0.5rem 0 0;
		line-height: 1.5rem;
		color: var(--text-1);
	}
</style>
