//! End-to-end rate limiting tests.
//!
//! These spin up a real TCP listener + `axum::serve` with
//! `into_make_service_with_connect_info::<SocketAddr>()` so the
//! `PeerIpKeyExtractor` has a real client address to key on. We then
//! hammer the Hub's `/api/health` endpoint with a small burst and
//! assert that the limiter kicks in at the right point.
//!
//! Unit tests in `server/src/rate_limit.rs` cover the config builder
//! math; these tests prove the layer is actually attached and that
//! clients get the expected 429 response.

use std::net::SocketAddr;
use std::sync::Arc;

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Request;
use ctxone_hub::{http, memory_tools::SessionRegistry};
use reqwest::StatusCode;
use tower::ServiceExt;

/// Build the Hub router in-process (no TCP) so tests can drive it via
/// `oneshot` and inject an arbitrary peer `ConnectInfo` — the only way
/// to exercise the limiter for a *remote* IP, since a real loopback
/// connection is now exempt (see `LoopbackExemptKeyExtractor`).
fn build_app(rate_limit_rpm: u32) -> Router {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    let config = http::HubConfig {
        rate_limit_rpm,
        ..Default::default()
    };
    http::router_with_config(repo, sessions, config)
}

/// One request through the router with a synthetic peer address in the
/// `ConnectInfo` extension (what `into_make_service_with_connect_info`
/// would set on a real connection). Returns (status, has-ratelimit-header).
async fn call_from(app: &Router, peer: &str) -> (StatusCode, bool) {
    let mut req = Request::builder()
        .uri("/api/health")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    let resp = app.clone().oneshot(req).await.expect("oneshot");
    let has_rl = resp
        .headers()
        .keys()
        .any(|k| k.as_str().starts_with("x-ratelimit-"))
        || resp.headers().get("retry-after").is_some();
    (resp.status(), has_rl)
}

/// Spin up a real Hub on an ephemeral port with the given rate limit
/// (requests/minute). Returns the base URL so tests can make HTTP
/// requests against it.
async fn start_hub(rate_limit_rpm: u32) -> String {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());

    let config = http::HubConfig {
        rate_limit_rpm,
        ..Default::default()
    };
    let app = http::router_with_config(repo, sessions, config);

    // :0 → OS picks a free port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("axum serve");
    });

    format!("http://{}", addr)
}

#[tokio::test]
async fn rate_limit_zero_disables_limiter() {
    // rpm=0 → GovernorLayer::build_layer returns None → no layer
    // attached → we can hammer the endpoint as fast as we want.
    let base = start_hub(0).await;
    let client = reqwest::Client::new();

    for _ in 0..30 {
        let resp = client
            .get(format!("{}/api/health", base))
            .send()
            .await
            .expect("request");
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn remote_ip_eventually_returns_429() {
    // rpm=60 → burst small. Hammering from a single REMOTE ip must
    // start returning 429s (the limiter still protects non-local peers).
    // We assert at least one OK and at least one 429 — exact counts
    // drift on the wall-clock token bucket.
    let app = build_app(60);
    let mut ok = 0;
    let mut limited = 0;
    for _ in 0..30 {
        let (status, _) = call_from(&app, "203.0.113.7:40000").await;
        match status {
            StatusCode::OK => ok += 1,
            StatusCode::TOO_MANY_REQUESTS => limited += 1,
            other => panic!("unexpected status: {}", other),
        }
    }
    assert!(ok > 0, "expected some OK, got {ok} OK / {limited} 429");
    assert!(
        limited > 0,
        "expected some 429 for a remote peer, got {ok} OK / {limited} 429"
    );
}

#[tokio::test]
async fn loopback_is_exempt_from_rate_limit() {
    // The whole point of the exemption: local traffic (agents, the ctx
    // CLI, /api/sessions/sync) is never throttled. Hammer well past the
    // burst from loopback → ZERO 429s.
    let app = build_app(60);
    for _ in 0..50 {
        let (status, _) = call_from(&app, "127.0.0.1:50000").await;
        assert_eq!(
            status,
            StatusCode::OK,
            "loopback must never be rate-limited"
        );
    }
}

#[tokio::test]
async fn rate_limited_response_has_retry_after_header() {
    let app = build_app(60);
    let mut saw_429 = false;
    for _ in 0..30 {
        let (status, has_rl) = call_from(&app, "203.0.113.9:40000").await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert!(
                has_rl,
                "429 response missing rate-limit / retry-after headers"
            );
            saw_429 = true;
            break;
        }
    }
    assert!(
        saw_429,
        "never hit rate limit within 30 rapid remote requests"
    );
}
