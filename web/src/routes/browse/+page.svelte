<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import ScopeBadge from '$lib/ScopeBadge.svelte';
	import BrowsePane from '$lib/BrowsePane.svelte';

	// Deep link (?path=…) from /recall and /why: hand the path to the pane as
	// its initial target so it selects it and expands the ancestors.
	let target: string | null = $state(null);

	onMount(() => {
		target = $page.url.searchParams.get('path');
	});
</script>

<h2>Browse Memory <ScopeBadge branch /></h2>

<BrowsePane bind:target />
