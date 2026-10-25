//! Tenant identity on a multi-tenant Hub (ctx-multi-tenant-hub t-002).
//!
//! With `HubConfig::tenants` set, every request needs a bearer token that
//! resolves to a tenant, loopback included, and the tenant is attached to the
//! request: `GET /api/tenant` reports it and rate limits are keyed by it. The
//! tenants file is re-read while the Hub runs. These oneshot tests inject
//! `ConnectInfo` to set the peer. That logs carry the tenant is tested in
//! `tenant_logs.rs`, in a process of its own.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use ctxone_hub::tenant::{
    Resolution, ResolveError, TenantResolver, TenantsFile, TenantsFileError, TokenHash,
};
use ctxone_hub::{http, memory_tools::SessionRegistry};

const LOOPBACK: &str = "127.0.0.1:5555";
const REMOTE: &str = "203.0.113.7:40000";

fn line(tenant: &str, token: &str) -> String {
    format!("{tenant} {}\n", TokenHash::of(token).to_hex())
}

/// A tenants file holding `contents`, re-read on every request.
fn tenants_file(contents: &str) -> (tempfile::TempDir, TenantsFile) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    std::fs::write(&path, contents).unwrap();
    let file = TenantsFile::open_with_reload(&path, Duration::ZERO).unwrap();
    (dir, file)
}

fn router(tenants: Option<Arc<dyn TenantResolver>>, rate_limit_rpm: u32) -> axum::Router {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    http::router_with_config(
        repo,
        sessions,
        http::HubConfig {
            tenants,
            rate_limit_rpm,
            ..Default::default()
        },
    )
}

fn multi_tenant(file: &TenantsFile) -> axum::Router {
    router(Some(Arc::new(file.clone())), 0)
}

/// GET `uri` from `peer`, with an optional bearer token.
fn get(uri: &str, peer: &str, bearer: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(uri);
    if let Some(token) = bearer {
        b = b.header("authorization", format!("Bearer {token}"));
    }
    let mut req = b.body(Body::empty()).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    req
}

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = app.clone().oneshot(req).await.expect("router call");
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// The tenant `GET /api/tenant` reports for `token`, or the status if refused.
async fn tenant_of(
    app: &axum::Router,
    token: Option<&str>,
) -> Result<serde_json::Value, StatusCode> {
    match call(app, get("/api/tenant", REMOTE, token)).await {
        (StatusCode::OK, body) => {
            Ok(serde_json::from_str::<serde_json::Value>(&body).unwrap()["tenant"].clone())
        }
        (status, _) => Err(status),
    }
}

#[tokio::test]
async fn each_token_reaches_its_own_tenant() {
    let contents =
        line("alice", "alice-laptop") + &line("alice", "alice-phone") + &line("bob", "bob-1");
    let (_dir, file) = tenants_file(&contents);
    let app = multi_tenant(&file);
    assert_eq!(
        tenant_of(&app, Some("alice-laptop")).await,
        Ok("alice".into())
    );
    assert_eq!(
        tenant_of(&app, Some("alice-phone")).await,
        Ok("alice".into())
    );
    assert_eq!(tenant_of(&app, Some("bob-1")).await, Ok("bob".into()));
    assert_eq!(file.counts(), (2, 3));
}

