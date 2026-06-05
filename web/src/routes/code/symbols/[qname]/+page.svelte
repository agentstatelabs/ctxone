<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import {
		getSymbolDetail,
		getCallers,
		getCallees,
		getCallGraph,
		getSymbolThinking
	} from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';
	import type {
		SymbolDetail,
		SymbolSummary,
		CallGraphResponse,
		CallGraphNode,
		PriorThinking
	} from '$lib/codeTypes';
	import CallGraph from '$lib/CallGraph.svelte';

	let detail = $state<SymbolDetail | null>(null);
	let callers = $state<SymbolSummary[]>([]);
	let callees = $state<SymbolSummary[]>([]);
	let graph = $state<CallGraphResponse | null>(null);
	let thinking = $state<PriorThinking | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let qname = $derived(decodeURIComponent($page.params.qname ?? ''));

	$effect(() => {
		const q = qname;
		if (!q) return;
		loading = true;
		error = null;
		detail = null;
		callers = [];
		callees = [];
		graph = null;

		Promise.all([
			getSymbolDetail($selectedRepo, q),
			getCallers($selectedRepo, q),
			getCallees($selectedRepo, q),
			getCallGraph($selectedRepo, q, 1),
			// Thinking is best-effort — older asd-serve installs don't have
			// the /thinking route. Swallow errors so the rest of the page
			// still renders.
			getSymbolThinking($selectedRepo, q).catch(() => null)
		])
			.then(([d, c, ce, g, t]) => {
				detail = d;
				callers = c;
				callees = ce;
				graph = g;
				thinking = t;
				loading = false;
			})
			.catch((e) => {
				error = String(e);
				loading = false;
			});
	});

	function handleNodeClick(node: CallGraphNode) {
		if (!node.is_focal) {
			goto(`/code/symbols/${encodeURIComponent(node.qname)}`);
		}
	}

	function ts(s: string): string {
		try {
			return new Date(s).toISOString().replace('T', ' ').slice(0, 19) + 'Z';
		} catch {
			return s;
		}
	}

	function qualStr(q: unknown): string | null {
		if (q == null) return null;
		if (typeof q === 'object' && Object.keys(q as object).length === 0) return null;
		try { return JSON.stringify(q); } catch { return null; }
	}

	const KIND_COLORS: Record<string, string> = {
		decision:   '#88c0d0',
		assumption: '#a3be8c',
		constraint: '#d08770',
		rationale:  '#b48ead',
		hazard:     '#bf616a',
		tradeoff:   '#ebcb8b'
	};
</script>

