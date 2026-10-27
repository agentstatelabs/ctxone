//! `ctxone-hub tenant` (ctx-multi-tenant-hub t-006): the commands that edit a
//! tenants file, the binary running them, and purge deleting a disabled
//! tenant's rows from Postgres and nothing else.
//!
//! The Postgres tests are skipped unless `TEST_DATABASE_URL` is set;
//! `CTXONE_REQUIRE_POSTGRES` turns the skip into a failure (CI sets both).
//! Tenant names are random, so a shared database is fine.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agentstategraph_storage::PostgresStorage;
use agentstategraph_storage::tokio_postgres;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use ctxone_hub::http;
use ctxone_hub::tenant::{Tenant, TenantsFile};
use ctxone_hub::tenant_admin::{Console, TENANT_TABLES, purge_rows, run};
use ctxone_hub::tenant_cache::{PostgresTenants, ROOT_TENANT, TenantCache};

/// What a `ctxone-hub tenant` command printed, and its exit code.
#[derive(Debug)]
struct Ran {
    code: i32,
    out: String,
    err: String,
}

/// Run `ctxone-hub tenant <args>` with no terminal to ask on, and no wait
/// before a purge deletes.
fn tenant_cmd(args: &[&str]) -> Ran {
    ran(args, None)
}

/// [`tenant_cmd`], answering purge's question with `answer`.
fn tenant_cmd_answering(args: &[&str], answer: &str) -> Ran {
    ran(args, Some(answer))
}

fn ran(args: &[&str], answer: Option<&str>) -> Ran {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let answer = answer.map(|a| format!("{a}\n"));
    let mut reply = || Ok(answer.clone().unwrap_or_default());
    let ask: Option<&mut dyn FnMut() -> std::io::Result<String>> = match answer {
        Some(_) => Some(&mut reply),
        None => None,
    };
    let mut console = Console {
        out: &mut out,
        err: &mut err,
        ask,
    };
    let code = run(&args, &mut console, Duration::ZERO);
    Ran {
        code,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

/// The tenant `token` reaches through the file at `path`, as a Hub reads it.
fn reaches(path: &Path, token: &str) -> Option<String> {
    TenantsFile::open(path)
        .unwrap()
        .lookup(token)
        .map(|t| t.to_string())
}

fn tenants_file() -> (tempfile::TempDir, PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    let shown = path.to_str().unwrap().to_owned();
    (dir, path, shown)
}

/// Add `name` and return its token.
fn add(file: &str, name: &str) -> String {
    let r = tenant_cmd(&["add", name, "--tenants-file", file]);
    assert_eq!(r.code, 0, "{r:?}");
    r.out.trim().to_owned()
}

#[test]
fn add_prints_the_token_alone_and_list_shows_the_tenants() {
    let (_dir, path, file) = tenants_file();
    let r = tenant_cmd(&["add", "alice", "--tenants-file", &file]);
    assert_eq!(r.code, 0, "{r:?}");
    // Only the token on stdout, for a script to read.
    assert_eq!(r.out.lines().count(), 1, "{r:?}");
    let token = r.out.trim();
    assert!(token.starts_with("ctx_"), "{r:?}");
    assert!(r.err.contains("Added tenant alice"), "{r:?}");
    assert_eq!(reaches(&path, token).as_deref(), Some("alice"));

    add(&file, "bob");
    assert_eq!(
        tenant_cmd(&["disable", "bob", "--tenants-file", &file]).code,
        0
    );
    let r = tenant_cmd(&["list", "--tenants-file", &file]);
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "alice\t1 token\nbob\tdisabled (1 token)\n");

    let r = tenant_cmd(&["add", "alice", "--tenants-file", &file]);
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("already has tenant alice"), "{r:?}");
}

#[test]
fn rotate_disable_and_enable_change_which_tokens_work() {
    let (_dir, path, file) = tenants_file();
    let first = add(&file, "alice");

    let r = tenant_cmd(&["rotate", "alice", "--tenants-file", &file]);
    assert_eq!(r.code, 0, "{r:?}");
    let second = r.out.trim();
    assert_eq!(reaches(&path, &first), None);
    assert_eq!(reaches(&path, second).as_deref(), Some("alice"));

    let r = tenant_cmd(&["disable", "alice", "--tenants-file", &file]);
    assert!(r.err.contains("Disabled tenant alice's 1 token"), "{r:?}");
    assert_eq!(reaches(&path, second), None);
    let r = tenant_cmd(&["disable", "alice", "--tenants-file", &file]);
    assert!(r.err.contains("already disabled"), "{r:?}");
    let r = tenant_cmd(&["rotate", "alice", "--tenants-file", &file]);
    assert_eq!(r.code, 1, "{r:?}");

    let r = tenant_cmd(&["enable", "alice", "--tenants-file", &file]);
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(reaches(&path, second).as_deref(), Some("alice"));

    let r = tenant_cmd(&["rotate", "bob", "--tenants-file", &file]);
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("has no tenant bob"), "{r:?}");
}

