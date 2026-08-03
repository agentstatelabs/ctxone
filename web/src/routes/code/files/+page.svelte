<script lang="ts">
	import { FileBrowser } from '@agentstate/lens-core';
	import { codeClient } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';
	import EmptyState from '$lib/EmptyState.svelte';
	import RepoBadge from '$lib/RepoBadge.svelte';

	let client = $derived(codeClient($selectedRepo));

	function fileHref(path: string): string {
		return `/code/files/${path}`;
	}
</script>

<div class="page">
	<h2>Files {#if $selectedRepo}<RepoBadge />{/if}</h2>

	{#if !$selectedRepo}
		<EmptyState icon="◧" title="No repo selected" description="Pick a repo from the sidebar's ASD section to browse its files." />
	{:else}
		{#key client}
			<FileBrowser {client} {fileHref} />
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
