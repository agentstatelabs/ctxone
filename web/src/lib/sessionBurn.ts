/**
 * Session burn metric — "am I still being productive, or just paying for
 * context?"
 *
 * ## The measurement
 *
 * The honest unit of a coding session is **context tokens spent per edit
 * landed**. Reading, grepping and reasoning are inputs; the session only
 * moves when a file changes. So:
 *
 *     cost_per_edit = context_tokens / mutating_tool_calls
 *
 * `context_tokens` is `cache_read + cache_creation` divided by the number of
 * tool calls in the turn. That division matters: an ingested "turn" folds
 * together every assistant message until the next user message, so its raw
 * `cache_read` is a SUM over many API calls. Dividing by call count recovers
 * an estimate of the context size each call actually carried.
 *
 * We then compare a trailing window against the session's own early baseline.
 *
 * ## Why a ratio and not an absolute number
 *
 * Measured over 14 real sessions (>40 LLM calls each) from this Hub, the
 * early-session cost-per-edit ranged from **29k to 2.07M** context tokens —
 * a ~70x spread driven by task type, not by health. A session doing Rust
 * refactors and one doing repo archaeology are not comparable in absolute
 * terms. Any fixed threshold would flag half the healthy sessions and miss
 * half the burning ones. The metric is therefore always *self-relative*.
 *
 * ## Threshold ladder — calibrated at the stage the user actually sees
 *
 * Calibrate at the stage that produces the number being labelled. The first
 * cut of this metric was calibrated on a *third-vs-third* comparison of the
 * same 14 sessions:
 *
 *     12 of 14 degraded; p25 2.01x, median 3.55x, p75 6.15x
 *
 * ...and a 2.0/4.0 ladder was drawn from it. But that is not what the metric
 * computes. It computes a **trailing 10-turn window against the baseline**,
 * which runs far hotter than a third-vs-third aggregate: a short window has
 * none of the averaging a 55-turn third does. Measured over every rolling
 * window past the baseline in those same sessions:
 *
 *     p25 1.1x   p50 2.38x   p75 6.48x   p85 13.7x   p95 34.0x
 *
 * The 2.0/4.0 ladder against THAT distribution labels the median window
 * "diminishing" and a full quarter of all windows "burning" — an alarm that
 * fires constantly is one you learn to ignore. The ladder below is drawn
 * from the rolling-window distribution instead:
 *
 *     ratio <  3.0   productive    at/below the typical window (p50 2.38x)
 *     ratio 3.0-8.0  diminishing   ~p60-p80; real decay, not yet worth a reset
 *     ratio >= 8.0   burning       ~top 20% of windows; costing 8x+ your own
 *                                  best rate per edit
 *
 * On the 14-session sample the end-of-session verdicts land 6 burning /
 * 6 diminishing / 2 productive — plausible for *completed* long sessions,
 * which is what those are. The earlier ladder called 10 of 14 "burning".
 *
 * (This is the same failure ASD hit in the 1.0.85-1.0.88 cliff-detection arc:
 * right idea, wrong stage. See CLAUDE.md, "Multi-stage filtering".)
 *
 * DIRECTION (this axis is easy to invert — see CLAUDE.md on the 1.0.59-1.0.68
 * calibration arc): **higher ratio is WORSE.** `ratio` is cost-now divided by
 * cost-at-best, so 1.0 means "as efficient as when I started" and 8.0 means
 * "each edit now costs eight times what it did". `productive` is the GOOD
 * bucket and sits at the LOW end. `burnDirectionIsHigherWorse` is asserted in
 * the tests so a future refactor cannot silently flip it.
 *
 * ## Known false-positive guard
 *
 * Exploration-first sessions (read the whole codebase, then edit) have a
 * near-zero early edit count, which makes the baseline denominator explode
 * and produces a meaningless ratio. Session 0d1dd7aa in the sample had 0.20
 * edits/turn early and scored a nonsense 0.21x. When the baseline window
 * holds fewer than MIN_BASELINE_EDITS edits we report `unknown` rather than
 * a confident wrong answer.
 */

/** Tool names that change the repo. Everything else is input-gathering. */
const MUTATING = new Set(['Edit', 'Write', 'MultiEdit', 'NotebookEdit']);

/** Trailing turns compared against the baseline. */
export const WINDOW = 10;

/** Baseline needs at least this many edits to be a meaningful denominator. */
export const MIN_BASELINE_EDITS = 3;

/** Below this many turns there is no trend to speak of. */
export const MIN_TURNS = 12;

/** ratio < this = productive. Just above the observed rolling-window p50 (2.38x). */
export const T_DIMINISHING = 3.0;

/** ratio >= this = burning. Between the rolling-window p75 (6.5x) and p85 (13.7x). */
export const T_BURNING = 8.0;

export type BurnLevel = 'productive' | 'diminishing' | 'burning' | 'unknown';

export interface BurnTurn {
	tool_calls?: string[];
	tokens?: { input?: number; output?: number; cache_read?: number; cache_creation?: number };
}

export interface BurnResult {
	level: BurnLevel;
	/** Cost-per-edit now vs the session's baseline. Higher is worse. */
	ratio: number | null;
	baseline: number | null;
	recent: number | null;
	/** Per-turn rolling cost-per-edit ratio, for a sparkline. */
	series: number[];
	/** Turn index where the session first crossed into `burning` and stayed. */
	knee: number | null;
	/** Total context tokens spent since the knee — the "wasted" spend. */
	sinceKneeTokens: number;
	headline: string;
	detail: string;
}

