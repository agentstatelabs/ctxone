//! HTTP REST API for CtxOne Hub.
//!
//! Exposes:
//!   - Basic read endpoints that the Lens web UI needs (health, stats, state, log, search)
//!   - Memory-oriented write endpoints matching the MCP tools (remember, recall, context, etc.)
//!   - Token savings endpoint: GET /api/stats/tokens

use std::sync::Arc;
use std::sync::atomic::Ordering;

use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    middleware::Next,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{debug, info, instrument, warn};

use agentstategraph::{CommitOptions, Repository};
use agentstategraph_core::{IntentCategory, Namespace};

use crate::asd_pool::AsdProcessPool;
use crate::memory_tools::{
    DEFAULT_AGENT_ID, DEFAULT_SESSION_ID, SessionRegistry, SessionSnapshot, SessionStats,
    ensure_flat_size, run_prime, run_recall,
};
use crate::plan_tools;
use crate::rate_limit;
use crate::reminder_tools;
use crate::session_links;

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
    /// Named ASD repos with pre-known base URLs: (name, base_url) pairs.
    /// Each entry exposes GET /api/code/{name}/* proxied to <base_url>/api/v1/*.
    /// Parsed from repeated --asd-url name=http://... flags.
    pub asd_repos: Vec<(String, String)>,
    /// Named ASD repos managed by the process pool: (name, db_path) pairs.
    /// The hub spawns `asd-serve` on demand for each repo and kills it after
    /// idle timeout.  Parsed from repeated --asd-repo name=/path/db flags.
    pub asd_pool_repos: Vec<(String, String)>,
    /// Override path to the `asd-serve` binary. `None` → use PATH.
    pub asd_serve_binary: Option<String>,
    /// Idle timeout (seconds) before the pool kills a spawned asd-serve child.
    /// `None` → AsdProcessPool default.
    pub asd_idle_timeout_secs: Option<u64>,
    /// When true, mount the MCP tool surface at `/mcp` (Streamable HTTP) so a
    /// single daemon serves MCP + REST + Lens. Off by default so unit tests
    /// and library callers build a plain REST router. See [`crate::mcp_http`].
    pub mcp_http: bool,
    /// Agent id stamped on commits made through the `/mcp` surface (parity with
    /// `ctxone-hub --agent-id`). Only read when `mcp_http` is true.
    pub agent_id: String,
    /// Optional bearer token guarding the whole HTTP surface (REST + `/mcp`).
    /// When `Some`, non-loopback requests must send `Authorization: Bearer
    /// <token>`; loopback peers are always exempt. When `None`, no per-request
    /// auth is enforced (the binary warns at startup if bound non-loopback).
    /// See [`crate::mcp_http`] and the auth middleware in this module.
    pub auth_token: Option<String>,
    /// Extra browser origins allowed to call the API (beyond same-origin, which
    /// is always allowed). A request carrying an `Origin` header that is neither
    /// same-origin nor in this list is rejected (CSRF/DNS-rebinding guard), and
    /// only these origins get CORS response headers. Non-browser clients (CLI,
    /// native MCP) send no `Origin` and are unaffected. From `--allowed-origin`.
    pub allowed_origins: Vec<String>,
    /// Override path to the `ctx` CLI binary used by `POST /api/sessions/sync`
    /// to re-ingest local Claude Code transcripts. `None` → resolve `"ctx"` on
    /// PATH. From `--ctx-binary`.
    pub ctx_binary: Option<String>,
    /// The hub's own loopback base URL (e.g. `http://127.0.0.1:3001`), passed to
    /// the spawned `ctx ingest-session --all` as `--server` so it ingests back
    /// into this hub. `None` on library/test callers (session-sync then targets
    /// the default port). Built from the bind addr by the binary.
    pub self_base_url: Option<String>,
}

#[derive(Clone)]
pub struct HubState {
    pub repo: Arc<Repository>,
    pub sessions: Arc<SessionRegistry>,
    /// Path to the live sqlite db file, or None for memory/postgres.
    pub db_path: Option<String>,
    /// Named ASD repos with pre-known base URLs (static, no pool).
    pub asd_repos: Arc<Vec<(String, String)>>,
    /// Process pool for dynamically spawned `asd-serve` instances.
    pub asd_pool: Option<Arc<AsdProcessPool>>,
    /// Path to the `ctx` CLI binary for `POST /api/sessions/sync`. `None` → PATH.
    pub ctx_binary: Option<String>,
    /// The hub's own loopback base URL, passed to `ctx ingest-session --all`.
    pub self_base_url: Option<String>,
}

impl HubState {
    /// Resolve the session for this request. Always returns a valid
    /// `Arc<SessionStats>` — if the session didn't exist, it's
    /// created on the fly.
    fn session_for(&self, id: &SessionId) -> Arc<SessionStats> {
        self.sessions.get_or_create(&id.0)
    }

    /// Repository scoped to the request's namespace. `"default"` returns
    /// the base repo; anything else forks a sibling Repository sharing the
    /// same storage but keyed on `(namespace, branch)` at the ref layer.
    /// Forking is cheap (no data copy). The namespace must already exist —
    /// ref operations in an unknown namespace surface as 404 via
    /// [`internal_error`]'s `NamespaceNotFound` mapping.
    fn repo_for(&self, ns: &NamespaceId) -> Result<Arc<Repository>, (StatusCode, String)> {
        if ns.0 == Namespace::DEFAULT {
            return Ok(self.repo.clone());
        }
        let namespace =
            Namespace::new(ns.0.as_str()).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        Ok(Arc::new(self.repo.fork_namespace(namespace)))
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

/// Extractor for the namespace this request operates in. Resolution
/// order: `?namespace=` query parameter (explicit in the URL wins),
/// then the `X-CTXone-Namespace` header, then `"default"`. Namespaces
/// are created by registering a project (`POST /api/projects`) — ref
/// operations in a namespace that doesn't exist return 404.
#[derive(Debug, Clone)]
pub struct NamespaceId(pub String);

impl<S> axum::extract::FromRequestParts<S> for NamespaceId
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        // Valid namespace names are ASCII [A-Za-z0-9_-], so no
        // percent-decoding is needed; an encoded (thus invalid) name
        // fails Namespace::new with a 400 downstream.
        // Emptiness is filtered per source, NOT once at the end: a bare
        // `?namespace=` used to win the `or_else` as `Some("")` and then get
        // filtered away, so a malformed query silently suppressed a perfectly
        // good header and the request fell through to `default`.
        let from_query = parts
            .uri
            .query()
            .and_then(|q| {
                q.split('&').find_map(|pair| {
                    let (k, v) = pair.split_once('=')?;
                    (k == "namespace").then(|| v.trim().to_string())
                })
            })
            .filter(|s| !s.is_empty());
        let explicit = from_query.or_else(|| {
            parts
                .headers
                .get("x-ctxone-namespace")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });

        match explicit {
            Some(id) => Ok(NamespaceId(id)),
            None => {
                // This extractor only runs for namespace-scoped handlers, so
                // an absent namespace here means a caller is operating on a
                // workspace it never named. Silently answering "default" is
                // how sessions from ~40 repos ended up in one workspace, and
                // how a ranking got rendered under the wrong branch. Falling
                // back is still the compatible behaviour, but it is no longer
                // a secret.
                warn_implicit_namespace(parts.uri.path());
                Ok(NamespaceId(Namespace::DEFAULT.to_string()))
            }
        }
    }
}

/// Warn once per path that a namespace-scoped request arrived without one.
///
/// Deduped because a polling client would otherwise bury the log in identical
/// lines; the first occurrence per path is what tells you which caller still
/// needs updating.
fn warn_implicit_namespace(path: &str) {
    if first_time_for_path(path) {
        tracing::warn!(
            path,
            "request has no namespace; assuming 'default'. Send X-CTXone-Namespace \
             (or ?namespace=) to name the workspace explicitly."
        );
    }
}

/// True the first time `path` is seen this process.
///
/// Split out from the warning so the dedup can be tested without capturing
/// log output.
fn first_time_for_path(path: &str) -> bool {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    match seen.lock() {
        Ok(mut s) => s.insert(path.to_string()),
        // A poisoned lock must not take the request down; warning twice is
        // strictly better than failing here.
        Err(_) => true,
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

/// Build the Hub router and attach a sqlite db path. Used by main()
/// when storage is sqlite — the path is what `/api/admin/backup`
/// VACUUMs INTO. Memory/postgres builds use the path-less variants.
pub fn router_with_db_path(
    repo: Arc<Repository>,
    sessions: Arc<SessionRegistry>,
    config: HubConfig,
    db_path: Option<String>,
    with_lens: bool,
) -> Router {
    let mut router = router_with_config_inner(repo, sessions, config, db_path);
    if with_lens {
        router = router.fallback(crate::lens::lens_handler);
    }
    router
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
    router_with_config(repo, sessions, config).fallback(crate::lens::lens_handler)
}

/// Build the Hub router with explicit HTTP configuration. Convenience
/// wrapper that defaults `db_path` to None (admin/backup endpoints
/// will refuse with a 400 — they need an explicit sqlite path).
pub fn router_with_config(
    repo: Arc<Repository>,
    sessions: Arc<SessionRegistry>,
    config: HubConfig,
) -> Router {
    router_with_config_inner(repo, sessions, config, None)
}

/// State for the bearer-auth middleware: the configured token (if any).
#[derive(Clone)]
struct AuthState {
    token: Option<Arc<String>>,
}

/// The authority (`host[:port]`) of an `Origin` header value, lowercased and
/// with the scheme stripped. `http://Localhost:3001` → `localhost:3001`.
/// Returns `None` for opaque origins like `null`.
fn origin_authority(origin: &str) -> Option<String> {
    let rest = origin.split_once("://").map(|(_, a)| a).unwrap_or(origin);
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.eq_ignore_ascii_case("null") {
        None
    } else {
        Some(authority.to_ascii_lowercase())
    }
}

/// Is this browser `Origin` allowed to call the hub? Same-origin (the Origin's
/// authority equals the request `Host`) is always allowed; otherwise the origin
/// must be in the configured allow-list. Non-browser clients send no `Origin`
/// and never reach this check.
fn origin_is_allowed(origin: &str, host: Option<&str>, allow: &[String]) -> bool {
    match origin_authority(origin) {
        None => false, // opaque/`null` origin — never same-origin, never listed
        Some(auth) => {
            if let Some(h) = host
                && auth == h.to_ascii_lowercase()
            {
                return true; // same-origin
            }
            allow.iter().any(|a| {
                a.eq_ignore_ascii_case(origin) || origin_authority(a).is_some_and(|aa| aa == auth)
            })
        }
    }
}

/// State for the Origin-guard middleware: the extra allowed origins.
#[derive(Clone)]
struct OriginState {
    allowed: Arc<Vec<String>>,
}

/// Reject requests carrying a disallowed `Origin` — the CSRF / DNS-rebinding
/// guard. A page you visit in a browser can `fetch()` a loopback hub, and
/// loopback peers are auth-exempt, so without this a malicious site could drive
/// the API. Non-browser clients (CLI, native MCP) send no `Origin` and pass
/// straight through; same-origin (Lens) is always allowed.
async fn require_allowed_origin(
    State(state): State<OriginState>,
    req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    if let Some(origin) = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    {
        let host = req
            .headers()
            .get(axum::http::header::HOST)
            .and_then(|v| v.to_str().ok());
        if !origin_is_allowed(origin, host, &state.allowed) {
            return (
                StatusCode::FORBIDDEN,
                "cross-origin request rejected (add the origin with --allowed-origin)",
            )
                .into_response();
        }
    }
    next.run(req).await
}

/// Constant-time string comparison, so a wrong token can't be recovered by
/// timing how far the comparison got. Length mismatch returns false but still
/// scans the provided token to keep timing independent of the secret's length.
fn constant_time_eq(provided: &str, expected: &str) -> bool {
    let (a, b) = (provided.as_bytes(), expected.as_bytes());
    let mut diff = (a.len() ^ b.len()) as u8;
    for (i, &byte) in a.iter().enumerate() {
        diff |= byte ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

/// Whole-surface bearer auth. Loopback peers are always exempt (local CLI,
/// Lens, and same-host agents keep working tokenless). When a token is
/// configured, every non-loopback request must carry `Authorization: Bearer
/// <token>`. When no token is configured, nothing is enforced here (the binary
/// warns at startup if it bound to a non-loopback address). A request whose peer
/// address is unknown (no `ConnectInfo`) is treated as untrusted — fail closed.
async fn require_auth(
    State(auth): State<AuthState>,
    req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let Some(token) = auth.token.as_deref() else {
        // No token configured → no per-request enforcement.
        return next.run(req).await;
    };

    // Peer address is attached by `into_make_service_with_connect_info`. Absent
    // in unit tests (oneshot) and any misconfig → treat as non-loopback (fail
    // closed). Tests inject `ConnectInfo` explicitly to exercise both paths.
    let is_loopback = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().is_loopback())
        .unwrap_or(false);
    if is_loopback {
        return next.run(req).await;
    }

    let presented = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));

    match presented {
        Some(t) if constant_time_eq(t, token) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            "missing or invalid bearer token (Authorization: Bearer <token>)",
        )
            .into_response(),
    }
}

