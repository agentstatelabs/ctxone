//! Integration tests for the MCP-over-HTTP surface (`/mcp`).
//!
//! A single `--http` hub serves MCP alongside REST + Lens. These tests build
//! the router with `mcp_http` enabled and drive the `/mcp` route in-process via
//! `tower::ServiceExt::oneshot`, covering the Streamable-HTTP `initialize`
//! handshake and confirming the route is absent when the flag is off.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use ctxone_hub::{http, memory_tools::SessionRegistry};

fn mcp_router() -> axum::Router {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    let config = http::HubConfig {
        mcp_http: true,
        agent_id: "test-agent".to_string(),
        ..Default::default()
    };
    http::router_with_config(repo, sessions, config)
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
