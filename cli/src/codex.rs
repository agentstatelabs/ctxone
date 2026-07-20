//! Parse Codex rollout transcripts into source-neutral [`Turn`]s.
//!
//! A rollout is JSONL, one `{timestamp, type, payload}` per line. Turn
//! reconstruction differs from Claude Code in one important way: Codex does
//! not group a request and its response into a single record. The file is a
//! flat event stream, so a turn is "a user message and everything that
//! followed it until the next user message".
//!
//! Records we read:
//!
//! - `response_item` / `message` — `role` is `user`, `assistant`, or
//!   `developer`. **Developer messages are system prompts**, not user input;
//!   attributing them to the user would poison derived session titles the way
//!   it once did on the Claude path (see the transcript-attribution fix).
//!   `content` is a list of `{type, text}` where `input_text` is inbound and
//!   `output_text` is the model's reply.
//! - `response_item` / `function_call` — `{name, arguments, call_id}`.
//! - `event_msg` / `token_count` — usage. Carries `total_token_usage`
//!   (cumulative) and `last_token_usage` (this turn); we sum the latter so a
//!   truncated file still reports the turns it does contain.

use crate::ingest::{Turn, TurnTokens};
use serde_json::Value;
use std::path::Path;

/// Token classes Codex reports that the normalised four cannot express.
/// Preserved verbatim under Codex's own field name (see the Hub's
/// `extra_tokens`) rather than folded into `output`, which would make a
/// Codex session look artificially expensive next to a Claude one.
const EXTRA_TOKEN_FIELDS: &[&str] = &["reasoning_output_tokens"];

/// Parse one rollout file into turns. Returns empty on unreadable input — a
/// bad transcript must not abort a whole-machine scan.
pub fn parse_rollout(path: &Path) -> Vec<Turn> {
    let Ok(content) = std::fs::read_to_string(path) else {
        eprintln!("  warn: could not read {}", path.display());
        return vec![];
    };

    let mut turns: Vec<Turn> = vec![];
    let mut cur: Option<Turn> = None;
    let mut model = String::new();
    // Codex states provenance once in `session_meta` and restates it in
    // `turn_context` when it changes, so this carries forward to each turn.
    let mut prov = Prov::default();

    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue; // tolerate a torn final line
        };
        let ts = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
        let payload = v.get("payload").unwrap_or(&Value::Null);
        let outer = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let inner = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match (outer, inner) {
            ("session_meta", _) => {
                if let Some(m) = payload.get("model").and_then(|m| m.as_str()) {
                    model = m.to_string();
                }
                prov.absorb(payload);
            }
            ("turn_context", _) => {
                // Codex records the active model here when it changes.
                if let Some(m) = payload.get("model").and_then(|m| m.as_str()) {
                    model = m.to_string();
                }
                // ...and the working directory / git state, when those change.
                prov.absorb(payload);
            }
            ("response_item", "message") => {
                let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                let text = join_text(payload.get("content"));
                if text.is_empty() {
                    continue;
                }
                match role {
                    "user" => {
                        // A new user message starts a new turn.
                        if let Some(t) = cur.take() {
                            turns.push(t);
                        }
                        cur = Some(Turn {
                            user_text: text,
                            assistant_text: String::new(),
                            tool_calls: vec![],
                            tool_calls_raw: vec![],
                            tokens: TurnTokens::default(),
                            model: model.clone(),
                            timestamp: ts.to_string(),
                            cwd: prov.cwd.clone(),
                            git_branches: prov.branch.clone().into_iter().collect(),
                            git_commit: prov.commit.clone(),
                        });
                    }
                    "assistant" => {
                        let t = cur.get_or_insert_with(|| empty_turn(&model, ts, &prov));
                        if !t.assistant_text.is_empty() {
                            t.assistant_text.push_str("\n\n");
                        }
                        t.assistant_text.push_str(&text);
                        if t.model.is_empty() {
                            t.model = model.clone();
                        }
                    }
                    // "developer" is the system prompt — deliberately dropped.
                    _ => {}
                }
            }
            ("response_item", "function_call") => {
                let name = payload
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("tool");
                let t = cur.get_or_insert_with(|| empty_turn(&model, ts, &prov));
                t.tool_calls.push(name.to_string());
                // Keep the raw call so the real arguments survive, matching
                // what the Claude path stores. Shape it like a Claude
                // tool_use block so downstream consumers (Lens's Commits
                // section reads input.command) work without special-casing.
                t.tool_calls_raw.push(serde_json::json!({
                    "name": name,
                    "input": parse_arguments(payload.get("arguments")),
                    "id": payload.get("call_id").cloned().unwrap_or(Value::Null),
                    "source": "codex",
                }));
            }
            ("event_msg", "token_count") => {
                // `last_token_usage` is this turn's cost; summing it across
                // the file reconstructs the total without trusting a single
                // cumulative record that may be missing from a truncated file.
                if let Some(last) = payload.get("info").and_then(|i| i.get("last_token_usage")) {
                    let t = cur.get_or_insert_with(|| empty_turn(&model, ts, &prov));
                    t.tokens.add(&tokens_from(last));
                }
            }
            _ => {}
        }
    }

    if let Some(t) = cur.take() {
        turns.push(t);
    }
    turns
}