fn router_with_config_inner(
    repo: Arc<Repository>,
    sessions: Arc<SessionRegistry>,
    config: HubConfig,
    db_path: Option<String>,
) -> Router {
    // CORS reflects an Origin only when it's same-origin or explicitly allowed
    // (never `Any`), so cross-origin pages can't read API responses. The
    // Origin-guard middleware below additionally blocks such requests from
    // executing; this layer just supplies correct headers for allowed origins.
    let cors_allow = Arc::new(config.allowed_origins.clone());
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::predicate(
            move |origin: &axum::http::HeaderValue, parts: &axum::http::request::Parts| {
                let host = parts
                    .headers
                    .get(axum::http::header::HOST)
                    .and_then(|v| v.to_str().ok());
                origin
                    .to_str()
                    .map(|o| origin_is_allowed(o, host, &cors_allow))
                    .unwrap_or(false)
            },
        ))
        .allow_methods(Any)
        .allow_headers(Any);

    // Request tracing layer — emits a span per HTTP request. At `info` level
    // you get one line per request with method, URI, status, and latency.
    // At `debug` you also get the request body, at `trace` the response body.
    let trace = TraceLayer::new_for_http();

    // Rate limiter — returns None when rpm=0 (disabled).
    let governor = rate_limit::build_layer(config.rate_limit_rpm);

    let asd_repos = Arc::new(config.asd_repos.clone());

    // Build a process pool if any pool repos were configured.
    let asd_pool = if config.asd_pool_repos.is_empty() {
        None
    } else {
        Some(Arc::new(AsdProcessPool::new(
            config.asd_pool_repos.clone(),
            config.asd_serve_binary.clone(),
            config
                .asd_idle_timeout_secs
                .map(std::time::Duration::from_secs),
        )))
    };

    // Prepare the MCP-over-HTTP state before `repo`/`asd_*` move into HubState.
    // Only built when enabled (the daemon path); library/test callers leave
    // `mcp_http` false and get a plain REST router.
    let mcp_state = config.mcp_http.then(|| {
        crate::mcp_http::McpHttpState::new(
            repo.clone(),
            config.agent_id.clone(),
            asd_repos.clone(),
            asd_pool.clone(),
            // When a bearer token guards the surface, the auth middleware is the
            // real gate, so relax rmcp's loopback-only Host allow-list to let
            // authenticated remote clients reach /mcp.
            config.auth_token.is_some(),
        )
    });

    let state = HubState {
        repo,
        sessions,
        db_path,
        asd_repos,
        asd_pool,
        ctx_binary: config.ctx_binary.clone(),
        self_base_url: config.self_base_url.clone(),
    };

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
        // Per-day commit counts for the activity heatmap. Aggregated here so
        // the browser does not fetch thousands of commits just to count them.
        .route("/api/stats/activity/{ref_name}", get(activity_stats))
        .route("/api/blame/{ref_name}", get(blame))
        .route("/api/diff", get(diff_refs))
        .route("/api/merge", post(merge_refs))
        .route("/api/branches", get(list_branches).post(create_branch))
        .route("/api/branches/reset", post(reset_branch))
        .route("/api/branches/{name}", axum::routing::delete(delete_branch))
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
        // On-demand instruction disclosure (t-005). Docs are compiled in, so
        // these need no repo/session — they describe the running binary.
        .route("/api/help", get(help_lookup))
        .route("/api/help/manifest", get(help_manifest))
        // Plan endpoints
        .route("/api/export", get(export_graph))
        .route("/api/import", post(import_graph))
        .route("/api/fsck", post(fsck))
        .route("/api/db/upgrade", post(db_upgrade))
        .route("/api/docs", get(list_docs).post(register_doc))
        .route("/api/plans", get(list_plans).post(create_plan))
        // Static path registered before `/api/plans/{name}` so it wins the match.
        .route("/api/plans/stale", get(stale_plan_tasks))
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
            "/api/plans/{name}/tasks/{task_id}/link",
            post(link_plan_task),
        )
        .route(
            "/api/plans/{name}/tasks/{task_id}/abandon",
            post(abandon_plan_task),
        )
        .route("/api/plans/{name}/next", get(next_plan_task))
        .route("/api/plans/{name}/archive", post(archive_plan))
        .route(
            "/api/plans/{name}/force_complete",
            post(force_complete_plan),
        )
        .route("/api/plans/{name}/move", post(move_plan_handler))
        .route("/api/plans/{name}/relocate", post(relocate_plan))
        // Reminder endpoints
        .route(
            "/api/reminders",
            get(list_reminders_handler).post(create_reminder_handler),
        )
        .route("/api/reminders/due", get(remind_me_handler))
        .route("/api/reminders/{id}", get(get_reminder_handler))
        .route("/api/reminders/{id}/snooze", post(snooze_reminder_handler))
        .route(
            "/api/reminders/{id}/approve",
            post(approve_reminder_handler),
        )
        .route("/api/reminders/{id}/cancel", post(cancel_reminder_handler))
        .route("/api/reminders/{id}/start", post(start_reminder_handler))
        .route("/api/reminders/{id}/record", post(record_reminder_handler))
        // Session turn capture (full request/response/tool/usage JSON)
        .route(
            "/api/sessions/{sid}/turns",
            get(list_session_turns).put(put_session_turns_bulk),
        )
        .route(
            "/api/sessions/{sid}/turns/{idx}",
            post(put_session_turn).get(get_session_turn),
        )
        // Session title (t-016): human-readable name for a session id.
        .route(
            "/api/sessions/{sid}/title",
            axum::routing::put(put_session_title).get(get_session_title),
        )
        .route(
            "/api/sessions/{sid}/meta",
            axum::routing::put(put_session_meta),
        )
        .route(
            "/api/sessions/{sid}/provenance",
            axum::routing::put(put_session_provenance),
        )
        .route(
            "/api/sessions/{sid}/burn",
            axum::routing::put(put_session_burn),
        )
        .route("/api/sessions/{sid}", axum::routing::delete(delete_session))
        .route(
            "/api/sessions/{sid}/move",
            axum::routing::post(move_session),
        )
        .route(
            "/api/sessions/{sid}/derive_links",
            axum::routing::post(derive_session_links),
        )
        .route(
            "/api/sessions/{sid}/links",
            axum::routing::post(add_session_link),
        )
        .route("/api/namespaces", get(list_namespaces))
        .route("/api/session_tombstones", get(list_session_tombstones))
        // Session sync (t-019): re-ingest local Claude Code transcripts by
        // spawning the co-located `ctx ingest-session --all` CLI.
        .route("/api/sessions/sync", post(sync_sessions))
        // Taint / quarantine / watch
        .route(
            "/api/taint",
            get(list_taints_handler).post(apply_taint_handler),
        )
        .route("/api/taint/check", get(check_taint_handler))
        .route(
            "/api/taint/{id}",
            axum::routing::delete(remove_taint_handler),
        )
        // Projects (namespace registry) — one project = one code repo = one
        // ASG namespace. See crate::project for the registry itself.
        .route(
            "/api/projects",
            get(list_projects_handler).post(register_project_handler),
        )
        .route("/api/projects/detect", get(detect_project_handler))
        .route("/api/projects/{id}", get(get_project_handler))
        .route("/api/projects/{id}/paths", post(add_project_path_handler))
        // Admin endpoints
        .route("/api/admin/backup", post(admin_backup));

    // Mount ASD repo registry + per-repo proxy routes when any repos are configured
    // (either static URLs or pool-managed repos).
    if !state.asd_repos.is_empty() || state.asd_pool.is_some() {
        router = router
            .route("/api/code", get(list_asd_repos))
            .route("/api/code/{repo}/prefetch", post(prefetch_asd_repo))
            .route("/api/code/{repo}/{*path}", get(proxy_asd).post(proxy_asd));
    }

    // Raise the request-body limit from Axum's 2 MB default: a whole-session
    // bulk turn write (or an /api/import seed) legitimately runs to many MB, and
    // the default silently 413'd them before the handler's own 128 MiB cap could
    // apply. The handler still enforces SESSION_TURNS_BULK_MAX_BYTES.
    let mut router = router
        .layer(DefaultBodyLimit::max(SESSION_TURNS_BULK_MAX_BYTES))
        .layer(trace)
        .layer(cors)
        .with_state(state);

    // Mount the MCP surface at `/mcp` when enabled. Merged after `with_state`
    // (both are `Router<()>`) so it carries its own `McpHttpState` and sits
    // outside the REST rate limiter — long-lived MCP sessions would otherwise
    // trip the per-IP RPM cap.
    if let Some(mcp_state) = mcp_state {
        router = router.merge(crate::mcp_http::mcp_router(mcp_state));
    }

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

    // Bearer auth outermost — runs first, before rate limiting and routing, so
    // unauthenticated non-loopback traffic is rejected before doing any work.
    // Added last = outermost layer. Guards REST + /mcp alike.
    let auth_state = AuthState {
        token: config.auth_token.clone().map(Arc::new),
    };
    router = router.layer(axum::middleware::from_fn_with_state(
        auth_state,
        require_auth,
    ));

    // Origin guard even further out — reject disallowed cross-origin browser
    // requests before auth/rate-limit/routing. Whole-surface (REST + /mcp).
    let origin_state = OriginState {
        allowed: Arc::new(config.allowed_origins.clone()),
    };
    router = router.layer(axum::middleware::from_fn_with_state(
        origin_state,
        require_allowed_origin,
    ));

    router
}

// -- ASD proxy --

#[derive(Serialize)]
struct AsdRepoInfo {
    name: String,
    url: String,
    /// "static" for pre-running URLs registered via --asd-url; "pool" for
    /// pool-managed children registered via --asd-path.
    source: &'static str,
    /// "running" if a process exists (or the static URL is always assumed
    /// running); "idle" if pool-managed but not yet spawned.
    status: &'static str,
}

/// GET /api/code — list all configured ASD repos (static + pool-managed),
/// each annotated with its `source` and live `status`.
async fn list_asd_repos(State(s): State<HubState>) -> Json<Vec<AsdRepoInfo>> {
    let mut out: Vec<AsdRepoInfo> = s
        .asd_repos
        .iter()
        .map(|(name, url)| AsdRepoInfo {
            name: name.clone(),
            url: url.clone(),
            source: "static",
            status: "running",
        })
        .collect();
    // Append pool repos (without resolving their port — they may not be running yet).
    if let Some(pool) = &s.asd_pool {
        let static_names: std::collections::HashSet<&str> =
            s.asd_repos.iter().map(|(n, _)| n.as_str()).collect();
        for name in pool.repo_names().await {
            if static_names.contains(name.as_str()) {
                continue;
            }
            let status = if pool.is_running(&name).await {
                "running"
            } else {
                "idle"
            };
            out.push(AsdRepoInfo {
                name: name.clone(),
                url: format!("pool:{name}"),
                source: "pool",
                status,
            });
        }
    }
    Json(out)
}

/// POST /api/code/{repo}/prefetch — warm a pool-managed repo by spawning its
/// `asd-serve` child (if not already running). Returns 200 with the base URL
/// once /health passes. Idempotent.
async fn prefetch_asd_repo(
    State(s): State<HubState>,
    Path(repo): Path<String>,
) -> axum::response::Response {
    match resolve_asd_base(&s, &repo).await {
        Ok(base) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            format!(
                r#"{{"name":"{}","url":"{}","status":"running"}}"#,
                repo, base
            ),
        )
            .into_response(),
        Err(msg) => (StatusCode::NOT_FOUND, msg).into_response(),
    }
}

/// Resolve the base URL for a repo, trying static URLs first then the pool.
async fn resolve_asd_base(s: &HubState, repo: &str) -> Result<String, String> {
    // Static URL wins (user-configured, no process management needed)
    if let Some((_, url)) = s.asd_repos.iter().find(|(n, _)| n == repo) {
        return Ok(url.trim_end_matches('/').to_string());
    }
    // Fall back to process pool
    if let Some(pool) = &s.asd_pool {
        return pool.base_url(repo).await;
    }
    Err(format!("unknown ASD repo: {repo}"))
}

/// Forward GET/POST /api/code/{repo}/{*path} → <asd_url>/api/v1/{path}.
/// Routes to the ASD instance registered under {repo} name (static or pool).
async fn proxy_asd(
    State(s): State<HubState>,
    Path((repo, path)): Path<(String, String)>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> axum::response::Response {
    let base = match resolve_asd_base(&s, &repo).await {
        Ok(b) => b,
        Err(msg) => return (StatusCode::NOT_FOUND, msg).into_response(),
    };

    let target = match query.filter(|q| !q.is_empty()) {
        Some(q) => format!("{}/api/v1/{}?{}", base, path, q),
        None => format!("{}/api/v1/{}", base, path),
    };

    let client = reqwest::Client::new();
    let upstream = match client.get(&target).send().await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };

    let status_u16: u16 = upstream.status().as_u16();
    let status = axum::http::StatusCode::from_u16(status_u16).unwrap_or(StatusCode::BAD_GATEWAY);
    let ct: String = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    // Preserve cache-control when the upstream sets it — asd-serve's SSE
    // endpoint sends `no-cache`, and intermediaries respect it.
    let cache_control = upstream
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // Forward the body as a STREAM instead of buffering it. This is what
    // lets `/api/v1/events` (SSE, text/event-stream) work through the
    // proxy — that response never ends, so the old `.bytes().await` would
    // hang forever. Ordinary JSON responses stream through identically
    // (the client sees chunked transfer instead of content-length, which
    // every HTTP client handles), so no content-type branching is needed.
    let body = axum::body::Body::from_stream(upstream.bytes_stream());
    let mut resp = axum::http::Response::builder()
        .status(status)
        .header(axum::http::header::CONTENT_TYPE, ct);
    if let Some(cc) = cache_control {
        resp = resp.header(axum::http::header::CACHE_CONTROL, cc);
    }
    resp.body(body)
        .unwrap_or_else(|e| (StatusCode::BAD_GATEWAY, e.to_string()).into_response())
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
        | RepoError::BranchNotFound(_)
        | RepoError::CommitNotFound(_)
        | RepoError::NamespaceNotFound(_)
        | RepoError::Storage(agentstategraph_storage::StorageError::NamespaceNotFound(_)) => {
            StatusCode::NOT_FOUND
        }
        // Ambiguous prefix is a client input problem, not "not found".
        RepoError::AmbiguousCommitPrefix { .. } => StatusCode::BAD_REQUEST,
        RepoError::CrossNamespaceAccessDenied => StatusCode::FORBIDDEN,
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
    ns: NamespaceId,
    Path(session_id): Path<String>,
) -> Result<Json<SessionSnapshot>, (StatusCode, String)> {
    // Refresh flat-size against the default session so the cache
    // reflects current graph state; the max-across-sessions rule in
    // aggregate() does not help a single-session read.
    let default_session = s.sessions.get_or_create(DEFAULT_SESSION_ID);
    ensure_flat_size(&s.repo, &default_session, "main");

    match s.sessions.snapshot(&session_id) {
        Some(mut snap) => {
            // Best-effort session title (t-016) + meta (t-021). Read from
            // the request's namespace; None when absent.
            if let Ok(repo) = s.repo_for(&ns) {
                snap.name = read_session_title(&repo, "main", &session_id);
                let meta = read_session_meta(&repo, "main", &session_id);
                snap.source = meta.source;
                snap.started_at = meta.started_at;
                snap.updated_at = meta.updated_at;
                snap.models_used = meta.models_used;
            }
            Ok(Json(snap))
        }
        None => Err((
            StatusCode::NOT_FOUND,
            format!("session not found: {}", session_id),
        )),
    }
}

/// `GET /api/stats/sessions` — per-session breakdown.
///
/// Each snapshot's `name` is populated best-effort from the
/// `/sessions/{id}/title` graph node in the request's namespace (t-016);
/// sessions with no ingested title report `name: null`.
/// `GET /api/stats/sessions` — the sessions that live in this workspace.
///
/// The session REGISTRY (token/burn stats) is process-global — one LRU keyed
/// by globally-unique session id — but a session's NODES live in exactly one
/// namespace's graph (Phase 4 moved the pre-existing ones there). So the graph
/// is the source of truth for which workspace a session belongs to, and this
/// filters the global registry down to the sessions actually present in the
/// requested namespace.
///
/// Membership is a shallow `repo.get` on `/sessions/{id}` — it returns the map
/// node without resolving its children, so this stays cheap even for the
/// default namespace's large sessions. It also catches the titleless early
/// sessions (only turns, no title/meta) that a title-based filter would miss.
///
/// Session ids that have a node in this namespace's graph.
///
/// One `list_paths` at depth 2 rather than a `get` per session: a session in a
/// named namespace always carries a `title`/`meta` leaf, and depth 2 stops
/// above the `turns` map — so a 900-turn session contributes ~5 leaves, not
/// 900. Reliable for named namespaces, where every session was ingested or
/// moved with a full subtree.
fn session_ids_with_nodes(repo: &Repository) -> std::collections::HashSet<String> {
    repo.list_paths("main", "/sessions", Some(2))
        .unwrap_or_default()
        .iter()
        .filter_map(|p| p.strip_prefix("/sessions/"))
        .filter_map(|rest| rest.split('/').next())
        .map(str::to_string)
        .collect()
}

/// `GET /api/stats/sessions` — the sessions that live in this workspace.
///
/// The registry (token/burn stats) is process-global — one LRU keyed by
/// globally-unique session id — but a session's NODES live in one namespace's
/// graph (Phase 4 placed the pre-existing ones). Two kinds of session need
/// different treatment, which is why this is not a single graph check:
///
/// - **Ingested / moved** sessions have a `/sessions/{id}` subtree in exactly
///   one namespace. They show in that workspace.
/// - **Registry-only** sessions — created by `recall` or `record_llm_usage`
///   with a session header but never ingested — have stats and NO node
///   anywhere. Their rightful home is `default`, the catch-all.
///
/// So `default` shows everything NOT placed in a named workspace, and a named
/// workspace shows the sessions whose nodes are actually there. `default` is
/// computed as the complement over named namespaces (each small, one
/// `list_paths` apiece) so a registry-only session is never orphaned.
async fn list_sessions(State(s): State<HubState>, ns: NamespaceId) -> impl IntoResponse {
    let default_session = s.sessions.get_or_create(DEFAULT_SESSION_ID);
    ensure_flat_size(&s.repo, &default_session, "main");
    let mut snaps = s.sessions.snapshot_all();
    let is_default_ns = ns.0 == Namespace::DEFAULT;

    if is_default_ns {
        // Placed elsewhere = has a node in any named namespace. Everything
        // else (default-resident sessions AND registry-only ghosts) stays.
        let mut placed = std::collections::HashSet::new();
        if let Ok(names) = s.repo.list_namespaces() {
            for name in names {
                if name.as_str() == Namespace::DEFAULT {
                    continue;
                }
                if let Ok(repo) = s.repo_for(&NamespaceId(name.as_str().to_string())) {
                    placed.extend(session_ids_with_nodes(&repo));
                }
            }
        }
        snaps.retain(|snap| !placed.contains(&snap.session_id));
    } else if let Ok(repo) = s.repo_for(&ns) {
        let here = session_ids_with_nodes(&repo);
        snaps.retain(|snap| here.contains(&snap.session_id));
    }

    // Decorate with title/meta from the workspace being viewed.
    if let Ok(repo) = s.repo_for(&ns) {
        for snap in &mut snaps {
            snap.name = read_session_title(&repo, "main", &snap.session_id);
            let meta = read_session_meta(&repo, "main", &snap.session_id);
            snap.source = meta.source;
            snap.started_at = meta.started_at;
            snap.updated_at = meta.updated_at;
            snap.models_used = meta.models_used;
        }
    }
    Json(snaps)
}

