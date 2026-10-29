//! Row-level security on a Hub's database (ctx-multi-tenant-hub t-011):
//! `ctxone-hub db row-security` switches it for the Hub's own tables with
//! AgentStateGraph's, each `ctxone_*` table's policy shows a connection only
//! its tenant's rows, and a Hub says at startup whether it binds it.
//!
//! Each test runs in a schema of its own, owned by a role that's neither a
//! superuser nor `BYPASSRLS` (see `support/row_security.rs`). Skipped unless
//! `TEST_DATABASE_URL` is set; `CTXONE_REQUIRE_POSTGRES` turns the skip into a
//! failure (CI sets both).

#[path = "support/row_security.rs"]
mod row_security;

use std::time::Duration;

use agentstategraph_storage::PostgresStorage;
use agentstategraph_storage::tokio_postgres;
use ctxone_hub::hub_store::{self, HubStore, PgStore};
use ctxone_hub::mcp_sessions::McpSessionStore;
use ctxone_hub::memory_tools::SessionRegistry;
use ctxone_hub::tenant::TokenHash;
use ctxone_hub::tenant_admin::TENANT_TABLES;
use rmcp::transport::streamable_http_server::session::store::{SessionState, SessionStore};
use row_security::{Schema, connect};

fn url() -> Option<String> {
    match std::env::var("TEST_DATABASE_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            let required =
                std::env::var("CTXONE_REQUIRE_POSTGRES").is_ok_and(|v| !v.is_empty() && v != "0");
            assert!(
                !required,
                "CTXONE_REQUIRE_POSTGRES is set, but TEST_DATABASE_URL isn't"
            );
            eprintln!("skip: TEST_DATABASE_URL not set");
            None
        }
    }
}

#[derive(Debug)]
struct Ran {
    code: i32,
    out: String,
    err: String,
}

/// `ctxone-hub db <args>`, run as the binary.
fn db_cmd(args: &[&str]) -> Ran {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ctxone-hub"))
        .arg("db")
        .args(args)
        .env_remove("DATABASE_URL")
        .output()
        .expect("run ctxone-hub db");
    Ran {
        code: output.status.code().unwrap_or(-1),
        out: String::from_utf8(output.stdout).unwrap(),
        err: String::from_utf8(output.stderr).unwrap(),
    }
}

/// `ctxone-hub db row-security <command>` on the database at `url`.
async fn row_security(command: &str, url: &str) -> Ran {
    let (command, url) = (command.to_owned(), url.to_owned());
    tokio::task::spawn_blocking(move || db_cmd(&["row-security", &command, "--database-url", &url]))
        .await
        .unwrap()
}

/// The schema's tables with row-level security enabled and forced, sorted.
async fn forced(db: &tokio_postgres::Client) -> Vec<String> {
    db.query(
        "SELECT c.relname::text FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = current_schema() AND c.relkind = 'r'
           AND c.relrowsecurity AND c.relforcerowsecurity
         ORDER BY 1",
        &[],
    )
    .await
    .unwrap()
    .iter()
    .map(|r| r.get(0))
    .collect()
}

/// Whether `table` in the schema has the tenant policy.
async fn has_policy(db: &tokio_postgres::Client, table: &str) -> bool {
    db.query_one(
        "SELECT EXISTS (SELECT 1 FROM pg_policies WHERE schemaname = current_schema()
                        AND tablename = $1 AND policyname = 'asg_tenant')",
        &[&table],
    )
    .await
    .unwrap()
    .get(0)
}

