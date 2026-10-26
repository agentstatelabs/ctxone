//! Each tenant's data on a multi-tenant Hub (docs/design/MULTI_TENANT_HUB.md,
//! "Per-tenant state").
//!
//! A [`TenantState`] holds one tenant's data: its repository, its hub tables,
//! its session statistics and its MCP services. A single-tenant Hub has one,
//! made at startup. A multi-tenant Hub keeps a [`TenantCache`]: a tenant opens
//! on its first request, through a [`TenantOpener`], and closes when
//! [`TenantCache::evict_idle`] finds it idle.
//!
//! Handlers reach data only through the request's `TenantState` (`HubState` in
//! [`crate::http`] is built from it), so on a multi-tenant Hub every path to a
//! repository goes through the tenant the auth layer attached to the request.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use agentstategraph::Repository;
use agentstategraph_storage::{MemoryStorage, PostgresStorage};
use axum::http::{Extensions, StatusCode};
use tracing::{info, warn};

use crate::hub_store::{HubStore, PgStore, pg_error};
use crate::memory_tools::SessionRegistry;
use crate::tenant::Tenant;

/// One tenant's data, and what the Hub keeps for it.
pub struct TenantState {
    pub repo: Arc<Repository>,
    pub sessions: Arc<SessionRegistry>,
    /// Where `sessions` and the project registry are kept. `None` keeps
    /// session statistics in memory and has no project registry.
    pub store: Option<HubStore>,
    /// Merges running in this tenant, keyed by (namespace, target ref).
    pub(crate) merges_in_flight: Arc<Mutex<HashSet<String>>>,
    /// The tenant's MCP services, one per (namespace, session).
    pub(crate) mcp: crate::mcp_http::McpServices,
    /// The last `GET /api/namespaces/summary`, reused for a few seconds.
    pub(crate) namespaces_summary: Mutex<Option<(Instant, serde_json::Value)>>,
    /// Requests that have reached the tenant, so eviction can tell whether
    /// one arrived while it flushed.
    uses: AtomicU64,
    last_used: Mutex<Instant>,
}

impl TenantState {
    pub fn new(
        repo: Arc<Repository>,
        sessions: Arc<SessionRegistry>,
        store: Option<HubStore>,
    ) -> Self {
        Self {
            repo,
            sessions,
            store,
            merges_in_flight: Arc::default(),
            mcp: Default::default(),
            namespaces_summary: Mutex::new(None),
            uses: AtomicU64::new(0),
            last_used: Mutex::new(Instant::now()),
        }
    }

    /// Write the session statistics to the tenant's store, if it has one.
    /// Blocking.
    pub fn flush(&self) {
        if let Some(store) = &self.store {
            self.sessions.flush(store);
        }
    }

    fn touch(&self) {
        self.uses.fetch_add(1, Ordering::Relaxed);
        *self
            .last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed()
    }
}

impl std::fmt::Debug for TenantState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantState")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

/// A tenant's data as a [`TenantOpener`] opens it. The cache loads its session
/// statistics from `store`.
pub struct OpenedTenant {
    pub repo: Arc<Repository>,
    pub store: Option<HubStore>,
}

/// Opens tenants' data for a multi-tenant Hub: creates a tenant's data the
/// first time and brings its schema up to date.
pub trait TenantOpener: Send + Sync + std::fmt::Debug {
    /// Open `tenant`. Blocking: [`TenantCache`] calls it off the async
    /// runtime, for one tenant at a time.
    fn open(&self, tenant: &Tenant) -> Result<OpenedTenant, String>;
}

/// The open tenants of a multi-tenant Hub. Cheap to clone; clones share the
/// cache.
#[derive(Clone, Debug)]
pub struct TenantCache {
    inner: Arc<CacheInner>,
}

#[derive(Debug)]
struct CacheInner {
    opener: Arc<dyn TenantOpener>,
    open: RwLock<HashMap<Tenant, Arc<TenantState>>>,
    /// Tenants open one at a time, so requests arriving together for a new
    /// tenant open it once, and [`PostgresTenants`] holds at most one lock
    /// connection beside its pool.
    opening: tokio::sync::Mutex<()>,
}

