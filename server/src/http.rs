//! HTTP REST API for CtxOne Hub.
//!
//! Exposes:
//!   - Basic read endpoints that the Lens web UI needs (health, stats, state, log, search)
//!   - Memory-oriented write endpoints matching the MCP tools (remember, recall, context, etc.)
//!   - Token savings endpoint: GET /api/stats/tokens

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

use agentstategraph::{CommitOptions, Repository};
use agentstategraph_core::IntentCategory;

use crate::memory_tools::{SessionStats, refresh_flat_size, run_prime, run_recall};

#[derive(Clone)]
pub struct HubState {
    pub repo: Arc<Repository>,
    pub session: Arc<SessionStats>,
}

pub fn router(repo: Arc<Repository>, session: Arc<SessionStats>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let state = HubState { repo, session };

    Router::new()
        // Health + stats
        .route("/api/health", get(health))
        .route("/api/stats/tokens", get(token_stats))
        .route("/api/stats/{ref_name}", get(stats))
        // Read endpoints (for Lens)
        .route("/api/state/{ref_name}", get(get_state))
        .route("/api/state/{ref_name}/paths", get(list_paths))
        .route("/api/state/{ref_name}/search", get(search_values))
        .route("/api/log/{ref_name}", get(get_log))
        .route("/api/blame/{ref_name}", get(blame))
        .route("/api/branches", get(list_branches))
        // Memory endpoints (high-level)
        .route("/api/memory/remember", post(remember))
        .route("/api/memory/recall", get(recall))
        .route("/api/memory/context/{project}", get(context))
        .route("/api/memory/prime", post(prime))
        .route("/api/memory/pinned", get(list_pinned))
        .route("/api/memory/summarize_session", post(summarize_session))
        .route("/api/memory/what_changed_since", get(what_changed_since))
        .route("/api/memory/why_did_we", get(why_did_we))
        .layer(cors)
        .with_state(state)
}

// -- Helpers --

fn internal_error(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
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

// -- Handlers --

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok", "service": "ctxone-hub" }))
}

#[derive(Serialize)]
struct TokenStatsResponse {
    session_tokens_used: u64,
    session_tokens_saved: u64,
    total_graph_size_chars: u64,
    total_graph_size_tokens: u64,
    cumulative_ratio: f64,
}

async fn token_stats(State(s): State<HubState>) -> impl IntoResponse {
    let used = s.session.tokens_sent.load(Ordering::Relaxed);
    let saved = s.session.tokens_saved.load(Ordering::Relaxed);
    let graph_chars = s.session.total_graph_size_chars.load(Ordering::Relaxed);
    let graph_tokens = graph_chars / 4;
    let ratio = if used > 0 {
        (used + saved) as f64 / used as f64
    } else {
        0.0
    };

    Json(TokenStatsResponse {
        session_tokens_used: used,
        session_tokens_saved: saved,
        total_graph_size_chars: graph_chars,
        total_graph_size_tokens: graph_tokens,
        cumulative_ratio: ratio,
    })
}

