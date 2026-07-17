<script lang="ts">
	import type { Task } from '$lib/plansApi';
	import { STATUS_META, layerTasks } from './model';

	let {
		tasks,
		onOpen
	}: {
		tasks: Task[];
		onOpen: (t: Task) => void;
	} = $props();

	// Geometry constants — simple layered DAG, lanes left → right in
	// dependency order (a task sits right of everything that blocks it).
	const NW = 172; // node width
	const NH = 46; // node height
	const XGAP = 88; // horizontal gap between lanes (edge room)
	const YGAP = 14; // vertical gap between nodes in a lane
	const PAD = 20;

	let dag = $derived(layerTasks(tasks));

	interface NodePos {
		task: Task;
		x: number;
		y: number;
	}
	interface Edge {
		id: string;
		from: NodePos;
		to: NodePos;
	}

	let nodes = $derived.by(() => {
		const out = new Map<string, NodePos>();
		dag.layers.forEach((lane, li) => {
			lane.forEach((t, i) => {
				out.set(t.id, {
					task: t,
					x: PAD + li * (NW + XGAP),
					y: PAD + i * (NH + YGAP)
				});
			});
		});
		return out;
	});

	let edges = $derived.by(() => {
		const out: Edge[] = [];
		for (const pos of nodes.values()) {
			for (const b of pos.task.blocked_by) {
				const from = nodes.get(b);
				if (from) out.push({ id: `${b}->${pos.task.id}`, from, to: pos });
			}
		}
		return out;
	});

	let width = $derived(
		dag.layers.length > 0 ? PAD * 2 + dag.layers.length * NW + (dag.layers.length - 1) * XGAP : 0
	);
	let height = $derived(
		PAD * 2 +
			Math.max(0, ...dag.layers.map((l) => l.length)) * (NH + YGAP) -
			(dag.layers.some((l) => l.length > 0) ? YGAP : 0)
	);

	let hoveredEdge: string | null = $state(null);
	let hoveredNode: string | null = $state(null);

	function edgeHighlighted(e: Edge): boolean {
		if (hoveredEdge === e.id) return true;
		if (hoveredNode && (e.from.task.id === hoveredNode || e.to.task.id === hoveredNode))
			return true;
		return false;
	}

	function edgePath(e: Edge): string {
		const x1 = e.from.x + NW;
		const y1 = e.from.y + NH / 2;
		const x2 = e.to.x;
		const y2 = e.to.y + NH / 2;
		const mx = (x1 + x2) / 2;
		return `M ${x1} ${y1} C ${mx} ${y1}, ${mx} ${y2}, ${x2} ${y2}`;
	}

	function truncate(s: string, n: number): string {
		return s.length > n ? s.slice(0, n - 1) + '…' : s;
	}
</script>

