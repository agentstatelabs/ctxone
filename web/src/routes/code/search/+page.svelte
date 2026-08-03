<script lang="ts">
	import { page } from '$app/stores';
	import { SymbolSearch } from '@agentstate/lens-core';
	import { codeClient } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';
	import EmptyState from '$lib/EmptyState.svelte';
	import RepoBadge from '$lib/RepoBadge.svelte';

	let client = $derived(codeClient($selectedRepo));

	// Seed the component from the URL; mirror its edits back without navigation.
	const initialQuery = $page.url.searchParams.get('q') ?? '';
	const initialKind = $page.url.searchParams.get('kind') ?? '';
	const initialLanguage = $page.url.searchParams.get('lang') ?? '';

	function syncUrl(p: { q: string; kind: string; language: string }) {
		const params = new URLSearchParams();
		if (p.q) params.set('q', p.q);
		if (p.kind) params.set('kind', p.kind);
		if (p.language) params.set('lang', p.language);
		const qs = params.toString();
		history.replaceState(null, '', qs ? `?${qs}` : location.pathname);
	}

	function symbolHref(qname: string): string {
		return `/code/symbols/${encodeURIComponent(qname)}`;
	}
</script>

<div class="page">
	<h2>Code Search {#if $selectedRepo}<RepoBadge />{/if}</h2>

	{#if !$selectedRepo}
		<EmptyState icon="◧" title="No repo selected" description="Pick a repo from the sidebar's ASD section to search its symbols." />
	{:else}
		{#key client}
			<SymbolSearch
				{client}
				{symbolHref}
				{initialQuery}
				{initialKind}
				{initialLanguage}
				onParamsChange={syncUrl}
			/>
		{/key}
	{/if}
</div>

<style>
	.page {
		max-width: 800px;
	}

	h2 {
		margin: 0 0 1.25rem;
	}
</style>
