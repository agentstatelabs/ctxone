<script lang="ts">
	import { onMount } from 'svelte';
	import type { CallGraphNode, CallGraphEdge } from './codeTypes';

	interface Props {
		nodes: CallGraphNode[];
		edges: CallGraphEdge[];
		width?: number;
		height?: number;
		onNodeClick?: (node: CallGraphNode) => void;
	}

	let { nodes, edges, width = 600, height = 400, onNodeClick }: Props = $props();

	// Force-directed layout state — positions keyed by node.id
	interface NodePos {
		x: number;
		y: number;
		vx: number;
		vy: number;
		node: CallGraphNode;
	}

	let positions = $state<Map<string, NodePos>>(new Map());
	let hoveredId = $state<string | null>(null);
	let animating = $state(false);
	let rafId: number | null = null;

	const NODE_R = 10;
	const REPULSION = 3000;
	const SPRING_LEN = 90;
	const SPRING_K = 0.04;
	const DAMPING = 0.85;
	const FOCAL_STRENGTH = 0.06;
	const ITERATIONS = 120;

	function initPositions(ns: CallGraphNode[]) {
		const cx = width / 2;
		const cy = height / 2;
		const map = new Map<string, NodePos>();
		ns.forEach((n, i) => {
			const angle = (2 * Math.PI * i) / ns.length;
			const r = Math.min(width, height) * 0.3;
			map.set(n.id, {
				x: n.is_focal ? cx : cx + Math.cos(angle) * r,
				y: n.is_focal ? cy : cy + Math.sin(angle) * r,
				vx: 0,
				vy: 0,
				node: n
			});
		});
		return map;
	}

	function simulate(pos: Map<string, NodePos>, iters: number) {
		const ids = [...pos.keys()];
		const cx = width / 2;
		const cy = height / 2;

		for (let iter = 0; iter < iters; iter++) {
			// Repulsion between all pairs
			for (let i = 0; i < ids.length; i++) {
				for (let j = i + 1; j < ids.length; j++) {
					const a = pos.get(ids[i])!;
					const b = pos.get(ids[j])!;
					const dx = b.x - a.x;
					const dy = b.y - a.y;
					const dist = Math.sqrt(dx * dx + dy * dy) || 1;
					const force = REPULSION / (dist * dist);
					const fx = (dx / dist) * force;
					const fy = (dy / dist) * force;
					a.vx -= fx;
					a.vy -= fy;
					b.vx += fx;
					b.vy += fy;
				}
			}

			// Spring attraction along edges
			for (const edge of edges) {
				const a = pos.get(edge.source);
				const b = pos.get(edge.target);
				if (!a || !b) continue;
				const dx = b.x - a.x;
				const dy = b.y - a.y;
				const dist = Math.sqrt(dx * dx + dy * dy) || 1;
				const stretch = dist - SPRING_LEN;
				const fx = (dx / dist) * stretch * SPRING_K;
				const fy = (dy / dist) * stretch * SPRING_K;
				a.vx += fx;
				a.vy += fy;
				b.vx -= fx;
				b.vy -= fy;
			}

			// Pull focal node toward center
			for (const p of pos.values()) {
				if (p.node.is_focal) {
					p.vx += (cx - p.x) * FOCAL_STRENGTH;
					p.vy += (cy - p.y) * FOCAL_STRENGTH;
				}
			}

			// Integrate + damp + clamp to viewport
			for (const p of pos.values()) {
				p.vx *= DAMPING;
				p.vy *= DAMPING;
				p.x = Math.max(NODE_R + 4, Math.min(width - NODE_R - 4, p.x + p.vx));
				p.y = Math.max(NODE_R + 4, Math.min(height - NODE_R - 4, p.y + p.vy));
			}
		}
	}

	function kindColor(kind: string): string {
		const map: Record<string, string> = {
			function: 'var(--kind-function, #88c0d0)',
			method:   'var(--kind-method,   #a3be8c)',
			class:    'var(--kind-class,     #d08770)',
			module:   'var(--kind-module,    #b48ead)',
			variable: 'var(--kind-variable,  #ebcb8b)'
		};
		return map[kind] ?? 'var(--text-3)';
	}

	$effect(() => {
		if (nodes.length === 0) return;
		const pos = initPositions(nodes);
		simulate(pos, ITERATIONS);
		positions = pos;
	});

	function shortLabel(qname: string): string {
		const parts = qname.split('.');
		return parts.length > 2 ? '…' + parts.slice(-2).join('.') : qname;
	}

	function edgePath(src: NodePos, tgt: NodePos): string {
		const mx = (src.x + tgt.x) / 2;
		const my = (src.y + tgt.y) / 2;
		return `M${src.x},${src.y} Q${mx},${my} ${tgt.x},${tgt.y}`;
	}
