//! End to end (ctx-hub-sync t-006, t-007, t-008, t-015): two machines, each
//! a single-tenant Hub on its own SQLite store with history of its own, sync
//! through one multi-tenant Hub over HTTP. The serving Hub keeps its tenants
//! in memory, and on Postgres too when `TEST_DATABASE_URL` is set
//! (`CTXONE_REQUIRE_POSTGRES` makes that a must).
//!
//! Covered: memory, a plan with a task renumbered after both machines added
//! one offline, a session's envelope, a reminder that doesn't run on the
//! machine that pulled it, and personal agent rules all reach the other
//! machine; a store with its own history joins the remote's; no raw turn
//! and no credential-shaped value reaches the serving Hub.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use agentstategraph::Repository;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ctxone_hub::tenant::{Tenant, TenantsFile, TokenHash};
use ctxone_hub::tenant_cache::{MemoryTenants, PostgresTenants, TenantCache};
use ctxone_hub::{http, memory_tools::SessionRegistry};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

/// The serving Hub: one tenant for the user, behind its token.
struct Serving {
    url: String,
    data: TenantCache,
    tenant: Tenant,
    token: String,
    _dir: tempfile::TempDir,
}

async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    format!("http://{addr}")
}

async fn serving(data: TenantCache) -> Serving {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let tenant = Tenant::new(&format!("me-{}", &id[..12])).unwrap();
    let token = format!("token-{id}");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    std::fs::write(
        &path,
        format!("{tenant} {}\n", TokenHash::of(&token).to_hex()),
    )
    .unwrap();
    let file = TenantsFile::open_with_reload(&path, Duration::ZERO).unwrap();
    let app = http::multi_tenant_router(
        http::HubConfig {
            serve_sync: true,
            ..Default::default()
        },
        http::MultiTenant {
            resolver: Arc::new(file),
            data: data.clone(),
        },
    );
    Serving {
        url: serve(app).await,
        data,
        tenant,
        token,
        _dir: dir,
    }
}

/// The serving Hubs to run on: memory, and Postgres when there's one.
async fn backends() -> Vec<(&'static str, Serving)> {
    let mut all = vec![(
        "memory",
        serving(TenantCache::new(Arc::new(MemoryTenants::default()))).await,
    )];
    match std::env::var("TEST_DATABASE_URL") {
        Ok(url) if !url.is_empty() => {
            let id = uuid::Uuid::new_v4().simple().to_string();
            let root = agentstategraph_storage::PostgresStorage::connect_tenant_with_pool_size(
                &url,
                &format!("root-{}", &id[..12]),
                4,
            )
            .await
            .expect("connect");
            let data = TenantCache::new(Arc::new(PostgresTenants::new(root).expect("tenants")));
            all.push(("postgres", serving(data).await));
        }
        _ => {
            let required =
                std::env::var("CTXONE_REQUIRE_POSTGRES").is_ok_and(|v| !v.is_empty() && v != "0");
            assert!(
                !required,
                "CTXONE_REQUIRE_POSTGRES is set, but TEST_DATABASE_URL isn't"
            );
            eprintln!("skip postgres: TEST_DATABASE_URL not set");
        }
    }
    all
}

/// One machine: a single-tenant Hub on its own SQLite store, its schema
/// brought up to date as a running Hub's is, with the serving Hub's token in
/// its token file.
struct Machine {
    repo: Arc<Repository>,
    router: axum::Router,
    _dir: tempfile::TempDir,
}

