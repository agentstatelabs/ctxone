//! Session ingestion: parse Claude Code JSONL transcripts, extract structured
//! memories via Haiku, and store token usage + facts in the Hub.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Truncate `s` to at most `max_bytes` bytes on a UTF-8 char boundary.
///
/// A naive `&s[..max_bytes]` panics when `max_bytes` lands inside a
/// multibyte char (e.g. an em-dash spanning bytes 118..121 when the cut is
/// at 120). We back the cut up to the nearest boundary instead. Returns the
/// original string when it already fits.
fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ── JSONL structures ──────────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize)]
pub struct TurnTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    /// Token classes these four fields cannot express, under the reporting
    /// agent's own names — Codex `reasoning_output_tokens`, Gemini
    /// `thoughts`/`tool`. Carried verbatim to the Hub's `extra_tokens` so a
    /// class we cannot yet display is still never lost.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, u64>,
}

impl TurnTokens {
    pub fn is_empty(&self) -> bool {
        self.input == 0 && self.output == 0 && self.extra.is_empty()
    }

    pub fn add(&mut self, other: &TurnTokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_creation += other.cache_creation;
        for (k, v) in &other.extra {
            *self.extra.entry(k.clone()).or_insert(0) += *v;
        }
    }
}

#[derive(Debug)]
pub struct Turn {
    pub user_text: String,
    pub assistant_text: String,
    pub tool_calls: Vec<String>,
    /// Full tool_use blocks as they appeared in the JSONL — preserves the
    /// real arguments (Bash command, full Edit diff, etc.) that the
    /// summarized `tool_calls` list throws away.
    pub tool_calls_raw: Vec<Value>,
    pub tokens: TurnTokens,
    pub model: String,
    pub timestamp: String,
}

impl Turn {
    /// True if this turn has enough substance to extract memories from.
    pub fn is_substantial(&self) -> bool {
        self.assistant_text.len() > 100 || !self.tool_calls.is_empty()
    }

    /// Render turn as text for the extraction LLM.
    pub fn to_exchange_text(&self) -> String {
        let mut out = String::new();
        out.push_str("USER:\n");
        out.push_str(&self.user_text);
        out.push_str("\n\nASSISTANT:\n");
        // Truncate very long assistant text to keep Haiku call cheap.
        let text = if self.assistant_text.len() > 8000 {
            format!(
                "{}…[truncated]",
                truncate_on_char_boundary(&self.assistant_text, 8000)
            )
        } else {
            self.assistant_text.clone()
        };
        out.push_str(&text);
        if !self.tool_calls.is_empty() {
            out.push_str("\n\nTOOLS USED:\n");
            for t in &self.tool_calls {
                out.push_str("- ");
                out.push_str(t);
                out.push('\n');
            }
        }
        out
    }
}

/// Parse a Claude Code JSONL file into a list of turns.
pub fn parse_turns(path: &Path) -> Vec<Turn> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  warn: could not read {}: {}", path.display(), e);
            return vec![];
        }
    };

    let entries: Vec<Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let mut turns: Vec<Turn> = vec![];
    let mut current_user: Option<String> = None;
    let mut current_ts: String = String::new();

    for entry in &entries {
        let typ = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match typ {
            "user" => {
                // Start a new turn. Strip harness-injected system content
                // (task notifications / reminders / local-command output) so
                // it isn't attributed to the user. A message that had text but
                // was fully synthetic still opens a turn (with empty user
                // text) — the agent's reply then renders as agent-only.
                // A message with NO text at all (a bare tool_result) is part
                // of the current turn, so we leave `current_user` untouched.
                let raw = extract_text_content(entry.get("message"));
                if !raw.trim().is_empty() {
                    current_user = Some(strip_synthetic_blocks(&raw));
                    current_ts = entry
                        .get("timestamp")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                }
            }
            "assistant" => {
                let Some(ref user_text) = current_user else {
                    continue;
                };
                let msg = match entry.get("message") {
                    Some(m) => m,
                    None => continue,
                };

                let assistant_text = extract_text_content(Some(msg));
                let tool_calls = extract_tool_calls(msg);
                let tool_calls_raw = extract_tool_calls_raw(msg);
                let tokens = extract_tokens(msg);
                let model = msg
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                // Merge with previous turn if same user message (split responses).
                if let Some(last) = turns.last_mut() {
                    if last.user_text == *user_text {
                        last.assistant_text.push('\n');
                        last.assistant_text.push_str(&assistant_text);
                        last.tool_calls.extend(tool_calls);
                        last.tool_calls_raw.extend(tool_calls_raw);
                        last.tokens.add(&tokens);
                        if !model.is_empty() && model != "unknown" {
                            last.model = model;
                        }
                        continue;
                    }
                }

                turns.push(Turn {
                    user_text: user_text.clone(),
                    assistant_text,
                    tool_calls,
                    tool_calls_raw,
                    tokens,
                    model,
                    timestamp: current_ts.clone(),
                });
            }
            _ => {}
        }
    }

    turns
}

