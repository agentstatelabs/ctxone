/**
 * CtxOne Lens — HTTP client for the Hub REST API.
 */

import { namespaceStore } from './namespaceStore.svelte';

// Dev: default to Hub on 3001. Embedded (adapter-static build served from Hub):
// same-origin relative URLs. Explicit VITE_CTXONE_API_URL overrides both.
const API_BASE: string = import.meta.env.VITE_CTXONE_API_URL
	?? (import.meta.env.DEV ? 'http://localhost:3001' : '');

/**
 * Single choke point for Hub requests: prefixes API_BASE and threads
 * the current namespace via the `X-CTXone-Namespace` header, so no
 * call site needs to know about namespaces. The ASD `/api/code/*`
 * proxy (codeApi.ts) intentionally does NOT go through here.
 */
export function hubFetch(path: string, init?: RequestInit): Promise<Response> {
	const headers = new Headers(init?.headers);
	headers.set('X-CTXone-Namespace', namespaceStore.current);
	return fetch(`${API_BASE}${path}`, { ...init, headers });
}

export interface StatsResponse {
	commit_count: number;
	path_count: number;
	branch_count: number;
	epoch_count: number;
	agents: string[];
	categories: string[];
}

export interface TokenStats {
	session_tokens_used: number;
	/** Honest, bounded savings ESTIMATE (not a measurement). See the server's
	 *  RECONSTRUCTION_FACTOR. Always render it as an estimate. */
	session_tokens_saved: number;
	/** Injected tokens of the session's first recall — the startup boost that
	 *  brought the agent up to speed. May be absent on older Hubs. */
	session_startup_tokens?: number;
	total_graph_size_chars: number;
	total_graph_size_tokens: number;
	cumulative_ratio: number;
	/** Read-time reconciliation ratio, set by the Hub only when this session
	 *  has LLM usage but `cumulative_ratio` is 0 (its recall savings accrued on
	 *  another session id). It's the workspace-aggregate ratio, to be shown as a
	 *  clearly-estimated "≈" figure. Absent when the session has its own ratio. */
	fallback_ratio?: number | null;
	/** LLM-observed fields, populated by agent usage reports. All
	 * optional for back-compat with older Hubs that don't serialize
	 * them, though current Hubs always include them (zeros until an
	 * agent reports).
	 */
	llm_input_tokens?: number;
	llm_output_tokens?: number;
	llm_cache_read_tokens?: number;
	llm_cache_create_tokens?: number;
	llm_call_count?: number;
	last_model?: string | null;
	last_provider?: string | null;
}

export interface SessionSnapshot extends TokenStats {
	session_id: string;
	/** Human-readable title, server-derived from the first user turn. */
	name?: string | null;
	/** Originating agent ("Claude Code", "Codex", …). */
	source?: string | null;
	/** ISO timestamps. All optional: older Hubs omit them, and sessions
	 * ingested before session-meta capture have no start time. */
	started_at?: string | null;
	updated_at?: string | null;
	/** Every model the session touched, not just the last one. */
	models_used?: string[];
	/** Token classes the normalised counters cannot express, under the
	 * reporting agent's own field names (Codex `reasoning_output_tokens`,
	 * Gemini `thoughts`/`tool`). Absent for Anthropic-shaped sessions. */
	extra_tokens?: Record<string, number>;
	/** True per-model usage split (t-023): which models the session used and how
	 * much on each, so efficiency views don't have to bucket a whole session
	 * onto `last_model`. Summed across sessions on the aggregate snapshot.
	 * Absent for sessions ingested before per-model capture. */
	llm_by_model?: Record<string, ModelUsage>;
	/** Whether the session has a stored `/turns` subtree in the listed workspace.
	 * The Sessions view gates its first-turn title probe on this so turn-less
	 * sessions don't each log a 404. Absent on older hubs (treated as unknown →
	 * the probe still runs, preserving prior behaviour). */
	has_turns?: boolean;
}

/** One model's share of a session's LLM usage. Mirrors the server's ModelUsage. */
export interface ModelUsage {
	input_tokens: number;
	output_tokens: number;
	cache_read_tokens: number;
	cache_create_tokens: number;
	call_count: number;
}

