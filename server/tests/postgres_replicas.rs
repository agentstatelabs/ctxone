//! More than one Hub on one Postgres database (ctx-multi-tenant-hub t-012):
//! an MCP session begun on one Hub continues on another, and Hubs flushing a
//! tenant's session statistics add them up instead of overwriting each other.
//! Each Hub here has its own pool, as a replica in its own process would.
//!
//! Skipped unless `TEST_DATABASE_URL` is set; `CTXONE_REQUIRE_POSTGRES` turns
//! the skip into a failure (CI sets both). Tenant names are random, so a shared
//! database is fine.

use std::net::SocketAddr;
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
use ctxone_hub::hub_store::{HubStore, PgStore};
use ctxone_hub::memory_tools::SessionRegistry;
use ctxone_hub::tenant::{Tenant, TenantsFile, TokenHash};
use ctxone_hub::tenant_cache::{PostgresTenants, TenantCache};

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
fn fresh(tag: &str) -> Tenant {
    let id = uuid::Uuid::new_v4().simple().to_string();
    Tenant::new(&format!("{tag}-{}", &id[..12])).unwrap()
}

/// Hubs serving the same tenants from one database, each on its own pool.
struct Replicas {
    hubs: Vec<(axum::Router, TenantCache)>,
    /// The tenants and their tokens.
    tenants: Vec<(Tenant, String)>,
    _dir: tempfile::TempDir,
}

async fn replicas(url: &str, hubs: usize, tenants: usize) -> Replicas {
    let tenants: Vec<(Tenant, String)> = (0..tenants)
        .map(|i| {
            (
                fresh(&format!("r{i}")),
                format!("token-{}", uuid::Uuid::new_v4()),
            )
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    let lines: String = tenants
        .iter()
        .map(|(t, token)| format!("{t} {}\n", TokenHash::of(token).to_hex()))
        .collect();
    std::fs::write(&path, lines).unwrap();
    let file = TenantsFile::open(&path).unwrap();
    let mut built = Vec::new();
    for _ in 0..hubs {
        let root = PostgresStorage::connect_tenant_with_pool_size(url, fresh("root").as_str(), 4)
            .await
            .expect("connect");
        let data = TenantCache::new(Arc::new(PostgresTenants::new(root).expect("tenants")));
        let app = http::multi_tenant_router(
            http::HubConfig {
                mcp_http: true,
                agent_id: "itest".into(),
                ..Default::default()
            },
            http::MultiTenant {
                resolver: Arc::new(file.clone()),
                data: data.clone(),
            },
        );
        built.push((app, data));
    }
    Replicas {
        hubs: built,
        tenants,
        _dir: dir,
    }
}

fn request(method: &str, uri: &str, token: &str, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "hub.example.test")
        .header("authorization", format!("Bearer {token}"))
        .header("x-ctxone-session", "replica-session");
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

fn mcp(uri: &str, token: &str, session: Option<&str>, body: Option<Value>) -> Request<Body> {
    let method = if body.is_some() { "POST" } else { "DELETE" };
    let mut req = request(method, uri, token, body);
    let headers = req.headers_mut();
    headers.insert(
        "accept",
        "application/json, text/event-stream".parse().unwrap(),
    );
    if let Some(id) = session {
        headers.insert("mcp-session-id", id.parse().unwrap());
    }
    req
}

/// Start an MCP session as `token`'s tenant on `app` and return its id.
async fn mcp_session(app: &axum::Router, token: &str) -> String {
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "itest", "version": "0" }
        }
    });
    let resp = app
        .clone()
        .oneshot(mcp("/mcp", token, None, Some(init)))
        .await
        .unwrap();
    if resp.status() != StatusCode::OK {
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        panic!("initialize: {status}: {}", String::from_utf8_lossy(&body));
    }
    let id = resp.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    resp.into_body().collect().await.unwrap();
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let (status, body) = call(app, mcp("/mcp", token, Some(&id), Some(initialized))).await;
    assert!(status.is_success(), "{status}: {body}");
    id
}

/// The JSON-RPC response in an MCP answer's SSE `data:` frames.
fn answer(body: &str) -> Value {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find(|m| m.get("result").is_some() || m.get("error").is_some())
        .unwrap_or_else(|| panic!("no JSON-RPC response in {body}"))
}

/// List the tools of MCP session `id` at `uri` on `app`.
async fn list_tools(app: &axum::Router, uri: &str, token: &str, id: &str) -> (StatusCode, String) {
    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
    call(app, mcp(uri, token, Some(id), Some(list))).await
}

/// A connection of the test's own.
async fn connect(url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(connection);
    client
}

