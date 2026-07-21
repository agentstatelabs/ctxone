//! Parse Gemini CLI session transcripts into source-neutral [`Turn`]s.
//!
//! Gemini stores one JSON file per session under
//! `~/.gemini/tmp/<project>/chats/session-*.json`, shaped:
//!
//! ```json
//! { "sessionId", "startTime", "lastUpdated", "projectHash",
//!   "messages": [ { "type": "user"|"gemini"|"info", "content", "timestamp",
//!                   "model"?, "tokens"?, "toolCalls"?, "thoughts"? } ] }
//! ```
//!
//! Turn reconstruction matches Codex: the file is a flat message stream, so a
//! turn is "a user message and every `gemini` message that followed it until
//! the next user message". `info` messages are session notices, not
//! conversation, and are dropped — the same reason `developer` messages are
//! dropped on the Codex path.

use crate::ingest::{Turn, TurnTokens};
use serde_json::Value;
use std::path::Path;

/// Token classes Gemini reports that the normalised four cannot express.
/// Preserved verbatim under Gemini's own names (see the Hub's `extra_tokens`)
/// rather than folded into `output`.
const EXTRA_TOKEN_FIELDS: &[&str] = &["thoughts", "tool"];

/// Parse one session file into turns. Empty on unreadable input — a bad
/// transcript must not abort a whole-machine scan.
pub fn parse_session(path: &Path) -> Vec<Turn> {
    let Ok(content) = std::fs::read_to_string(path) else {
        eprintln!("  warn: could not read {}", path.display());
        return vec![];
    };
    let Ok(root) = serde_json::from_str::<Value>(&content) else {
        return vec![];
    };
    let Some(messages) = root.get("messages").and_then(|m| m.as_array()) else {
        return vec![];
    };

    let mut turns: Vec<Turn> = vec![];
    let mut cur: Option<Turn> = None;

    for msg in messages {
        let kind = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let ts = msg
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        match kind {
            "user" => {
                if let Some(t) = cur.take() {
                    turns.push(t);
                }
                cur = Some(Turn {
                    user_text: extract_content(msg.get("content")),
                    assistant_text: String::new(),
                    tool_calls: vec![],
                    tool_calls_raw: vec![],
                    tokens: TurnTokens::default(),
                    model: String::new(),
                    timestamp: ts,
                    cwd: None, // set by the source from projects.json
                    git_branches: vec![],
                    git_commit: None,
                });
            }
            "gemini" => {
                let t = cur.get_or_insert_with(|| empty_turn(&ts));
                let text = extract_content(msg.get("content"));
                if !text.is_empty() {
                    if !t.assistant_text.is_empty() {
                        t.assistant_text.push_str("\n\n");
                    }
                    t.assistant_text.push_str(&text);
                }
                if let Some(m) = msg.get("model").and_then(|m| m.as_str()) {
                    if !m.is_empty() {
                        t.model = m.to_string();
                    }
                }
                if let Some(calls) = msg.get("toolCalls").and_then(|c| c.as_array()) {
                    for call in calls {
                        let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                        t.tool_calls.push(summarize_call(name, call.get("args")));
                        // Shape like a Claude tool_use block so the Lens's
                        // commit scan (reads input.command) works unchanged.
                        t.tool_calls_raw.push(serde_json::json!({
                            "name": name,
                            "input": call.get("args").cloned().unwrap_or(Value::Null),
                            "id": call.get("id").cloned().unwrap_or(Value::Null),
                            "source": "gemini",
                        }));
                    }
                }
                if let Some(tok) = msg.get("tokens") {
                    t.tokens.add(&tokens_from(tok));
                }
            }
            _ => {} // "info" and anything else: not conversation
        }
    }

    if let Some(t) = cur.take() {
        turns.push(t);
    }
    turns
}

fn empty_turn(ts: &str) -> Turn {
    Turn {
        user_text: String::new(),
        assistant_text: String::new(),
        tool_calls: vec![],
        tool_calls_raw: vec![],
        tokens: TurnTokens::default(),
        model: String::new(),
        timestamp: ts.to_string(),
        cwd: None,
        git_branches: vec![],
        git_commit: None,
    }
}

/// `content` is a string (gemini) or a list of `{text}` parts (user).
fn extract_content(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => {
            let mut out = String::new();
            for p in parts {
                if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// A short `name: arg` summary, matching the Claude/Codex tool_calls style.
fn summarize_call(name: &str, args: Option<&Value>) -> String {
    let arg = args.and_then(|a| {
        // Prefer a path/command-ish field, else the first string value.
        for k in ["command", "file_path", "path", "dir_path", "absolute_path"] {
            if let Some(s) = a.get(k).and_then(|v| v.as_str()) {
                return Some(s.to_string());
            }
        }
        a.as_object()
            .and_then(|o| o.values().find_map(|v| v.as_str()))
            .map(str::to_string)
    });
    match arg {
        Some(a) if !a.is_empty() => format!("{}: {}", name, a),
        _ => name.to_string(),
    }
}

/// Gemini tokens `{input, output, cached, thoughts, tool, total}` → the
/// normalised four plus `extra` for the classes they can't hold.
fn tokens_from(v: &Value) -> TurnTokens {
    let g = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let mut t = TurnTokens {
        input: g("input"),
        output: g("output"),
        cache_read: g("cached"),
        cache_creation: 0,
        ..Default::default()
    };
    for k in EXTRA_TOKEN_FIELDS {
        let n = g(k);
        if n > 0 {
            t.extra.insert((*k).to_string(), n);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_messages_into_turns() {
        let json = serde_json::json!({
            "sessionId": "s1",
            "messages": [
                { "type": "info", "content": "session start", "timestamp": "t0" },
                { "type": "user", "content": [{"text": "hello"}], "timestamp": "t1" },
                { "type": "gemini", "content": "hi", "model": "gemini-3-pro",
                  "timestamp": "t2",
                  "tokens": {"input": 100, "output": 5, "cached": 20, "thoughts": 8},
                  "toolCalls": [{"name": "list_directory", "args": {"dir_path": "/x"}, "id": "c1"}] },
                { "type": "user", "content": [{"text": "thanks"}], "timestamp": "t3" },
                { "type": "gemini", "content": "welcome", "model": "gemini-3-pro", "timestamp": "t4" }
            ]
        });
        let dir = std::env::temp_dir();
        let p = dir.join("ctxone-gemini-test.json");
        std::fs::write(&p, serde_json::to_string(&json).unwrap()).unwrap();
        let turns = parse_session(&p);
        std::fs::remove_file(&p).ok();

        assert_eq!(turns.len(), 2);
        // info dropped; first turn is user "hello" + gemini "hi".
        assert_eq!(turns[0].user_text, "hello");
        assert_eq!(turns[0].assistant_text, "hi");
        assert_eq!(turns[0].model, "gemini-3-pro");
        assert_eq!(turns[0].tool_calls, vec!["list_directory: /x"]);
        // Normalised tokens + extras preserved.
        assert_eq!(turns[0].tokens.input, 100);
        assert_eq!(turns[0].tokens.cache_read, 20);
        assert_eq!(turns[0].tokens.extra.get("thoughts"), Some(&8));
        assert_eq!(turns[1].user_text, "thanks");
        assert_eq!(turns[1].assistant_text, "welcome");
    }

    #[test]
    fn tolerates_missing_and_malformed() {
        assert!(parse_session(Path::new("/nonexistent/x.json")).is_empty());
    }
}
