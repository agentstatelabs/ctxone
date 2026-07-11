//! Integration tests for the Origin guard (CSRF / DNS-rebinding).
//!
//! A request with no `Origin` (CLI, native MCP) always passes. A request with
//! an `Origin` passes only when it's same-origin (Origin authority == `Host`)
//! or in the configured allow-list; otherwise it's 403. This blocks a browser
//! page from driving the loopback hub even though loopback peers are auth-exempt.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use ctxone_hub::{http, memory_tools::SessionRegistry};

fn router(allowed: &[&str]) -> axum::Router {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    let config = http::HubConfig {
        allowed_origins: allowed.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    http::router_with_config(repo, sessions, config)
}

fn health(host: &str, origin: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .uri("/api/health")
        .method("GET")
        .header("host", host);
    if let Some(o) = origin {
        b = b.header("origin", o);
    }
    b.body(Body::empty()).unwrap()
}

async fn status(router: axum::Router, req: Request<Body>) -> StatusCode {
    router.oneshot(req).await.expect("call").status()
}

#[tokio::test]
async fn no_origin_passes() {
    // Non-browser client: no Origin header.
    assert_eq!(
        status(router(&[]), health("localhost:3001", None)).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn same_origin_passes() {
    // Lens fetch: Origin authority equals Host.
    assert_eq!(
        status(
            router(&[]),
            health("localhost:3001", Some("http://localhost:3001"))
        )
        .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn cross_origin_rejected() {
    assert_eq!(
        status(
            router(&[]),
            health("localhost:3001", Some("http://evil.example"))
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn allowlisted_cross_origin_passes() {
    assert_eq!(
        status(
            router(&["http://app.example"]),
            health("localhost:3001", Some("http://app.example"))
        )
        .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn null_origin_rejected() {
    assert_eq!(
        status(router(&[]), health("localhost:3001", Some("null"))).await,
        StatusCode::FORBIDDEN
    );
}
