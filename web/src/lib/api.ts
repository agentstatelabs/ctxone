/**
 * CtxOne Lens — HTTP client for the Hub REST API.
 */

const API_BASE = import.meta.env.VITE_CTXONE_API_URL || 'http://localhost:3001';

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
	const resp = await fetch(`${API_BASE}${path}`);
	if (!resp.ok) {
		throw new Error(`API error: ${resp.status} ${resp.statusText}`);
	}
	return resp.json();
}

async function fetchText(path: string): Promise<string> {
	const resp = await fetch(`${API_BASE}${path}`);
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
	return fetchJson(`/api/stats/${ref_name}`);
}

export async function getTokenStats(): Promise<TokenStats> {
	return fetchJson('/api/stats/tokens');
}

export async function getState(ref_name = 'main', path = '/'): Promise<unknown> {
	return fetchJson(`/api/state/${ref_name}?path=${encodeURIComponent(path)}`);
}

export async function listPaths(ref_name = 'main', prefix = '/'): Promise<string[]> {
	return fetchJson(`/api/state/${ref_name}/paths?prefix=${encodeURIComponent(prefix)}`);
}

export async function searchValues(ref_name = 'main', query: string): Promise<SearchResult[]> {
	return fetchJson(`/api/state/${ref_name}/search?query=${encodeURIComponent(query)}`);
}

export async function getLog(ref_name = 'main', limit = 20): Promise<CommitEntry[]> {
	return fetchJson(`/api/log/${ref_name}?limit=${limit}`);
}

export async function getBlame(ref_name = 'main', path: string): Promise<BlameEntry[]> {
	return fetchJson(`/api/blame/${ref_name}?path=${encodeURIComponent(path)}`);
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
	const resp = await fetch(`${API_BASE}/api/memory/remember`, {
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
