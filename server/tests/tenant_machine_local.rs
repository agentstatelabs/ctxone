//! A multi-tenant Hub acts on nothing of its own machine (ctx-multi-tenant-hub
//! t-004): its tenants are elsewhere, so its disk, transcripts and programs
//! aren't theirs. A single-tenant Hub, whose user is the machine's, still
//! reaches them.
//!
//! The tests put a fake `asd` first on `PATH` and a help index naming it, so
//! a Hub that ran this machine's programs would get an answer. That's
//! process-wide, which is why these tests have a file of their own.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use ctxone_hub::http;
use ctxone_hub::memory_tools::SessionRegistry;
use ctxone_hub::tenant::{TenantsFile, TokenHash};
use ctxone_hub::tenant_cache::{MemoryTenants, TenantCache};

const ALICE: &str = "alice-token-machine";

/// What the fake `asd` prints, whatever it's asked.
const FROM_THIS_MACHINE: &str = "answered-by-this-machine";

/// Put the fake `asd` and its help index in place, once per process.
fn fake_machine() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let asd = bin.join("asd");
        let reply = json!({ "feature": "fake", "detail": FROM_THIS_MACHINE });
        std::fs::write(&asd, format!("#!/bin/sh\necho '{reply}'\n")).unwrap();
        std::fs::set_permissions(&asd, std::fs::Permissions::from_mode(0o755)).unwrap();
        let index = dir.path().join("help-index.json");
        std::fs::write(&index, json!({ "tools": { "asd": {} } }).to_string()).unwrap();
        let path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: inside OnceLock's initializer, before any test here reads
        // the environment; nothing else in this process sets it.
        unsafe {
            std::env::set_var("PATH", format!("{}:{path}", bin.display()));
            std::env::set_var("AGENTSTATE_HELP_INDEX", &index);
        }
        dir
    })
    .path()
}

fn multi_tenant_hub() -> (axum::Router, TenantCache, tempfile::TempDir) {
    fake_machine();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    std::fs::write(&path, format!("alice {}\n", TokenHash::of(ALICE).to_hex())).unwrap();
    let data = TenantCache::new(Arc::new(MemoryTenants::default()));
    let config = http::HubConfig {
        mcp_http: true,
        agent_id: "itest".into(),
        // Ignored on a multi-tenant Hub.
        asd_repos: vec![("app".into(), "http://127.0.0.1:9".into())],
        ctx_binary: Some("ctx".into()),
        ..Default::default()
    };
    let app = http::multi_tenant_router(
        config,
        http::MultiTenant {
            resolver: Arc::new(TenantsFile::open(&path).unwrap()),
            data: data.clone(),
        },
    );
    (app, data, dir)
}

fn single_tenant_hub() -> axum::Router {
    fake_machine();
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().unwrap();
    let config = http::HubConfig {
        mcp_http: true,
        agent_id: "itest".into(),
        ..Default::default()
    };
    http::router_with_config(repo, Arc::new(SessionRegistry::new()), config)
}

fn request(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost")
        .header("authorization", format!("Bearer {ALICE}"))
        .header("x-ctxone-session", "machine-session");
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    let body = body.map_or_else(Body::empty, |v| Body::from(v.to_string()));
    let mut req = b.body(body).unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:40000".parse::<SocketAddr>().unwrap(),
    ));
    req
}

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = app.clone().oneshot(req).await.expect("router call");
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

fn mcp(session: Option<&str>, body: Value) -> Request<Body> {
    let mut req = request("POST", "/mcp?namespace=default", Some(body));
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

/// Start an MCP session and return its id.
async fn mcp_session(app: &axum::Router) -> String {
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "itest", "version": "0" }
        }
    });
    let resp = app.clone().oneshot(mcp(None, init)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let id = resp.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    resp.into_body().collect().await.unwrap();
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let (status, body) = call(app, mcp(Some(&id), initialized)).await;
    assert!(status.is_success(), "{status}: {body}");
    id
}