fn empty_turn(model: &str, ts: &str, prov: &Prov) -> Turn {
    Turn {
        user_text: String::new(),
        assistant_text: String::new(),
        tool_calls: vec![],
        tool_calls_raw: vec![],
        tokens: TurnTokens::default(),
        model: model.to_string(),
        timestamp: ts.to_string(),
        cwd: prov.cwd.clone(),
        git_branches: prov.branch.clone().into_iter().collect(),
        git_commit: prov.commit.clone(),
    }
}

/// Working directory and git state, as last stated by the rollout.
///
/// Codex reports these in `session_meta` and again in `turn_context` when
/// they change, so a turn records where it actually ran rather than
/// inheriting the session's opening state.
#[derive(Default, Clone)]
struct Prov {
    cwd: Option<String>,
    branch: Option<String>,
    commit: Option<String>,
}

impl Prov {
    /// Take whatever this payload states, leaving prior values in place for
    /// fields it does not mention.
    fn absorb(&mut self, payload: &Value) {
        if let Some(c) = non_empty(payload.get("cwd")) {
            self.cwd = Some(c);
        }
        let git = payload.get("git");
        if let Some(b) = git.and_then(|g| non_empty(g.get("branch"))) {
            self.branch = Some(b);
        }
        if let Some(h) = git.and_then(|g| non_empty(g.get("commit_hash"))) {
            self.commit = Some(h);
        }
    }
}

fn non_empty(v: Option<&Value>) -> Option<String> {
    v.and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Concatenate the `text` of every content block.
fn join_text(content: Option<&Value>) -> String {
    let Some(items) = content.and_then(|c| c.as_array()) else {
        // Some records carry a bare string rather than a block list.
        return content
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string();
    };
    items
        .iter()
        .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// `arguments` is a JSON *string*. Parse it so the real arguments are
/// queryable; fall back to keeping the raw string when it will not parse.
fn parse_arguments(args: Option<&Value>) -> Value {
    match args.and_then(|a| a.as_str()) {
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string())),
        None => args.cloned().unwrap_or(Value::Null),
    }
}

