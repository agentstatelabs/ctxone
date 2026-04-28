<script lang="ts">
	/**
	 * Command palette overlay (Cmd-K / Ctrl-K).
	 *
	 * Indexes four sources on open and exposes them as a single
	 * fuzzy-matched, keyboard-navigable list:
	 *   - Routes  (static sidebar nav — fastest path to any page)
	 *   - Plans   (id + title, scoped to the active branch)
	 *   - Paths   (top-level keys from /api/paths, active branch)
	 *   - Branches (every known branch — picking switches branchStore)
	 *
	 * Selecting a result either navigates (`href`) or runs an action
	 * (`run`). The palette closes on Escape, click-outside, or after a
	 * successful select. We keep keyboard focus inside the input so the
	 * user can type-then-arrow-then-enter without ever touching the
	 * mouse.
	 */
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { listPaths, getBranches } from '$lib/api';
	import { listPlans, type Plan } from '$lib/plansApi';
	import { branchStore } from '$lib/branchStore.svelte';

	type ResultKind = 'route' | 'plan' | 'path' | 'branch';
	interface Result {
		kind: ResultKind;
		label: string;
		hint?: string;
		score?: number;
		run: () => void | Promise<void>;
	}

	let { open = $bindable(false) }: { open?: boolean } = $props();

	let query = $state('');
	let cursor = $state(0);
	let inputEl: HTMLInputElement | null = $state(null);
	let plans: Plan[] = $state([]);
	let paths: string[] = $state([]);
	let branches: string[] = $state([]);
	let indexedFor: string | null = $state(null); // last branch we indexed for

	// Static routes — keep in sync with the sidebar in +layout.svelte.
	const ROUTES: { href: string; label: string }[] = [
		{ href: '/', label: 'Dashboard' },
		{ href: '/plans', label: 'Plans' },
		{ href: '/sessions', label: 'Sessions' },
		{ href: '/pinned', label: 'Pinned' },
		{ href: '/browse', label: 'Browse' },
		{ href: '/search', label: 'Search' },
		{ href: '/history', label: 'History' },
		{ href: '/diff', label: 'Diff' },
		{ href: '/branches', label: 'Branches' },
		{ href: '/taint', label: 'Taint' }
	];

	function close() {
		open = false;
		query = '';
		cursor = 0;
	}

	async function reindex(branch: string) {
		indexedFor = branch;
		// Fire all three in parallel; tolerate any one failing — partial
		// results are better than an empty palette.
		const [p, paths_, b] = await Promise.allSettled([
			listPlans(branch),
			listPaths(branch),
			getBranches()
		]);
		plans = p.status === 'fulfilled' ? p.value : [];
		paths = paths_.status === 'fulfilled' ? paths_.value : [];
		branches = b.status === 'fulfilled' ? b.value.map((x) => x.name) : [];
	}

	$effect(() => {
		if (open && indexedFor !== branchStore.current) {
			void reindex(branchStore.current);
		}
		if (open) {
			// Refocus + reset on each open so the user always starts fresh.
			queueMicrotask(() => inputEl?.focus());
		}
	});

	// ── Scoring ────────────────────────────────────────────────────────────
	// Cheap subsequence match: every char of q must appear in label in
	// order (case-insensitive). Score rewards earlier matches, contiguous
	// runs, and exact prefix hits — good enough for a few hundred items
	// without a fuzzy-match dependency.
	function score(label: string, q: string): number | null {
		if (!q) return 0;
		const L = label.toLowerCase();
		const Q = q.toLowerCase();
		if (L === Q) return 1000;
		if (L.startsWith(Q)) return 800 - L.length;
		let li = 0;
		let qi = 0;
		let s = 0;
		let run = 0;
		while (li < L.length && qi < Q.length) {
			if (L[li] === Q[qi]) {
				run += 1;
				s += 10 + run * 2;
				if (li === 0 || /[\s/_-]/.test(L[li - 1])) s += 15; // word-boundary
				qi += 1;
			} else {
				run = 0;
			}
			li += 1;
		}
		return qi === Q.length ? s - L.length : null;
	}

	let results = $derived.by<Result[]>(() => {
		const q = query.trim();
		const out: Result[] = [];

		const push = (kind: ResultKind, label: string, hint: string, run: Result['run']) => {
			const sc = score(label, q);
			if (sc === null) return;
			out.push({ kind, label, hint, score: sc, run });
		};

		for (const r of ROUTES) push('route', r.label, r.href, () => goto(r.href));
		for (const p of plans) {
			push('plan', p.name, p.description || '', () => goto('/plans'));
		}
		for (const path of paths) {
			push('path', path, 'browse', () => goto('/browse'));
		}
		for (const b of branches) {
			push('branch', b, 'switch branch', () => {
				branchStore.current = b;
			});
		}

		out.sort((a, b) => (b.score ?? 0) - (a.score ?? 0));
		return out.slice(0, 50);
	});

	$effect(() => {
		// Reset cursor whenever the result set changes shape so we never
		// point past the end (Svelte re-runs this when `results` updates).
		void results.length;
		if (cursor >= results.length) cursor = 0;
	});

	function pick(r: Result) {
		void r.run();
		close();
	}

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			close();
			return;
		}
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			cursor = Math.min(results.length - 1, cursor + 1);
			return;
		}
		if (e.key === 'ArrowUp') {
			e.preventDefault();
			cursor = Math.max(0, cursor - 1);
			return;
		}
		if (e.key === 'Enter') {
			e.preventDefault();
			const r = results[cursor];
			if (r) pick(r);
		}
	}

	// Lazy first-time index so opening the palette is instant.
	onMount(() => {});