async fn stats(
    State(s): State<HubState>,
    Path(ref_name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    s.repo.stats(&ref_name).map(Json).map_err(internal_error)
}

#[derive(Deserialize)]
struct PathQuery {
    path: Option<String>,
}

async fn get_state(
    State(s): State<HubState>,
    Path(ref_name): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = q.path.unwrap_or_else(|| "/".to_string());
    s.repo
        .get_json(&ref_name, &path)
        .map(Json)
        .map_err(internal_error)
}

#[derive(Deserialize)]
struct PrefixQuery {
    prefix: Option<String>,
    max_depth: Option<usize>,
}

async fn list_paths(
    State(s): State<HubState>,
    Path(ref_name): Path<String>,
    Query(q): Query<PrefixQuery>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let prefix = q.prefix.unwrap_or_else(|| "/".to_string());
    s.repo
        .list_paths(&ref_name, &prefix, q.max_depth)
        .map(Json)
        .map_err(internal_error)
}

#[derive(Deserialize)]
struct SearchQuery {
    query: String,
    max_results: Option<usize>,
}

async fn search_values(
    State(s): State<HubState>,
    Path(ref_name): Path<String>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResult>>, (StatusCode, String)> {
    let results = s
        .repo
        .search_values(&ref_name, &q.query, q.max_results)
        .map_err(internal_error)?;
    let out = results
        .into_iter()
        .map(|(path, value)| SearchResult { path, value })
        .collect();
    Ok(Json(out))
}

#[derive(Serialize)]
struct SearchResult {
    path: String,
    value: String,
}

#[derive(Deserialize)]
struct LogQuery {
    limit: Option<usize>,
}

async fn get_log(
    State(s): State<HubState>,
    Path(ref_name): Path<String>,
    Query(q): Query<LogQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(20);
    let commits = s.repo.log(&ref_name, limit).map_err(internal_error)?;

    let out: Vec<serde_json::Value> = commits
        .into_iter()
        .map(|c| {
            serde_json::json!({
                "id": format!("{}", c.id.short()),
                "timestamp": c.timestamp.to_rfc3339(),
                "agent_id": c.agent_id,
                "confidence": c.confidence,
                "intent": {
                    "category": format!("{:?}", c.intent.category),
                    "description": c.intent.description,
                    "tags": c.intent.tags,
                },
                "reasoning": c.reasoning,
            })
        })
        .collect();

    Ok(Json(out))
}

async fn blame(
    State(s): State<HubState>,
    Path(ref_name): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = q.path.unwrap_or_else(|| "/".to_string());
    let blame = s.repo.blame(&ref_name, &path).map_err(internal_error)?;
    Ok(Json(serde_json::to_value(&blame).unwrap_or_default()))
}

async fn list_branches(
    State(s): State<HubState>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let branches = s.repo.list_branches(None).map_err(internal_error)?;
    let out: Vec<serde_json::Value> = branches
        .into_iter()
        .map(|(name, id)| serde_json::json!({ "name": name, "id": format!("{}", id.short()) }))
        .collect();
    Ok(Json(out))
}

// -- Memory endpoints --

#[derive(Deserialize)]
struct RememberRequest {
    fact: String,
    #[serde(default = "default_importance")]
    importance: String,
    context: Option<String>,
    tags: Option<Vec<String>>,
}

fn default_importance() -> String {
    "medium".to_string()
}

async fn remember(
    State(s): State<HubState>,
    Json(req): Json<RememberRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = match &req.context {
        Some(ctx) => format!("/memory/{}/{}", ctx, timestamp_id()),
        None => format!("/memory/facts/{}", timestamp_id()),
    };

    let confidence = importance_to_confidence(&req.importance);
    let mut opts = CommitOptions::new(
        "ctxone",
        IntentCategory::Custom("Observe".to_string()),
        &req.fact,
    );
    opts = opts.with_confidence(confidence);
    if let Some(tags) = req.tags {
        opts = opts.with_tags(tags);
    }

    let value = serde_json::Value::String(req.fact.clone());
    let commit_id = s
        .repo
        .set_json("main", &path, &value, opts)
        .map_err(internal_error)?;

    refresh_flat_size(&s.repo, &s.session);

    Ok(Json(serde_json::json!({
        "status": "ok",
        "path": path,
        "commit_id": format!("{}", commit_id.short()),
    })))
}

#[derive(Deserialize)]
struct RecallQuery {
    topic: String,
    budget: Option<usize>,
}

async fn recall(
    State(s): State<HubState>,
    Query(q): Query<RecallQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let budget = q.budget.unwrap_or(1500);
    Ok(Json(run_recall(&s.repo, &s.session, &q.topic, budget)))
}

async fn context(
    State(s): State<HubState>,
    Path(project): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = format!("/memory/projects/{}", project);
    match s.repo.get_json("main", &path) {
        Ok(value) => {
            let flat_size = s.session.total_graph_size_chars.load(Ordering::Relaxed) as usize;
            let sent = serde_json::to_string(&value).unwrap_or_default().len();
            s.session.record(sent, flat_size);
            Ok(Json(serde_json::json!({
                "project": project,
                "context": value,
                "ctx_tokens_sent": sent / 4,
                "ctx_tokens_estimated_flat": flat_size / 4,
            })))
        }
        Err(_) => Ok(Json(serde_json::json!({
            "project": project,
            "context": null,
            "message": "No context found",
        }))),
    }
}

#[derive(Deserialize)]
struct SummarizeSessionRequest {
    session_id: String,
    key_points: Vec<String>,
    #[serde(default)]
    decisions: Vec<String>,
}

async fn summarize_session(
    State(s): State<HubState>,
    Json(req): Json<SummarizeSessionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let summary = req.key_points.join(". ");
    let summary_opts = CommitOptions::new(
        "ctxone",
        IntentCategory::Checkpoint,
        format!("Session {} summary", req.session_id),
    )
    .with_confidence(0.9);

    let summary_val = serde_json::Value::String(summary);
    s.repo
        .set_json(
            "main",
            &format!("/sessions/{}/summary", req.session_id),
            &summary_val,
            summary_opts,
        )
        .map_err(internal_error)?;

    if !req.decisions.is_empty() {
        let decisions_val = serde_json::json!(req.decisions);
        let decisions_opts = CommitOptions::new(
            "ctxone",
            IntentCategory::Checkpoint,
            format!("Session {} decisions", req.session_id),
        )
        .with_confidence(0.95);

        s.repo
            .set_json(
                "main",
                &format!("/sessions/{}/decisions", req.session_id),
                &decisions_val,
                decisions_opts,
            )
            .map_err(internal_error)?;
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "session_id": req.session_id,
        "key_points": req.key_points.len(),
        "decisions": req.decisions.len(),
    })))
}

