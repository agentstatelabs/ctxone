<script lang="ts">
	import { ThinkingList } from '@agentstate/lens-core';
	import { codeClient } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';

	let client = $derived(codeClient($selectedRepo));
</script>

<div class="page">
	<header class="page-head">
		<h2>Thinking</h2>
		<p class="muted">
			Hypotheses, mental models, open questions, and failed attempts captured by
			<code>asd think</code>. Below the confidence floor, Hypotheses are suppressed — slide it down
			to inspect them.
		</p>
	</header>

	{#if !$selectedRepo}
		<div class="card">
			<strong>No repo selected.</strong> Pick one from the sidebar's
			<strong>ASD</strong> section.
		</div>
	{:else}
		{#key client}
			<ThinkingList {client} />
		{/key}
	{/if}
</div>

<style>
	.page {
		max-width: 1100px;
	}
	.page-head h2 {
		margin: 0 0 0.25rem;
	}
	.muted {
		color: var(--text-2);
	}
	.card {
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 1rem;
		background: var(--bg-1);
	}
</style>
