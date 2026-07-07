<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { SymbolDetail } from '@agentstate/lens-core';
	import { codeClient } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';

	let client = $derived(codeClient($selectedRepo));
	let qname = $derived(decodeURIComponent($page.params.qname ?? ''));

	function symbolHref(q: string): string {
		return `/code/symbols/${encodeURIComponent(q)}`;
	}

	function graphHref(q: string): string {
		return `/code/graph?q=${encodeURIComponent(q)}`;
	}
</script>

{#key client}
	<SymbolDetail
		{client}
		{qname}
		{symbolHref}
		{graphHref}
		thinkingHref="/code/thinking"
		onSymbolNavigate={(q) => goto(symbolHref(q))}
	/>
{/key}
