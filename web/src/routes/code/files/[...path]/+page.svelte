<script lang="ts">
	import { page } from '$app/stores';
	import { FileView } from '@agentstate/lens-core';
	import { codeClient } from '$lib/codeApi';
	import { selectedRepo } from '$lib/repoStore';

	let client = $derived(codeClient($selectedRepo));
	let filePath = $derived($page.params.path ?? '');

	function symbolHref(qname: string): string {
		return `/code/symbols/${encodeURIComponent(qname)}`;
	}

	function fileHref(path: string): string {
		return `/code/files/${path}`;
	}
</script>

<div class="page">
	{#key client}
		<FileView {client} path={filePath} {symbolHref} filesHref="/code/files" {fileHref} />
	{/key}
</div>

<style>
	.page {
		max-width: 1000px;
	}
</style>
