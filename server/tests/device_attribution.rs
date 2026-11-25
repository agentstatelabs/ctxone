//! Commits carry the caller's device (ctx-hub-sync t-016): the REST API reads
//! it from `X-CTXone-Device`, and every commit made for the request records it
//! with the agent, plan commits included.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use ctxone_hub::{http, memory_tools::SessionRegistry};

const LAPTOP: &str = "0123456789abcdef0123456789abcdef";

fn hub() -> (Arc<Repository>, axum::Router) {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let router = http::router(repo.clone(), Arc::new(SessionRegistry::new()));
    (repo, router)
}

async fn call(router: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = router.clone().oneshot(req).await.expect("router call");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| panic!("non-JSON body: {}", String::from_utf8_lossy(&bytes)))
    };
    (status, body)
}

/// A POST from `agent`, on `device` if given.
fn post(uri: &str, agent: &str, device: Option<&str>, body: Value) -> Request<Body> {
    let mut req = Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header("x-ctxone-agent", agent);
    if let Some(device) = device {
        req = req.header("x-ctxone-device", device);
    }
    req.body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

/// The newest commit on main whose description mentions `needle`.
async fn commit_about(router: &axum::Router, needle: &str) -> Value {
    let (_, log) = call(router, get("/api/log/main?limit=50")).await;
    log.as_array()
        .unwrap()
        .iter()
        .find(|c| {
            c["intent"]["description"]
                .as_str()
                .is_some_and(|d| d.contains(needle))
        })
        .cloned()
        .unwrap_or_else(|| panic!("no commit about {needle:?} in {log}"))
}

#[tokio::test]
async fn a_write_carries_the_device_its_request_names() {
    let (_, router) = hub();
    let (status, _) = call(
        &router,
        post(
            "/api/memory/remember",
            "claude-code",
            Some(LAPTOP),
            json!({"fact": "written on the laptop", "context": "t"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let commit = commit_about(&router, "written on the laptop").await;
    assert_eq!(commit["agent_id"], "claude-code");
    assert_eq!(commit["device"], LAPTOP);
}

#[tokio::test]
async fn a_write_without_a_plausible_device_carries_none() {
    let (_, router) = hub();
    for (fact, device) in [
        ("no device header", None),
        ("an implausible device", Some("not/a device")),
    ] {
        call(
            &router,
            post(
                "/api/memory/remember",
                "claude-code",
                device,
                json!({"fact": fact, "context": "t"}),
            ),
        )
        .await;
        let commit = commit_about(&router, fact).await;
        assert_eq!(commit["agent_id"], "claude-code");
        assert!(commit["device"].is_null(), "{fact}: {commit}");
    }
}

#[tokio::test]
async fn plan_commits_carry_the_device() {
    let (repo, router) = hub();
    let on_laptop = |uri: &str, body: Value| post(uri, "claude-code", Some(LAPTOP), body);
    let steps = [
        on_laptop("/api/plans", json!({"name": "p1"})),
        on_laptop("/api/plans/p1/tasks", json!({"title": "first"})),
        on_laptop("/api/plans/p1/tasks/t-001/start", json!({})),
        on_laptop(
            "/api/plans/p1/tasks/t-001/complete",
            json!({"proof": {"kind": "commit", "value": "abc123"}}),
        ),
    ];
    for step in steps {
        let uri = step.uri().to_string();
        let (status, body) = call(&router, step).await;
        assert!(status.is_success(), "{uri}: {status} {body}");
    }

    // The plan commits TaskStore made inside AgentStateGraph, read from the
    // store itself: every one names the agent and the device.
    let plan_commits: Vec<_> = repo
        .log("main", 50)
        .unwrap()
        .into_iter()
        .filter(|c| {
            matches!(
                c.intent.category,
                agentstategraph_core::IntentCategory::Plan
            )
        })
        .collect();
    assert!(plan_commits.len() >= 4, "{plan_commits:?}");
    for c in &plan_commits {
        assert_eq!(c.agent_id, "claude-code", "{}", c.intent.description);
        assert_eq!(
            c.device.as_deref(),
            Some(LAPTOP),
            "{}",
            c.intent.description
        );
    }
}
