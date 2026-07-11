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
//! Branch auto-mirroring (stdio-only, cwd-derived) does not apply here: the
//! default ref is `main` unless a tool call targets a branch explicitly.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use tokio::sync::RwLock;

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use agentstategraph::Repository;
use agentstategraph_core::Namespace;

use crate::asd_pool::AsdProcessPool;
use crate::memory_tools::CtxOneServer;

type McpService = StreamableHttpService<CtxOneServer, LocalSessionManager>;

/// State for the `/mcp` route: everything needed to build a namespace-scoped
/// MCP service, plus the per-namespace service cache.
#[derive(Clone)]
pub struct McpHttpState {
    /// Root (default-namespace) repo. Forked per namespace on demand; the
    /// forked `Arc` is captured by that namespace's service factory.
    repo: Arc<Repository>,
    /// Agent id stamped on commits made through MCP (parity with
    /// `ctxone-hub --agent-id`). Reported as `X-CTXone-Agent` on the REST side.
    agent_id: String,
    /// Registered ASD code-graph repos with pre-known base URLs.
    asd_repos: Arc<Vec<(String, String)>>,
    /// Process pool for dynamically spawned `asd-serve` instances.
    asd_pool: Option<Arc<AsdProcessPool>>,
    /// One MCP service per namespace, built on first use.
    services: Arc<RwLock<HashMap<String, Arc<McpService>>>>,
}

impl McpHttpState {
    pub fn new(
        repo: Arc<Repository>,
        agent_id: String,
        asd_repos: Arc<Vec<(String, String)>>,
        asd_pool: Option<Arc<AsdProcessPool>>,
    ) -> Self {
        Self {
            repo,
            agent_id,
            asd_repos,
            asd_pool,
            services: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get (or lazily create) the MCP service for `ns`. Double-checked under
    /// the write lock so concurrent first-hits don't build twice.
    async fn service_for(&self, ns: &str) -> Arc<McpService> {
        if let Some(svc) = self.services.read().await.get(ns) {
            return svc.clone();
        }
        let mut guard = self.services.write().await;
        if let Some(svc) = guard.get(ns) {
            return svc.clone();
        }
        let svc = Arc::new(self.build_service(ns));
        guard.insert(ns.to_string(), svc.clone());
        svc
    }

    /// Build a fresh `StreamableHttpService` scoped to `ns`. The repo is forked
    /// once here; the factory clones the `Arc` for each new MCP session.
    fn build_service(&self, ns: &str) -> McpService {
        let repo = match Namespace::new(ns) {
            Ok(namespace) if ns != Namespace::DEFAULT => {
                // Fork + init so the namespace has a `main` branch, mirroring the
                // stdio path. `init()` is idempotent; a shared daemon must
                // create the namespace on first use (unlike the REST extractor,
                // which assumes it already exists). On error, fall back to the
                // root repo so a bad namespace can't wedge the session.
                let forked = self.repo.fork_namespace(namespace);
                match forked.init() {
                    Ok(_) => Arc::new(forked),
                    Err(e) => {
                        tracing::warn!(namespace = ns, error = %e, "namespace init failed; serving MCP in default namespace");
                        self.repo.clone()
                    }
                }
            }
            // "default" or (defensively) an invalid name → root repo.
            _ => self.repo.clone(),
        };
        let agent_id = self.agent_id.clone();
        let asd_repos = (*self.asd_repos).clone();
        let asd_pool = self.asd_pool.clone();
        let ns_label = ns.to_string();

        let factory = move || {
            let mut server = CtxOneServer::with_agent_id_and_repos(
                repo.clone(),
                agent_id.clone(),
                asd_repos.clone(),
            )
            .with_namespace(ns_label.clone());
            if let Some(pool) = asd_pool.clone() {
                server = server.with_pool(pool);
            }
            Ok(server)
        };

        // Defaults are what we want: `stateful_mode: true` (per-client session
        // with SSE reconnection, as MCP clients expect) and a loopback Host
        // allow-list, matching a localhost daemon. The struct is
        // `#[non_exhaustive]`, so build from Default rather than a literal.
        let config = StreamableHttpServerConfig::default();
        StreamableHttpService::new(factory, Arc::new(LocalSessionManager::default()), config)
    }
}

/// Resolve the namespace for an MCP request. `?namespace=` query wins, then the
/// `X-CTXone-Namespace` header, then `"default"`. An invalid name falls back to
/// `"default"` so a bad config can never panic the fork.
fn resolve_namespace(req: &Request) -> String {
    let from_query = req.uri().query().and_then(|q| {
        q.split('&').find_map(|pair| {
            pair.strip_prefix("namespace=")
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string())
        })
    });
    let ns = from_query
        .or_else(|| {
            req.headers()
                .get("x-ctxone-namespace")
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| Namespace::DEFAULT.to_string());

    if ns == Namespace::DEFAULT || Namespace::new(&ns).is_ok() {
        ns
    } else {
        Namespace::DEFAULT.to_string()
    }
}

/// Axum handler for every method on `/mcp`. The Streamable-HTTP service
/// dispatches GET (SSE stream) / POST (JSON-RPC) / DELETE (session close)
/// internally, so we route all methods here.
async fn mcp_handler(State(state): State<McpHttpState>, req: Request) -> Response {
    let ns = resolve_namespace(&req);
    let svc = state.service_for(&ns).await;
    svc.handle(req).await.into_response()
}

/// Build the `/mcp` router. Merged into the main hub router; kept separate so
/// it carries its own `McpHttpState` and bypasses the REST rate limiter (MCP
/// sessions are long-lived and chatty; per-IP RPM caps would throttle them).
pub fn mcp_router(state: McpHttpState) -> Router {
    Router::new().route("/mcp", any(mcp_handler)).with_state(state)
}