#[derive(Deserialize)]
struct LlmUsageRequest {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    // Accept both spellings: the `ctx` CLI's ingest path posts
    // `cache_creation_tokens` (matching the provider's raw usage field name),
    // while native callers use `cache_create_tokens`. Without the alias the
    // CLI's cache-creation tokens were silently dropped to 0.
    #[serde(default, alias = "cache_creation_tokens")]
    cache_create_tokens: u64,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    /// Token classes the four fields above cannot express, under the
    /// reporting agent's own names (Codex `reasoning_output_tokens`, Gemini
    /// `thoughts`/`tool`, Android Studio's TEXT/IMAGE modality split).
    ///
    /// Captured verbatim and accumulated additively. Callers should send
    /// these *in addition to* the normalised four, not instead of them: the
    /// four remain the cross-agent common denominator that ratios are
    /// computed from.
    #[serde(default)]
    extra_tokens: std::collections::BTreeMap<String, u64>,
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
    session.record_extra_tokens(&req.extra_tokens);

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
    ns: NamespaceId,
    Path(ref_name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    repo.stats(&ref_name).map(Json).map_err(internal_error)
}

#[derive(Deserialize)]
struct HelpQuery {
    /// Feature name or phrase; absent -> full catalog.
    topic: Option<String>,
    /// When true, resolve locally only — do not proxy an unknown topic to the
    /// other tool. Set by the proxied child call to break the chain.
    #[serde(default)]
    no_proxy: bool,
}

/// `GET /api/help?topic=<feature|phrase>[&no_proxy=true]` — on-demand
/// instruction disclosure. No repo/session: the docs are compiled into this
/// binary (t-005). On a local miss it proxies to the owning binary (e.g. asd).
async fn help_lookup(Query(q): Query<HelpQuery>) -> Json<serde_json::Value> {
    Json(crate::help::resolve(q.topic.as_deref(), !q.no_proxy))
}

/// `GET /api/help/manifest` — this binary's lightweight feature index, published
/// so a unified `help` can route across the separate ctx/asd binaries.
async fn help_manifest() -> Json<serde_json::Value> {
    Json(crate::help::manifest())
}

#[derive(Deserialize)]
struct PathQuery {
    path: Option<String>,
}

async fn get_state(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(ref_name): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let path = q.path.unwrap_or_else(|| "/".to_string());
    repo.get_json(&ref_name, &path)
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
    ns: NamespaceId,
    Path(ref_name): Path<String>,
    Query(q): Query<PrefixQuery>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let prefix = q.prefix.unwrap_or_else(|| "/".to_string());
    repo.list_paths(&ref_name, &prefix, q.max_depth)
        .map(Json)
        .map_err(internal_error)
}

#[derive(Deserialize)]
struct ExportQuery {
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
}

/// `GET /api/export` — dump every leaf path+value on a ref into a JSON map. A
/// portable, human-editable snapshot: prune it, then `import` into a fresh db to
/// keep only what you want.
async fn export_graph(
    State(s): State<HubState>,
    ns: NamespaceId,
    Query(q): Query<ExportQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let paths = repo
        .list_paths(&q.ref_name, "/", None)
        .map_err(internal_error)?;
    let mut map = serde_json::Map::new();
    for p in paths {
        // Skip DB-internal schema metadata — it's per-db and only writable by
        // migration commits, so it can't (and shouldn't) be re-imported.
        if p.starts_with("/_meta/") {
            continue;
        }
        if let Ok(v) = repo.get_json(&q.ref_name, &p) {
            map.insert(p, v);
        }
    }
    Ok(Json(serde_json::json!({
        "ref": q.ref_name,
        "namespace": ns.0,
        "count": map.len(),
        "paths": serde_json::Value::Object(map),
    })))
}

#[derive(Deserialize, Default)]
struct FsckRequest {
    /// When true, rewind every damaged ref to its nearest fully-readable
    /// ancestor commit. Destructive (abandons the dangling commit); the CLI
    /// gates it behind an explicit `--repair` + confirmation.
    #[serde(default)]
    repair: bool,
}

/// `POST /api/fsck` — verify repository integrity across every namespace and
/// branch: walk each branch head's state tree and report any object that is
/// referenced but missing from the store (the "dangling interior node" class
/// of corruption). With `repair: true`, rewind each damaged ref to its nearest
/// fully-readable ancestor. Because no GC ever deletes objects, that ancestor's
/// tree is guaranteed intact and the completed work below the break survives.
async fn fsck(
    State(s): State<HubState>,
    Json(req): Json<FsckRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Every namespace, plus `default` (which list_namespaces omits — it is the
    // root, not a fork).
    let mut namespaces = s.repo.list_namespaces().map_err(internal_error)?;
    if !namespaces.iter().any(|n| n.as_str() == Namespace::DEFAULT) {
        namespaces.push(
            Namespace::new(Namespace::DEFAULT)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        );
    }

    let mut checked = 0usize;
    let mut problems = Vec::new();

    for ns in namespaces {
        let ns_name = ns.as_str().to_string();
        let repo = s.repo_for(&NamespaceId(ns_name.clone()))?;
        let branches = repo.list_branches(None).map_err(internal_error)?;
        for (branch, head) in branches {
            checked += 1;
            // A branch is damaged if its head commit is missing, or any object
            // reachable from the commit's state tree is missing.
            let missing: String = match repo.first_missing_object(&head) {
                Ok(None) => continue, // healthy
                Ok(Some(id)) => id.to_hex(),
                // A missing head commit (or any read error) is itself a defect.
                Err(e) => e.to_string(),
            };

            let ancestor = repo.nearest_readable_ancestor(&head).ok().flatten();
            let mut repaired = false;
            let mut repaired_to = serde_json::Value::Null;
            if req.repair {
                if let Some(anc) = ancestor {
                    match repo.set_ref(&branch, anc) {
                        Ok(()) => {
                            repaired = true;
                            repaired_to = serde_json::Value::String(anc.to_hex());
                        }
                        Err(e) => {
                            problems.push(serde_json::json!({
                                "namespace": ns_name,
                                "branch": branch,
                                "head": head.to_hex(),
                                "missing_object": missing.clone(),
                                "nearest_readable_ancestor": anc.to_hex(),
                                "repaired": false,
                                "repair_error": e.to_string(),
                            }));
                            continue;
                        }
                    }
                }
            }

            problems.push(serde_json::json!({
                "namespace": ns_name,
                "branch": branch,
                "head": head.to_hex(),
                "missing_object": missing,
                "nearest_readable_ancestor":
                    ancestor.map(|a| a.to_hex()).map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null),
                "repaired": repaired,
                "repaired_to": repaired_to,
            }));
        }
    }

    Ok(Json(serde_json::json!({
        "checked": checked,
        "healthy": checked - problems.len(),
        "problems": problems,
        "repair": req.repair,
    })))
}

#[derive(Deserialize, Default)]
struct DbUpgradeRequest {
    /// Report the current vs target schema version without applying anything.
    #[serde(default)]
    check: bool,
}

/// `POST /api/db/upgrade` — run any pending CTXone schema migrations up to the
/// version this hub binary expects. With `check: true`, only report the version
/// gap. Migrations also run automatically on hub startup; this is the explicit,
/// snapshot-and-fsck-wrapped path the CLI drives. Refuses (409) if the graph is
/// from a NEWER hub than this binary — never downgrades a schema.
async fn db_upgrade(
    State(s): State<HubState>,
    Json(req): Json<DbUpgradeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let current = crate::migrations::current_schema_version(&s.repo);
    let target = crate::migrations::CTXONE_SCHEMA_VERSION;

    if req.check {
        return Ok(Json(serde_json::json!({
            "current_version": current,
            "target_version": target,
            "pending": current < target,
            "migrated": false,
        })));
    }

    if current < target {
        crate::migrations::run_migrations(&s.repo)
            // NewerGraph / migration failure → 409 Conflict, ref/data untouched.
            .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;
    }

    let now = crate::migrations::current_schema_version(&s.repo);
    Ok(Json(serde_json::json!({
        "current_version": now,
        "target_version": target,
        "was_version": current,
        "migrated": current < target,
    })))
}

#[derive(Deserialize)]
struct ImportRequest {
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
    /// `{path: value}` map, as produced by `export` (its `paths` object).
    paths: serde_json::Map<String, serde_json::Value>,
}

/// `POST /api/import` — write a `{path: value}` map onto a ref, to seed a fresh
/// db from a (pruned) export snapshot.
async fn import_graph(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Json(req): Json<ImportRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let mut imported = 0usize;
    for (path, value) in &req.paths {
        // Never write DB-internal schema metadata (reserved for migrations).
        if path.starts_with("/_meta/") {
            continue;
        }
        let opts = CommitOptions::new(
            &agent_id.0,
            IntentCategory::Custom("Import".to_string()),
            format!("import {path}"),
        );
        repo.set_json(&req.ref_name, path, value, opts)
            .map_err(internal_error)?;
        imported += 1;
    }
    s.sessions.mark_all_dirty();
    Ok(Json(
        serde_json::json!({ "ref": req.ref_name, "imported": imported }),
    ))
}

/// Slug for a doc registry entry: the path lowercased with runs of non
/// alnum collapsed to `-`. Keeps re-registering the same path idempotent.
fn doc_slug(path: &str) -> String {
    let mut s = String::new();
    let mut dash = false;
    for c in path.to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c);
            dash = false;
        } else if !dash && !s.is_empty() {
            s.push('-');
            dash = true;
        }
    }
    s.trim_matches('-').to_string()
}

#[derive(Deserialize)]
struct DocRegisterRequest {
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
    path: String,
    status: Option<String>,
    scope: Option<String>,
    owner: Option<String>,
    answers: Option<String>,
    supersedes: Option<String>,
    last_verified_commit: Option<String>,
}

/// `POST /api/docs` — register (or update) a canonical-doc entry so agents can
/// find docs without scanning the repo. Keeps the repo file canonical; this is
/// just the index/pointer.
async fn register_doc(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Json(req): Json<DocRegisterRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let slug = doc_slug(&req.path);
    if slug.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "path is required".to_string()));
    }
    let entry = serde_json::json!({
        "path": req.path,
        "status": req.status.unwrap_or_else(|| "canonical".to_string()),
        "scope": req.scope,
        "owner": req.owner,
        "answers": req.answers,
        "supersedes": req.supersedes,
        "last_verified_commit": req.last_verified_commit,
    });
    let opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Custom("DocRegistry".to_string()),
        format!("register doc {}", req.path),
    );
    repo.set_json(&req.ref_name, &format!("/docs/{slug}"), &entry, opts)
        .map_err(internal_error)?;
    Ok(Json(entry))
}