export interface CommitEntry {
	id: string;
	timestamp: string;
	intent: {
		category: string;
		description: string;
		confidence?: number;
		reasoning?: string;
	};
	agent_id: string;
}

export interface BlameEntry {
	path: string;
	commit_id: string;
	timestamp: string;
	agent_id: string;
	intent_description: string;
	confidence?: number;
}

export interface SearchResult {
	path: string;
	value: string;
}

async function fetchJson<T>(path: string): Promise<T> {
	const resp = await hubFetch(path);
	if (!resp.ok) {
		throw new Error(`API error: ${resp.status} ${resp.statusText}`);
	}
	return resp.json();
}

async function fetchText(path: string): Promise<string> {
	const resp = await hubFetch(path);
	if (!resp.ok) {
		throw new Error(`API error: ${resp.status} ${resp.statusText}`);
	}
	return resp.text();
}

export async function getHealth(): Promise<boolean> {
	try {
		await fetchText('/api/health');
		return true;
	} catch {
		return false;
	}
}

export async function getStats(ref_name = 'main'): Promise<StatsResponse> {
	return fetchJson(`/api/stats/${encodeURIComponent(ref_name)}`);
}

export async function getTokenStats(): Promise<TokenStats> {
	return fetchJson('/api/stats/tokens');
}

export async function getSessions(): Promise<SessionSnapshot[]> {
	return fetchJson('/api/stats/sessions');
}

/** A sealed per-plan epoch checkpoint (audit bundle). */
export interface Epoch {
	id: string;
	namespace: string;
	plan: string;
	status: string;
	created_at: string;
	sealed_at: string | null;
	commit_count: number;
}

/** Sealed per-plan epoch checkpoints for the current workspace, or every
 * workspace with `all=true` (the hub-level view). Newest-sealed first. */
export async function getEpochs(all = false): Promise<Epoch[]> {
	const data = await fetchJson<{ epochs: Epoch[] }>(`/api/epochs${all ? '?all=true' : ''}`);
	return data.epochs;
}

/** Download URL for one epoch's audit bundle. Uses `?namespace=` (not the
 * `X-CTXone-Namespace` header) so a plain `<a download>` works; pass the
 * epoch's own namespace. */
export function epochExportUrl(id: string, namespace: string): string {
	return `/api/epochs/${encodeURIComponent(id)}/export?namespace=${encodeURIComponent(namespace)}`;
}

/**
 * One workspace's aggregate stats from `GET /api/namespaces/summary` — the
 * hub-global rollup that feeds the Hub Home. Token totals are summed from the
 * process-global session registry, scoped to the sessions resident in the
 * workspace; `graph` mirrors the per-ref stats (`/api/stats/{ref}`) for `main`.
 * Project metadata (display name, remote) is NOT here — join it client-side
 * from `listProjects()`.
 */
export interface WorkspaceSummary {
	namespace: string;
	session_count: number;
	/** Most-common last_model across the workspace's sessions — for a rough
	 * (≈) cost estimate on the Hub Home. Null when no session reported a model. */
	representative_model?: string | null;
	tokens: {
		used: number;
		saved: number;
		llm_input: number;
		llm_output: number;
		llm_cache_read: number;
		llm_cache_create: number;
		/** Per-model token split, summed across the workspace's sessions. Lets the
		 * Hub Home price each model at its own rate. Absent on older hubs. */
		by_model?: Record<string, ModelUsage>;
	};
	/** Graph counts for `main`; shape matches StatsResponse (extra fields ignored). */
	graph: {
		commit_count: number;
		path_count: number;
		branch_count: number;
		epoch_count: number;
	} | null;
}

/** `GET /api/namespaces/summary` — per-workspace rollup for the Hub Home. */
export async function getNamespacesSummary(): Promise<WorkspaceSummary[]> {
	const data = await fetchJson<{ workspaces: WorkspaceSummary[] }>('/api/namespaces/summary');
	return data.workspaces;
}

/** `GET /api/namespaces` — plain list of workspace (namespace) names. */
export async function listNamespaces(): Promise<string[]> {
	const data = await fetchJson<{ namespaces: { namespace: string }[] }>('/api/namespaces');
	return data.namespaces.map((n) => n.namespace);
}