</script>

<svg
	{width}
	{height}
	class="callgraph"
	aria-label="Call graph"
>
	<defs>
		<marker
			id="arrowhead"
			viewBox="0 -5 10 10"
			refX="18"
			refY="0"
			markerWidth="6"
			markerHeight="6"
			orient="auto"
		>
			<path d="M0,-5L10,0L0,5" fill="var(--text-3)" />
		</marker>
	</defs>

	<!-- Edges -->
	{#each edges as edge (edge.source + '->' + edge.target)}
		{@const src = positions.get(edge.source)}
		{@const tgt = positions.get(edge.target)}
		{#if src && tgt}
			<path
				d={edgePath(src, tgt)}
				fill="none"
				stroke="var(--border)"
				stroke-width="1.5"
				marker-end="url(#arrowhead)"
				opacity="0.7"
			/>
		{/if}
	{/each}

	<!-- Nodes -->
	{#each [...positions.values()] as p (p.node.id)}
		<!-- svelte-ignore a11y_click_events_have_key_events -->
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<g
			transform="translate({p.x},{p.y})"
			class="node"
			class:focal={p.node.is_focal}
			class:hovered={hoveredId === p.node.id}
			onmouseenter={() => (hoveredId = p.node.id)}
			onmouseleave={() => (hoveredId = null)}
			onclick={() => onNodeClick?.(p.node)}
		>
			<circle
				r={p.node.is_focal ? NODE_R + 3 : NODE_R}
				fill={kindColor(p.node.kind)}
				opacity={p.node.is_focal ? 1 : 0.75}
				stroke={p.node.is_focal ? 'var(--text-0)' : 'transparent'}
				stroke-width={p.node.is_focal ? 2 : 0}
			/>
			<text
				y={p.node.is_focal ? NODE_R + 16 : NODE_R + 13}
				text-anchor="middle"
				class="node-label"
				class:focal-label={p.node.is_focal}
			>
				{shortLabel(p.node.qname)}
			</text>
			{#if hoveredId === p.node.id}
				<title>{p.node.qname}</title>
				<rect
					x={-3}
					y={p.node.is_focal ? NODE_R + 4 : NODE_R + 2}
					width={shortLabel(p.node.qname).length * 6.5 + 6}
					height="14"
					transform="translate({-(shortLabel(p.node.qname).length * 6.5 + 6) / 2}, 0)"
					fill="var(--bg-1)"
					rx="2"
					opacity="0.85"
				/>
			{/if}
		</g>
	{/each}
</svg>

<style>
	.callgraph {
		display: block;
		background: var(--bg-1);
		border-radius: 8px;
	}

	.node {
		cursor: pointer;
	}

	.node-label {
		font-family: monospace;
		font-size: 10px;
		fill: var(--text-2);
		pointer-events: none;
	}

	.focal-label {
		fill: var(--text-0);
		font-weight: 600;
	}

	.node.hovered circle {
		opacity: 1;
	}

	.node.hovered .node-label {
		fill: var(--accent);
	}
</style>