/// Harness-injected tags that arrive inside a "user" turn but are NOT
/// human input: background-task notifications, system reminders, and the
/// output of local slash-commands. Left in place they mis-attribute
/// system/agent content to the USER in the transcript.
const SYNTHETIC_TAGS: &[&str] = &[
    "system-reminder",
    "task-notification",
    "local-command-caveat",
    "local-command-stdout",
    "local-command-message",
    "local-command-name",
    "local-command-args",
];

/// Remove synthetic `<tag>…</tag>` blocks from a user message, returning
/// the genuine human remainder. A fully-synthetic message reduces to an
/// empty string — the caller still opens a turn for it, so the agent's
/// reply renders as agent-only (no false USER bubble).
fn strip_synthetic_blocks(text: &str) -> String {
    let mut s = text.to_string();
    for tag in SYNTHETIC_TAGS {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        loop {
            let Some(start) = s.find(&open) else { break };
            match s[start + open.len()..].find(&close) {
                Some(rel) => {
                    let end = start + open.len() + rel + close.len();
                    s.replace_range(start..end, "");
                }
                // Unclosed tag — drop from the marker to the end, defensively.
                None => {
                    s.truncate(start);
                    break;
                }
            }
        }
    }
    // Some notifications carry a bare "[SYSTEM NOTIFICATION - NOT USER
    // INPUT]" preamble ahead of the tag; strip a leading one if it's left.
    let trimmed = s.trim_start();
    if trimmed.starts_with("[SYSTEM NOTIFICATION") {
        if let Some(nl) = trimmed.find("\n\n") {
            return trimmed[nl..].trim().to_string();
        }
    }
    s.trim().to_string()
}

fn extract_text_content(msg: Option<&Value>) -> String {
    let msg = match msg {
        Some(m) => m,
        None => return String::new(),
    };
    // content can be a string or array of blocks.
    match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => {
            let mut out = String::new();
            for block in blocks {
                let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match btype {
                    "text" => {
                        if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                            if !out.is_empty() {
                                out.push('\n');
                            }
                            out.push_str(t);
                        }
                    }
                    // Skip "thinking" blocks — they're internal reasoning.
                    // Skip "tool_use" blocks — handled separately.
                    _ => {}
                }
            }
            out
        }
        _ => String::new(),
    }
}

fn extract_tool_calls(msg: &Value) -> Vec<String> {
    let blocks = match msg.get("content").and_then(|v| v.as_array()) {
        Some(b) => b,
        None => return vec![],
    };
    let mut calls = vec![];
    for block in blocks {
        if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
            continue;
        }
        let name = block
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");
        let input = block.get("input").cloned().unwrap_or(Value::Null);
        let summary = match name {
            "Bash" => input
                .get("command")
                .and_then(|v| v.as_str())
                .map(|c| {
                    let c = c.trim();
                    if c.len() > 120 {
                        format!("{}…", truncate_on_char_boundary(c, 120))
                    } else {
                        c.to_string()
                    }
                })
                .unwrap_or_default(),
            "Write" | "Read" | "Edit" | "Glob" | "Grep" => input
                .get("file_path")
                .or_else(|| input.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            "Agent" => input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("sub-agent")
                .to_string(),
            _ => String::new(),
        };
        if summary.is_empty() {
            calls.push(name.to_string());
        } else {
            calls.push(format!("{}: {}", name, summary));
        }
    }
    calls
}

