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
use tower_http::trace::TraceLayer;
use tracing::{debug, info, instrument, warn};

use agentstategraph::{CommitOptions, Repository};
use agentstategraph_core::IntentCategory;

use crate::memory_tools::{
    DEFAULT_AGENT_ID, DEFAULT_SESSION_ID, SessionRegistry, SessionSnapshot, SessionStats,
    ensure_flat_size, run_prime, run_recall,
};
use crate::plan_tools;
use crate::rate_limit;

/// Hub-wide HTTP configuration.
///
/// The `Default` impl is derived — the zero value for `rate_limit_rpm`
/// (disabled) is deliberately the library default so in-process tests
/// using `tower::ServiceExt::oneshot` (no real peer IP) work without
/// flakes. The Hub binary sets this explicitly from the
/// `--rate-limit-rpm` CLI flag (which defaults to 600 in production).
#[derive(Clone, Debug, Default)]
pub struct HubConfig {
    /// Requests-per-minute per peer IP. `0` disables rate limiting.
    pub rate_limit_rpm: u32,
}

#[derive(Clone)]
pub struct HubState {
    pub repo: Arc<Repository>,
    pub sessions: Arc<SessionRegistry>,
}

impl HubState {
    /// Resolve the session for this request. Always returns a valid
    /// `Arc<SessionStats>` — if the session didn't exist, it's
    /// created on the fly.
    fn session_for(&self, id: &SessionId) -> Arc<SessionStats> {
        self.sessions.get_or_create(&id.0)
    }
}

/// Extractor for the session identifier carried by the
/// `X-CTXone-Session` request header. Falls back to `"default"` when
/// the header is absent or malformed.
#[derive(Debug, Clone)]
pub struct SessionId(pub String);

impl<S> axum::extract::FromRequestParts<S> for SessionId
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let id = parts
            .headers
            .get("x-ctxone-session")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_SESSION_ID.to_string());
        Ok(SessionId(id))
    }
}

/// Extractor for the agent identifier carried by the `X-CTXone-Agent`
/// request header. Falls back to `"ctxone"` when the header is absent
/// or empty. The agent ID is what `ctx blame` displays in the "who"
/// column, so setting it correctly is the cheapest way to answer the
/// question "which tool wrote this fact?".
#[derive(Debug, Clone)]
pub struct AgentId(pub String);

impl<S> axum::extract::FromRequestParts<S> for AgentId
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let id = parts
            .headers
            .get("x-ctxone-agent")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_AGENT_ID.to_string());
        Ok(AgentId(id))
    }
}

/// Build the Hub router with default config.
///
/// Call sites typically construct an empty `SessionRegistry` via
/// `Arc::new(SessionRegistry::new())`. The registry grows as
/// requests arrive and each new `X-CTXone-Session` header is seen.
pub fn router(repo: Arc<Repository>, sessions: Arc<SessionRegistry>) -> Router {
    router_with_config(repo, sessions, HubConfig::default())
}

/// Build the Hub router with Lens UI mounted at `/`.
///
/// API routes (`/api/*`) are handled normally. Every other path falls
/// through to the embedded Lens SPA — index.html is served as the
/// catch-all for client-side routing. Start the Hub with `--lens` to
/// activate this router.
pub fn router_with_lens(
    repo: Arc<Repository>,
    sessions: Arc<SessionRegistry>,
    config: HubConfig,
) -> Router {
    router_with_config(repo, sessions, config)
        .fallback(crate::lens::lens_handler)
}

/// Build the Hub router with explicit HTTP configuration.
pub fn router_with_config(
    repo: Arc<Repository>,
    sessions: Arc<SessionRegistry>,
    config: HubConfig,
) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Request tracing layer — emits a span per HTTP request. At `info` level
    // you get one line per request with method, URI, status, and latency.
    // At `debug` you also get the request body, at `trace` the response body.
    let trace = TraceLayer::new_for_http();

    // Rate limiter — returns None when rpm=0 (disabled).
    let governor = rate_limit::build_layer(config.rate_limit_rpm);

    let state = HubState { repo, sessions };

    let mut router = Router::new()
        // Health + stats
        .route("/api/health", get(health))
        .route("/api/stats/tokens", get(token_stats))
        .route("/api/stats/tokens/{session_id}", get(session_token_stats))
        .route("/api/stats/sessions", get(list_sessions))
        .route("/api/stats/llm_usage", post(record_llm_usage))
        .route("/api/stats/{ref_name}", get(stats))
        // Read endpoints (for Lens)
        .route("/api/state/{ref_name}", get(get_state))
        .route("/api/state/{ref_name}/paths", get(list_paths))
        .route("/api/state/{ref_name}/search", get(search_values))
        .route("/api/log/{ref_name}", get(get_log))
        .route("/api/blame/{ref_name}", get(blame))
        .route("/api/diff", get(diff_refs))
        .route("/api/merge", post(merge_refs))
        .route("/api/branches", get(list_branches).post(create_branch))
        // Memory endpoints (high-level)
        .route("/api/memory/remember", post(remember))
        .route("/api/memory/forget", post(forget))
        .route("/api/memory/recall", get(recall))
        .route("/api/memory/context/{project}", get(context))
        .route("/api/memory/prime", post(prime))
        .route("/api/memory/pinned", get(list_pinned))
        .route("/api/memory/summarize_session", post(summarize_session))
        .route("/api/memory/what_changed_since", get(what_changed_since))
        .route("/api/memory/why_did_we", get(why_did_we))
        // Plan endpoints
        .route("/api/plans", get(list_plans).post(create_plan))
        .route("/api/plans/{name}", get(get_plan).delete(delete_plan))
        .route(
            "/api/plans/{name}/tasks",
            get(list_plan_tasks).post(add_plan_task),
        )
        .route("/api/plans/{name}/tasks/{task_id}", get(get_plan_task))
        .route(
            "/api/plans/{name}/tasks/{task_id}/start",
            post(start_plan_task),
        )
        .route(
            "/api/plans/{name}/tasks/{task_id}/complete",
            post(complete_plan_task),
        )
        .route(
            "/api/plans/{name}/tasks/{task_id}/abandon",
            post(abandon_plan_task),
        )
        .route("/api/plans/{name}/next", get(next_plan_task))
        .route("/api/plans/{name}/archive", post(archive_plan))
        // Session turn capture (full request/response/tool/usage JSON)
        .route("/api/sessions/{sid}/turns", get(list_session_turns))
        .route("/api/sessions/{sid}/turns/{idx}", post(put_session_turn).get(get_session_turn))
        // Taint / quarantine / watch
        .route("/api/taint", get(list_taints_handler).post(apply_taint_handler))
        .route("/api/taint/check", get(check_taint_handler))
        .route("/api/taint/{id}", axum::routing::delete(remove_taint_handler))
        .layer(trace)
        .layer(cors)
        .with_state(state);

    // Apply the rate limiter LAST so it runs FIRST in the request
    // lifecycle (tower layers execute in reverse insertion order).
    // tower_governor's GovernorLayer produces axum-native responses on
    // its own — no HandleErrorLayer wrapping needed. When rpm=0, the
    // build_layer call returns None and rate limiting is completely
    // absent, which is what unit tests using `oneshot` rely on since
    // they have no real peer IP to key against.
    if let Some(layer) = governor {
        router = router.layer(layer);
    }

    router
}

