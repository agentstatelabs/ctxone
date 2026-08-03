<script lang="ts">
	import type { Snippet } from 'svelte';

	/**
	 * One consistent empty / first-run / error state for collection views,
	 * built on the shared `.lens-state` tokens in app.css. Replaces the ad-hoc
	 * `<p class="muted">Nothing yet</p>` scattered across pages so a fresh hub
	 * reads as guided rather than broken.
	 */
	interface Props {
		/** Optional leading emoji glyph. */
		icon?: string;
		title: string;
		description?: string;
		/** Optional primary action — a link (actionHref) or a callback (onAction). */
		actionLabel?: string;
		actionHref?: string;
		onAction?: () => void;
		tone?: 'empty' | 'error';
		/** Extra inline content (e.g. CLI hints) rendered under the description. */
		children?: Snippet;
	}
	let {
		icon,
		title,
		description,
		actionLabel,
		actionHref,
		onAction,
		tone = 'empty',
		children
	}: Props = $props();
</script>

<div class="lens-state" class:lens-state--error={tone === 'error'} role={tone === 'error' ? 'alert' : undefined}>
	{#if icon}<div class="es-icon" aria-hidden="true">{icon}</div>{/if}
	<div class="lens-state__title">{title}</div>
	{#if description}<p class="es-desc">{description}</p>{/if}
	{#if children}<div class="es-extra">{@render children()}</div>{/if}
	{#if actionLabel && actionHref}
		<a class="es-action" href={actionHref}>{actionLabel}</a>
	{:else if actionLabel && onAction}
		<button type="button" class="es-action" onclick={onAction}>{actionLabel}</button>
	{/if}
</div>

<style>
	.es-icon {
		font-size: 2rem;
		line-height: 1;
		opacity: 0.85;
	}
	.es-desc {
		color: var(--lens-muted);
		font-size: var(--lens-font-size-sm);
		max-width: 46ch;
		margin: 0;
		text-align: center;
		line-height: 1.5;
	}
	.es-extra {
		font-size: var(--lens-font-size-sm);
		color: var(--lens-text-secondary);
		text-align: center;
	}
	.es-action {
		margin-top: var(--lens-space-2);
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		text-decoration: none;
		font-size: var(--lens-font-size-sm);
		font-weight: 600;
		color: var(--lens-text-strong);
		background: var(--lens-accent-surface);
		border: 1px solid var(--lens-accent-border, var(--lens-border-strong));
		border-radius: var(--lens-radius-md);
		padding: 0.4rem 0.8rem;
		cursor: pointer;
	}
	.es-action:hover {
		background: var(--lens-accent-surface-hi, var(--lens-accent-surface));
		border-color: var(--lens-accent);
	}
</style>
