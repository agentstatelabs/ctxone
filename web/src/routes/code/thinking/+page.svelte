<script lang="ts">
	import { ThinkingList } from '@agentstate/lens-core';
	import { codeClient } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';
	import EmptyState from '$lib/EmptyState.svelte';
	import RepoBadge from '$lib/RepoBadge.svelte';

	let client = $derived(codeClient($selectedRepo));
</script>

<div class="page">
	<header class="page-head">
		<h2>Thinking {#if $selectedRepo}<RepoBadge />{/if}</h2>
		<p class="muted">
			Hypotheses, mental models, open questions, and failed attempts captured by
			<code>asd think</code>. Below the confidence floor, Hypotheses are suppressed — slide it down
			to inspect them.
		</p>
	</header>

	{#if !$selectedRepo}
		<EmptyState icon="◧" title="No repo selected" description="Pick a repo from the sidebar's ASD section to inspect its thinking log." />
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
</style>
