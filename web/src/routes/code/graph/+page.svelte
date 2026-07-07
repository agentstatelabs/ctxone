<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { CallGraphView } from '@agentstate/lens-core';
	import { codeClient } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';

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
	<h2>Graph Explorer</h2>

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
</div>

<style>
	.page {
		max-width: 1100px;
	}

	h2 {
		margin: 0 0 1.25rem;
	}
</style>
