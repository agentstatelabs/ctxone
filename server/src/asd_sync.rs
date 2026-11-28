//! The Hub serves sync for AgentStateDeveloper judgment (asd-hub-sync t-012,
//! HUB_SYNC.md decision 12).
//!
//! ASD keeps what no re-index can reproduce (its ledger, rebinds, feedback,
//! declared effects and evidence) on a `judgment` ref, one namespace per repo
//! (`<repo>-<8 hex>`, from the repo's normalized remote). With
//! `--serve-sync`, a Hub serves that ref for each tenant at
//! `/asd/sync/v1/<store>/{refs,fetch,push}`, where `<store>` is `me` or the
//! tenant's name, the same as `/sync/v1` and behind the same token.
//!
//! **A store of its own.** A tenant's ASD judgment lives in a second
//! AgentStateGraph store beside its CTXone memory, never in it: CTXone walks
//! every namespace of its own store (workspace lists, search, migrations, its
//! own sync), and none of that should see a repo's judgment. Each tenant's
//! repos are namespaces in that one store. Where it's kept:
//!
//! | Hub | ASD store |
//! |---|---|
//! | multi-tenant, Postgres | AgentStateGraph tenant `<tenant>.asd`, in the same database |
//! | single-tenant, Postgres | AgentStateGraph tenant `<its tenant>.asd` |
//! | single-tenant, SQLite | a file beside the Hub's: `ctxone.db` → `ctxone.asd.db` |
//! | memory | in memory, as long as the process runs |
//!
//! A tenant's name is `[a-z0-9-]`, so no tenant's own data is ever kept under
//! a `.asd` id. The store opens on its first sync request, never before, so a
//! Hub nobody syncs ASD with keeps no ASD file or rows.
//!
//! **Sync only.** The Hub never indexes code, runs ASD or serves code tools
//! for a tenant: the store holds pushed refs and nothing reads them yet. The
//! receiving side's checks are AgentStateGraph's: pushes only move a ref
//! forward, and local-only refs (ASD's `main`, which holds its index) never
//! arrive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use agentstategraph::Repository;
use agentstategraph::sync::BundleLimits;
use agentstategraph_storage::{MemoryStorage, PostgresStorage, SqliteStorage};
use agentstategraph_sync_http::{Rejection, Resolved, StoreResolver};
use axum::Router;
use axum::http::StatusCode;
use axum::http::request::Parts;

use crate::sync_service::{OWN_STORE, SINGLE_STORE, pusher};
use crate::tenant::Tenant;
use crate::tenant_cache::Tenancy;

/// Where the ASD sync routes are mounted: `/asd/sync/v1/<store>/…`.
pub const PATH_PREFIX: &str = "/asd";

/// What a tenant's data id gets to name its ASD store's.
pub const STORE_SUFFIX: &str = ".asd";

/// The AgentStateGraph tenant id of the ASD store beside the data of
/// AgentStateGraph tenant `data_tenant`.
pub fn tenant_id(data_tenant: &str) -> String {
    format!("{data_tenant}{STORE_SUFFIX}")
}

/// The SQLite file a single-tenant Hub on `db_path` keeps its ASD store in:
/// `ctxone.db` → `ctxone.asd.db`, beside it.
pub fn sqlite_path(db_path: &Path) -> PathBuf {
    let stem = db_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ctxone".into());
    let ext = db_path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "db".into());
    db_path.with_file_name(format!("{stem}{STORE_SUFFIX}.{ext}"))
}

/// Where a Hub keeps its tenants' ASD stores. Cheap to clone.
#[derive(Clone)]
pub enum AsdStores {
    /// A single-tenant Hub on SQLite: this one file.
    Sqlite(PathBuf),
    /// AgentStateGraph tenants on `root`'s pool: `<tenant>.asd` for each
    /// tenant, or `<fixed>.asd` on a single-tenant Hub, whose data is
    /// AgentStateGraph tenant `fixed`.
    Postgres {
        root: Arc<PostgresStorage>,
        fixed: Option<String>,
    },
    /// In memory, kept as long as the process runs, closed tenants' too.
    Memory(Arc<Mutex<HashMap<String, Arc<Repository>>>>),
}

impl std::fmt::Debug for AsdStores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(path) => f.debug_tuple("Sqlite").field(path).finish(),
            Self::Postgres { fixed, .. } => f
                .debug_struct("Postgres")
                .field("fixed", fixed)
                .finish_non_exhaustive(),
            Self::Memory(_) => f.write_str("Memory"),
        }
    }
}

impl AsdStores {
    /// Stores kept in memory.
    pub fn memory() -> Self {
        Self::Memory(Arc::default())
    }

    /// Where it keeps them, for the startup log.
    pub fn describe(&self) -> String {
        match self {
            Self::Sqlite(path) => format!("sqlite {}", path.display()),
            Self::Postgres { fixed: Some(t), .. } => format!("postgres tenant {}", tenant_id(t)),
            Self::Postgres { fixed: None, .. } => {
                format!("postgres tenant <tenant>{STORE_SUFFIX} for each tenant")
            }
            Self::Memory(_) => "memory".into(),
        }
    }

