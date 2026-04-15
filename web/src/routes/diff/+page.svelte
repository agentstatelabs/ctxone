<script lang="ts">
	import { onMount } from 'svelte';
	import { getBranches, getDiff } from '$lib/api';
	import type { DiffOp, DiffResponse } from '$lib/api';
	import { branchStore } from '$lib/branchStore.svelte';

	let branches: string[] = $state(['main']);
	let refA = $state('main');
	let refB = $state('main');
	let diff: DiffResponse | null = $state(null);
	let loading = $state(false);
	let error: string | null = $state(null);

	onMount(async () => {
		try {
			const list = await getBranches();
			branches = list.map((b) => b.name);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load branches';
		}
		// Default: current branch vs main, or two most recent branches
		refA = 'main';
		refB = branchStore.current !== 'main' ? branchStore.current : branches[1] ?? 'main';
	});

	async function runDiff() {
		if (refA === refB) {
			error = 'Pick two different branches';
			diff = null;
			return;
		}
		loading = true;
		error = null;
		try {
			diff = await getDiff(refA, refB);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Diff failed';
			diff = null;
		} finally {
			loading = false;
		}
	}

	function opClass(op: DiffOp): string {
		const kind = op.op.toLowerCase();
		if (kind.includes('add') || kind.includes('insert')) return 'op-add';
		if (kind.includes('remove') || kind.includes('delete')) return 'op-del';
		if (kind.includes('replace') || kind.includes('update')) return 'op-mod';
		return '';
	}

	function formatValue(v: unknown): string {
		if (v === undefined || v === null) return '';
		if (typeof v === 'string') return v;
		try {
			return JSON.stringify(v);
		} catch {
			return String(v);
		}
	}

	function summarizeOps(ops: DiffOp[]): { adds: number; dels: number; mods: number } {
		let adds = 0, dels = 0, mods = 0;
		for (const o of ops) {
			const kind = o.op.toLowerCase();
			if (kind.includes('add') || kind.includes('insert')) adds++;
			else if (kind.includes('remove') || kind.includes('delete')) dels++;
			else if (kind.includes('replace') || kind.includes('update')) mods++;
		}
		return { adds, dels, mods };
	}
</script>

<h2>Diff Branches</h2>

<div class="controls">
	<label>
		From
		<select bind:value={refA}>
			{#each branches as name}
				<option value={name}>{name}</option>
			{/each}
		</select>
	</label>
	<span class="arrow">→</span>
	<label>
		To
		<select bind:value={refB}>
			{#each branches as name}
				<option value={name}>{name}</option>
			{/each}
		</select>
	</label>
	<button type="button" onclick={runDiff} disabled={loading}>
		{loading ? 'Diffing…' : 'Compare'}
	</button>
</div>

{#if error}
	<p class="error">{error}</p>
{/if}

{#if diff}
	{@const summary = summarizeOps(diff.ops)}
	<div class="summary">
		<span class="count"><span class="op-add">+{summary.adds}</span> added</span>
		<span class="count"><span class="op-del">−{summary.dels}</span> removed</span>
		<span class="count"><span class="op-mod">~{summary.mods}</span> modified</span>
		<span class="total">{diff.ops.length} total</span>
	</div>

	{#if diff.ops.length === 0}
		<p class="empty">No differences. {diff.ref_a} and {diff.ref_b} are at the same state.</p>
	{:else}
		<div class="ops">
			{#each diff.ops as op}
				<div class="op {opClass(op)}">
					<span class="op-kind">{op.op}</span>
					<span class="op-path">{op.path}</span>
					{#if op.value !== undefined}
						<pre class="op-value">{formatValue(op.value)}</pre>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
{/if}

<style>
	.controls {
		display: flex;
		align-items: center;
		gap: 1rem;
		margin-bottom: 1.5rem;
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		padding: 1rem 1.25rem;
	}

	.controls label {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: #666;
	}

	.controls select {
		background: #0a0a0a;
		border: 1px solid #333;
		color: #e0e0e0;
		padding: 0.4rem 0.6rem;
		border-radius: 4px;
		font-family: monospace;
		font-size: 0.9rem;
		min-width: 10rem;
	}

	.arrow {
		color: #555;
		font-size: 1.3rem;
		margin-top: 1.1rem;
	}

	.controls button {
		background: #3b82f6;
		border: none;
		color: #fff;
		padding: 0.5rem 1.1rem;
		border-radius: 4px;
		cursor: pointer;
		font-size: 0.9rem;
		margin-top: 1.1rem;
	}

	.controls button:hover:not(:disabled) { background: #2563eb; }
	.controls button:disabled { opacity: 0.5; cursor: not-allowed; }

	.summary {
		display: flex;
		gap: 1.5rem;
		margin-bottom: 1rem;
		padding: 0.6rem 1rem;
		background: #111;
		border: 1px solid #222;
		border-radius: 6px;
		font-size: 0.85rem;
	}

	.count { color: #888; }
	.total { margin-left: auto; color: #555; font-family: monospace; }

	.ops {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		overflow: hidden;
	}

	.op {
		padding: 0.6rem 1rem;
		border-bottom: 1px solid #1a1a1a;
		font-family: monospace;
		font-size: 0.85rem;
	}

	.op:last-child { border-bottom: none; }

	.op-kind {
		display: inline-block;
		min-width: 5rem;
		padding: 0.1rem 0.5rem;
		margin-right: 0.75rem;
		border-radius: 3px;
		font-size: 0.72rem;
		text-transform: uppercase;
		background: #222;
		color: #888;
	}

	.op.op-add .op-kind { background: #14321d; color: #6ee7b7; }
	.op.op-del .op-kind { background: #321414; color: #fca5a5; }
	.op.op-mod .op-kind { background: #322814; color: #fcd34d; }

	.op-path { color: #ccc; }

	.op-value {
		margin: 0.5rem 0 0 5.8rem;
		color: #888;
		font-size: 0.78rem;
		white-space: pre-wrap;
		word-break: break-word;
	}

	.op-add { color: #6ee7b7; }
	.op-del { color: #fca5a5; }
	.op-mod { color: #fcd34d; }

	.error { color: #ef4444; }
	.empty { color: #555; padding: 2rem; text-align: center; }
</style>
