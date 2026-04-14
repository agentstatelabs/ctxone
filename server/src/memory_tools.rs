//! CtxOne memory-oriented MCP tools.
//!
//! Higher-level memory operations built on top of AgentStateGraph primitives.
//! Each tool includes token usage metadata (`_ctxone_stats`) for tracking savings.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use agentstategraph::{CommitOptions, Repository};
use agentstategraph_core::IntentCategory;

/// Token usage statistics for a single response.
#[derive(Serialize)]
pub struct TokenStats {
    /// Tokens actually sent in this response.
    pub ctx_tokens_sent: usize,
    /// Estimated tokens if full memory had been loaded (flat model).
    pub ctx_tokens_estimated_flat: usize,
    /// Savings ratio.
    pub ctx_savings_ratio: f64,
}

/// Cumulative session statistics.
///
/// The graph size is cached lazily: writes set `graph_size_dirty = true`,
/// and the next read that needs the size calls `ensure_flat_size` to
/// refresh it. This means a batch of writes only pays the full-walk
/// cost once, on the next read that cares.
pub struct SessionStats {
    pub tokens_sent: AtomicU64,
    pub tokens_saved: AtomicU64,
    pub total_graph_size_chars: AtomicU64,
    graph_size_dirty: AtomicBool,
}

impl SessionStats {
    pub fn new() -> Self {
        Self {
            tokens_sent: AtomicU64::new(0),
            tokens_saved: AtomicU64::new(0),
            total_graph_size_chars: AtomicU64::new(0),
            graph_size_dirty: AtomicBool::new(true),
        }
    }

    pub fn record(&self, sent_chars: usize, flat_chars: usize) {
        let sent_tokens = sent_chars / 4;
        let flat_tokens = flat_chars / 4;
        self.tokens_sent
            .fetch_add(sent_tokens as u64, Ordering::Relaxed);
        self.tokens_saved.fetch_add(
            flat_tokens.saturating_sub(sent_tokens) as u64,
            Ordering::Relaxed,
        );
    }

    /// Mark the cached graph size as stale. Call after any write.
    pub fn mark_dirty(&self) {
        self.graph_size_dirty.store(true, Ordering::Relaxed);
    }
}

/// Estimate the total flat memory size by counting all values in the graph.
pub fn estimate_flat_size(repo: &Repository, ref_name: &str) -> usize {
    match repo.get_json(ref_name, "/") {
        Ok(val) => serde_json::to_string(&val).unwrap_or_default().len(),
        Err(_) => 0,
    }
}

/// Ensure the cached flat-size is current. If dirty, refreshes it and clears the flag.
/// Call this just before reading `session.total_graph_size_chars` in a read-heavy path.
pub fn ensure_flat_size(repo: &Repository, session: &SessionStats, ref_name: &str) {
    if session.graph_size_dirty.load(Ordering::Relaxed) {
        let size = estimate_flat_size(repo, ref_name) as u64;
        session
            .total_graph_size_chars
            .store(size, Ordering::Relaxed);
        session.graph_size_dirty.store(false, Ordering::Relaxed);
    }
}

/// Wrap a response string with token stats metadata.
/// Used by the older MCP tools that return plain text (context, summarize_session, etc).
fn with_stats(response: &str, flat_size: usize, session: &SessionStats) -> String {
    let sent = response.len();
    session.record(sent, flat_size);

    let stats = TokenStats {
        ctx_tokens_sent: sent / 4,
        ctx_tokens_estimated_flat: flat_size / 4,
        ctx_savings_ratio: if sent > 0 {
            flat_size as f64 / sent as f64
        } else {
            0.0
        },
    };

    let stats_json = serde_json::to_string(&stats).unwrap_or_default();
    format!("{}\n\n_ctxone_stats: {}", response, stats_json)
}

// -- Shared helpers used by both MCP tools and HTTP handlers --

/// A structured {path, title, body} entry for a pinned memory section.
#[derive(Clone)]
pub struct PinnedEntry {
    pub path: String,
    pub title: String,
    pub body: String,
}