#[test]
fn commands_used_wrongly_exit_with_usage() {
    let (_dir, _path, file) = tenants_file();
    let help = tenant_cmd(&[]);
    assert_eq!(help.code, 0);
    assert!(help.out.contains("Usage: ctxone-hub tenant"), "{help:?}");
    for args in [
        vec!["frob", "--tenants-file", &file],
        vec!["add", "--tenants-file", &file],
        vec!["add", "Alice", "--tenants-file", &file],
        vec!["add", "alice", "bob", "--tenants-file", &file],
        vec!["list", "alice", "--tenants-file", &file],
        vec!["add", "alice", "--tenants-file"],
        vec!["add", "alice", "--bogus", "--tenants-file", &file],
    ] {
        let r = tenant_cmd(&args);
        assert_eq!(r.code, 64, "{args:?}: {r:?}");
        assert!(r.err.contains("Usage:"), "{args:?}: {r:?}");
    }
    // Nothing was written.
    assert!(!Path::new(&file).exists());
}

#[test]
fn purge_refuses_before_deleting_anything() {
    let (_dir, path, file) = tenants_file();
    let token = add(&file, "alice");
    // Never reached: each refusal comes first.
    let db = "postgres://nobody@unused.invalid/none";

    let r = tenant_cmd(&[
        "purge",
        "alice",
        "--tenants-file",
        &file,
        "--database-url",
        db,
        "--yes",
    ]);
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("Disable it first"), "{r:?}");

    tenant_cmd(&["disable", "alice", "--tenants-file", &file]);
    let purge = [
        "purge",
        "alice",
        "--tenants-file",
        &file,
        "--database-url",
        db,
    ];
    let r = tenant_cmd(&purge);
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("Run it from a terminal"), "{r:?}");
    let r = tenant_cmd_answering(&purge, "bob");
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("Type the tenant's name"), "{r:?}");
    assert!(r.err.contains("nothing was deleted"), "{r:?}");

    // Still there, disabled.
    tenant_cmd(&["enable", "alice", "--tenants-file", &file]);
    assert_eq!(reaches(&path, &token).as_deref(), Some("alice"));

    // A tenant the file doesn't list, or a file that isn't there, could be a
    // wrong --tenants-file with the tenant in use elsewhere.
    let r = tenant_cmd(&[
        "purge",
        "bob",
        "--tenants-file",
        &file,
        "--database-url",
        db,
        "--yes",
    ]);
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("has no tenant bob"), "{r:?}");
    let missing = format!("{file}-typo");
    let r = tenant_cmd(&[
        "purge",
        "alice",
        "--tenants-file",
        &missing,
        "--database-url",
        db,
        "--yes",
    ]);
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("there's no tenants file at"), "{r:?}");
    assert!(!Path::new(&missing).exists());
}

#[test]
fn a_missing_tenants_file_is_named_not_made() {
    let (_dir, path, file) = tenants_file();
    for args in [
        vec!["list", "--tenants-file", &file],
        vec!["disable", "alice", "--tenants-file", &file],
        vec!["rotate", "alice", "--tenants-file", &file],
    ] {
        let r = tenant_cmd(&args);
        assert_eq!(r.code, 1, "{args:?}: {r:?}");
        assert!(
            r.err.contains("there's no tenants file at"),
            "{args:?}: {r:?}"
        );
    }
    assert!(!path.exists());
}

#[test]
fn the_binary_runs_tenant_commands() {
    let (_dir, path, file) = tenants_file();
    let hub = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_ctxone-hub"))
            .arg("tenant")
            .args(args)
            .env_remove("CTXONE_TENANTS_FILE")
            .output()
            .unwrap()
    };
    let added = hub(&["add", "alice", "--tenants-file", &file]);
    assert!(added.status.success(), "{added:?}");
    let token = String::from_utf8(added.stdout).unwrap();
    assert_eq!(reaches(&path, token.trim()).as_deref(), Some("alice"));

    let listed = hub(&["list", "--tenants-file", &file]);
    assert_eq!(String::from_utf8_lossy(&listed.stdout), "alice\t1 token\n");
    let wrong = hub(&["frob"]);
    assert_eq!(wrong.status.code(), Some(64), "{wrong:?}");
}

// -- Purge on Postgres --

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

/// A tenant name no other test run uses.
fn fresh(tag: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{tag}-{}", &id[..12])
}

struct PgHub {
    app: axum::Router,
    data: TenantCache,
    file: TenantsFile,
    path: String,
    _dir: tempfile::TempDir,
}

