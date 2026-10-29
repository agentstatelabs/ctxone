//! MCP sessions that any Hub process sharing the database can continue
//! (ctx-multi-tenant-hub t-012).
//!
//! A streamable-HTTP MCP session lives in the process that began it. rmcp
//! keeps what it takes to continue one elsewhere, the client's `initialize`
//! parameters, in a [`SessionStore`]: when a request names a session the
//! process doesn't hold, rmcp loads them and replays the handshake, and the
//! client carries on as if nothing happened. [`McpSessionStore`] keeps them in
//! the Hub's database, its Postgres tables or its SQLite file, so a session
//! continues after its own process let it go for being idle (rmcp after five
//! minutes, the tenant cache after thirty), after a restart, and on Postgres
//! on another replica. Without it the client gets the 404 the MCP spec
//! prescribes and has to start a new session.
//!
//! A session continues only in the workspace and statistics session it began
//! in, and on Postgres only for its tenant: the rows are the tenant's, under
//! its policy. One unused for [`TTL_DAYS`] days is forgotten.

use agentstategraph_storage::PostgresStorage;
use rmcp::transport::streamable_http_server::session::store::{
    SessionState, SessionStore, SessionStoreError,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::hub_store::{HubStore, pg_error};

/// Days a session can go unused before it can no longer be continued
/// elsewhere. Its own process can keep serving it for longer.
pub const TTL_DAYS: i32 = 7;

/// The table in a SQLite Hub's file: the Postgres one without `tenant_id`,
/// since the file holds one tenant. `seen_at` is UTC in the
/// `YYYY-MM-DDTHH:MM:SSZ` form of the file's other timestamps, which sorts
/// as it compares.
const SQLITE_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS ctxone_mcp_sessions (
        session_id  TEXT PRIMARY KEY,
        namespace   TEXT NOT NULL,
        ctx_session TEXT NOT NULL,
        state       TEXT NOT NULL,
        seen_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
    );
";

/// The MCP sessions of one tenant's service for one (workspace, statistics
/// session) pair, which is what each service in [`crate::mcp_http`] serves.
#[derive(Debug)]
pub struct McpSessionStore {
    store: HubStore,
    namespace: String,
    ctx_session: String,
}

impl McpSessionStore {
    pub fn new(store: HubStore, namespace: &str, ctx_session: &str) -> Self {
        Self {
            store,
            namespace: namespace.to_owned(),
            ctx_session: ctx_session.to_owned(),
        }
    }

    /// The workspace and statistics session that scope this store's rows.
    fn scope(&self) -> [String; 2] {
        [self.namespace.clone(), self.ctx_session.clone()]
    }
}

/// Run `query` on AgentStateGraph's runtime, where its pool's connections
/// live, without holding up this one.
async fn on_pool_runtime<T: Send + 'static>(
    query: impl Future<Output = Result<T, String>> + Send + 'static,
) -> Result<T, SessionStoreError> {
    tokio::task::spawn_blocking(move || PostgresStorage::run(query))
        .await
        .map_err(|e| e.to_string())?
        .map_err(Into::into)
}

/// Run `query` on a connection of its own to the SQLite file at `path`, with
/// the table in place, without holding up the runtime.
async fn on_sqlite<T: Send + 'static>(
    path: &str,
    query: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
) -> Result<T, SessionStoreError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let db = Connection::open(&path)?;
        db.execute_batch(SQLITE_SCHEMA)?;
        query(&db)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(Into::into)
}

