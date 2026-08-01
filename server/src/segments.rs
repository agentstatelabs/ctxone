//! Topic-arc segmentation of a session's turns (t-003, curated summaries).
//!
//! A session is rarely one topic — of the largest transcripts measured, most
//! spanned several branches. Ingested turns are already stored per-turn with
//! provenance (`cwd`, `git_branches`, timestamp), so we can split a session
//! into topic ARCS cheaply, with no LLM: a new arc starts when the git branch
//! changes, the working directory changes, or a long idle gap elapses.
//!
//! This is the foundation for user-curated summaries: segment first, then let
//! the agent draft candidate memories per arc and the user keep/drop whole
//! arcs. Pure and unit-tested; the HTTP/MCP layers just feed it turn JSON.

use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;

/// The minimal per-turn signal segmentation needs.
#[derive(Debug, Clone)]
pub struct TurnMeta {
    pub index: usize,
    pub cwd: Option<String>,
    /// Primary branch for the turn (first recorded), if any.
    pub branch: Option<String>,
    pub timestamp: Option<String>,
    /// First non-empty line of the user's message, for a human label.
    pub label_hint: Option<String>,
    pub tokens: u64,
}

/// One contiguous topic arc.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Segment {
    pub start: usize,
    pub end: usize,
    pub turn_count: usize,
    pub branch: Option<String>,
    pub cwd: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub tokens: u64,
    pub label: String,
    /// Why this arc started: "start" | "branch" | "cwd" | "gap".
    pub reason: &'static str,
}

/// Extract [`TurnMeta`] from a `/sessions/<id>/turns` subtree map
/// (`{ "t0000": {..turn json..}, ... }`), sorted by turn key.
pub fn metas_from_turns_tree(turns: &Value) -> Vec<TurnMeta> {
    let Some(obj) = turns.as_object() else {
        return Vec::new();
    };
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort(); // t0000 < t0001 lexically
    keys.into_iter()
        .enumerate()
        .map(|(i, k)| {
            let t = &obj[k];
            let index = t
                .get("turn_index")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(i);
            let branch = t
                .get("git_branches")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let tokens = t
                .get("tokens")
                .and_then(|v| v.as_object())
                .map(|m| m.values().filter_map(|x| x.as_u64()).sum())
                .unwrap_or(0);
            let label_hint = t
                .get("user_text")
                .and_then(|v| v.as_str())
                .and_then(|s| s.lines().map(str::trim).find(|l| !l.is_empty()))
                .map(|l| l.chars().take(80).collect());
            TurnMeta {
                index,
                cwd: t.get("cwd").and_then(|v| v.as_str()).map(str::to_string),
                branch,
                timestamp: t
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                label_hint,
                tokens,
            }
        })
        .collect()
}

/// Minutes between two RFC-3339 timestamps, if both parse.
fn gap_minutes(a: &Option<String>, b: &Option<String>) -> Option<i64> {
    let (a, b) = (a.as_deref()?, b.as_deref()?);
    let ta = DateTime::parse_from_rfc3339(a).ok()?;
    let tb = DateTime::parse_from_rfc3339(b).ok()?;
    Some((tb - ta).num_minutes().abs())
}

/// A readable arc label: the branch (minus a `plan/` prefix), else the cwd's
/// basename, else a snippet of the first user message, else the turn range.
fn label_for(
    branch: &Option<String>,
    cwd: &Option<String>,
    hint: &Option<String>,
    start: usize,
    end: usize,
) -> String {
    if let Some(b) = branch.as_deref().filter(|b| !b.is_empty() && *b != "HEAD") {
        return b.strip_prefix("plan/").unwrap_or(b).to_string();
    }
    if let Some(c) = cwd.as_deref().filter(|c| !c.is_empty()) {
        if let Some(base) = c.rsplit('/').next().filter(|b| !b.is_empty()) {
            return base.to_string();
        }
    }
    if let Some(h) = hint.as_deref().filter(|h| !h.is_empty()) {
        return h.to_string();
    }
    format!("turns {start}-{end}")
}