/// `GET /api/docs` — list all registered doc entries.
async fn list_docs(
    State(s): State<HubState>,
    ns: NamespaceId,
    Query(q): Query<ExportQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    // set_json stores each entry as a tree, so list_paths returns the leaf
    // FIELDS (…/<slug>/status, …). Collect the distinct slugs, then get_json
    // each `/docs/<slug>` to reassemble the whole object.
    let leaves = repo
        .list_paths(&q.ref_name, "/docs", None)
        .unwrap_or_default();
    let mut slugs = std::collections::BTreeSet::new();
    for p in leaves {
        if let Some(rest) = p.strip_prefix("/docs/")
            && let Some(slug) = rest.split('/').next()
            && !slug.is_empty()
        {
            slugs.insert(slug.to_string());
        }
    }
    let mut out = Vec::new();
    for slug in slugs {
        if let Ok(v) = repo.get_json(&q.ref_name, &format!("/docs/{slug}")) {
            out.push(v);
        }
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
struct SearchQuery {
    query: String,
    max_results: Option<usize>,
}

async fn search_values(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(ref_name): Path<String>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResult>>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let results = repo
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

#[derive(Deserialize)]
struct ActivityQuery {
    /// Days of history to report, counting back from today. Default 120.
    days: Option<u32>,
}

/// How far back the walk is willing to read before giving up.
///
/// The underlying `repo.log` takes a commit count, not a date, so a day
/// window has to be carved out of a bounded walk. Full-turn capture writes
/// one commit per turn, so a busy day can be thousands — this needs to be
/// large enough that the cap is rare, while still bounding a pathological
/// request. When it does bite, the response says so rather than silently
/// reporting a short history as if it were the whole truth.
const ACTIVITY_SCAN_LIMIT: usize = 50_000;

/// `GET /api/stats/activity?days=N` — commits per day, for the dashboard
/// heatmap.
///
/// Exists because the heatmap previously counted `/api/log?limit=1000`
/// client-side, which charts *a commit-count window, not a time window*:
/// the busier the machine, the less history it showed. On a machine mid
/// session-import, 1000 commits covered 80 minutes.
///
/// Returns `{ days: [{date, count}], truncated, scanned }`. `truncated` is
/// true when the scan hit its cap before reaching the requested cutoff, so
/// the UI can say the history is partial instead of implying a quiet period.
async fn activity_stats(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(ref_name): Path<String>,
    Query(q): Query<ActivityQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let days = q.days.unwrap_or(120).clamp(1, 730) as i64;
    let cutoff = chrono::Utc::now().date_naive() - chrono::Duration::days(days - 1);

    let commits = repo
        .log(&ref_name, ACTIVITY_SCAN_LIMIT)
        .map_err(internal_error)?;
    let scanned = commits.len();

    let mut counts: std::collections::BTreeMap<chrono::NaiveDate, u64> = Default::default();
    let mut oldest_seen: Option<chrono::NaiveDate> = None;
    for c in &commits {
        let d = c.timestamp.date_naive();
        oldest_seen = Some(oldest_seen.map_or(d, |o: chrono::NaiveDate| o.min(d)));
        if d < cutoff {
            continue;
        }
        *counts.entry(d).or_insert(0) += 1;
    }

    // Emit EVERY day in the window, zero-filled — not just the days with
    // activity. The heatmap derives its grid from the min/max date it is
    // handed, so a sparse series makes the chart's span (and width) a
    // function of when work happened: it visibly resizes between refreshes,
    // and a quiet stretch at either end silently shortens the range. A dense
    // series pins the grid to the requested window. 120 days of {date,count}
    // is a few KB.
    let today = chrono::Utc::now().date_naive();
    let mut per_day: Vec<(String, u64)> = Vec::with_capacity(days as usize);
    let mut d = cutoff;
    while d <= today {
        per_day.push((
            d.format("%Y-%m-%d").to_string(),
            counts.get(&d).copied().unwrap_or(0),
        ));
        d += chrono::Duration::days(1);
    }

    // Truncated only if the walk was capped AND never reached back past the
    // cutoff — a capped walk that already covers the window is complete for
    // the purpose of this request.
    let truncated = scanned >= ACTIVITY_SCAN_LIMIT && oldest_seen.is_some_and(|o| o >= cutoff);

    let active_days = per_day.iter().filter(|(_, c)| *c > 0).count();
    let out: Vec<serde_json::Value> = per_day
        .into_iter()
        .map(|(date, count)| serde_json::json!({ "date": date, "count": count }))
        .collect();

    Ok(Json(serde_json::json!({
        "days": out,
        "requested_days": days,
        "active_days": active_days,
        "scanned": scanned,
        "truncated": truncated,
    })))
}

async fn get_log(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(ref_name): Path<String>,
    Query(q): Query<LogQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let limit = q.limit.unwrap_or(20);
    let commits = repo.log(&ref_name, limit).map_err(internal_error)?;

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
    ns: NamespaceId,
    Path(ref_name): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let path = q.path.unwrap_or_else(|| "/".to_string());
    let blame = repo.blame(&ref_name, &path).map_err(internal_error)?;
    Ok(Json(serde_json::to_value(&blame).unwrap_or_default()))
}

async fn list_branches(
    State(s): State<HubState>,
    ns: NamespaceId,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let branches = repo.list_branches(None).map_err(internal_error)?;
    let out: Vec<serde_json::Value> = branches
        .into_iter()
        .map(|(name, id)| {
            // The head commit's timestamp is the branch's "last updated" — what
            // the dashboard's recently-updated panel sorts on. Best-effort: a
            // branch whose head commit can't be read still lists, just without
            // a timestamp.
            let updated_at = repo
                .get_commit(&id)
                .ok()
                .flatten()
                .map(|c| c.timestamp.to_rfc3339());
            serde_json::json!({
                "name": name,
                "id": format!("{}", id.short()),
                "updated_at": updated_at,
            })
        })
        .collect();
    Ok(Json(out))
}

#[derive(Deserialize)]
struct CreateBranchRequest {
    name: String,
    #[serde(default = "default_ref")]
    from: String,
    /// Idempotent mode: an already-existing branch is success, not 500.
    /// Branch mirroring re-ensures on every CLI invocation.
    #[serde(default)]
    if_missing: bool,
    /// Raw git branch this ASG branch mirrors, recorded as metadata
    /// (sanitization is lossy: `feature/x` → `feature-x`).
    git_branch: Option<String>,
}

async fn create_branch(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Json(req): Json<CreateBranchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let created = match repo.branch(&req.name, &req.from) {
        Ok(id) => Some(id),
        Err(agentstategraph::RepoError::BranchAlreadyExists(_)) if req.if_missing => None,
        Err(e) => return Err(internal_error(e)),
    };
    // Record the mirrored git branch as namespace-global metadata on the
    // `from` ref. Written once, on actual creation.
    if let (Some(raw), Some(_)) = (&req.git_branch, &created) {
        let opts = CommitOptions::new(
            &agent_id.0,
            IntentCategory::Custom("Observe".to_string()),
            format!("branch {} mirrors git branch {}", req.name, raw),
        );
        let _ = repo.set_json(
            &req.from,
            &format!("/ctxone/branches/{}/git_branch", req.name),
            &serde_json::json!(raw),
            opts,
        );
    }
    let mut out = serde_json::json!({
        "status": "ok",
        "name": req.name,
        "from": req.from,
        "existed": created.is_none(),
    });
    if let Some(id) = created {
        out["commit_id"] = serde_json::json!(format!("{}", id.short()));
    }
    Ok(Json(out))
}

/// `DELETE /api/branches/{name}` — drop a branch ref from this workspace.
///
/// Refuses `main`: it is the workspace's trunk and every other ref forks from
/// it, so deleting it would strand the namespace. Content is content-addressed
/// and shared, so removing a ref frees no other branch's nodes — this only
/// forgets the label. `deleted:false` means the branch was already absent
/// (idempotent, so re-running a prune is safe).
async fn delete_branch(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if name == "main" {
        return Err((
            StatusCode::BAD_REQUEST,
            "refusing to delete 'main' — it is the workspace trunk".to_string(),
        ));
    }
    let repo = s.repo_for(&ns)?;
    let deleted = repo.delete_branch(&name).map_err(internal_error)?;
    Ok(Json(serde_json::json!({
        "status": "ok",
        "name": name,
        "deleted": deleted,
    })))
}

#[derive(Deserialize)]
struct ResetBranchRequest {
    /// Branch whose ref is moved.
    name: String,
    /// Ref-spec to reset to (branch, tag, full commit id, or unique prefix).
    to: String,
    /// Retain the current target as a timestamped recovery ref first.
    #[serde(default)]
    backup: bool,
}

/// `POST /api/branches/reset` — move a branch ref to another ref, optionally
/// retaining the old target as a timestamped recovery ref so the reset is
/// auditable and reversible without direct storage surgery.
async fn reset_branch(
    State(s): State<HubState>,
    ns: NamespaceId,
    Json(req): Json<ResetBranchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;

    // Resolve the destination first (accepts commit ids/prefixes via head()).
    let to_commit = repo.head(&req.to).map_err(internal_error)?;

    // Back up the current target before moving it.
    let mut backup_ref = None;
    if req.backup {
        // Only if the target currently exists.
        if let Ok(_old) = repo.head(&req.name) {
            let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
            let name = format!("{}-recovery-{}", req.name, ts);
            repo.branch(&name, &req.name).map_err(internal_error)?;
            backup_ref = Some(name);
        }
    }

    repo.set_ref(&req.name, to_commit).map_err(internal_error)?;
    s.sessions.mark_all_dirty();

    let mut out = serde_json::json!({
        "status": "ok",
        "name": req.name,
        "to": req.to,
        "commit_id": format!("{}", to_commit.short()),
    });
    if let Some(b) = backup_ref {
        out["backup_ref"] = serde_json::json!(b);
    }
    Ok(Json(out))
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
    ns: NamespaceId,
    Query(q): Query<DiffQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let ops = repo.diff(&q.ref_a, &q.ref_b).map_err(internal_error)?;
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
    /// Preview the merge without advancing the target ref.
    #[serde(default)]
    dry_run: bool,
    /// Permit the merge to remove top-level entries (e.g. an entire /plans or
    /// /memory map). Without this, such a merge is refused.
    #[serde(default)]
    allow_deletions: bool,
    /// Permit the merge to move a completed (terminal) task back to a
    /// non-terminal state. Without this, such a regression is refused.
    #[serde(default)]
    allow_regressions: bool,
}

fn default_merge_description() -> String {
    "Merge".to_string()
}

#[instrument(skip_all, fields(source = %req.source, target = %req.target, agent = %agent_id.0))]
async fn merge_refs(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Json(req): Json<MergeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let mut opts = CommitOptions::new(&agent_id.0, IntentCategory::Merge, &req.description);
    if let Some(r) = req.reasoning {
        opts = opts.with_reasoning(r);
    }

    // Plans-domain guard: a task that is terminal on the target must not be
    // pulled back to a non-terminal state by the source.
    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
    let regressions = plan_tools::detect_plan_regressions(&store, &req.source, &req.target);

    // Dry-run: report what the merge would do without touching the target ref.
    if req.dry_run {
        return match repo.preview_merge(&req.source, &req.target) {
            Ok(preview) => Ok(Json(serde_json::json!({
                "status": "dry_run",
                "source": req.source,
                "target": req.target,
                "fast_forward": preview.fast_forward,
                "added": preview.added,
                "changed": preview.changed,
                "removed": preview.removed,
                "would_delete": preview.has_deletions(),
                "regressions": serde_json::to_value(&regressions).unwrap_or_default(),
                "conflicts": serde_json::to_value(&preview.conflicts).unwrap_or_default(),
            }))),
            Err(e) => Err(internal_error(e)),
        };
    }

    // Terminal-state regressions are refused by default — un-settling a
    // completed task and dropping its proof is rarely intended — but the
    // caller can opt in with allow_regressions when it genuinely is.
    if !regressions.is_empty() && !req.allow_regressions {
        return Err((
            StatusCode::CONFLICT,
            serde_json::json!({
                "status": "regression",
                "source": req.source,
                "target": req.target,
                "regressions": serde_json::to_value(&regressions).unwrap_or_default(),
                "hint": "source would move a completed task back to a non-terminal state",
            })
            .to_string(),
        ));
    }

    match repo.merge_checked(&req.source, &req.target, opts, req.allow_deletions) {
        Ok(commit_id) => {
            // Restore the "plan completed iff all tasks terminal" invariant for
            // plans whose final task arrived via this merge.
            let promoted =
                plan_tools::promote_completed_plans(&store, &repo, &req.target, &agent_id.0);
            // Graph size may have changed — invalidate every session's cache.
            s.sessions.mark_all_dirty();
            Ok(Json(serde_json::json!({
                "status": "ok",
                "source": req.source,
                "target": req.target,
                "commit_id": format!("{}", commit_id.short()),
                "promoted_plans": promoted,
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
        Err(agentstategraph::RepoError::MergeWouldDelete(removed)) => {
            // Data-loss guard tripped. 409 with the entries that would vanish so
            // the caller can re-run with allow_deletions if intended.
            Err((
                StatusCode::CONFLICT,
                serde_json::json!({
                    "status": "would_delete",
                    "source": req.source,
                    "target": req.target,
                    "removed": removed,
                    "hint": "re-run with allow_deletions=true to proceed",
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
    ns: NamespaceId,
    agent_id: AgentId,
    session_id: SessionId,
    Json(req): Json<RememberRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
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
    let commit_id = repo
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
    ns: NamespaceId,
    agent_id: AgentId,
    Json(req): Json<ForgetRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let opts = CommitOptions::new(&agent_id.0, IntentCategory::Rollback, &req.reason);

    let commit_id = repo
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
    ns: NamespaceId,
    session_id: SessionId,
    Query(q): Query<RecallQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let budget = q.budget.unwrap_or(1500);
    let session = s.session_for(&session_id);
    let result = run_recall(&repo, &session, &q.topic, budget, &q.ref_name);
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
    ns: NamespaceId,
    session_id: SessionId,
    Path(project): Path<String>,
    Query(q): Query<ContextQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let path = format!("/memory/projects/{}", project);
    match repo.get_json(&q.ref_name, &path) {
        Ok(value) => {
            let session = s.session_for(&session_id);
            ensure_flat_size(&repo, &session, &q.ref_name);
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

/// Writes `/sessions/{id}/{summary,decisions,details}` into the REQUEST's
/// namespace at ref `main` — the same place `ctx ingest-session` puts a
/// session's turns, title and meta.
///
/// Now that sessions live in per-project workspaces rather than all in
/// `default`, summarising a session while pointed at the wrong workspace
/// writes an orphan node that no session view will ever show. The namespace
/// is echoed in the response so a caller can catch that rather than have it
/// pass silently.
async fn summarize_session(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Json(req): Json<SummarizeSessionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let summary = req.key_points.join(". ");
    let summary_opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Checkpoint,
        format!("Session {} summary", req.session_id),
    )
    .with_confidence(0.9);

    let summary_val = serde_json::Value::String(summary);
    repo.set_json(
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

        repo.set_json(
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
        // Which workspace this landed in. Silent scope is what put 98
        // sessions in the wrong one.
        "namespace": ns.0,
    })))
}

#[derive(Deserialize)]
struct WhatChangedQuery {
    since: String,
}

async fn what_changed_since(
    State(s): State<HubState>,
    ns: NamespaceId,
    Query(q): Query<WhatChangedQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let commits = repo.log("main", 100).map_err(internal_error)?;
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
    ns: NamespaceId,
    Query(q): Query<WhyDidWeQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let results = repo
        .search_values("main", &q.decision, Some(5))
        .map_err(internal_error)?;

    let mut traces = Vec::new();
    for (path, _) in &results {
        if let Ok(blame_info) = repo.blame("main", path) {
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
    ns: NamespaceId,
    session_id: SessionId,
    agent_id: AgentId,
    Json(req): Json<PrimeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let sections: Vec<(String, String)> = req
        .sections
        .into_iter()
        .map(|s| (s.title, s.body))
        .collect();
    debug!(count = sections.len(), agent = %agent_id.0, "priming sections");

    let session = s.session_for(&session_id);
    let result = run_prime(
        &repo,
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
    ns: NamespaceId,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    // If /memory/pinned doesn't exist yet, return an empty list instead of 500.
    let paths = repo
        .list_paths("main", "/memory/pinned", Some(20))
        .unwrap_or_default();

    let mut out = Vec::new();
    for p in &paths {
        if let Ok(val) = repo.get_json("main", p) {
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

/// Like `RefQuery` but the branch is optional, so the handler can tell
/// "caller omitted `?ref=`" from "caller asked for `main`" and fall back
/// to another source (e.g. a JSON body) before defaulting.
#[derive(Deserialize)]
struct OptionalRefQuery {
    #[serde(default, rename = "ref")]
    ref_name: Option<String>,
}

/// Optional JSON body for `archive_plan`. Older CLIs sent the branch here
/// (`{"ref": "..."}`) instead of as `?ref=`; accepting it as a fallback
/// keeps those clients working rather than silently archiving on `main`.
#[derive(Deserialize, Default)]
struct ArchiveBody {
    #[serde(default, rename = "ref")]
    ref_name: Option<String>,
}

#[derive(Deserialize)]
struct PlanListQuery {
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
    #[serde(default)]
    status: Option<String>,
    /// List plans across every namespace (each result tagged with its
    /// `namespace`), instead of just the request's namespace.
    #[serde(default)]
    all_namespaces: bool,
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
    /// Bypass the "plan nearing completion" lock (see env var
    /// `CTXONE_PLAN_LOCK_RATIO`). When the env var is unset the lock
    /// is disabled and this flag is a no-op. Default: false.
    #[serde(default)]
    force: bool,
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
    /// Pick order: `priority` (default — highest priority, id tiebreak) or
    /// `order` (first unstarted by task id, for sequential plans).
    #[serde(default)]
    mode: Option<String>,
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
        // 423 Locked — plan is nearing completion and the
        // `CTXONE_PLAN_LOCK_RATIO` guard is engaged. Caller can pass
        // `force=true` to bypass.
        plan_tools::PlanToolError::PlanLocked { .. } => StatusCode::LOCKED,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, err.to_string())
}

fn substrate_error_to_response(err: agentstategraph_tasks::TaskStoreError) -> (StatusCode, String) {
    plan_error_to_response(plan_tools::PlanToolError::Substrate(err))
}

#[derive(Deserialize)]
struct StaleQuery {
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
    #[serde(default = "default_stale_days")]
    days: i64,
    #[serde(default)]
    all_namespaces: bool,
}

fn default_stale_days() -> i64 {
    7
}

/// `GET /api/plans/stale?days=N` — in-progress tasks in active plans whose
/// `started_at` (fallback `created_at`) is older than N days. Surfaces stale
/// in-progress state so agents/humans notice work that stopped mid-flight.
async fn stale_plan_tasks(
    State(s): State<HubState>,
    ns: NamespaceId,
    Query(q): Query<StaleQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    use agentstategraph_tasks::TaskStatus;
    let now = chrono::Utc::now();
    let cutoff = now - chrono::Duration::days(q.days.max(0));

    let namespaces: Vec<String> = if q.all_namespaces {
        let mut names: Vec<String> = s
            .repo
            .list_namespaces()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .into_iter()
            .map(|n| n.to_string())
            .collect();
        let default = Namespace::DEFAULT.to_string();
        if !names.iter().any(|n| n == &default) {
            names.push(default);
        }
        names.sort();
        names.dedup();
        names
    } else {
        vec![ns.0.clone()]
    };

    let mut out = Vec::new();
    for ns_name in namespaces {
        let repo = s.repo_for(&NamespaceId(ns_name.clone()))?;
        let store = plan_tools::make_store(repo, DEFAULT_AGENT_ID);
        let plans = match store
            .list_plans_by_status(&q.ref_name, Some(agentstategraph_tasks::PlanStatus::Active))
        {
            Ok(p) => p,
            Err(_) if q.all_namespaces => continue,
            Err(e) => return Err(substrate_error_to_response(e)),
        };
        for plan in plans {
            let tasks = store
                .list_tasks(&q.ref_name, &plan.name)
                .unwrap_or_default();
            for t in tasks {
                if t.status != TaskStatus::InProgress {
                    continue;
                }
                let since = t.started_at.unwrap_or(t.created_at);
                if since < cutoff {
                    let mut entry = serde_json::json!({
                        "plan": plan.name,
                        "id": t.id.as_str(),
                        "title": t.title,
                        "started_at": since.to_rfc3339(),
                        "age_days": (now - since).num_days(),
                    });
                    if q.all_namespaces
                        && let Some(obj) = entry.as_object_mut()
                    {
                        obj.insert("namespace".to_string(), serde_json::json!(ns_name));
                    }
                    out.push(entry);
                }
            }
        }
    }
    // Most stale first.
    out.sort_by(|a, b| {
        b["age_days"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&a["age_days"].as_i64().unwrap_or(0))
    });
    Ok(Json(out))
}

#[instrument(skip_all, fields(ref_name = %q.ref_name, status = q.status.as_deref().unwrap_or("")))]
async fn list_plans(
    State(s): State<HubState>,
    ns: NamespaceId,
    Query(q): Query<PlanListQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let filter = q
        .status
        .as_deref()
        .and_then(plan_tools::plan_status_from_str);

    // Which namespaces to scan: just the request's namespace (default), or
    // every namespace for a global inventory (--all-namespaces).
    let namespaces: Vec<String> = if q.all_namespaces {
        let mut names: Vec<String> = s
            .repo
            .list_namespaces()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .into_iter()
            .map(|n| n.to_string())
            .collect();
        // Ensure the default namespace is always covered.
        let default = Namespace::DEFAULT.to_string();
        if !names.iter().any(|n| n == &default) {
            names.push(default);
        }
        names.sort();
        names.dedup();
        names
    } else {
        vec![ns.0.clone()]
    };

    let mut out = Vec::new();
    for ns_name in namespaces {
        let repo = s.repo_for(&NamespaceId(ns_name.clone()))?;
        let store = plan_tools::make_store(repo, DEFAULT_AGENT_ID);
        let plans = match store.list_plans_by_status(&q.ref_name, filter) {
            Ok(p) => p,
            // A namespace with no ref/data yet shouldn't abort the whole listing.
            Err(_) if q.all_namespaces => continue,
            Err(e) => return Err(substrate_error_to_response(e)),
        };
        for plan in plans {
            let tasks = store
                .list_tasks(&q.ref_name, &plan.name)
                .unwrap_or_default();
            let mut pj = plan_tools::plan_to_json(&plan, &tasks, false);
            if q.all_namespaces
                && let Some(obj) = pj.as_object_mut()
            {
                obj.insert("namespace".to_string(), serde_json::json!(ns_name));
            }
            out.push(pj);
        }
    }
    Ok(Json(out))
}

#[instrument(skip_all, fields(name = %req.name, ref_name = %req.ref_name, agent = %agent_id.0))]
async fn create_plan(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Json(req): Json<CreatePlanRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
    let plan = plan_tools::create_plan(&store, &req.ref_name, &req.name, req.description)
        .map_err(plan_error_to_response)?;
    s.sessions.mark_all_dirty();
    let body = plan_tools::plan_to_json(&plan, &[], false);
    Ok((StatusCode::CREATED, Json(body)))
}

/// Graph path holding a task's cross-plan "satisfies" links. Stored outside the
/// substrate's task state machine (which only models within-plan `blocked_by`),
/// so a task in one plan can point at a task in another.
fn plan_link_path(plan: &str, task: &str) -> String {
    format!("/plan_links/{plan}/{task}")
}

/// Read the `plan/task` targets a task satisfies (empty if none/unset).
fn read_satisfies(repo: &Repository, ref_name: &str, plan: &str, task: &str) -> Vec<String> {
    repo.get_json(ref_name, &plan_link_path(plan, task))
        .ok()
        .and_then(|v| {
            v.get("satisfies").and_then(|s| s.as_array()).map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct LinkRequest {
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
    /// Target this task satisfies, as `plan/task`.
    target: String,
}

/// `POST /api/plans/{name}/tasks/{id}/link` — record that this task, when done,
/// satisfies a task in another plan (a cross-plan dependency pointer).
async fn link_plan_task(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path((name, task_id)): Path<(String, String)>,
    Json(req): Json<LinkRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !req.target.contains('/') {
        return Err((
            StatusCode::BAD_REQUEST,
            "target must be 'plan/task' (e.g. other-plan/t-002)".to_string(),
        ));
    }
    let repo = s.repo_for(&ns)?;
    let mut links = read_satisfies(&repo, &req.ref_name, &name, &task_id);
    if !links.contains(&req.target) {
        links.push(req.target.clone());
    }
    let opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Custom("Link".to_string()),
        format!("{name}/{task_id} satisfies {}", req.target),
    );
    repo.set_json(
        &req.ref_name,
        &plan_link_path(&name, &task_id),
        &serde_json::json!({ "satisfies": links }),
        opts,
    )
    .map_err(internal_error)?;
    Ok(Json(
        serde_json::json!({ "plan": name, "task": task_id, "satisfies": links }),
    ))
}

#[instrument(skip_all, fields(name = %name, ref_name = %q.ref_name))]
async fn get_plan(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(name): Path<String>,
    Query(q): Query<RefQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let store = plan_tools::make_store(repo.clone(), DEFAULT_AGENT_ID);
    let plan = store
        .get_plan(&q.ref_name, &name)
        .map_err(substrate_error_to_response)?;
    let tasks = store.list_tasks(&q.ref_name, &name).unwrap_or_default();
    let mut out = plan_tools::plan_to_json(&plan, &tasks, true);
    // Attach cross-plan "satisfies" links to each task from the graph.
    if let Some(arr) = out["tasks"].as_array_mut() {
        for tj in arr.iter_mut() {
            if let Some(id) = tj["id"].as_str() {
                let links = read_satisfies(&repo, &q.ref_name, &name, id);
                if !links.is_empty()
                    && let Some(obj) = tj.as_object_mut()
                {
                    obj.insert("satisfies".to_string(), serde_json::json!(links));
                }
            }
        }
    }
    Ok(Json(out))
}

#[instrument(skip_all, fields(name = %name, ref_name = %q.ref_name, agent = %agent_id.0))]
async fn delete_plan(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path(name): Path<String>,
    Query(q): Query<RefQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
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
    ns: NamespaceId,
    Path(name): Path<String>,
    Query(q): Query<RefQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let store = plan_tools::make_store(repo.clone(), DEFAULT_AGENT_ID);
    let tasks = store
        .list_tasks(&q.ref_name, &name)
        .map_err(substrate_error_to_response)?;
    let out: Vec<serde_json::Value> = tasks.iter().map(plan_tools::task_to_json).collect();
    Ok(Json(out))
}

#[instrument(skip_all, fields(name = %name, title = %req.title, agent = %agent_id.0))]
async fn add_plan_task(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path(name): Path<String>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
    // Enforce the "plan nearing completion" lock before mutating.
    // No-op unless `CTXONE_PLAN_LOCK_RATIO` is set; `force=true` bypasses.
    plan_tools::check_plan_lock(&store, &req.ref_name, &name, req.force)
        .map_err(plan_error_to_response)?;
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
    ns: NamespaceId,
    Path((name, task_id)): Path<(String, String)>,
    Query(q): Query<RefQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    use agentstategraph_tasks::TaskId;
    let store = plan_tools::make_store(repo.clone(), DEFAULT_AGENT_ID);
    let task = store
        .get_task(&q.ref_name, &name, &TaskId(task_id))
        .map_err(substrate_error_to_response)?;
    Ok(Json(plan_tools::task_to_json(&task)))
}

#[instrument(skip_all, fields(name = %name, task_id = %task_id, agent = %agent_id.0))]
async fn start_plan_task(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path((name, task_id)): Path<(String, String)>,
    Json(req): Json<StartTaskRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    use agentstategraph_tasks::TaskId;
    let _ = req.reason; // reserved for future richer blame (annotation)
    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
    let task = store
        .start_task(&req.ref_name, &name, &TaskId(task_id))
        .map_err(substrate_error_to_response)?;
    // Non-blocking warning if other tasks in this plan are already in progress.
    let warning = plan_tools::active_task_warning(&store, &req.ref_name, &name, &task.id);
    s.sessions.mark_all_dirty();
    Ok(Json(plan_tools::task_to_json_with_warning(&task, warning)))
}

#[instrument(skip_all, fields(name = %name, task_id = %task_id, agent = %agent_id.0))]
async fn complete_plan_task(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path((name, task_id)): Path<(String, String)>,
    Json(req): Json<CompleteTaskRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    use agentstategraph_tasks::TaskId;
    let _ = req.reason;
    let proof = plan_tools::parse_proof(&req.proof.kind, &req.proof.value, req.proof.note)
        .map_err(plan_error_to_response)?;
    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
    let task = store
        .complete_task(&req.ref_name, &name, &TaskId(task_id.clone()), proof)
        .map_err(substrate_error_to_response)?;
    s.sessions.mark_all_dirty();
    let mut out = plan_tools::task_to_json(&task);
    // Remind the caller if finishing this task satisfies a task elsewhere.
    let links = read_satisfies(&repo, &req.ref_name, &name, &task_id);
    if !links.is_empty()
        && let Some(obj) = out.as_object_mut()
    {
        obj.insert("satisfies".to_string(), serde_json::json!(links));
    }
    Ok(Json(out))
}

#[instrument(skip_all, fields(name = %name, task_id = %task_id, agent = %agent_id.0))]
async fn abandon_plan_task(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path((name, task_id)): Path<(String, String)>,
    Json(req): Json<AbandonTaskRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    use agentstategraph_tasks::TaskId;
    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
    let task = store
        .abandon_task(&req.ref_name, &name, &TaskId(task_id), &req.reason)
        .map_err(substrate_error_to_response)?;
    s.sessions.mark_all_dirty();
    Ok(Json(plan_tools::task_to_json(&task)))
}

#[instrument(skip_all, fields(name = %name, ref_name = %q.ref_name, assigned_to = q.assigned_to.as_deref().unwrap_or("")))]
async fn next_plan_task(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path(name): Path<String>,
    Query(q): Query<NextTaskQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let store = plan_tools::make_store(repo.clone(), DEFAULT_AGENT_ID);
    let assignee = match q.assigned_to.as_deref() {
        Some("me") => Some(agent_id.0.clone()),
        Some(x) if !x.is_empty() => Some(x.to_string()),
        _ => None,
    };
    // Substrate's next_task_for takes a single include_unassigned flag.
    // Preserve CTXone's historical semantics: assigned_only=true forces
    // unassigned tasks out regardless of include_unassigned.
    let include_unassigned = q.include_unassigned && !q.assigned_only;
    // `order` = first unstarted by id (sequential); default `priority`.
    let task = if q.mode.as_deref() == Some("order") {
        plan_tools::next_task_ordered(
            &store,
            &q.ref_name,
            &name,
            assignee.as_deref(),
            include_unassigned,
        )
        .map_err(substrate_error_to_response)?
    } else {
        store
            .next_task_for(&q.ref_name, &name, assignee.as_deref(), include_unassigned)
            .map_err(substrate_error_to_response)?
    };
    // Surface active work separately from the next unstarted task.
    let in_progress = plan_tools::in_progress_tasks(&store, &q.ref_name, &name)
        .map_err(substrate_error_to_response)?;
    let body = match task {
        None => serde_json::json!({ "task": null, "in_progress": in_progress }),
        Some(t) => {
            serde_json::json!({ "task": plan_tools::task_to_json(&t), "in_progress": in_progress })
        }
    };
    Ok(Json(body))
}

#[instrument(skip_all, fields(name = %name, agent = %agent_id.0))]
async fn archive_plan(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path(name): Path<String>,
    Query(q): Query<OptionalRefQuery>,
    body: Option<Json<ArchiveBody>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Branch resolution precedence: explicit `?ref=` wins; then the JSON
    // body's `ref` (legacy CLIs); then the default branch. Resolving the
    // branch exactly like show/list/complete is the whole fix — reading it
    // only from the query with a silent "main" default made archives on any
    // other branch 404.
    let ref_name = q
        .ref_name
        .or_else(|| body.and_then(|Json(b)| b.ref_name))
        .unwrap_or_else(default_ref);
    let repo = s.repo_for(&ns)?;
    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
    let plan = store
        .archive_plan(&ref_name, &name)
        .map_err(substrate_error_to_response)?;
    s.sessions.mark_all_dirty();
    Ok(Json(plan_tools::plan_to_json(&plan, &[], false)))
}

#[derive(Deserialize)]
struct MovePlanBody {
    /// Branch the plan should be moved onto.
    target_ref: String,
}

#[instrument(skip_all, fields(name = %name, ref_name = %q.ref_name, agent = %agent_id.0))]
async fn move_plan_handler(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path(name): Path<String>,
    Query(q): Query<RefQuery>,
    Json(body): Json<MovePlanBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
    let result = plan_tools::move_plan(&repo, &store, &q.ref_name, &body.target_ref, &name)
        .map_err(plan_error_to_response)?;
    s.sessions.mark_all_dirty();
    Ok(Json(serde_json::json!({
        "plan": plan_tools::plan_to_json(&result.plan, &[], false),
        "source_ref": result.source_ref,
        "target_ref": result.target_ref,
        "task_count": result.task_count,
    })))
}

#[derive(Deserialize, Default)]
struct ForceCompleteBody {
    /// Reason recorded on every still-open task. Optional — falls
    /// back to the standard "Plan force-completed by user" string.
    #[serde(default)]
    reason: Option<String>,
}

#[instrument(skip_all, fields(name = %name, ref_name = %q.ref_name, agent = %agent_id.0))]
async fn force_complete_plan(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path(name): Path<String>,
    Query(q): Query<RefQuery>,
    body: Option<Json<ForceCompleteBody>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
    let reason = body.and_then(|Json(b)| b.reason);
    let result = plan_tools::force_complete_plan(&store, &q.ref_name, &name, reason)
        .map_err(plan_error_to_response)?;
    let tasks = store.list_tasks(&q.ref_name, &name).unwrap_or_default();
    s.sessions.mark_all_dirty();
    Ok(Json(serde_json::json!({
        "plan": plan_tools::plan_to_json(&result.plan, &tasks, true),
        "abandoned_task_ids": result.abandoned_task_ids,
    })))
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
    ns: NamespaceId,
    Query(q): Query<ListTaintsQuery>,
) -> Result<Json<ListTaintsResponse>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let kind = parse_kind(q.kind.as_deref())?;
    let taints = repo
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
    ns: NamespaceId,
    Query(q): Query<CheckTaintQuery>,
) -> Result<Json<CheckTaintResponse>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let check = repo
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
        Some(t) => (
            Some(taint_effect_str(t.effect).to_string()),
            Some(t.id.clone()),
        ),
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
    ns: NamespaceId,
    Json(body): Json<ApplyTaintBody>,
) -> Result<Json<ApplyTaintResponse>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    use agentstategraph_taint::{QuarantineParams, TaintKind, TaintParams, WatchParams};
    let kind = parse_kind(Some(&body.kind))?
        .ok_or((StatusCode::BAD_REQUEST, "kind required".to_string()))?;
    let severity = parse_severity(body.severity.as_deref())?;
    let ref_name = body.ref_name.unwrap_or_else(|| "main".to_string());
    let now = chrono::Utc::now();

    let taint_id = match kind {
        TaintKind::Taint => {
            let effect = parse_effect(&body.effect)?;
            repo.taint(
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
        TaintKind::Quarantine => repo.quarantine(
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
        TaintKind::Watch => repo.watch_path(
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
    ns: NamespaceId,
    Path(id): Path<String>,
    Json(body): Json<RemoveTaintBody>,
) -> Result<Json<RemoveTaintResponse>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    use agentstategraph_taint::{TaintKind, UntaintParams, UnwatchParams};
    let taint = repo
        .get_taint(&id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, format!("taint not found: {id}")))?;
    let ref_name = body.ref_name.unwrap_or_else(|| "main".to_string());

    match taint.kind {
        TaintKind::Taint => repo.untaint(
            &ref_name,
            &taint.path,
            &taint.name,
            UntaintParams {
                reason: body.reason,
                proof: None,
                agent_id: body.agent_id,
            },
        ),
        TaintKind::Quarantine => repo.unquarantine(
            &ref_name,
            &taint.path,
            &taint.name,
            UntaintParams {
                reason: body.reason,
                proof: None,
                agent_id: body.agent_id,
            },
        ),
        TaintKind::Watch => repo.unwatch(
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
        Some(other) => Err((StatusCode::BAD_REQUEST, format!("invalid kind: {other}"))),
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
        other => Err((StatusCode::BAD_REQUEST, format!("invalid effect: {other}"))),
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

/// Max bytes for a whole-session bulk turn write. Generous — it is one
/// transcript — but bounded so a runaway payload can't exhaust memory.
const SESSION_TURNS_BULK_MAX_BYTES: usize = 128 * 1024 * 1024;

/// `PUT /api/sessions/{sid}/turns` — write ALL of a session's turns in ONE
/// commit, replacing the turns subtree.
///
/// The per-turn endpoint (`.../turns/{idx}`) is one graph commit per turn, so
/// a 900-turn session was 900 write-transactions against a large db — the
/// dominant cost of the multi-hour sync. This collapses them to one commit.
/// Body is the turns map `{ "t0000": {…}, "t0001": {…}, … }`.
///
/// A full replace is correct for how ingest uses it: a session is only
/// re-written when its transcript changed (the fingerprint skip), and then
/// every turn is re-derived, so there is nothing to preserve.
async fn put_session_turns_bulk(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(sid): Path<String>,
    agent_id: AgentId,
    Query(q): Query<SessionTurnQuery>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !body.is_object() {
        return Err((
            StatusCode::BAD_REQUEST,
            "bulk turns body must be a map { tNNNN: turn }".to_string(),
        ));
    }
    let bytes = serde_json::to_vec(&body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    if bytes.len() > SESSION_TURNS_BULK_MAX_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "bulk turns payload {} bytes exceeds {} byte cap",
                bytes.len(),
                SESSION_TURNS_BULK_MAX_BYTES
            ),
        ));
    }
    let repo = s.repo_for(&ns)?;
    let path = format!("/sessions/{}/turns", sid);
    let count = body.as_object().map(|m| m.len()).unwrap_or(0);
    let opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Custom("Observe".to_string()),
        format!("capture session {} ({} turns)", sid, count),
    )
    .with_tags(vec![
        format!("session:{}", sid),
        "kind:full-turn".to_string(),
    ]);
    let commit_id = repo
        .set_json(&q.ref_name, &path, &body, opts)
        .map_err(internal_error)?;
    s.sessions.mark_all_dirty();
    Ok(Json(serde_json::json!({
        "status": "ok",
        "ref": q.ref_name,
        "turns": count,
        "commit_id": format!("{}", commit_id.short()),
    })))
}

async fn put_session_turn(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path((sid, idx)): Path<(String, u32)>,
    agent_id: AgentId,
    Query(q): Query<SessionTurnQuery>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let bytes = serde_json::to_vec(&body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    if bytes.len() > SESSION_TURNS_MAX_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "turn payload {} bytes exceeds {} byte cap",
                bytes.len(),
                SESSION_TURNS_MAX_BYTES
            ),
        ));
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
    let commit_id = repo
        .set_json(&q.ref_name, &path, &body, opts)
        .map_err(internal_error)?;
    // Session writes change the graph, so cached flat sizes are stale. The
    // other write paths have always done this; these four did not, which left
    // token-savings figures computed against a tree that no longer existed.
    s.sessions.mark_all_dirty();
    Ok(Json(serde_json::json!({
        "status": "ok",
        "ref": q.ref_name,
        "path": path,
        "commit_id": format!("{}", commit_id.short()),
    })))
}

async fn get_session_turn(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path((sid, idx)): Path<(String, u32)>,
    Query(q): Query<SessionTurnQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let path = session_turn_path(&sid, idx);
    repo.get_json(&q.ref_name, &path)
        .map(Json)
        .map_err(internal_error)
}

async fn list_session_turns(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(sid): Path<String>,
    Query(q): Query<SessionTurnQuery>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let prefix = format!("/sessions/{}/turns", sid);
    let all = repo
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

// -- Session title (t-016) ----------------------------------------------

/// Graph path holding a session's human-readable title.
fn session_title_path(sid: &str) -> String {
    format!("/sessions/{}/title", sid)
}

/// Read a session's title from the graph, best-effort. Returns `None` when
/// the node is absent, unreadable, or not a string. Used to populate
/// `SessionSnapshot::name` on the stats endpoints.
fn read_session_title(repo: &Repository, ref_name: &str, sid: &str) -> Option<String> {
    repo.get_json(ref_name, &session_title_path(sid))
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|s| !s.is_empty())
}

/// `PUT /api/sessions/{sid}/title` — set (or overwrite) a session's title.
///
/// Body is a bare JSON string, e.g. `"Fix the flush-on-exit bug"`. Idempotent:
/// re-ingesting a transcript overwrites the prior title. Written into the
/// request's namespace at `/sessions/{sid}/title`.
async fn put_session_title(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(sid): Path<String>,
    agent_id: AgentId,
    Query(q): Query<SessionTurnQuery>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Accept a bare string, or an object with a `title` field, so callers can
    // POST either shape. Anything else is a 400.
    let title = match &body {
        serde_json::Value::String(sv) => sv.clone(),
        serde_json::Value::Object(m) => m
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "title object must carry a string `title` field".to_string(),
                )
            })?,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "title body must be a JSON string or {\"title\": \"…\"}".to_string(),
            ));
        }
    };
    let repo = s.repo_for(&ns)?;
    let path = session_title_path(&sid);
    let intent = format!("name session {}", sid);
    let opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Custom("Observe".to_string()),
        &intent,
    )
    .with_tags(vec![
        format!("session:{}", sid),
        "kind:session-title".to_string(),
    ]);
    let commit_id = repo
        .set_json(&q.ref_name, &path, &serde_json::json!(title), opts)
        .map_err(internal_error)?;
    // Session writes change the graph, so cached flat sizes are stale. The
    // other write paths have always done this; these four did not, which left
    // token-savings figures computed against a tree that no longer existed.
    s.sessions.mark_all_dirty();
    Ok(Json(serde_json::json!({
        "status": "ok",
        "ref": q.ref_name,
        "path": path,
        "title": title,
        "commit_id": format!("{}", commit_id.short()),
    })))
}

async fn get_session_title(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(sid): Path<String>,
    Query(q): Query<SessionTurnQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let path = session_title_path(&sid);
    repo.get_json(&q.ref_name, &path)
        .map(Json)
        .map_err(internal_error)
}

// -- Session meta: source + timestamps (t-021) --------------------------

/// Graph path holding a session's meta object `{source, started_at, updated_at}`.
fn session_meta_path(sid: &str) -> String {
    format!("/sessions/{}/meta", sid)
}

fn session_provenance_path(sid: &str) -> String {
    format!("/sessions/{}/provenance", sid)
}

fn session_burn_path(sid: &str) -> String {
    format!("/sessions/{}/burn", sid)
}

/// `PUT /api/sessions/{sid}/burn` — store the session's efficiency score.
///
/// Written at ingest so the dashboard's burn board reads a number rather than
/// re-scanning every transcript. Stored verbatim like meta/provenance, so the
/// CLI can add fields without a lockstep hub rebuild.
async fn put_session_burn(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(sid): Path<String>,
    agent_id: AgentId,
    Query(q): Query<SessionTurnQuery>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !body.is_object() {
        return Err((
            StatusCode::BAD_REQUEST,
            "burn body must be a JSON object {level, ratio?, …}".to_string(),
        ));
    }
    let repo = s.repo_for(&ns)?;
    let path = session_burn_path(&sid);
    let opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Custom("Observe".to_string()),
        format!("session burn {}", sid),
    )
    .with_tags(vec![
        format!("session:{}", sid),
        "kind:session-burn".to_string(),
    ]);
    let commit_id = repo
        .set_json(&q.ref_name, &path, &body, opts)
        .map_err(internal_error)?;
    s.sessions.mark_all_dirty();
    Ok(Json(serde_json::json!({
        "status": "ok",
        "ref": q.ref_name,
        "path": path,
        "commit_id": format!("{}", commit_id.short()),
    })))
}

/// Where a deleted session is remembered as deleted.
///
/// A top-level prefix mirroring `/plan_links`, deliberately NOT under
/// `/_meta/` — that subtree is reserved for migrations and is skipped by
/// `/api/import`, so a tombstone placed there would not survive an
/// export/import round trip.
fn session_tombstone_path(sid: &str) -> String {
    format!("/session_tombstones/{}", sid)
}

#[derive(Deserialize, Default)]
struct DeleteSessionQuery {
    #[serde(default = "default_ref")]
    ref_name: String,
    /// Why it was deleted, kept on the tombstone for later readers.
    #[serde(default)]
    reason: Option<String>,
    /// Delete the data but allow a future sync to re-import it.
    #[serde(default)]
    no_tombstone: bool,
}

/// `DELETE /api/sessions/{sid}` — remove a session and remember the deletion.
///
/// Two things have to happen together, and doing either alone leaves the hub
/// in a worse state than before:
///
/// 1. **The graph subtree.** `/sessions/{sid}` and everything under it goes in
///    a single commit — the tree delete is recursive, so a 200-turn session
///    costs one commit rather than the 200+ `forget` calls this previously
///    required.
/// 2. **The session registry.** Token stats live outside the graph; leaving
///    them behind shows the session in `/api/stats/sessions` with totals and
///    no title.
///
/// A tombstone at `/session_tombstones/{sid}` stops the next
/// `ctx ingest-session` re-importing it from the transcript still sitting on
/// disk. Without it "delete" means "delete until the next sync", which is not
/// a deletion. Pass `no_tombstone=true` to deliberately allow re-import.
async fn delete_session(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(sid): Path<String>,
    agent_id: AgentId,
    Query(q): Query<DeleteSessionQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let root = format!("/sessions/{}", sid);

    // Count what is there first, so the response can report what went — and
    // so a caller can tell "deleted 214 nodes" from "there was nothing here".
    let removed = repo
        .list_paths(&q.ref_name, &root, None)
        .map(|p| p.len())
        .unwrap_or(0);

    let opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Rollback,
        format!("delete session {}", sid),
    )
    .with_tags(vec![
        format!("session:{}", sid),
        "kind:session-delete".to_string(),
    ]);

    // A session with no nodes is not an error: the registry may still hold
    // stats for it, and the tombstone is still worth writing.
    let deleted = match repo.delete(&q.ref_name, &root, opts) {
        Ok(id) => Some(format!("{}", id.short())),
        Err(_) if removed == 0 => None,
        Err(e) => return Err(internal_error(e)),
    };

    let tombstoned = if q.no_tombstone {
        false
    } else {
        let t = serde_json::json!({
            "deleted_at": chrono::Utc::now().to_rfc3339(),
            "deleted_by": agent_id.0,
            "reason": q.reason,
            "namespace": ns.0,
        });
        let t_opts = CommitOptions::new(
            &agent_id.0,
            IntentCategory::Custom("Observe".to_string()),
            format!("tombstone session {}", sid),
        )
        .with_tags(vec![
            format!("session:{}", sid),
            "kind:session-tombstone".to_string(),
        ]);
        repo.set_json(&q.ref_name, &session_tombstone_path(&sid), &t, t_opts)
            .map_err(internal_error)?;
        true
    };

    let from_registry = s.sessions.remove(&sid, s.db_path.as_deref());
    // Cached flat sizes reference a tree that no longer has this session.
    s.sessions.mark_all_dirty();

    Ok(Json(serde_json::json!({
        "status": "ok",
        "session_id": sid,
        "namespace": ns.0,
        "ref": q.ref_name,
        "nodes_removed": removed,
        "commit_id": deleted,
        "tombstoned": tombstoned,
        "removed_from_registry": from_registry,
    })))
}

fn session_links_path(sid: &str) -> String {
    format!("/session_links/{}", sid)
}

/// `GET /api/namespaces` — every workspace, with its session count.
///
/// Closes a real gap: namespaces could only be listed indirectly via
/// `/api/projects`, and there was no `ctx workspace` command at all. This is
/// the direct enumeration agents and the CLI need to answer "which workspaces
/// exist and how much is in each".
async fn list_namespaces(State(s): State<HubState>) -> impl IntoResponse {
    let mut out = Vec::new();
    match s.repo.list_namespaces() {
        Ok(names) => {
            for name in names {
                let ns = name.as_str().to_string();
                let count = s
                    .repo_for(&NamespaceId(ns.clone()))
                    .map(|r| session_ids_with_nodes(&r).len())
                    .unwrap_or(0);
                out.push(serde_json::json!({ "namespace": ns, "sessions": count }));
            }
        }
        Err(e) => return internal_error(e).into_response(),
    }
    // `default` is always a workspace even if list_namespaces omits it (it is
    // the root, not a fork), so a caller always sees it.
    if !out.iter().any(|n| n["namespace"] == Namespace::DEFAULT) {
        let count = session_ids_with_nodes(&s.repo).len();
        out.insert(
            0,
            serde_json::json!({ "namespace": Namespace::DEFAULT, "sessions": count }),
        );
    }
    Json(serde_json::json!({ "namespaces": out })).into_response()
}

#[derive(Deserialize)]
struct AddSessionLinkRequest {
    plan: String,
    task: String,
    #[serde(default = "default_ref")]
    ref_name: String,
    /// Optional note recorded as the link's evidence.
    #[serde(default)]
    note: Option<String>,
}

/// `POST /api/sessions/{sid}/links` — manually associate a session with a task.
///
/// Complements `derive_links`: the derivation catches work a commit message
/// named, this catches work it did not (a refactor with no `(plan t-NNN)`
/// trailer, a session that discussed a task without committing). The task is
/// validated against this workspace and the link is merged into the same
/// `/session_links/{sid}` sidecar, so both sources read the same way.
async fn add_session_link(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(sid): Path<String>,
    agent_id: AgentId,
    Json(req): Json<AddSessionLinkRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;

    // Normalise the task ref the same way the derivation does, so a manual
    // `t-4` and a derived `t-004` are the same link.
    let task = normalize_task_ref(&req.task).ok_or((
        StatusCode::BAD_REQUEST,
        format!("task must look like 't-NNN', got '{}'", req.task),
    ))?;
    let plan = req.plan.trim().to_ascii_lowercase();

    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
    let validated = task
        .strip_prefix("t-")
        .and_then(|d| d.parse::<u32>().ok())
        .map(|n| {
            store
                .get_task(&req.ref_name, &plan, &agentstategraph_tasks::TaskId::new(n))
                .is_ok()
        })
        .unwrap_or(false);

    // Merge into the existing sidecar, deduped on (plan, task).
    let path = session_links_path(&sid);
    let mut node = repo
        .get_json(&req.ref_name, &path)
        .unwrap_or_else(|_| serde_json::json!({ "links": [] }));
    let links = node
        .get_mut("links")
        .and_then(|l| l.as_array_mut())
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "corrupt links node".to_string(),
        ))?;

    let already = links.iter().any(|l| l["plan"] == plan && l["task"] == task);
    if !already {
        links.push(serde_json::json!({
            "plan": plan,
            "task": task,
            "evidence": req.note.clone().unwrap_or_else(|| "manually linked".to_string()),
            "turn_index": serde_json::Value::Null,
            "validated": validated,
            "manual": true,
        }));
    }
    node["generated_at"] = serde_json::json!(chrono::Utc::now().to_rfc3339());

    let opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Custom("Link".to_string()),
        format!("link session {} to {}/{}", sid, plan, task),
    )
    .with_tags(vec![
        format!("session:{}", sid),
        "kind:session-links".to_string(),
    ]);
    repo.set_json(&req.ref_name, &path, &node, opts)
        .map_err(internal_error)?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "session_id": sid,
        "plan": plan,
        "task": task,
        "validated": validated,
        "already_linked": already,
    })))
}

/// Public wrapper so the MCP tools share one task-ref normaliser with the
/// HTTP handlers rather than drifting a second copy.
pub fn normalize_task_ref_pub(raw: &str) -> Option<String> {
    normalize_task_ref(raw)
}

/// `t-4` / `T-004` / `t-004` → canonical `t-NNN`, or None if not a task ref.
fn normalize_task_ref(raw: &str) -> Option<String> {
    let num = raw
        .trim()
        .strip_prefix("t-")
        .or_else(|| raw.trim().strip_prefix("T-"))?;
    let n: u32 = num.parse().ok()?;
    Some(format!("t-{n:03}"))
}

/// `POST /api/sessions/{sid}/derive_links` — find the plan tasks this session
/// worked on and persist them at `/session_links/{sid}`.
///
/// Replaces the client-side regex (recomputed per render, persisted nowhere,
/// validated against nothing) with a whole-session server pass: it reads every
/// turn once, extracts commit task-refs, validates each against the plan/task
/// actually existing in this namespace, and writes a sidecar the whole system
/// can read. Re-runnable and idempotent — a re-derive overwrites the node.
async fn derive_session_links(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(sid): Path<String>,
    agent_id: AgentId,
    Query(q): Query<SessionTurnQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;

    // The turns subtree: `/sessions/{sid}/turns` = { tNNNN: {tool_calls_raw…} }.
    let turns = repo
        .get_json(&q.ref_name, &format!("/sessions/{}/turns", sid))
        .map_err(|_| {
            (
                StatusCode::NOT_FOUND,
                format!("no turns for session {}", sid),
            )
        })?;

    // Flatten to (turn_index, command) candidates. A turn's index is parsed
    // from its `tNNNN` key so a link points back at the right turn.
    let mut commands: Vec<(usize, String)> = Vec::new();
    if let Some(map) = turns.as_object() {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        for k in keys {
            let idx = k
                .trim_start_matches('t')
                .parse::<usize>()
                .unwrap_or(usize::MAX);
            if let Some(raw) = map[k].get("tool_calls_raw").and_then(|v| v.as_array()) {
                for call in raw {
                    if let Some(cmd) = call
                        .get("input")
                        .and_then(|i| i.get("command"))
                        .and_then(|c| c.as_str())
                    {
                        commands.push((idx, cmd.to_string()));
                    }
                }
            }
        }
    }
    let candidates: Vec<session_links::Candidate> = commands
        .iter()
        .map(|(i, c)| session_links::Candidate {
            turn_index: *i,
            command: c,
        })
        .collect();

    // Validation closure: a plan/task exists iff the task store resolves it in
    // this namespace. Built once and reused across every candidate.
    let store = plan_tools::make_store(repo.clone(), &agent_id.0);
    let ref_name = q.ref_name.clone();
    let links = session_links::derive(&candidates, |plan, task| {
        // `task` is normalized `t-NNN`; the store keys on the numeric id.
        let Some(n) = task.strip_prefix("t-").and_then(|d| d.parse::<u32>().ok()) else {
            return false;
        };
        store
            .get_task(&ref_name, plan, &agentstategraph_tasks::TaskId::new(n))
            .is_ok()
    });

    let validated = links.iter().filter(|l| l.validated).count();
    let node = serde_json::json!({
        "links": links,
        "generated_at": chrono::Utc::now().to_rfc3339(),
    });

    // Nothing found is a valid, writeable result: it records that the session
    // was scanned and had no task refs, so a reader need not re-derive.
    let opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Custom("Observe".to_string()),
        format!("session links {}", sid),
    )
    .with_tags(vec![
        format!("session:{}", sid),
        "kind:session-links".to_string(),
    ]);
    repo.set_json(&q.ref_name, &session_links_path(&sid), &node, opts)
        .map_err(internal_error)?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "session_id": sid,
        "namespace": ns.0,
        "links": links.len(),
        "validated": validated,
    })))
}