/// A Hub on Postgres whose tenants file `ctxone-hub tenant` edits, re-read on
/// every request.
async fn pg_hub(url: &str) -> PgHub {
    let root = PostgresStorage::connect_tenant_with_pool_size(url, ROOT_TENANT, 4)
        .await
        .expect("connect");
    let data = TenantCache::new(Arc::new(PostgresTenants::new(root).expect("tenants")));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    std::fs::write(&path, "").unwrap();
    let file = TenantsFile::open_with_reload(&path, Duration::ZERO).unwrap();
    let app = http::multi_tenant_router(
        http::HubConfig::default(),
        http::MultiTenant {
            resolver: Arc::new(file.clone()),
            data: data.clone(),
        },
    );
    PgHub {
        app,
        data,
        file,
        path: path.to_str().unwrap().to_owned(),
        _dir: dir,
    }
}

fn request(method: &str, uri: &str, token: &str, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-ctxone-session", "purge-session");
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    let body = body.map_or_else(Body::empty, |v| Body::from(v.to_string()));
    let mut req = b.body(body).unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "203.0.113.7:40000".parse::<SocketAddr>().unwrap(),
    ));
    req
}

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = app.clone().oneshot(req).await.expect("router call");
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn ok(app: &axum::Router, token: &str, method: &str, uri: &str, body: Value) -> String {
    let body = (!body.is_null()).then_some(body);
    let (status, text) = call(app, request(method, uri, token, body)).await;
    assert!(status.is_success(), "{method} {uri}: {status}: {text}");
    text
}

/// Give a tenant data in as many tables as the API reaches: a project and its
/// path, facts and their search index, a closed plan's epoch, a reminder, a
/// taint and session statistics.
async fn seed(hub: &PgHub, token: &str, marker: &str) {
    let post = |uri: &'static str, body: Value| ok(&hub.app, token, "POST", uri, body);
    post(
        "/api/projects",
        json!({ "id": "app", "remote_url": "https://git.example.com/app.git", "local_path": "/work/app" }),
    )
    .await;
    for fact in ["pins the release train", "keeps the changelog"] {
        post(
            "/api/memory/remember",
            json!({ "fact": format!("{marker} {fact}"), "context": "ops" }),
        )
        .await;
    }
    post("/api/plans", json!({ "name": "shipped" })).await;
    post("/api/plans/shipped/tasks", json!({ "title": "ship it" })).await;
    post("/api/plans/shipped/tasks/t-001/start", json!({})).await;
    post(
        "/api/plans/shipped/tasks/t-001/complete",
        json!({ "proof": { "kind": "text", "value": "shipped" } }),
    )
    .await;
    post(
        "/api/plans/shipped/close",
        json!({ "summary": format!("{marker} shipped") }),
    )
    .await;
    post("/api/plans/backfill_epochs", Value::Null).await;
    post(
        "/api/reminders",
        json!({ "title": "rotate keys", "instructions": "rotate them", "due_at": "2099-01-01T00:00:00Z" }),
    )
    .await;
    post(
        "/api/taint",
        json!({
            "path": "/memory/ops", "name": format!("{marker}-hold"), "kind": "taint",
            "effect": "warn", "reason": "held", "agent_id": "itest",
        }),
    )
    .await;
    for uri in [
        "/api/memory/recall?topic=release",
        // Builds the leaf index that search reads.
        "/api/state/main/search?query=release",
    ] {
        ok(&hub.app, token, "GET", uri, Value::Null).await;
    }
}

/// A connection of the test's own, to count rows.
async fn db(url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(connection);
    client
}

/// Every table of the database with a `tenant_id` column, whether or not
/// purge knows it.
async fn tenant_id_tables(db: &tokio_postgres::Client) -> Vec<String> {
    db.query(
        "SELECT c.table_name::text FROM information_schema.columns c
         JOIN information_schema.tables t
           ON t.table_schema = c.table_schema AND t.table_name = c.table_name
         WHERE c.table_schema = current_schema() AND c.column_name = 'tenant_id'
           AND t.table_type = 'BASE TABLE'
         ORDER BY 1",
        &[],
    )
    .await
    .unwrap()
    .iter()
    .map(|r| r.get(0))
    .collect()
}

/// `tenant`'s row count in each of `tables`.
async fn rows(db: &tokio_postgres::Client, tables: &[String], tenant: &str) -> Vec<(String, i64)> {
    let mut counts = Vec::new();
    for table in tables {
        let n: i64 = db
            .query_one(
                &format!("SELECT count(*) FROM \"{table}\" WHERE tenant_id = $1"),
                &[&tenant],
            )
            .await
            .unwrap()
            .get(0);
        counts.push((table.clone(), n));
    }
    counts
}

