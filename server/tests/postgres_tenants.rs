//! A multi-tenant Hub on Postgres (ctx-multi-tenant-hub t-003): tenants share
//! one pool and the `ctxone_*` tables, each opens with its own migrations under
//! a lock other Hubs respect, and its session statistics outlive a close.
//!
//! Skipped unless `TEST_DATABASE_URL` is set; `CTXONE_REQUIRE_POSTGRES` turns
//! the skip into a failure (CI sets both). Tenant names are random, so a shared
//! database is fine.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use agentstategraph_storage::PostgresStorage;
use agentstategraph_storage::tokio_postgres;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use ctxone_hub::http;
use ctxone_hub::migrations::{CTXONE_SCHEMA_VERSION, current_schema_version};
use ctxone_hub::tenant::{Tenant, TenantsFile, TokenHash};
use ctxone_hub::tenant_cache::{OPEN_LOCK_CLASS, PostgresTenants, TenantCache};

fn url() -> Option<String> {
    match std::env::var("TEST_DATABASE_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            let required =
                std::env::var("CTXONE_REQUIRE_POSTGRES").is_ok_and(|v| !v.is_empty() && v != "0");
            assert!(
                !required,
                "CTXONE_REQUIRE_POSTGRES is set, but TEST_DATABASE_URL isn't"
            );
            eprintln!("skip: TEST_DATABASE_URL not set");
            None
        }
    }
}

/// A tenant name no other test run uses.
fn fresh(tag: &str) -> Tenant {
    let id = uuid::Uuid::new_v4().simple().to_string();
    Tenant::new(&format!("{tag}-{}", &id[..12])).unwrap()
}

struct TestHub {
    app: axum::Router,
    data: TenantCache,
    /// The tenants and their tokens.
    tenants: Vec<(Tenant, String)>,
    _dir: tempfile::TempDir,
}

