/**
 * CTXone Code Lens — transport + hub-registry helpers for the ASD REST API.
 *
 * Multi-repo model:
 *   - Dev direct:  VITE_ASD_API_URL=http://localhost:8787 → all calls go to
 *                  <VITE_ASD_API_URL>/api/v1/*
 *   - CTX-hub:     /api/code/{repo}/* is proxied to the named ASD instance.
 *                  Repo list comes from GET /api/code.
 *
 * The per-symbol/search/file/graph/thinking methods now live on the shared
 * `AsdClient` from @agentstate/lens-core — build one for the selected repo
 * with `codeClient(repo)`. What remains here is the repo-registry surface
 * (hub-only: list/prefetch/health) plus the couple of raw helpers the
 * non-extracted pages still use.
 */

import { createAsdClient, createHttpTransport, type AsdClient } from '@agentstate/lens-core';
import type { AsdHealth, AsdRepoInfo, FileEntry, SymbolSummary } from './codeTypes';

/** When VITE_ASD_API_URL is set we talk directly to one ASD process (dev mode). */
const DIRECT_ASD: string | undefined = import.meta.env.VITE_ASD_API_URL as string | undefined;

/** Resolve the API base for a given repo name. */
function base(repo: string): string {
	if (DIRECT_ASD) return `${DIRECT_ASD.replace(/\/$/, '')}/api/v1`;
	return `/api/code/${encodeURIComponent(repo)}`;
}

/** Typed ASD client bound to one hub-registered repo (or the dev-direct URL). */
export function codeClient(repo: string): AsdClient {
	return createAsdClient(createHttpTransport(base(repo)));
}

async function getJson<T>(repo: string, path: string): Promise<T> {
	const url = `${base(repo)}${path}`;
	const res = await fetch(url);
	if (!res.ok) throw new Error(`ASD API ${res.status} — ${url}`);
	return res.json() as Promise<T>;
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

export function listFiles(repo: string): Promise<FileEntry[]> {
	return getJson<FileEntry[]>(repo, '/files');
}