#[async_trait::async_trait]
impl SessionStore for McpSessionStore {
    async fn load(&self, session_id: &str) -> Result<Option<SessionState>, SessionStoreError> {
        let [namespace, ctx_session] = self.scope();
        let id = session_id.to_owned();
        let state = match &self.store {
            HubStore::Postgres(pg) => {
                let (pool, tenant) = (pg.tenant_pool(), pg.tenant().to_owned());
                on_pool_runtime(async move {
                    let client = pool.get().await.map_err(|e| e.to_string())?;
                    let row = client
                        .query_opt(
                            "UPDATE ctxone_mcp_sessions SET seen_at = now()
                             WHERE tenant_id = $1 AND session_id = $2
                               AND namespace = $3 AND ctx_session = $4
                               AND seen_at > now() - make_interval(days => $5)
                             RETURNING state",
                            &[&tenant, &id, &namespace, &ctx_session, &TTL_DAYS],
                        )
                        .await
                        .map_err(|e| pg_error(&e))?;
                    Ok(row.map(|r| r.get::<_, String>(0)))
                })
                .await?
            }
            HubStore::Sqlite(path) => {
                on_sqlite(path, move |db| {
                    db.query_row(
                        "UPDATE ctxone_mcp_sessions
                         SET seen_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
                         WHERE session_id = ?1 AND namespace = ?2 AND ctx_session = ?3
                           AND seen_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now',
                                                  printf('-%d days', ?4))
                         RETURNING state",
                        params![id, namespace, ctx_session, TTL_DAYS],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                })
                .await?
            }
        };
        Ok(state.map(|s| serde_json::from_str(&s)).transpose()?)
    }

    async fn store(&self, session_id: &str, state: &SessionState) -> Result<(), SessionStoreError> {
        let [namespace, ctx_session] = self.scope();
        let (id, state) = (session_id.to_owned(), serde_json::to_string(state)?);
        match &self.store {
            HubStore::Postgres(pg) => {
                let (pool, tenant) = (pg.tenant_pool(), pg.tenant().to_owned());
                on_pool_runtime(async move {
                    let client = pool.get().await.map_err(|e| e.to_string())?;
                    client
                        .execute(
                            "INSERT INTO ctxone_mcp_sessions
                                (tenant_id, session_id, namespace, ctx_session, state)
                             VALUES ($1, $2, $3, $4, $5)
                             ON CONFLICT (tenant_id, session_id) DO UPDATE SET
                                namespace = excluded.namespace, ctx_session = excluded.ctx_session,
                                state = excluded.state, seen_at = now()",
                            &[&tenant, &id, &namespace, &ctx_session, &state],
                        )
                        .await
                        .map_err(|e| pg_error(&e))?;
                    // New sessions are rare enough to pay for forgetting old ones.
                    client
                        .execute(
                            "DELETE FROM ctxone_mcp_sessions
                             WHERE tenant_id = $1 AND seen_at <= now() - make_interval(days => $2)",
                            &[&tenant, &TTL_DAYS],
                        )
                        .await
                        .map_err(|e| pg_error(&e))?;
                    Ok(())
                })
                .await
            }
            HubStore::Sqlite(path) => {
                on_sqlite(path, move |db| {
                    db.execute(
                        "INSERT INTO ctxone_mcp_sessions (session_id, namespace, ctx_session, state)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (session_id) DO UPDATE SET
                            namespace = excluded.namespace, ctx_session = excluded.ctx_session,
                            state = excluded.state,
                            seen_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
                        params![id, namespace, ctx_session, state],
                    )?;
                    // As on Postgres, each new session forgets the old ones.
                    db.execute(
                        "DELETE FROM ctxone_mcp_sessions
                         WHERE seen_at <= strftime('%Y-%m-%dT%H:%M:%SZ', 'now',
                                                   printf('-%d days', ?1))",
                        params![TTL_DAYS],
                    )?;
                    Ok(())
                })
                .await
            }
        }
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionStoreError> {
        let id = session_id.to_owned();
        match &self.store {
            HubStore::Postgres(pg) => {
                let (pool, tenant) = (pg.tenant_pool(), pg.tenant().to_owned());
                on_pool_runtime(async move {
                    let client = pool.get().await.map_err(|e| e.to_string())?;
                    client
                        .execute(
                            "DELETE FROM ctxone_mcp_sessions WHERE tenant_id = $1 AND session_id = $2",
                            &[&tenant, &id],
                        )
                        .await
                        .map_err(|e| pg_error(&e))?;
                    Ok(())
                })
                .await
            }
            HubStore::Sqlite(path) => {
                on_sqlite(path, move |db| {
                    db.execute(
                        "DELETE FROM ctxone_mcp_sessions WHERE session_id = ?1",
                        params![id],
                    )?;
                    Ok(())
                })
                .await
            }
        }
    }
}
