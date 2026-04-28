/**
 * Typed client for the CTXone Plans HTTP API.
 *
 * Mirrors the shape of `api.ts` so the Plans route can import from
 * here without re-discovering the error conventions.
 */

const API_BASE: string = import.meta.env.VITE_CTXONE_API_URL
	?? (import.meta.env.DEV ? 'http://localhost:3001' : '');

export type TaskStatus = 'pending' | 'in_progress' | 'done' | 'abandoned';
export type PlanStatus = 'active' | 'completed' | 'archived';
export type Priority = 'low' | 'medium' | 'high' | 'critical';
export type ProofKind = 'commit' | 'file' | 'test' | 'text';

export interface Proof {
	kind: ProofKind;
	value: string;
	note?: string | null;
}

export interface TaskCounts {
	pending: number;
	in_progress: number;
	done: number;
	abandoned: number;
	total: number;
}

export interface Task {
	id: string;
	title: string;
	status: TaskStatus;
	priority: Priority;
	parent_id: string | null;
	blocked_by: string[];
	assigned_to: string | null;
	created_at: string | null;
	created_by: string | null;
	started_at: string | null;
	started_by: string | null;
	completed_at: string | null;
	completed_by: string | null;
	abandoned_at: string | null;
	abandoned_reason: string | null;
	proof: Proof | null;
}

export interface Plan {
	name: string;
	description: string | null;
	status: PlanStatus;
	created_at: string | null;
	created_by: string | null;
	archived_at: string | null;
	task_counts: TaskCounts;
	tasks?: Task[];
}

async function fetchJson<T>(path: string, init?: RequestInit): Promise<T> {
	const resp = await fetch(`${API_BASE}${path}`, init);
	if (!resp.ok) {
		const text = await resp.text().catch(() => '');
		throw new Error(text || `${resp.status} ${resp.statusText}`);
	}
	return resp.json();
}

function ref(branch: string): string {
	return encodeURIComponent(branch);
}

export async function listPlans(
	branch = 'main',
	status?: PlanStatus
): Promise<Plan[]> {
	const qs = status
		? `?ref=${ref(branch)}&status=${status}`
		: `?ref=${ref(branch)}`;
	return fetchJson(`/api/plans${qs}`);
}

export async function getPlan(name: string, branch = 'main'): Promise<Plan> {
	return fetchJson(`/api/plans/${encodeURIComponent(name)}?ref=${ref(branch)}`);
}

export async function createPlan(
	name: string,
	description: string | null,
	branch = 'main'
): Promise<Plan> {
	const body: Record<string, unknown> = { name, ref: branch };
	if (description) body.description = description;
	return fetchJson('/api/plans', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(body)
	});
}

export async function listPlanTasks(
	name: string,
	branch = 'main'
): Promise<Task[]> {
	return fetchJson(
		`/api/plans/${encodeURIComponent(name)}/tasks?ref=${ref(branch)}`
	);
}

export interface AddTaskRequest {
	title: string;
	description?: string;
	priority?: Priority;
	parent_id?: string;
	assigned_to?: string;
	blocked_by?: string[];
}

export async function addTask(
	plan: string,
	req: AddTaskRequest,
	branch = 'main'
): Promise<Task> {
	return fetchJson(`/api/plans/${encodeURIComponent(plan)}/tasks`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ ...req, ref: branch })
	});
}

export async function startTask(
	plan: string,
	taskId: string,
	branch = 'main'
): Promise<Task> {
	return fetchJson(
		`/api/plans/${encodeURIComponent(plan)}/tasks/${encodeURIComponent(taskId)}/start`,
		{
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ ref: branch })
		}
	);
}

export async function completeTask(
	plan: string,
	taskId: string,
	proof: Proof,
	branch = 'main'
): Promise<Task> {
	return fetchJson(
		`/api/plans/${encodeURIComponent(plan)}/tasks/${encodeURIComponent(taskId)}/complete`,
		{
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ proof, ref: branch })
		}
	);
}

export async function abandonTask(
	plan: string,
	taskId: string,
	reason: string,
	branch = 'main'
): Promise<Task> {
	return fetchJson(
		`/api/plans/${encodeURIComponent(plan)}/tasks/${encodeURIComponent(taskId)}/abandon`,
		{
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ reason, ref: branch })
		}
	);
}

export async function archivePlan(name: string, branch = 'main'): Promise<Plan> {
	return fetchJson(`/api/plans/${encodeURIComponent(name)}/archive`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ ref: branch })
	});
}

/**
 * Force-complete a plan: server abandons every still-open task with
 * the given reason (or the default), then auto-promotes the plan to
 * `completed`. Returns the freshly-loaded plan + the ids of tasks
 * that were abandoned this call.
 */
/**
 * Move a plan and all its tasks from one branch to another.
 * Task ids and statuses are preserved.
 */
export async function movePlan(
	name: string,
	sourceBranch: string,
	targetBranch: string
): Promise<{ plan: Plan; source_ref: string; target_ref: string; task_count: number }> {
	const url = `/api/plans/${encodeURIComponent(name)}/move?ref=${encodeURIComponent(sourceBranch)}`;
	return fetchJson(url, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ target_ref: targetBranch })
	});
}

export async function forceCompletePlan(
	name: string,
	branch = 'main',
	reason?: string
): Promise<{ plan: Plan; abandoned_task_ids: string[] }> {
	const url = `/api/plans/${encodeURIComponent(name)}/force_complete?ref=${encodeURIComponent(branch)}`;
	return fetchJson(url, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ reason: reason ?? null })
	});
}

export async function nextTask(
	plan: string,
	opts: {
		branch?: string;
		assignedTo?: string;
		includeUnassigned?: boolean;
		assignedOnly?: boolean;
	} = {}
): Promise<Task | null> {
	const branch = opts.branch ?? 'main';
	const params = new URLSearchParams();
	params.set('ref', branch);
	if (opts.assignedTo) params.set('assigned_to', opts.assignedTo);
	if (opts.includeUnassigned !== undefined)
		params.set('include_unassigned', String(opts.includeUnassigned));
	if (opts.assignedOnly) params.set('assigned_only', 'true');
	const data = await fetchJson<{ task: Task | null }>(
		`/api/plans/${encodeURIComponent(plan)}/next?${params.toString()}`
	);
	return data.task;
}
