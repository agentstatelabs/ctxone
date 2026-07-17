<!--
	Panel — shared dashboard card shell: surface + border, an uppercase
	eyebrow title with optional link-throughs, and the four load states
	(loading / error / empty / ready) rendered with the shared .lens-state
	vocabulary from app.css. Each panel owns its own state so one failing
	endpoint never blanks the rest of the dashboard.
-->
<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		links = [],
		status = 'ready',
		errorText = '',
		emptyTitle = 'Nothing yet',
		emptyText = '',
		children
	}: {
		title: string;
		/** Link-throughs rendered in the header (e.g. Plans →). */
		links?: Array<{ href: string; label: string }>;
		status?: 'loading' | 'error' | 'empty' | 'ready';
		errorText?: string;
		emptyTitle?: string;
		emptyText?: string;
		children: Snippet;
	} = $props();
</script>

<section class="panel">
	<header class="panel-head">
		<h3>{title}</h3>
		{#if links.length > 0}
			<span class="panel-links">
				{#each links as l (l.href)}
					<a href={l.href}>{l.label} →</a>
				{/each}
			</span>
		{/if}
	</header>

	{#if status === 'loading'}
		<div class="lens-state">Loading…</div>
	{:else if status === 'error'}
		<div class="lens-state lens-state--error">
			<span class="lens-state__title">Unavailable</span>
			<span>{errorText}</span>
		</div>
	{:else if status === 'empty'}
		<div class="lens-state">
			<span class="lens-state__title">{emptyTitle}</span>
			{#if emptyText}<span>{emptyText}</span>{/if}
		</div>
	{:else}
		{@render children()}
	{/if}
</section>

<style>
	.panel {
		background: var(--lens-surface);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-lg, 12px);
		padding: var(--lens-space-4) var(--lens-space-5);
		min-width: 0;
	}

	.panel-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--lens-space-3);
		margin-bottom: var(--lens-space-4);
	}

	.panel-head h3 {
		margin: 0;
		font-size: var(--lens-font-size-2xs);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}

	.panel-links {
		display: flex;
		gap: var(--lens-space-3);
		flex-shrink: 0;
	}

	.panel-links a {
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-secondary);
		text-decoration: none;
		transition: color var(--lens-dur-fast, 120ms) var(--lens-ease, ease);
	}

	.panel-links a:hover {
		color: var(--lens-accent);
	}
</style>
