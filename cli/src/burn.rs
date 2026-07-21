//! Session burn metric, computed at ingest.
//!
//! A direct port of the Lens's `web/src/lib/sessionBurn.ts` — the calibrated
//! constants and the scoring are identical, and MUST stay in lockstep with it.
//! The point of computing it here is that a session's turns are already parsed
//! during ingest, so the score can be stored on the session; the dashboard's
//! burn board then reads a number instead of re-downloading and re-scanning
//! every transcript (~29MB / 14s for a full scan today).
//!
//! ## The metric, in one line
//!
//! Cost-per-edit over a trailing window, relative to the session's own early
//! baseline. Higher is worse. See sessionBurn.ts for the full calibration
//! writeup (the 3.0/8.0 ladder is measured against the rolling-window
//! distribution, not intuition).
//!
//! DIRECTION is the axis most likely to get flipped (CLAUDE.md, the
//! 1.0.59-1.0.68 arc): `productive` is the GOOD bucket at the LOW end;
//! `burning` is BAD at the HIGH end. A test asserts it.

use crate::ingest::Turn;

/// Tool names that change the repo. Everything else is input-gathering.
/// Kept in sync with `MUTATING` in sessionBurn.ts.
const MUTATING: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

const WINDOW: usize = 10;
const MIN_BASELINE_EDITS: u32 = 3;
const MIN_TURNS: usize = 12;
const T_DIMINISHING: f64 = 3.0;
const T_BURNING: f64 = 8.0;

/// The stored score. `level` is always set; the numbers are `None` for an
/// `unknown` verdict so a reader cannot mistake a guessed 0.0 for a real one.
#[derive(Debug, Clone, PartialEq)]
pub struct BurnSummary {
    pub level: &'static str,
    pub ratio: Option<f64>,
    pub baseline: Option<f64>,
    pub recent: Option<f64>,
    pub turns: usize,
}

impl BurnSummary {
    fn unknown(turns: usize) -> Self {
        Self {
            level: "unknown",
            ratio: None,
            baseline: None,
            recent: None,
            turns,
        }
    }

