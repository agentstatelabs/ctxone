<script lang="ts">
	/**
	 * Shows its children only on a Hub that serves one tenant. They act on the
	 * Hub's own machine (its ASD repos, its agents' transcripts), which a
	 * multi-tenant Hub has none of for its tenants: its routes for them answer
	 * 404. `otherwise`, when given, shows on a multi-tenant Hub instead.
	 */
	import type { Snippet } from 'svelte';
	import { authStore } from './authStore.svelte';

	let { children, otherwise }: { children: Snippet; otherwise?: Snippet } = $props();
</script>

{#if !authStore.multiTenant}
	{@render children()}
{:else if otherwise}
	{@render otherwise()}
{/if}
