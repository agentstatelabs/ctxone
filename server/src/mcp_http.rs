//! MCP-over-HTTP: serve the MCP tool surface as a Streamable-HTTP route so a
//! single hub daemon serves MCP + REST + Lens from one port. Agents connect by
//! URL (`/mcp?namespace=<ns>`) instead of spawning their own stdio child, which
//! removes the startup-order race and the two-hubs-one-db lockfile collision.
//!
//! ## Why a dispatch layer
//!
//! rmcp's [`StreamableHttpService`] takes a *context-free* service factory
//! (`Fn() -> Result<S, io::Error>`) — it is called with no arguments, so the
//! `CtxOneServer` it builds cannot see the request's namespace. The stdio path
//! derives the namespace from the spawning process's cwd; a shared daemon has
//! no per-client cwd. So we resolve the namespace at the axum layer (query
//! `?namespace=` wins, then `X-CTXone-Namespace`, then `"default"` — the same
//! order as the REST [`crate::http`] `NamespaceId` extractor) and dispatch to
//! one `StreamableHttpService` *per namespace*, created lazily and cached. Each
//! service's factory captures a repo already forked to that namespace.
//!
//! Each tenant has its own cache of services, in its
//! [`TenantState`](crate::tenant_cache::TenantState): on a multi-tenant Hub a
//! request reaches the services of the tenant its token belongs to, built on
//! that tenant's repository and session statistics.
//!
//! Branch auto-mirroring (stdio-only, cwd-derived) does not apply here: the
//! default ref is `main` unless a tool call targets a branch explicitly.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use tokio::sync::RwLock;

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use agentstategraph_core::Namespace;
use tracing::Instrument;

use crate::asd_pool::AsdProcessPool;
use crate::memory_tools::{CtxOneServer, DEFAULT_SESSION_ID};
use crate::tenant_cache::{Tenancy, TenantState};

/// Each MCP request gets a span and `mcp.server.operation.duration`, per the
/// OpenTelemetry MCP conventions.
type McpService =
    StreamableHttpService<oxtel_rmcp::Instrumented<CtxOneServer>, LocalSessionManager>;

/// One MCP service per (namespace, session id), built on first use. Keyed by
/// both because the service captures a namespace-scoped repo AND a specific
/// session's stats; a stable per-project `X-CTXone-Session` header keeps the
/// cardinality bounded (one entry per project, not per request). Each tenant
/// has its own.
#[derive(Default)]
pub(crate) struct McpServices(RwLock<HashMap<String, Arc<McpService>>>);

/// What every tenant's MCP services share.
struct McpConfig {
    /// Agent id stamped on commits made through MCP (parity with
    /// `ctxone-hub --agent-id`). Reported as `X-CTXone-Agent` on the REST side.
    agent_id: String,
    /// Registered ASD code-graph repos with pre-known base URLs.
    asd_repos: Arc<Vec<(String, String)>>,
    /// Process pool for dynamically spawned `asd-serve` instances.
    asd_pool: Option<Arc<AsdProcessPool>>,
    /// When true, disable rmcp's loopback-only Host allow-list so authenticated
    /// remote clients can reach `/mcp`. Set when a bearer token guards the
    /// surface (the auth middleware is then the real gate). See [`crate::http`].
    allow_remote_hosts: bool,
    /// Whether tools may act on the Hub machine itself; false on a
    /// multi-tenant Hub. See [`CtxOneServer::machine_local`].
    machine_local: bool,
}

/// State for the `/mcp` route: where each request's tenant data comes from,
/// and what every tenant's services share.
#[derive(Clone)]
pub struct McpHttpState {
    tenancy: Tenancy,
    config: Arc<McpConfig>,
}

impl McpHttpState {
    pub(crate) fn new(
        tenancy: Tenancy,
        agent_id: String,
        asd_repos: Arc<Vec<(String, String)>>,
        asd_pool: Option<Arc<AsdProcessPool>>,
        allow_remote_hosts: bool,
    ) -> Self {
        let machine_local = tenancy.machine_local();
        Self {
            tenancy,
            config: Arc::new(McpConfig {
                agent_id,
                asd_repos,
                asd_pool,
                allow_remote_hosts,
                machine_local,
            }),
        }
    }