</script>

{#if open}
	<div
		class="backdrop"
		onclick={close}
		role="presentation"
	></div>
	<div class="palette" role="dialog" aria-label="Command palette">
		<input
			bind:this={inputEl}
			bind:value={query}
			onkeydown={onKey}
			type="text"
			placeholder="Jump to plan, path, branch, or page…"
			autocomplete="off"
			spellcheck="false"
		/>
		<ul class="results" role="listbox">
			{#each results as r, i (r.kind + ':' + r.label)}
				<li
					class="row"
					class:active={i === cursor}
					role="option"
					aria-selected={i === cursor}
					onmousemove={() => (cursor = i)}
					onclick={() => pick(r)}
				>
					<span class="kind kind-{r.kind}">{r.kind}</span>
					<span class="label">{r.label}</span>
					{#if r.hint}<span class="hint">{r.hint}</span>{/if}
				</li>
			{/each}
			{#if results.length === 0}
				<li class="empty">No matches</li>
			{/if}
		</ul>
		<div class="footer">
			<span><kbd>↑↓</kbd> navigate</span>
			<span><kbd>↵</kbd> select</span>
			<span><kbd>esc</kbd> close</span>
			<span class="indexed">indexed on <code>{indexedFor ?? '—'}</code></span>
		</div>
	</div>
{/if}

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		background: color-mix(in srgb, var(--bg-0) 70%, transparent);
		z-index: 90;
	}

	.palette {
		position: fixed;
		top: 12vh;
		left: 50%;
		transform: translateX(-50%);
		width: min(640px, 92vw);
		background: var(--bg-1);
		border: 1px solid var(--border-hi);
		border-radius: 10px;
		box-shadow: 0 20px 60px rgba(0, 0, 0, 0.4);
		z-index: 100;
		display: flex;
		flex-direction: column;
		max-height: 70vh;
	}

	input {
		background: transparent;
		border: 0;
		border-bottom: 1px solid var(--border);
		color: var(--text-0);
		padding: 0.9rem 1rem;
		font-size: 1rem;
		outline: none;
	}

	.results {
		list-style: none;
		margin: 0;
		padding: 0.25rem 0;
		overflow-y: auto;
		flex: 1;
	}

	.row {
		display: flex;
		align-items: center;
		gap: 0.65rem;
		padding: 0.45rem 1rem;
		cursor: pointer;
		font-size: 0.88rem;
		color: var(--text-1);
	}

	.row.active {
		background: var(--accent-bg);
		color: var(--text-0);
	}

	.kind {
		font-family: monospace;
		font-size: 0.66rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		padding: 0.05rem 0.4rem;
		border-radius: 3px;
		background: var(--bg-hover);
		color: var(--text-3);
		min-width: 3.5rem;
		text-align: center;
	}
	.kind-route   { color: var(--accent);  background: var(--accent-bg); }
	.kind-plan    { color: var(--success); background: color-mix(in srgb, var(--success) 18%, transparent); }
	.kind-branch  { color: var(--warning); background: color-mix(in srgb, var(--warning) 18%, transparent); }
	.kind-path    { color: var(--text-2);  background: var(--bg-hover); }

	.label {
		font-family: monospace;
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.hint {
		color: var(--text-3);
		font-size: 0.78rem;
		font-family: monospace;
		max-width: 40%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.empty {
		padding: 1.5rem 1rem;
		text-align: center;
		color: var(--text-3);
		font-size: 0.85rem;
	}

	.footer {
		display: flex;
		gap: 1rem;
		padding: 0.5rem 1rem;
		border-top: 1px solid var(--border);
		font-size: 0.72rem;
		color: var(--text-3);
		font-family: monospace;
	}

	.footer .indexed { margin-left: auto; }

	kbd {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		border-radius: 3px;
		padding: 0.05rem 0.35rem;
		font-family: monospace;
		font-size: 0.7rem;
		color: var(--text-2);
	}

	code {
		background: transparent;
		color: var(--accent);
	}
</style>
