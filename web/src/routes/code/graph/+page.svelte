<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { CallGraphView } from '@agentstate/lens-core';
	import { codeClient } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';
	import EmptyState from '$lib/EmptyState.svelte';
	import RepoBadge from '$lib/RepoBadge.svelte';

	let client = $derived(codeClient($selectedRepo));

	// Seed from ?q= and keep the URL in sync as the user explores.
	let initialQuery = $derived($page.url.searchParams.get('q') ?? '');

	function syncUrl(qname: string) {
		const params = new URLSearchParams({ q: qname });
		history.replaceState(null, '', `?${params}`);
	}

	function symbolHref(qname: string): string {
		return `/code/symbols/${encodeURIComponent(qname)}`;
	}
</script>

<div class="page">
	<h2>Graph Explorer {#if $selectedRepo}<RepoBadge />{/if}</h2>

	{#if !$selectedRepo}
		<EmptyState icon="◧" title="No repo selected" description="Pick a repo from the sidebar's ASD section to explore its call graph." />
	{:else}
		{#key client}
			<CallGraphView
				{client}
				{symbolHref}
				onViewSymbol={(q) => goto(symbolHref(q))}
				searchHref="/code/search"
				{initialQuery}
				onQueryChange={syncUrl}
			/>
		{/key}
	{/if}
</div>

<style>
	.page {
		max-width: 1100px;
	}

	h2 {
		margin: 0 0 1.25rem;
	}
</style>
