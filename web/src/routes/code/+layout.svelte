<script lang="ts">
	import type { Snippet } from 'svelte';
	import EmptyState from '$lib/EmptyState.svelte';
	import MachineLocal from '$lib/MachineLocal.svelte';

	let { children }: { children: Snippet } = $props();
</script>

<!-- The sidebar and palette hide Code on a multi-tenant Hub; this answers a
     link or bookmark that reaches it anyway. -->
<MachineLocal>
	{@render children()}
	{#snippet otherwise()}
		<EmptyState
			icon="🧩"
			title="No code intelligence on this Hub"
			description="This Hub serves many tenants, so it has no ASD repositories of its own to show. Code intelligence comes from a Hub on your own machine."
		/>
	{/snippet}
</MachineLocal>