/// Like `extract_tool_calls` but returns the raw `{name, input}` pairs so
/// we can persist the full call (e.g., the actual Bash command, the full
/// Edit diff) rather than just the human-readable summary.
fn extract_tool_calls_raw(msg: &Value) -> Vec<Value> {
    let blocks = match msg.get("content").and_then(|v| v.as_array()) {
        Some(b) => b,
        None => return vec![],
    };
    let mut out = vec![];
    for block in blocks {
        if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
            continue;
        }
        let name = block.get("name").cloned().unwrap_or(Value::Null);
        let input = block.get("input").cloned().unwrap_or(Value::Null);
        let id = block.get("id").cloned().unwrap_or(Value::Null);
        out.push(serde_json::json!({ "id": id, "name": name, "input": input }));
    }
    out
}

fn extract_tokens(msg: &Value) -> TurnTokens {
    let usage = match msg.get("usage") {
        Some(u) => u,
        None => return TurnTokens::default(),
    };
    TurnTokens {
        input: usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output: usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_read: usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_creation: usage
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        // Claude's usage has no classes beyond the four; extras stay empty.
        extra: BTreeMap::new(),
    }
}

// ── Extraction via Haiku ──────────────────────────────────────────────────────

const EXTRACTION_SYSTEM: &str = r#"You extract structured facts from AI coding conversations.
Given a conversation exchange, return a JSON array of memory objects.

Each object:
{
  "path": "/category/subcategory/slug",
  "title": "Short descriptive title (under 80 chars)",
  "body": "The fact plus reasoning. Include why if a decision was made (1-3 sentences).",
  "importance": "high|medium|low",
  "context": "topic-tag"
}

Path categories:
  /decisions/   — architectural or design choices and their rationale
  /problems/    — issues encountered and how they were solved
  /preferences/ — user preferences revealed through corrections or feedback
  /conventions/ — coding, naming, or process conventions established
  /constraints/ — technical or business limitations discovered
  /progress/    — significant milestones or completions
  /next-steps/  — what was explicitly deferred or planned next

Rules:
- Include options that were REJECTED and why — this is as valuable as what was chosen
- Capture user corrections ("no, do X instead") as /preferences/
- Only extract facts that will be useful to a future agent on this project
- Skip: routine file reads, search results, compilation output unless something important was revealed
- Return ONLY a valid JSON array. No markdown fences, no explanation."#;

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<AnthropicMessage>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Deserialize)]
struct AnthropicContent {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct ExtractedMemory {
    pub path: String,
    pub title: String,
    pub body: String,
    pub importance: String,
    pub context: String,
}

pub async fn extract_memories(
    turn: &Turn,
    api_key: &str,
    client: &reqwest::Client,
) -> Vec<ExtractedMemory> {
    let exchange = turn.to_exchange_text();
    let req = AnthropicRequest {
        model: "claude-haiku-4-5-20251001".to_string(),
        max_tokens: 4096,
        system: EXTRACTION_SYSTEM.to_string(),
        messages: vec![AnthropicMessage {
            role: "user".to_string(),
            content: exchange,
        }],
    };

    let resp = match client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&req)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  warn: Haiku API call failed: {}", e);
            return vec![];
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        eprintln!(
            "  warn: Haiku returned {}: {}",
            status,
            truncate_on_char_boundary(&body, 200)
        );
        return vec![];
    }

    let ar: AnthropicResponse = match resp.json().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  warn: failed to parse Haiku response: {}", e);
            return vec![];
        }
    };

    let raw = ar
        .content
        .iter()
        .filter(|c| c.content_type == "text")
        .filter_map(|c| c.text.as_deref())
        .collect::<Vec<_>>()
        .join("");

    // Strip markdown fences if model added them despite instructions.
    let json_str = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    match serde_json::from_str::<Vec<ExtractedMemory>>(json_str) {
        Ok(memories) => memories,
        Err(e) => {
            eprintln!("  warn: could not parse extracted memories: {}", e);
            eprintln!("  raw: {}", truncate_on_char_boundary(json_str, 300));
            vec![]
        }
    }
}

