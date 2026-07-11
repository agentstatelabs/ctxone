//! Integration tests for the whole-surface bearer auth middleware.
//!
//! Posture: loopback peers are always exempt; when a token is configured,
//! non-loopback (and unknown-peer) requests must carry `Authorization: Bearer
//! <token>`. With no token configured, nothing is enforced. Peer address is
//! normally attached by `into_make_service_with_connect_info`; these oneshot
//! tests inject `ConnectInfo` into request extensions to drive both paths.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use ctxone_hub::{http, memory_tools::SessionRegistry};

fn router(auth_token: Option<&str>) -> axum::Router {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    let config = http::HubConfig {
        auth_token: auth_token.map(str::to_string),
        ..Default::default()
    };
    http::router_with_config(repo, sessions, config)
}

/// GET /api/health with an optional peer address and optional bearer token.
fn health_req(peer: Option<&str>, bearer: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri("/api/health").method("GET");
    if let Some(tok) = bearer {
        b = b.header("authorization", format!("Bearer {tok}"));
    }
    let mut req = b.body(Body::empty()).unwrap();
    if let Some(addr) = peer {
        let sa: SocketAddr = addr.parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(sa));
    }
    req
}

async fn status(router: axum::Router, req: Request<Body>) -> StatusCode {
    router.oneshot(req).await.expect("router call").status()
}

#[tokio::test]
async fn no_token_allows_everything() {
    // No token configured → no enforcement, even from a remote peer.
    let s = status(router(None), health_req(Some("10.0.0.5:5555"), None)).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn loopback_is_exempt_even_with_token_set() {
    let s = status(
        router(Some("s3cret")),
        health_req(Some("127.0.0.1:5555"), None),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "loopback peer must not need a token");
}

#[tokio::test]
async fn remote_without_bearer_is_rejected() {
    let s = status(
        router(Some("s3cret")),
        health_req(Some("10.0.0.5:5555"), None),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn remote_with_correct_bearer_is_allowed() {
    let s = status(
        router(Some("s3cret")),
        health_req(Some("10.0.0.5:5555"), Some("s3cret")),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn remote_with_wrong_bearer_is_rejected() {
    let s = status(
        router(Some("s3cret")),
        health_req(Some("10.0.0.5:5555"), Some("nope")),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unknown_peer_fails_closed_when_token_set() {
    // No ConnectInfo attached → treated as non-loopback → token required.
    let s = status(router(Some("s3cret")), health_req(None, None)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}
