//! Where the hub keeps the state that lives outside the graph: the project
//! registry ([`crate::project`]) and session statistics
//! ([`crate::memory_tools::SessionRegistry`]), and the MCP sessions another
//! Hub process can continue ([`crate::mcp_sessions`]).
//!
//! A SQLite hub keeps them in its database file (`projects`, `project_paths`,
//! `ctxone_sessions`, `ctxone_mcp_sessions`). A Postgres hub keeps them in
//! `ctxone_*` tables of the same database, through AgentStateGraph's
//! connection pool, so TLS and the pool size are the storage's and a hub pod
//! holds one set of connections.
//! Every Postgres row carries its tenant, so tenants sharing a database, in
//! one hub or in several, never see each other's registry or statistics.
//! Each table has AgentStateGraph's tenant policy too, so with row-level
//! security switched on (`ctxone-hub db row-security on`) the database backs
//! up every query's tenant filter, for these tables as for the graph's.

use agentstategraph_storage::postgres::add_tenant_policy;
use agentstategraph_storage::{PostgresStorage, TenantPool};

/// The hub's own state store.
#[derive(Clone, Debug)]
pub enum HubStore {
    /// Tables in the hub's SQLite file at this path.
    Sqlite(String),
    /// Tables in the hub's Postgres database.
    Postgres(PgStore),
}

/// The hub's tables in Postgres: AgentStateGraph's pool, for the tenant that
/// scopes every row. Each connection it hands out has that tenant set, which
/// is what row-level security compares rows with.
#[derive(Clone, Debug)]
pub struct PgStore {
    pool: TenantPool,
}

/// Serializes schema setup across every hub sharing the database
/// ("ctx-sch" as bytes). `CREATE TABLE IF NOT EXISTS` from two processes at
/// once can still fail on Postgres's catalog unique index.
const SCHEMA_LOCK_KEY: i64 = 0x0063_7478_2d73_6368;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS ctxone_projects (
        tenant_id    TEXT NOT NULL,
        id           TEXT NOT NULL,
        remote_url   TEXT,
        namespace_id TEXT NOT NULL,
        display_name TEXT,
        created_at   TEXT NOT NULL
            DEFAULT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'),
        PRIMARY KEY (tenant_id, id),
        UNIQUE (tenant_id, remote_url)
    );
    CREATE TABLE IF NOT EXISTS ctxone_project_paths (
        tenant_id  TEXT NOT NULL,
        project_id TEXT NOT NULL,
        local_path TEXT NOT NULL,
        PRIMARY KEY (tenant_id, project_id, local_path),
        FOREIGN KEY (tenant_id, project_id) REFERENCES ctxone_projects (tenant_id, id)
    );
    CREATE TABLE IF NOT EXISTS ctxone_sessions (
        tenant_id               TEXT NOT NULL,
        session_id              TEXT NOT NULL,
        tokens_sent             BIGINT NOT NULL DEFAULT 0,
        tokens_saved            BIGINT NOT NULL DEFAULT 0,
        llm_input_tokens        BIGINT NOT NULL DEFAULT 0,
        llm_output_tokens       BIGINT NOT NULL DEFAULT 0,
        llm_cache_read_tokens   BIGINT NOT NULL DEFAULT 0,
        llm_cache_create_tokens BIGINT NOT NULL DEFAULT 0,
        llm_call_count          BIGINT NOT NULL DEFAULT 0,
        last_model              TEXT,
        last_provider           TEXT,
        extra_tokens            TEXT,
        by_model                TEXT,
        updated_at              TEXT NOT NULL,
        PRIMARY KEY (tenant_id, session_id)
    );
    CREATE TABLE IF NOT EXISTS ctxone_mcp_sessions (
        tenant_id   TEXT NOT NULL,
        session_id  TEXT NOT NULL,
        namespace   TEXT NOT NULL,
        ctx_session TEXT NOT NULL,
        state       TEXT NOT NULL,
        seen_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
        PRIMARY KEY (tenant_id, session_id)
    );
";

/// The hub's tables, each of which gets the tenant policy.
pub const TABLES: &[&str] = &[
    "ctxone_projects",
    "ctxone_project_paths",
    "ctxone_sessions",
    "ctxone_mcp_sessions",
];

impl PgStore {
    /// The hub's tables for `storage`'s tenant, on its pool. Creates them if
    /// they're missing, so a fresh database works at once, and gives each one
    /// the tenant policy if it lacks it, as tables an earlier hub made do.
    pub fn new(storage: &PostgresStorage) -> Result<Self, String> {
        let store = Self {
            pool: storage.tenant_pool(),
        };
        let pool = store.pool.clone();
        PostgresStorage::run(async move {
            let mut client = pool.get().await.map_err(|e| e.to_string())?;
            let tx = client.transaction().await.map_err(|e| pg_error(&e))?;
            tx.execute("SELECT pg_advisory_xact_lock($1)", &[&SCHEMA_LOCK_KEY])
                .await
                .map_err(|e| pg_error(&e))?;
            tx.batch_execute(SCHEMA).await.map_err(|e| pg_error(&e))?;
            for table in TABLES {
                add_tenant_policy(&tx, table)
                    .await
                    .map_err(|e| format!("add the tenant policy to {table}: {}", pg_error(&e)))?;
            }
            tx.commit().await.map_err(|e| pg_error(&e))
        })
        .map_err(|e| format!("set up the hub's Postgres tables: {e}"))?;
        Ok(store)
    }

    /// The same tables for another tenant, on the same pool. Nothing to set
    /// up: [`new`](Self::new) created the tables for every tenant.
    pub fn for_tenant(&self, tenant: &str) -> Self {
        Self {
            pool: self.pool.for_tenant(tenant),
        }
    }

    /// The tenant every row is scoped to.
    pub fn tenant(&self) -> &str {
        self.pool.tenant_id()
    }

    /// The pool, for this tenant: use it inside [`PostgresStorage::run`].
    pub(crate) fn tenant_pool(&self) -> TenantPool {
        self.pool.clone()
    }
}

/// A Postgres error with the database's reason; `tokio_postgres::Error`'s own
/// `Display` says only "db error".
pub(crate) fn pg_error(e: &agentstategraph_storage::tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => format!("{}: {}", db.severity(), db.message()),
        None => e.to_string(),
    }
}
