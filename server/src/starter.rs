//! Session STARTER generation from the user's own words (t-002).
//!
//! When a session gets long enough that it's costing more context than it's
//! landing (see `web/src/lib/sessionBurn.ts`), the right move is to start a
//! fresh session — but only if the new one comes up to speed instantly. The
//! honest seed for that is **what the user actually said**, verbatim and in
//! order, NOT an agent's summary of what it thinks happened. The agent's
//! surmise is exactly the thing we don't want to carry forward; the user's
//! stated goals, decisions and constraints are.
//!
//! So this module reads a session's ingested turns, pulls each user utterance
//! (`user_text`) verbatim (trimmed and de-noised), groups them by topic arc
//! (reusing [`crate::segments`]), and renders a paste-ready markdown starter.
//! Pure and no-LLM, like `segments.rs` — the HTTP layer just feeds it the
//! turns tree.

use serde::Serialize;
use serde_json::Value;

use crate::segments::{metas_from_turns_tree, segment_turns};

/// Longest a single user utterance is kept in the starter (chars). Enough for
/// an ask plus a constraint or two; the point is a seed, not a transcript.
const MAX_SAY_CHARS: usize = 280;

/// At most this many non-empty lines are kept from one user message.
const MAX_SAY_LINES: usize = 3;

/// Hard cap on user utterances in the whole starter, so a 400-turn session
/// still yields a readable seed rather than a wall.
const MAX_SAYS: usize = 40;

/// Idle-gap (minutes) for arc segmentation inside the starter. Matches the
/// default used elsewhere; a new arc after a long break reads naturally.
const ARC_GAP_MIN: i64 = 90;

/// The generated starter and enough provenance to prove where it came from.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StarterResult {
    /// Paste-ready markdown: the user's own asks, grouped by arc.
    pub markdown: String,
    /// Distinct topic arcs the session spanned.
    pub arc_count: usize,
    /// How many user utterances contributed (after de-noising).
    pub user_turn_count: usize,
    /// Turn indices that contributed, in order — provenance, not content.
    pub source_turns: Vec<usize>,
}

/// One user utterance pulled verbatim from a turn.
struct UserSay {
    index: usize,
    text: String,
}

/// True for lines that are transport/wrapper noise rather than the user's
/// words: harness reminders, command wrappers, tool-output fences.
fn is_noise_line(l: &str) -> bool {
    let l = l.trim_start();
    l.starts_with("<system-reminder")
        || l.starts_with("</system-reminder")
        || l.starts_with("<command-")
        || l.starts_with("</command-")
        || l.starts_with("<local-command")
        || l.starts_with("Caveat:")
        || l.starts_with("```")
}

/// Distill one raw `user_text` blob into a trimmed utterance, or `None` when
/// nothing survives de-noising (tool-only / system-only turns).
fn distill(user_text: &str) -> Option<String> {
    let mut kept: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in user_text.lines() {
        let t = line.trim();
        // Skip the entire contents of a ``` fenced block (pasted tool output /
        // code), not just the fence markers — it isn't the user's directive.
        if t.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || t.is_empty() || is_noise_line(t) {
            continue;
        }
        kept.push(t);
        if kept.len() >= MAX_SAY_LINES {
            break;
        }
    }
    if kept.is_empty() {
        return None;
    }
    let mut text = kept.join(" ");
    if text.chars().count() > MAX_SAY_CHARS {
        // Cut on a char boundary and mark the truncation.
        let truncated: String = text.chars().take(MAX_SAY_CHARS).collect();
        text = format!("{}…", truncated.trim_end());
    }
    Some(text)
}

/// Extract user utterances from a `/sessions/<id>/turns` tree, in turn order.
fn user_says(turns: &Value) -> Vec<UserSay> {
    let Some(obj) = turns.as_object() else {
        return Vec::new();
    };
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort(); // t0000 < t0001 lexically
    let mut out = Vec::new();
    for (i, k) in keys.into_iter().enumerate() {
        let t = &obj[k];
        let index = t
            .get("turn_index")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(i);
        let Some(raw) = t.get("user_text").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(text) = distill(raw) {
            out.push(UserSay { index, text });
        }
    }
    out
}

/// Which arc (by 0-based position) an absolute turn index falls in.
fn arc_of(index: usize, arcs: &[(usize, usize, String)]) -> Option<usize> {
    arcs.iter()
        .position(|(start, end, _)| index >= *start && index <= *end)
}