// ── Hub writes ────────────────────────────────────────────────────────────────

/// Best-effort provider for a model name.
///
/// The provider used to be hardcoded to "anthropic", which was true while
/// Claude Code was the only source and silently wrong the moment a Codex or
/// Gemini turn came through the same path. Derived from the model rather than
/// threaded from the source so turns keep the right provider even when they
/// are replayed outside their originating adapter.
pub fn provider_for_model(model: &str) -> &'static str {
    let m = model.to_ascii_lowercase();
    if m.starts_with("claude") {
        "anthropic"
    } else if m.starts_with("gpt") || m.starts_with("o1") || m.starts_with("o3") {
        "openai"
    } else if m.starts_with("gemini") {
        "google"
    } else {
        "unknown"
    }
}

pub async fn record_turn_tokens(
    tokens: &TurnTokens,
    model: &str,
    provider: &str,
    hub: &str,
    session: Option<&str>,
    client: &reqwest::Client,
) {
    if tokens.is_empty() {
        return;
    }
    let body = serde_json::json!({
        "input_tokens": tokens.input,
        "output_tokens": tokens.output,
        "cache_read_tokens": tokens.cache_read,
        "cache_creation_tokens": tokens.cache_creation,
        "extra_tokens": tokens.extra,
        "model": model,
        "provider": provider,
    });
    let mut req = client
        .post(format!("{}/api/stats/llm_usage", hub))
        .json(&body);
    if let Some(sid) = session {
        req = req.header("X-CTXone-Session", sid);
    }
    let _ = req.send().await;
}

/// Persist the full turn (request, assistant response, tool calls with
/// real arguments, token usage, model, timestamp) as a memory at a
/// deterministic per-session path. This complements the Haiku-extracted
/// memories: extraction loses fidelity, the raw turn keeps everything.
///
/// Path: `/sessions/{session}/turns/{idx:04}` so turns sort lexically.
/// `idx` is the per-session turn number (0-based).
pub async fn store_full_turn(
    turn: &Turn,
    idx: usize,
    source_file: &str,
    hub: &str,
    branch: &str,
    session: Option<&str>,
    client: &reqwest::Client,
) {
    let sid = session.unwrap_or("default");
    let snapshot = serde_json::json!({
        "session": sid,
        "turn_index": idx,
        "timestamp": turn.timestamp,
        "model": turn.model,
        "source_file": source_file,
        "user_text": turn.user_text,
        "assistant_text": turn.assistant_text,
        "tool_calls": turn.tool_calls,
        "tool_calls_raw": turn.tool_calls_raw,
        "tokens": turn.tokens,
    });
    let url = format!(
        "{}/api/sessions/{}/turns/{}?ref={}",
        hub,
        crate::urlencoding(sid),
        idx,
        crate::urlencoding(branch),
    );
    let mut req = client.post(url).json(&snapshot);
    if let Some(s) = session {
        req = req.header("X-CTXone-Session", s);
    }
    let _ = req.send().await;
}

/// Max length of a derived session title (chars), before an ellipsis.
const TITLE_MAX_CHARS: usize = 70;

/// True when a user message is substantive enough to title a session by.
///
/// Skips the noise Claude Code injects as `user`-role entries: slash commands
/// (`/clear`, `/init`), XML-ish meta wrappers (`<command-name>`,
/// `<local-command-stdout>`, `<system-reminder>`), and the caveat preamble.
/// Tool-result echoes are already dropped upstream — `parse_turns` only keeps
/// user text extracted from `text` blocks, so a tool_result turn arrives here
/// as empty and is filtered by the `is_empty` guard.
fn is_substantive_user_text(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    // Slash-command invocations (the whole message is the command).
    if t.starts_with('/') {
        return false;
    }
    // Meta/wrapper blocks Claude Code emits as user turns.
    if t.starts_with('<') {
        return false;
    }
    if t.starts_with("Caveat:") {
        return false;
    }
    // Require at least one alphanumeric char so pure punctuation is skipped.
    t.chars().any(|c| c.is_alphanumeric())
}

