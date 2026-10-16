//! Integration tests for the whole-surface bearer auth middleware.
//!
//! Posture: loopback peers are exempt unless `require_auth_on_loopback` is set;
//! when a token is configured (fixed, or a token file re-read while the hub
//! runs), every other request must carry `Authorization: Bearer <token>`. With
//! no token configured, nothing is enforced. Peer address is normally attached
//! by `into_make_service_with_connect_info`; these oneshot tests inject
//! `ConnectInfo` into request extensions to drive both paths.

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
    router_with(http::HubConfig {
        auth_token: auth_token.map(str::to_string),
        ..Default::default()
    })
}

fn router_with(config: http::HubConfig) -> axum::Router {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    http::router_with_config(repo, sessions, config)
}

/// A token file holding `contents`, re-read on every request.
fn token_file(contents: &str) -> (tempfile::TempDir, http::AuthTokenFile) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tokens");
    std::fs::write(&path, contents).unwrap();
    let file = http::AuthTokenFile::open_with_reload(&path, std::time::Duration::ZERO).unwrap();
    (dir, file)
}

fn router_with_file(file: &http::AuthTokenFile) -> axum::Router {
    router_with(http::HubConfig {
        auth_token_file: Some(file.clone()),
        ..Default::default()
    })
}

const REMOTE: Option<&str> = Some("10.0.0.5:5555");

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

#[tokio::test]
async fn loopback_must_send_the_token_when_not_exempt() {
    let r = router_with(http::HubConfig {
        auth_token: Some("s3cret".into()),
        require_auth_on_loopback: true,
        ..Default::default()
    });
    for peer in ["127.0.0.1:5555", "[::1]:5555"] {
        assert_eq!(
            status(r.clone(), health_req(Some(peer), None)).await,
            StatusCode::UNAUTHORIZED,
            "{peer} without a token"
        );
        assert_eq!(
            status(r.clone(), health_req(Some(peer), Some("wrong"))).await,
            StatusCode::UNAUTHORIZED,
            "{peer} with a wrong token"
        );
        assert_eq!(
            status(r.clone(), health_req(Some(peer), Some("s3cret"))).await,
            StatusCode::OK,
            "{peer} with the token"
        );
    }
}

#[tokio::test]
async fn token_file_accepts_every_listed_token() {
    let (_dir, file) = token_file("# current, then the one being retired\n\nfirst\n  second  \n");
    let r = router_with_file(&file);
    for tok in ["first", "second"] {
        assert_eq!(
            status(r.clone(), health_req(REMOTE, Some(tok))).await,
            StatusCode::OK,
            "{tok}"
        );
    }
    for tok in ["# current, then the one being retired", "third"] {
        assert_eq!(
            status(r.clone(), health_req(REMOTE, Some(tok))).await,
            StatusCode::UNAUTHORIZED,
            "{tok}"
        );
    }
    assert_eq!(
        status(r.clone(), health_req(REMOTE, None)).await,
        StatusCode::UNAUTHORIZED
    );
    // Loopback stays exempt unless asked otherwise.
    assert_eq!(
        status(r, health_req(Some("127.0.0.1:1"), None)).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn token_file_rotates_without_a_restart() {
    let (_dir, file) = token_file("old\n");
    let r = router_with_file(&file);
    assert_eq!(
        status(r.clone(), health_req(REMOTE, Some("old"))).await,
        StatusCode::OK
    );
    assert_eq!(
        status(r.clone(), health_req(REMOTE, Some("new"))).await,
        StatusCode::UNAUTHORIZED
    );

    // Add the new token beside the old one: both work while clients move over.
    std::fs::write(file.path(), "new\nold\n").unwrap();
    assert_eq!(
        status(r.clone(), health_req(REMOTE, Some("new"))).await,
        StatusCode::OK
    );
    assert_eq!(
        status(r.clone(), health_req(REMOTE, Some("old"))).await,
        StatusCode::OK
    );

    // Remove the old one: it stops working at once.
    std::fs::write(file.path(), "new\n").unwrap();
    assert_eq!(
        status(r.clone(), health_req(REMOTE, Some("new"))).await,
        StatusCode::OK
    );
    assert_eq!(
        status(r, health_req(REMOTE, Some("old"))).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn an_empty_token_file_accepts_no_token() {
    let (_dir, file) = token_file("# every token revoked\n");
    let r = router_with_file(&file);
    assert_eq!(
        status(r.clone(), health_req(REMOTE, Some("anything"))).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(r, health_req(REMOTE, None)).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn an_unreadable_token_file_keeps_the_last_tokens() {
    let (_dir, file) = token_file("kept\n");
    let r = router_with_file(&file);
    std::fs::remove_file(file.path()).unwrap();
    assert_eq!(
        status(r.clone(), health_req(REMOTE, Some("kept"))).await,
        StatusCode::OK
    );
    assert_eq!(
        status(r, health_req(REMOTE, Some("other"))).await,
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn a_missing_token_file_is_refused_at_open() {
    let dir = tempfile::tempdir().unwrap();
    assert!(http::AuthTokenFile::open(dir.path().join("absent")).is_err());
}

#[test]
fn the_hubs_own_ctx_gets_a_token_and_debug_hides_it() {
    let fixed = http::AuthTokens::new(Some("fixed-secret-123".into()), None);
    assert_eq!(fixed.for_children().as_deref(), Some("fixed-secret-123"));
    assert!(!format!("{fixed:?}").contains("fixed-secret-123"));

    let (_dir, file) = token_file("file-secret-456\nfile-secret-789\n");
    let from_file = http::AuthTokens::new(None, Some(file.clone()));
    assert_eq!(from_file.for_children().as_deref(), Some("file-secret-456"));
    assert!(!format!("{from_file:?}").contains("file-secret"));
    assert!(!format!("{file:?}").contains("file-secret"));

    assert_eq!(http::AuthTokens::new(None, None).for_children(), None);
    assert!(!http::AuthTokens::new(Some(String::new()), None).is_configured());
}

/// The Hub's background import runs `ctx` against its own loopback URL; with
/// loopback no longer exempt, that `ctx` must carry the token. A stand-in `ctx`
/// records the `CTX_TOKEN` it was given.
#[cfg(unix)]
#[tokio::test]
async fn session_sync_hands_ctx_the_token() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen");
    let fake_ctx = dir.path().join("ctx");
    std::fs::write(
        &fake_ctx,
        format!(
            "#!/bin/sh\nprintf '%s' \"$CTX_TOKEN\" > '{}'\n",
            seen.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake_ctx, std::fs::Permissions::from_mode(0o755)).unwrap();

    let _ = http::run_session_sync(
        fake_ctx.to_str().unwrap(),
        "http://127.0.0.1:1",
        Some("child-token"),
    )
    .await;
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), "child-token");
}
