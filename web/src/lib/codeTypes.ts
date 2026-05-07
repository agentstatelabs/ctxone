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
}