    /// Get (or lazily create) `tenant`'s MCP service for `(ns, session_id)`.
    /// Double-checked under the write lock so concurrent first-hits don't
    /// build twice.
    async fn service_for(
        &self,
        tenant: &TenantState,
        ns: &str,
        session_id: &str,
    ) -> Arc<McpService> {
        // NUL can't appear in a header value, so it's a safe composite delimiter.
        let key = format!("{ns}\u{0}{session_id}");
        if let Some(svc) = tenant.mcp.0.read().await.get(&key) {
            return svc.clone();
        }
        let mut guard = tenant.mcp.0.write().await;
        if let Some(svc) = guard.get(&key) {
            return svc.clone();
        }
        let svc = Arc::new(self.build_service(tenant, ns, session_id));
        guard.insert(key, svc.clone());
        svc
    }

    /// Build a fresh `StreamableHttpService` on `tenant`'s data, scoped to `ns`
    /// and `session_id`. The repo is forked once here; the factory clones the
    /// `Arc` and the shared session for each new MCP session.
    fn build_service(&self, tenant: &TenantState, ns: &str, session_id: &str) -> McpService {
        let repo = match Namespace::new(ns) {
            Ok(namespace) if ns != Namespace::DEFAULT => {
                // Fork + init so the namespace has a `main` branch, mirroring the
                // stdio path. `init()` is idempotent; a shared daemon must
                // create the namespace on first use (unlike the REST extractor,
                // which assumes it already exists). On error, fall back to the
                // root repo so a bad namespace can't wedge the session.
                let forked = tenant.repo.fork_namespace(namespace);
                match forked.init() {
                    Ok(_) => Arc::new(forked),
                    Err(e) => {
                        tracing::warn!(namespace = ns, error = %e, "namespace init failed; serving MCP in default namespace");
                        tenant.repo.clone()
                    }
                }
            }
            // "default" or (defensively) an invalid name → root repo.
            _ => tenant.repo.clone(),
        };
        let agent_id = self.config.agent_id.clone();
        let asd_repos = (*self.config.asd_repos).clone();
        let asd_pool = self.config.asd_pool.clone();
        let machine_local = self.config.machine_local;
        // Back this service's sessions with the tenant's SHARED, flushed
        // registry, keyed by the caller's X-CTXone-Session id. Every MCP session
        // rmcp spins up for this (ns, session) pair shares this one
        // `SessionStats`, so recall and savings persist under the real session
        // instead of a per-connection counter that dies unflushed.
        let session = tenant.sessions.get_or_create(session_id);
        // Owned: the factory closure outlives this method's borrows.
        let session_id_owned = session_id.to_string();

        let factory = move || {
            let mut server = CtxOneServer::with_agent_id_and_repos(
                repo.clone(),
                agent_id.clone(),
                asd_repos.clone(),
            )
            // HTTP MCP callers select their workspace via ?namespace= / the
            // X-CTXone-Namespace header (baked in by `ctx init --transport
            // http`), so treat it as explicit — the fallback-default write
            // block is a stdio-only concern (that transport auto-detects from
            // cwd and can silently miss). TODO: per-request block for a bare
            // /mcp hit with no namespace once services aren't cached per-ns.
            .with_namespace_explicit(true)
            .with_session_id(session_id_owned.clone())
            .with_session(session.clone())
            .with_machine_local(machine_local);
            if let Some(pool) = asd_pool.clone() {
                server = server.with_pool(pool);
            }
            Ok(oxtel_rmcp::Instrumented::new(server))
        };

        // Defaults are what we want: `stateful_mode: true` (per-client session
        // with SSE reconnection, as MCP clients expect) and a loopback Host
        // allow-list, matching a localhost daemon. The struct is
        // `#[non_exhaustive]`, so build from Default rather than a literal.
        let mut config = StreamableHttpServerConfig::default();
        if self.config.allow_remote_hosts {
            // Bearer auth gates access; let non-loopback Host headers through so
            // authenticated remote clients aren't rejected by the DNS-rebinding
            // guard before the auth layer even runs.
            config = config.disable_allowed_hosts();
        }
        StreamableHttpService::new(factory, Arc::new(LocalSessionManager::default()), config)
    }
}

/// Env var gating the fail-closed identity policy. When set to a truthy value,
/// an MCP request that names neither a namespace nor a session is REJECTED
/// instead of silently landing in the shared `"default"` bucket.
///
/// Default OFF so upgrading the hub cannot strand a client that still connects
/// with a bare URL (`ctx init` bakes both in; hand-written configs may not).
/// Turn it on once every client on the machine is running a config that carries
/// identity — see `docs/TROUBLESHOOTING.md`.
pub const REQUIRE_IDENTITY_ENV: &str = "CTXONE_REQUIRE_IDENTITY";

