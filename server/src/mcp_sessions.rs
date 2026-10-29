//! MCP sessions that any Hub process sharing the database can continue
//! (ctx-multi-tenant-hub t-012).
//!
//! A streamable-HTTP MCP session lives in the process that began it. rmcp
//! keeps what it takes to continue one elsewhere, the client's `initialize`
//! parameters, in a [`SessionStore`]: when a request names a session the
//! process doesn't hold, rmcp loads them and replays the handshake, and the
//! client carries on as if nothing happened. [`McpSessionStore`] keeps them in
//! the Hub's Postgres tables, so a session continues on another replica, after
//! its own process let it go for being idle (rmcp after five minutes, the
//! tenant cache after thirty), and after a restart. Without it the client gets
//! the 404 the MCP spec prescribes and has to start a new session.
//!
//! A session continues only in the workspace and statistics session it began
//! in, and only for its tenant: the rows are the tenant's, under its policy.
//! One unused for [`TTL_DAYS`] days is forgotten.

use agentstategraph_storage::PostgresStorage;
use rmcp::transport::streamable_http_server::session::store::{
    SessionState, SessionStore, SessionStoreError,
};

use crate::hub_store::{PgStore, pg_error};

/// Days a session can go unused before it can no longer be continued
/// elsewhere. Its own process can keep serving it for longer.
pub const TTL_DAYS: i32 = 7;

/// The MCP sessions of one tenant's service for one (workspace, statistics
/// session) pair, which is what each service in [`crate::mcp_http`] serves.
#[derive(Debug)]
pub struct McpSessionStore {
    pg: PgStore,
    namespace: String,
    ctx_session: String,
}

impl McpSessionStore {
    pub fn new(pg: PgStore, namespace: &str, ctx_session: &str) -> Self {
        Self {
            pg,
            namespace: namespace.to_owned(),
            ctx_session: ctx_session.to_owned(),
        }
    }

    /// The pool, and the tenant, workspace and statistics session that
    /// scope this store's rows.
    fn scope(&self) -> (agentstategraph_storage::TenantPool, [String; 3]) {
        let keys = [
            self.pg.tenant().to_owned(),
            self.namespace.clone(),
            self.ctx_session.clone(),
        ];
        (self.pg.tenant_pool(), keys)
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

#[async_trait::async_trait]
impl SessionStore for McpSessionStore {
    async fn load(&self, session_id: &str) -> Result<Option<SessionState>, SessionStoreError> {
        let (pool, [tenant, namespace, ctx_session]) = self.scope();
        let id = session_id.to_owned();
        let state = on_pool_runtime(async move {
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
        .await?;
        Ok(state.map(|s| serde_json::from_str(&s)).transpose()?)
    }

    async fn store(&self, session_id: &str, state: &SessionState) -> Result<(), SessionStoreError> {
        let (pool, [tenant, namespace, ctx_session]) = self.scope();
        let (id, state) = (session_id.to_owned(), serde_json::to_string(state)?);
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

    async fn delete(&self, session_id: &str) -> Result<(), SessionStoreError> {
        let (pool, [tenant, ..]) = self.scope();
        let id = session_id.to_owned();
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
}