#[tokio::test]
async fn a_request_without_a_known_token_is_refused_even_from_loopback() {
    let (_dir, file) = tenants_file(&line("alice", "alice-laptop"));
    let app = multi_tenant(&file);
    for (peer, token) in [
        (LOOPBACK, None),
        (REMOTE, None),
        (LOOPBACK, Some("not-a-token")),
        (REMOTE, Some("")),
        // The hash itself isn't a token.
        (
            REMOTE,
            Some(TokenHash::of("alice-laptop").to_hex().as_str()),
        ),
    ] {
        let (status, _) = call(&app, get("/api/health", peer, token)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{peer} with {token:?}");
    }
    let (status, _) = call(&app, get("/api/health", LOOPBACK, Some("alice-laptop"))).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn tenants_and_tokens_change_without_a_restart() {
    let (dir, file) = tenants_file(&line("alice", "alice-1"));
    let app = multi_tenant(&file);
    let path = dir.path().join("tenants");
    assert_eq!(
        tenant_of(&app, Some("carol-1")).await,
        Err(StatusCode::UNAUTHORIZED)
    );

    // Add a tenant and rotate alice's token: both apply on the next request.
    std::fs::write(&path, line("alice", "alice-2") + &line("carol", "carol-1")).unwrap();
    assert_eq!(tenant_of(&app, Some("carol-1")).await, Ok("carol".into()));
    assert_eq!(tenant_of(&app, Some("alice-2")).await, Ok("alice".into()));
    assert_eq!(
        tenant_of(&app, Some("alice-1")).await,
        Err(StatusCode::UNAUTHORIZED)
    );

    // A bad line locks out only itself, and never keeps a removed token alive.
    std::fs::write(
        &path,
        "Carol not-a-hash\n".to_string() + &line("alice", "alice-2"),
    )
    .unwrap();
    assert_eq!(
        tenant_of(&app, Some("carol-1")).await,
        Err(StatusCode::UNAUTHORIZED)
    );
    assert_eq!(tenant_of(&app, Some("alice-2")).await, Ok("alice".into()));

    // A file that can't be read keeps the tenants last read.
    std::fs::remove_file(&path).unwrap();
    assert_eq!(tenant_of(&app, Some("alice-2")).await, Ok("alice".into()));

    // An empty file refuses every token.
    std::fs::write(&path, "").unwrap();
    assert_eq!(
        tenant_of(&app, Some("alice-2")).await,
        Err(StatusCode::UNAUTHORIZED)
    );
}

#[tokio::test]
async fn a_token_listed_for_two_tenants_reaches_neither() {
    let contents = line("alice", "shared") + &line("bob", "shared") + &line("bob", "bob-1");
    let (dir, file) = tenants_file(&line("bob", "bob-1"));
    let app = multi_tenant(&file);
    std::fs::write(dir.path().join("tenants"), contents).unwrap();
    assert_eq!(
        tenant_of(&app, Some("shared")).await,
        Err(StatusCode::UNAUTHORIZED)
    );
    assert_eq!(tenant_of(&app, Some("bob-1")).await, Ok("bob".into()));
}

#[test]
fn opening_refuses_a_file_with_bad_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    std::fs::write(
        &path,
        line("alice", "a") + "bob\n" + &line("alice", "shared") + &line("carol", "shared"),
    )
    .unwrap();
    let err = TenantsFile::open(&path).unwrap_err();
    let TenantsFileError::Invalid { problems, .. } = &err else {
        panic!("expected Invalid, got {err}");
    };
    let lines: Vec<usize> = problems.iter().map(|p| p.line).collect();
    assert_eq!(lines, [2, 3, 4], "{err}");

    let missing = TenantsFile::open(dir.path().join("missing")).unwrap_err();
    assert!(
        matches!(missing, TenantsFileError::Read { .. }),
        "{missing}"
    );
}

#[derive(Debug)]
struct Unreachable;

impl TenantResolver for Unreachable {
    fn resolve<'a>(&'a self, _token: &'a str) -> Resolution<'a> {
        Box::pin(async { Err(ResolveError("account service unreachable".into())) })
    }
}

#[tokio::test]
async fn a_resolver_that_cant_answer_is_a_503_not_a_401() {
    let app = router(Some(Arc::new(Unreachable)), 0);
    let (status, _) = call(&app, get("/api/health", REMOTE, Some("any"))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    // Without a token there's nothing to look up.
    let (status, _) = call(&app, get("/api/health", REMOTE, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_single_tenant_hub_reports_no_tenant() {
    let app = router(None, 0);
    assert_eq!(tenant_of(&app, None).await, Ok(serde_json::Value::Null));
}

#[tokio::test]
async fn rate_limits_are_per_tenant_and_spare_no_one_on_loopback() {
    let contents = line("alice", "alice-1") + &line("alice", "alice-2") + &line("bob", "bob-1");
    let (_dir, file) = tenants_file(&contents);
    // 60 a minute: a burst of 6, then one a second.
    let app = router(Some(Arc::new(file)), 60);
    let mut alice = (0, 0);
    for _ in 0..30 {
        match call(&app, get("/api/health", LOOPBACK, Some("alice-1")))
            .await
            .0
        {
            StatusCode::OK => alice.0 += 1,
            StatusCode::TOO_MANY_REQUESTS => alice.1 += 1,
            other => panic!("unexpected {other}"),
        }
    }
    assert!(
        alice.0 > 0 && alice.1 > 0,
        "alice: {} OK, {} limited",
        alice.0,
        alice.1
    );
    // Alice's other token, from another address, shares her bucket.
    let (status, _) = call(&app, get("/api/health", REMOTE, Some("alice-2"))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // Bob, from the same address, has his own.
    let (status, _) = call(&app, get("/api/health", LOOPBACK, Some("bob-1"))).await;
    assert_eq!(status, StatusCode::OK);
}

/// The Hub binary checks a tenants file but won't serve with one until each
/// tenant gets its own data.
#[test]
fn the_hub_refuses_to_serve_tenants_until_per_tenant_data_exists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    let hub = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_ctxone-hub"))
            .args(args)
            .env("HOME", dir.path())
            .env_remove("CTXONE_TENANTS_FILE")
            .env_remove("CTXONE_AUTH_TOKEN")
            .env_remove("CTXONE_AUTH_TOKEN_FILE")
            .output()
            .expect("run ctxone-hub")
    };
    let p = path.to_str().unwrap();

    std::fs::write(&path, line("alice", "a") + &line("bob", "b")).unwrap();
    let out = hub(&["--http", "--tenants-file", p]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(64), "{stderr}");
    assert!(
        stderr.contains("is valid (2 tenants, 2 tokens)"),
        "{stderr}"
    );
    assert!(stderr.contains("Not starting"), "{stderr}");

    std::fs::write(&path, "Alice abc\n").unwrap();
    let out = hub(&["--http", "--tenants-file", p]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(66), "{stderr}");
    assert!(stderr.contains("line 1"), "{stderr}");

    let out = hub(&["--http", "--tenants-file", p, "--auth-token", "x"]);
    assert_eq!(out.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&out.stderr).contains("replaces --auth-token"));
}
