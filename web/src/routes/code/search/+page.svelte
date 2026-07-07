<script lang="ts">
	import { page } from '$app/stores';
	import { SymbolSearch } from '@agentstate/lens-core';
	import { codeClient } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';

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
	<h2>Code Search</h2>

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
</div>

<style>
	.page {
		max-width: 800px;
	}

	h2 {
		margin: 0 0 1.25rem;
	}
</style>
