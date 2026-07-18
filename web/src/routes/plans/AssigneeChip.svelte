<script lang="ts">
	import { SERIES_COLORS, seriesColor } from '@agentstate/lens-core';
	import { hashString, initials } from './model';

	let {
		assignee,
		showName = false
	}: { assignee: string; showName?: boolean } = $props();

	let hue = $derived(seriesColor(hashString(assignee) % SERIES_COLORS.length));
</script>

<span class="assignee" title="Assigned to {assignee}">
	<span
		class="avatar"
		style:color={hue}
		style:background="color-mix(in srgb, {hue} 14%, transparent)"
		style:border-color="color-mix(in srgb, {hue} 40%, transparent)"
	>{initials(assignee)}</span>
	{#if showName}<span class="name">{assignee}</span>{/if}
</span>

<style>
	.assignee {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		min-width: 0;
	}
	.avatar {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.15rem;
		height: 1.15rem;
		border-radius: var(--lens-radius-full);
		border: 1px solid transparent;
		font-family: var(--lens-font-mono);
		font-size: 0.55rem;
		font-weight: 700;
		letter-spacing: 0.02em;
		flex: none;
	}
	.name {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-secondary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