#[derive(Deserialize)]
struct MoveSessionRequest {
    to_namespace: String,
    #[serde(default = "default_ref")]
    ref_name: String,
}

#[derive(Deserialize)]
struct MovePlanRequest {
    to_namespace: String,
    /// Source ref to read the plan from. `rename = "ref"` so callers send the
    /// short `ref` key (matching every other endpoint).
    #[serde(default = "default_ref", rename = "ref")]
    ref_name: String,
    /// Destination ref in the target namespace. Defaults to the source ref; the
    /// plan migration passes "main" so plans land on each target workspace's
    /// trunk regardless of which feature branch they came from in `default`.
    #[serde(default)]
    to_ref: Option<String>,
}

/// `POST /api/plans/{name}/relocate` — move a plan to another workspace.
///
/// Plans collapsed into `default` for the same reason sessions did — created
/// before per-workspace routing — so this is the plan analogue of
/// `move_session`. Not named `/move`: `/api/plans/{name}/move` already exists
/// for reordering a task within a plan.
///
/// A plan is two subtrees, and both must travel together or the plan arrives
/// broken: `/plans/{name}` (the `_meta` + tasks the substrate owns) and
/// `/plan_links/{name}` (the cross-plan "satisfies" sidecars CTXone keeps
/// alongside). The original of each is deleted, so this is a move, not a copy.
///
/// Idempotent by the plan's `created_at`: a target that already holds an
/// equal-or-newer copy is left untouched, so a re-run cannot clobber a plan
/// that moved on in its new home.
async fn relocate_plan(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(name): Path<String>,
    agent_id: AgentId,
    Json(req): Json<MovePlanRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if req.to_namespace == ns.0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "to_namespace is the current namespace; nothing to move".to_string(),
        ));
    }
    let src = s.repo_for(&ns)?;
    let dst = s.repo_for(&NamespaceId(req.to_namespace.clone()))?;
    let plan_root = format!("{}/{}", plan_tools::PLANS_PREFIX, name);

    // The plan subtree. Absent = 404: moving a plan that isn't here is a
    // caller error, not a silent success.
    let plan_tree = src.get_tree(&req.ref_name, &plan_root).map_err(|_| {
        (
            StatusCode::NOT_FOUND,
            format!("no plan {} in {}", name, ns.0),
        )
    })?;

    // Destination ref — the source ref unless the caller consolidates onto a
    // different trunk (the migration passes "main").
    let dst_ref = req.to_ref.clone().unwrap_or_else(|| req.ref_name.clone());

    // Idempotency on the plan's own `_meta.created_at` (plans are immutable in
    // identity; created_at is the stable key). If the target already holds a
    // plan by this name, don't overwrite.
    let dst_has = dst.get_tree(&dst_ref, &plan_root).is_ok();

    if !dst_has {
        let opts = CommitOptions::new(
            &agent_id.0,
            IntentCategory::Custom("Migrate".to_string()),
            format!("move plan {} to {}", name, req.to_namespace),
        )
        .with_tags(vec![format!("plan:{}", name), "kind:plan-move".to_string()]);
        dst.set_json(&dst_ref, &plan_root, &plan_tree, opts)
            .map_err(internal_error)?;

        // Carry the cross-plan link sidecars, if any. Absent is fine — most
        // plans have none.
        let links_root = format!("/plan_links/{}", name);
        if let Ok(links) = src.get_tree(&req.ref_name, &links_root) {
            let link_opts = CommitOptions::new(
                &agent_id.0,
                IntentCategory::Custom("Migrate".to_string()),
                format!("move plan links {} to {}", name, req.to_namespace),
            )
            .with_tags(vec![format!("plan:{}", name), "kind:plan-move".to_string()]);
            dst.set_json(&dst_ref, &links_root, &links, link_opts)
                .map_err(internal_error)?;
        }
    }

    // Remove the originals — plan subtree and its links. This is what makes it
    // a move and stops the migration leaving duplicates in `default`.
    let del_opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Rollback,
        format!("remove plan {} after move to {}", name, req.to_namespace),
    )
    .with_tags(vec![
        format!("plan:{}", name),
        "kind:plan-move-cleanup".to_string(),
    ]);
    src.delete(&req.ref_name, &plan_root, del_opts)
        .map_err(internal_error)?;
    let links_root = format!("/plan_links/{}", name);
    if src.get_tree(&req.ref_name, &links_root).is_ok() {
        let del_links = CommitOptions::new(
            &agent_id.0,
            IntentCategory::Rollback,
            format!("remove plan links {} after move", name),
        )
        .with_tags(vec![
            format!("plan:{}", name),
            "kind:plan-move-cleanup".to_string(),
        ]);
        let _ = src.delete(&req.ref_name, &links_root, del_links);
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "plan": name,
        "from": ns.0,
        "to": req.to_namespace,
        "wrote_target": !dst_has,
        "target_already_had_it": dst_has,
    })))
}