fn require_identity() -> bool {
    std::env::var(REQUIRE_IDENTITY_ENV)
        .ok()
        .is_some_and(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
}

/// Why an MCP request was refused before it could reach a namespace's data.
///
/// Both variants are the SAME class of bug — an agent whose writes would land
/// somewhere other than the workspace the user thinks they are in — so both
/// carry an actionable remedy rather than a bare 400.
#[derive(Debug)]
pub enum IdentityError {
    /// A namespace was named but is not a legal name (typo, shell mangling).
    /// Always rejected, regardless of [`require_identity`]: silently rewriting
    /// a typo'd workspace to `default` is how memory leaks between projects.
    InvalidNamespace(String),
    /// No namespace named, and the fail-closed policy is on.
    MissingNamespace,
    /// No session id named, and the fail-closed policy is on.
    MissingSession,
}

impl IdentityError {
    fn message(&self) -> String {
        match self {
            Self::InvalidNamespace(ns) => format!(
                "invalid namespace {ns:?}: names must be ASCII [A-Za-z0-9_-]. \
                 Refusing to fall back to the shared \"default\" workspace — \
                 fix the `?namespace=` in your MCP config, or run `ctx init` in the repo."
            ),
            Self::MissingNamespace => format!(
                "no namespace on this MCP request and {REQUIRE_IDENTITY_ENV} is on. \
                 Add `?namespace=<workspace>` to the MCP URL (run `ctx init` in the \
                 repo to write it), or pass `?namespace=default` to opt in explicitly."
            ),
            Self::MissingSession => format!(
                "no session id on this MCP request and {REQUIRE_IDENTITY_ENV} is on. \
                 Without one every agent's recall savings merge into one anonymous \
                 row. Send `X-CTXone-Session`, or add `&session=<id>` to the MCP URL \
                 for clients that cannot set headers. `ctx init` writes whichever fits."
            ),
        }
    }
}

/// Resolve the namespace for an MCP request. `?namespace=` query wins, then the
/// `X-CTXone-Namespace` header.
///
/// An explicitly named but INVALID namespace is always an error — it used to
/// fall back to `"default"`, which meant a typo'd workspace silently wrote its
/// memory into the shared bucket. Omitting the namespace entirely still yields
/// `"default"` unless [`require_identity`] is on.
fn resolve_namespace(req: &Request) -> Result<String, IdentityError> {
    let from_query = req.uri().query().and_then(|q| {
        q.split('&').find_map(|pair| {
            pair.strip_prefix("namespace=")
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string())
        })
    });
    let explicit = from_query.or_else(|| {
        req.headers()
            .get("x-ctxone-namespace")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    });

    let Some(ns) = explicit else {
        return if require_identity() {
            Err(IdentityError::MissingNamespace)
        } else {
            Ok(Namespace::DEFAULT.to_string())
        };
    };

    if ns == Namespace::DEFAULT || Namespace::new(&ns).is_ok() {
        Ok(ns)
    } else {
        Err(IdentityError::InvalidNamespace(ns))
    }
}