/// Map one Codex usage object onto [`TurnTokens`].
///
/// `cached_input_tokens` maps to `cache_read`: both mean "input served from
/// cache". Codex has no counterpart to Anthropic's cache *creation*, so
/// `cache_creation` stays zero rather than being invented.
fn tokens_from(u: &Value) -> TurnTokens {
    let get = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let mut t = TurnTokens {
        input: get("input_tokens"),
        output: get("output_tokens"),
        cache_read: get("cached_input_tokens"),
        cache_creation: 0,
        extra: Default::default(),
    };
    for f in EXTRA_TOKEN_FIELDS {
        let v = get(f);
        if v > 0 {
            t.extra.insert((*f).to_string(), v);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_rollout(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("tempfile");
        for l in lines {
            writeln!(f, "{}", l).expect("write");
        }
        f.flush().expect("flush");
        f
    }

    #[test]
    fn splits_turns_on_user_messages() {
        let f = write_rollout(&[
            r#"{"timestamp":"t0","type":"session_meta","payload":{"id":"s1","cwd":"/a/b","model":"gpt-5.2"}}"#,
            r#"{"timestamp":"t1","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]}}"#,
            r#"{"timestamp":"t2","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"reply one"}]}}"#,
            r#"{"timestamp":"t3","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"second"}]}}"#,
        ]);
        let turns = parse_rollout(f.path());
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].user_text, "first");
        assert_eq!(turns[0].assistant_text, "reply one");
        assert_eq!(turns[0].model, "gpt-5.2");
        assert_eq!(turns[1].user_text, "second");
    }

    #[test]
    fn developer_messages_are_not_user_input() {
        // Regression guard: attributing the system prompt to the user
        // poisons derived session titles.
        let f = write_rollout(&[
            r#"{"timestamp":"t0","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions instructions>"}]}}"#,
            r#"{"timestamp":"t1","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"real question"}]}}"#,
        ]);
        let turns = parse_rollout(f.path());
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].user_text, "real question");
    }

    #[test]
    fn reasoning_tokens_are_preserved_not_folded_into_output() {
        let f = write_rollout(&[
            r#"{"timestamp":"t1","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"q"}]}}"#,
            r#"{"timestamp":"t2","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":21461,"cached_input_tokens":14592,"output_tokens":258,"reasoning_output_tokens":171,"total_tokens":21719}}}}"#,
        ]);
        let turns = parse_rollout(f.path());
        assert_eq!(turns[0].tokens.input, 21461);
        assert_eq!(turns[0].tokens.output, 258, "reasoning must not inflate output");
        assert_eq!(turns[0].tokens.cache_read, 14592);
        assert_eq!(
            turns[0].tokens.extra.get("reasoning_output_tokens"),
            Some(&171)
        );
    }

    #[test]
    fn token_counts_sum_across_a_session() {
        let f = write_rollout(&[
            r#"{"timestamp":"t1","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"q"}]}}"#,
            r#"{"timestamp":"t2","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"output_tokens":10,"reasoning_output_tokens":1}}}}"#,
            r#"{"timestamp":"t3","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":50,"output_tokens":5,"reasoning_output_tokens":2}}}}"#,
        ]);
        let turns = parse_rollout(f.path());
        assert_eq!(turns[0].tokens.input, 150);
        assert_eq!(turns[0].tokens.output, 15);
        assert_eq!(turns[0].tokens.extra.get("reasoning_output_tokens"), Some(&3));
    }

    #[test]
    fn function_call_arguments_are_parsed_not_left_as_a_string() {
        let f = write_rollout(&[
            r#"{"timestamp":"t1","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"q"}]}}"#,
            r#"{"timestamp":"t2","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"git commit -m \\\"x\\\"\"}","call_id":"c1"}}"#,
        ]);
        let turns = parse_rollout(f.path());
        assert_eq!(turns[0].tool_calls, vec!["exec_command"]);
        let raw = &turns[0].tool_calls_raw[0];
        assert_eq!(raw["input"]["cmd"], "git commit -m \"x\"");
    }

    #[test]
    fn malformed_lines_do_not_abort_the_parse() {
        let f = write_rollout(&[
            r#"{"timestamp":"t1","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"q"}]}}"#,
            r#"{"timestamp":"t2","type":"resp"#, // torn final line
        ]);
        let turns = parse_rollout(f.path());
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].user_text, "q");
    }
}

#[cfg(test)]
mod real_data_probe {
    //! Ignored by default: parses this machine's real Codex rollouts. Run with
    //! `cargo test --bin ctx -- --ignored --nocapture codex_real`.
    use super::*;

    #[test]
    #[ignore]
    fn codex_real_rollouts_parse() {
        use crate::sources::{Codex, SessionSource};
        let src = Codex;
        if !src.is_available() {
            eprintln!("no ~/.codex on this machine; skipping");
            return;
        }
        let refs = src.discover_all();
        let mut turns = 0usize;
        let mut input = 0u64;
        let mut output = 0u64;
        let mut reasoning = 0u64;
        let mut empty_files = 0usize;
        for r in &refs {
            let ts = src.parse(r);
            if ts.is_empty() {
                empty_files += 1;
            }
            for t in &ts {
                turns += 1;
                input += t.tokens.input;
                output += t.tokens.output;
                reasoning += t.tokens.extra.get("reasoning_output_tokens").copied().unwrap_or(0);
            }
        }
        eprintln!(
            "sessions={} turns={} input={} output={} reasoning={} files_with_no_turns={}",
            refs.len(), turns, input, output, reasoning, empty_files
        );
        // Only assert when there is actually data — ~/.codex can exist with
        // no rollouts, and this probe must not fail for that reason.
        if !refs.is_empty() {
            assert!(turns > 0, "discovered rollouts but parsed no turns");
        }
    }
}