/** `POST /api/sessions/{id}/move` — relocate a session to another workspace.
 * Reads from the current namespace (via the `X-CTXone-Namespace` header). */
export async function moveSession(
	sessionId: string,
	toNamespace: string
): Promise<{ to?: string; deleted_source?: boolean; dst_leaves?: number }> {
	const resp = await hubFetch(`/api/sessions/${encodeURIComponent(sessionId)}/move`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-CTXone-Agent': 'lens' },
		body: JSON.stringify({ to_namespace: toNamespace, ref: 'main' })
	});
	if (!resp.ok) {
		throw new Error(`session move failed (${resp.status}): ${await resp.text().catch(() => '')}`);
	}
	return resp.json();
}

export async function getSessionTokenStats(sessionId: string): Promise<SessionSnapshot> {
	return fetchJson(`/api/stats/tokens/${encodeURIComponent(sessionId)}`);
}

export async function getState(ref_name = 'main', path = '/'): Promise<unknown> {
	return fetchJson(`/api/state/${encodeURIComponent(ref_name)}?path=${encodeURIComponent(path)}`);
}

export async function listPaths(ref_name = 'main', prefix = '/'): Promise<string[]> {
	return fetchJson(`/api/state/${encodeURIComponent(ref_name)}/paths?prefix=${encodeURIComponent(prefix)}`);
}

export async function searchValues(ref_name = 'main', query: string): Promise<SearchResult[]> {
	return fetchJson(`/api/state/${encodeURIComponent(ref_name)}/search?query=${encodeURIComponent(query)}`);
}

export async function getLog(ref_name = 'main', limit = 20): Promise<CommitEntry[]> {
	return fetchJson(`/api/log/${encodeURIComponent(ref_name)}?limit=${limit}`);
}

/** Per-day commit counts for the activity heatmap. */
export interface ActivityResponse {
	days: Array<{ date: string; count: number }>;
	requested_days: number;
	/** Commits the server walked to build this. */
	scanned: number;
	/** True when the walk hit its cap before reaching the requested cutoff —
	 * the history shown is partial, not a quiet period. */
	truncated: boolean;
}

/**
 * Commits per day over the last `days`.
 *
 * Replaces counting `getLog(ref, 1000)` client-side, which charted a
 * commit-count window rather than a time window: on a busy machine those
 * 1000 commits covered under two hours.
 */
export async function getActivity(ref_name = 'main', days = 120): Promise<ActivityResponse> {
	return fetchJson(`/api/stats/activity/${encodeURIComponent(ref_name)}?days=${days}`);
}

export async function getBlame(ref_name = 'main', path: string): Promise<BlameEntry[]> {
	return fetchJson(`/api/blame/${encodeURIComponent(ref_name)}?path=${encodeURIComponent(path)}`);
}

export interface BranchInfo {
	name: string;
	id: string;
	/** Head-commit timestamp; absent on older hubs or unreadable heads. */
	updated_at?: string | null;
}
export async function getBranches(): Promise<BranchInfo[]> {
	return fetchJson('/api/branches');
}

export interface RememberRequest {
	fact: string;
	importance?: 'high' | 'medium' | 'low';
	context?: string;
	tags?: string[];
}

export async function remember(req: RememberRequest): Promise<{ path: string; commit_id: string }> {
	const resp = await hubFetch(`/api/memory/remember`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(req)
	});
	if (!resp.ok) {
		throw new Error(`remember failed: ${resp.status}`);
	}
	return resp.json();
}

/**
 * One entry in a recall response. Pinned entries carry `title`/`body`;
 * topic matches carry `value`/`score`/`full_match`.
 */
export interface RecallEntry {
	path: string;
	pinned: boolean;
	title?: string;
	body?: string;
	value?: string;
	score?: number;
	full_match?: boolean;
}

export interface RecallResponse {
	topic: string;
	ref: string;
	results: RecallEntry[];
	pinned_count: number;
	topic_matches: number;
	ctx_tokens_sent: number;
	ctx_tokens_estimated_flat: number;
	ctx_savings_ratio: number;
}