/// Build a session starter from a raw `/sessions/<id>/turns` tree and an
/// optional session title. Pure; safe on empty/degenerate input.
pub fn build_starter(turns: &Value, title: Option<&str>) -> StarterResult {
    let says = user_says(turns);
    let source_turns: Vec<usize> = says.iter().map(|s| s.index).collect();

    // Arc ranges (start, end, label) for grouping.
    let segs = segment_turns(&metas_from_turns_tree(turns), ARC_GAP_MIN);
    let arcs: Vec<(usize, usize, String)> = segs
        .iter()
        .map(|s| (s.start, s.end, s.label.clone()))
        .collect();

    let mut md = String::new();
    md.push_str("# Session starter — distilled from what you said\n\n");
    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
        md.push_str(&format!("_Continuing: {t}_\n\n"));
    }
    md.push_str(
        "_Seeded from your own messages, in order — not an agent summary. \
         Trim anything that's already done, then send._\n\n",
    );

    if says.is_empty() {
        md.push_str("_(No user messages were found in this session's ingested turns.)_\n");
        return StarterResult {
            markdown: md,
            arc_count: arcs.len(),
            user_turn_count: 0,
            source_turns,
        };
    }

    // Group utterances under their arc heading, preserving order. Utterances
    // that fall outside any arc (shouldn't happen, but be safe) go last.
    let capped = says.iter().take(MAX_SAYS);
    let mut current_arc: Option<usize> = None;
    let mut wrote_any_heading = false;
    for say in capped {
        let arc_idx = arc_of(say.index, &arcs);
        if arc_idx != current_arc {
            current_arc = arc_idx;
            let label = arc_idx
                .and_then(|i| arcs.get(i))
                .map(|(_, _, l)| l.as_str())
                .filter(|l| !l.is_empty());
            if arcs.len() > 1 || label.is_some() {
                md.push_str(&format!("\n## {}\n", label.unwrap_or("Session")));
                wrote_any_heading = true;
            }
        }
        md.push_str(&format!("- {}\n", say.text));
    }
    let _ = wrote_any_heading;

    let kept = says.len().min(MAX_SAYS);
    if says.len() > MAX_SAYS {
        md.push_str(&format!(
            "\n_({} more user messages omitted — this is a seed, not the full log.)_\n",
            says.len() - MAX_SAYS
        ));
    }

    StarterResult {
        markdown: md,
        arc_count: arcs.len(),
        user_turn_count: kept,
        source_turns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn turn(idx: usize, branch: &str, ts: &str, user_text: &str) -> (String, Value) {
        (
            format!("t{idx:04}"),
            json!({
                "turn_index": idx,
                "git_branches": [branch],
                "cwd": "/repo",
                "timestamp": ts,
                "user_text": user_text,
            }),
        )
    }

    fn tree(turns: Vec<(String, Value)>) -> Value {
        Value::Object(turns.into_iter().collect())
    }

    #[test]
    fn pulls_user_words_verbatim_in_order() {
        let t = tree(vec![
            turn(
                0,
                "main",
                "2026-08-01T10:00:00Z",
                "Add a rate limiter to the API",
            ),
            turn(
                1,
                "main",
                "2026-08-01T10:05:00Z",
                "Use a token bucket, not a fixed window",
            ),
        ]);
        let s = build_starter(&t, Some("API work"));
        assert!(s.markdown.contains("Add a rate limiter to the API"));
        assert!(
            s.markdown
                .contains("Use a token bucket, not a fixed window")
        );
        // The user's words come out in order.
        let a = s.markdown.find("rate limiter").unwrap();
        let b = s.markdown.find("token bucket").unwrap();
        assert!(a < b);
        assert_eq!(s.user_turn_count, 2);
        assert_eq!(s.source_turns, vec![0, 1]);
        assert!(s.markdown.contains("not an agent summary"));
    }

    #[test]
    fn drops_noise_and_tool_only_turns() {
        let t = tree(vec![
            turn(
                0,
                "main",
                "2026-08-01T10:00:00Z",
                "<system-reminder>ignore me</system-reminder>\nKeep this line",
            ),
            turn(
                1,
                "main",
                "2026-08-01T10:05:00Z",
                "<command-name>/foo</command-name>\n```\ntool output\n```",
            ),
        ]);
        let s = build_starter(&t, None);
        assert!(s.markdown.contains("Keep this line"));
        assert!(!s.markdown.contains("ignore me"));
        // Turn 1 was entirely noise -> no utterance kept.
        assert_eq!(s.user_turn_count, 1);
        assert_eq!(s.source_turns, vec![0]);
    }

    #[test]
    fn groups_by_arc_on_branch_change() {
        let t = tree(vec![
            turn(0, "main", "2026-08-01T10:00:00Z", "Fix the login bug"),
            turn(1, "plan/auth", "2026-08-01T10:05:00Z", "Now add OAuth"),
        ]);
        let s = build_starter(&t, None);
        assert_eq!(s.arc_count, 2);
        assert!(s.markdown.contains("## auth")); // plan/ stripped label
    }

    #[test]
    fn long_message_is_capped() {
        let long = "x".repeat(1000);
        let t = tree(vec![turn(0, "main", "2026-08-01T10:00:00Z", &long)]);
        let s = build_starter(&t, None);
        assert!(s.markdown.contains('…'));
        // The full 1000-char blob is not carried through.
        assert!(!s.markdown.contains(&"x".repeat(400)));
    }

    #[test]
    fn empty_session_is_safe() {
        let s = build_starter(&json!({}), None);
        assert_eq!(s.user_turn_count, 0);
        assert!(s.markdown.contains("No user messages"));
    }
}
