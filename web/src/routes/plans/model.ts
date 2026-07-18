/**
 * Plans view-model helpers — shared by Board / List / Timeline and the
 * task detail panel. Pure functions only; no Svelte state in here so
 * everything is unit-testable and safe to import from any component.
 */

import type { Plan, Priority, ProofKind, Task, TaskStatus } from '$lib/plansApi';

/* ------------------------------------------------------------------ *
 *  Status + priority metadata                                        *
 * ------------------------------------------------------------------ */

/** Board column order — the task lifecycle, left to right. */
export const TASK_COLUMNS: TaskStatus[] = ['pending', 'in_progress', 'done', 'abandoned'];

export interface StatusMeta {
	label: string;
	/** Semantic color token (text / border accents). */
	color: string;
	/** Matching tint token for pills and column headers. */
	tint: string;
	border: string;
}

export const STATUS_META: Record<TaskStatus, StatusMeta> = {
	pending: {
		label: 'Pending',
		color: 'var(--lens-muted)',
		tint: 'color-mix(in srgb, var(--lens-muted) 9%, transparent)',
		border: 'color-mix(in srgb, var(--lens-muted) 35%, transparent)'
	},
	in_progress: {
		label: 'In progress',
		color: 'var(--lens-accent)',
		tint: 'var(--lens-accent-tint)',
		border: 'var(--lens-accent-border)'
	},
	done: {
		label: 'Done',
		color: 'var(--lens-ok)',
		tint: 'var(--lens-ok-tint)',
		border: 'var(--lens-ok-border)'
	},
	abandoned: {
		label: 'Abandoned',
		color: 'var(--lens-warn)',
		tint: 'var(--lens-warn-tint)',
		border: 'var(--lens-warn-border)'
	}
};

/** Highest urgency first — rank 0 sorts to the top. */
export const PRIORITY_ORDER: Priority[] = ['critical', 'high', 'medium', 'low'];

export interface PriorityMeta {
	label: string;
	short: string;
	rank: number;
	color: string;
	tint: string;
	border: string;
}

export const PRIORITY_META: Record<Priority, PriorityMeta> = {
	critical: {
		label: 'Critical',
		short: 'CR',
		rank: 0,
		color: 'var(--lens-danger)',
		tint: 'var(--lens-danger-tint)',
		border: 'var(--lens-danger-border)'
	},
	high: {
		label: 'High',
		short: 'HI',
		rank: 1,
		color: 'var(--lens-warn)',
		tint: 'var(--lens-warn-tint)',
		border: 'var(--lens-warn-border)'
	},
	medium: {
		label: 'Medium',
		short: 'ME',
		rank: 2,
		color: 'var(--lens-accent)',
		tint: 'var(--lens-accent-tint)',
		border: 'var(--lens-accent-border)'
	},
	low: {
		label: 'Low',
		short: 'LO',
		rank: 3,
		color: 'var(--lens-muted)',
		tint: 'color-mix(in srgb, var(--lens-muted) 9%, transparent)',
		border: 'color-mix(in srgb, var(--lens-muted) 35%, transparent)'
	}
};

/** Monospace glyph per proof kind — restrained, no images. */
export const PROOF_GLYPH: Record<ProofKind, string> = {
	commit: '#',
	file: '▤',
	test: '⚑',
	text: '❝'
};

/* ------------------------------------------------------------------ *
 *  Task graph — parent/child + dependency indices                    *
 * ------------------------------------------------------------------ */

export interface TaskGraph {
	byId: Map<string, Task>;
	/** parent id → direct children. */
	children: Map<string, Task[]>;
	/** task id → tasks that list it in blocked_by (i.e. tasks it blocks). */
	blocks: Map<string, Task[]>;
}

export function buildGraph(tasks: Task[]): TaskGraph {
	const byId = new Map<string, Task>();
	const children = new Map<string, Task[]>();
	const blocks = new Map<string, Task[]>();
	for (const t of tasks) byId.set(t.id, t);
	for (const t of tasks) {
		if (t.parent_id) {
			const list = children.get(t.parent_id) ?? [];
			list.push(t);
			children.set(t.parent_id, list);
		}
		for (const b of t.blocked_by) {
			const list = blocks.get(b) ?? [];
			list.push(t);
			blocks.set(b, list);
		}
	}
	return { byId, children, blocks };
}

/** A blocker is "open" while it is neither done nor abandoned. */
export function isOpen(status: TaskStatus): boolean {
	return status === 'pending' || status === 'in_progress';
}