fn count(counts: &[(String, i64)], table: &str) -> i64 {
    counts
        .iter()
        .find(|(t, _)| t == table)
        .map_or(0, |(_, n)| *n)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn purge_deletes_every_row_of_the_tenant_and_nothing_else() {
    let Some(url) = url() else { return };
    let hub = pg_hub(&url).await;
    let (alice, bob) = (fresh("purged"), fresh("kept"));
    let alice_token = add(&hub.path, &alice);
    let bob_token = add(&hub.path, &bob);
    seed(&hub, &alice_token, "alpha").await;
    seed(&hub, &bob_token, "bravo").await;
    let data = hub.data.clone();
    tokio::task::spawn_blocking(move || data.flush_all())
        .await
        .unwrap();

    let db = db(&url).await;
    let tables = tenant_id_tables(&db).await;
    let alice_before = rows(&db, &tables, &alice).await;
    let bob_before = rows(&db, &tables, &bob).await;
    for table in [
        "objects",
        "commits",
        "namespaces",
        "refs",
        "epochs",
        "reminders",
        "taints",
        "leaf_rows",
        "ctxone_projects",
        "ctxone_project_paths",
        "ctxone_sessions",
    ] {
        assert!(
            count(&alice_before, table) > 0,
            "the seed left {table} empty: {alice_before:?}"
        );
    }

    // Disabled, and closed by the Hub, as purge's wait assumes.
    assert_eq!(
        tenant_cmd(&["disable", &alice, "--tenants-file", &hub.path]).code,
        0
    );
    assert_eq!(
        hub.data.close_refused(&hub.file),
        [Tenant::new(&alice).unwrap()]
    );

    let args = [
        "purge".to_string(),
        alice.clone(),
        "--tenants-file".into(),
        hub.path.clone(),
        "--database-url".into(),
        url.clone(),
        "--yes".into(),
    ];
    let r = std::thread::spawn(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        tenant_cmd(&args)
    })
    .join()
    .unwrap();
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.err.contains(&format!("Purged tenant {alice}")), "{r:?}");

    // Every table, purge's list or not, holds none of alice's rows, even
    // after the Hub flushes again: it closed alice, so it has nothing of
    // alice's to write.
    let data = hub.data.clone();
    tokio::task::spawn_blocking(move || data.flush_all())
        .await
        .unwrap();
    let alice_after = rows(&db, &tables, &alice).await;
    assert!(
        alice_after.iter().all(|(_, n)| *n == 0),
        "left behind: {alice_after:?}"
    );
    assert_eq!(rows(&db, &tables, &bob).await, bob_before);
    let r = tenant_cmd(&["list", "--tenants-file", &hub.path]);
    assert_eq!(r.out, format!("{bob}\t1 token\n"));
    let (_, recalled) = call(
        &hub.app,
        request("GET", "/api/memory/recall?topic=release", &bob_token, None),
    )
    .await;
    assert!(
        recalled.contains("bravo pins the release train"),
        "{recalled}"
    );

    // Added again, the name starts with nothing.
    let again = add(&hub.path, &alice);
    let (status, recalled) = call(
        &hub.app,
        request("GET", "/api/memory/recall?topic=release", &again, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{recalled}");
    assert!(!recalled.contains("alpha"), "{recalled}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn purge_refuses_a_database_with_a_tenant_table_it_does_not_know() {
    let Some(url) = url() else { return };
    let hub = pg_hub(&url).await;
    let alice = fresh("unpurged");
    let token = add(&hub.path, &alice);
    seed(&hub, &token, "alpha").await;

    let db = db(&url).await;
    let tables = tenant_id_tables(&db).await;
    let before = rows(&db, &tables, &alice).await;
    // Every table with a tenant_id is one purge knows.
    let unknown: Vec<&String> = tables
        .iter()
        .filter(|t| !TENANT_TABLES.contains(&t.as_str()))
        .collect();
    assert!(unknown.is_empty(), "purge doesn't know {unknown:?}");

    // As if `commits` were a table added after this list.
    let known: Vec<&'static str> = TENANT_TABLES
        .iter()
        .copied()
        .filter(|t| *t != "commits")
        .collect();
    let storage = PostgresStorage::connect_tenant_with_pool_size(&url, ROOT_TENANT, 1)
        .await
        .unwrap();
    let pool = storage.pool();
    let tenant = Tenant::new(&alice).unwrap();
    let err = tokio::task::spawn_blocking(move || purge_rows(&pool, &tenant, &known))
        .await
        .unwrap()
        .unwrap_err();
    assert!(err.contains("doesn't know: commits"), "{err}");
    assert!(err.contains("nothing was deleted"), "{err}");
    assert_eq!(rows(&db, &tables, &alice).await, before);
}
