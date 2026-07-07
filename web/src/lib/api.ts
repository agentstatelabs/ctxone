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
	session_tokens_saved: number;
	total_graph_size_chars: number;
	total_graph_size_tokens: number;
	cumulative_ratio: number;
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

export async function getBlame(ref_name = 'main', path: string): Promise<BlameEntry[]> {
	return fetchJson(`/api/blame/${encodeURIComponent(ref_name)}?path=${encodeURIComponent(path)}`);
}

export async function getBranches(): Promise<Array<{ name: string; id: string }>> {
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

export async function recall(topic: string, budget = 1500): Promise<unknown> {
	return fetchJson(
		`/api/memory/recall?topic=${encodeURIComponent(topic)}&budget=${budget}`
	);
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