<div class="timeline">
	{#if tasks.length === 0}
		<p class="empty">No tasks match.</p>
	{:else}
		{#if dag.layers.length > 0}
			<div class="scroll">
				<svg
					{width}
					{height}
					viewBox="0 0 {width} {height}"
					role="img"
					aria-label="Dependency timeline: {tasks.length} tasks in {dag.layers.length} lanes"
				>
					<!-- edges under nodes -->
					{#each edges as e (e.id)}
						{@const hi = edgeHighlighted(e)}
						<g
							class="edge"
							class:hi
							onmouseenter={() => (hoveredEdge = e.id)}
							onmouseleave={() => (hoveredEdge = null)}
							role="presentation"
						>
							<!-- fat invisible hit area so hover isn't pixel-hunting -->
							<path d={edgePath(e)} class="edge-hit" />
							<path d={edgePath(e)} class="edge-line" />
							<!-- arrowhead drawn inline so it inherits the hover color -->
							<path
								class="edge-head"
								d="M {e.to.x - 7} {e.to.y + NH / 2 - 4} L {e.to.x} {e.to.y + NH / 2} L {e.to.x - 7} {e.to.y + NH / 2 + 4} Z"
							/>
						</g>
					{/each}
					{#each [...nodes.values()] as pos (pos.task.id)}
						{@const meta = STATUS_META[pos.task.status]}
						<g
							class="node"
							class:dim={hoveredNode !== null && hoveredNode !== pos.task.id}
							transform="translate({pos.x}, {pos.y})"
							onmouseenter={() => (hoveredNode = pos.task.id)}
							onmouseleave={() => (hoveredNode = null)}
							onclick={() => onOpen(pos.task)}
							onkeydown={(e) => {
								if (e.key === 'Enter' || e.key === ' ') {
									e.preventDefault();
									onOpen(pos.task);
								}
							}}
							role="button"
							tabindex="0"
							aria-label="{pos.task.id} {pos.task.title} — {meta.label}"
						>
							<title>{pos.task.id} · {pos.task.title} · {meta.label}</title>
							<rect
								width={NW}
								height={NH}
								rx="6"
								class="node-rect"
								style:stroke={meta.color}
							/>
							<circle cx="12" cy={NH / 2} r="3.5" style:fill={meta.color} />
							<text x="22" y="19" class="node-title">{truncate(pos.task.title, 22)}</text>
							<text x="22" y="35" class="node-id">{pos.task.id}</text>
						</g>
					{/each}
				</svg>
			</div>
		{/if}

		{#if dag.cyclic.length > 0}
			<div class="cycle-notice" role="note">
				<p class="cycle-msg">
					⚠ {dag.cyclic.length} task{dag.cyclic.length === 1 ? '' : 's'} form a dependency
					cycle and can't be ordered:
				</p>
				<div class="cycle-strip">
					{#each dag.cyclic as t (t.id)}
						<button
							type="button"
							class="cycle-chip"
							style:border-color={STATUS_META[t.status].border}
							onclick={() => onOpen(t)}
						>
							<span class="dot" style:background={STATUS_META[t.status].color}></span>
							<span class="cid">{t.id}</span>
							{truncate(t.title, 32)}
						</button>
					{/each}
				</div>
			</div>
		{/if}

		<p class="legend">
			{#each Object.entries(STATUS_META) as [key, meta] (key)}
				<span class="legend-item">
					<span class="dot" style:background={meta.color}></span>{meta.label}
				</span>
			{/each}
			<span class="legend-hint">lanes flow left → right in dependency order · click a task for details</span>
		</p>
	{/if}
</div>

<style>
	.timeline {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-3);
	}
	.scroll {
		overflow: auto;
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-md);
		background: var(--lens-surface);
		padding: var(--lens-space-2);
	}
	.edge-hit {
		fill: none;
		stroke: transparent;
		stroke-width: 12;
	}
	.edge-line {
		fill: none;
		stroke: var(--lens-border-strong);
		stroke-width: 1.5;
		transition: stroke var(--lens-dur-fast) var(--lens-ease);
	}
	.edge-head {
		fill: var(--lens-border-strong);
		stroke: none;
		transition: fill var(--lens-dur-fast) var(--lens-ease);
	}
	.edge.hi .edge-line {
		stroke: var(--lens-accent);
		stroke-width: 2;
	}
	.edge.hi .edge-head {
		fill: var(--lens-accent);
	}
	.node {
		cursor: pointer;
	}
	.node.dim {
		opacity: 0.55;
	}
	.node-rect {
		fill: var(--lens-surface-raised);
		stroke-width: 1.25;
		transition: fill var(--lens-dur-fast) var(--lens-ease);
	}
	.node:hover .node-rect,
	.node:focus-visible .node-rect {
		fill: var(--lens-overlay);
	}
	.node-title {
		fill: var(--lens-text);
		font-family: var(--lens-font-sans);
		font-size: 11.5px;
	}
	.node-id {
		fill: var(--lens-muted);
		font-family: var(--lens-font-mono);
		font-size: 10px;
	}
	.cycle-notice {
		border: 1px solid var(--lens-warn-border);
		background: var(--lens-warn-tint);
		border-radius: var(--lens-radius-md);
		padding: var(--lens-space-3);
	}
	.cycle-msg {
		margin: 0 0 var(--lens-space-2);
		color: var(--lens-warn);
		font-size: var(--lens-font-size-xs);
	}
	.cycle-strip {
		display: flex;
		flex-wrap: wrap;
		gap: var(--lens-space-2);
	}
	.cycle-chip {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		background: var(--lens-surface);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		font-size: var(--lens-font-size-xs);
		padding: 0.25rem 0.5rem;
		cursor: pointer;
	}
	.cycle-chip:hover {
		background: var(--lens-surface-raised);
	}
	.cid {
		font-family: var(--lens-font-mono);
		color: var(--lens-muted);
		font-size: var(--lens-font-size-2xs);
	}
	.dot {
		display: inline-block;
		width: 0.45rem;
		height: 0.45rem;
		border-radius: var(--lens-radius-full);
		flex: none;
	}
	.legend {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--lens-space-4);
		margin: 0;
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-text-secondary);
	}
	.legend-item {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
	}
	.legend-hint {
		margin-left: auto;
		color: var(--lens-text-faint);
	}
	.empty {
		color: var(--lens-text-faint);
		font-style: italic;
		text-align: center;
		padding: var(--lens-space-6);
		margin: 0;
	}
</style>