/** Blockers of `t` that are still open (unknown ids are ignored). */
export function openBlockers(t: Task, g: TaskGraph): Task[] {
	const out: Task[] = [];
	for (const id of t.blocked_by) {
		const b = g.byId.get(id);
		if (b && isOpen(b.status)) out.push(b);
	}
	return out;
}

/** Direct-children progress, or null when the task has no children. */
export function subtaskProgress(t: Task, g: TaskGraph): { done: number; total: number } | null {
	const kids = g.children.get(t.id);
	if (!kids || kids.length === 0) return null;
	const done = kids.filter((k) => k.status === 'done').length;
	return { done, total: kids.length };
}

/* ------------------------------------------------------------------ *
 *  Legal transitions — the drag/drop + action-button contract        *
 * ------------------------------------------------------------------ */

export type DropAction =
	| { kind: 'start' }
	| { kind: 'complete' } // must route through the proof form
	| { kind: 'abandon' } // must route through the reason form
	| { kind: 'noop' }
	| { kind: 'illegal'; reason: string };

/**
 * What a drag from `from` to `to` means. Proof-gated transitions are
 * returned as 'complete' / 'abandon' — the caller opens the relevant
 * form instead of mutating directly (the engine requires proof/reason).
 */
export function dropAction(from: TaskStatus, to: TaskStatus): DropAction {
	if (from === to) return { kind: 'noop' };
	if (from === 'pending' && to === 'in_progress') return { kind: 'start' };
	if (from === 'in_progress' && to === 'done') return { kind: 'complete' };
	if ((from === 'pending' || from === 'in_progress') && to === 'abandoned')
		return { kind: 'abandon' };
	if (from === 'pending' && to === 'done')
		return { kind: 'illegal', reason: 'Pending tasks must be started before they can complete.' };
	if (from === 'done')
		return { kind: 'illegal', reason: 'Done tasks are immutable — proof is already recorded.' };
	if (from === 'abandoned')
		return { kind: 'illegal', reason: 'Abandoned tasks cannot be revived from the board.' };
	if (from === 'in_progress' && to === 'pending')
		return { kind: 'illegal', reason: 'The engine has no in-progress → pending transition.' };
	return { kind: 'illegal', reason: `No ${from} → ${to} transition exists.` };
}

export function canStart(t: Task): boolean {
	return t.status === 'pending';
}
export function canComplete(t: Task): boolean {
	return t.status === 'in_progress';
}
export function canAbandon(t: Task): boolean {
	return isOpen(t.status);
}

/* ------------------------------------------------------------------ *
 *  Sorting + timestamps                                              *
 * ------------------------------------------------------------------ */

/** Most recent lifecycle stamp on the task, ms epoch (0 when unknown). */
export function taskActivityTs(t: Task): number {
	const stamps = [t.completed_at, t.abandoned_at, t.started_at, t.created_at]
		.filter((s): s is string => !!s)
		.map((s) => new Date(s).getTime())
		.filter((n) => !Number.isNaN(n));
	return stamps.length > 0 ? Math.max(...stamps) : 0;
}

export function createdTs(t: Task): number {
	if (!t.created_at) return 0;
	const n = new Date(t.created_at).getTime();
	return Number.isNaN(n) ? 0 : n;
}

/** Default in-column order: priority first, then oldest-created first. */
export function compareBoardOrder(a: Task, b: Task): number {
	const pa = PRIORITY_META[a.priority]?.rank ?? 99;
	const pb = PRIORITY_META[b.priority]?.rank ?? 99;
	if (pa !== pb) return pa - pb;
	return createdTs(a) - createdTs(b);
}

/** Compact relative-time label for dense rows ("3d", "5h", "now"). */
export function agoShort(ms: number, now = Date.now()): string {
	if (ms <= 0) return '—';
	const sec = Math.round((now - ms) / 1000);
	if (sec < 60) return 'now';
	const min = Math.round(sec / 60);
	if (min < 60) return `${min}m`;
	const hr = Math.round(min / 60);
	if (hr < 24) return `${hr}h`;
	const day = Math.round(hr / 24);
	if (day < 30) return `${day}d`;
	const mo = Math.round(day / 30);
	return `${mo}mo`;
}

export function formatTs(ts: string | null): string {
	if (!ts) return '—';
	try {
		return new Date(ts).toLocaleString();
	} catch {
		return ts;
	}
}

/* ------------------------------------------------------------------ *
 *  Plan effective status (carried over from the original page)       *
 * ------------------------------------------------------------------ */

