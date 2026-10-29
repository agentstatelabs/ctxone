//! A Postgres hub's project registry and session statistics: the `ctxone_*`
//! tables [`PgStore`] keeps on AgentStateGraph's pool, scoped to the hub's
//! tenant.
//!
//! Skipped unless `TEST_DATABASE_URL` is set; `CTXONE_REQUIRE_POSTGRES` turns
//! the skip into a failure (CI sets both). Each test uses its own tenants, so a
//! shared database is fine.

use std::collections::BTreeMap;
use std::sync::Arc;

use agentstategraph::Repository;
use agentstategraph_storage::PostgresStorage;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ctxone_hub::http;
use ctxone_hub::hub_store::{HubStore, PgStore};
use ctxone_hub::memory_tools::{ModelUsage, SessionRegistry};
use ctxone_hub::project::{DetectResult, RegistryError, detect_project};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

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

fn connect(url: &str, tag: &str) -> PostgresStorage {
    let tenant = format!("ctx-{tag}-{}", uuid::Uuid::new_v4().simple());
    PostgresStorage::connect_tenant_blocking(url, &tenant, 4).expect("connect")
}

/// A hub's store for a fresh tenant.
fn store(url: &str, tag: &str) -> HubStore {
    HubStore::Postgres(PgStore::new(&connect(url, tag)).expect("hub tables"))
}

/// The sessions a registry holds besides the built-in `default`, sorted.
fn sessions(registry: &SessionRegistry) -> Vec<String> {
    let mut ids = registry.list_ids();
    ids.retain(|id| id != "default");
    ids.sort();
    ids
}

#[test]
fn projects_round_trip() {
    let Some(url) = url() else { return };
    let store = store(&url, "projects");

    store
        .register_project(
            "app",
            Some("https://git.example.com/g/app"),
            "app-ns",
            Some("App"),
            Some("/opt/app"),
        )
        .unwrap();
    store
        .register_project("bare", None, "bare", None, None)
        .unwrap();
    store.add_local_path("app", "/srv/app").unwrap();
    store.add_local_path("app", "/srv/app").unwrap(); // idempotent

    let app = store.resolve_by_id("app").unwrap().expect("app");
    assert_eq!(app.namespace_id, "app-ns");
    assert_eq!(app.display_name.as_deref(), Some("App"));
    assert_eq!(app.local_paths, ["/opt/app", "/srv/app"]);
    assert!(app.created_at.ends_with('Z'), "{}", app.created_at);

    let by_remote = store
        .resolve_by_remote_url("https://git.example.com/g/app")
        .unwrap()
        .expect("by remote");
    assert_eq!(by_remote.id, "app");
    assert!(store.resolve_by_id("nope").unwrap().is_none());
    assert!(
        store
            .resolve_by_remote_url("https://nope")
            .unwrap()
            .is_none()
    );

    let all = store.list_projects().unwrap();
    let ids: Vec<_> = all.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"app") && ids.contains(&"bare"));
    let bare = all.iter().find(|p| p.id == "bare").unwrap();
    assert!(bare.remote_url.is_none() && bare.local_paths.is_empty());
}

#[test]
fn duplicates_are_reported_as_duplicates() {
    let Some(url) = url() else { return };
    let store = store(&url, "dup");
    store
        .register_project("a", Some("https://r/a"), "a", None, None)
        .unwrap();

    let same_id = store.register_project("a", None, "a2", None, None);
    assert!(
        matches!(same_id, Err(RegistryError::Duplicate(_))),
        "{same_id:?}"
    );
    let same_remote = store.register_project("b", Some("https://r/a"), "b", None, None);
    assert!(
        matches!(same_remote, Err(RegistryError::Duplicate(_))),
        "{same_remote:?}"
    );
    // The failed registration left nothing behind.
    assert!(store.resolve_by_id("b").unwrap().is_none());

    // A path for a project that isn't registered is an error, not an orphan row.
    let orphan = store.add_local_path("ghost", "/x");
    assert!(matches!(orphan, Err(RegistryError::Other(_))), "{orphan:?}");
}

