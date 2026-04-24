/**
 * CtxOne Lens — HTTP client for team-tier Hub REST endpoints.
 */

const API_BASE: string = import.meta.env.VITE_CTXONE_API_URL
	?? (import.meta.env.DEV ? 'http://localhost:3001' : '');

export interface TeamMember {
	id: string;
	kind: 'agent' | 'human';
	last_seen: string;
	commit_count: number;
}

export interface TeamActivityEntry {
	commit_id: string;
	timestamp: string;
	agent_id: string;
	path: string;
	message: string;
	category: string;
}

export interface TeamContributor {
	agent_id: string;
	tokens_saved: number;
}

export interface TeamSavings {
	total_tokens_saved: number;
	savings_ratio: number;
	top_contributors: TeamContributor[];
}

export interface TeamGraphNode {
	path: string;
	agent_id: string;
	timestamp: string;
	value_preview: string;
}

export interface TeamGraphResponse {
	prefix: string;
	branch: string;
	nodes: TeamGraphNode[];
}

async function fetchJson<T>(path: string): Promise<T> {
	const resp = await fetch(`${API_BASE}${path}`);
	if (!resp.ok) {
		throw new Error(`API error: ${resp.status} ${resp.statusText}`);
	}
	return resp.json();
}

export async function getTeamMembers(): Promise<TeamMember[]> {
	return fetchJson('/api/team/members');
}

export async function getTeamActivity(
	limit = 20,
	before?: string
): Promise<TeamActivityEntry[]> {
	const params = new URLSearchParams();
	params.set('limit', String(limit));
	if (before) params.set('before', before);
	return fetchJson(`/api/team/activity?${params.toString()}`);
}

export async function getTeamSavings(): Promise<TeamSavings> {
	return fetchJson('/api/team/savings');
}

export async function getTeamGraph(
	prefix: string,
	branch: string
): Promise<TeamGraphResponse> {
	const params = new URLSearchParams();
	params.set('prefix', prefix);
	params.set('branch', branch);
	return fetchJson(`/api/team/graph?${params.toString()}`);
}
