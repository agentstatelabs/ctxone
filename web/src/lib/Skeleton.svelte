<script lang="ts">
	/**
	 * A shimmering placeholder block for first-load states — smoother than a
	 * "Loading…" line. Use several to mimic the shape of the content that's
	 * about to appear (a card, a row). Respects prefers-reduced-motion.
	 */
	let {
		width = '100%',
		height = '1rem',
		radius = 'var(--lens-radius-sm, 4px)'
	}: { width?: string; height?: string; radius?: string } = $props();
</script>

<span class="skeleton" style="width:{width};height:{height};border-radius:{radius}" aria-hidden="true"></span>

<style>
	.skeleton {
		display: block;
		background: linear-gradient(
			90deg,
			var(--lens-surface) 25%,
			var(--lens-surface-raised) 37%,
			var(--lens-surface) 63%
		);
		background-size: 400% 100%;
		animation: skeleton-shimmer 1.4s ease-in-out infinite;
	}
	@keyframes skeleton-shimmer {
		0% {
			background-position: 100% 50%;
		}
		100% {
			background-position: 0 50%;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.skeleton {
			animation: none;
		}
	}
</style>