/// Resolve the session id for an MCP request: `?session=` query first, then the
/// `X-CTXone-Session` header (both baked in by `ctx init`). This is what ties
/// recall/savings to the caller's real session so they persist, instead of an
/// anonymous per-connection counter.
///
/// The query form exists because not every client can send headers: Codex's
/// `config.toml` accepts only a `url` for an MCP server, so a header-only
/// contract would lock it out of identity entirely (and, under
/// [`require_identity`], out of the hub). Mirrors `?namespace=`.
///
/// Falls back to the shared `"default"` id unless [`require_identity`] is on.
fn resolve_session(req: &Request) -> Result<String, IdentityError> {
    let from_query = req.uri().query().and_then(|q| {
        q.split('&').find_map(|pair| {
            let v = pair.strip_prefix("session=")?;
            let v = percent_decode(v);
            (!v.trim().is_empty()).then(|| v.trim().to_string())
        })
    });
    let from_header = || {
        req.headers()
            .get("x-ctxone-session")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    match from_query.or_else(from_header) {
        Some(s) => Ok(s),
        None if require_identity() => Err(IdentityError::MissingSession),
        None => Ok(DEFAULT_SESSION_ID.to_string()),
    }
}

/// Minimal percent-decoding for the `?session=` value. Session ids carry `:`
/// separators, which clients may or may not escape; nothing else needs decoding.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v as char);
            i += 3;
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// Axum handler for every method on `/mcp`. The Streamable-HTTP service
/// dispatches GET (SSE stream) / POST (JSON-RPC) / DELETE (session close)
/// internally, so we route all methods here.
async fn mcp_handler(State(state): State<McpHttpState>, req: Request) -> Response {
    let (tenant, data) = match state.tenancy.state_for(req.extensions()).await {
        Ok(found) => found,
        Err(refusal) => return refusal.into_response(),
    };
    // The `/mcp` route has no HTTP request span, so this one carries the
    // tenant for what's logged here.
    let span = tracing::info_span!(
        "mcp_request",
        ctxone.tenant = tenant.as_ref().map(|t| t.as_str())
    );
    async move {
        let (ns, session_id) = match (resolve_namespace(&req), resolve_session(&req)) {
            (Ok(ns), Ok(sid)) => (ns, sid),
            (Err(e), _) | (_, Err(e)) => {
                let msg = e.message();
                // Warn, not error: this is a misconfigured client, not a hub
                // fault — but it must be visible, because the symptom on the
                // client side is silence rather than a stack trace.
                tracing::warn!(reason = %msg, "mcp request refused: identity");
                return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
            }
        };
        let svc = state.service_for(&data, &ns, &session_id).await;
        svc.handle(req).await.into_response()
    }
    .instrument(span)
    .await
}

/// Build the `/mcp` router. Merged into the main hub router; kept separate so
/// it carries its own `McpHttpState` and bypasses the REST rate limiter (MCP
/// sessions are long-lived and chatty; per-IP RPM caps would throttle them).
pub fn mcp_router(state: McpHttpState) -> Router {
    Router::new()
        .route("/mcp", any(mcp_handler))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_tools::SessionRegistry;
    use agentstategraph::Repository;
    use agentstategraph_storage::SqliteStorage;

    // Part B: a session named in the request must be registered in the SHARED
    // registry (the one the process flushes), so MCP recall/savings persist
    // instead of dying on an ephemeral per-connection counter.
    #[tokio::test]
    async fn mcp_service_backs_session_with_shared_registry() {
        let repo = Arc::new(Repository::new(Box::new(
            SqliteStorage::in_memory().expect("in-memory sqlite"),
        )));
        repo.init().unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let tenant = Arc::new(TenantState::new(repo, registry.clone(), None));
        let state = McpHttpState::new(
            Tenancy::One(tenant.clone()),
            "agent".to_string(),
            Arc::new(Vec::new()),
            None,
            false,
        );

        assert!(registry.snapshot("proj-sess").is_none());
        // Building the service for this (ns, session) must get_or_create the
        // session in the shared registry.
        let _svc = state.service_for(&tenant, "default", "proj-sess").await;
        assert!(
            registry.snapshot("proj-sess").is_some(),
            "MCP service must back its session with the shared, flushed registry"
        );
    }

    /// `CTXONE_REQUIRE_IDENTITY` is process-global, so the fail-closed tests
    /// share one lock and always restore the previous value. Without this they
    /// race each other (and every other test in the binary) under `cargo test`.
    static IDENTITY_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_require_identity<T>(on: bool, f: impl FnOnce() -> T) -> T {
        let _g = IDENTITY_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(REQUIRE_IDENTITY_ENV).ok();
        // SAFETY: guarded by IDENTITY_ENV_LOCK; restored before the guard drops.
        unsafe {
            if on {
                std::env::set_var(REQUIRE_IDENTITY_ENV, "1");
            } else {
                std::env::remove_var(REQUIRE_IDENTITY_ENV);
            }
        }
        let out = f();
        unsafe {
            match prev {
                Some(v) => std::env::set_var(REQUIRE_IDENTITY_ENV, v),
                None => std::env::remove_var(REQUIRE_IDENTITY_ENV),
            }
        }
        out
    }

    fn req(headers: &[(&str, &str)], uri: &str) -> Request {
        let mut b = Request::builder().uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(axum::body::Body::empty()).unwrap()
    }

    #[test]
    fn resolve_session_reads_header_else_default() {
        with_require_identity(false, || {
            let with = req(&[("x-ctxone-session", "abc123")], "/mcp");
            assert_eq!(resolve_session(&with).unwrap(), "abc123");

            let without = req(&[], "/mcp");
            assert_eq!(resolve_session(&without).unwrap(), DEFAULT_SESSION_ID);
        });
    }

    // -- Phase 1: fail-closed identity ---------------------------------

    /// A typo'd workspace must NEVER be rewritten to `default` — that is the
    /// exact path by which one project's memory leaked into another's.
    /// Rejected whether or not the fail-closed policy is on.
    #[test]
    fn invalid_namespace_is_rejected_regardless_of_policy() {
        for on in [false, true] {
            with_require_identity(on, || {
                let r = req(&[], "/mcp?namespace=not%20a%20valid%20ns");
                assert!(
                    matches!(
                        resolve_namespace(&r),
                        Err(IdentityError::InvalidNamespace(_))
                    ),
                    "invalid namespace must not fall back to default (policy on={on})"
                );
            });
        }
    }

    /// Policy OFF preserves the historical behaviour exactly, so upgrading the
    /// hub cannot strand a client that still connects with a bare URL.
    #[test]
    fn missing_identity_defaults_when_policy_off() {
        with_require_identity(false, || {
            let r = req(&[], "/mcp");
            assert_eq!(resolve_namespace(&r).unwrap(), Namespace::DEFAULT);
            assert_eq!(resolve_session(&r).unwrap(), DEFAULT_SESSION_ID);
        });
    }

    #[test]
    fn missing_identity_is_rejected_when_policy_on() {
        with_require_identity(true, || {
            let r = req(&[], "/mcp");
            assert!(matches!(
                resolve_namespace(&r),
                Err(IdentityError::MissingNamespace)
            ));
            assert!(matches!(
                resolve_session(&r),
                Err(IdentityError::MissingSession)
            ));
        });
    }

    /// `default` stays reachable when asked for BY NAME — the rule is "nothing
    /// lands in default unless specified", not "default is unreachable". This
    /// is what keeps Codex working while it is pinned there.
    #[test]
    fn explicit_default_namespace_is_allowed_under_policy() {
        with_require_identity(true, || {
            let r = req(
                &[("x-ctxone-session", "codex:default")],
                "/mcp?namespace=default",
            );
            assert_eq!(resolve_namespace(&r).unwrap(), Namespace::DEFAULT);
            assert_eq!(resolve_session(&r).unwrap(), "codex:default");
        });
    }

    /// Codex's `config.toml` accepts only a `url` — no headers. Without a query
    /// form, turning the policy on locks Codex out of the hub entirely.
    #[test]
    fn session_query_satisfies_policy_for_header_less_clients() {
        with_require_identity(true, || {
            let r = req(&[], "/mcp?namespace=default&session=default:codex:abc123");
            assert_eq!(resolve_session(&r).unwrap(), "default:codex:abc123");
            assert_eq!(resolve_namespace(&r).unwrap(), Namespace::DEFAULT);
        });
    }

    /// Session ids contain `:` separators; a client that percent-escapes them
    /// must resolve to the same id as one that does not.
    #[test]
    fn session_query_is_percent_decoded() {
        with_require_identity(true, || {
            let r = req(&[], "/mcp?namespace=ctxone&session=ctxone%3Acodex%3Aabc123");
            assert_eq!(resolve_session(&r).unwrap(), "ctxone:codex:abc123");
        });
    }

    /// Query wins over header, matching how `?namespace=` resolves.
    #[test]
    fn session_query_beats_header() {
        with_require_identity(true, || {
            let r = req(
                &[("x-ctxone-session", "from-header")],
                "/mcp?session=from-query",
            );
            assert_eq!(resolve_session(&r).unwrap(), "from-query");
        });
    }

    /// The header spelling is the other half of the contract; a client that
    /// sends `X-CTXone-Namespace` instead of `?namespace=` must satisfy policy.
    #[test]
    fn namespace_header_satisfies_policy() {
        with_require_identity(true, || {
            let r = req(&[("x-ctxone-namespace", "sessiondrift")], "/mcp");
            assert_eq!(resolve_namespace(&r).unwrap(), "sessiondrift");
        });
    }

    /// Every refusal must name the remedy — the client-side symptom is silence,
    /// so a bare "400 Bad Request" would be a dead end for the user.
    #[test]
    fn identity_errors_name_the_remedy() {
        for e in [
            IdentityError::InvalidNamespace("bad ns".into()),
            IdentityError::MissingNamespace,
            IdentityError::MissingSession,
        ] {
            assert!(
                e.message().contains("ctx init"),
                "remedy missing: {}",
                e.message()
            );
        }
    }
}
