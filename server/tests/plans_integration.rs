//! HTTP integration tests for the plans API.
//!
//! Covers create → add → start → complete / abandon flows plus the
//! multi-agent orchestration pattern via `assigned_to` + `plan_next`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use ctxone_hub::{http, memory_tools::SessionRegistry};

fn test_router() -> axum::Router {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    http::router(repo, sessions)
}

async fn call_json(router: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = router.oneshot(req).await.expect("router call");
    let status = resp.status();
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json_value: Value = if body_bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body_bytes).unwrap_or_else(|_| {
            panic!(
                "non-JSON body for response: {}",
                String::from_utf8_lossy(&body_bytes)
            )
        })
    };
    (status, json_value)
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("GET")
        .body(Body::empty())
        .unwrap()
}

fn post_json_with_agent(uri: &str, agent: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header("x-ctxone-agent", agent)
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get_with_agent(uri: &str, agent: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("GET")
        .header("x-ctxone-agent", agent)
        .body(Body::empty())
        .unwrap()
}

fn delete_with_agent(uri: &str, agent: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("DELETE")
        .header("x-ctxone-agent", agent)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn create_plan_returns_201_with_plan_object() {
    let router = test_router();
    let (status, body) = call_json(
        router,
        post_json_with_agent(
            "/api/plans",
            "claude-code",
            json!({"name": "website-v2", "description": "Brand pivot"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["name"], "website-v2");
    assert_eq!(body["status"], "active");
    assert_eq!(body["created_by"], "claude-code");
    assert_eq!(body["task_counts"]["total"], 0);
}

#[tokio::test]
async fn create_duplicate_plan_returns_409() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "claude-code", json!({"name": "p1"})),
    )
    .await;
    let resp = router
        .oneshot(post_json_with_agent(
            "/api/plans",
            "claude-code",
            json!({"name": "p1"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body_bytes);
    assert!(text.contains("already exists"), "got body: {}", text);
}

#[tokio::test]
async fn list_plans_returns_empty_by_default() {
    let router = test_router();
    let (status, body) = call_json(router, get("/api/plans")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn list_plans_filter_by_status() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p2"})),
    )
    .await;
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans/p2/archive", "test", json!({})),
    )
    .await;

    let (_, body) = call_json(router.clone(), get("/api/plans?status=active")).await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "p1");

    let (_, body) = call_json(router, get("/api/plans?status=archived")).await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "p2");
}

#[tokio::test]
async fn get_plan_includes_tasks() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans/p1/tasks", "test", json!({"title": "first"})),
    )
    .await;

    let (status, body) = call_json(router, get("/api/plans/p1")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "p1");
    let tasks = body["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["title"], "first");
    assert_eq!(tasks[0]["status"], "pending");
}

#[tokio::test]
async fn get_plan_unknown_returns_404() {
    let router = test_router();
    let resp = router.oneshot(get("/api/plans/ghost")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn add_task_with_priority_and_assigned_to() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;

    let (status, body) = call_json(
        router,
        post_json_with_agent(
            "/api/plans/p1/tasks",
            "test",
            json!({
                "title": "ship it",
                "priority": "high",
                "assigned_to": "claude-code",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["priority"], "high");
    assert_eq!(body["assigned_to"], "claude-code");
    assert_eq!(body["status"], "pending");
}

#[tokio::test]
async fn start_task_warns_when_another_is_in_progress() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "alice", json!({"name": "p1"})),
    )
    .await;
    let mut ids = Vec::new();
    for title in ["first", "second"] {
        let (_, t) = call_json(
            router.clone(),
            post_json_with_agent("/api/plans/p1/tasks", "alice", json!({ "title": title })),
        )
        .await;
        ids.push(t["id"].as_str().unwrap().to_string());
    }

    // First start: nothing else in progress → no warning.
    let (status, body) = call_json(
        router.clone(),
        post_json_with_agent(&format!("/api/plans/p1/tasks/{}/start", ids[0]), "alice", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("warning").is_none(), "no warning for the only active task");

    // Second start: first is in progress → non-blocking warning naming it.
    let (status, body) = call_json(
        router,
        post_json_with_agent(&format!("/api/plans/p1/tasks/{}/start", ids[1]), "alice", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "start is non-blocking");
    assert_eq!(body["status"], "in_progress");
    let warning = body["warning"].as_str().expect("warning field present");
    assert!(warning.contains(&ids[0]), "warning names the other in-progress task");
}

#[tokio::test]
async fn start_task_then_complete_with_commit_proof() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "alice", json!({"name": "p1"})),
    )
    .await;
    let (_, task_body) = call_json(
        router.clone(),
        post_json_with_agent("/api/plans/p1/tasks", "alice", json!({"title": "do it"})),
    )
    .await;
    let id = task_body["id"].as_str().unwrap().to_string();

    let (status, body) = call_json(
        router.clone(),
        post_json_with_agent(
            &format!("/api/plans/p1/tasks/{}/start", id),
            "alice",
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "in_progress");
    assert_eq!(body["started_by"], "alice");

    let (status, body) = call_json(
        router,
        post_json_with_agent(
            &format!("/api/plans/p1/tasks/{}/complete", id),
            "alice",
            json!({"proof": {"kind": "commit", "value": "abc123"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "done");
    assert_eq!(body["proof"]["kind"], "commit");
    assert_eq!(body["proof"]["value"], "abc123");
    assert_eq!(body["completed_by"], "alice");
}

#[tokio::test]
async fn complete_without_proof_returns_400() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let (_, task_body) = call_json(
        router.clone(),
        post_json_with_agent("/api/plans/p1/tasks", "test", json!({"title": "do it"})),
    )
    .await;
    let id = task_body["id"].as_str().unwrap().to_string();
    let _ = call_json(
        router.clone(),
        post_json_with_agent(
            &format!("/api/plans/p1/tasks/{}/start", id),
            "test",
            json!({}),
        ),
    )
    .await;

    // Proof with empty value should 400.
    let resp = router
        .oneshot(post_json_with_agent(
            &format!("/api/plans/p1/tasks/{}/complete", id),
            "test",
            json!({"proof": {"kind": "commit", "value": ""}}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn start_blocked_task_returns_409_with_blocker_list() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let (_, t1) = call_json(
        router.clone(),
        post_json_with_agent("/api/plans/p1/tasks", "test", json!({"title": "root"})),
    )
    .await;
    let t1_id = t1["id"].as_str().unwrap().to_string();

    let (_, t2) = call_json(
        router.clone(),
        post_json_with_agent(
            "/api/plans/p1/tasks",
            "test",
            json!({"title": "blocked", "blocked_by": [t1_id]}),
        ),
    )
    .await;
    let t2_id = t2["id"].as_str().unwrap().to_string();

    let resp = router
        .oneshot(post_json_with_agent(
            &format!("/api/plans/p1/tasks/{}/start", t2_id),
            "test",
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn abandon_requires_reason() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let (_, t) = call_json(
        router.clone(),
        post_json_with_agent("/api/plans/p1/tasks", "test", json!({"title": "t"})),
    )
    .await;
    let id = t["id"].as_str().unwrap().to_string();

    // Empty reason is rejected.
    let resp = router
        .clone()
        .oneshot(post_json_with_agent(
            &format!("/api/plans/p1/tasks/{}/abandon", id),
            "test",
            json!({"reason": ""}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // With reason, succeeds.
    let (status, body) = call_json(
        router,
        post_json_with_agent(
            &format!("/api/plans/p1/tasks/{}/abandon", id),
            "test",
            json!({"reason": "superseded"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "abandoned");
    assert_eq!(body["abandoned_reason"], "superseded");
}

#[tokio::test]
async fn list_plan_tasks_returns_empty_for_new_plan() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let (status, body) = call_json(router, get("/api/plans/p1/tasks")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn get_task_by_id() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let (_, t) = call_json(
        router.clone(),
        post_json_with_agent("/api/plans/p1/tasks", "test", json!({"title": "hello"})),
    )
    .await;
    let id = t["id"].as_str().unwrap();
    let (status, body) = call_json(router, get(&format!("/api/plans/p1/tasks/{}", id))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], id);
    assert_eq!(body["title"], "hello");
}

#[tokio::test]
async fn get_task_unknown_returns_404() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let resp = router
        .oneshot(get("/api/plans/p1/tasks/t-999"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn next_task_returns_null_when_empty() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let (status, body) = call_json(router, get("/api/plans/p1/next")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["task"].is_null());
}

#[tokio::test]
async fn multi_agent_orchestration_via_assigned_to() {
    // Two agents, each calling plan_next with their own assigned_to,
    // pick up their own tasks without stepping on each other.
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "lead", json!({"name": "p1"})),
    )
    .await;

    // Task A for codex, Task B for claude, Task C unassigned.
    let _ = call_json(
        router.clone(),
        post_json_with_agent(
            "/api/plans/p1/tasks",
            "lead",
            json!({"title": "A", "priority": "high", "assigned_to": "codex"}),
        ),
    )
    .await;
    let _ = call_json(
        router.clone(),
        post_json_with_agent(
            "/api/plans/p1/tasks",
            "lead",
            json!({"title": "B", "priority": "medium", "assigned_to": "claude-code"}),
        ),
    )
    .await;
    let _ = call_json(
        router.clone(),
        post_json_with_agent(
            "/api/plans/p1/tasks",
            "lead",
            json!({"title": "C", "priority": "low"}),
        ),
    )
    .await;

    // codex asks with assigned_to=me — sees A.
    let (_, body) = call_json(
        router.clone(),
        get_with_agent("/api/plans/p1/next?assigned_to=me", "codex"),
    )
    .await;
    assert_eq!(body["task"]["title"], "A");

    // claude-code asks with assigned_to=me — sees B.
    let (_, body) = call_json(
        router.clone(),
        get_with_agent("/api/plans/p1/next?assigned_to=me", "claude-code"),
    )
    .await;
    assert_eq!(body["task"]["title"], "B");

    // An unknown agent with include_unassigned=true sees C.
    let (_, body) = call_json(
        router.clone(),
        get_with_agent(
            "/api/plans/p1/next?assigned_to=me&include_unassigned=true",
            "gemini",
        ),
    )
    .await;
    assert_eq!(body["task"]["title"], "C");

    // An unknown agent with assigned_only=true sees nothing.
    let (_, body) = call_json(
        router,
        get_with_agent(
            "/api/plans/p1/next?assigned_to=me&assigned_only=true",
            "gemini",
        ),
    )
    .await;
    assert!(body["task"].is_null());
}

#[tokio::test]
async fn parent_task_with_subtask_rolls_up() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let (_, parent) = call_json(
        router.clone(),
        post_json_with_agent("/api/plans/p1/tasks", "test", json!({"title": "parent"})),
    )
    .await;
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let (status, child) = call_json(
        router,
        post_json_with_agent(
            "/api/plans/p1/tasks",
            "test",
            json!({"title": "child", "parent_id": parent_id}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(child["parent_id"], parent["id"]);
}

#[tokio::test]
async fn archive_plan_sets_status_and_archived_at() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let (status, body) = call_json(
        router,
        post_json_with_agent("/api/plans/p1/archive", "test", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "archived");
    assert!(body["archived_at"].is_string());
}

#[tokio::test]
async fn delete_plan_removes_it() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let resp = router
        .clone()
        .oneshot(delete_with_agent("/api/plans/p1", "test"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = router.oneshot(get("/api/plans/p1")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn completing_last_task_promotes_plan_to_completed() {
    let router = test_router();
    let _ = call_json(
        router.clone(),
        post_json_with_agent("/api/plans", "test", json!({"name": "p1"})),
    )
    .await;
    let (_, t) = call_json(
        router.clone(),
        post_json_with_agent("/api/plans/p1/tasks", "test", json!({"title": "only one"})),
    )
    .await;
    let id = t["id"].as_str().unwrap().to_string();

    let _ = call_json(
        router.clone(),
        post_json_with_agent(
            &format!("/api/plans/p1/tasks/{}/start", id),
            "test",
            json!({}),
        ),
    )
    .await;
    let _ = call_json(
        router.clone(),
        post_json_with_agent(
            &format!("/api/plans/p1/tasks/{}/complete", id),
            "test",
            json!({"proof": {"kind": "text", "value": "done"}}),
        ),
    )
    .await;

    let (_, body) = call_json(router, get("/api/plans/p1")).await;
    assert_eq!(body["status"], "completed");
}