/// Whether MCP session `id` of `tenant` is kept in the database.
async fn kept(db: &tokio_postgres::Client, tenant: &Tenant, id: &str) -> bool {
    db.query_one(
        "SELECT count(*) FROM ctxone_mcp_sessions WHERE tenant_id = $1 AND session_id = $2",
        &[&tenant.as_str(), &id],
    )
    .await
    .unwrap()
    .get::<_, i64>(0)
        > 0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_mcp_session_continues_on_another_hub_and_after_its_own_lets_it_go() {
    let Some(url) = url() else { return };
    let r = replicas(&url, 2, 1).await;
    let [(a, a_data), (b, _)] = &r.hubs[..] else {
        unreachable!()
    };
    let (tenant, token) = &r.tenants[0];

    let id = mcp_session(a, token).await;
    assert!(kept(&connect(&url).await, tenant, &id).await);

    // The other Hub has never seen the session, and continues it.
    let (status, body) = list_tools(b, "/mcp", token, &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        answer(&body)["result"]["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|t| t["name"] == "remember")),
        "{body}"
    );
    // And serves it the tenant's data: what it remembers, the first Hub recalls.
    let remember = json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "remember", "arguments": { "fact": "Replicas meet on Wednesdays" } }
    });
    let (status, body) = call(b, mcp("/mcp", token, Some(&id), Some(remember))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(answer(&body).get("result").is_some(), "{body}");
    let (status, text) = call(
        a,
        request("GET", "/api/memory/recall?topic=Wednesdays", token, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(text.contains("Replicas meet on Wednesdays"), "{text}");

    // The first Hub closes the idle tenant, which ends its MCP services, and
    // carries on with the session at its next request.
    assert_eq!(
        a_data.evict_idle(Duration::ZERO),
        std::slice::from_ref(tenant)
    );
    let (status, body) = list_tools(a, "/mcp", token, &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_continues_only_for_its_tenant_in_its_workspace_and_statistics_session() {
    let Some(url) = url() else { return };
    let r = replicas(&url, 2, 2).await;
    let [(a, _), (b, _)] = &r.hubs[..] else {
        unreachable!()
    };
    let ((_, alpha), (_, bravo)) = (&r.tenants[0], &r.tenants[1]);
    let id = mcp_session(a, alpha).await;

    let (status, body) = list_tools(b, "/mcp", bravo, &id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another tenant: {body}");
    let (status, body) = list_tools(b, "/mcp?namespace=elsewhere", alpha, &id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another workspace: {body}");
    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
    let mut req = mcp("/mcp", alpha, Some(&id), Some(list));
    req.headers_mut()
        .insert("x-ctxone-session", "another-session".parse().unwrap());
    let (status, body) = call(b, req).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another statistics session: {body}"
    );

    let (status, body) = list_tools(b, "/mcp", alpha, &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_its_client_closed_continues_nowhere() {
    let Some(url) = url() else { return };
    let r = replicas(&url, 2, 1).await;
    let [(a, _), (b, _)] = &r.hubs[..] else {
        unreachable!()
    };
    let (tenant, token) = &r.tenants[0];
    let id = mcp_session(a, token).await;

    let (status, body) = call(a, mcp("/mcp", token, Some(&id), None)).await;
    assert!(status.is_success(), "close: {status}: {body}");
    assert!(!kept(&connect(&url).await, tenant, &id).await);
    let (status, body) = list_tools(b, "/mcp", token, &id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_unused_for_a_week_continues_nowhere() {
    let Some(url) = url() else { return };
    let r = replicas(&url, 2, 1).await;
    let [(a, _), (b, _)] = &r.hubs[..] else {
        unreachable!()
    };
    let (tenant, token) = &r.tenants[0];
    let (old, recent) = (mcp_session(a, token).await, mcp_session(a, token).await);
    let db = connect(&url).await;
    db.execute(
        "UPDATE ctxone_mcp_sessions SET seen_at = now() - make_interval(days => $3)
         WHERE tenant_id = $1 AND session_id = $2",
        &[&tenant.as_str(), &old, &ctxone_hub::mcp_sessions::TTL_DAYS],
    )
    .await
    .unwrap();

    let (status, body) = list_tools(b, "/mcp", token, &old).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    // The next new session forgets it.
    assert!(kept(&db, tenant, &old).await);
    mcp_session(b, token).await;
    assert!(!kept(&db, tenant, &old).await);
    let (status, body) = list_tools(b, "/mcp", token, &recent).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

// -- Session statistics --

/// Tenant `tenant`'s hub tables on a pool of their own, as another Hub
/// process has them.
fn store(url: &str, tenant: &Tenant) -> PgStore {
    let storage =
        PostgresStorage::connect_tenant_blocking(url, tenant.as_str(), 2).expect("connect");
    PgStore::new(&storage).expect("hub tables")
}

/// `session`'s tokens sent, as `registry` has it.
fn sent(registry: &SessionRegistry, session: &str) -> Option<u64> {
    registry
        .snapshot(session)
        .map(|snap| snap.session_tokens_used)
}

/// `session`'s row in `tenant`'s hub tables: tokens sent, LLM calls and the
/// models they were made with.
async fn row(
    db: &tokio_postgres::Client,
    tenant: &Tenant,
    session: &str,
) -> Option<(i64, i64, Value)> {
    db.query_opt(
        "SELECT tokens_sent, llm_call_count, by_model FROM ctxone_sessions
         WHERE tenant_id = $1 AND session_id = $2",
        &[&tenant.as_str(), &session],
    )
    .await
    .unwrap()
    .map(|r| {
        let by_model: Option<String> = r.get(2);
        (
            r.get(0),
            r.get(1),
            by_model.map_or(Value::Null, |s| serde_json::from_str(&s).unwrap()),
        )
    })
}

#[test]
fn hubs_sharing_a_tenant_add_up_its_statistics() {
    let Some(url) = url() else { return };
    let tenant = fresh("stats");
    let (pg_a, pg_b) = (store(&url, &tenant), store(&url, &tenant));
    let (a, b) = (
        SessionRegistry::load_from_pg(&pg_a),
        SessionRegistry::load_from_pg(&pg_b),
    );
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db = runtime.block_on(connect(&url));

    // One session served by both Hubs at once, and one only the second began.
    let shared = a.get_or_create("shared");
    shared.record(400, 0);
    shared.record_llm_usage(10, 20, 0, 0, Some("model-a".into()), None);
    let on_b = b.get_or_create("shared");
    on_b.record(40, 0);
    on_b.record_llm_usage(1, 2, 0, 0, Some("model-b".into()), None);
    b.get_or_create("only-b").record(80, 0);

    a.flush_to_pg(&pg_a);
    b.flush_to_pg(&pg_b);
    let (sent_total, calls, by_model) = runtime.block_on(row(&db, &tenant, "shared")).unwrap();
    assert_eq!((sent_total, calls), (110, 2), "the row lost a Hub's counts");
    assert_eq!(by_model["model-a"]["input_tokens"], 10, "{by_model}");
    assert_eq!(by_model["model-b"]["input_tokens"], 1, "{by_model}");
    // The second Hub caught up with the first at its flush, the first with
    // the second at its next.
    assert_eq!(sent(&b, "shared"), Some(110));
    a.flush_to_pg(&pg_a);
    assert_eq!(sent(&a, "shared"), Some(110));
    assert_eq!(sent(&a, "only-b"), Some(20), "a session another Hub began");

    // Flushing again adds nothing twice.
    for _ in 0..2 {
        a.flush_to_pg(&pg_a);
        b.flush_to_pg(&pg_b);
    }
    assert_eq!(
        runtime.block_on(row(&db, &tenant, "shared")).unwrap().0,
        110
    );
    assert_eq!(
        (sent(&a, "shared"), sent(&b, "shared")),
        (Some(110), Some(110))
    );

    // What one records after another flushed lands on top of that.
    a.get_or_create("shared").record(4, 0);
    b.get_or_create("shared").record(8, 0);
    b.flush_to_pg(&pg_b);
    a.flush_to_pg(&pg_a);
    assert_eq!(
        runtime.block_on(row(&db, &tenant, "shared")).unwrap().0,
        113
    );

    // Deleted through one Hub, the session stays deleted when the other,
    // which recorded more meanwhile, flushes.
    assert!(a.remove("shared", Some(&HubStore::Postgres(pg_a.clone()))));
    b.get_or_create("shared").record(400, 0);
    b.flush_to_pg(&pg_b);
    assert_eq!(runtime.block_on(row(&db, &tenant, "shared")), None);
    assert_eq!(sent(&b, "shared"), None);

    // A Hub that starts now finds what's left.
    let c = SessionRegistry::load_from_pg(&store(&url, &tenant));
    assert_eq!((sent(&c, "shared"), sent(&c, "only-b")), (None, Some(20)));
}

#[test]
fn hubs_flushing_at_once_lose_nothing() {
    let Some(url) = url() else { return };
    let tenant = fresh("race");
    const ROUNDS: u64 = 40;
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let (url, tenant) = (&url, &tenant);
            scope.spawn(move || {
                let pg = store(url, tenant);
                let registry = SessionRegistry::load_from_pg(&pg);
                for _ in 0..ROUNDS {
                    registry.get_or_create("busy").record(4, 0);
                    registry.flush_to_pg(&pg);
                }
            });
        }
    });
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db = runtime.block_on(connect(&url));
    let (sent_total, ..) = runtime.block_on(row(&db, &tenant, "busy")).unwrap();
    assert_eq!(sent_total, 2 * ROUNDS as i64);
}