/// Every table that holds tenants' rows, sorted.
fn all_tables() -> Vec<String> {
    let mut tables: Vec<String> = TENANT_TABLES.iter().map(|t| t.to_string()).collect();
    tables.sort();
    tables
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_db_command_switches_the_hubs_tables_with_the_graphs() {
    let Some(url) = url() else { return };
    let schema = Schema::new(&url, "cmd").await;
    let admin = connect(&schema.admin_url).await;
    let count = TENANT_TABLES.len();

    // On a schema no Hub has used yet: it sets the tables up first.
    let r = row_security("status", &schema.app_url).await;
    assert_eq!((r.code, r.out.as_str()), (0, "off\n"), "{r:?}");
    assert!(
        r.err.contains(&format!("off for all {count} tables")),
        "{r:?}"
    );
    assert!(!r.err.contains("BYPASSRLS"), "{r:?}");
    assert!(forced(&admin).await.is_empty());
    for table in hub_store::TABLES {
        assert!(has_policy(&admin, table).await, "{table} has no policy");
    }

    // A Hub table from before the policy gets it from the next Hub to start,
    // or from the command, before anything is switched.
    admin
        .batch_execute("DROP POLICY asg_tenant ON ctxone_sessions")
        .await
        .unwrap();
    let r = row_security("on", &schema.app_url).await;
    assert_eq!((r.code, r.out.as_str()), (0, "on\n"), "{r:?}");
    assert!(
        r.err.contains(&format!("on for all {count} tables")),
        "{r:?}"
    );
    assert!(!r.err.contains("no tenant policy"), "{r:?}");
    assert_eq!(forced(&admin).await, all_tables());

    let r = row_security("status", &schema.app_url).await;
    assert_eq!((r.code, r.out.as_str()), (0, "on\n"), "{r:?}");

    // The admin role skips it, and the command says so.
    let r = row_security("status", &schema.admin_url).await;
    assert_eq!((r.code, r.out.as_str()), (0, "on\n"), "{r:?}");
    assert!(r.err.contains("superuser or has BYPASSRLS"), "{r:?}");

    // A table with tenants' rows that nothing guards is named.
    admin
        .batch_execute("CREATE TABLE unguarded_rows (tenant_id TEXT NOT NULL)")
        .await
        .unwrap();
    let r = row_security("status", &schema.admin_url).await;
    assert_eq!((r.code, r.out.as_str()), (0, "on\n"), "{r:?}");
    assert!(
        r.err.contains("no tenant policy") && r.err.contains("unguarded_rows"),
        "{r:?}"
    );
    admin
        .batch_execute("DROP TABLE unguarded_rows")
        .await
        .unwrap();

    let r = row_security("off", &schema.app_url).await;
    assert_eq!((r.code, r.out.as_str()), (0, "off\n"), "{r:?}");
    assert!(forced(&admin).await.is_empty());

    // Without a database it's a usage error, before connecting to anything.
    let r = tokio::task::spawn_blocking(|| db_cmd(&["row-security", "status"]))
        .await
        .unwrap();
    assert_eq!(r.code, 64, "{r:?}");
    assert!(r.err.contains("--database-url"), "{r:?}");
    schema.drop().await;
}

/// The rows of `table` that `db` sees, those `filter` (a `WHERE` clause, or
/// nothing) picks.
async fn rows(db: &tokio_postgres::Client, table: &str, filter: &str) -> i64 {
    let sql = format!("SELECT count(*) FROM {table} {filter}");
    db.query_one(&sql, &[]).await.unwrap().get(0)
}

/// `tokens_sent` for the session `seed` gave `tenant`.
async fn sent(db: &tokio_postgres::Client, tenant: &str) -> i64 {
    db.query_one(
        "SELECT tokens_sent FROM ctxone_sessions WHERE tenant_id = $1 AND session_id = $2",
        &[&tenant, &format!("{tenant}-session")],
    )
    .await
    .unwrap()
    .get(0)
}

/// Rows of two tenants in each of the Hub's tables: a project with a path,
/// session statistics and an MCP session. Call it from a blocking thread.
fn seed(store: &HubStore, session: &str) {
    store
        .register_project(
            "app",
            Some("https://git.example.com/app.git"),
            "app",
            None,
            Some("/work/app"),
        )
        .unwrap();
    let registry = SessionRegistry::new();
    registry.get_or_create(session).record(100, 10);
    registry.flush(store);
    let HubStore::Postgres(pg) = store else {
        unreachable!("a Postgres store")
    };
    let params = serde_json::from_value(serde_json::json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": { "name": "itest", "version": "0" }
    }))
    .unwrap();
    tokio::runtime::Handle::current()
        .block_on(
            McpSessionStore::new(pg.clone(), "default", session)
                .store(&format!("{session}-mcp"), &SessionState::new(params)),
        )
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_it_on_a_connection_sees_and_writes_only_its_tenants_rows_in_the_hubs_tables() {
    let Some(url) = url() else { return };
    let schema = Schema::new(&url, "rows").await;
    let app_url = schema.app_url.clone();
    let (alpha, bravo) = tokio::task::spawn_blocking(move || {
        let storage = PostgresStorage::connect_tenant_blocking(&app_url, "alpha", 2).unwrap();
        let alpha = PgStore::new(&storage).unwrap();
        let bravo = alpha.for_tenant("bravo");
        let status = storage.set_row_security(true).unwrap();
        assert!(status.enforced(), "{status:?}");
        // The Hub's own reads and writes go on working, each in its tenant.
        let (alpha, bravo) = (HubStore::Postgres(alpha), HubStore::Postgres(bravo));
        seed(&alpha, "alpha-session");
        seed(&bravo, "bravo-session");
        let ids = |store: &HubStore| {
            let mut ids = SessionRegistry::load(store).list_ids();
            ids.retain(|id| id != "default");
            ids
        };
        assert_eq!(ids(&alpha), ["alpha-session"]);
        assert_eq!(ids(&bravo), ["bravo-session"]);
        assert_eq!(alpha.list_projects().unwrap().len(), 1);
        (alpha, bravo)
    })
    .await
    .unwrap();

    let admin = connect(&schema.admin_url).await;
    let app = connect(&schema.app_url).await;
    let mut alphas = Vec::new();
    for table in hub_store::TABLES {
        let alpha = rows(&admin, table, "WHERE tenant_id = 'alpha'").await;
        let bravo = rows(&admin, table, "WHERE tenant_id = 'bravo'").await;
        assert!(alpha > 0 && bravo > 0, "{table}: {alpha} and {bravo}");
        assert_eq!(rows(&admin, table, "").await, alpha + bravo, "{table}");
        // No tenant set: no rows.
        assert_eq!(rows(&app, table, "").await, 0, "{table}");
        alphas.push(alpha);
    }
    app.batch_execute("SELECT set_config('asg.tenant', 'alpha', false)")
        .await
        .unwrap();
    for (table, alpha) in hub_store::TABLES.iter().zip(alphas) {
        // No tenant filter in the query: the policy is the filter.
        assert_eq!(rows(&app, table, "").await, alpha, "{table}");
    }
    let refused = [
        "INSERT INTO ctxone_projects (tenant_id, id, namespace_id) VALUES ('bravo', 'x', 'x')",
        "INSERT INTO ctxone_project_paths (tenant_id, project_id, local_path)
         VALUES ('bravo', 'app', '/elsewhere')",
        "INSERT INTO ctxone_sessions (tenant_id, session_id, updated_at)
         VALUES ('bravo', 'x', 'now')",
        "UPDATE ctxone_sessions SET tenant_id = 'bravo'",
        "INSERT INTO ctxone_mcp_sessions (tenant_id, session_id, namespace, ctx_session, state)
         VALUES ('bravo', 'x', 'default', 'x', '{}')",
    ];
    for sql in refused {
        let err = app.batch_execute(sql).await.expect_err(sql);
        assert_eq!(
            err.code(),
            Some(&tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE),
            "{sql}: {err:?}"
        );
    }
    // A statement meant for every row reaches only alpha's.
    let bravo_sent = sent(&admin, "bravo").await;
    assert!(sent(&admin, "alpha").await > 0 && bravo_sent > 0);
    app.batch_execute("UPDATE ctxone_sessions SET tokens_sent = 0")
        .await
        .unwrap();
    assert_eq!(sent(&admin, "alpha").await, 0);
    assert_eq!(sent(&admin, "bravo").await, bravo_sent);
    drop((alpha, bravo));
    schema.drop().await;
}