    /// As the JSON stored at `/sessions/<sid>/burn`. `updated_at` is threaded
    /// in by the caller (the session's last-turn timestamp) so the dashboard
    /// can tell a stale score from a current one without re-scanning.
    pub fn to_json(&self, updated_at: &str) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        o.insert("level".into(), serde_json::json!(self.level));
        o.insert("turns".into(), serde_json::json!(self.turns));
        if let Some(r) = self.ratio {
            o.insert("ratio".into(), serde_json::json!(round2(r)));
        }
        if let Some(b) = self.baseline {
            o.insert("baseline".into(), serde_json::json!(round2(b)));
        }
        if let Some(r) = self.recent {
            o.insert("recent".into(), serde_json::json!(round2(r)));
        }
        if !updated_at.is_empty() {
            // The gate: a stored burn whose `updated_at` matches the session's
            // is current, and the dashboard can use `ratio` directly.
            o.insert("updated_at".into(), serde_json::json!(updated_at));
        }
        serde_json::Value::Object(o)
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

struct PerTurn {
    context: f64,
    edits: u32,
}

/// The tool name from a `"Read: /path"` / bare `"ToolSearch"` string.
fn tool_name(call: &str) -> &str {
    match call.find(':') {
        Some(i) => call[..i].trim(),
        None => call.trim(),
    }
}

fn per_turn(t: &Turn) -> PerTurn {
    let calls = t.tool_calls.len().max(1) as f64;
    // Per-call context size: the turn's cache traffic spread over its calls.
    let context = (t.tokens.cache_read + t.tokens.cache_creation) as f64 / calls;
    let edits = t
        .tool_calls
        .iter()
        .filter(|c| MUTATING.contains(&tool_name(c)))
        .count() as u32;
    PerTurn { context, edits }
}

fn edits_in(rows: &[PerTurn]) -> u32 {
    rows.iter().map(|r| r.edits).sum()
}

/// Aggregate cost-per-edit over a slice, or `None` when it has no edits.
fn cost_per_edit(rows: &[PerTurn]) -> Option<f64> {
    let ed: u32 = rows.iter().map(|r| r.edits).sum();
    if ed == 0 {
        return None;
    }
    let ctx: f64 = rows.iter().map(|r| r.context).sum();
    Some(ctx / ed as f64)
}

/// Score a session's turns. Turns must be in chronological order. Mirrors
/// `computeBurn` in sessionBurn.ts; returns `unknown` rather than a guessed
/// level when the session is too short or the baseline too edit-sparse.
pub fn compute(turns: &[Turn]) -> BurnSummary {
    let n = turns.len();
    if n < MIN_TURNS {
        return BurnSummary::unknown(n);
    }

    let rows: Vec<PerTurn> = turns.iter().map(per_turn).collect();

    // Baseline = the session's own opening third (min one window).
    let base_end = WINDOW.max(n / 3);
    let base_rows = &rows[..base_end];

    if edits_in(base_rows) < MIN_BASELINE_EDITS {
        return BurnSummary::unknown(n);
    }
    let baseline = match cost_per_edit(base_rows) {
        Some(b) if b > 0.0 => b,
        _ => return BurnSummary::unknown(n),
    };

    let recent_rows = &rows[n.saturating_sub(WINDOW)..];
    let recent_edits = edits_in(recent_rows);
    let recent_ctx: f64 = recent_rows.iter().map(|r| r.context).sum();

    // Zero edits in the trailing window is the strongest burn signal: full
    // context price, nothing landed. Score it as one edit's worth so it ranks
    // above ordinary decay rather than dividing by zero.
    let recent = if recent_edits > 0 {
        recent_ctx / recent_edits as f64
    } else {
        recent_ctx
    };
    let ratio = recent / baseline;

    let level = if ratio >= T_BURNING {
        "burning"
    } else if ratio >= T_DIMINISHING {
        "diminishing"
    } else {
        "productive"
    };

    BurnSummary {
        level,
        ratio: Some(ratio),
        baseline: Some(baseline),
        recent: Some(recent),
        turns: n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::TurnTokens;

    /// A turn with `edits` mutating calls, `reads` reads, and a per-call
    /// context size. Mirrors the TS test's `turn()` helper so the two ports
    /// are checked against the same shapes.
    fn turn(edits: usize, reads: usize, per_call_context: u64) -> Turn {
        let mut calls: Vec<String> = Vec::new();
        for i in 0..edits {
            calls.push(format!("Edit: /src/f{i}.rs"));
        }
        for i in 0..reads {
            calls.push(format!("Read: /src/r{i}.rs"));
        }
        let n = calls.len().max(1) as u64;
        Turn {
            user_text: "u".into(),
            assistant_text: "a".into(),
            tool_calls: calls,
            tool_calls_raw: vec![],
            tokens: TurnTokens {
                cache_read: per_call_context * n,
                ..Default::default()
            },
            model: "m".into(),
            timestamp: "t".into(),
            cwd: None,
            git_branches: vec![],
            git_commit: None,
        }
    }

    fn steady(n: usize) -> Vec<Turn> {
        (0..n).map(|_| turn(3, 3, 100_000)).collect()
    }

    #[test]
    fn short_session_is_unknown() {
        assert_eq!(compute(&steady(MIN_TURNS - 1)).level, "unknown");
    }

    #[test]
    fn edit_sparse_baseline_is_unknown() {
        // Research-first: heavy reading, no early edits, edits arrive late.
        let mut turns: Vec<Turn> = (0..20).map(|_| turn(0, 6, 100_000)).collect();
        turns.extend((0..10).map(|_| turn(4, 2, 100_000)));
        assert_eq!(compute(&turns).level, "unknown");
    }

    #[test]
    fn steady_session_is_productive() {
        let r = compute(&steady(40));
        assert_eq!(r.level, "productive");
        assert!(r.ratio.unwrap() < T_DIMINISHING);
    }

    #[test]
    fn higher_cost_per_edit_is_worse_not_better() {
        // The direction guard, ported from the TS test: `productive` is GOOD
        // and LOW, `burning` is BAD and HIGH. Asserted against cost, not the
        // implementation's own labels.
        let cheap = compute(&steady(40));
        let mut expensive = steady(20);
        expensive.extend((0..20).map(|_| turn(3, 3, 1_000_000)));
        let expensive = compute(&expensive);

        assert!(expensive.ratio.unwrap() > cheap.ratio.unwrap());
        assert_eq!(cheap.level, "productive");
        assert_eq!(expensive.level, "burning");
    }

    #[test]
    fn severe_collapse_is_burning() {
        // 4x context for a quarter of the edits => ~16x, past p85.
        let mut turns: Vec<Turn> = (0..20).map(|_| turn(4, 4, 100_000)).collect();
        turns.extend((0..20).map(|_| turn(1, 7, 400_000)));
        let r = compute(&turns);
        assert!(r.ratio.unwrap() >= T_BURNING);
        assert_eq!(r.level, "burning");
    }

    #[test]
    fn zero_edit_trailing_window_is_burning() {
        let mut turns: Vec<Turn> = (0..20).map(|_| turn(4, 2, 100_000)).collect();
        turns.extend((0..WINDOW).map(|_| turn(0, 8, 300_000)));
        assert_eq!(compute(&turns).level, "burning");
    }

    #[test]
    fn unknown_stores_no_numbers() {
        let j = BurnSummary::unknown(5).to_json("2026-01-01");
        assert!(j.get("ratio").is_none());
        assert_eq!(j["level"], "unknown");
        assert_eq!(j["updated_at"], "2026-01-01");
    }
}