impl Machine {
    fn new(token: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hub.db").to_string_lossy().to_string();
        let storage = agentstategraph_storage::SqliteStorage::open(&db).expect("sqlite");
        let repo = Arc::new(Repository::new(Box::new(storage)));
        repo.init().unwrap();
        ctxone_hub::migrations::migrate_asg(&repo).unwrap();
        ctxone_hub::migrations::run_migrations(&repo).unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join("sync-tokens")).unwrap();
        std::fs::write(home.join("sync-tokens").join("hub"), token).unwrap();
        let router = http::router_with_db_path(
            repo.clone(),
            Arc::new(SessionRegistry::new()),
            http::HubConfig {
                sync_home: Some(home),
                ..Default::default()
            },
            Some(db),
            false,
        );
        Self {
            repo,
            router,
            _dir: dir,
        }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().uri(uri).method(method);
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(serde_json::to_vec(&b).unwrap())
            }
            None => Body::empty(),
        };
        let resp = self
            .router
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, v)
    }

    async fn ok(&self, method: &str, uri: &str, body: Option<Value>) -> Value {
        let (status, v) = self.call(method, uri, body).await;
        assert!(status.is_success(), "{method} {uri}: {status} {v}");
        v
    }

    async fn remember(&self, fact: &str, personal: bool) {
        let uri = if personal {
            "/api/memory/remember?namespace=_personal"
        } else {
            "/api/memory/remember"
        };
        self.ok(
            "POST",
            uri,
            Some(json!({ "fact": fact, "context": "facts" })),
        )
        .await;
    }

    /// Connect to the serving Hub and sync the default workspace with it.
    async fn connect(&self, url: &str) {
        self.ok(
            "POST",
            "/api/sync/remotes",
            Some(json!({ "name": "hub", "url": url })),
        )
        .await;
        self.ok(
            "PUT",
            "/api/sync/namespaces/default",
            Some(json!({ "remote": "hub" })),
        )
        .await;
    }

    async fn sync(&self) -> Value {
        self.ok("POST", "/api/sync/run", Some(json!({}))).await
    }

    /// Every value under `prefix` in `ns`, as text.
    fn texts(&self, ns: &str, prefix: &str) -> String {
        let repo = if ns == "default" {
            self.repo
                .fork_namespace(agentstategraph_core::Namespace::default_ns())
        } else {
            self.repo
                .fork_namespace(agentstategraph_core::Namespace::new(ns).unwrap())
        };
        repo.get_json("main", prefix)
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}

fn clean(report: &Value) {
    for w in report["workspaces"].as_array().unwrap() {
        assert!(w.get("error").is_none(), "{w}");
        assert_eq!(w["blocked"], json!([]), "{w}");
        assert_eq!(w["errors"], json!([]), "{w}");
    }
}

/// One workspace's part of a sync report.
fn ws<'a>(report: &'a Value, ns: &str) -> &'a Value {
    report["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["namespace"] == ns)
        .unwrap_or_else(|| panic!("{ns} didn't sync: {report}"))
}