/// Collect pinned memories as title+body pairs grouped by section path.
/// Each section has /title and /body children — this pairs them up.
pub fn collect_pinned(repo: &Repository, ref_name: &str) -> Vec<PinnedEntry> {
    let paths = match repo.list_paths(ref_name, "/memory/pinned", Some(20)) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    let mut sections: std::collections::BTreeMap<String, (Option<String>, Option<String>)> =
        std::collections::BTreeMap::new();

    for path in &paths {
        let (section_path, field) = if let Some(stripped) = path.strip_suffix("/title") {
            (stripped.to_string(), "title")
        } else if let Some(stripped) = path.strip_suffix("/body") {
            (stripped.to_string(), "body")
        } else {
            continue;
        };

        let Ok(value) = repo.get_json(ref_name, path) else {
            continue;
        };
        let Some(text) = value.as_str() else {
            continue;
        };

        let entry = sections.entry(section_path).or_insert((None, None));
        match field {
            "title" => entry.0 = Some(text.to_string()),
            "body" => entry.1 = Some(text.to_string()),
            _ => {}
        }
    }

    sections
        .into_iter()
        .filter_map(|(path, (title, body))| match (title, body) {
            (Some(t), Some(b)) => Some(PinnedEntry {
                path,
                title: t,
                body: b,
            }),
            _ => None,
        })
        .collect()
}

/// Shared recall implementation: always-include pinned, then topic matches, budget-capped.
/// Returns a structured JSON value. Both MCP tools and HTTP handlers call this.
pub fn run_recall(
    repo: &Repository,
    session: &SessionStats,
    topic: &str,
    budget: usize,
    ref_name: &str,
) -> serde_json::Value {
    let budget_chars = budget * 4;

    let mut out = Vec::new();
    let mut total = 0usize;
    let mut seen_paths = std::collections::HashSet::new();

    // 1. Pinned memories (up to half the budget)
    let pinned = collect_pinned(repo, ref_name);
    let pinned_count = pinned.len();
    let pinned_budget = budget_chars / 2;
    let mut pinned_total = 0usize;
    for p in &pinned {
        let entry_size = p.path.len() + p.title.len() + p.body.len() + 30;
        if pinned_total + entry_size > pinned_budget && !out.is_empty() {
            break;
        }
        out.push(serde_json::json!({
            "path": p.path,
            "title": p.title,
            "body": p.body,
            "pinned": true,
        }));
        seen_paths.insert(p.path.clone());
        pinned_total += entry_size;
        total += entry_size;
    }

    // 2. Topic search: tokenize the query, search each token, aggregate by path
    //    and score by how many tokens matched + whether the full phrase hit.
    let tokens = tokenize_query(topic);

    // Map path -> (value, token_hit_count, full_phrase_hit)
    let mut scored: std::collections::HashMap<String, (String, usize, bool)> =
        std::collections::HashMap::new();

    // Full-phrase search: counts extra, so exact matches win
    if !topic.trim().is_empty()
        && let Ok(results) = repo.search_values(ref_name, topic.trim(), Some(50))
    {
        for (path, value) in results {
            scored
                .entry(path)
                .and_modify(|e| e.2 = true)
                .or_insert((value, 0, true));
        }
    }

    // Per-token search: each token adds one to the score
    for token in &tokens {
        let Ok(results) = repo.search_values(ref_name, token, Some(50)) else {
            continue;
        };
        for (path, value) in results {
            let entry = scored.entry(path).or_insert((value, 0, false));
            entry.1 += 1;
        }
    }

    // Sort: full-phrase hits first, then by token-hit count desc, then by path
    let mut ranked: Vec<(String, String, usize, bool)> = scored
        .into_iter()
        .map(|(p, (v, count, full))| (p, v, count, full))
        .collect();
    ranked.sort_by(|a, b| {
        b.3.cmp(&a.3) // full phrase wins
            .then_with(|| b.2.cmp(&a.2)) // then token count
            .then_with(|| a.0.cmp(&b.0)) // stable by path
    });

    let mut topic_matches = 0usize;
    for (path, value, score, full_match) in &ranked {
        let section_path = path
            .strip_suffix("/title")
            .or_else(|| path.strip_suffix("/body"))
            .map(String::from)
            .unwrap_or_else(|| path.clone());
        if seen_paths.contains(&section_path) {
            continue;
        }

        let entry_size = path.len() + value.len() + 10;
        if total + entry_size > budget_chars {
            break;
        }
        out.push(serde_json::json!({
            "path": path,
            "value": value,
            "pinned": false,
            "score": score,
            "full_match": full_match,
        }));
        total += entry_size;
        topic_matches += 1;
    }

    ensure_flat_size(repo, session, ref_name);
    let flat_size = session.total_graph_size_chars.load(Ordering::Relaxed) as usize;
    session.record(total, flat_size);

    serde_json::json!({
        "topic": topic,
        "ref": ref_name,
        "results": out,
        "pinned_count": pinned_count,
        "topic_matches": topic_matches,
        "ctx_tokens_sent": total / 4,
        "ctx_tokens_estimated_flat": flat_size / 4,
        "ctx_savings_ratio": if total > 0 { flat_size as f64 / total as f64 } else { 0.0 },
    })
}