/// Derive a session title from parsed turns: the first substantive user
/// message, truncated to [`TITLE_MAX_CHARS`]. Returns `None` when no turn
/// qualifies (caller supplies the `<project> · <date>` fallback).
pub fn derive_session_title(turns: &[Turn]) -> Option<String> {
    let raw = turns
        .iter()
        .map(|t| t.user_text.as_str())
        .find(|txt| is_substantive_user_text(txt))?;
    // Collapse internal whitespace/newlines into single spaces for a clean
    // one-line title.
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(truncate_title(&collapsed))
}

/// Truncate to [`TITLE_MAX_CHARS`] on a char boundary, appending an ellipsis
/// when the text was cut.
pub fn truncate_title(s: &str) -> String {
    if s.chars().count() <= TITLE_MAX_CHARS {
        return s.to_string();
    }
    let truncated: String = s.chars().take(TITLE_MAX_CHARS).collect();
    format!("{}…", truncated.trim_end())
}

/// Persist a session's human-readable title at `/sessions/{session}/title`
/// via the Hub. Idempotent — re-ingesting overwrites. No-op on empty title.
pub async fn store_session_title(
    title: &str,
    hub: &str,
    branch: &str,
    session: Option<&str>,
    client: &reqwest::Client,
) {
    if title.trim().is_empty() {
        return;
    }
    let sid = session.unwrap_or("default");
    let url = format!(
        "{}/api/sessions/{}/title?ref={}",
        hub,
        crate::urlencoding(sid),
        crate::urlencoding(branch),
    );
    let mut req = client.put(url).json(&serde_json::json!(title));
    if let Some(s) = session {
        req = req.header("X-CTXone-Session", s);
    }
    let _ = req.send().await;
}

/// Persist a session's meta object `{source, started_at, updated_at}` at
/// `/sessions/{session}/meta` via the Hub. Idempotent; drives the Lens
/// agent-type filter and date sort. No-op when all fields are empty.
pub async fn store_session_meta(
    source: &str,
    started_at: &str,
    updated_at: &str,
    models_used: &[String],
    hub: &str,
    branch: &str,
    session: Option<&str>,
    client: &reqwest::Client,
) {
    if source.is_empty() && started_at.is_empty() && updated_at.is_empty() && models_used.is_empty()
    {
        return;
    }
    let sid = session.unwrap_or("default");
    let url = format!(
        "{}/api/sessions/{}/meta?ref={}",
        hub,
        crate::urlencoding(sid),
        crate::urlencoding(branch),
    );
    let mut meta = serde_json::Map::new();
    if !source.is_empty() {
        meta.insert("source".into(), serde_json::json!(source));
    }
    if !started_at.is_empty() {
        meta.insert("started_at".into(), serde_json::json!(started_at));
    }
    if !updated_at.is_empty() {
        meta.insert("updated_at".into(), serde_json::json!(updated_at));
    }
    if !models_used.is_empty() {
        meta.insert("models_used".into(), serde_json::json!(models_used));
    }
    let mut req = client.put(url).json(&serde_json::Value::Object(meta));
    if let Some(s) = session {
        req = req.header("X-CTXone-Session", s);
    }
    let _ = req.send().await;
}

pub async fn store_memory(
    mem: &ExtractedMemory,
    hub: &str,
    branch: &str,
    session: Option<&str>,
    client: &reqwest::Client,
) {
    let body = serde_json::json!({
        "fact": mem.body,
        "path": mem.path,
        "importance": mem.importance,
        "context": mem.context,
        "ref": branch,
        "source": "ingest",
    });
    let mut req = client
        .post(format!("{}/api/memory/remember", hub))
        .json(&body);
    if let Some(sid) = session {
        req = req.header("X-CTXone-Session", sid);
    }
    let _ = req.send().await;
}

// ── File discovery ────────────────────────────────────────────────────────────

/// Find Claude Code session JSONL files for a given project directory.
///
/// Thin wrapper over the Claude Code [`SessionSource`](crate::sources::SessionSource);
/// kept so existing call sites keep working while sources are added.
pub fn find_session_files(project_dir: &Path) -> Vec<PathBuf> {
    use crate::sources::SessionSource;
    crate::sources::ClaudeCode
        .discover_for_project(project_dir)
        .into_iter()
        .map(|r| r.path)
        .collect()
}