/// A Hub process, killed when the test ends however it ends.
#[cfg(unix)]
struct Running(std::process::Child);

#[cfg(unix)]
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Start the Hub binary with `args`, and return its log once it has said where
/// row-level security stands.
#[cfg(unix)]
async fn startup_log(args: &[&str]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let log_path = dir.path().join("hub.log");
    let mut hub = Running(
        std::process::Command::new(env!("CARGO_BIN_EXE_ctxone-hub"))
            .args(["--http", "--port", &port.to_string(), "--pg-pool-size", "2"])
            .args(args)
            .current_dir(dir.path())
            .env("HOME", dir.path())
            .env_remove("DATABASE_URL")
            .env_remove("CTXONE_TENANTS_FILE")
            .env_remove("CTXONE_AUTH_TOKEN")
            .env_remove("CTXONE_AUTH_TOKEN_FILE")
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&log_path).unwrap())
            .spawn()
            .expect("start ctxone-hub"),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let log = std::fs::read_to_string(&log_path).unwrap();
        if log.contains("row-level security") {
            return log;
        }
        if let Some(status) = hub.0.try_wait().unwrap() {
            panic!("the hub exited ({status}): {log}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the hub said nothing of row-level security: {log}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hub_says_at_startup_whether_row_security_binds_it() {
    let Some(url) = url() else { return };
    let schema = Schema::new(&url, "startup").await;
    let r = row_security("on", &schema.app_url).await;
    assert_eq!((r.code, r.out.as_str()), (0, "on\n"), "{r:?}");

    // One tenant, as the admin role: it skips row-level security.
    let log = startup_log(&["--storage", "postgres", "--database-url", &schema.admin_url]).await;
    assert!(log.contains("superuser or has BYPASSRLS"), "{log}");

    // Many tenants, as the role it binds.
    let dir = tempfile::tempdir().unwrap();
    let tenants = dir.path().join("tenants");
    std::fs::write(&tenants, format!("alpha {}\n", TokenHash::of("t").to_hex())).unwrap();
    let log = startup_log(&[
        "--tenants-file",
        tenants.to_str().unwrap(),
        "--storage",
        "postgres",
        "--database-url",
        &schema.app_url,
    ])
    .await;
    assert!(
        log.contains(&format!(
            "row-level security is on for all {} tables",
            TENANT_TABLES.len()
        )),
        "{log}"
    );
    assert!(!log.contains("BYPASSRLS"), "{log}");
    schema.drop().await;
}
