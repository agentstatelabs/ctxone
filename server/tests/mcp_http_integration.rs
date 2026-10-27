//! Integration tests for the MCP-over-HTTP surface (`/mcp`).
//!
//! A single `--http` hub serves MCP alongside REST + Lens. These tests build
//! the router with `mcp_http` enabled and drive the `/mcp` route in-process via
//! `tower::ServiceExt::oneshot`, covering the Streamable-HTTP `initialize`
//! handshake, confirming the route is absent when the flag is off, and checking
//! that tools report and seal under the workspace the URL names.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use ctxone_hub::{http, memory_tools::SessionRegistry};

fn mcp_router() -> axum::Router {
    mcp_router_and_repo().0
}

/// The router plus its root repo, for tests that inspect what MCP wrote.
fn mcp_router_and_repo() -> (axum::Router, Arc<Repository>) {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    let config = http::HubConfig {
        mcp_http: true,
        agent_id: "test-agent".to_string(),
        ..Default::default()
    };
    (
        http::router_with_config(repo.clone(), sessions, config),
        repo,
    )
}

fn initialize_request(uri: &str) -> Request<Body> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "itest", "version": "0" }
        }
    });
    Request::builder()
        .uri(uri)
        .method("POST")
        // rmcp's Streamable-HTTP transport validates Host against a loopback
        // allow-list by default (DNS-rebinding guard); real clients send it.
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn mcp_initialize_handshake_succeeds() {
    let resp = mcp_router()
        .oneshot(initialize_request("/mcp"))
        .await
        .expect("router call");
    assert_eq!(resp.status(), StatusCode::OK);
    // Streamable HTTP hands back a session id and an SSE stream.
    assert!(
        resp.headers().contains_key("mcp-session-id"),
        "expected mcp-session-id header on initialize response"
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body);
    // The initialize result is delivered as an SSE `data:` frame.
    assert!(
        text.contains("\"result\"") && text.contains("protocolVersion"),
        "unexpected initialize body: {text}"
    );
}

#[tokio::test]
async fn mcp_route_scopes_by_namespace_query() {
    // A namespaced initialize is accepted (namespace is created on demand).
    let resp = mcp_router()
        .oneshot(initialize_request("/mcp?namespace=itest-ns"))
        .await
        .expect("router call");
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().contains_key("mcp-session-id"));
}

#[tokio::test]
async fn mcp_route_absent_when_disabled() {
    // Default HubConfig leaves mcp_http off → the plain REST router has no /mcp.
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    let router = http::router(repo, sessions);
    let resp = router
        .oneshot(initialize_request("/mcp"))
        .await
        .expect("router call");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

fn mcp_request(uri: &str, session: Option<&str>, body: &Value) -> Request<Body> {
    let mut req = Request::builder()
        .uri(uri)
        .method("POST")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(id) = session {
        req = req.header("mcp-session-id", id);
    }
    req.body(Body::from(body.to_string())).unwrap()
}

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = app.clone().oneshot(req).await.expect("router call");
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Start an MCP session on `uri` and return its id.
async fn mcp_session(app: &axum::Router, uri: &str) -> String {
    let resp = app
        .clone()
        .oneshot(initialize_request(uri))
        .await
        .expect("router call");
    assert_eq!(resp.status(), StatusCode::OK);
    let id = resp.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    resp.into_body().collect().await.unwrap();
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let (status, body) = call(app, mcp_request(uri, Some(&id), &initialized)).await;
    assert!(status.is_success(), "{status}: {body}");
    id
}

/// Call MCP tool `name` and return its text, parsed as JSON.
async fn tool(app: &axum::Router, uri: &str, session: &str, name: &str, arguments: Value) -> Value {
    let call_tool = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    let (status, body) = call(app, mcp_request(uri, Some(session), &call_tool)).await;
    assert_eq!(status, StatusCode::OK, "{name}: {body}");
    // The answer is an SSE `data:` frame holding the JSON-RPC response.
    let text = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find_map(|m| {
            m["result"]["content"][0]["text"]
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("{name}: no tool text in {body}"));
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("{name}: not JSON: {text}"))
}

/// Close plan `name` in `ns` over MCP: a plan needs a task before it can be
/// completed, and `plan_complete` abandons it.
async fn close_plan_over_mcp(app: &axum::Router, ns: &str, name: &str) {
    let uri = format!("/mcp?namespace={ns}");
    let session = mcp_session(app, &uri).await;
    for (tool_name, arguments) in [
        ("plan_new", json!({ "name": name })),
        ("plan_add", json!({ "plan_id": name, "title": "ship it" })),
        (
            "plan_complete",
            json!({ "plan_id": name, "summary": format!("{name} shipped in {ns}") }),
        ),
    ] {
        let answer = tool(app, &uri, &session, tool_name, arguments).await;
        assert!(
            answer.get("error").is_none(),
            "{tool_name} in {ns}: {answer}"
        );
    }
}

/// The ids and workspaces of the store's sealed epochs, once there are
/// `count` of them. Plan closes seal in the background, so this waits.
async fn sealed_epochs(repo: &Repository, count: usize) -> Vec<(String, Option<String>)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut sealed: Vec<_> = repo
            .list_all_epochs()
            .expect("list epochs")
            .into_iter()
            .filter(|e| e.sealed_at.is_some())
            .map(|e| (e.id, e.namespace))
            .collect();
        if sealed.len() >= count || Instant::now() > deadline {
            sealed.sort();
            return sealed;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn mcp_tools_report_the_workspace_the_url_names() {
    let app = mcp_router();
    let uri = "/mcp?namespace=web";
    let session = mcp_session(&app, uri).await;

    let status = tool(&app, uri, &session, "project_status", json!({})).await;
    assert_eq!(status["namespace"], "web", "{status}");

    let remembered = tool(
        &app,
        uri,
        &session,
        "remember",
        json!({ "fact": "The web app deploys on Fridays" }),
    )
    .await;
    assert_eq!(remembered["namespace"], "web", "{remembered}");

    // A bare /mcp still serves, and reports, the default workspace.
    let session = mcp_session(&app, "/mcp").await;
    let status = tool(&app, "/mcp", &session, "project_status", json!({})).await;
    assert_eq!(status["namespace"], "default", "{status}");
}

#[tokio::test]
async fn mcp_plan_closes_seal_under_their_own_workspace() {
    let (app, repo) = mcp_router_and_repo();
    close_plan_over_mcp(&app, "web", "release").await;
    assert_eq!(
        sealed_epochs(&repo, 1).await,
        [("plan:web:release".to_string(), Some("web".to_string()))]
    );
}

#[tokio::test]
async fn one_plan_name_closed_in_two_workspaces_seals_two_epochs() {
    let (app, repo) = mcp_router_and_repo();
    close_plan_over_mcp(&app, "web", "release").await;
    close_plan_over_mcp(&app, "api", "release").await;
    assert_eq!(
        sealed_epochs(&repo, 2).await,
        [
            ("plan:api:release".to_string(), Some("api".to_string())),
            ("plan:web:release".to_string(), Some("web".to_string())),
        ]
    );
}