#[test]
fn tenants_never_see_each_others_projects() {
    let Some(url) = url() else { return };
    let a = store(&url, "iso-a");
    let b = store(&url, "iso-b");

    a.register_project("same", Some("https://r/same"), "a-ns", None, Some("/a"))
        .unwrap();
    // The same id and remote URL in another tenant is not a duplicate.
    b.register_project("same", Some("https://r/same"), "b-ns", None, Some("/b"))
        .unwrap();

    assert_eq!(
        a.resolve_by_id("same").unwrap().unwrap().namespace_id,
        "a-ns"
    );
    assert_eq!(
        b.resolve_by_id("same").unwrap().unwrap().namespace_id,
        "b-ns"
    );
    assert_eq!(
        b.resolve_by_remote_url("https://r/same")
            .unwrap()
            .unwrap()
            .local_paths,
        ["/b"]
    );
    assert_eq!(a.list_projects().unwrap().len(), 1);
    a.register_project("only-a", None, "only-a", None, None)
        .unwrap();
    assert!(b.resolve_by_id("only-a").unwrap().is_none());
    assert_eq!(b.list_projects().unwrap().len(), 1);
}

#[test]
fn detection_finds_a_registered_path() {
    let Some(url) = url() else { return };
    let store = store(&url, "detect");
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let nested = repo.join("src").join("deep");
    std::fs::create_dir_all(&nested).unwrap();

    assert!(matches!(
        detect_project(&nested, Some(&store)),
        DetectResult::NotFound
    ));

    store
        .register_project("by-path", None, "by-path-ns", None, repo.to_str())
        .unwrap();
    match detect_project(&nested, Some(&store)) {
        DetectResult::FoundByPath {
            project_id,
            namespace_id,
            local_path,
        } => {
            assert_eq!(project_id, "by-path");
            assert_eq!(namespace_id, "by-path-ns");
            assert_eq!(local_path, repo.to_str().unwrap());
        }
        other => panic!("expected FoundByPath, got {other:?}"),
    }

    // A `.ctxproject` marker resolves through the same tables.
    std::fs::write(nested.join(".ctxproject"), "by-path\n").unwrap();
    assert!(matches!(
        detect_project(&nested, Some(&store)),
        DetectResult::FoundByFile { .. }
    ));
}

#[test]
fn session_stats_survive_a_restart() {
    let Some(url) = url() else { return };
    let store = store(&url, "stats");

    let before = SessionRegistry::new();
    let s = before.get_or_create("sess-1");
    s.record(4_000, 40_000);
    s.record_llm_usage(
        1_200,
        300,
        900,
        100,
        Some("claude-opus".into()),
        Some("anthropic".into()),
    );
    s.record_extra_tokens(&BTreeMap::from([("reasoning_output".to_string(), 77)]));
    s.set_by_model(BTreeMap::from([(
        "claude-opus".to_string(),
        ModelUsage {
            input_tokens: 1_200,
            output_tokens: 300,
            cache_read_tokens: 900,
            cache_create_tokens: 100,
            call_count: 1,
        },
    )]));
    before.get_or_create("sess-2").record(400, 400);
    before.flush(&store);
    // A second flush updates the rows in place. (With a per-model breakdown,
    // a snapshot's totals are its sums, so this call names its model.)
    s.record_llm_usage(10, 5, 0, 0, Some("claude-opus".into()), None);
    before.flush(&store);

    let after = SessionRegistry::load(&store);
    assert_eq!(sessions(&after), ["sess-1", "sess-2"]);
    let want = before.snapshot("sess-1").unwrap();
    let got = after.snapshot("sess-1").unwrap();
    assert_eq!(got.session_tokens_used, want.session_tokens_used);
    assert_eq!(got.session_tokens_saved, want.session_tokens_saved);
    assert_eq!(got.llm_input_tokens, 1_210);
    assert_eq!(got.llm_output_tokens, 305);
    assert_eq!(got.llm_cache_read_tokens, 900);
    assert_eq!(got.llm_cache_create_tokens, 100);
    assert_eq!(got.llm_call_count, 2);
    assert_eq!(got.last_model, want.last_model);
    assert_eq!(got.last_provider.as_deref(), Some("anthropic"));
    assert_eq!(got.extra_tokens, want.extra_tokens);
    assert_eq!(
        serde_json::to_value(&got.llm_by_model).unwrap(),
        serde_json::to_value(&want.llm_by_model).unwrap()
    );

    // Removing a session deletes its row; the other stays.
    assert!(after.remove("sess-1", Some(&store)));
    assert_eq!(sessions(&SessionRegistry::load(&store)), ["sess-2"]);
}