export async function recall(topic: string, budget = 1500, ref = 'main'): Promise<RecallResponse> {
	return fetchJson(
		`/api/memory/recall?topic=${encodeURIComponent(topic)}&budget=${budget}&ref=${encodeURIComponent(ref)}`
	);
}

/**
 * Blame shape as the engine actually serializes it (agentstategraph-core
 * `BlameEntry`). Note this differs from the older `BlameEntry` interface
 * above: no `confidence`, plus `intent_category`/`reasoning`/`timestamp_anomaly`.
 */
export interface WhyBlame {
	path: string;
	commit_id: string;
	agent_id: string;
	intent_category: string;
	intent_description: string;
	reasoning: string | null;
	timestamp: string;
	timestamp_anomaly?: boolean;
}

export interface WhyTrace {
	path: string;
	/** The Hub serializes a SINGLE blame entry per trace (repo.blame returns
	 * one entry), despite HTTP_API.md showing an array. Accept both. */
	blame: WhyBlame | WhyBlame[] | null;
}

export interface WhyResponse {
	decision: string;
	traces: WhyTrace[];
}

/** GET /api/memory/why_did_we?decision=… — always searches/blames `main`. */
export async function whyDidWe(decision: string): Promise<WhyResponse> {
	return fetchJson(`/api/memory/why_did_we?decision=${encodeURIComponent(decision)}`);
}

// -- Session arcs (t-003) & recall log (t-004) ----------------------------

/**
 * One topic arc from `GET /api/sessions/{sid}/segments`. Arcs split with no
 * LLM: a new arc starts on a branch/cwd change or an idle gap. `start`/`end`
 * are turn indices (inclusive); `reason` is why THIS arc began.
 */
export interface SessionSegment {
	start: number;
	end: number;
	turn_count: number;
	branch: string | null;
	cwd: string | null;
	started_at: string | null;
	ended_at: string | null;
	tokens: number;
	label: string;
	/** "start" | "branch" | "cwd" | "gap". */
	reason: string;
}

export interface SegmentsResponse {
	session: string;
	gap_minutes: number;
	segment_count: number;
	segments: SessionSegment[];
}

/** `GET /api/sessions/{sid}/segments?gap=<min>` — split a session into arcs. */
export async function getSessionSegments(
	sessionId: string,
	gapMinutes = 30
): Promise<SegmentsResponse> {
	return fetchJson(
		`/api/sessions/${encodeURIComponent(sessionId)}/segments?gap=${gapMinutes}`
	);
}

/** One recall injection recorded for a session (t-004). */
export interface RecallLogEntry {
	at: string;
	topic: string;
	/** Memory paths injected into context (not their content). */
	paths: string[];
	tokens_sent: number;
	savings_ratio: number;
}

export interface RecallLogResponse {
	session_id: string;
	recall_log: RecallLogEntry[];
}

/**
 * `GET /api/sessions/{sid}/recall-log` — the in-memory audit of what memory
 * each recall injected. Session-global (no namespace); non-durable, so empty
 * for a session with no live recalls since the Hub last started.
 */
export async function getSessionRecallLog(
	sessionId: string
): Promise<RecallLogResponse> {
	return fetchJson(`/api/sessions/${encodeURIComponent(sessionId)}/recall-log`);
}

// -- Reminders ------------------------------------------------------------

export type ReminderStatus =
	| 'pending'
	| 'due'
	| 'awaiting_permission'
	| 'in_progress'
	| 'completed'
	| 'snoozed'
	| 'cancelled';

export type ReminderPriority = 'critical' | 'high' | 'medium' | 'low' | 'minimal';

export interface ReminderRef {
	kind: string;
	id: string;
	label: string | null;
	stale: boolean;
}

export interface ReminderExecution {
	started_at: string;
	completed_at: string | null;
	agent_id: string;
	approved_by: string | null;
	result: 'success' | 'failed' | 'deferred' | 'snoozed' | 'cancelled';
	notes: string[];
	task_id: string | null;
}

