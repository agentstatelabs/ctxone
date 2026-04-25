//! Session ingestion: parse Claude Code JSONL transcripts, extract structured
//! memories via Haiku, and store token usage + facts in the Hub.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

// ── JSONL structures ──────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct TurnTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

impl TurnTokens {
    pub fn is_empty(&self) -> bool {
        self.input == 0 && self.output == 0
    }

    pub fn add(&mut self, other: &TurnTokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_creation += other.cache_creation;
    }
}

#[derive(Debug)]
pub struct Turn {
    pub user_text: String,
    pub assistant_text: String,
    pub tool_calls: Vec<String>,
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
            format!("{}…[truncated]", &self.assistant_text[..8000])
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
                // Start a new turn.
                let text = extract_text_content(entry.get("message"));
                if !text.trim().is_empty() {
                    current_user = Some(text);
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
                    if c.len() > 120 { format!("{}…", &c[..120]) } else { c.to_string() }
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
        eprintln!("  warn: Haiku returned {}: {}", status, &body[..body.len().min(200)]);
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
            eprintln!("  raw: {}", &json_str[..json_str.len().min(300)]);
            vec![]
        }
    }
}

// ── Hub writes ────────────────────────────────────────────────────────────────

pub async fn record_turn_tokens(
    tokens: &TurnTokens,
    model: &str,
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
        "model": model,
        "provider": "anthropic",
    });
    let mut req = client.post(format!("{}/api/stats/llm_usage", hub)).json(&body);
    if let Some(sid) = session {
        req = req.header("X-CTXone-Session", sid);
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
pub fn find_session_files(project_dir: &Path) -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    // Claude Code hashes the project path: replace '/' with '-'.
    let hash = project_dir
        .to_string_lossy()
        .replace('/', "-");
    let sessions_dir = home.join(".claude").join("projects").join(&hash);

    if !sessions_dir.exists() {
        return vec![];
    }

    let mut files: Vec<PathBuf> = std::fs::read_dir(&sessions_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false))
        .collect();

    // Sort by modification time, oldest first.
    files.sort_by_key(|p| {
        p.metadata()
            .and_then(|m| m.modified())
            .ok()
    });

    files
}

/// Find the single most-recent session file (for capture-turn).
pub fn latest_session_file(project_dir: &Path) -> Option<PathBuf> {
    find_session_files(project_dir).into_iter().last()
}

/// Return the last N turns from a session file (for per-turn hook capture).
pub fn last_turns(path: &Path, n: usize) -> Vec<Turn> {
    let all = parse_turns(path);
    let skip = all.len().saturating_sub(n);
    all.into_iter().skip(skip).collect()
}