    /// Open the ASD store of `tenant` (`None` on a single-tenant Hub),
    /// creating it the first time. Blocking.
    pub fn open(&self, tenant: Option<&Tenant>) -> Result<Arc<Repository>, String> {
        let key = tenant.map_or(SINGLE_STORE, |t| t.as_str());
        let repo = match self {
            Self::Sqlite(path) => {
                if let Some(parent) = path.parent()
                    && !parent.as_os_str().is_empty()
                {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("create {}: {e}", parent.display()))?;
                }
                let storage = SqliteStorage::open(path)
                    .map_err(|e| format!("open {}: {e}", path.display()))?;
                Repository::new(Box::new(storage))
            }
            Self::Postgres { root, fixed } => {
                let id = tenant_id(fixed.as_deref().unwrap_or(key));
                let storage = root.for_tenant(&id).map_err(|e| e.to_string())?;
                Repository::new(Box::new(storage))
            }
            Self::Memory(stores) => {
                let mut stores = stores.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(repo) = stores.get(key) {
                    return Ok(repo.clone());
                }
                let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
                repo.init().map_err(|e| format!("init: {e}"))?;
                stores.insert(key.to_owned(), repo.clone());
                return Ok(repo);
            }
        };
        repo.init().map_err(|e| format!("init: {e}"))?;
        Ok(Arc::new(repo))
    }
}

/// Resolves each ASD sync request to its tenant's ASD store.
struct AsdSyncResolver {
    tenancy: Tenancy,
    stores: AsdStores,
}

impl StoreResolver for AsdSyncResolver {
    async fn resolve(&self, store: &str, request: &Parts) -> Result<Resolved, Rejection> {
        // `require_auth` ran first, as for `/sync/v1`.
        let (tenant, data) =
            self.tenancy
                .state_for(&request.extensions)
                .await
                .map_err(|(status, message)| match status {
                    StatusCode::SERVICE_UNAVAILABLE => Rejection::NotFound(message.to_string()),
                    _ => Rejection::Unauthorized(message.to_string()),
                })?;
        let own = tenant.as_ref().map_or(SINGLE_STORE, |t| t.as_str());
        if store != OWN_STORE && store != own {
            return Err(Rejection::Forbidden(format!(
                "this token's ASD store is {own:?} (or {OWN_STORE:?}), not {store:?}"
            )));
        }
        let repo = data
            .asd
            .get_or_try_init(|| {
                let stores = self.stores.clone();
                let tenant = tenant.clone();
                let label = own.to_owned();
                async move {
                    let opened = tokio::task::spawn_blocking(move || stores.open(tenant.as_ref()))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                    if let Err(e) = &opened {
                        tracing::warn!(store = %label, error = %e, "couldn't open the ASD store");
                    }
                    opened
                }
            })
            .await
            .map_err(|_| {
                Rejection::NotFound(
                    "can't open this tenant's ASD store right now; try again".into(),
                )
            })?;
        Ok(Resolved {
            repo: repo.clone(),
            pusher: pusher(own, request),
        })
    }
}

/// The `/asd/sync/v1` routes, serving each request's own ASD store from
/// `stores`, with `max_push_bytes` (AgentStateGraph's default when `None`) as
/// the most one push may send. Merge them inside the auth layer.
pub(crate) fn router(tenancy: Tenancy, stores: AsdStores, max_push_bytes: Option<u64>) -> Router {
    let mut limits = BundleLimits::default();
    if let Some(max) = max_push_bytes {
        limits.max_bytes = max;
    }
    Router::new().nest(
        PATH_PREFIX,
        agentstategraph_sync_http::router_with_limits(
            Arc::new(AsdSyncResolver { tenancy, stores }),
            limits,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sqlite_store_sits_beside_the_hubs() {
        assert_eq!(
            sqlite_path(Path::new("/data/ctxone.db")),
            Path::new("/data/ctxone.asd.db")
        );
        assert_eq!(
            sqlite_path(Path::new("target/hub")),
            Path::new("target/hub.asd.db")
        );
    }

    #[test]
    fn no_tenant_can_take_an_asd_store_id() {
        let id = tenant_id("alice");
        assert_eq!(id, "alice.asd");
        assert!(Tenant::new(&id).is_err());
    }

    #[test]
    fn memory_stores_are_per_tenant_and_outlive_a_close() {
        let stores = AsdStores::memory();
        let alice = Tenant::new("alice").unwrap();
        let bob = Tenant::new("bob").unwrap();
        let a = stores.open(Some(&alice)).unwrap();
        assert!(Arc::ptr_eq(&a, &stores.open(Some(&alice)).unwrap()));
        assert!(!Arc::ptr_eq(&a, &stores.open(Some(&bob)).unwrap()));
        assert!(!Arc::ptr_eq(&a, &stores.open(None).unwrap()));
    }
}
