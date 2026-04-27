/**
 * CtxOne Lens — HTTP client for Hub taint / quarantine / watch endpoints.
 */

const API_BASE: string = import.meta.env.VITE_CTXONE_API_URL
	?? (import.meta.env.DEV ? 'http://localhost:3001' : '');

async function fetchJson<T>(path: string): Promise<T> {
	const resp = await fetch(`${API_BASE}${path}`);
	if (!resp.ok) {
		throw new Error(`API error: ${resp.status} ${resp.statusText}`);
	}
	return resp.json();
}

export interface TaintRecord {
	id: string;
	path: string;
	name: string;
	kind: 'taint' | 'quarantine' | 'watch';
	effect: 'warn' | 'block' | 'review' | 'isolate' | 'advisory';
	severity: 'low' | 'medium' | 'high' | 'critical';
	reason: string;
	agent_id: string;
	created_at: string;
	resolved_at?: string;
}

export interface TaintCheck {
	can_write: boolean;
	effect?: string;
	matching_taint_id?: string;
	required_confidence?: number;
	isolated: boolean;
	warnings: string[];
}

export async function listTaints(
	pathPrefix?: string,
	kind?: string,
	limit = 50
): Promise<TaintRecord[]> {
	const params = new URLSearchParams();
	if (pathPrefix) params.set('path_prefix', pathPrefix);
	if (kind) params.set('kind', kind);
	params.set('limit', String(limit));
	const data = await fetchJson<{ taints: TaintRecord[] }>(`/api/taint?${params.toString()}`);
	return data.taints;
}

export async function checkTaint(
	path: string,
	agentId: string,
	confidence: number
): Promise<TaintCheck> {
	const params = new URLSearchParams();
	params.set('path', path);
	params.set('agent_id', agentId);
	params.set('confidence', String(confidence));
	return fetchJson<TaintCheck>(`/api/taint/check?${params.toString()}`);
}

export async function applyTaint(body: {
	path: string;
	name: string;
	kind: TaintRecord['kind'];
	effect: TaintRecord['effect'];
	severity: TaintRecord['severity'];
	reason: string;
	agent_id: string;
}): Promise<{ taint_id: string; path: string; created_at: string }> {
	const resp = await fetch(`${API_BASE}/api/taint`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(body)
	});
	if (!resp.ok) {
		throw new Error(`API error: ${resp.status} ${resp.statusText}`);
	}
	return resp.json();
}

export async function removeTaint(
	id: string,
	reason: string,
	agentId: string
): Promise<{ resolved_at: string }> {
	const resp = await fetch(`${API_BASE}/api/taint/${encodeURIComponent(id)}`, {
		method: 'DELETE',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ reason, agent_id: agentId })
	});
	if (!resp.ok) {
		throw new Error(`API error: ${resp.status} ${resp.statusText}`);
	}
	return resp.json();
}