/// `POST /api/sessions/{sid}/move` — relocate a session to another workspace.
///
/// The migration for sessions that were all imported into `default` before
/// per-transcript routing existed. Reads the `/sessions/{sid}` subtree from
/// the request namespace, writes it whole into `to_namespace`, then deletes
/// the original — the delete is the point, since re-ingesting alone would
/// leave the stale copy behind and produce duplicates.
///
/// Idempotent by `updated_at`: if the target already holds a copy that is
/// equal or newer, the source is removed but the target is left untouched, so
/// a re-run cannot clobber a target that has moved on.
async fn move_session(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(sid): Path<String>,
    agent_id: AgentId,
    Json(req): Json<MoveSessionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if req.to_namespace == ns.0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "to_namespace is the current namespace; nothing to move".to_string(),
        ));
    }
    let src = s.repo_for(&ns)?;
    let dst = s.repo_for(&NamespaceId(req.to_namespace.clone()))?;
    let root = format!("/sessions/{}", sid);

    // The subtree to move. A session with no nodes here is a 404 — moving
    // nothing is a caller error worth surfacing, not a silent success.
    let subtree = src.get_tree(&req.ref_name, &root).map_err(|_| {
        (
            StatusCode::NOT_FOUND,
            format!("no session {} in {}", sid, ns.0),
        )
    })?;

    // Skip the write when the target already has an equal-or-newer copy, so a
    // re-run does not overwrite a session that was updated in its new home.
    let src_updated = subtree
        .get("meta")
        .and_then(|m| m.get("updated_at"))
        .and_then(|v| v.as_str());
    let dst_existing = dst.get_tree(&req.ref_name, &root).ok();
    let dst_updated = dst_existing
        .as_ref()
        .and_then(|t| t.get("meta"))
        .and_then(|m| m.get("updated_at"))
        .and_then(|v| v.as_str());
    let target_is_current = match (src_updated, dst_updated) {
        (Some(src_ts), Some(dst_ts)) => dst_ts >= src_ts,
        // Target has a copy but neither carries a timestamp: treat as current
        // rather than risk clobbering.
        (_, _) if dst_existing.is_some() => true,
        _ => false,
    };

    if !target_is_current {
        let opts = CommitOptions::new(
            &agent_id.0,
            IntentCategory::Custom("Migrate".to_string()),
            format!("move session {} to {}", sid, req.to_namespace),
        )
        .with_tags(vec![
            format!("session:{}", sid),
            "kind:session-move".to_string(),
        ]);
        dst.set_json(&req.ref_name, &root, &subtree, opts)
            .map_err(internal_error)?;
    }

    // Remove the original. This is what makes the move a move rather than a
    // copy, and what stops the migration leaving duplicates in `default`.
    let del_opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Rollback,
        format!("remove session {} after move to {}", sid, req.to_namespace),
    )
    .with_tags(vec![
        format!("session:{}", sid),
        "kind:session-move-cleanup".to_string(),
    ]);
    src.delete(&req.ref_name, &root, del_opts)
        .map_err(internal_error)?;

    s.sessions.mark_all_dirty();

    Ok(Json(serde_json::json!({
        "status": "ok",
        "session_id": sid,
        "from": ns.0,
        "to": req.to_namespace,
        "wrote_target": !target_is_current,
        "target_was_current": target_is_current,
    })))
}

