/**
 * CTXone Code Lens — HTTP client for the ASD REST API.
 *
 * Dev: default to ASD server on 8787. Embedded (ctxone-hub --asd-url):
 * proxy lives at /api/code/*. Explicit VITE_ASD_API_URL overrides both.
 */

import type {
	AsdHealth,
	CallGraphResponse,
	FileEntry,
	SearchResult,
	SymbolDetail,
	SymbolSummary
} from './codeTypes';

const ASD_BASE: string =
	(import.meta.env.VITE_ASD_API_URL as string | undefined) ??
	(import.meta.env.DEV ? 'http://localhost:8787' : '/api/code');

async function getJson<T>(path: string): Promise<T> {
	const res = await fetch(`${ASD_BASE}${path}`);
	if (!res.ok) {
		throw new Error(`ASD API ${res.status} — ${path}`);
	}
	return res.json() as Promise<T>;
}

async function getText(path: string): Promise<string> {
	const res = await fetch(`${ASD_BASE}${path}`);
	if (!res.ok) {
		throw new Error(`ASD API ${res.status} — ${path}`);
	}
	return res.text();
}

export async function getAsdHealth(): Promise<AsdHealth | null> {
	try {
		return await getJson<AsdHealth>('/api/v1/health');
	} catch {
		return null;
	}
}

export function getSymbols(): Promise<SymbolSummary[]> {
	return getJson<SymbolSummary[]>('/api/v1/symbols');
}

export function getSymbolDetail(qname: string): Promise<SymbolDetail> {
	return getJson<SymbolDetail>(`/api/v1/symbols/${encodeURIComponent(qname)}`);
}

export function getCallers(qname: string): Promise<SymbolSummary[]> {
	return getJson<SymbolSummary[]>(`/api/v1/symbols/${encodeURIComponent(qname)}/callers`);
}

export function getCallees(qname: string): Promise<SymbolSummary[]> {
	return getJson<SymbolSummary[]>(`/api/v1/symbols/${encodeURIComponent(qname)}/callees`);
}

export function getCallGraph(qname: string, hops = 1): Promise<CallGraphResponse> {
	return getJson<CallGraphResponse>(
		`/api/v1/symbols/${encodeURIComponent(qname)}/callgraph?hops=${hops}`
	);
}

export interface SearchParams {
	q: string;
	kind?: string;
	language?: string;
	limit?: number;
}

export function searchSymbols(params: SearchParams): Promise<SearchResult[]> {
	const p = new URLSearchParams({ q: params.q });
	if (params.kind) p.set('kind', params.kind);
	if (params.language) p.set('language', params.language);
	if (params.limit) p.set('limit', String(params.limit));
	return getJson<SearchResult[]>(`/api/v1/search?${p}`);
}

export function listFiles(): Promise<FileEntry[]> {
	return getJson<FileEntry[]>('/api/v1/files');
}

export function readFile(path: string): Promise<string> {
	return getText(`/api/v1/files/${path}`);
}

export function getSymbolsByFile(file: string): Promise<SymbolSummary[]> {
	return getSymbols().then((all) => all.filter((s) => s.file === file));
}