/// Shared prime implementation: write sections under /memory/{pinned|primed}/{source}/{slug}.
pub fn run_prime(
    repo: &Repository,
    session: &SessionStats,
    source: &str,
    pinned: bool,
    sections: &[(String, String)], // (title, body)
    ref_name: &str,
) -> Result<serde_json::Value, String> {
    let namespace = if pinned { "pinned" } else { "primed" };
    let mut written = Vec::new();

    for (title, body) in sections {
        let slug = slugify(title);
        if slug.is_empty() {
            continue;
        }
        let path = format!("/memory/{}/{}/{}", namespace, source, slug);

        let opts = CommitOptions::new(
            "ctxone-prime",
            IntentCategory::Checkpoint,
            format!("Prime: {}", title),
        )
        .with_confidence(0.95)
        .with_tags(vec![
            namespace.to_string(),
            source.to_string(),
            "prime".to_string(),
        ]);

        let value = serde_json::json!({
            "title": title,
            "body": body,
        });

        repo.set_json(ref_name, &path, &value, opts)
            .map_err(|e| e.to_string())?;

        written.push(path);
    }

    session.mark_dirty();

    Ok(serde_json::json!({
        "status": "ok",
        "ref": ref_name,
        "source": source,
        "pinned": pinned,
        "sections_written": written.len(),
        "paths": written,
    }))
}

/// Tokenize a recall query: lowercase, split on non-alphanumeric, drop stopwords
/// and tokens shorter than 3 characters.
pub fn tokenize_query(q: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "with", "about", "that", "this", "what", "how", "why", "our", "use",
        "are", "was", "were", "from", "into", "their", "its", "has", "have", "will", "can", "did",
        "does", "some", "any", "all", "some",
    ];

    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in q.chars() {
        if c.is_ascii_alphanumeric() {
            current.push(c.to_ascii_lowercase());
        } else if !current.is_empty() {
            if current.len() >= 3 && !STOPWORDS.contains(&current.as_str()) {
                tokens.push(current.clone());
            }
            current.clear();
        }
    }
    if current.len() >= 3 && !STOPWORDS.contains(&current.as_str()) {
        tokens.push(current);
    }
    tokens
}

/// Slugify a string for use in paths.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

// -- Parameter types --