export interface ReminderSchedule {
	kind: 'once' | 'interval' | 'daily' | 'weekly';
	every_seconds?: number;
	time?: string;
	day?: string;
}

/** Wire shape from `server/src/reminder_tools.rs::reminder_to_json`. */
export interface Reminder {
	id: string;
	title: string;
	instructions: string;
	commands: string[];
	refs: ReminderRef[];
	priority: ReminderPriority;
	due_at: string;
	schedule: ReminderSchedule | null;
	autonomous: boolean;
	created_by: string;
	created_at: string;
	status: ReminderStatus;
	snoozed_until: string | null;
	executions: ReminderExecution[];
	tags: string[];
}

/** GET /api/reminders — optional status filter (pending|due|…). */
export async function listReminders(status?: ReminderStatus): Promise<Reminder[]> {
	const qs = status ? `?status=${encodeURIComponent(status)}` : '';
	return fetchJson(`/api/reminders${qs}`);
}

/** GET /api/reminders/due — actionable (due / awaiting_permission), by priority. */
export async function getDueReminders(): Promise<Reminder[]> {
	return fetchJson('/api/reminders/due');
}

export interface CreateReminderRequest {
	title: string;
	instructions: string;
	/** ISO 8601 / RFC 3339, e.g. 2026-05-10T09:00:00Z */
	due_at: string;
	priority?: ReminderPriority;
	autonomous?: boolean;
	tags?: string[];
}

export async function createReminder(req: CreateReminderRequest): Promise<Reminder> {
	const resp = await hubFetch('/api/reminders', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(req)
	});
	if (!resp.ok) {
		const msg = await resp.text();
		throw new Error(`create reminder failed: ${resp.status} ${msg}`);
	}
	return resp.json();
}

async function reminderAction(id: string, action: string, body: object): Promise<Reminder> {
	const resp = await hubFetch(`/api/reminders/${encodeURIComponent(id)}/${action}`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(body)
	});
	if (!resp.ok) {
		const msg = await resp.text();
		throw new Error(`${action} failed: ${resp.status} ${msg}`);
	}
	return resp.json();
}

export function snoozeReminder(id: string, until: string): Promise<Reminder> {
	return reminderAction(id, 'snooze', { until });
}

export function approveReminder(id: string, approvedBy?: string): Promise<Reminder> {
	return reminderAction(id, 'approve', approvedBy ? { approved_by: approvedBy } : {});
}

export function cancelReminder(id: string): Promise<Reminder> {
	return reminderAction(id, 'cancel', {});
}

export function startReminder(id: string): Promise<Reminder> {
	return reminderAction(id, 'start', {});
}

export function recordReminder(
	id: string,
	result: ReminderExecution['result'],
	notes?: string[]
): Promise<Reminder> {
	// Quirk: the Hub's record handler deserializes ReminderRecordParams,
	// whose `id` field is REQUIRED in the JSON body (it's then overridden
	// by the path param). Omitting it 422s, so send it redundantly.
	return reminderAction(id, 'record', { id, result, notes: notes ?? [] });
}

export interface ForgetRequest {
	path: string;
	reason?: string;
	ref?: string;
}

export async function forget(req: ForgetRequest): Promise<{ path: string; commit_id: string }> {
	const resp = await hubFetch(`/api/memory/forget`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({
			path: req.path,
			reason: req.reason ?? 'forgotten via Lens',
			ref: req.ref ?? 'main'
		})
	});
	if (!resp.ok) {
		const msg = await resp.text();
		throw new Error(`forget failed: ${resp.status} ${msg}`);
	}
	return resp.json();
}

export interface DiffOp {
	op: string;
	path: string;
	value?: unknown;
}

export interface DiffResponse {
	ref_a: string;
	ref_b: string;
	ops: DiffOp[];
}

export async function getDiff(refA: string, refB: string): Promise<DiffResponse> {
	return fetchJson(
		`/api/diff?ref_a=${encodeURIComponent(refA)}&ref_b=${encodeURIComponent(refB)}`
	);
}

export interface MergeRequest {
	source: string;
	target: string;
	description?: string;
	reasoning?: string;
}

export interface MergeOk {
	status: 'ok';
	source: string;
	target: string;
	commit_id: string;
}

