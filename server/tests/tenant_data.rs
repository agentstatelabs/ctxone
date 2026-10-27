//! Each tenant's own data on a multi-tenant Hub (ctx-multi-tenant-hub t-003).
//!
//! A request reaches the repository, session statistics and MCP services of
//! the tenant its token belongs to, even when tenants use the same names. A
//! tenant opens on its first request and closes once idle. These tests keep
//! tenants in memory; `postgres_tenants.rs` covers Postgres.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use ctxone_hub::http;
use ctxone_hub::tenant::{Tenant, TenantsFile, TokenHash};
use ctxone_hub::tenant_cache::{MemoryTenants, OpenedTenant, TenantCache, TenantOpener};

const ALICE: &str = "alice-token-3b8e";
const BOB: &str = "bob-token-71c2";

struct TestHub {
    app: axum::Router,
    data: TenantCache,
    _dir: tempfile::TempDir,
}

/// A Hub for alice and bob, opening their data with `opener`.
fn hub(opener: Arc<dyn TenantOpener>, mcp_http: bool) -> TestHub {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    let lines = [("alice", ALICE), ("bob", BOB)]
        .map(|(t, token)| format!("{t} {}\n", TokenHash::of(token).to_hex()));
    std::fs::write(&path, lines.concat()).unwrap();
    let data = TenantCache::new(opener);
    let app = http::multi_tenant_router(
        http::HubConfig {
            mcp_http,
            agent_id: "itest".into(),
            ..Default::default()
        },
        http::MultiTenant {
            resolver: Arc::new(TenantsFile::open(&path).unwrap()),
            data: data.clone(),
        },
    );
    TestHub {
        app,
        data,
        _dir: dir,
    }
}

fn memory_hub() -> TestHub {
    hub(Arc::new(MemoryTenants::default()), false)
}

fn request(method: &str, uri: &str, token: &str, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "hub.example.com")
        .header("authorization", format!("Bearer {token}"))
        .header("x-ctxone-session", "shared-session-name");
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