export type EffectiveStatus = 'in_progress' | 'active' | 'completed' | 'archived';

export function effectivePlanStatus(p: Plan): string {
	if (p.status === 'archived') return 'archived';
	const tc = p.task_counts;
	if (tc.in_progress > 0) return 'in_progress';
	if (tc.pending > 0) return 'active';
	if (tc.total > 0 && tc.done + tc.abandoned === tc.total) return 'completed';
	return p.status;
}

export const EFFECTIVE_STATUS_ORDER = ['in_progress', 'active', 'completed', 'archived'] as const;
export const EFFECTIVE_STATUS_LABELS: Record<string, string> = {
	in_progress: 'In progress',
	active: 'Active',
	completed: 'Completed',
	archived: 'Archived'
};

/* ------------------------------------------------------------------ *
 *  Assignee helpers                                                  *
 * ------------------------------------------------------------------ */

/** "claude-code" → "CC", "alice" → "AL". Deterministic, uppercase. */
export function initials(name: string): string {
	const parts = name.split(/[^a-zA-Z0-9]+/).filter(Boolean);
	if (parts.length === 0) return '??';
	if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
	return (parts[0][0] + parts[1][0]).toUpperCase();
}

/** Stable small hash for color assignment. */
export function hashString(s: string): number {
	let h = 0;
	for (let i = 0; i < s.length; i++) {
		h = (h * 31 + s.charCodeAt(i)) | 0;
	}
	return Math.abs(h);
}

/* ------------------------------------------------------------------ *
 *  Search                                                            *
 * ------------------------------------------------------------------ */

/** Case-insensitive match over title, id, assignee and description. */
export function taskMatches(t: Task, q: string): boolean {
	if (!q) return true;
	const needle = q.toLowerCase();
	return (
		t.title.toLowerCase().includes(needle) ||
		t.id.toLowerCase().includes(needle) ||
		(t.assigned_to ?? '').toLowerCase().includes(needle) ||
		(t.description ?? '').toLowerCase().includes(needle)
	);
}

/* ------------------------------------------------------------------ *
 *  Timeline DAG layering                                             *
 * ------------------------------------------------------------------ */

export interface DagLayout {
	/** Topological layers, left → right. Tasks with no in-set blockers land in layer 0. */
	layers: Task[][];
	/** Tasks stuck in a dependency cycle — render separately with a notice. */
	cyclic: Task[];
}

/**
 * Longest-path layering over the blocked_by edges restricted to the
 * given task set (Kahn's algorithm). Cycle members never reach
 * in-degree 0 and are returned in `cyclic`.
 */
export function layerTasks(tasks: Task[]): DagLayout {
	const ids = new Set(tasks.map((t) => t.id));
	const indeg = new Map<string, number>();
	const layer = new Map<string, number>();
	const byId = new Map(tasks.map((t) => [t.id, t]));
	// dependents[b] = ids blocked by b (edge b → dependent)
	const dependents = new Map<string, string[]>();
	for (const t of tasks) {
		const inSet = t.blocked_by.filter((b) => ids.has(b));
		indeg.set(t.id, inSet.length);
		for (const b of inSet) {
			const list = dependents.get(b) ?? [];
			list.push(t.id);
			dependents.set(b, list);
		}
	}
	const queue: string[] = [];
	for (const t of tasks) if ((indeg.get(t.id) ?? 0) === 0) {
		queue.push(t.id);
		layer.set(t.id, 0);
	}
	const placed = new Set<string>();
	while (queue.length > 0) {
		const id = queue.shift()!;
		placed.add(id);
		const l = layer.get(id) ?? 0;
		for (const dep of dependents.get(id) ?? []) {
			layer.set(dep, Math.max(layer.get(dep) ?? 0, l + 1));
			const d = (indeg.get(dep) ?? 1) - 1;
			indeg.set(dep, d);
			if (d === 0) queue.push(dep);
		}
	}
	const maxLayer = Math.max(0, ...[...placed].map((id) => layer.get(id) ?? 0));
	const layers: Task[][] = Array.from({ length: placed.size > 0 ? maxLayer + 1 : 0 }, () => []);
	for (const id of placed) layers[layer.get(id) ?? 0].push(byId.get(id)!);
	// Stable in-lane order: priority then created.
	for (const lane of layers) lane.sort(compareBoardOrder);
	const cyclic = tasks.filter((t) => !placed.has(t.id));
	return { layers, cyclic };
}