export interface MergeConflict {
	status: 'conflict';
	source: string;
	target: string;
	conflicts: unknown;
}

export type MergeResult = MergeOk | MergeConflict;

/** POST /api/merge. Returns ok on success, conflict on 409. Throws for other errors. */
export async function mergeRefs(req: MergeRequest): Promise<MergeResult> {
	const resp = await hubFetch(`/api/merge`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(req)
	});
	if (resp.status === 409) {
		// Hub returns the conflict envelope as the response body (text-encoded JSON).
		const text = await resp.text();
		try {
			return JSON.parse(text) as MergeConflict;
		} catch {
			throw new Error(`merge conflict (unparseable): ${text}`);
		}
	}
	if (!resp.ok) {
		const msg = await resp.text();
		throw new Error(`merge failed: ${resp.status} ${msg}`);
	}
	return resp.json();
}

export interface CreateBranchRequest {
	name: string;
	from?: string;
}

export async function createBranch(
	req: CreateBranchRequest
): Promise<{ status: string; name: string; commit_id: string }> {
	const resp = await hubFetch(`/api/branches`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ name: req.name, from: req.from ?? 'main' })
	});
	if (!resp.ok) {
		const msg = await resp.text();
		throw new Error(`branch failed: ${resp.status} ${msg}`);
	}
	return resp.json();
}

export interface PinnedItem {
	path: string;
	value: unknown;
}

export async function getPinned(): Promise<PinnedItem[]> {
	return fetchJson('/api/memory/pinned');
}

export interface PrimeSection {
	title: string;
	body: string;
}

export interface PrimeResult {
	status: string;
	source: string;
	pinned: boolean;
	sections_written: number;
	paths: string[];
}

export async function primeSections(
	source: string,
	pinned: boolean,
	sections: PrimeSection[]
): Promise<PrimeResult> {
	const resp = await hubFetch(`/api/memory/prime`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ source, pinned, sections })
	});
	if (!resp.ok) {
		throw new Error(`prime failed: ${resp.status}`);
	}
	return resp.json();
}

/**
 * A registered project: maps a code repo to the ASG namespace holding
 * its branches, plans, memory, taints, and history.
 */
export interface Project {
	id: string;
	remote_url: string | null;
	namespace: string;
	display_name: string | null;
	created_at: string;
	local_paths: string[];
	asd_repos: string[];
}

export async function listProjects(): Promise<Project[]> {
	return fetchJson('/api/projects');
}

export async function getProject(id: string): Promise<Project> {
	return fetchJson(`/api/projects/${encodeURIComponent(id)}`);
}

export interface RegisterProjectRequest {
	id: string;
	remote_url?: string;
	namespace?: string;
	display_name?: string;
	local_path?: string;
}

/** POST /api/projects — creates the namespace; 409 on duplicate id/remote_url. */
export async function registerProject(req: RegisterProjectRequest): Promise<Project> {
	const resp = await hubFetch('/api/projects', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(req)
	});
	if (!resp.ok) {
		const msg = await resp.text();
		throw new Error(`register project failed: ${resp.status} ${msg}`);
	}
	return resp.json();
}

/// Parse markdown into sections at H1/H2 headings. Ported from cli/src/main.rs.
export function parseMarkdownSections(content: string): PrimeSection[] {
	const sections: PrimeSection[] = [];
	let currentTitle: string | null = null;
	let currentBody: string[] = [];

	const flush = () => {
		const body = currentBody.join('\n').trim();
		if (!body) return;
		sections.push({
			title: currentTitle ?? 'Intro',
			body
		});
	};

	for (const line of content.split('\n')) {
		const trimmed = line.replace(/^\s+/, '');
		const isH1 = trimmed.startsWith('# ') && !trimmed.startsWith('## ');
		const isH2 = trimmed.startsWith('## ') && !trimmed.startsWith('### ');

		if (isH1 || isH2) {
			flush();
			currentBody = [];
			const prefixLen = isH1 ? 2 : 3;
			currentTitle = trimmed.slice(prefixLen).trim();
		} else {
			currentBody.push(line);
		}
	}
	flush();

	return sections;
}
