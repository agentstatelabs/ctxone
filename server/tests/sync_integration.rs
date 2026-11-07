//! The Hub serves AgentStateGraph's sync protocol (ctx-hub-sync t-015), over
//! real HTTP: stores push to a tenant's store and pull from it with
//! AgentStateGraph's own client, behind the Hub's auth.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use agentstategraph::{CommitOptions, Repository};
use agentstategraph_core::IntentCategory;
use agentstategraph_storage::MemoryStorage;
use agentstategraph_sync_http::HttpRemote;
use ctxone_hub::tenant::{TenantsFile, TokenHash};
use ctxone_hub::tenant_cache::{MemoryTenants, TenantCache};
use ctxone_hub::{http, memory_tools::SessionRegistry};
use serde_json::json;

fn opts() -> CommitOptions {
    CommitOptions::new("test", IntentCategory::Checkpoint, "write")
}

/// A store as a new machine has one: `init` and nothing else.
fn fresh() -> Repository {
    let repo = Repository::new(Box::new(MemoryStorage::new()));
    repo.init().unwrap();
    repo
}

fn line(tenant: &str, token: &str) -> String {
    format!("{tenant} {}\n", TokenHash::of(token).to_hex())
}

/// Serve `app` on a loopback port; returns its base URL.
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

fn multi_tenant(serve_sync: bool) -> (tempfile::TempDir, axum::Router) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    std::fs::write(
        &path,
        format!("{}{}", line("alice", "tok-a"), line("bob", "tok-b")),
    )
    .unwrap();
    let file = TenantsFile::open_with_reload(&path, Duration::ZERO).unwrap();
    let app = http::multi_tenant_router(
        http::HubConfig {
            serve_sync,
            ..Default::default()
        },
        http::MultiTenant {
            resolver: Arc::new(file),
            data: TenantCache::new(Arc::new(MemoryTenants::default())),
        },
    );
    (dir, app)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_tenant_syncs_its_own_store_and_no_other() {
    let (_dir, app) = multi_tenant(true);
    let url = serve(app).await;
    tokio::task::spawn_blocking(move || {
        let alice = |store: &str| HttpRemote::new(&url, store).with_token("tok-a");

        // A new laptop joins alice's store, writes, and pushes.
        let laptop = fresh();
        laptop.pull("hub", &alice("me"), "main", opts()).unwrap();
        laptop
            .set_json(
                "main",
                "/memory/facts/f1",
                &json!("from the laptop"),
                opts(),
            )
            .unwrap();
        laptop.push("hub", &alice("me"), "main").unwrap();

        // A new desktop pulls it, naming the store by the tenant's name.
        let desktop = fresh();
        desktop
            .pull("hub", &alice("alice"), "main", opts())
            .unwrap();
        assert_eq!(
            desktop.get_json("main", "/memory/facts/f1").unwrap(),
            "from the laptop"
        );

        // Bob's store doesn't have it, and Bob's token reaches only his.
        let bob = HttpRemote::new(&url, "me").with_token("tok-b");
        let other = fresh();
        other.pull("hub", &bob, "main", opts()).unwrap();
        assert!(other.get("main", "/memory/facts/f1").is_err());
        let err = fresh()
            .fetch(
                "hub",
                &HttpRemote::new(&url, "alice").with_token("tok-b"),
                &[],
            )
            .unwrap_err();
        assert!(err.to_string().contains("403"), "{err}");

        // No token, or a wrong one, reaches nothing.
        for remote in [
            HttpRemote::new(&url, "me"),
            HttpRemote::new(&url, "me").with_token("nope"),
        ] {
            let err = fresh().fetch("hub", &remote, &[]).unwrap_err();
            assert!(err.to_string().contains("401"), "{err}");
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_push_sends_no_turns_and_the_turns_ref_never_goes() {
    let repo = Arc::new(fresh());
    let app = http::router_with_config(
        repo.clone(),
        Arc::new(SessionRegistry::new()),
        http::HubConfig {
            serve_sync: true,
            ..Default::default()
        },
    );
    let url = serve(app).await;
    tokio::task::spawn_blocking(move || {
        let hub = HttpRemote::new(&url, "me");
        let laptop = fresh();
        laptop.pull("hub", &hub, "main", opts()).unwrap();
        laptop
            .set_json("main", "/sessions/s1/title", &json!("one"), opts())
            .unwrap();
        ctxone_hub::turns::write_all(&laptop, "s1", &json!({ "t0000": { "text": "hi" } }), opts())
            .unwrap();
        let err = laptop
            .push("hub", &hub, ctxone_hub::turns::TURNS_REF)
            .unwrap_err();
        assert_eq!(err.code(), "local_only_ref");
        laptop.push("hub", &hub, "main").unwrap();

        // The Hub has the envelope and no turns, and offers no turns ref.
        assert_eq!(repo.get_json("main", "/sessions/s1/title").unwrap(), "one");
        assert!(repo.head(ctxone_hub::turns::TURNS_REF).is_err());
        assert!(
            agentstategraph::sync::Remote::list_refs(&hub)
                .unwrap()
                .iter()
                .all(|r| !r.name.starts_with("_local/")),
        );

        // A single-tenant Hub's store is `default` or `me`, and nothing else.
        fresh()
            .fetch("hub", &HttpRemote::new(&url, "default"), &[])
            .unwrap();
        let err = fresh()
            .fetch("hub", &HttpRemote::new(&url, "alice"), &[])
            .unwrap_err();
        assert!(err.to_string().contains("403"), "{err}");
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_is_off_unless_asked_for() {
    let (_dir, app) = multi_tenant(false);
    let url = serve(app).await;
    tokio::task::spawn_blocking(move || {
        let err = fresh()
            .fetch("hub", &HttpRemote::new(&url, "me").with_token("tok-a"), &[])
            .unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
    })
    .await
    .unwrap();
}
