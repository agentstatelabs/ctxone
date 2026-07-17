<script lang="ts">
	import type { Priority } from '$lib/plansApi';
	import { PRIORITY_META } from './model';

	let { priority, compact = false }: { priority: Priority; compact?: boolean } = $props();
	let meta = $derived(PRIORITY_META[priority]);
</script>

<span
	class="chip"
	style:color={meta.color}
	style:background={meta.tint}
	style:border-color={meta.border}
	title="Priority: {meta.label}"
>{compact ? meta.short : meta.label}</span>

<style>
	.chip {
		display: inline-flex;
		align-items: center;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		font-weight: 600;
		letter-spacing: var(--lens-tracking-wide);
		text-transform: uppercase;
		padding: 0.05rem 0.4rem;
		border: 1px solid transparent;
		border-radius: var(--lens-radius-full);
		line-height: 1.5;
		white-space: nowrap;
	}
</style>