/// Segment turns into topic arcs. A new arc starts on a branch change, a cwd
/// change, or an idle gap longer than `gap_minutes` (<= 0 disables the gap
/// rule). Returns arcs in order; empty input yields no arcs.
pub fn segment_turns(turns: &[TurnMeta], gap_min: i64) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for t in turns {
        let boundary = match out.last() {
            None => Some("start"),
            Some(cur) => {
                if t.branch != cur.branch {
                    Some("branch")
                } else if t.cwd != cur.cwd {
                    Some("cwd")
                } else if gap_min > 0
                    && gap_minutes(&cur.ended_at, &t.timestamp).is_some_and(|g| g > gap_min)
                {
                    Some("gap")
                } else {
                    None
                }
            }
        };

        if let Some(reason) = boundary {
            let label = label_for(&t.branch, &t.cwd, &t.label_hint, t.index, t.index);
            out.push(Segment {
                start: t.index,
                end: t.index,
                turn_count: 1,
                branch: t.branch.clone(),
                cwd: t.cwd.clone(),
                started_at: t.timestamp.clone(),
                ended_at: t.timestamp.clone(),
                tokens: t.tokens,
                label,
                reason,
            });
        } else {
            let cur = out.last_mut().unwrap();
            cur.end = t.index;
            cur.turn_count += 1;
            cur.ended_at = t.timestamp.clone().or(cur.ended_at.take());
            cur.tokens += t.tokens;
        }
    }
    // Refine labels now that ranges are known.
    for seg in &mut out {
        let hint = None; // range-level label already set from the first turn
        seg.label = label_for(&seg.branch, &seg.cwd, &hint, seg.start, seg.end);
        if seg.label == format!("turns {}-{}", seg.start, seg.start) {
            seg.label = format!("turns {}-{}", seg.start, seg.end);
        }
    }
    out
}

/// Segment a raw `/sessions/<id>/turns` tree in one call.
pub fn segments_from_tree(turns: &Value, gap_min: i64) -> Vec<Segment> {
    segment_turns(&metas_from_turns_tree(turns), gap_min)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(index: usize, branch: &str, cwd: &str, ts: &str) -> TurnMeta {
        TurnMeta {
            index,
            cwd: Some(cwd.to_string()),
            branch: Some(branch.to_string()),
            timestamp: Some(ts.to_string()),
            label_hint: None,
            tokens: 10,
        }
    }

    #[test]
    fn splits_on_branch_change() {
        let turns = vec![
            t(0, "main", "/repo", "2026-08-01T10:00:00Z"),
            t(1, "main", "/repo", "2026-08-01T10:05:00Z"),
            t(2, "plan/auth", "/repo", "2026-08-01T10:10:00Z"),
            t(3, "plan/auth", "/repo", "2026-08-01T10:12:00Z"),
        ];
        let segs = segment_turns(&turns, 0);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].start, 0);
        assert_eq!(segs[0].end, 1);
        assert_eq!(segs[0].turn_count, 2);
        assert_eq!(segs[1].reason, "branch");
        assert_eq!(segs[1].label, "auth"); // plan/ stripped
        assert_eq!(segs[1].tokens, 20);
    }

    #[test]
    fn splits_on_idle_gap() {
        let turns = vec![
            t(0, "main", "/repo", "2026-08-01T10:00:00Z"),
            t(1, "main", "/repo", "2026-08-01T10:05:00Z"),
            // 90-minute gap -> new arc even on the same branch.
            t(2, "main", "/repo", "2026-08-01T11:35:00Z"),
        ];
        let segs = segment_turns(&turns, 30);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].reason, "gap");
    }

    #[test]
    fn gap_rule_disabled_when_zero() {
        let turns = vec![
            t(0, "main", "/repo", "2026-08-01T10:00:00Z"),
            t(1, "main", "/repo", "2026-08-01T14:00:00Z"),
        ];
        assert_eq!(segment_turns(&turns, 0).len(), 1);
    }

    #[test]
    fn splits_on_cwd_change() {
        let turns = vec![
            t(0, "main", "/repo-a", "2026-08-01T10:00:00Z"),
            t(1, "main", "/repo-b", "2026-08-01T10:01:00Z"),
        ];
        let segs = segment_turns(&turns, 0);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].reason, "cwd");
        assert_eq!(segs[1].cwd.as_deref(), Some("/repo-b"));
        // Branch is the same across the split, so the label falls to the branch
        // ("main") by precedence — the cwd is still on the segment for the UI.
        assert_eq!(segs[1].label, "main");
    }

    #[test]
    fn empty_input_no_segments() {
        assert!(segment_turns(&[], 30).is_empty());
    }

    #[test]
    fn extracts_metas_from_tree() {
        let tree = serde_json::json!({
            "t0000": {"turn_index": 0, "git_branches": ["main"], "cwd": "/r", "timestamp": "2026-08-01T10:00:00Z", "tokens": {"input": 5, "output": 3}, "user_text": "  \nfix the bug"},
            "t0001": {"turn_index": 1, "git_branches": ["plan/x"], "cwd": "/r", "timestamp": "2026-08-01T10:05:00Z", "tokens": {"input": 2}},
        });
        let metas = metas_from_turns_tree(&tree);
        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].branch.as_deref(), Some("main"));
        assert_eq!(metas[0].tokens, 8);
        assert_eq!(metas[0].label_hint.as_deref(), Some("fix the bug"));
        let segs = segments_from_tree(&tree, 0);
        assert_eq!(segs.len(), 2);
    }
}