#[test]
fn tenants_never_see_each_others_sessions() {
    let Some(url) = url() else { return };
    let a = store(&url, "sess-a");
    let b = store(&url, "sess-b");

    let ra = SessionRegistry::new();
    ra.get_or_create("shared-id").record(8_000, 8_000);
    ra.flush(&a);
    let rb = SessionRegistry::new();
    rb.get_or_create("shared-id").record(400, 400);
    rb.flush(&b);

    let la = SessionRegistry::load(&a);
    let lb = SessionRegistry::load(&b);
    assert_eq!(la.snapshot("shared-id").unwrap().session_tokens_used, 2_000);
    assert_eq!(lb.snapshot("shared-id").unwrap().session_tokens_used, 100);

    // Removing it in one tenant leaves the other's row alone.
    lb.remove("shared-id", Some(&b));
    assert!(sessions(&SessionRegistry::load(&b)).is_empty());
    assert_eq!(sessions(&SessionRegistry::load(&a)), ["shared-id"]);
}

/// Hubs starting together on a fresh database each set up the tables; the
/// advisory lock keeps their `CREATE TABLE IF NOT EXISTS` from colliding.
///
/// Run in a schema of its own, since the other tests have created the tables
/// in the default one already.
#[test]
fn hubs_starting_at_once_all_get_their_tables() {
    let Some(url) = url() else { return };
    let schema = format!("ctx_race_{}", uuid::Uuid::new_v4().simple());
    let pool = connect(&url, "race-setup").tenant_pool();
    let in_default = |sql: String| {
        let pool = pool.clone();
        PostgresStorage::run(async move {
            let client = pool.get().await.expect("a pooled connection");
            client.batch_execute(&sql).await.expect(&sql);
        })
    };
    in_default(format!("CREATE SCHEMA {schema}"));

    let sep = if url.contains('?') { '&' } else { '?' };
    let in_schema = format!("{url}{sep}options=-c%20search_path%3D{schema}");
    // Connect one at a time (the storage's own setup isn't under test), then
    // set up the hub's tables all at once.
    let storages: Vec<_> = (0..8)
        .map(|i| connect(&in_schema, &format!("race{i}")))
        .collect();
    let barrier = std::sync::Barrier::new(storages.len());
    std::thread::scope(|scope| {
        for storage in &storages {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                PgStore::new(storage).expect("hub tables")
            });
        }
    });
    drop(storages);
    in_default(format!("DROP SCHEMA {schema} CASCADE"));
}

async fn call(router: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = router.oneshot(req).await.expect("router call");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, body)
}

fn post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

/// The project API of a hub whose graph and registry both live in Postgres,
/// set up the way `ctxone-hub --storage postgres` does it.
#[test]
fn the_project_api_works_on_a_postgres_hub() {
    let Some(url) = url() else { return };
    let storage = connect(&url, "http");
    let tables = PgStore::new(&storage).expect("hub tables");
    let repo = Arc::new(Repository::new(Box::new(storage)));
    repo.init().expect("repo init");
    let router = http::router_with_db_path(
        repo.clone(),
        Arc::new(SessionRegistry::new()),
        http::HubConfig {
            postgres_tables: Some(tables),
            ..Default::default()
        },
        None,
        false,
    );

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (status, body) = call(
            router.clone(),
            post(
                "/api/projects",
                json!({
                    "id": "pgproj",
                    "remote_url": "https://git.example.com/g/pgproj.git",
                    "local_path": "/opt/pgproj",
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "register: {body}");
        assert_eq!(body["remote_url"], "https://git.example.com/g/pgproj");
        let namespaces = repo.list_namespaces().unwrap();
        assert!(
            namespaces.iter().any(|n| n.as_str() == "pgproj"),
            "{namespaces:?}"
        );

        let (status, _) = call(
            router.clone(),
            post("/api/projects", json!({"id": "pgproj"})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        let (status, body) = call(
            router.clone(),
            post(
                "/api/projects/pgproj/paths",
                json!({"local_path": "/srv/pgproj"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "add path: {body}");
        assert_eq!(body["local_paths"], json!(["/opt/pgproj", "/srv/pgproj"]));

        let (status, _) = call(
            router.clone(),
            post("/api/projects/nope/paths", json!({"local_path": "/x"})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) = call(router.clone(), get("/api/projects")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_array().map(Vec::len), Some(1), "{body}");

        let (status, body) = call(router.clone(), get("/api/projects/pgproj")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["namespace"], "pgproj", "{body}");
    });
}
