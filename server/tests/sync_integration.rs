//! The Hub serves AgentStateGraph's sync protocol (ctx-hub-sync t-015), over
//! real HTTP: stores push to a tenant's store and pull from it with
//! AgentStateGraph's own client, behind the Hub's auth.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use agentstategraph::{CommitOptions, Repository};
use agentstategraph_core::{IntentCategory, Namespace};
use agentstategraph_storage::MemoryStorage;
use agentstategraph_sync_http::HttpRemote;
use ctxone_hub::asd_sync::AsdStores;
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
    multi_tenant_with(serve_sync, Some(AsdStores::memory()))
}

fn multi_tenant_with(
    serve_sync: bool,
    asd_sync: Option<AsdStores>,
) -> (tempfile::TempDir, axum::Router) {
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
            asd_sync,
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

// -- ASD judgment (asd-hub-sync t-012) --

/// A repo's namespace in an ASD store.
const REPO_NS: &str = "app-1a2b3c4d";

/// An ASD store as a machine has one: the repo's namespace, with its index
/// on `main`, marked local-only, and its judgment on `judgment`.
fn asd_store() -> Repository {
    let base = fresh();
    base.create_namespace(REPO_NS).unwrap();
    let repo = base.fork_namespace(Namespace::new(REPO_NS).unwrap());
    repo.init().unwrap();
    repo.set_json("main", "/asd/v1/index/sym", &json!("indexed"), opts())
        .unwrap();
    repo.set_local_only("main", true).unwrap();
    repo
}

/// An ASD store with judgment of its own, ready to push.
fn judged(entry: &str) -> Repository {
    let repo = asd_store();
    repo.branch("judgment", "main").unwrap();
    repo.set_local_only("judgment", false).unwrap();
    repo.delete("judgment", "/asd/v1/index", opts()).unwrap();
    repo.set_json(
        "judgment",
        &format!("/asd/v1/ledger/{entry}"),
        &json!({ "claim": entry }),
        opts(),
    )
    .unwrap();
    repo
}

fn asd(url: &str, store: &str, token: &str) -> HttpRemote {
    HttpRemote::new(format!("{url}/asd"), store).with_token(token)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asd_judgment_syncs_to_a_store_of_its_own_per_tenant() {
    let (_dir, app) = multi_tenant(true);
    let url = serve(app).await;
    tokio::task::spawn_blocking(move || {
        // A laptop pushes the repo's judgment to alice's ASD store; its index
        // never goes.
        let laptop = judged("e1");
        let err = laptop
            .push("hub", &asd(&url, "me", "tok-a"), "main")
            .unwrap_err();
        assert_eq!(err.code(), "local_only_ref");
        laptop
            .push("hub", &asd(&url, "me", "tok-a"), "judgment")
            .unwrap();

        // A desktop with the same repo pulls it, naming the tenant.
        let desktop = asd_store();
        desktop
            .pull("hub", &asd(&url, "alice", "tok-a"), "judgment", opts())
            .unwrap();
        assert_eq!(
            desktop.get_json("judgment", "/asd/v1/ledger/e1").unwrap(),
            json!({ "claim": "e1" })
        );

        // The ASD store offers the repo's judgment and nothing of memory's;
        // alice's memory store offers nothing of the repo's.
        let offered = agentstategraph::sync::Remote::list_refs(&asd(&url, "me", "tok-a")).unwrap();
        assert!(
            offered
                .iter()
                .any(|r| r.namespace.as_str() == REPO_NS && r.name == "judgment"),
            "{offered:?}"
        );
        assert!(
            offered
                .iter()
                .all(|r| r.name != "main" || r.namespace.as_str() != REPO_NS)
        );
        let memory = agentstategraph::sync::Remote::list_refs(
            &HttpRemote::new(&url, "me").with_token("tok-a"),
        )
        .unwrap();
        assert!(
            memory.iter().all(|r| r.namespace.as_str() != REPO_NS),
            "{memory:?}"
        );

        // Bob reaches only his own ASD store, which has none of it.
        let err = asd_store()
            .fetch("hub", &asd(&url, "alice", "tok-b"), &[])
            .unwrap_err();
        assert!(err.to_string().contains("403"), "{err}");
        let err = asd_store()
            .fetch("hub", &asd(&url, "me", "tok-b"), &["judgment"])
            .unwrap_err();
        assert_eq!(err.code(), "no_such_ref", "{err}");

        // No token, or a wrong one, reaches nothing.
        for token in [None, Some("nope")] {
            let mut remote = HttpRemote::new(format!("{url}/asd"), "me");
            if let Some(t) = token {
                remote = remote.with_token(t);
            }
            let err = asd_store().fetch("hub", &remote, &[]).unwrap_err();
            assert!(err.to_string().contains("401"), "{err}");
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_single_tenant_hub_keeps_asd_judgment_in_a_file_beside_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let file = ctxone_hub::asd_sync::sqlite_path(&dir.path().join("ctxone.db"));
    let repo = Arc::new(fresh());
    let app = http::router_with_config(
        repo.clone(),
        Arc::new(SessionRegistry::new()),
        http::HubConfig {
            serve_sync: true,
            asd_sync: Some(AsdStores::Sqlite(file.clone())),
            ..Default::default()
        },
    );
    let url = serve(app).await;
    // Nothing is made until a sync asks for it.
    assert!(!file.exists());
    tokio::task::spawn_blocking(move || {
        let hub = HttpRemote::new(format!("{url}/asd"), "me");
        judged("e1").push("hub", &hub, "judgment").unwrap();
        let desktop = asd_store();
        desktop
            .pull(
                "hub",
                &HttpRemote::new(format!("{url}/asd"), "default"),
                "judgment",
                opts(),
            )
            .unwrap();
        assert_eq!(
            desktop.get_json("judgment", "/asd/v1/ledger/e1").unwrap(),
            json!({ "claim": "e1" })
        );
        assert!(file.exists(), "{}", file.display());
        // The Hub's own store has none of it.
        assert!(
            !repo
                .list_namespaces()
                .unwrap()
                .iter()
                .any(|n| n.as_str() == REPO_NS)
        );
        let err = asd_store()
            .fetch("hub", &HttpRemote::new(format!("{url}/asd"), "alice"), &[])
            .unwrap_err();
        assert!(err.to_string().contains("403"), "{err}");
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asd_sync_is_off_unless_asked_for() {
    for (serve_sync, stores) in [(false, Some(AsdStores::memory())), (true, None)] {
        let (_dir, app) = multi_tenant_with(serve_sync, stores);
        let url = serve(app).await;
        tokio::task::spawn_blocking(move || {
            let err = asd_store()
                .fetch("hub", &asd(&url, "me", "tok-a"), &[])
                .unwrap_err();
            assert!(err.to_string().contains("404"), "{err}");
        })
        .await
        .unwrap();
    }
}
