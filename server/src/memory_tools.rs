//! CtxOne memory-oriented MCP tools.
//!
//! Higher-level memory operations built on top of AgentStateGraph primitives.
//! Each tool includes token usage metadata (`_ctxone_stats`) for tracking savings.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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
pub struct SessionStats {
    pub tokens_sent: AtomicU64,
    pub tokens_saved: AtomicU64,
    pub total_graph_size_chars: AtomicU64,
}

impl SessionStats {
    pub fn new() -> Self {
        Self {
            tokens_sent: AtomicU64::new(0),
            tokens_saved: AtomicU64::new(0),
            total_graph_size_chars: AtomicU64::new(0),
        }
    }

    pub fn record(&self, sent_chars: usize, flat_chars: usize) {
        let sent_tokens = sent_chars / 4;
        let flat_tokens = flat_chars / 4;
        self.tokens_sent.fetch_add(sent_tokens as u64, Ordering::Relaxed);
        self.tokens_saved
            .fetch_add(flat_tokens.saturating_sub(sent_tokens) as u64, Ordering::Relaxed);
    }
}

/// Estimate the total flat memory size by counting all values in the graph.
fn estimate_flat_size(repo: &Repository) -> usize {
    match repo.get_json("main", "/") {
        Ok(val) => serde_json::to_string(&val).unwrap_or_default().len(),
        Err(_) => 0,
    }
}

/// Wrap a response string with token stats metadata.
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
}

#[derive(Deserialize, JsonSchema)]
pub struct RecallParams {
    /// Topic to search for.
    pub topic: String,
    /// Maximum token budget for the response (default: 1500).
    #[serde(default = "default_budget")]
    pub budget: usize,
}

#[derive(Deserialize, JsonSchema)]
pub struct ContextParams {
    /// Project or domain name.
    pub project: String,
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

fn default_importance() -> String {
    "medium".to_string()
}
fn default_budget() -> usize {
    1500
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
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl CtxOneServer {
    pub fn new(repo: Arc<Repository>) -> Self {
        let session = Arc::new(SessionStats::new());
        let flat = estimate_flat_size(&repo);
        session
            .total_graph_size_chars
            .store(flat as u64, Ordering::Relaxed);

        Self {
            repo,
            session,
            tool_router: Self::tool_router(),
        }
    }

    pub fn from_parts(repo: Arc<Repository>, session: Arc<SessionStats>) -> Self {
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
        let mut opts = CommitOptions::new("ctxone", IntentCategory::Custom("Observe".to_string()), &p.fact);
        opts = opts.with_confidence(confidence);
        if let Some(tags) = p.tags {
            opts = opts.with_tags(tags);
        }

        let value = serde_json::Value::String(p.fact.clone());
        match self.repo.set_json("main", &path, &value, opts) {
            Ok(commit_id) => {
                // Update flat size estimate
                let flat = estimate_flat_size(&self.repo);
                self.session
                    .total_graph_size_chars
                    .store(flat as u64, Ordering::Relaxed);

                let response = format!("Remembered at {}: {}", path, commit_id);
                with_stats(&response, flat, &self.session)
            }
            Err(e) => format!("Error: {}", e),
        }
    }

    #[tool(
        description = "Retrieve relevant memories for a topic, respecting a token budget. Returns the most important matches first."
    )]
    async fn recall(&self, params: Parameters<RecallParams>) -> String {
        let p = params.0;
        let flat_size = self
            .session
            .total_graph_size_chars
            .load(Ordering::Relaxed) as usize;

        // Search values for the topic
        let max_results = 50;
        match self.repo.search_values("main", &p.topic, Some(max_results)) {
            Ok(results) => {
                let mut output = String::new();
                let budget_chars = p.budget * 4; // ~4 chars per token
                for (path, value) in &results {
                    let line = format!("{}: {}\n", path, value);
                    if output.len() + line.len() > budget_chars {
                        break;
                    }
                    output.push_str(&line);
                }

                if output.is_empty() {
                    output = format!("No memories found for '{}'", p.topic);
                }

                with_stats(&output, flat_size, &self.session)
            }
            Err(e) => format!("Error: {}", e),
        }
    }

    #[tool(
        description = "Load the full context tree for a specific project or domain. Returns all stored state under that project path."
    )]
    async fn context(&self, params: Parameters<ContextParams>) -> String {
        let p = params.0;
        let flat_size = self
            .session
            .total_graph_size_chars
            .load(Ordering::Relaxed) as usize;

        let path = format!("/memory/projects/{}", p.project);
        match self.repo.get_json("main", &path) {
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
            &format!("Session {} summary", p.session_id),
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
                &format!("Session {} decisions", p.session_id),
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
            &format!("Session {} details", p.session_id),
        );
        let _ = self.repo.set_json(
            "main",
            &format!("/sessions/{}/details", p.session_id),
            &details_val,
            details_opts,
        );

        let flat = estimate_flat_size(&self.repo);
        self.session
            .total_graph_size_chars
            .store(flat as u64, Ordering::Relaxed);

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
        let flat_size = self
            .session
            .total_graph_size_chars
            .load(Ordering::Relaxed) as usize;

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
        let flat_size = self
            .session
            .total_graph_size_chars
            .load(Ordering::Relaxed) as usize;

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
                            output.push_str(&format!(
                                "{}",
                                serde_json::to_string_pretty(&blame).unwrap_or_default()
                            ));
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