fn outcome(report: &Value, ns: &str, branch: &str) -> String {
    report["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["namespace"] == ns)
        .and_then(|w| {
            w["pulled"]
                .as_array()?
                .iter()
                .find(|p| p["branch"] == branch)
        })
        .map(|p| p["outcome"].as_str().unwrap_or("").to_string())
        .unwrap_or_default()
}

/// A provider-shaped token, put together at run time so no source file holds
/// one.
fn credential() -> String {
    let tail: String = (0..36)
        .map(|i| match i % 3 {
            0 => 'b',
            1 => '4',
            _ => 'R',
        })
        .collect();
    format!("ghp_{tail}")
}

async fn two_machines_sync_through_a_hub(hub: &Serving) {
    let laptop = Machine::new(&hub.token);
    let desktop = Machine::new(&hub.token);

    // The laptop's own history: memory, an agent rule, a plan, a session
    // with its turns, and a reminder that runs a command.
    laptop.remember("the laptop's fact", false).await;
    laptop.remember("my own agent rule", true).await;
    laptop
        .ok("POST", "/api/plans", Some(json!({ "name": "trip" })))
        .await;
    laptop
        .ok(
            "POST",
            "/api/plans/trip/tasks",
            Some(json!({ "title": "pack" })),
        )
        .await;
    laptop
        .ok("PUT", "/api/sessions/s1/title", Some(json!("One")))
        .await;
    laptop
        .ok(
            "PUT",
            "/api/sessions/s1/turns",
            Some(json!({ "t0000": { "text": "the raw transcript" } })),
        )
        .await;
    let reminder = laptop
        .ok(
            "POST",
            "/api/reminders",
            Some(json!({
                "title": "rotate logs",
                "instructions": "run it",
                "due_at": "2020-01-01T00:00:00Z",
                "autonomous": true,
                "commands": ["true"],
            })),
        )
        .await;
    let reminder_id = reminder["id"].as_str().unwrap().to_string();

    // The desktop has history of its own.
    desktop.remember("the desktop's fact", false).await;

    // The laptop goes first: it joins the tenant's history (a fresh tenant
    // has CTXone's own first commits) and pushes.
    laptop.connect(&hub.url).await;
    let first = laptop.sync().await;
    clean(&first);
    assert_eq!(outcome(&first, "default", "main"), "merged", "{first}");

    // The desktop joins too, and gets everything that syncs.
    desktop.connect(&hub.url).await;
    let joined = desktop.sync().await;
    clean(&joined);
    assert_eq!(outcome(&joined, "default", "main"), "merged", "{joined}");
    assert!(
        desktop
            .texts("default", "/memory")
            .contains("the laptop's fact")
    );
    assert!(
        desktop
            .texts("default", "/memory")
            .contains("the desktop's fact")
    );
    assert!(
        desktop
            .texts("_personal", "/memory")
            .contains("my own agent rule")
    );
    assert!(desktop.texts("default", "/plans").contains("pack"));
    assert_eq!(
        desktop.repo.get_json("main", "/sessions/s1/title").unwrap(),
        "One"
    );
    // The turns stayed on the laptop.
    let (_, turns) = desktop
        .call("GET", "/api/sessions/s1/turns?tree=true", None)
        .await;
    assert!(turns.is_null(), "{turns}");
    // The reminder is here, but its command doesn't run here until approved.
    let due = desktop.ok("GET", "/api/reminders/due", None).await;
    let pulled = due
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == reminder_id)
        .unwrap_or_else(|| panic!("the reminder didn't arrive: {due}"));
    assert_eq!(pulled["trusted_here"], false, "{pulled}");

    // The laptop gets the desktop's history back.
    clean(&laptop.sync().await);
    assert!(
        laptop
            .texts("default", "/memory")
            .contains("the desktop's fact")
    );

    // Both add a task to the plan offline: both mint t-002.
    laptop
        .ok(
            "POST",
            "/api/plans/trip/tasks",
            Some(json!({ "title": "laptop's task" })),
        )
        .await;
    desktop
        .ok(
            "POST",
            "/api/plans/trip/tasks",
            Some(json!({ "title": "desktop's task" })),
        )
        .await;
    clean(&laptop.sync().await);
    let renumbered = desktop.sync().await;
    clean(&renumbered);
    let moves = &ws(&renumbered, "default")["pulled"][0]["renumbered"];
    assert!(moves.to_string().contains("t-003"), "{renumbered}");
    clean(&laptop.sync().await);
    let tasks = laptop.ok("GET", "/api/plans/trip/tasks", None).await;
    let titles: Vec<(String, String)> = tasks
        .as_array()
        .or_else(|| tasks["tasks"].as_array())
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["id"].as_str().unwrap_or("").to_string(),
                t["title"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert!(
        titles.contains(&("t-002".into(), "laptop's task".into())),
        "{titles:?}"
    );
    assert!(
        titles.contains(&("t-003".into(), "desktop's task".into())),
        "{titles:?}"
    );

    // A credential stops the push, and names the record to forget.
    laptop
        .remember(&format!("CI uses {}", credential()), false)
        .await;
    let blocked = laptop.sync().await;
    let findings = &ws(&blocked, "default")["blocked"][0]["findings"];
    assert!(
        findings
            .as_array()
            .is_some_and(|f| f.iter().any(|f| f["forget"].is_string())),
        "{blocked}"
    );
    assert!(!blocked.to_string().contains(&credential()));

    // What reached the serving Hub: no raw turn, no turns ref, and no
    // credential.
    let state = hub.data.get(&hub.tenant).await.unwrap();
    let served = state.repo.clone();
    assert!(served.get("main", "/sessions/s1/turns").is_err());
    assert!(served.head(ctxone_hub::turns::TURNS_REF).is_err());
    assert_eq!(
        served.get_json("main", "/sessions/s1/title").unwrap(),
        "One"
    );
    let everything = served.get_json("main", "/").unwrap().to_string();
    assert!(!everything.contains("the raw transcript"));
    assert!(!everything.contains(&credential()));

    // Where each branch stands, as the last sync left it.
    let status = laptop.ok("GET", "/api/sync/status", None).await;
    assert!(
        status["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["namespace"] == "default"),
        "{status}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_machines_sync_through_a_multi_tenant_hub() {
    for (backend, hub) in backends().await {
        eprintln!("serving Hub on {backend}");
        two_machines_sync_through_a_hub(&hub).await;
    }
}
