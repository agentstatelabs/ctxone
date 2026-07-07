<script lang="ts">
	import { FileBrowser } from '@agentstate/lens-core';
	import { codeClient } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';

	let client = $derived(codeClient($selectedRepo));

	function fileHref(path: string): string {
		return `/code/files/${path}`;
	}
</script>

<div class="page">
	<h2>Files</h2>

	{#key client}
		<FileBrowser {client} {fileHref} />
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