interface PerTurn {
	context: number;
	edits: number;
	calls: number;
}

/** Tool call strings are stored as `"Read: /path"` or bare `"ToolSearch"`. */
export function toolName(call: string): string {
	const i = call.indexOf(':');
	return (i === -1 ? call : call.slice(0, i)).trim();
}

function perTurn(t: BurnTurn): PerTurn {
	const calls = t.tool_calls?.length ?? 0;
	const tk = t.tokens ?? {};
	// Per-call context size: the turn's cache traffic spread over its calls.
	const context = ((tk.cache_read ?? 0) + (tk.cache_creation ?? 0)) / Math.max(1, calls);
	const edits = (t.tool_calls ?? []).filter((c) => MUTATING.has(toolName(c))).length;
	return { context, edits, calls };
}

/** Aggregate cost-per-edit over a slice. `null` when the slice has no edits. */
function costPerEdit(rows: PerTurn[]): number | null {
	let ctx = 0;
	let ed = 0;
	for (const r of rows) {
		ctx += r.context;
		ed += r.edits;
	}
	return ed > 0 ? ctx / ed : null;
}

function editsIn(rows: PerTurn[]): number {
	return rows.reduce((a, r) => a + r.edits, 0);
}

function fmtTokens(n: number): string {
	if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
	if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
	return String(Math.round(n));
}

/**
 * Score a session's turns. Turns must be in chronological order.
 *
 * Returns `unknown` (never a guessed level) when the session is too short or
 * the baseline is too edit-sparse to divide by.
 */
export function computeBurn(turns: BurnTurn[]): BurnResult {
	const none = (headline: string, detail: string): BurnResult => ({
		level: 'unknown',
		ratio: null,
		baseline: null,
		recent: null,
		series: [],
		knee: null,
		sinceKneeTokens: 0,
		headline,
		detail
	});

	if (turns.length < MIN_TURNS) {
		return none('Too early to tell', `Needs ${MIN_TURNS}+ turns to establish a trend.`);
	}

	const rows = turns.map(perTurn);

	// Baseline = the session's own opening third (min one window). This is the
	// span the late-vs-early calibration above was measured on.
	const baseEnd = Math.max(WINDOW, Math.floor(rows.length / 3));
	const baseRows = rows.slice(0, baseEnd);

	if (editsIn(baseRows) < MIN_BASELINE_EDITS) {
		return none(
			'No productive baseline',
			'The opening of this session made almost no edits, so there is no rate to compare against. Common in research-first sessions.'
		);
	}

	const baseline = costPerEdit(baseRows);
	if (baseline === null || baseline <= 0) {
		return none('No productive baseline', 'Could not establish a baseline edit rate.');
	}

	const recentRows = rows.slice(-WINDOW);
	const recentEdits = editsIn(recentRows);
	const recentCtx = recentRows.reduce((a, r) => a + r.context, 0);

	// Zero edits in the trailing window is the strongest burn signal there is:
	// full context price, nothing landed. Score it as the ratio it would take
	// to spend that much with a single edit, so it ranks above ordinary decay.
	const recent = recentEdits > 0 ? recentCtx / recentEdits : recentCtx / 1;
	const ratio = recent / baseline;

	// Rolling series, for the sparkline and for locating the knee.
	const series: number[] = [];
	for (let i = 0; i < rows.length; i++) {
		const w = rows.slice(Math.max(0, i - WINDOW + 1), i + 1);
		const ed = editsIn(w);
		const ctx = w.reduce((a, r) => a + r.context, 0);
		series.push((ed > 0 ? ctx / ed : ctx) / baseline);
	}

	// Knee = first index that crosses into burning and stays there for the
	// rest of the run (a single spike is noise, a sustained shelf is not).
	let knee: number | null = null;
	for (let i = baseEnd; i < series.length; i++) {
		if (series[i] >= T_BURNING && series.slice(i).every((v) => v >= T_BURNING)) {
			knee = i;
			break;
		}
	}
	const sinceKneeTokens =
		knee === null
			? 0
			: rows.slice(knee).reduce((a, r) => a + r.context * Math.max(1, r.calls), 0);

	const level: BurnLevel =
		ratio >= T_BURNING ? 'burning' : ratio >= T_DIMINISHING ? 'diminishing' : 'productive';

	const x = `${ratio.toFixed(1)}x`;
	if (level === 'burning') {
		return {
			level,
			ratio,
			baseline,
			recent,
			series,
			knee,
			sinceKneeTokens,
			headline: `Burning — ${x} your best rate`,
			detail:
				recentEdits === 0
					? `No edits in the last ${WINDOW} turns while spending ${fmtTokens(recentCtx)} context tokens. Start a new session and bring only what matters.`
					: `Each edit now costs ${x} what it did earlier in this session. Start a new session and bring only what matters.`
		};
	}
	if (level === 'diminishing') {
		return {
			level,
			ratio,
			baseline,
			recent,
			series,
			knee,
			sinceKneeTokens,
			headline: `Diminishing — ${x} your best rate`,
			detail: `Still landing work, but each edit costs ${x} what it did earlier. Good point to finish the current thread rather than open a new one.`
		};
	}
	return {
		level,
		ratio,
		baseline,
		recent,
		series,
		knee,
		sinceKneeTokens,
		headline: `Productive — ${x} your best rate`,
		detail: 'Edits are landing at close to this session’s best cost per change.'
	};
}
