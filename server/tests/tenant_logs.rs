//! A multi-tenant Hub's request log lines carry the tenant, never the token.
//!
//! In a test binary of its own: a test's log subscriber is per thread, and
//! tests running beside it on other threads can make tracing skip the events
//! it waits for.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use ctxone_hub::tenant::{TenantsFile, TokenHash};
use ctxone_hub::{http, memory_tools::SessionRegistry};

/// Log output captured by a test subscriber.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn status(app: &axum::Router, token: &str) -> StatusCode {
    let mut req = Request::builder()
        .uri("/api/health")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "203.0.113.7:40000".parse::<SocketAddr>().unwrap(),
    ));
    app.clone()
        .oneshot(req)
        .await
        .expect("router call")
        .status()
}

#[tokio::test]
async fn request_logs_carry_the_tenant_and_never_the_token() {
    let token = "tok-alice-5f2c9e";
    let wrong = "wrong-token-9d1";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    std::fs::write(&path, format!("alice {}\n", TokenHash::of(token).to_hex())).unwrap();
    let file = TenantsFile::open_with_reload(&path, Duration::ZERO).unwrap();

    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let app = http::router_with_config(
        repo,
        Arc::new(SessionRegistry::new()),
        http::HubConfig {
            tenants: Some(Arc::new(file)),
            ..Default::default()
        },
    );

    let logs = Captured::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .finish();
    let _default = tracing::subscriber::set_default(subscriber);

    assert_eq!(status(&app, token).await, StatusCode::OK);
    assert_eq!(status(&app, wrong).await, StatusCode::UNAUTHORIZED);

    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    let request_line = text
        .lines()
        .find(|l| l.contains("request") && l.contains("/api/health"))
        .unwrap_or_else(|| panic!("no request log line in:\n{text}"));
    assert!(
        request_line.contains("ctxone.tenant=\"alice\""),
        "{request_line}"
    );
    for secret in [token, wrong, &TokenHash::of(token).to_hex()] {
        assert!(!text.contains(secret), "logs contain {secret:?}:\n{text}");
    }
}