/// Find the single most-recent session file (for capture-turn).
pub fn latest_session_file(project_dir: &Path) -> Option<PathBuf> {
    find_session_files(project_dir).into_iter().last()
}

/// Scan EVERY project under `~/.claude/projects/*` for Claude Code session
/// JSONL files and return them grouped by project as `(label, files)` pairs.
///
/// Each subdirectory of `~/.claude/projects/` is a hashed project path
/// (Claude Code replaces `/` with `-`). We derive a short human-readable
/// `label` from the last two path components of the recovered path so
/// `ctx ingest-session --all` can report per-project counts. Files within
/// each project are sorted oldest-first (by mtime); projects are sorted by
/// label for stable output. Projects with no `.jsonl` files are omitted.
///
/// This is the `--all` counterpart to [`find_session_files`], which scans
/// only the single project matching a given directory.
pub fn find_all_session_files() -> Vec<(String, Vec<PathBuf>)> {
    use crate::sources::SessionSource;
    crate::sources::group_by_label(crate::sources::ClaudeCode.discover_all())
}

/// Return the last N turns from a session file (for per-turn hook capture).
pub fn last_turns(path: &Path, n: usize) -> Vec<Turn> {
    let all = parse_turns(path);
    let skip = all.len().saturating_sub(n);
    all.into_iter().skip(skip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_synthetic_removes_task_notification() {
        let raw = "<task-notification>\n<task-id>abc</task-id>\n<summary>Agent finished</summary>\n</task-notification>";
        assert_eq!(strip_synthetic_blocks(raw), "");
    }

    #[test]
    fn strip_synthetic_keeps_genuine_and_drops_reminder() {
        let raw = "Do the thing.\n<system-reminder>internal context</system-reminder>";
        assert_eq!(strip_synthetic_blocks(raw), "Do the thing.");
    }

    #[test]
    fn strip_synthetic_removes_local_command_blocks() {
        let raw = "<local-command-caveat>Caveat: …</local-command-caveat>\n<local-command-stdout>Set model to x</local-command-stdout>";
        assert_eq!(strip_synthetic_blocks(raw), "");
    }

    #[test]
    fn strip_synthetic_leaves_plain_user_text_untouched() {
        let raw = "Just a normal message with < and > but no synthetic tags.";
        assert_eq!(strip_synthetic_blocks(raw), raw);
    }

    #[test]
    fn truncate_shorter_than_limit_is_unchanged() {
        assert_eq!(truncate_on_char_boundary("hello", 120), "hello");
    }

    #[test]
    fn truncate_backs_up_off_a_multibyte_boundary() {
        // Em-dash '—' is 3 bytes. Build a string whose byte 120 lands INSIDE
        // it — the exact case that panicked `&c[..120]` on real transcripts.
        let s = format!("{}—tail", "x".repeat(118)); // '—' occupies bytes 118..121
        assert!(!s.is_char_boundary(120));
        let out = truncate_on_char_boundary(&s, 120);
        // Cut backed up to byte 118 (start of the em-dash), never panicking.
        assert_eq!(out.len(), 118);
        assert_eq!(out, "x".repeat(118));
    }

    #[test]
    fn truncate_on_exact_boundary_keeps_full_bytes() {
        let s = "abcdef";
        assert_eq!(truncate_on_char_boundary(s, 6), "abcdef");
        assert_eq!(truncate_on_char_boundary(s, 3), "abc");
    }

    #[test]
    fn extract_tool_calls_survives_long_unicode_bash_command() {
        // Regression: a Bash command with a multibyte char straddling byte 120
        // must summarize without panicking.
        let cmd = format!("echo {}—done", "a".repeat(130));
        let msg = serde_json::json!({
            "content": [
                { "type": "tool_use", "name": "Bash", "input": { "command": cmd } }
            ]
        });
        let calls = extract_tool_calls(&msg);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].starts_with("Bash: echo "));
    }
}