/// `GET /api/session_tombstones` — session ids that must not be re-imported.
///
/// Read once per run by `ctx ingest-session`; returning the whole list beats
/// probing per session, since a machine-wide sync considers hundreds.
async fn list_session_tombstones(
    State(s): State<HubState>,
    ns: NamespaceId,
    Query(q): Query<SessionTurnQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    // Absent prefix = nothing deleted yet, which is not an error.
    let paths = repo
        .list_paths(&q.ref_name, "/session_tombstones", None)
        .unwrap_or_default();
    let ids: Vec<String> = paths
        .iter()
        .filter_map(|p| p.strip_prefix("/session_tombstones/"))
        // list_paths returns leaves, so a tombstone's fields appear as
        // `/session_tombstones/<sid>/deleted_at`; keep the id segment only.
        .filter_map(|rest| rest.split('/').next())
        .map(str::to_string)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(Json(serde_json::json!({ "sessions": ids })))
}

/// Read a session's meta (source / started_at / updated_at) from the graph,
/// best-effort. Any missing piece is `None`. Populates the matching
/// `SessionSnapshot` fields so the Lens can filter by agent and sort by date.
struct SessionMeta {
    source: Option<String>,
    started_at: Option<String>,
    updated_at: Option<String>,
    models_used: Vec<String>,
}

fn read_session_meta(repo: &Repository, ref_name: &str, sid: &str) -> SessionMeta {
    let Ok(v) = repo.get_json(ref_name, &session_meta_path(sid)) else {
        return SessionMeta {
            source: None,
            started_at: None,
            updated_at: None,
            models_used: Vec::new(),
        };
    };
    let str_field = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty())
    };
    let models_used = v
        .get("models_used")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|m| m.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    SessionMeta {
        source: str_field("source"),
        started_at: str_field("started_at"),
        updated_at: str_field("updated_at"),
        models_used,
    }
}

/// `PUT /api/sessions/{sid}/provenance` — where a session ran.
///
/// Body is `{cwds?, branches?, namespace?, project_id?, source_file?,
/// archived?}` where `branches` is every branch the session touched, with
/// per-branch turn counts. Deliberately a separate node from `meta`: `meta`
/// has a fixed shape the Lens filters consume and a different writer.
///
/// Stored verbatim, like `meta`, so a newer CLI can add fields without
/// needing the hub rebuilt in lockstep.
async fn put_session_provenance(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(sid): Path<String>,
    agent_id: AgentId,
    Query(q): Query<SessionTurnQuery>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !body.is_object() {
        return Err((
            StatusCode::BAD_REQUEST,
            "provenance body must be a JSON object {cwds?, branches?, …}".to_string(),
        ));
    }
    let repo = s.repo_for(&ns)?;
    let path = session_provenance_path(&sid);
    let opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Custom("Observe".to_string()),
        format!("session provenance {}", sid),
    )
    .with_tags(vec![
        format!("session:{}", sid),
        "kind:session-provenance".to_string(),
    ]);
    let commit_id = repo
        .set_json(&q.ref_name, &path, &body, opts)
        .map_err(internal_error)?;
    // Session writes change the graph, so cached flat sizes are stale. The
    // other write paths have always done this; these four did not, which left
    // token-savings figures computed against a tree that no longer existed.
    s.sessions.mark_all_dirty();
    Ok(Json(serde_json::json!({
        "status": "ok",
        "ref": q.ref_name,
        "path": path,
        "commit_id": format!("{}", commit_id.short()),
    })))
}

/// `PUT /api/sessions/{sid}/meta` — set a session's meta object. Body is
/// `{source?, started_at?, updated_at?}`. Idempotent; written into the
/// request namespace at `/sessions/{sid}/meta`. Written by `ctx
/// ingest-session` alongside the title.
async fn put_session_meta(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(sid): Path<String>,
    agent_id: AgentId,
    Query(q): Query<SessionTurnQuery>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !body.is_object() {
        return Err((
            StatusCode::BAD_REQUEST,
            "meta body must be a JSON object {source?, started_at?, updated_at?}".to_string(),
        ));
    }
    let repo = s.repo_for(&ns)?;
    let path = session_meta_path(&sid);
    let opts = CommitOptions::new(
        &agent_id.0,
        IntentCategory::Custom("Observe".to_string()),
        format!("session meta {}", sid),
    )
    .with_tags(vec![
        format!("session:{}", sid),
        "kind:session-meta".to_string(),
    ]);
    let commit_id = repo
        .set_json(&q.ref_name, &path, &body, opts)
        .map_err(internal_error)?;
    // Session writes change the graph, so cached flat sizes are stale. The
    // other write paths have always done this; these four did not, which left
    // token-savings figures computed against a tree that no longer existed.
    s.sessions.mark_all_dirty();
    Ok(Json(serde_json::json!({
        "status": "ok",
        "ref": q.ref_name,
        "path": path,
        "commit_id": format!("{}", commit_id.short()),
    })))
}

// -- Session sync (t-019) ----------------------------------------------

/// Wall-clock cap on a full `ctx ingest-session --all` run before the
/// endpoint gives up and returns 504. A whole-machine sync of many large
/// transcripts is I/O bound but should never run for minutes.
const SESSION_SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// `POST /api/sessions/sync` — re-pull ALL local Claude Code transcripts into
/// this hub so the Sessions view reflects the latest turns, titles, and token
/// metrics.
///
/// **Local-only.** This spawns the co-located `ctx` CLI
/// (`ctx ingest-session --all --full-turn --server <self> --namespace <ns>`),
/// which reads `~/.claude/projects/*` on the *hub's* machine and POSTs the
/// parsed turns back into this hub. It is only meaningful when the transcripts
/// live on the same box as the hub; there is no remote-transcript path. When
/// `~/.claude/projects` is empty the CLI no-ops cleanly and this returns zeros.
///
/// The CLI's final stdout line is a JSON object
/// `{"sessions":N,"turns":M,"tokens":T}` which we parse and echo back along
/// with `elapsed_ms`. Timeout → 504; missing `ctx` binary → 400 (set
/// `--ctx-binary`); non-zero CLI exit → 500 with the stderr tail.
async fn sync_sessions(
    State(s): State<HubState>,
    // Deliberately not used to target the import: sync is machine-wide and
    // each transcript routes to its own workspace. Kept so the extractor still
    // validates the header and the handler signature documents the choice.
    _ns: NamespaceId,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ctx_bin = s.ctx_binary.clone().unwrap_or_else(|| "ctx".to_string());
    // Fall back to the conventional default port if the binary wasn't built
    // with a self URL (library/test callers). The Hub binary always sets this.
    let base_url = s
        .self_base_url
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:3001".to_string());

    let start = std::time::Instant::now();

    let mut cmd = tokio::process::Command::new(&ctx_bin);
    cmd.arg("ingest-session")
        .arg("--all")
        .arg("--full-turn")
        // Every installed agent, not just Claude Code. The CLI's `--source`
        // default is `claude`, so omitting this silently skipped Codex —
        // a fully implemented source — and any source added later. "Sync"
        // that quietly covers one agent is worse than no sync at all.
        .arg("--source")
        .arg("all")
        .arg("--server")
        .arg(&base_url)
        // NOT this request's namespace. `--all` walks every project on the
        // machine, so pinning one namespace for the whole run is what
        // collapsed transcripts from N repos into a single workspace. The CLI
        // now routes each transcript by its own recorded `cwd`.
        //
        // `default` is still passed explicitly, for two reasons: it stops the
        // CLI re-detecting a project from the *hub daemon's* cwd (which has
        // nothing to do with the transcripts being imported), and it pins
        // where unroutable sessions land — otherwise they would drift into
        // whichever workspace the user happened to be viewing when they hit
        // sync.
        .arg("--namespace")
        .arg("default")
        // Pin the ref to `main`. Session titles/turns are read from `main`
        // (see `read_session_title` / `default_ref`), and an EXPLICIT --branch
        // also suppresses the CLI's git-branch mirroring — which would
        // otherwise divert these writes onto the hub's current git branch when
        // a namespace is set, leaving the Sessions view with null names.
        .arg("--branch")
        .arg("main")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "ctx binary not found ('{ctx_bin}'); set --ctx-binary. Session sync runs \
                     the local CLI and requires a co-located hub."
                ),
            ));
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to spawn ctx: {e}"),
            ));
        }
    };

    let output = match tokio::time::timeout(SESSION_SYNC_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("ctx ingest-session failed: {e}"),
            ));
        }
        Err(_) => {
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                format!(
                    "session sync timed out after {}s (ctx ingest-session --all still running)",
                    SESSION_SYNC_TIMEOUT.as_secs()
                ),
            ));
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr.lines().rev().take(5).collect::<Vec<_>>().join(" | ");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "ctx ingest-session exited {}: {}",
                output
                    .status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string()),
                tail
            ),
        ));
    }

    // The CLI prints a machine-readable JSON object as its final stdout line
    // under --all. Scan from the bottom for the first line that parses as an
    // object carrying `sessions` (prose lines above won't match).
    let summary = stdout.lines().rev().find_map(|line| {
        serde_json::from_str::<serde_json::Value>(line.trim())
            .ok()
            .filter(|v| v.get("sessions").is_some())
    });

    let (sessions, turns, tokens) = match summary {
        Some(v) => (
            v.get("sessions").and_then(|x| x.as_u64()).unwrap_or(0),
            v.get("turns").and_then(|x| x.as_u64()).unwrap_or(0),
            v.get("tokens").and_then(|x| x.as_u64()).unwrap_or(0),
        ),
        None => (0, 0, 0),
    };

    let elapsed_ms = start.elapsed().as_millis() as u64;
    info!(
        sessions,
        turns,
        tokens,
        elapsed_ms,
        // The namespace the request arrived on, for correlation only — the
        // import itself routes per transcript and may write many workspaces.
        requested_namespace = %_ns.0,
        "session sync complete"
    );

    Ok(Json(serde_json::json!({
        "status": "ok",
        "sessions": sessions,
        "turns": turns,
        "tokens": tokens,
        "elapsed_ms": elapsed_ms,
    })))
}

