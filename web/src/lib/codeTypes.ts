export type SymbolKind = 'module' | 'function' | 'method' | 'class' | 'variable';

export interface Position {
	line: number;
	col: number;
}

export interface SymbolSummary {
	symbol_id: string;
	symbol_fp: string;
	qname: string;
	language: string;
	kind: SymbolKind;
	file: string;
	start: Position;
	end: Position;
	signature: string | null;
	doc: string | null;
}

export type EffectCategory =
	| 'io.fs.read'
	| 'io.fs.write'
	| 'io.net.in'
	| 'io.net.out'
	| 'io.db.read'
	| 'io.db.write'
	| 'state.global.read'
	| 'state.global.write'
	| 'state.process'
	| 'env.read'
	| 'time.read'
	| 'time.sleep'
	| 'random'
	| 'proc.spawn'
	| 'throw'
	| 'log'
	| 'pure';

export interface Effect {
	effect: EffectCategory;
	qualifiers: unknown;
	note: string | null;
}

export interface TransitiveEffect {
	effect: EffectCategory;
	via: string[];
	qualifiers: unknown;
}

export interface Verification {
	by: 'static-checker' | 'runtime-tracer' | 'test-observed';
	at: string;
	status: 'ok' | 'mismatch' | 'unverified';
	mismatches: unknown[];
}

export interface EffectDecl {
	symbol_id: string;
	declared: Effect[];
	transitive: TransitiveEffect[];
	verification: Verification | null;
	confidence: number | null;
	matched_policy: string | null;
}

export type LedgerKind = 'decision' | 'assumption' | 'constraint' | 'rationale' | 'hazard' | 'tradeoff';

export interface Author {
	kind: 'agent' | 'human';
	id: string;
}

export interface LedgerEntry {
	entry_id: string;
	symbol_id: string;
	kind: LedgerKind;
	summary: string;
	body?: string;
	author: Author;
	confidence?: number;
	evidence?: unknown[];
	supersedes?: string[];
	created_at: string;
	tags?: string[];
	matched_policy?: string;
}

export interface SymbolDetail {
	symbol: SymbolSummary;
	effects: EffectDecl | null;
	ledger: LedgerEntry[];
}

export interface SearchResult {
	score: number;
	qname: string;
	kind: SymbolKind;
	language: string;
	file: string;
	start: Position;
	signature: string | null;
	doc: string | null;
}

export interface FileEntry {
	path: string;
	language: string;
	symbol_count: number;
}

export interface CallGraphNode {
	id: string;
	qname: string;
	kind: SymbolKind;
	language: string;
	file: string;
	is_focal: boolean;
}

export interface CallGraphEdge {
	source: string;
	target: string;
}

export interface CallGraphResponse {
	nodes: CallGraphNode[];
	edges: CallGraphEdge[];
}

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

// ---------------------------------------------------------------------------
// Plan G / K — captured "thinking" (hypotheses, mental models, open
// questions, failed attempts). Mirrors PriorThinking / ThinkingSummary
// from agentstatedeveloper_core::thinking.
// ---------------------------------------------------------------------------

export type ThinkingKind = 'hypothesis' | 'mental_model' | 'open_question' | 'failed_attempt';

export interface ThinkingHypothesis {
	qname: string;
	summary: string;
	confidence: number;
	body?: string;
}
export interface ThinkingMentalModel {
	name?: string;
	summary: string;
	symbols?: string[];
	body?: string;
}
export interface ThinkingOpenQuestion {
	qname: string;
	summary: string;
	body?: string;
}
export interface ThinkingFailedAttempt {
	qname: string;
	summary: string;
	body?: string;
}

/** `entries` is `null` when nothing surfaces; otherwise the projection object. */
export interface ThinkingEntries {
	hypotheses?: ThinkingHypothesis[];
	mental_models?: ThinkingMentalModel[];
	open_questions?: ThinkingOpenQuestion[];
	failed_attempts?: ThinkingFailedAttempt[];
}

/** Always-emitted metadata. `surfaced > 0` is the boolean for "show entries". */
export interface ThinkingSummary {
	scanned_qnames?: number;
	matched_for_query?: number;
	surfaced?: number;
	by_kind?: Partial<Record<ThinkingKind, number>>;
	/** Load-bearing: when `by_kind_dropped.hypothesis > 0` and
	 *  `by_kind.hypothesis === 0`, hypotheses exist but fell below the
	 *  confidence floor. */
	by_kind_dropped?: Partial<Record<ThinkingKind, number>>;
	entries_in_workspace?: number;
}

export interface PriorThinking {
	entries: ThinkingEntries | null;
	summary: ThinkingSummary;
}
