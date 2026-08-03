<script lang="ts">
	import { untrack } from 'svelte';
	import { recall, type RecallResponse } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import ScopeBadge from '$lib/ScopeBadge.svelte';

	let topic = $state('');
	let budget = $state(1500);
	let response: RecallResponse | null = $state(null);
	let searched = $state(false);
	let error: string | null = $state(null);

	async function runRecall() {
		const t = topic.trim();
		if (!t) return;
		searched = true;
		error = null;
		try {
			response = await recall(t, budget, branchStore.current);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Recall failed';
			response = null;
		}
	}

	function handleSubmit(e: Event) {
		e.preventDefault();
		void runRecall();
	}

	// Re-run the last recall when the branch or namespace changes so
	// results always reflect the active scope. `topic`/`searched` are
	// read untracked so typing doesn't re-trigger the effect.
	$effect(() => {
		void branchStore.current;
		void namespaceStore.current;
		untrack(() => {
			if (searched && topic.trim()) void runRecall();
		});
	});
</script>

<h2>Recall <ScopeBadge branch /></h2>
<p class="hint">
	The same budgeted, pinned-first retrieval agents get from <code>ctx recall</code> — pinned
	memories load first, then topic matches until the token budget runs out.
</p>

<form onsubmit={handleSubmit} class="search-form">
	<input
		type="text"
		bind:value={topic}
		placeholder="Recall a topic... (e.g., licensing decision, deploy checklist)"
	/>
	<label class="budget-label">
		budget
		<select bind:value={budget}>
			<option value={500}>500 tokens</option>
			<option value={1500}>1500 tokens</option>
			<option value={5000}>5000 tokens</option>
		</select>
	</label>
	<button type="submit">Recall</button>
</form>

{#if error}
	<p class="error">{error}</p>
{/if}

{#if response}
	<p class="count">
		{response.results.length} result{response.results.length !== 1 ? 's' : ''}
		({response.pinned_count} pinned, {response.topic_matches} topic match{response.topic_matches !==
		1
			? 'es'
			: ''})
		· {response.ctx_tokens_sent} tokens sent
		{#if response.ctx_savings_ratio > 0}
			· {response.ctx_savings_ratio.toFixed(1)}× smaller than the flat graph
		{/if}
	</p>

	{#if response.results.length === 0}
		<p class="muted">Nothing recalled for “{response.topic}”.</p>
	{:else}
		<div class="results">
			{#each response.results as r}
				<div class="result">
					<div class="result-head">
						<a class="result-path" href={`/browse?path=${encodeURIComponent(r.path)}`}>
							{r.path}
						</a>
						{#if r.pinned}
							<span class="tag pinned">pinned</span>
						{:else}
							{#if r.full_match}
								<span class="tag full">exact phrase</span>
							{/if}
							{#if r.score !== undefined}
								<span class="tag score" title="matched query tokens">score {r.score}</span>
							{/if}
						{/if}
					</div>
					{#if r.pinned}
						{#if r.title}<div class="result-title">{r.title}</div>{/if}
						<div class="result-value">{r.body}</div>
					{:else}
						<div class="result-value">{r.value}</div>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
{:else if searched && !error}
	<p class="muted">Recalling…</p>
{/if}

<style>
	.hint {
		color: var(--text-3);
		font-size: 0.85rem;
		margin: 0 0 1rem;
	}
	.hint code {
		font-family: monospace;
		color: var(--text-2);
	}

	.search-form {
		display: flex;
		gap: 0.5rem;
		margin-bottom: 1.5rem;
		align-items: center;
	}

	input {
		flex: 1;
		padding: 0.75rem 1rem;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text-0);
		font-size: 1rem;
	}

	input:focus {
		outline: none;
		border-color: var(--border-hi);
	}

	.budget-label {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		font-size: 0.8rem;
		color: var(--text-2);
		white-space: nowrap;
	}

	select {
		background: var(--bg-1);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.5rem;
		border-radius: 6px;
	}

	button {
		padding: 0.75rem 1.5rem;
		background: var(--accent);
		border: none;
		border-radius: 6px;
		color: var(--text-0);
		cursor: pointer;
		font-size: 1rem;
	}

	button:hover {
		background: color-mix(in srgb, var(--accent) 80%, black);
	}

	.count {
		color: var(--text-3);
		margin-bottom: 1rem;
	}
	.error {
		color: var(--danger);
	}
	.muted {
		color: var(--text-3);
	}


	.results {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow: hidden;
	}

	.result {
		padding: 0.75rem 1rem;
		border-bottom: 1px solid var(--bg-hover);
	}

	.result:last-child {
		border-bottom: none;
	}

	.result-head {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin-bottom: 0.25rem;
	}

	.result-path {
		font-family: monospace;
		font-size: 0.8rem;
		color: var(--accent);
		text-decoration: none;
	}
	.result-path:hover {
		text-decoration: underline;
	}

	.tag {
		font-size: 0.65rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		padding: 0.05rem 0.4rem;
		border-radius: 3px;
		border: 1px solid var(--border);
		color: var(--text-3);
	}
	.tag.pinned {
		color: var(--accent);
		background: var(--accent-bg);
		border-color: var(--accent-bg-hi);
	}
	.tag.full {
		color: var(--accent);
	}

	.result-title {
		color: var(--text-0);
		font-size: 0.9rem;
		font-weight: 600;
		margin-bottom: 0.15rem;
	}

	.result-value {
		color: var(--text-1);
		font-size: 0.9rem;
		white-space: pre-wrap;
	}
</style>