impl TenantCache {
    pub fn new(opener: Arc<dyn TenantOpener>) -> Self {
        Self {
            inner: Arc::new(CacheInner {
                opener,
                open: RwLock::new(HashMap::new()),
                opening: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// `tenant`'s data, opening it if it isn't open. An error is the
    /// opener's; the next request tries again.
    pub async fn get(&self, tenant: &Tenant) -> Result<Arc<TenantState>, String> {
        if let Some(state) = self.cached(tenant) {
            return Ok(state);
        }
        let _opening = self.inner.opening.lock().await;
        if let Some(state) = self.cached(tenant) {
            return Ok(state);
        }
        let started = Instant::now();
        let opener = self.inner.opener.clone();
        let name = tenant.clone();
        let state = tokio::task::spawn_blocking(move || {
            let opened = opener.open(&name)?;
            let sessions = match &opened.store {
                Some(store) => SessionRegistry::load(store),
                None => SessionRegistry::new(),
            };
            Ok::<_, String>(TenantState::new(
                opened.repo,
                Arc::new(sessions),
                opened.store,
            ))
        })
        .await
        .map_err(|e| format!("opening the tenant failed: {e}"))??;
        let state = Arc::new(state);
        state.touch();
        self.inner
            .open
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(tenant.clone(), state.clone());
        info!(
            tenant = %tenant,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "tenant opened"
        );
        Ok(state)
    }

    fn cached(&self, tenant: &Tenant) -> Option<Arc<TenantState>> {
        let state = self
            .inner
            .open
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(tenant)
            .cloned()?;
        state.touch();
        Some(state)
    }

    /// The tenants open now, sorted.
    pub fn open_tenants(&self) -> Vec<Tenant> {
        let mut tenants: Vec<Tenant> = self
            .inner
            .open
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        tenants.sort();
        tenants
    }

    /// Write every open tenant's session statistics to its store. Blocking.
    pub fn flush_all(&self) {
        let open: Vec<Arc<TenantState>> = self
            .inner
            .open
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for state in open {
            state.flush();
        }
    }

    /// Close the tenants no request has reached for `idle`, after flushing
    /// their session statistics, and return them, sorted. A tenant a request is using
    /// stays open, and so does one a request reached while it flushed.
    /// Blocking: flushing writes to the tenants' stores.
    pub fn evict_idle(&self, idle: Duration) -> Vec<Tenant> {
        let candidates: Vec<(Tenant, Arc<TenantState>, u64)> = self
            .inner
            .open
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, state)| Arc::strong_count(state) == 1 && state.idle_for() >= idle)
            .map(|(tenant, state)| {
                let uses = state.uses.load(Ordering::Relaxed);
                (tenant.clone(), state.clone(), uses)
            })
            .collect();
        let mut evicted = Vec::new();
        for (tenant, state, uses) in candidates {
            state.flush();
            let mut open = self
                .inner
                .open
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            // Still the same state, held only by the cache and `state`, and
            // no request since the flush began.
            let untouched = open.get(&tenant).is_some_and(|current| {
                Arc::ptr_eq(current, &state)
                    && Arc::strong_count(&state) == 2
                    && state.uses.load(Ordering::Relaxed) == uses
            });
            if untouched {
                open.remove(&tenant);
                info!(tenant = %tenant, "tenant closed after being idle");
                evicted.push(tenant);
            }
        }
        evicted.sort();
        evicted
    }
}

/// Where a request's data comes from.
#[derive(Clone, Debug)]
pub(crate) enum Tenancy {
    /// A single-tenant Hub: every request reaches this one tenant.
    One(Arc<TenantState>),
    /// A multi-tenant Hub: each request reaches its own tenant.
    Many(TenantCache),
}

impl Tenancy {
    /// The tenant of a request the auth layer let through, and its data. A
    /// single-tenant Hub has no tenant to report.
    pub(crate) async fn state_for(
        &self,
        extensions: &Extensions,
    ) -> Result<(Option<Tenant>, Arc<TenantState>), (StatusCode, &'static str)> {
        let cache = match self {
            Self::One(state) => return Ok((None, state.clone())),
            Self::Many(cache) => cache,
        };
        let Some(tenant) = extensions.get::<Tenant>() else {
            // The auth layer gives every request it lets through a tenant, so
            // this request came by a route outside it.
            tracing::error!("a request without a tenant reached tenant data");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "this request has no tenant",
            ));
        };
        match cache.get(tenant).await {
            Ok(state) => Ok((Some(tenant.clone()), state)),
            Err(e) => {
                warn!(tenant = %tenant, error = %e, "couldn't open the tenant");
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "can't open this tenant's data right now; try again",
                ))
            }
        }
    }
}

