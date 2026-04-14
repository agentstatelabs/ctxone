/**
 * Shared TypeScript types for CtxOne Lens.
 */

export interface MemoryFact {
	path: string;
	value: unknown;
	confidence?: number;
	timestamp?: string;
	tags?: string[];
}

export interface SessionSummary {
	session_id: string;
	summary: string;
	decisions: string[];
	details: string[];
	timestamp: string;
}

export interface ProjectContext {
	name: string;
	status: string;
	recent_work: string[];
	decisions: Record<string, string>;
}

export interface TokenSavingsDisplay {
	sent: number;
	flat_estimate: number;
	ratio: number;
	total_saved: number;
}