async fn remember(app: &axum::Router, token: &str, fact: &str) {
    let body = json!({ "fact": fact, "context": "ops" });
    let (status, text) = call(
        app,
        request("POST", "/api/memory/remember", token, Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
}

async fn recall(app: &axum::Router, token: &str, topic: &str) -> String {
    let uri = format!("/api/memory/recall?topic={topic}&budget=4000");
    let (status, text) = call(app, request("GET", &uri, token, None)).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    text
}

fn tenant(name: &str) -> Tenant {
    Tenant::new(name).unwrap()
}

#[tokio::test]
async fn each_tenant_reaches_only_its_own_memory() {
    let hub = memory_hub();
    remember(&hub.app, ALICE, "Alice rotates the deploy key on Fridays").await;
    remember(&hub.app, BOB, "Bob freezes deploys on Fridays").await;

    let alice = recall(&hub.app, ALICE, "Fridays").await;
    assert!(alice.contains("Alice rotates"), "{alice}");
    assert!(!alice.contains("Bob freezes"), "{alice}");
    let bob = recall(&hub.app, BOB, "Fridays").await;
    assert!(bob.contains("Bob freezes"), "{bob}");
    assert!(!bob.contains("Alice rotates"), "{bob}");
}

#[tokio::test]
async fn session_statistics_are_per_tenant() {
    let hub = memory_hub();
    let mut req = request("GET", "/api/memory/recall?topic=x", ALICE, None);
    req.headers_mut()
        .insert("x-ctxone-session", "alice-only-session".parse().unwrap());
    assert_eq!(call(&hub.app, req).await.0, StatusCode::OK);

    let (_, alice) = call(&hub.app, request("GET", "/api/stats/sessions", ALICE, None)).await;
    assert!(alice.contains("alice-only-session"), "{alice}");
    let (_, bob) = call(&hub.app, request("GET", "/api/stats/sessions", BOB, None)).await;
    assert!(!bob.contains("alice-only-session"), "{bob}");
}

/// The workspace rollup is memoized for a few seconds; one tenant's must
/// never be served to another.
#[tokio::test]
async fn the_workspace_summary_is_each_tenants_own() {
    let hub = memory_hub();
    for i in 0..3 {
        remember(&hub.app, ALICE, &format!("Alice's fact number {i}")).await;
    }
    let summary = |token| request("GET", "/api/namespaces/summary", token, None);
    let (status, alice) = call(&hub.app, summary(ALICE)).await;
    assert_eq!(status, StatusCode::OK, "{alice}");
    let (status, bob) = call(&hub.app, summary(BOB)).await;
    assert_eq!(status, StatusCode::OK, "{bob}");
    assert_ne!(alice, bob, "bob was served alice's workspace summary");
}

#[tokio::test]
async fn a_tenant_opens_on_its_first_request_and_closes_when_idle() {
    let hub = memory_hub();
    // A request that needs no data opens nothing.
    let (status, _) = call(&hub.app, request("GET", "/api/health", ALICE, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(hub.data.open_tenants().is_empty());

    remember(&hub.app, ALICE, "Alice keeps this after closing").await;
    assert_eq!(hub.data.open_tenants(), [tenant("alice")]);
    call(&hub.app, request("GET", "/api/stats/tokens", BOB, None)).await;
    assert_eq!(hub.data.open_tenants(), [tenant("alice"), tenant("bob")]);

    assert!(hub.data.evict_idle(Duration::from_secs(3600)).is_empty());
    assert_eq!(
        hub.data.evict_idle(Duration::ZERO),
        [tenant("alice"), tenant("bob")]
    );
    assert!(hub.data.open_tenants().is_empty());

    // It opens again on its next request, with its data.
    let alice = recall(&hub.app, ALICE, "closing").await;
    assert!(alice.contains("Alice keeps this"), "{alice}");
    assert_eq!(hub.data.open_tenants(), [tenant("alice")]);
}

#[tokio::test]
async fn idle_tenants_close_in_the_background() {
    let hub = memory_hub();
    remember(&hub.app, ALICE, "Alice's fact before the upkeep").await;
    let held = hub.data.get(&tenant("bob")).await.unwrap();
    let upkeep = hub
        .data
        .spawn_upkeep(Duration::from_millis(20), Duration::ZERO);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while hub.data.open_tenants() != [tenant("bob")] {
        assert!(
            tokio::time::Instant::now() < deadline,
            "alice is still open: {:?}",
            hub.data.open_tenants()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Bob is in use, so he stays.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(hub.data.open_tenants(), [tenant("bob")]);
    drop(held);
    upkeep.abort();
}

#[tokio::test]
async fn a_tenant_in_use_stays_open() {
    let hub = memory_hub();
    let held = hub.data.get(&tenant("alice")).await.unwrap();
    assert!(hub.data.evict_idle(Duration::ZERO).is_empty());
    drop(held);
    assert_eq!(hub.data.evict_idle(Duration::ZERO), [tenant("alice")]);
}

/// Opens tenants in memory, after failing `failures` times and waiting
/// `delay` each time.
#[derive(Debug, Default)]
struct Flaky {
    failures: AtomicUsize,
    delay: Duration,
    opens: AtomicUsize,
    inner: MemoryTenants,
}

impl TenantOpener for Flaky {
    fn open(&self, tenant: &Tenant) -> Result<OpenedTenant, String> {
        std::thread::sleep(self.delay);
        let failing = self
            .failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if failing {
            return Err("the database is on fire".into());
        }
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.inner.open(tenant)
    }
}

#[tokio::test]
async fn a_tenant_that_cant_open_is_a_503_until_it_can() {
    let hub = hub(
        Arc::new(Flaky {
            failures: AtomicUsize::new(1),
            ..Default::default()
        }),
        false,
    );
    let tokens = || request("GET", "/api/stats/tokens", ALICE, None);
    let (status, body) = call(&hub.app, tokens()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !body.contains("on fire"),
        "the opener's error reached the client: {body}"
    );
    assert!(hub.data.open_tenants().is_empty());

    let (status, _) = call(&hub.app, tokens()).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tenant_opens_once_however_many_requests_arrive_together() {
    let opener = Arc::new(Flaky {
        delay: Duration::from_millis(50),
        ..Default::default()
    });
    let hub = hub(opener.clone(), false);
    let mut requests = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let app = hub.app.clone();
        requests.spawn(async move {
            call(&app, request("GET", "/api/stats/tokens", ALICE, None))
                .await
                .0
        });
    }
    while let Some(status) = requests.join_next().await {
        assert_eq!(status.unwrap(), StatusCode::OK);
    }
    assert_eq!(opener.opens.load(Ordering::SeqCst), 1);
}

// -- MCP over HTTP --

fn mcp(token: &str, session: Option<&str>, body: Value) -> Request<Body> {
    let mut req = request("POST", "/mcp", token, Some(body));
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

/// Start an MCP session as `token`'s tenant and return its id.
async fn mcp_session(app: &axum::Router, token: &str) -> String {
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "itest", "version": "0" }
        }
    });
    let resp = app.clone().oneshot(mcp(token, None, init)).await.unwrap();
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
    let (status, body) = call(app, mcp(token, Some(&id), initialized)).await;
    assert!(status.is_success(), "{status}: {body}");
    id
}

fn tool_call(name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
}

#[tokio::test]
async fn mcp_sessions_and_writes_stay_in_their_tenant() {
    let hub = hub(Arc::new(MemoryTenants::default()), true);
    let session = mcp_session(&hub.app, ALICE).await;

    let remember = tool_call(
        "remember",
        json!({ "fact": "Alice's MCP fact about tuning" }),
    );
    let (status, body) = call(&hub.app, mcp(ALICE, Some(&session), remember.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.contains("\"error\""), "{body}");

    // Bob can't use alice's session.
    let (status, _) = call(&hub.app, mcp(BOB, Some(&session), remember)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    assert!(
        recall(&hub.app, ALICE, "tuning")
            .await
            .contains("Alice's MCP fact")
    );
    assert!(
        !recall(&hub.app, BOB, "tuning")
            .await
            .contains("Alice's MCP fact")
    );
}

/// Recall results are cached per tenant, for REST and MCP alike: a recall in
/// one tenant fills no other tenant's cache, so a cached result can't reach
/// another tenant whatever its key.
#[tokio::test]
async fn recall_results_are_cached_per_tenant() {
    let hub = hub(Arc::new(MemoryTenants::default()), true);
    remember(&hub.app, ALICE, "Alice caches recall results").await;
    remember(&hub.app, BOB, "Bob caches nothing yet").await;
    recall(&hub.app, ALICE, "caches").await;
    let alice = hub.data.get(&tenant("alice")).await.unwrap();
    let bob = hub.data.get(&tenant("bob")).await.unwrap();
    assert_eq!(alice.recall_cache.len(), 1);
    assert!(bob.recall_cache.is_empty());

    let session = mcp_session(&hub.app, BOB).await;
    let call_recall = tool_call("recall", json!({ "topic": "caches" }));
    let (status, body) = call(&hub.app, mcp(BOB, Some(&session), call_recall)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Bob caches"), "{body}");
    assert_eq!(bob.recall_cache.len(), 1, "MCP recall missed bob's cache");
    assert_eq!(alice.recall_cache.len(), 1);
}
