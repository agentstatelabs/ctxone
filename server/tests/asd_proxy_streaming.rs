//! Tests for the ASD proxy passthrough (`/api/code/{repo}/{*path}`),
//! specifically that it STREAMS upstream bodies instead of buffering them.
//!
//! Buffering is invisible for ordinary JSON responses but fatal for SSE
//! (`/api/v1/events` never ends — a buffering proxy hangs forever). The
//! streaming test below drives a fake `asd-serve` whose response body is
//! fed chunk-by-chunk through a channel that is never closed: each chunk
//! must surface through the proxy while the upstream stream is still open,
//! which is impossible if the proxy waits for the full body.
//!
//! The fake upstream binds a real TCP port because `proxy_asd` issues a
//! real reqwest call; the Hub router itself is driven in-process via
//! `oneshot` like the other integration tests.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use http_body_util::BodyExt;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tower::ServiceExt;

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use ctxone_hub::{
    http::{self, HubConfig},
    memory_tools::SessionRegistry,
};

type ChunkSender = mpsc::UnboundedSender<Result<Bytes, Infallible>>;

/// Spawn a fake asd-serve on an ephemeral port. Returns its base URL and a
/// slot that receives the SSE chunk sender once `/api/v1/events` is hit —
/// the test uses it to push more chunks while the stream stays open.
async fn spawn_fake_asd() -> (String, Arc<Mutex<Option<ChunkSender>>>) {
    let slot: Arc<Mutex<Option<ChunkSender>>> = Arc::new(Mutex::new(None));
    let slot_for_handler = slot.clone();

    let app = axum::Router::new()
        .route(
            "/api/v1/hello",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"ok":true}"#,
                )
            }),
        )
        .route(
            "/api/v1/events",
            get(move || {
                let slot = slot_for_handler.clone();
                async move {
                    let (tx, rx) = mpsc::unbounded_channel::<Result<Bytes, Infallible>>();
                    tx.send(Ok(Bytes::from_static(b"data: first\n\n")))
                        .expect("first chunk");
                    *slot.lock().await = Some(tx); // keep the sender alive
                    Response::builder()
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .header(header::CACHE_CONTROL, "no-cache")
                        .body(Body::from_stream(UnboundedReceiverStream::new(rx)))
                        .unwrap()
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upstream");
    });
    (format!("http://{}", addr), slot)
}

/// Hub router with the fake upstream registered as static repo "test".
fn hub_router(asd_base: String) -> axum::Router {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    http::router_with_config(
        repo,
        sessions,
        HubConfig {
            asd_repos: vec![("test".to_string(), asd_base)],
            ..Default::default()
        },
    )
}

fn get_req(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

/// Read the next non-empty data chunk from a streaming response body,
/// failing loudly if it doesn't arrive promptly (i.e. the proxy buffered).
async fn next_chunk(body: &mut Body) -> String {
    let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .expect("proxy did not forward the chunk within 5s — response is being buffered")
        .expect("stream ended unexpectedly")
        .expect("frame error");
    let data = frame.into_data().expect("expected a data frame");
    String::from_utf8_lossy(&data).into_owned()
}

#[tokio::test]
async fn proxy_still_serves_buffered_json() {
    let (base, _slot) = spawn_fake_asd().await;
    let hub = hub_router(base);

    let resp = hub.oneshot(get_req("/api/code/test/hello")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
    assert_eq!(v["ok"], true);
}

#[tokio::test]
async fn proxy_streams_sse_chunks_before_upstream_closes() {
    let (base, slot) = spawn_fake_asd().await;
    let hub = hub_router(base);

    let resp = hub.oneshot(get_req("/api/code/test/events")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    assert_eq!(
        resp.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-cache"
    );

    let mut body = resp.into_body();

    // First chunk was sent before the upstream response body ended (it
    // never ends) — a buffering proxy would hang here until the timeout.
    let first = next_chunk(&mut body).await;
    assert!(first.contains("data: first"), "chunk={:?}", first);

    // Push a second chunk through the still-open upstream stream and make
    // sure it flows through incrementally too.
    slot.lock()
        .await
        .as_ref()
        .expect("upstream handler ran")
        .send(Ok(Bytes::from_static(b"data: second\n\n")))
        .expect("send second chunk");
    let second = next_chunk(&mut body).await;
    assert!(second.contains("data: second"), "chunk={:?}", second);
}

#[tokio::test]
async fn proxy_unknown_repo_is_404() {
    let (base, _slot) = spawn_fake_asd().await;
    let hub = hub_router(base);
    let resp = hub.oneshot(get_req("/api/code/nope/health")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