/// Make a tenant's repository ready to serve: its `main` branch, then the
/// AgentStateGraph and CTXone schemas.
fn prepare(repo: &Repository) -> Result<(), String> {
    repo.init().map_err(|e| format!("init: {e}"))?;
    crate::migrations::migrate_asg(repo).map_err(|e| e.to_string())?;
    crate::migrations::run_migrations(repo).map_err(|e| e.to_string())
}

/// Advisory-lock class for opening a tenant ("ctxt" as bytes): a Hub opening
/// a tenant holds `pg_advisory_lock(OPEN_LOCK_CLASS, hashtext(<tenant>))`.
pub const OPEN_LOCK_CLASS: i32 = 0x6374_7874;

/// How long opening a tenant waits for another Hub that's opening it.
const OPEN_LOCK_TIMEOUT: &str = "30s";

/// Tenants on one Postgres pool: each gets AgentStateGraph's `for_tenant`
/// storage and [`PgStore::for_tenant`] hub tables, every row carrying its
/// tenant.
pub struct PostgresTenants {
    root: PostgresStorage,
    tables: PgStore,
}

impl std::fmt::Debug for PostgresTenants {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresTenants")
            .field("tables", &self.tables)
            .finish_non_exhaustive()
    }
}

impl PostgresTenants {
    /// Tenants on `root`'s pool. Creates the hub tables if they're missing.
    /// `root`'s own tenant serves no data.
    pub fn new(root: PostgresStorage) -> Result<Self, String> {
        let tables = PgStore::new(&root)?;
        Ok(Self { root, tables })
    }

    /// Wait until no other Hub is opening `tenant`, and keep it that way
    /// while the guard lives.
    fn lock(&self, tenant: &Tenant) -> Result<OpenLock, String> {
        let pool = self.root.pool();
        let name = tenant.as_str().to_owned();
        PostgresStorage::run(async move {
            let pooled = deadpool_postgres::Pool::get(&pool)
                .await
                .map_err(|e| e.to_string())?;
            // Taken out of the pool, which frees its place there for the
            // migrations. Dropping the guard closes the connection, and
            // closing it releases the lock however the open ended.
            let client = deadpool_postgres::Object::take(pooled);
            client
                .batch_execute(&format!("SET lock_timeout = '{OPEN_LOCK_TIMEOUT}'"))
                .await
                .map_err(|e| pg_error(&e))?;
            client
                .execute(
                    "SELECT pg_advisory_lock($1, hashtext($2))",
                    &[&OPEN_LOCK_CLASS, &name],
                )
                .await
                .map_err(|e| pg_error(&e))?;
            Ok(OpenLock(client))
        })
    }
}

/// A tenant held for opening: see [`PostgresTenants::lock`].
struct OpenLock(#[allow(dead_code)] deadpool_postgres::ClientWrapper);

impl TenantOpener for PostgresTenants {
    fn open(&self, tenant: &Tenant) -> Result<OpenedTenant, String> {
        let _lock = self
            .lock(tenant)
            .map_err(|e| format!("lock the tenant for opening: {e}"))?;
        let storage = self
            .root
            .for_tenant(tenant.as_str())
            .map_err(|e| e.to_string())?;
        let repo = Repository::new(Box::new(storage));
        prepare(&repo)?;
        Ok(OpenedTenant {
            repo: Arc::new(repo),
            store: Some(HubStore::Postgres(self.tables.for_tenant(tenant.as_str()))),
        })
    }
}

/// Tenants in memory, for tests and for trying multi-tenant mode out. A
/// tenant's data lasts as long as the process, closed or not. With no store,
/// its session statistics start over when it reopens.
#[derive(Default)]
pub struct MemoryTenants {
    repos: Mutex<HashMap<Tenant, Arc<Repository>>>,
}

impl std::fmt::Debug for MemoryTenants {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryTenants").finish_non_exhaustive()
    }
}

impl TenantOpener for MemoryTenants {
    fn open(&self, tenant: &Tenant) -> Result<OpenedTenant, String> {
        let mut repos = self.repos.lock().unwrap_or_else(PoisonError::into_inner);
        let repo = match repos.get(tenant) {
            Some(repo) => repo.clone(),
            None => {
                let repo = Repository::new(Box::new(MemoryStorage::new()));
                prepare(&repo)?;
                let repo = Arc::new(repo);
                repos.insert(tenant.clone(), repo.clone());
                repo
            }
        };
        Ok(OpenedTenant { repo, store: None })
    }
}
