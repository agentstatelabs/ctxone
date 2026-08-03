<script lang="ts">
	import { namespaceStore } from './namespaceStore.svelte';
	import { branchStore } from './branchStore.svelte';

	/**
	 * A consistent scope indicator for drill-down views, so it is always clear
	 * WHICH workspace (and, for branch-scoped views, which branch) the data on
	 * screen belongs to. Branch-scoped pages silently re-query when the branch
	 * switcher changes — without this, an empty result reads as "collection
	 * stopped" rather than "this branch is empty".
	 *
	 * `branch` — include the current branch (use on branch-scoped views like
	 * plans, browse, history, search, recall, tail, diff). Omit for
	 * namespace-scoped views that are branch-agnostic.
	 */
	let { branch = false }: { branch?: boolean } = $props();
</script>

<span
	class="scope-badge"
	title={branch
		? `Scope: workspace “${namespaceStore.current}”, branch “${branchStore.current}”`
		: `Scope: workspace “${namespaceStore.current}” (all branches)`}
>
	<span class="seg ws">{namespaceStore.current}</span>
	{#if branch}
		<span class="sep">·</span>
		<span class="seg br">⎇ {branchStore.current}</span>
	{/if}
</span>

<style>
	.scope-badge {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		font-size: var(--lens-font-size-xs);
		font-family: var(--lens-font-mono);
		color: var(--lens-text-secondary);
		background: var(--lens-overlay);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-full);
		padding: 0.1rem 0.55rem;
		vertical-align: middle;
	}
	.seg.ws {
		color: var(--lens-text);
	}
	.seg.br {
		color: var(--lens-accent);
	}
	.sep {
		opacity: 0.5;
	}
</style>
