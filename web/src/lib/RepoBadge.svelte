<script lang="ts">
	import { selectedRepo } from './repoStore';

	/**
	 * Scope indicator for the Code (ASD) views — the analogue of ScopeBadge, but
	 * for the repo axis (which is orthogonal to the workspace/branch switcher).
	 * The active repo is otherwise only visible in the sidebar picker, so a
	 * drill-down code view gives no in-content signal of which repo it's showing.
	 */
	let { health }: { health?: 'running' | 'idle' | 'error' | null } = $props();
</script>

<span class="repo-badge" title="Active code-intelligence repo">
	<span class="glyph" aria-hidden="true">◧</span>
	<span class="repo">{$selectedRepo || 'no repo'}</span>
	{#if health}<span class="dot {health}" title={health}></span>{/if}
</span>

<style>
	.repo-badge {
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
	.glyph {
		opacity: 0.6;
	}
	.repo {
		color: var(--lens-text);
	}
	.dot {
		width: 7px;
		height: 7px;
		border-radius: var(--lens-radius-full);
		background: var(--lens-muted);
	}
	.dot.running {
		background: var(--lens-ok);
	}
	.dot.error {
		background: var(--lens-danger);
	}
</style>