{#if loading}
	<p class="muted">loading…</p>
{:else if error}
	<p class="error">{error}</p>
{:else if detail}
	{@const s = detail.symbol}
	<div class="sym-header">
		<div class="sym-title">
			<span class="kind kind-{s.kind}">{s.kind}</span>
			<h2>{s.qname}</h2>
		</div>
		<div class="sym-loc">
			<code>{s.file}</code>
			<span class="range">:{s.start.line}–{s.end.line}</span>
			<span class="lang-tag">{s.language}</span>
		</div>
		{#if s.signature}
			<pre class="signature"><code>{s.signature}</code></pre>
		{/if}
		{#if s.doc}
			<p class="doc">{s.doc}</p>
		{/if}
	</div>

	<!-- Inherited thinking (Plan G/K) -->
	{#if thinking && thinking.entries && (thinking.summary.surfaced ?? 0) > 0}
		<section class="thinking-section">
			<h3>
				Inherited thinking
				<span class="count-badge">{thinking.summary.surfaced}</span>
			</h3>
			{#if thinking.entries.hypotheses?.length}
				<div class="thinking-block">
					<h4>Hypotheses</h4>
					<ul>
						{#each thinking.entries.hypotheses as h}
							<li>
								<span class="conf">{h.confidence.toFixed(2)}</span> {h.summary}
							</li>
						{/each}
					</ul>
				</div>
			{/if}
			{#if thinking.entries.mental_models?.length}
				<div class="thinking-block">
					<h4>Mental models</h4>
					<ul>
						{#each thinking.entries.mental_models as m}
							<li>{m.summary}</li>
						{/each}
					</ul>
				</div>
			{/if}
			{#if thinking.entries.open_questions?.length}
				<div class="thinking-block">
					<h4>Open questions</h4>
					<ul>
						{#each thinking.entries.open_questions as q}
							<li>{q.summary}</li>
						{/each}
					</ul>
				</div>
			{/if}
			{#if thinking.entries.failed_attempts?.length}
				<div class="thinking-block">
					<h4>Failed attempts</h4>
					<ul>
						{#each thinking.entries.failed_attempts as f}
							<li>{f.summary}</li>
						{/each}
					</ul>
				</div>
			{/if}
		</section>
	{:else if thinking && (thinking.summary.by_kind_dropped?.hypothesis ?? 0) > 0}
		<section class="thinking-section subtle">
			<p class="thinking-hint">
				{thinking.summary.by_kind_dropped?.hypothesis} hypothesis entries exist
				below the confidence floor — see them on
				<a href="/code/thinking">Thinking</a>.
			</p>
		</section>
	{/if}

	<!-- Mini call graph -->
	{#if graph && (graph.nodes.length > 1 || graph.edges.length > 0)}
		<section class="graph-section">
			<h3>Call Graph <span class="hop-hint">1-hop</span></h3>
			<div class="graph-wrap">
				<CallGraph nodes={graph.nodes} edges={graph.edges} width={700} height={280} onNodeClick={handleNodeClick} />
			</div>
			<p class="graph-hint">Click a node to navigate · <a href="/code/graph?q={encodeURIComponent(s.qname)}">Open in full graph explorer →</a></p>
		</section>
	{/if}

	<!-- Callers / Callees -->
	<div class="call-cols">
		<section>
			<h3>Callers <span class="count-badge">{callers.length}</span></h3>
			{#if callers.length === 0}
				<p class="muted empty">none recorded</p>
			{:else}
				<ul class="sym-list">
					{#each callers as c}
						<li>
							<a href="/code/symbols/{encodeURIComponent(c.qname)}">
								<span class="kind kind-{c.kind}">{c.kind}</span>
								<span class="qname">{c.qname}</span>
							</a>
						</li>
					{/each}
				</ul>
			{/if}
		</section>

		<section>
			<h3>Callees <span class="count-badge">{callees.length}</span></h3>
			{#if callees.length === 0}
				<p class="muted empty">none recorded</p>
			{:else}
				<ul class="sym-list">
					{#each callees as c}
						<li>
							<a href="/code/symbols/{encodeURIComponent(c.qname)}">
								<span class="kind kind-{c.kind}">{c.kind}</span>
								<span class="qname">{c.qname}</span>
							</a>
						</li>
					{/each}
				</ul>
			{/if}
		</section>
	</div>

	<!-- Effects -->
	<section>
		<h3>Effects</h3>
		{#if !detail.effects}
			<p class="muted">no effect record</p>
		{:else}
			{@const ed = detail.effects}
			{#if ed.verification}
				{@const v = ed.verification}
				<div class="verif verif-{v.status}">
					<span class="verif-label">verification</span>
					<span class="verif-by">{v.by}</span>
					<span class="verif-status">{v.status}</span>
					<span class="verif-at">{ts(v.at)}</span>
				</div>
			{/if}
			{#if ed.declared.length === 0}
				<p class="muted">no declared effects</p>
			{:else}
				<ul class="effects">
					{#each ed.declared as eff}
						<li>
							<span class="eff-cat">{eff.effect}</span>
							{#if qualStr(eff.qualifiers)}<code class="qual">{qualStr(eff.qualifiers)}</code>{/if}
							{#if eff.note}<span class="eff-note">{eff.note}</span>{/if}
						</li>
					{/each}
				</ul>
			{/if}
			{#if ed.transitive && ed.transitive.length > 0}
				<h4>Transitive</h4>
				<ul class="effects">
					{#each ed.transitive as t}
						<li>
							<span class="eff-cat">{t.effect}</span>
							<span class="via">via {t.via.join(', ')}</span>
						</li>
					{/each}
				</ul>
			{/if}
		{/if}
	</section>

	<!-- Ledger -->
	<section>
		<h3>Decision Ledger <span class="count-badge">{detail.ledger.length}</span></h3>
		{#if detail.ledger.length === 0}
			<p class="muted">no ledger entries</p>
		{:else}
			<div class="ledger">
				{#each detail.ledger as entry}
					<div class="ledger-entry">
						<div class="entry-header">
							<span class="entry-kind" style="color: {KIND_COLORS[entry.kind] ?? 'var(--text-2)'}">{entry.kind}</span>
							<span class="entry-author">{entry.author.kind}:{entry.author.id}</span>
							<span class="entry-at">{ts(entry.created_at)}</span>
							{#if entry.confidence != null}
								<span class="entry-conf">{Math.round(entry.confidence * 100)}%</span>
							{/if}
						</div>
						<div class="entry-summary">{entry.summary}</div>
						{#if entry.body}
							<div class="entry-body">{entry.body}</div>
						{/if}
						{#if entry.tags && entry.tags.length > 0}
							<div class="entry-tags">
								{#each entry.tags as tag}
									<span class="tag">{tag}</span>
								{/each}
							</div>
						{/if}
					</div>
				{/each}
			</div>
		{/if}
	</section>
{/if}

<style>
	.muted { color: var(--text-3); }
	.error { color: var(--danger); }

	.sym-header {
		margin-bottom: 1.5rem;
	}

	.sym-title {
		display: flex;
		align-items: baseline;
		gap: 0.75rem;
		margin-bottom: 0.4rem;
	}

	.sym-title h2 {
		margin: 0;
		font-family: monospace;
		font-size: 1.1rem;
		word-break: break-all;
	}

	.sym-loc {
		font-size: 0.85rem;
		color: var(--text-3);
		margin-bottom: 0.5rem;
	}

	.range { margin: 0 0.5rem; }

	.lang-tag {
		font-size: 0.75rem;
		padding: 1px 5px;
		background: var(--bg-hover);
		border-radius: 3px;
		color: var(--text-2);
	}

	.signature {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 0.6rem 0.9rem;
		font-size: 0.85rem;
		overflow-x: auto;
		margin: 0.5rem 0;
	}

	.doc {
		font-size: 0.88rem;
		color: var(--text-2);
		margin: 0.4rem 0 0;
	}

	section {
		margin-bottom: 1.75rem;
	}

	h3 {
		font-size: 0.8rem;
		text-transform: uppercase;
		letter-spacing: 0.07em;
		color: var(--text-3);
		margin: 0 0 0.6rem;
	}

	h4 {
		font-size: 0.78rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-3);
		margin: 0.75rem 0 0.4rem;
	}

	.hop-hint {
		font-size: 0.7rem;
		background: var(--bg-hover);
		padding: 1px 5px;
		border-radius: 3px;
		text-transform: none;
		letter-spacing: 0;
	}

	.count-badge {
		font-size: 0.75rem;
		background: var(--bg-hover);
		padding: 1px 5px;
		border-radius: 3px;
		text-transform: none;
		letter-spacing: 0;
		color: var(--text-2);
	}

	.graph-section .graph-wrap {
		overflow-x: auto;
	}

	.thinking-section {
		margin: 1rem 0;
		padding: 0.85rem 1rem;
		border: 1px solid var(--border);
		border-left: 3px solid var(--accent);
		border-radius: 4px;
		background: var(--bg-1);
	}
	.thinking-section.subtle {
		border-left-color: var(--border);
		background: transparent;
	}
	.thinking-section h3 {
		margin: 0 0 0.5rem;
		font-size: 0.95rem;
	}
	.thinking-block {
		margin-top: 0.6rem;
	}
	.thinking-block h4 {
		margin: 0 0 0.25rem;
		font-size: 0.78rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-3);
	}
	.thinking-block ul {
		margin: 0;
		padding-left: 1.1rem;
		font-size: 0.88rem;
	}
	.thinking-block .conf {
		font-family: monospace;
		font-size: 0.75rem;
		color: var(--text-3);
		margin-right: 0.4rem;
	}
	.thinking-hint {
		margin: 0;
		font-size: 0.82rem;
		color: var(--text-2);
	}
	.thinking-hint a {
		color: var(--accent);
	}

	.graph-hint {
		font-size: 0.78rem;
		color: var(--text-3);
		margin: 0.4rem 0 0;
	}

	.graph-hint a {
		color: var(--accent);
	}

	.call-cols {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1.5rem;
		margin-bottom: 1.75rem;
	}

	.sym-list {
		list-style: none;
		margin: 0;
		padding: 0;
	}

	.sym-list li a {
		display: flex;
		align-items: baseline;
		gap: 0.5rem;
		padding: 0.3rem 0.5rem;
		border-radius: 4px;
		color: inherit;
		text-decoration: none;
		font-size: 0.88rem;
	}

	.sym-list li a:hover {
		background: var(--bg-hover);
	}

	.sym-list .qname {
		font-family: monospace;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.empty { font-size: 0.85rem; }

	.verif {
		display: flex;
		gap: 0.75rem;
		align-items: center;
		padding: 0.4rem 0.75rem;
		border-radius: 6px;
		font-size: 0.82rem;
		margin-bottom: 0.6rem;
	}

	.verif-ok { background: rgba(107, 207, 151, 0.1); border: 1px solid rgba(107, 207, 151, 0.3); }
	.verif-mismatch { background: rgba(191, 97, 106, 0.1); border: 1px solid rgba(191, 97, 106, 0.3); }
	.verif-unverified { background: var(--bg-1); border: 1px solid var(--border); }

	.verif-label { color: var(--text-3); text-transform: uppercase; font-size: 0.7rem; letter-spacing: 0.06em; }
	.verif-by, .verif-at { color: var(--text-2); }
	.verif-status { font-weight: 600; }

	.effects {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.3rem;
	}

	.effects li {
		display: flex;
		gap: 0.6rem;
		align-items: baseline;
		font-size: 0.88rem;
		padding: 0.25rem 0;
		border-bottom: 1px solid var(--bg-hover);
	}

	.eff-cat {
		font-family: monospace;
		color: var(--accent);
		white-space: nowrap;
	}

	.qual { color: var(--text-2); font-size: 0.8rem; }
	.eff-note { color: var(--text-3); font-style: italic; }
	.via { color: var(--text-3); font-size: 0.82rem; }

	.ledger {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}

	.ledger-entry {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 0.75rem 1rem;
	}

	.entry-header {
		display: flex;
		gap: 0.75rem;
		align-items: baseline;
		margin-bottom: 0.35rem;
		font-size: 0.8rem;
	}

	.entry-kind {
		font-weight: 600;
		text-transform: uppercase;
		font-size: 0.72rem;
		letter-spacing: 0.05em;
	}

	.entry-author { color: var(--text-3); font-family: monospace; }
	.entry-at { color: var(--text-3); font-family: monospace; margin-left: auto; }
	.entry-conf { color: var(--text-3); font-size: 0.75rem; }

	.entry-summary {
		font-size: 0.9rem;
		color: var(--text-0);
	}

	.entry-body {
		font-size: 0.85rem;
		color: var(--text-2);
		margin-top: 0.35rem;
		white-space: pre-wrap;
	}

	.entry-tags {
		display: flex;
		gap: 0.3rem;
		flex-wrap: wrap;
		margin-top: 0.4rem;
	}

	.tag {
		font-size: 0.72rem;
		padding: 1px 6px;
		background: var(--bg-hover);
		border-radius: 10px;
		color: var(--text-2);
	}

	.kind {
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		flex-shrink: 0;
	}

	.kind-function  { color: var(--kind-function,  #88c0d0); }
	.kind-method    { color: var(--kind-method,    #a3be8c); }
	.kind-class     { color: var(--kind-class,     #d08770); }
	.kind-module    { color: var(--kind-module,    #b48ead); }
	.kind-variable  { color: var(--kind-variable,  #ebcb8b); }
</style>