// -- Helpers --

/// Map a `RepoError` to an HTTP status. Missing paths / objects /
/// refs become 404; everything else is a 500 (and logged).
fn internal_error(e: agentstategraph::RepoError) -> (StatusCode, String) {
    use agentstategraph::RepoError;
    use agentstategraph::tree::TreeError;
    let msg = e.to_string();
    let status = match &e {
        RepoError::Tree(TreeError::PathNotFound(_))
        | RepoError::Tree(TreeError::ObjectNotFound(_))
        | RepoError::RefNotFound(_)
        | RepoError::BranchNotFound(_) => StatusCode::NOT_FOUND,
        _ => {
            warn!(error = %msg, "request returned 500");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (status, msg)
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

/// `GET /api/stats/tokens` — process-wide aggregate across every session.
///
/// For backward compat this endpoint keeps returning a single
/// snapshot-shaped object. The shape matches `SessionSnapshot`, with
/// `session_id = "_aggregate"` to make the roll-up obvious.
async fn token_stats(State(s): State<HubState>) -> impl IntoResponse {
    // Refresh the default session's flat-size cache. Since graph size
    // is process-global we only need to populate one cache — the
    // aggregate reads the max across sessions.
    let default_session = s.sessions.get_or_create(DEFAULT_SESSION_ID);
    ensure_flat_size(&s.repo, &default_session, "main");
    Json(s.sessions.aggregate())
}

/// `GET /api/stats/tokens/{session_id}` — stats for a specific session.
///
/// Returns 404 if the session hasn't written anything yet. (The
/// registry auto-creates sessions on header read, so once a client
/// has touched the Hub once, its ID is valid here.)
async fn session_token_stats(
    State(s): State<HubState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionSnapshot>, (StatusCode, String)> {
    // Refresh flat-size against the default session so the cache
    // reflects current graph state; the max-across-sessions rule in
    // aggregate() does not help a single-session read.
    let default_session = s.sessions.get_or_create(DEFAULT_SESSION_ID);
    ensure_flat_size(&s.repo, &default_session, "main");

    match s.sessions.snapshot(&session_id) {
        Some(snap) => Ok(Json(snap)),
        None => Err((
            StatusCode::NOT_FOUND,
            format!("session not found: {}", session_id),
        )),
    }
}

/// `GET /api/stats/sessions` — per-session breakdown.
async fn list_sessions(State(s): State<HubState>) -> impl IntoResponse {
    let default_session = s.sessions.get_or_create(DEFAULT_SESSION_ID);
    ensure_flat_size(&s.repo, &default_session, "main");
    Json(s.sessions.snapshot_all())
}

#[derive(Deserialize)]
struct LlmUsageRequest {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    cache_create_tokens: u64,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<String>,
}

/// `POST /api/stats/llm_usage` — record one LLM turn's token usage
/// against the caller's session.
///
/// Agents call this after each significant LLM turn with the numbers
/// copied straight from the provider's `usage` field. The Hub
/// accumulates per-session counters and returns the updated
/// `SessionSnapshot` so callers can see the running totals in one
/// round trip.
///
/// The session is resolved via `X-CTXone-Session` (same mechanism as
/// every other endpoint). Unknown sessions are auto-created.
///
/// Returns **400** when the JSON body fails to deserialize (axum's
/// extractor surfaces this automatically). `input_tokens` and
/// `output_tokens` are required; cache tokens, model, and provider
/// are optional. All numeric fields are `u64`, so negative values
/// are rejected by the JSON parser.
#[instrument(skip_all, fields(session = %session_id.0, model = req.model.as_deref().unwrap_or("")))]
async fn record_llm_usage(
    State(s): State<HubState>,
    session_id: SessionId,
    Json(req): Json<LlmUsageRequest>,
) -> Result<Json<SessionSnapshot>, (StatusCode, String)> {
    let session = s.session_for(&session_id);
    session.record_llm_usage(
        req.input_tokens,
        req.output_tokens,
        req.cache_read_tokens,
        req.cache_create_tokens,
        req.model.clone(),
        req.provider.clone(),
    );

    // Refresh the flat-size cache so the returned snapshot's
    // graph-size fields aren't stale — cheap, one walk at worst.
    ensure_flat_size(&s.repo, &session, "main");

    let snap = SessionSnapshot::from_session(&session_id.0, &session);
    info!(
        input = req.input_tokens,
        output = req.output_tokens,
        cache_read = req.cache_read_tokens,
        cache_create = req.cache_create_tokens,
        session = %session_id.0,
        "llm_usage"
    );
    Ok(Json(snap))
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

#[derive(Deserialize)]
struct CreateBranchRequest {
    name: String,
    #[serde(default = "default_ref")]
    from: String,
}

async fn create_branch(
    State(s): State<HubState>,
    Json(req): Json<CreateBranchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let id = s
        .repo
        .branch(&req.name, &req.from)
        .map_err(internal_error)?;
    Ok(Json(serde_json::json!({
        "status": "ok",
        "name": req.name,
        "from": req.from,
        "commit_id": format!("{}", id.short()),
    })))
}

#[derive(Deserialize)]
struct DiffQuery {
    /// First ref (usually the older / base).
    ref_a: String,
    /// Second ref (usually the newer / target).
    ref_b: String,
}

async fn diff_refs(
    State(s): State<HubState>,
    Query(q): Query<DiffQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ops = s.repo.diff(&q.ref_a, &q.ref_b).map_err(internal_error)?;
    let json_ops = serde_json::to_value(&ops).unwrap_or_default();
    Ok(Json(serde_json::json!({
        "ref_a": q.ref_a,
        "ref_b": q.ref_b,
        "ops": json_ops,
    })))
}

#[derive(Deserialize)]
struct MergeRequest {
    /// Branch with changes to merge from.
    source: String,
    /// Branch to merge into. Defaults to "main".
    #[serde(default = "default_ref")]
    target: String,
    /// Commit message describing the merge.
    #[serde(default = "default_merge_description")]
    description: String,
    /// Optional reasoning for the merge.
    reasoning: Option<String>,
}

fn default_merge_description() -> String {
    "Merge".to_string()
}

#[instrument(skip_all, fields(source = %req.source, target = %req.target, agent = %agent_id.0))]
async fn merge_refs(
    State(s): State<HubState>,
    agent_id: AgentId,
    Json(req): Json<MergeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let mut opts = CommitOptions::new(&agent_id.0, IntentCategory::Merge, &req.description);
    if let Some(r) = req.reasoning {
        opts = opts.with_reasoning(r);
    }

    match s.repo.merge(&req.source, &req.target, opts) {
        Ok(commit_id) => {
            // Graph size may have changed — invalidate every session's cache.
            s.sessions.mark_all_dirty();
            Ok(Json(serde_json::json!({
                "status": "ok",
                "source": req.source,
                "target": req.target,
                "commit_id": format!("{}", commit_id.short()),
            })))
        }
        Err(agentstategraph::RepoError::MergeConflicts(conflicts)) => {
            // Conflicts are a domain-level result, not a 500. Return 409 with details.
            let conflict_json = serde_json::to_value(&conflicts).unwrap_or_default();
            Err((
                StatusCode::CONFLICT,
                serde_json::json!({
                    "status": "conflict",
                    "source": req.source,
                    "target": req.target,
                    "conflicts": conflict_json,
                })
                .to_string(),
            ))
        }
        Err(e) => Err(internal_error(e)),
    }
}

// -- Memory endpoints --

#[derive(Deserialize)]
struct RememberRequest {
    fact: String,
    #[serde(default = "default_importance")]
    importance: String,
    context: Option<String>,
    tags: Option<Vec<String>>,
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

fn default_importance() -> String {
    "medium".to_string()
}

fn default_ref() -> String {
    "main".to_string()
}

#[instrument(
    skip_all,
    fields(
        context = req.context.as_deref().unwrap_or(""),
        importance = %req.importance,
        ref_name = %req.ref_name,
        fact_len = req.fact.len(),
        agent = %agent_id.0,
    )
)]
async fn remember(
    State(s): State<HubState>,
    agent_id: AgentId,
    session_id: SessionId,
    Json(req): Json<RememberRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = match &req.context {
        Some(ctx) => format!("/memory/{}/{}", ctx, timestamp_id()),
        None => format!("/memory/facts/{}", timestamp_id()),
    };

    let confidence = importance_to_confidence(&req.importance);
    let mut opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Custom("Observe".to_string()),
        &req.fact,
    );
    opts = opts.with_confidence(confidence);
    let mut tags = req.tags.unwrap_or_default();
    // Auto-tag with the originating session so Lens can group
    // memories per session without a separate index. Apply this
    // even on DEFAULT_SESSION_ID so the "default" bucket is
    // visible on the Sessions page (otherwise it's a black hole
    // for any caller that didn't set X-CtxOne-Session).
    let stag = format!("session:{}", session_id.0);
    if !tags.iter().any(|t| t == &stag) {
        tags.push(stag);
    }
    if !tags.is_empty() {
        opts = opts.with_tags(tags);
    }

    let value = serde_json::Value::String(req.fact.clone());
    let commit_id = s
        .repo
        .set_json(&req.ref_name, &path, &value, opts)
        .map_err(internal_error)?;

    s.sessions.mark_all_dirty();

    Ok(Json(serde_json::json!({
        "status": "ok",
        "ref": req.ref_name,
        "path": path,
        "commit_id": format!("{}", commit_id.short()),
    })))
}

#[derive(Deserialize)]
struct ForgetRequest {
    /// Exact path to forget (e.g., /memory/licensing/abc).
    path: String,
    /// Why this is being forgotten. Shows up in blame.
    #[serde(default = "default_forget_reason")]
    reason: String,
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

fn default_forget_reason() -> String {
    "forgotten by user".to_string()
}

#[instrument(skip_all, fields(path = %req.path, ref_name = %req.ref_name, agent = %agent_id.0))]
async fn forget(
    State(s): State<HubState>,
    agent_id: AgentId,
    Json(req): Json<ForgetRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let opts = CommitOptions::new(&agent_id.0, IntentCategory::Rollback, &req.reason);

    let commit_id = s
        .repo
        .delete(&req.ref_name, &req.path, opts)
        .map_err(internal_error)?;

    s.sessions.mark_all_dirty();

    Ok(Json(serde_json::json!({
        "status": "ok",
        "ref": req.ref_name,
        "path": req.path,
        "commit_id": format!("{}", commit_id.short()),
    })))
}

#[derive(Deserialize)]
struct RecallQuery {
    topic: String,
    budget: Option<usize>,
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

#[instrument(skip_all, fields(ref_name = %q.ref_name, budget = q.budget.unwrap_or(1500), session = %session_id.0))]
async fn recall(
    State(s): State<HubState>,
    session_id: SessionId,
    Query(q): Query<RecallQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let budget = q.budget.unwrap_or(1500);
    let session = s.session_for(&session_id);
    let result = run_recall(&s.repo, &session, &q.topic, budget, &q.ref_name);
    // Log savings inline — at info level this gives one line per recall
    // showing the topic and the ratio. Useful for seeing the memory layer
    // earning its keep in real time.
    let tokens_sent = result
        .get("ctx_tokens_sent")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let ratio = result
        .get("ctx_savings_ratio")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    info!(
        topic = %q.topic,
        tokens_sent,
        ratio = format!("{:.1}x", ratio),
        session_id = %session_id.0,
        "recall"
    );
    Ok(Json(result))
}

#[derive(Deserialize)]
struct ContextQuery {
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

async fn context(
    State(s): State<HubState>,
    session_id: SessionId,
    Path(project): Path<String>,
    Query(q): Query<ContextQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = format!("/memory/projects/{}", project);
    match s.repo.get_json(&q.ref_name, &path) {
        Ok(value) => {
            let session = s.session_for(&session_id);
            ensure_flat_size(&s.repo, &session, &q.ref_name);
            let flat_size = session.total_graph_size_chars.load(Ordering::Relaxed) as usize;
            let sent = serde_json::to_string(&value).unwrap_or_default().len();
            session.record(sent, flat_size);
            Ok(Json(serde_json::json!({
                "project": project,
                "ref": q.ref_name,
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
    agent_id: AgentId,
    Json(req): Json<SummarizeSessionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let summary = req.key_points.join(". ");
    let summary_opts = CommitOptions::new(
        &agent_id.0,
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
            &agent_id.0,
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
    /// Branch to write to (default: "main").
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

#[instrument(
    skip_all,
    fields(
        source = %req.source,
        pinned = req.pinned,
        ref_name = %req.ref_name,
        sections = req.sections.len(),
    )
)]
async fn prime(
    State(s): State<HubState>,
    session_id: SessionId,
    agent_id: AgentId,
    Json(req): Json<PrimeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let sections: Vec<(String, String)> = req
        .sections
        .into_iter()
        .map(|s| (s.title, s.body))
        .collect();
    debug!(count = sections.len(), agent = %agent_id.0, "priming sections");

    let session = s.session_for(&session_id);
    let result = run_prime(
        &s.repo,
        &session,
        &agent_id.0,
        &req.source,
        req.pinned,
        &sections,
        &req.ref_name,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Priming changes the graph, so every session's cached
    // flat-size is now stale.
    s.sessions.mark_all_dirty();
    Ok(Json(result))
}

async fn list_pinned(
    State(s): State<HubState>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    // If /memory/pinned doesn't exist yet, return an empty list instead of 500.
    let paths = s
        .repo
        .list_paths("main", "/memory/pinned", Some(20))
        .unwrap_or_default();

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

// -- Plan endpoints --------------------------------------------------
//
// Thin wrappers around `plan_tools::*` + `TaskStore` calls. Each
// endpoint honors `X-CTXone-Agent` for blame attribution and
// `X-CTXone-Session` for stats accounting.

#[derive(Deserialize)]
struct RefQuery {
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

#[derive(Deserialize)]
struct PlanListQuery {
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize)]
struct CreatePlanRequest {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

#[derive(Deserialize)]
struct CreateTaskRequest {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    assigned_to: Option<String>,
    #[serde(default)]
    blocked_by: Vec<String>,
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

#[derive(Deserialize)]
struct StartTaskRequest {
    #[serde(default)]
    reason: Option<String>,
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

#[derive(Deserialize)]
struct CompleteTaskRequest {
    proof: plan_tools::ProofParam,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

#[derive(Deserialize)]
struct AbandonTaskRequest {
    reason: String,
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

#[derive(Deserialize)]
struct NextTaskQuery {
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
    #[serde(default)]
    assigned_to: Option<String>,
    #[serde(default = "default_true")]
    include_unassigned: bool,
    #[serde(default)]
    assigned_only: bool,
}

fn default_true() -> bool {
    true
}

fn plan_error_to_response(err: plan_tools::PlanToolError) -> (StatusCode, String) {
    use agentstategraph_tasks::TaskStoreError as TE;
    let status = match &err {
        plan_tools::PlanToolError::Substrate(TE::PlanNotFound(_)) => StatusCode::NOT_FOUND,
        plan_tools::PlanToolError::Substrate(TE::TaskNotFound { .. }) => StatusCode::NOT_FOUND,
        plan_tools::PlanToolError::Substrate(TE::PlanAlreadyExists(_)) => StatusCode::CONFLICT,
        plan_tools::PlanToolError::Substrate(TE::Blocked { .. }) => StatusCode::CONFLICT,
        plan_tools::PlanToolError::Substrate(TE::BlockerNotFound { .. }) => StatusCode::CONFLICT,
        plan_tools::PlanToolError::Substrate(TE::InvalidTransition { .. }) => StatusCode::CONFLICT,
        plan_tools::PlanToolError::Substrate(TE::ProofRequired) => StatusCode::BAD_REQUEST,
        plan_tools::PlanToolError::Substrate(TE::ReasonRequired) => StatusCode::BAD_REQUEST,
        plan_tools::PlanToolError::Substrate(TE::ParentNotFound(_)) => StatusCode::NOT_FOUND,
        plan_tools::PlanToolError::Substrate(TE::ParentIsSubtask(_)) => StatusCode::BAD_REQUEST,
        plan_tools::PlanToolError::InvalidProof(_) => StatusCode::BAD_REQUEST,
        plan_tools::PlanToolError::InvalidPriority(_) => StatusCode::BAD_REQUEST,
        plan_tools::PlanToolError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        plan_tools::PlanToolError::Substrate(TE::InvalidBlockerId(_)) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, err.to_string())
}

fn substrate_error_to_response(err: agentstategraph_tasks::TaskStoreError) -> (StatusCode, String) {
    plan_error_to_response(plan_tools::PlanToolError::Substrate(err))
}

#[instrument(skip_all, fields(ref_name = %q.ref_name, status = q.status.as_deref().unwrap_or("")))]
async fn list_plans(
    State(s): State<HubState>,
    Query(q): Query<PlanListQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let store = plan_tools::make_store(s.repo.clone(), DEFAULT_AGENT_ID);
    let filter = q
        .status
        .as_deref()
        .and_then(plan_tools::plan_status_from_str);
    let plans = store
        .list_plans_by_status(&q.ref_name, filter)
        .map_err(substrate_error_to_response)?;
    let mut out = Vec::new();
    for plan in plans {
        let tasks = store
            .list_tasks(&q.ref_name, &plan.name)
            .unwrap_or_default();
        out.push(plan_tools::plan_to_json(&plan, &tasks, false));
    }
    Ok(Json(out))
}

#[instrument(skip_all, fields(name = %req.name, ref_name = %req.ref_name, agent = %agent_id.0))]
async fn create_plan(
    State(s): State<HubState>,
    agent_id: AgentId,
    Json(req): Json<CreatePlanRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    let store = plan_tools::make_store(s.repo.clone(), &agent_id.0);
    let plan = plan_tools::create_plan(&store, &req.ref_name, &req.name, req.description)
        .map_err(plan_error_to_response)?;
    s.sessions.mark_all_dirty();
    let body = plan_tools::plan_to_json(&plan, &[], false);
    Ok((StatusCode::CREATED, Json(body)))
}

#[instrument(skip_all, fields(name = %name, ref_name = %q.ref_name))]
async fn get_plan(
    State(s): State<HubState>,
    Path(name): Path<String>,
    Query(q): Query<RefQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = plan_tools::make_store(s.repo.clone(), DEFAULT_AGENT_ID);
    let plan = store
        .get_plan(&q.ref_name, &name)
        .map_err(substrate_error_to_response)?;
    let tasks = store.list_tasks(&q.ref_name, &name).unwrap_or_default();
    Ok(Json(plan_tools::plan_to_json(&plan, &tasks, true)))
}

#[instrument(skip_all, fields(name = %name, ref_name = %q.ref_name, agent = %agent_id.0))]
async fn delete_plan(
    State(s): State<HubState>,
    agent_id: AgentId,
    Path(name): Path<String>,
    Query(q): Query<RefQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = plan_tools::make_store(s.repo.clone(), &agent_id.0);
    store
        .delete_plan(&q.ref_name, &name)
        .map_err(substrate_error_to_response)?;
    s.sessions.mark_all_dirty();
    Ok(Json(serde_json::json!({
        "status": "ok",
        "name": name,
        "ref": q.ref_name,
    })))
}

#[instrument(skip_all, fields(name = %name, ref_name = %q.ref_name))]
async fn list_plan_tasks(
    State(s): State<HubState>,
    Path(name): Path<String>,
    Query(q): Query<RefQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let store = plan_tools::make_store(s.repo.clone(), DEFAULT_AGENT_ID);
    let tasks = store
        .list_tasks(&q.ref_name, &name)
        .map_err(substrate_error_to_response)?;
    let out: Vec<serde_json::Value> = tasks.iter().map(plan_tools::task_to_json).collect();
    Ok(Json(out))
}

#[instrument(skip_all, fields(name = %name, title = %req.title, agent = %agent_id.0))]
async fn add_plan_task(
    State(s): State<HubState>,
    agent_id: AgentId,
    Path(name): Path<String>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    let store = plan_tools::make_store(s.repo.clone(), &agent_id.0);
    let task = plan_tools::add_task(
        &store,
        &req.ref_name,
        &name,
        &req.title,
        req.description.as_deref(),
        req.priority.as_deref(),
        req.parent_id.as_deref(),
        req.assigned_to.as_deref(),
        req.blocked_by,
    )
    .map_err(plan_error_to_response)?;
    s.sessions.mark_all_dirty();
    Ok((StatusCode::CREATED, Json(plan_tools::task_to_json(&task))))
}

#[instrument(skip_all, fields(name = %name, task_id = %task_id, ref_name = %q.ref_name))]
async fn get_plan_task(
    State(s): State<HubState>,
    Path((name, task_id)): Path<(String, String)>,
    Query(q): Query<RefQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use agentstategraph_tasks::TaskId;
    let store = plan_tools::make_store(s.repo.clone(), DEFAULT_AGENT_ID);
    let task = store
        .get_task(&q.ref_name, &name, &TaskId(task_id))
        .map_err(substrate_error_to_response)?;
    Ok(Json(plan_tools::task_to_json(&task)))
}

#[instrument(skip_all, fields(name = %name, task_id = %task_id, agent = %agent_id.0))]
async fn start_plan_task(
    State(s): State<HubState>,
    agent_id: AgentId,
    Path((name, task_id)): Path<(String, String)>,
    Json(req): Json<StartTaskRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use agentstategraph_tasks::TaskId;
    let _ = req.reason; // reserved for future richer blame (annotation)
    let store = plan_tools::make_store(s.repo.clone(), &agent_id.0);
    let task = store
        .start_task(&req.ref_name, &name, &TaskId(task_id))
        .map_err(substrate_error_to_response)?;
    s.sessions.mark_all_dirty();
    Ok(Json(plan_tools::task_to_json(&task)))
}

#[instrument(skip_all, fields(name = %name, task_id = %task_id, agent = %agent_id.0))]
async fn complete_plan_task(
    State(s): State<HubState>,
    agent_id: AgentId,
    Path((name, task_id)): Path<(String, String)>,
    Json(req): Json<CompleteTaskRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use agentstategraph_tasks::TaskId;
    let _ = req.reason;
    let proof = plan_tools::parse_proof(&req.proof.kind, &req.proof.value, req.proof.note)
        .map_err(plan_error_to_response)?;
    let store = plan_tools::make_store(s.repo.clone(), &agent_id.0);
    let task = store
        .complete_task(&req.ref_name, &name, &TaskId(task_id), proof)
        .map_err(substrate_error_to_response)?;
    s.sessions.mark_all_dirty();
    Ok(Json(plan_tools::task_to_json(&task)))
}

#[instrument(skip_all, fields(name = %name, task_id = %task_id, agent = %agent_id.0))]
async fn abandon_plan_task(
    State(s): State<HubState>,
    agent_id: AgentId,
    Path((name, task_id)): Path<(String, String)>,
    Json(req): Json<AbandonTaskRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use agentstategraph_tasks::TaskId;
    let store = plan_tools::make_store(s.repo.clone(), &agent_id.0);
    let task = store
        .abandon_task(&req.ref_name, &name, &TaskId(task_id), &req.reason)
        .map_err(substrate_error_to_response)?;
    s.sessions.mark_all_dirty();
    Ok(Json(plan_tools::task_to_json(&task)))
}

#[instrument(skip_all, fields(name = %name, ref_name = %q.ref_name, assigned_to = q.assigned_to.as_deref().unwrap_or("")))]
async fn next_plan_task(
    State(s): State<HubState>,
    agent_id: AgentId,
    Path(name): Path<String>,
    Query(q): Query<NextTaskQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = plan_tools::make_store(s.repo.clone(), DEFAULT_AGENT_ID);
    let assignee = match q.assigned_to.as_deref() {
        Some("me") => Some(agent_id.0.clone()),
        Some(x) if !x.is_empty() => Some(x.to_string()),
        _ => None,
    };
    // Substrate's next_task_for takes a single include_unassigned flag.
    // Preserve CTXone's historical semantics: assigned_only=true forces
    // unassigned tasks out regardless of include_unassigned.
    let include_unassigned = q.include_unassigned && !q.assigned_only;
    let task = store
        .next_task_for(&q.ref_name, &name, assignee.as_deref(), include_unassigned)
        .map_err(substrate_error_to_response)?;
    let body = match task {
        None => serde_json::json!({ "task": null }),
        Some(t) => serde_json::json!({ "task": plan_tools::task_to_json(&t) }),
    };
    Ok(Json(body))
}

#[instrument(skip_all, fields(name = %name, ref_name = %q.ref_name, agent = %agent_id.0))]
async fn archive_plan(
    State(s): State<HubState>,
    agent_id: AgentId,
    Path(name): Path<String>,
    Query(q): Query<RefQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = plan_tools::make_store(s.repo.clone(), &agent_id.0);
    let plan = store
        .archive_plan(&q.ref_name, &name)
        .map_err(substrate_error_to_response)?;
    s.sessions.mark_all_dirty();
    Ok(Json(plan_tools::plan_to_json(&plan, &[], false)))
}

// -- Taint endpoints --

#[derive(Deserialize)]
struct ListTaintsQuery {
    path_prefix: Option<String>,
    kind: Option<String>,
    #[serde(default)]
    include_resolved: bool,
}

#[derive(Serialize)]
struct ListTaintsResponse {
    taints: Vec<agentstategraph_taint::Taint>,
}

async fn list_taints_handler(
    State(s): State<HubState>,
    Query(q): Query<ListTaintsQuery>,
) -> Result<Json<ListTaintsResponse>, (StatusCode, String)> {
    let kind = parse_kind(q.kind.as_deref())?;
    let taints = s
        .repo
        .list_taints(q.path_prefix.as_deref(), kind, q.include_resolved)
        .map_err(internal_error)?;
    Ok(Json(ListTaintsResponse { taints }))
}

#[derive(Deserialize)]
struct CheckTaintQuery {
    path: String,
    agent_id: String,
    #[serde(default = "one")]
    confidence: f64,
}

fn one() -> f64 {
    1.0
}

#[derive(Serialize)]
struct CheckTaintResponse {
    can_write: bool,
    isolated: bool,
    required_confidence: f64,
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    matching_taint_id: Option<String>,
}

async fn check_taint_handler(
    State(s): State<HubState>,
    Query(q): Query<CheckTaintQuery>,
) -> Result<Json<CheckTaintResponse>, (StatusCode, String)> {
    let check = s
        .repo
        .check_taint(&q.path, &q.agent_id, q.confidence)
        .map_err(internal_error)?;
    let warnings: Vec<String> = check
        .taints
        .iter()
        .filter(|t| {
            matches!(
                t.effect,
                agentstategraph_taint::TaintEffect::Warn
                    | agentstategraph_taint::TaintEffect::Isolate
            )
        })
        .map(|t| format!("{}: {}", t.name, t.reason))
        .collect();
    let blocking = check
        .quarantines
        .iter()
        .find(|q| !q.authorized_agents().iter().any(|a| a == &q.agent_id))
        .or_else(|| {
            check.taints.iter().find(|t| {
                matches!(
                    t.effect,
                    agentstategraph_taint::TaintEffect::Block
                        | agentstategraph_taint::TaintEffect::Review
                )
            })
        });
    let (effect, matching_taint_id) = match blocking {
        Some(t) => (Some(taint_effect_str(t.effect).to_string()), Some(t.id.clone())),
        None => (None, None),
    };
    Ok(Json(CheckTaintResponse {
        can_write: check.can_write,
        isolated: check.isolated,
        required_confidence: check.required_confidence,
        warnings,
        effect,
        matching_taint_id,
    }))
}

#[derive(Deserialize)]
struct ApplyTaintBody {
    path: String,
    name: String,
    kind: String,
    effect: String,
    #[serde(default)]
    severity: Option<String>,
    reason: String,
    agent_id: String,
    #[serde(default)]
    ref_name: Option<String>,
    #[serde(default)]
    authorized_agents: Option<Vec<String>>,
}

#[derive(Serialize)]
struct ApplyTaintResponse {
    taint_id: String,
    path: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn apply_taint_handler(
    State(s): State<HubState>,
    Json(body): Json<ApplyTaintBody>,
) -> Result<Json<ApplyTaintResponse>, (StatusCode, String)> {
    use agentstategraph_taint::{
        QuarantineParams, TaintKind, TaintParams, WatchParams,
    };
    let kind = parse_kind(Some(&body.kind))?
        .ok_or((StatusCode::BAD_REQUEST, "kind required".to_string()))?;
    let severity = parse_severity(body.severity.as_deref())?;
    let ref_name = body.ref_name.unwrap_or_else(|| "main".to_string());
    let now = chrono::Utc::now();

    let taint_id = match kind {
        TaintKind::Taint => {
            let effect = parse_effect(&body.effect)?;
            s.repo.taint(
                &ref_name,
                &body.path,
                TaintParams {
                    name: body.name,
                    effect,
                    reason: body.reason,
                    severity,
                    expires_at: None,
                    propagate: true,
                    metadata: Default::default(),
                    agent_id: body.agent_id,
                },
            )
        }
        TaintKind::Quarantine => s.repo.quarantine(
            &ref_name,
            &body.path,
            QuarantineParams {
                name: body.name,
                reason: body.reason,
                severity,
                authorized_agents: body.authorized_agents.unwrap_or_default(),
                expires_at: None,
                propagate: true,
                agent_id: body.agent_id,
            },
        ),
        TaintKind::Watch => s.repo.watch_path(
            &ref_name,
            &body.path,
            WatchParams {
                name: body.name,
                reason: body.reason,
                metric: None,
                threshold: None,
                direction: Default::default(),
                check_interval_secs: None,
                expires_at: None,
                severity,
                propagate: true,
                agent_id: body.agent_id,
            },
        ),
    }
    .map_err(internal_error)?;

    Ok(Json(ApplyTaintResponse {
        taint_id,
        path: body.path,
        created_at: now,
    }))
}

#[derive(Deserialize)]
struct RemoveTaintBody {
    reason: String,
    agent_id: String,
    #[serde(default)]
    ref_name: Option<String>,
}

#[derive(Serialize)]
struct RemoveTaintResponse {
    resolved_at: chrono::DateTime<chrono::Utc>,
}

async fn remove_taint_handler(
    State(s): State<HubState>,
    Path(id): Path<String>,
    Json(body): Json<RemoveTaintBody>,
) -> Result<Json<RemoveTaintResponse>, (StatusCode, String)> {
    use agentstategraph_taint::{TaintKind, UntaintParams, UnwatchParams};
    let taint = s
        .repo
        .get_taint(&id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, format!("taint not found: {id}")))?;
    let ref_name = body.ref_name.unwrap_or_else(|| "main".to_string());

    match taint.kind {
        TaintKind::Taint => s.repo.untaint(
            &ref_name,
            &taint.path,
            &taint.name,
            UntaintParams {
                reason: body.reason,
                proof: None,
                agent_id: body.agent_id,
            },
        ),
        TaintKind::Quarantine => s.repo.unquarantine(
            &ref_name,
            &taint.path,
            &taint.name,
            UntaintParams {
                reason: body.reason,
                proof: None,
                agent_id: body.agent_id,
            },
        ),
        TaintKind::Watch => s.repo.unwatch(
            &ref_name,
            &taint.path,
            &taint.name,
            UnwatchParams {
                reason: Some(body.reason),
                agent_id: body.agent_id,
            },
        ),
    }
    .map_err(internal_error)?;

    Ok(Json(RemoveTaintResponse {
        resolved_at: chrono::Utc::now(),
    }))
}

fn parse_kind(
    s: Option<&str>,
) -> Result<Option<agentstategraph_taint::TaintKind>, (StatusCode, String)> {
    use agentstategraph_taint::TaintKind;
    match s {
        None | Some("") => Ok(None),
        Some("taint") => Ok(Some(TaintKind::Taint)),
        Some("quarantine") => Ok(Some(TaintKind::Quarantine)),
        Some("watch") => Ok(Some(TaintKind::Watch)),
        Some(other) => Err((
            StatusCode::BAD_REQUEST,
            format!("invalid kind: {other}"),
        )),
    }
}

fn parse_effect(s: &str) -> Result<agentstategraph_taint::TaintEffect, (StatusCode, String)> {
    use agentstategraph_taint::TaintEffect;
    match s {
        "warn" => Ok(TaintEffect::Warn),
        "block" => Ok(TaintEffect::Block),
        "review" => Ok(TaintEffect::Review),
        "isolate" => Ok(TaintEffect::Isolate),
        "advisory" => Ok(TaintEffect::Advisory),
        other => Err((
            StatusCode::BAD_REQUEST,
            format!("invalid effect: {other}"),
        )),
    }
}

fn parse_severity(
    s: Option<&str>,
) -> Result<agentstategraph_taint::TaintSeverity, (StatusCode, String)> {
    use agentstategraph_taint::TaintSeverity;
    match s {
        None | Some("") | Some("medium") => Ok(TaintSeverity::Medium),
        Some("low") => Ok(TaintSeverity::Low),
        Some("high") => Ok(TaintSeverity::High),
        Some("critical") => Ok(TaintSeverity::Critical),
        Some(other) => Err((
            StatusCode::BAD_REQUEST,
            format!("invalid severity: {other}"),
        )),
    }
}

fn taint_effect_str(e: agentstategraph_taint::TaintEffect) -> &'static str {
    use agentstategraph_taint::TaintEffect;
    match e {
        TaintEffect::Warn => "warn",
        TaintEffect::Block => "block",
        TaintEffect::Review => "review",
        TaintEffect::Isolate => "isolate",
        TaintEffect::Advisory => "advisory",
    }
}

// ── Session turn capture ────────────────────────────────────────────────────
//
// These endpoints persist the full per-turn payload (request + response +
// tool calls + token usage) at a deterministic path on the requested ref,
// so re-ingesting the same JSONL is idempotent.

const SESSION_TURNS_MAX_BYTES: usize = 1 * 1024 * 1024; // 1 MiB

fn session_turn_path(sid: &str, idx: u32) -> String {
    // Prefix with `t` so the segment is a map Key, not a list Index — ASG
    // path parsing turns all-digit segments into list indexes which can't
    // create-on-write under an empty parent.
    format!("/sessions/{}/turns/t{:04}", sid, idx)
}

#[derive(Deserialize)]
struct SessionTurnQuery {
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

async fn put_session_turn(
    State(s): State<HubState>,
    Path((sid, idx)): Path<(String, u32)>,
    agent_id: AgentId,
    Query(q): Query<SessionTurnQuery>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let bytes = serde_json::to_vec(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    if bytes.len() > SESSION_TURNS_MAX_BYTES {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, format!(
            "turn payload {} bytes exceeds {} byte cap",
            bytes.len(), SESSION_TURNS_MAX_BYTES
        )));
    }
    let path = session_turn_path(&sid, idx);
    let intent = format!("capture session {} turn {:04}", sid, idx);
    let opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Custom("Observe".to_string()),
        &intent,
    )
    .with_tags(vec![
        format!("session:{}", sid),
        format!("turn:{}", idx),
        "kind:full-turn".to_string(),
    ]);
    let commit_id = s
        .repo
        .set_json(&q.ref_name, &path, &body, opts)
        .map_err(internal_error)?;
    Ok(Json(serde_json::json!({
        "status": "ok",
        "ref": q.ref_name,
        "path": path,
        "commit_id": format!("{}", commit_id.short()),
    })))
}

async fn get_session_turn(
    State(s): State<HubState>,
    Path((sid, idx)): Path<(String, u32)>,
    Query(q): Query<SessionTurnQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = session_turn_path(&sid, idx);
    s.repo.get_json(&q.ref_name, &path).map(Json).map_err(internal_error)
}

async fn list_session_turns(
    State(s): State<HubState>,
    Path(sid): Path<String>,
    Query(q): Query<SessionTurnQuery>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let prefix = format!("/sessions/{}/turns", sid);
    let all = s
        .repo
        .list_paths(&q.ref_name, &prefix, None)
        .map_err(internal_error)?;
    // Collapse leaf paths down to one entry per turn root
    // (e.g. /sessions/X/turns/t0000/...).
    let prefix_with_slash = format!("{}/", prefix);
    let mut roots: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for p in all {
        if let Some(rest) = p.strip_prefix(&prefix_with_slash) {
            if let Some((first, _)) = rest.split_once('/') {
                roots.insert(format!("{}{}", prefix_with_slash, first));
            } else {
                roots.insert(format!("{}{}", prefix_with_slash, rest));
            }
        }
    }
    Ok(Json(roots.into_iter().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importance_high_maps_to_high_confidence() {
        assert_eq!(importance_to_confidence("high"), 0.95);
    }

    #[test]
    fn importance_medium_maps_to_default_confidence() {
        assert_eq!(importance_to_confidence("medium"), 0.7);
    }

    #[test]
    fn importance_low_maps_to_low_confidence() {
        assert_eq!(importance_to_confidence("low"), 0.4);
    }

    #[test]
    fn importance_unknown_falls_back_to_medium() {
        // An unrecognized importance should not panic or return zero —
        // it defaults to the "medium" confidence so callers can pass
        // any string safely.
        assert_eq!(importance_to_confidence("super-critical"), 0.7);
        assert_eq!(importance_to_confidence(""), 0.7);
    }

    #[test]
    fn timestamp_id_is_nonempty_hex() {
        let id = timestamp_id();
        assert!(!id.is_empty());
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "timestamp_id should be hex, got: {}",
            id
        );
    }

    #[test]
    fn timestamp_id_is_unique_across_calls() {
        // Back-to-back calls should not collide at nanosecond resolution.
        // (If two calls land on the same nanosecond we'd be in trouble —
        // the whole point of this ID is uniqueness.)
        let a = timestamp_id();
        // Tiny spin to guarantee clock advances on coarse-clock platforms
        for _ in 0..1000 {
            std::hint::black_box(());
        }
        let b = timestamp_id();
        assert_ne!(a, b, "two calls produced the same id: {}", a);
    }

    #[test]
    fn default_ref_is_main() {
        assert_eq!(default_ref(), "main");
    }

    #[test]
    fn default_merge_description_is_stable() {
        assert_eq!(default_merge_description(), "Merge");
    }

    #[test]
    fn default_forget_reason_is_human_readable() {
        // Not matching exact text — just checking we produce something
        // non-empty that a user would understand in a blame view.
        let r = default_forget_reason();
        assert!(!r.is_empty());
        assert!(r.contains("forgotten") || r.contains("forget"));
    }
}