#[derive(Deserialize)]
struct WhatChangedQuery {
    since: String,
}

async fn what_changed_since(
    State(s): State<HubState>,
    Query(q): Query<WhatChangedQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let commits = s.repo.log("main", 100).map_err(internal_error)?;
    let out: Vec<serde_json::Value> = commits
        .into_iter()
        .filter(|c| c.timestamp.to_rfc3339().as_str() >= q.since.as_str())
        .map(|c| {
            serde_json::json!({
                "timestamp": c.timestamp.to_rfc3339(),
                "category": format!("{:?}", c.intent.category),
                "description": c.intent.description,
                "confidence": c.confidence,
            })
        })
        .collect();
    Ok(Json(out))
}

#[derive(Deserialize)]
struct WhyDidWeQuery {
    decision: String,
}

async fn why_did_we(
    State(s): State<HubState>,
    Query(q): Query<WhyDidWeQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let results = s
        .repo
        .search_values("main", &q.decision, Some(5))
        .map_err(internal_error)?;

    let mut traces = Vec::new();
    for (path, _) in &results {
        if let Ok(blame_info) = s.repo.blame("main", path) {
            traces.push(serde_json::json!({
                "path": path,
                "blame": serde_json::to_value(&blame_info).unwrap_or_default(),
            }));
        }
    }

    Ok(Json(serde_json::json!({
        "decision": q.decision,
        "traces": traces,
    })))
}

// -- Prime / pinned context --

#[derive(Deserialize)]
struct PrimeSection {
    title: String,
    body: String,
}

#[derive(Deserialize)]
struct PrimeRequest {
    /// Source name (e.g., "project", "onboarding"). Groups sections together.
    source: String,
    /// If true, store under /memory/pinned/ (always loaded by recall).
    /// Otherwise store under /memory/primed/ (searchable like normal facts).
    #[serde(default)]
    pinned: bool,
    /// Parsed markdown sections.
    sections: Vec<PrimeSection>,
}

async fn prime(
    State(s): State<HubState>,
    Json(req): Json<PrimeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let sections: Vec<(String, String)> = req
        .sections
        .into_iter()
        .map(|s| (s.title, s.body))
        .collect();

    run_prime(&s.repo, &s.session, &req.source, req.pinned, &sections)
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
}

async fn list_pinned(
    State(s): State<HubState>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let paths = s
        .repo
        .list_paths("main", "/memory/pinned", Some(20))
        .map_err(internal_error)?;

    let mut out = Vec::new();
    for p in &paths {
        if let Ok(val) = s.repo.get_json("main", p) {
            out.push(serde_json::json!({
                "path": p,
                "value": val,
            }));
        }
    }
    Ok(Json(out))
}
