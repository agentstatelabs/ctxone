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
use ctxone_hub::{http, memory_tools::SessionRegistry};
use reqwest::StatusCode;

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
async fn rate_limit_eventually_returns_429() {
    // rpm=60 → 1 req/sec sustained, burst=6. So 6 back-to-back
    // requests should all succeed, and the 7th through ~20th should
    // start returning 429s because we haven't waited for the bucket
    // to refill.
    //
    // We can't assert "exactly N succeed then exactly M fail" because
    // the governor uses a wall-clock-based token bucket and time
    // drifts microseconds between requests. Instead we hammer the
    // endpoint and assert:
    //   1. At least one request was allowed (otherwise the limiter
    //      is broken and blocking everything).
    //   2. At least one request was rejected with 429 (otherwise the
    //      limiter isn't actually enforcing anything).
    let base = start_hub(60).await;
    let client = reqwest::Client::new();

    let mut ok_count = 0;
    let mut rate_limited_count = 0;

    for _ in 0..30 {
        let resp = client
            .get(format!("{}/api/health", base))
            .send()
            .await
            .expect("request");
        match resp.status() {
            StatusCode::OK => ok_count += 1,
            StatusCode::TOO_MANY_REQUESTS => rate_limited_count += 1,
            other => panic!("unexpected status: {}", other),
        }
    }

    assert!(
        ok_count > 0,
        "expected at least one successful request, got {} OK / {} 429",
        ok_count,
        rate_limited_count
    );
    assert!(
        rate_limited_count > 0,
        "expected at least one 429, got {} OK / {} 429 (limiter isn't firing)",
        ok_count,
        rate_limited_count
    );
}

#[tokio::test]
async fn rate_limited_response_has_retry_after_header() {
    let base = start_hub(60).await;
    let client = reqwest::Client::new();

    // Drain the burst then capture the first 429 response and inspect it.
    let mut saw_429 = false;
    for _ in 0..30 {
        let resp = client
            .get(format!("{}/api/health", base))
            .send()
            .await
            .expect("request");

        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            // tower_governor's default response sets x-ratelimit-*
            // headers and uses plain text body. Any of these headers
            // signals "this is a real rate-limit response, not some
            // other 429."
            let headers = resp.headers();
            let has_ratelimit_header = headers
                .keys()
                .any(|k| k.as_str().starts_with("x-ratelimit-"));
            assert!(
                has_ratelimit_header || headers.get("retry-after").is_some(),
                "429 response missing rate-limit headers: {:?}",
                headers
            );
            saw_429 = true;
            break;
        }
    }
    assert!(saw_429, "never hit rate limit within 30 rapid requests");
}