// -- Reminder endpoints -------------------------------------------------

fn reminder_error_to_response(e: reminder_tools::ReminderToolError) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}

async fn list_reminders_handler(
    State(s): State<HubState>,
    ns: NamespaceId,
    Query(q): Query<reminder_tools::ReminderListParams>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let mgr = reminder_tools::make_manager(repo.clone());
    let reminders = reminder_tools::list_reminders(&mgr, q).map_err(reminder_error_to_response)?;
    Ok(Json(
        reminders
            .iter()
            .map(reminder_tools::reminder_to_json)
            .collect(),
    ))
}

async fn create_reminder_handler(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Json(req): Json<reminder_tools::ReminderCreateParams>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let mgr = reminder_tools::make_manager(repo.clone());
    let r = reminder_tools::create_reminder(&mgr, req, &agent_id.0)
        .map_err(reminder_error_to_response)?;
    Ok((
        StatusCode::CREATED,
        Json(reminder_tools::reminder_to_json(&r)),
    ))
}

async fn remind_me_handler(
    State(s): State<HubState>,
    ns: NamespaceId,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let mgr = reminder_tools::make_manager(repo.clone());
    let reminders = mgr
        .remind_me()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        reminders
            .iter()
            .map(reminder_tools::reminder_to_json)
            .collect(),
    ))
}

async fn get_reminder_handler(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let mgr = reminder_tools::make_manager(repo.clone());
    match mgr.get(&id) {
        Ok(r) => Ok(Json(reminder_tools::reminder_to_json(&r))),
        Err(agentstategraph_reminders::ReminderError::NotFound(_)) => Err((
            StatusCode::NOT_FOUND,
            format!("reminder '{}' not found", id),
        )),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

#[derive(Deserialize)]
struct SnoozeBody {
    until: String,
}

async fn snooze_reminder_handler(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(id): Path<String>,
    Json(body): Json<SnoozeBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let mgr = reminder_tools::make_manager(repo.clone());
    let until = reminder_tools::parse_datetime(&body.until).map_err(reminder_error_to_response)?;
    let r = mgr
        .snooze(&id, until)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(reminder_tools::reminder_to_json(&r)))
}

#[derive(Deserialize)]
struct ApproveBody {
    approved_by: Option<String>,
}

async fn approve_reminder_handler(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path(id): Path<String>,
    body: Option<Json<ApproveBody>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let mgr = reminder_tools::make_manager(repo.clone());
    let approver = body.and_then(|b| b.0.approved_by).unwrap_or(agent_id.0);
    let r = mgr
        .approve(&id, &approver)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(reminder_tools::reminder_to_json(&r)))
}

async fn cancel_reminder_handler(
    State(s): State<HubState>,
    ns: NamespaceId,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let mgr = reminder_tools::make_manager(repo.clone());
    let r = mgr
        .cancel(&id)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(reminder_tools::reminder_to_json(&r)))
}

#[derive(Deserialize)]
struct StartBody {
    agent_id: Option<String>,
}

async fn start_reminder_handler(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path(id): Path<String>,
    body: Option<Json<StartBody>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    let mgr = reminder_tools::make_manager(repo.clone());
    let acting_agent = body.and_then(|b| b.0.agent_id).unwrap_or(agent_id.0);
    let r = mgr
        .start(&id, &acting_agent)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(reminder_tools::reminder_to_json(&r)))
}

async fn record_reminder_handler(
    State(s): State<HubState>,
    ns: NamespaceId,
    agent_id: AgentId,
    Path(path_id): Path<String>,
    Json(mut req): Json<reminder_tools::ReminderRecordParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = s.repo_for(&ns)?;
    // Path parameter is authoritative; body id is optional/overridden.
    req.id = path_id;
    if req.agent_id.is_none() {
        req.agent_id = Some(agent_id.0);
    }
    let mgr = reminder_tools::make_manager(repo.clone());
    let r = reminder_tools::record_execution(&mgr, req, DEFAULT_AGENT_ID)
        .map_err(reminder_error_to_response)?;
    Ok(Json(reminder_tools::reminder_to_json(&r)))
}

// -- Admin endpoints --

#[derive(Deserialize, Default)]
struct AdminBackupRequest {
    /// Optional: override the snapshot suffix. Default = current UTC.
    suffix: Option<String>,
}

/// `POST /api/admin/backup` — VACUUM INTO `<db>.bak.<suffix>`. Returns
/// the snapshot path. Errors with 400 if storage isn't sqlite (no
/// path to back up). Used by `ctx db backup`.
#[instrument(skip_all)]
async fn admin_backup(
    State(s): State<HubState>,
    body: Option<Json<AdminBackupRequest>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let db_path = s.db_path.as_deref().ok_or((
        StatusCode::BAD_REQUEST,
        "backup requires sqlite storage (db_path is unset)".to_string(),
    ))?;
    let suffix = body
        .and_then(|b| b.0.suffix)
        .unwrap_or_else(crate::backup::iso_utc_compact);
    // VACUUM INTO can take a moment on a large db — run on a blocking
    // thread so we don't park the async runtime.
    let db_path_owned = db_path.to_string();
    let result =
        tokio::task::spawn_blocking(move || crate::backup::snapshot_now(&db_path_owned, &suffix))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {}", e)))?;

    let path = result.map_err(|msg| (StatusCode::INTERNAL_SERVER_ERROR, msg))?;
    Ok(Json(serde_json::json!({
        "status": "ok",
        "path": path.to_string_lossy(),
    })))
}

// -- Projects (namespace registry) --

/// The project registry lives in the Hub's sqlite file. Memory/postgres
/// backends have no registry — those requests get a 400, mirroring
/// `/api/admin/backup`.
fn registry_db(s: &HubState) -> Result<String, (StatusCode, String)> {
    s.db_path.clone().ok_or((
        StatusCode::BAD_REQUEST,
        "project registry requires sqlite storage (db_path is unset)".to_string(),
    ))
}

/// Serialize a project, annotating the pool-managed ASD repos whose db
/// files live under one of the project's local paths. The binding is
/// derived, not stored — registering an ASD repo under a project's path
/// is what binds it.
async fn project_to_json(s: &HubState, p: crate::project::Project) -> serde_json::Value {
    let mut asd_repos: Vec<String> = Vec::new();
    if let Some(pool) = &s.asd_pool {
        for (name, db_path) in pool.repo_paths().await {
            if p.local_paths.iter().any(|lp| db_path.starts_with(lp)) {
                asd_repos.push(name);
            }
        }
    }
    serde_json::json!({
        "id": p.id,
        "remote_url": p.remote_url,
        "namespace": p.namespace_id,
        "display_name": p.display_name,
        "created_at": p.created_at,
        "local_paths": p.local_paths,
        "asd_repos": asd_repos,
    })
}

#[derive(Deserialize)]
struct RegisterProjectRequest {
    /// Project id (kebab-case). Doubles as the namespace name unless
    /// `namespace` is given explicitly.
    id: String,
    remote_url: Option<String>,
    namespace: Option<String>,
    display_name: Option<String>,
    local_path: Option<String>,
}

#[instrument(skip_all, fields(id = %req.id))]
async fn register_project_handler(
    State(s): State<HubState>,
    Json(req): Json<RegisterProjectRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let db = registry_db(&s)?;
    let namespace = req.namespace.clone().unwrap_or_else(|| req.id.clone());

    // Create the ASG namespace first — Namespace::new validates the name
    // (ASCII alnum/-/_, 1..=64 bytes), and init() creates the namespace row
    // plus an initialized `main` branch so ref operations work immediately.
    // Both are idempotent on re-register.
    let ns =
        Namespace::new(namespace.as_str()).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    s.repo.fork_namespace(ns).init().map_err(internal_error)?;

    let remote = req
        .remote_url
        .as_deref()
        .map(crate::project::normalize_remote_url);
    crate::project::register_project(
        &db,
        &req.id,
        remote.as_deref(),
        &namespace,
        req.display_name.as_deref(),
        req.local_path.as_deref(),
    )
    .map_err(|e| {
        let msg = e.to_string();
        // Duplicate id / remote_url is a client error, not a 500.
        let status = if msg.contains("UNIQUE constraint failed") {
            StatusCode::CONFLICT
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        (status, msg)
    })?;

    info!(id = %req.id, namespace = %namespace, "project registered");
    let p = crate::project::resolve_by_id(&db, &req.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "project vanished after insert".to_string(),
        ))?;
    Ok(Json(project_to_json(&s, p).await))
}

async fn list_projects_handler(
    State(s): State<HubState>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    let db = registry_db(&s)?;
    let projects = crate::project::list_projects(&db)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::with_capacity(projects.len());
    for p in projects {
        out.push(project_to_json(&s, p).await);
    }
    Ok(Json(out))
}

async fn get_project_handler(
    State(s): State<HubState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let db = registry_db(&s)?;
    match crate::project::resolve_by_id(&db, &id) {
        Ok(Some(p)) => Ok(Json(project_to_json(&s, p).await)),
        Ok(None) => Err((StatusCode::NOT_FOUND, format!("project not found: {id}"))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

#[derive(Deserialize)]
struct AddProjectPathRequest {
    local_path: String,
}

async fn add_project_path_handler(
    State(s): State<HubState>,
    Path(id): Path<String>,
    Json(req): Json<AddProjectPathRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let db = registry_db(&s)?;
    // sqlite doesn't enforce the FK by default — check existence explicitly
    // so a typo'd id is a 404, not a silent orphan row.
    if crate::project::resolve_by_id(&db, &id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err((StatusCode::NOT_FOUND, format!("project not found: {id}")));
    }
    crate::project::add_local_path(&db, &id, &req.local_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let p = crate::project::resolve_by_id(&db, &id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, format!("project not found: {id}")))?;
    Ok(Json(project_to_json(&s, p).await))
}

#[derive(Deserialize)]
struct DetectQuery {
    cwd: String,
}

/// `GET /api/projects/detect?cwd=/abs/path` — run the detection chain
/// (`.ctxproject` walk-up, then git remote lookup) and report which
/// namespace a session started in that directory would land in.
async fn detect_project_handler(
    State(s): State<HubState>,
    Query(q): Query<DetectQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::project::DetectResult;
    let db = s.db_path.clone();
    // Detection shells out to git — keep it off the async runtime.
    let result = tokio::task::spawn_blocking(move || {
        crate::project::detect_project(std::path::Path::new(&q.cwd), db.as_deref())
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {}", e)))?;

    let json = match result {
        DetectResult::FoundByFile {
            project_id,
            namespace_id,
        } => serde_json::json!({
            "status": "found", "via": "ctxproject",
            "project_id": project_id, "namespace": namespace_id,
        }),
        DetectResult::FoundByRemote {
            project_id,
            namespace_id,
            remote_url,
        } => serde_json::json!({
            "status": "found", "via": "remote",
            "project_id": project_id, "namespace": namespace_id,
            "remote_url": remote_url,
        }),
        DetectResult::FoundByPath {
            project_id,
            namespace_id,
            local_path,
        } => serde_json::json!({
            "status": "found", "via": "local_path",
            "project_id": project_id, "namespace": namespace_id,
            "local_path": local_path,
        }),
        DetectResult::NotFound => serde_json::json!({
            "status": "not_found", "namespace": "default",
        }),
        DetectResult::RegistryUnavailable => serde_json::json!({
            "status": "registry_unavailable", "namespace": "default",
        }),
    };
    Ok(Json(json))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolve a namespace the way a real request would.
    async fn extract_ns(uri: &str, header: Option<&str>) -> String {
        use axum::extract::FromRequestParts;
        let mut req = axum::http::Request::builder().uri(uri);
        if let Some(h) = header {
            req = req.header("X-CTXone-Namespace", h);
        }
        let (mut parts, _) = req.body(()).unwrap().into_parts();
        NamespaceId::from_request_parts(&mut parts, &())
            .await
            .unwrap()
            .0
    }

    #[tokio::test]
    async fn namespace_query_param_beats_the_header() {
        // Load-bearing: session ingest retargets per request via the query
        // string while its client carries a fixed header, so the query must
        // win or every routed write would land in the client's namespace.
        assert_eq!(
            extract_ns("/api/state/main?namespace=ctxone", Some("default")).await,
            "ctxone"
        );
    }

    #[tokio::test]
    async fn namespace_falls_back_to_header_then_default() {
        assert_eq!(
            extract_ns("/api/state/main", Some("ctxone")).await,
            "ctxone"
        );
        assert_eq!(extract_ns("/api/state/main", None).await, "default");
        // An empty value is not a namespace; it must not become one.
        assert_eq!(extract_ns("/api/state/main", Some("   ")).await, "default");
        assert_eq!(
            extract_ns("/api/state/main?namespace=", Some("ctxone")).await,
            "ctxone"
        );
    }

    #[test]
    fn implicit_namespace_warning_is_deduped_per_path() {
        // First hit for a path warns, later ones stay quiet — a polling client
        // must not bury the log.
        assert!(first_time_for_path("/api/test/dedupe/a"));
        assert!(!first_time_for_path("/api/test/dedupe/a"));
        assert!(first_time_for_path("/api/test/dedupe/b"));
    }

    #[test]
    fn llm_usage_accepts_cache_creation_alias() {
        // The `ctx` CLI ingest path posts `cache_creation_tokens`; the serde
        // alias must map it onto `cache_create_tokens` so session-sync doesn't
        // silently drop cache-creation tokens.
        let req: LlmUsageRequest = serde_json::from_str(
            r#"{"input_tokens":1,"output_tokens":2,"cache_read_tokens":3,"cache_creation_tokens":42}"#,
        )
        .expect("deserialize with cache_creation_tokens alias");
        assert_eq!(req.cache_create_tokens, 42);
        assert_eq!(req.cache_read_tokens, 3);
    }

    #[test]
    fn llm_usage_accepts_native_cache_create_name() {
        let req: LlmUsageRequest =
            serde_json::from_str(r#"{"input_tokens":1,"output_tokens":2,"cache_create_tokens":7}"#)
                .expect("deserialize with cache_create_tokens");
        assert_eq!(req.cache_create_tokens, 7);
    }

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