#[derive(Deserialize, JsonSchema)]
pub struct RememberParams {
    /// The fact, preference, or decision to store.
    pub fact: String,
    /// Importance: "high", "medium", "low".
    #[serde(default = "default_importance")]
    pub importance: String,
    /// Category/context (e.g., "licensing", "architecture", "preferences").
    pub context: Option<String>,
    /// Tags for queryability.
    pub tags: Option<Vec<String>>,
    /// Branch to write to (default: "main").
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct RecallParams {
    /// Topic to search for.
    pub topic: String,
    /// Maximum token budget for the response (default: 1500).
    #[serde(default = "default_budget")]
    pub budget: usize,
    /// Branch to search (default: "main").
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ContextParams {
    /// Project or domain name.
    pub project: String,
    /// Branch to read (default: "main").
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SummarizeSessionParams {
    /// Session identifier.
    pub session_id: String,
    /// Key points from this session.
    pub key_points: Vec<String>,
    /// Decisions made during this session.
    #[serde(default)]
    pub decisions: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct WhatChangedSinceParams {
    /// ISO 8601 timestamp (e.g., "2026-04-12T00:00:00Z").
    pub since: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct WhyDidWeParams {
    /// The decision to trace (e.g., "use BSL 1.1").
    pub decision: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PrimeSectionParam {
    /// Section title (used as a stable path slug).
    pub title: String,
    /// Section body (markdown or plain text).
    pub body: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PrimeParams {
    /// Source name (groups sections together, enables idempotent re-priming).
    pub source: String,
    /// If true, store as pinned memory (always included in every recall response).
    /// If false, store as primed memory (searchable like normal facts).
    #[serde(default)]
    pub pinned: bool,
    /// Pre-parsed markdown sections. Use ctx prime <file.md> on the CLI to parse a file.
    pub sections: Vec<PrimeSectionParam>,
    /// Branch to write to (default: "main").
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

fn default_importance() -> String {
    "medium".to_string()
}
fn default_budget() -> usize {
    1500
}
fn default_ref() -> String {
    "main".to_string()
}

fn importance_to_confidence(importance: &str) -> f64 {
    match importance {
        "high" => 0.95,
        "medium" => 0.7,
        "low" => 0.4,
        _ => 0.7,
    }
}

fn timestamp_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    format!("{:x}", now.as_nanos())
}

/// The CtxOne memory MCP server.
#[derive(Clone)]
pub struct CtxOneServer {
    pub repo: Arc<Repository>,
    pub session: Arc<SessionStats>,
    #[allow(dead_code)] // used by rmcp tool_router macro
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl CtxOneServer {
    pub fn new(repo: Arc<Repository>) -> Self {
        let session = Arc::new(SessionStats::new());
        // session starts dirty; first read will populate it.
        Self {
            repo,
            session,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Store a fact, preference, or decision in agent memory. Facts are searchable and carry confidence scores based on importance."
    )]
    async fn remember(&self, params: Parameters<RememberParams>) -> String {
        let p = params.0;
        let path = match &p.context {
            Some(ctx) => format!("/memory/{}/{}", ctx, timestamp_id()),
            None => format!("/memory/facts/{}", timestamp_id()),
        };

        let confidence = importance_to_confidence(&p.importance);
        let mut opts = CommitOptions::new(
            "ctxone",
            IntentCategory::Custom("Observe".to_string()),
            &p.fact,
        );
        opts = opts.with_confidence(confidence);
        if let Some(tags) = p.tags {
            opts = opts.with_tags(tags);
        }

        let value = serde_json::Value::String(p.fact.clone());
        match self.repo.set_json(&p.ref_name, &path, &value, opts) {
            Ok(commit_id) => {
                self.session.mark_dirty();
                serde_json::json!({
                    "status": "ok",
                    "ref": p.ref_name,
                    "fact": p.fact,
                    "path": path,
                    "commit_id": format!("{}", commit_id.short()),
                })
                .to_string()
            }
            Err(e) => format!("Error: {}", e),
        }
    }

    #[tool(
        description = "Retrieve relevant memories for a topic. Always includes pinned context first, then topic-matched facts, respecting a token budget. Response is JSON including token savings metadata."
    )]
    async fn recall(&self, params: Parameters<RecallParams>) -> String {
        let p = params.0;
        let result = run_recall(&self.repo, &self.session, &p.topic, p.budget, &p.ref_name);
        serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
    }

    #[tool(
        description = "Load markdown sections as pinned or primed memories. Pinned memories are always included in every recall response (critical context). Sections should be pre-parsed — each entry has a title and body."
    )]
    async fn prime(&self, params: Parameters<PrimeParams>) -> String {
        let p = params.0;
        let sections: Vec<(String, String)> =
            p.sections.into_iter().map(|s| (s.title, s.body)).collect();

        match run_prime(
            &self.repo,
            &self.session,
            &p.source,
            p.pinned,
            &sections,
            &p.ref_name,
        ) {
            Ok(result) => {
                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
            }
            Err(e) => format!("Error: {}", e),
        }
    }

    #[tool(
        description = "Load the full context tree for a specific project or domain. Returns all stored state under that project path."
    )]
    async fn context(&self, params: Parameters<ContextParams>) -> String {
        let p = params.0;
        ensure_flat_size(&self.repo, &self.session, &p.ref_name);
        let flat_size = self.session.total_graph_size_chars.load(Ordering::Relaxed) as usize;

        let path = format!("/memory/projects/{}", p.project);
        match self.repo.get_json(&p.ref_name, &path) {
            Ok(value) => {
                let response =
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "null".to_string());
                with_stats(&response, flat_size, &self.session)
            }
            Err(e) => format!("No context found for '{}': {}", p.project, e),
        }
    }

    #[tool(
        description = "End-of-session commit capturing what was learned and decided. Call this before closing a session to persist its knowledge."
    )]
    async fn summarize_session(&self, params: Parameters<SummarizeSessionParams>) -> String {
        let p = params.0;

        // Write summary
        let summary = p.key_points.join(". ");
        let summary_opts = CommitOptions::new(
            "ctxone",
            IntentCategory::Checkpoint,
            format!("Session {} summary", p.session_id),
        )
        .with_confidence(0.9);

        let summary_val = serde_json::Value::String(summary);
        let _ = self.repo.set_json(
            "main",
            &format!("/sessions/{}/summary", p.session_id),
            &summary_val,
            summary_opts,
        );

        // Write decisions
        if !p.decisions.is_empty() {
            let decisions_val = serde_json::json!(p.decisions);
            let decisions_opts = CommitOptions::new(
                "ctxone",
                IntentCategory::Checkpoint,
                format!("Session {} decisions", p.session_id),
            )
            .with_confidence(0.95);

            let _ = self.repo.set_json(
                "main",
                &format!("/sessions/{}/decisions", p.session_id),
                &decisions_val,
                decisions_opts,
            );
        }

        // Write full details
        let details_val = serde_json::json!(p.key_points);
        let details_opts = CommitOptions::new(
            "ctxone",
            IntentCategory::Custom("Observe".to_string()),
            format!("Session {} details", p.session_id),
        );
        let _ = self.repo.set_json(
            "main",
            &format!("/sessions/{}/details", p.session_id),
            &details_val,
            details_opts,
        );

        self.session.mark_dirty();
        ensure_flat_size(&self.repo, &self.session, "main");
        let flat = self.session.total_graph_size_chars.load(Ordering::Relaxed) as usize;

        let response = format!(
            "Session '{}' saved: {} key points, {} decisions",
            p.session_id,
            p.key_points.len(),
            p.decisions.len()
        );
        with_stats(&response, flat, &self.session)
    }

    #[tool(
        description = "See what has changed in the memory graph since a given date. Shows recent commits and their intents."
    )]
    async fn what_changed_since(&self, params: Parameters<WhatChangedSinceParams>) -> String {
        let p = params.0;
        ensure_flat_size(&self.repo, &self.session, "main");
        let flat_size = self.session.total_graph_size_chars.load(Ordering::Relaxed) as usize;

        // Get recent log and filter by date
        match self.repo.log("main", 100) {
            Ok(commits) => {
                let mut output = String::new();
                for commit in &commits {
                    let ts = commit.timestamp.to_rfc3339();
                    if ts.as_str() >= p.since.as_str() {
                        output.push_str(&format!(
                            "{} [{:?}] {} (confidence: {:.2})\n",
                            &ts[..19],
                            commit.intent.category,
                            commit.intent.description,
                            commit.confidence.unwrap_or(0.0),
                        ));
                    }
                }

                if output.is_empty() {
                    output = format!("No changes since {}", p.since);
                }

                with_stats(&output, flat_size, &self.session)
            }
            Err(e) => format!("Error: {}", e),
        }
    }

    #[tool(
        description = "Trace the reasoning behind a past decision. Searches for the decision and returns its full provenance chain (blame)."
    )]
    async fn why_did_we(&self, params: Parameters<WhyDidWeParams>) -> String {
        let p = params.0;
        ensure_flat_size(&self.repo, &self.session, "main");
        let flat_size = self.session.total_graph_size_chars.load(Ordering::Relaxed) as usize;

        // Search for the decision
        match self.repo.search_values("main", &p.decision, Some(5)) {
            Ok(results) => {
                if results.is_empty() {
                    return format!("No record found for decision: '{}'", p.decision);
                }

                let mut output = String::new();
                for (path, _value) in &results {
                    output.push_str(&format!("Path: {}\n", path));
                    // Get blame for this path
                    match self.repo.blame("main", path) {
                        Ok(blame) => {
                            output.push_str(
                                &serde_json::to_string_pretty(&blame)
                                    .unwrap_or_default()
                                    .to_string(),
                            );
                        }
                        Err(e) => {
                            output.push_str(&format!("  (blame unavailable: {})\n", e));
                        }
                    }
                    output.push('\n');
                }

                with_stats(&output, flat_size, &self.session)
            }
            Err(e) => format!("Error: {}", e),
        }
    }
}

#[tool_handler]
impl ServerHandler for CtxOneServer {}
