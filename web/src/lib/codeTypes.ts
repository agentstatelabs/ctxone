/**
 * CTXone Code Lens types.
 *
 * The ASD read-API payload shapes moved to @agentstate/lens-core (single
 * source of truth shared with ASD Lens) — they are re-exported here so
 * existing `$lib/codeTypes` imports keep working. Only the CTX-hub-specific
 * registry types are defined locally.
 *
 * Note: the `SymbolDetail` DATA type is intentionally not re-exported —
 * `SymbolDetail` at the lens-core root is the component; the payload type
 * only matters inside the shared components now.
 */

export type {
	SymbolKind,
	Position,
	Symbol,
	SymbolSummary,
	EffectCategory,
	Effect,
	TransitiveEffect,
	Verification,
	EffectDecl,
	LedgerKind,
	Author,
	LedgerEntry,
	SearchResult,
	FileEntry,
	CallGraphNode,
	CallGraphEdge,
	CallGraphResponse,
	ThinkingKind,
	ThinkingHypothesis,
	ThinkingMentalModel,
	ThinkingOpenQuestion,
	ThinkingFailedAttempt,
	ThinkingEntries,
	ThinkingSummary,
	PriorThinking
} from '@agentstate/lens-core';

export interface AsdHealth {
	status: string;
	name: string;
	db_path: string;
	symbol_count: number;
}

/** One entry from GET /api/code — the CTX-hub repo registry. */
export interface AsdRepoInfo {
	name: string;
	url: string;
	/** "static" (pre-running URL) or "pool" (hub spawns asd-serve on demand). */
	source?: 'static' | 'pool';
	/** "running" (process live or static URL) or "idle" (pool, not yet spawned). */
	status?: 'running' | 'idle';
}
