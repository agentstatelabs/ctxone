/**
 * CTXone Code Lens — HTTP client for the ASD REST API.
 *
 * Multi-repo model:
 *   - Dev direct:  VITE_ASD_API_URL=http://localhost:8787 → all calls go to
 *                  <VITE_ASD_API_URL>/api/v1/*
 *   - CTX-hub:     /api/code/{repo}/* is proxied to the named ASD instance.
 *                  Repo list comes from GET /api/code.
 *
 * The `repo` parameter in each function selects which hub-registered ASD
 * instance to target. Pass the empty string when using VITE_ASD_API_URL.
 */

import type {
	AsdHealth,
	AsdRepoInfo,
	CallGraphResponse,
	FileEntry,
	PriorThinking,
	SearchResult,
	SymbolDetail,
	SymbolSummary
} from './codeTypes';

/** When VITE_ASD_API_URL is set we talk directly to one ASD process (dev mode). */
const DIRECT_ASD: string | undefined = import.meta.env.VITE_ASD_API_URL as string | undefined;

/** Resolve the API base for a given repo name. */
function base(repo: string): string {
	if (DIRECT_ASD) return `${DIRECT_ASD.replace(/\/$/, '')}/api/v1`;
	return `/api/code/${encodeURIComponent(repo)}`;
}

async function getJson<T>(repo: string, path: string): Promise<T> {
	const url = `${base(repo)}${path}`;
	const res = await fetch(url);
	if (!res.ok) throw new Error(`ASD API ${res.status} — ${url}`);
	return res.json() as Promise<T>;
}

async function getText(repo: string, path: string): Promise<string> {
	const url = `${base(repo)}${path}`;
	const res = await fetch(url);
	if (!res.ok) throw new Error(`ASD API ${res.status} — ${url}`);
	return res.text();
}

/** List all ASD repos registered with CTX-hub. Returns [] when using VITE_ASD_API_URL. */
export async function listAsdRepos(): Promise<AsdRepoInfo[]> {
	if (DIRECT_ASD) return [];
	try {
		const res = await fetch('/api/code');
		if (!res.ok) return [];
		return res.json() as Promise<AsdRepoInfo[]>;
	} catch {
		return [];
	}
}

/** Warm a pool-managed repo by spawning its asd-serve child. No-op (200) for
 *  static URLs and for already-running pool entries. */
export async function prefetchAsdRepo(repo: string): Promise<void> {
	if (DIRECT_ASD) return;
	try {
		await fetch(`/api/code/${encodeURIComponent(repo)}/prefetch`, { method: 'POST' });
	} catch {
		// best-effort warm — surfacing a fetch error here would be noisier than useful
	}
}

export async function getAsdHealth(repo: string): Promise<AsdHealth | null> {
	try {
		return await getJson<AsdHealth>(repo, '/health');
	} catch {
		return null;
	}
}

export function getSymbols(repo: string): Promise<SymbolSummary[]> {
	return getJson<SymbolSummary[]>(repo, '/symbols');
}

export function getSymbolDetail(repo: string, qname: string): Promise<SymbolDetail> {
	return getJson<SymbolDetail>(repo, `/symbols/${encodeURIComponent(qname)}`);
}

export function getCallers(repo: string, qname: string): Promise<SymbolSummary[]> {
	return getJson<SymbolSummary[]>(repo, `/symbols/${encodeURIComponent(qname)}/callers`);
}

export function getCallees(repo: string, qname: string): Promise<SymbolSummary[]> {
	return getJson<SymbolSummary[]>(repo, `/symbols/${encodeURIComponent(qname)}/callees`);
}

export function getCallGraph(repo: string, qname: string, hops = 1): Promise<CallGraphResponse> {
	return getJson<CallGraphResponse>(
		repo,
		`/symbols/${encodeURIComponent(qname)}/callgraph?hops=${hops}`
	);
}

export interface SearchParams {
	q: string;
	kind?: string;
	language?: string;
	limit?: number;
}

export function searchSymbols(repo: string, params: SearchParams): Promise<SearchResult[]> {
	const p = new URLSearchParams({ q: params.q });
	if (params.kind) p.set('kind', params.kind);
	if (params.language) p.set('language', params.language);
	if (params.limit) p.set('limit', String(params.limit));
	return getJson<SearchResult[]>(repo, `/search?${p}`);
}

export function listFiles(repo: string): Promise<FileEntry[]> {
	return getJson<FileEntry[]>(repo, '/files');
}

export function readFile(repo: string, path: string): Promise<string> {
	return getText(repo, `/files/${path}`);
}

export function getSymbolsByFile(repo: string, file: string): Promise<SymbolSummary[]> {
	return getSymbols(repo).then((all) => all.filter((s) => s.file === file));
}

// ---------------------------------------------------------------------------
// Plan G / K thinking projection
// ---------------------------------------------------------------------------

/** Workspace-wide thinking projection. `qnames` filters to specific symbols;
 *  omit to scan everything in the index. */
export function getThinking(
	repo: string,
	qnames?: string[],
	minConfidence?: number
): Promise<PriorThinking> {
	const p = new URLSearchParams();
	if (qnames && qnames.length) p.set('qnames', qnames.join(','));
	if (minConfidence !== undefined) p.set('min_confidence', String(minConfidence));
	const qs = p.toString();
	return getJson<PriorThinking>(repo, `/thinking${qs ? `?${qs}` : ''}`);
}

/** Single-symbol thinking — for the "Inherited thinking" panel on the
 *  symbol detail page. */
export function getSymbolThinking(
	repo: string,
	qname: string,
	minConfidence?: number
): Promise<PriorThinking> {
	const qs = minConfidence !== undefined ? `?min_confidence=${minConfidence}` : '';
	return getJson<PriorThinking>(repo, `/symbols/${encodeURIComponent(qname)}/thinking${qs}`);
}