/// Call MCP tool `name` and return the response body.
async fn tool(app: &axum::Router, session: &str, name: &str, arguments: Value) -> String {
    let call_tool = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    let (status, body) = call(app, mcp(Some(session), call_tool)).await;
    assert_eq!(status, StatusCode::OK, "{name}: {body}");
    body
}

/// The machine-local routes as requests: `{*rest}` filled in.
fn machine_local_uris() -> Vec<String> {
    http::MACHINE_LOCAL_ROUTES
        .iter()
        .map(|route| route.replace("{*rest}", "app/search"))
        .collect()
}

#[tokio::test]
async fn the_routes_that_act_on_the_machine_are_off() {
    let (app, data, _dir) = multi_tenant_hub();
    for uri in machine_local_uris() {
        for method in ["GET", "POST", "DELETE"] {
            let (status, body) = call(&app, request(method, &uri, Some(json!({})))).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {body}");
            assert!(
                body.contains("serves many tenants"),
                "{method} {uri}: {body}"
            );
        }
    }
    assert!(
        data.open_tenants().is_empty(),
        "answering them opened a tenant"
    );
}

#[tokio::test]
async fn a_single_tenant_hub_keeps_them() {
    let app = single_tenant_hub();
    let (status, body) = call(&app, request("GET", "/api/sessions/autosync", None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(&app, request("POST", "/api/admin/backup", Some(json!({})))).await;
    // No SQLite file here, but the route is there to say so.
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("sqlite"), "{body}");
}

#[tokio::test]
async fn project_detection_reads_no_directory_of_the_hubs() {
    let (app, _data, _dir) = multi_tenant_hub();
    let checkout = tempfile::tempdir().unwrap();
    std::fs::write(checkout.path().join(".ctxproject"), "on-this-machine\n").unwrap();
    let cwd = checkout.path().display().to_string();

    let (status, body) = call(
        &app,
        request("GET", &format!("/api/projects/detect?cwd={cwd}"), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("client_read=true"), "{body}");

    let uri = format!("/api/projects/detect?cwd={cwd}&client_read=true&ctxproject=app");
    let (status, body) = call(&app, request("GET", &uri, None)).await;
    // In-memory tenants have no project registry; the point is it answered.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.contains("on-this-machine"), "{body}");
}

#[tokio::test]
async fn help_asks_no_other_program() {
    let uri = "/api/help?topic=zzzzznope";
    let (status, body) = call(&single_tenant_hub(), request("GET", uri, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(FROM_THIS_MACHINE),
        "the fake asd should answer a single-tenant Hub: {body}"
    );

    let (app, data, _dir) = multi_tenant_hub();
    let (status, body) = call(&app, request("GET", uri, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains(FROM_THIS_MACHINE), "{body}");
    assert!(data.open_tenants().is_empty(), "help opened a tenant");

    let session = mcp_session(&app).await;
    let body = tool(&app, &session, "help", json!({ "topic": "zzzzznope" })).await;
    assert!(!body.contains(FROM_THIS_MACHINE), "{body}");
}

#[tokio::test]
async fn the_code_tools_run_nothing_on_the_machine() {
    let single = single_tenant_hub();
    let session = mcp_session(&single).await;
    let body = tool(
        &single,
        &session,
        "code_impact",
        json!({ "target": "get_orders" }),
    )
    .await;
    assert!(
        body.contains(FROM_THIS_MACHINE),
        "the fake asd should answer a single-tenant Hub: {body}"
    );

    let (app, _data, _dir) = multi_tenant_hub();
    let session = mcp_session(&app).await;
    for (name, arguments) in [
        ("code_impact", json!({ "target": "get_orders" })),
        ("code_cross_repo_edges", json!({})),
        ("code_search", json!({ "query": "orders" })),
    ] {
        let body = tool(&app, &session, name, arguments).await;
        assert!(!body.contains(FROM_THIS_MACHINE), "{name}: {body}");
        assert!(body.contains("serves many tenants"), "{name}: {body}");
    }
}