/// A Hub for `count` fresh tenants on one pool of `pool_size`.
async fn hub(url: &str, count: usize, pool_size: usize) -> TestHub {
    let root =
        PostgresStorage::connect_tenant_with_pool_size(url, fresh("root").as_str(), pool_size)
            .await
            .expect("connect");
    let data = TenantCache::new(Arc::new(PostgresTenants::new(root).expect("tenants")));
    let tenants: Vec<(Tenant, String)> = (0..count)
        .map(|i| {
            (
                fresh(&format!("t{i}")),
                format!("token-{}", uuid::Uuid::new_v4()),
            )
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    let lines: String = tenants
        .iter()
        .map(|(t, token)| format!("{t} {}\n", TokenHash::of(token).to_hex()))
        .collect();
    std::fs::write(&path, lines).unwrap();
    let app = http::multi_tenant_router(
        http::HubConfig::default(),
        http::MultiTenant {
            resolver: Arc::new(TenantsFile::open(&path).unwrap()),
            data: data.clone(),
        },
    );
    TestHub {
        app,
        data,
        tenants,
        _dir: dir,
    }
}

fn request(method: &str, uri: &str, token: &str, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-ctxone-session", "pg-session");
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    let body = body.map_or_else(Body::empty, |v| Body::from(v.to_string()));
    let mut req = b.body(body).unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "203.0.113.7:40000".parse::<SocketAddr>().unwrap(),
    ));
    req
}

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = app.clone().oneshot(req).await.expect("router call");
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// A connection of the test's own, standing in for another Hub.
async fn other_hub(url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(connection);
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenants_on_one_pool_see_none_of_each_others_data() {
    let Some(url) = url() else { return };
    let hub = hub(&url, 2, 4).await;
    let [(_, a), (_, b)] = [&hub.tenants[0], &hub.tenants[1]];

    // The same fact context and project id in both tenants.
    for (token, fact) in [(a, "A ships on Tuesdays"), (b, "B ships on Thursdays")] {
        let body = json!({ "fact": fact, "context": "release" });
        let (status, text) = call(
            &hub.app,
            request("POST", "/api/memory/remember", token, Some(body)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let project = json!({ "id": "shared-name", "remote_url": format!("https://git.example.com/{token}") });
        let (status, text) = call(
            &hub.app,
            request("POST", "/api/projects", token, Some(project)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
    }

    let recall = |token| {
        request(
            "GET",
            "/api/memory/recall?topic=ships&budget=4000",
            token,
            None,
        )
    };
    let (_, text) = call(&hub.app, recall(a)).await;
    assert!(
        text.contains("Tuesdays") && !text.contains("Thursdays"),
        "{text}"
    );
    let (_, text) = call(&hub.app, recall(b)).await;
    assert!(
        text.contains("Thursdays") && !text.contains("Tuesdays"),
        "{text}"
    );

    for (mine, theirs) in [(a, b), (b, a)] {
        let (status, text) = call(&hub.app, request("GET", "/api/projects", mine, None)).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let projects: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(projects.as_array().map(Vec::len), Some(1), "{text}");
        assert!(
            text.contains(mine.as_str()) && !text.contains(theirs.as_str()),
            "{text}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tenant_opens_migrated_and_its_statistics_outlive_closing() {
    let Some(url) = url() else { return };
    let hub = hub(&url, 1, 4).await;
    let (tenant, token) = &hub.tenants[0];

    let state = hub.data.get(tenant).await.unwrap();
    assert_eq!(current_schema_version(&state.repo), CTXONE_SCHEMA_VERSION);
    drop(state);

    let mut req = request("GET", "/api/memory/recall?topic=anything", token, None);
    req.headers_mut()
        .insert("x-ctxone-session", "kept-session".parse().unwrap());
    assert_eq!(call(&hub.app, req).await.0, StatusCode::OK);

    assert_eq!(
        hub.data.evict_idle(Duration::ZERO),
        std::slice::from_ref(tenant)
    );
    let (status, text) = call(&hub.app, request("GET", "/api/stats/sessions", token, None)).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        text.contains("kept-session"),
        "the session's statistics were lost when the tenant closed: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opening_waits_while_another_hub_opens_the_tenant() {
    let Some(url) = url() else { return };
    let hub = hub(&url, 1, 4).await;
    let (tenant, token) = hub.tenants[0].clone();

    let other = other_hub(&url).await;
    let lock = "SELECT pg_advisory_lock($1, hashtext($2))";
    other
        .execute(lock, &[&OPEN_LOCK_CLASS, &tenant.as_str()])
        .await
        .unwrap();

    let app = hub.app.clone();
    let first = tokio::spawn(async move {
        call(&app, request("GET", "/api/stats/tokens", &token, None))
            .await
            .0
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !first.is_finished(),
        "opened while another Hub held the tenant"
    );

    let unlock = "SELECT pg_advisory_unlock($1, hashtext($2))";
    other
        .execute(unlock, &[&OPEN_LOCK_CLASS, &tenant.as_str()])
        .await
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(30), first)
        .await
        .expect("still waiting after the lock was released")
        .unwrap();
    assert_eq!(status, StatusCode::OK);

    // The Hub let go of its own lock once the tenant was open.
    other
        .batch_execute("SET lock_timeout = '5s'")
        .await
        .unwrap();
    if let Err(e) = other
        .execute(lock, &[&OPEN_LOCK_CLASS, &tenant.as_str()])
        .await
    {
        panic!("the Hub kept its open lock: {e}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_tenants_open_at_once_on_a_pool_of_one() {
    let Some(url) = url() else { return };
    let hub = hub(&url, 6, 1).await;
    let mut requests = tokio::task::JoinSet::new();
    for (_, token) in hub.tenants.clone() {
        let app = hub.app.clone();
        requests.spawn(async move {
            call(&app, request("GET", "/api/stats/tokens", &token, None))
                .await
                .0
        });
    }
    let all = async {
        while let Some(status) = requests.join_next().await {
            assert_eq!(status.unwrap(), StatusCode::OK);
        }
    };
    tokio::time::timeout(Duration::from_secs(60), all)
        .await
        .expect("tenants on a pool of one didn't all open");
    assert_eq!(hub.data.open_tenants().len(), 6);
}
