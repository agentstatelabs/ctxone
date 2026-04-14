<script lang="ts">
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();
</script>

<svelte:head>
	<title>CtxOne Lens</title>
</svelte:head>

<div class="app">
	<nav class="sidebar">
		<div class="logo">
			<h1>CtxOne</h1>
			<span class="subtitle">Lens</span>
		</div>
		<ul>
			<li><a href="/">Dashboard</a></li>
			<li><a href="/browse">Browse</a></li>
			<li><a href="/search">Search</a></li>
			<li><a href="/history">History</a></li>
		</ul>
	</nav>
	<main>
		{@render children()}
	</main>
</div>

<style>
	:global(body) {
		margin: 0;
		font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
		background: #0a0a0a;
		color: #e0e0e0;
	}

	.app {
		display: flex;
		min-height: 100vh;
	}

	.sidebar {
		width: 220px;
		background: #111;
		border-right: 1px solid #222;
		padding: 1.5rem 1rem;
		flex-shrink: 0;
	}

	.logo h1 {
		margin: 0;
		font-size: 1.4rem;
		color: #fff;
	}

	.subtitle {
		font-size: 0.75rem;
		color: #666;
		text-transform: uppercase;
		letter-spacing: 0.1em;
	}

	ul {
		list-style: none;
		padding: 0;
		margin-top: 2rem;
	}

	li {
		margin-bottom: 0.5rem;
	}

	a {
		color: #888;
		text-decoration: none;
		padding: 0.5rem 0.75rem;
		display: block;
		border-radius: 6px;
		transition: all 0.15s;
	}

	a:hover {
		color: #fff;
		background: #1a1a1a;
	}

	main {
		flex: 1;
		padding: 2rem;
		overflow-y: auto;
	}
</style>
