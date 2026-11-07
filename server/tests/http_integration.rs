//! Integration tests for the CtxOne Hub HTTP API.
//!
//! These tests spin up the axum router in-process (no TCP binding) and hit
//! it via `tower::ServiceExt::oneshot`. They cover the full round-trip
//! including request parsing, handler logic, and JSON response shape.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use agentstategraph::Repository;
use agentstategraph_storage::MemoryStorage;
use ctxone_hub::{http, memory_tools::SessionRegistry};

/// Build a fresh in-memory Hub + router for each test.
fn test_router() -> axum::Router {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    http::router(repo, sessions)
}

/// Helper: call the router and parse the JSON response body.
async fn call_json(router: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = router.oneshot(req).await.expect("router call");
    let status = resp.status();
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json_value: Value = if body_bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body_bytes)
            .unwrap_or_else(|_| panic!("non-JSON body: {}", String::from_utf8_lossy(&body_bytes)))
    };
    (status, json_value)
}

/// Helper: call the router and return the status + raw body string.
/// Use this for endpoints that may return plain-text errors (500s etc).
async fn call_raw(router: axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = router.oneshot(req).await.expect("router call");
    let status = resp.status();
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body_bytes).into_owned();
    (status, body)
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("GET")
        .body(Body::empty())
        .unwrap()
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Build a GET with an `X-CTXone-Session` header attached.
fn get_with_session(uri: &str, session: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("GET")
        .header("x-ctxone-session", session)
        .body(Body::empty())
        .unwrap()
}

/// Build a POST with an `X-CTXone-Session` header attached.
fn post_with_session(uri: &str, session: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header("x-ctxone-session", session)
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Build a POST with an `X-CTXone-Agent` header attached.
fn post_with_agent(uri: &str, agent: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header("x-ctxone-agent", agent)
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

// -------- Health --------

#[tokio::test]
async fn health_returns_ok() {
    let router = test_router();
    let (status, body) = call_json(router, get("/api/health")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "ctxone-hub");
}

// -------- Remember / recall round-trip --------

/// `ctx remember --context` posts here. The MCP `remember` tool rejected a
/// `/` in the context — it becomes a path segment (`/memory/<ctx>/<id>`) and
/// would write into another context's subtree — but this route took it.
#[tokio::test]
async fn remember_rejects_a_slash_in_context_like_the_mcp_tool() {
    let router = test_router();
    let (status, body) = call_raw(
        router,
        post_json(
            "/api/memory/remember",
            json!({"fact": "x", "context": "projects/app-a"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("'/'"), "{body}");
}

/// The MCP tool caps a fact at 64 KB; this route stored any size.
#[tokio::test]
async fn remember_caps_an_oversized_fact_like_the_mcp_tool() {
    let router = test_router();
    let (status, body) = call_json(
        router,
        post_json(
            "/api/memory/remember",
            json!({"fact": "a".repeat(64 * 1024 + 10), "context": "test"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truncated"], true);
}

#[tokio::test]
async fn remember_then_recall_round_trip() {
    let router = test_router();

    // Remember a fact
    let (status, body) = call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({
                "fact": "CtxOne uses BSL-1.1",
                "importance": "high",
                "context": "licensing",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(
        body["path"]
            .as_str()
            .unwrap()
            .starts_with("/memory/licensing/")
    );
    let commit_id = body["commit_id"].as_str().unwrap().to_string();
    assert!(!commit_id.is_empty());

    // Recall by a term that appears in the value (not just the path)
    let (status, body) = call_json(router, get("/api/memory/recall?topic=BSL&budget=1500")).await;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert!(!results.is_empty(), "expected at least one result");
    let has_fact = results.iter().any(|r| {
        r["value"]
            .as_str()
            .map(|v| v.contains("BSL-1.1"))
            .unwrap_or(false)
    });
    assert!(
        has_fact,
        "expected a result containing BSL-1.1, got: {:?}",
        results
    );
    assert!(body["topic_matches"].as_u64().unwrap() >= 1);
    assert_eq!(body["pinned_count"], 0);
}

#[tokio::test]
async fn recall_with_no_matches_returns_empty_results() {
    let router = test_router();
    let (status, body) = call_json(
        router,
        get("/api/memory/recall?topic=nonexistent-topic&budget=1500"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["results"].as_array().unwrap().len(), 0);
    assert_eq!(body["topic_matches"], 0);
}

// -------- Prime + pinned-first recall --------

#[tokio::test]
async fn prime_pinned_always_in_recall() {
    let router = test_router();

    // Prime a pinned section
    let (status, _body) = call_json(
        router.clone(),
        post_json(
            "/api/memory/prime",
            json!({
                "source": "project",
                "pinned": true,
                "sections": [
                    {"title": "Vision", "body": "CtxOne is the memory layer"}
                ],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Recall an unrelated topic — pinned should still appear
    let (status, body) = call_json(
        router,
        get("/api/memory/recall?topic=unrelated-xyz&budget=1500"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pinned_count"], 1);
    let results = body["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .any(|r| r["pinned"].as_bool().unwrap_or(false)),
        "pinned entry missing from recall: {:?}",
        results
    );
}

// -------- Token stats --------

#[tokio::test]
async fn token_stats_track_session_usage() {
    let router = test_router();

    // Remember a fact and run a recall so there's activity
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({
                "fact": "seed fact for token tracking",
                "context": "test",
            }),
        ),
    )
    .await;

    call_json(router.clone(), get("/api/memory/recall?topic=seed")).await;

    let (status, body) = call_json(router, get("/api/stats/tokens")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["session_tokens_used"].as_u64().unwrap() > 0);
    assert!(body["total_graph_size_chars"].as_u64().unwrap() > 0);
}

// -------- Forget --------

#[tokio::test]
async fn forget_removes_a_fact() {
    let router = test_router();

    // Remember and capture path
    let (_, body) = call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "temp fact", "context": "test"}),
        ),
    )
    .await;
    let path = body["path"].as_str().unwrap().to_string();

    // Verify it's searchable
    let (_, body) = call_json(router.clone(), get("/api/memory/recall?topic=temp")).await;
    assert!(body["topic_matches"].as_u64().unwrap() >= 1);

    // Forget it
    let (status, body) = call_json(
        router.clone(),
        post_json(
            "/api/memory/forget",
            json!({"path": path, "reason": "cleanup"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");

    // Verify it's gone
    let (_, body) = call_json(router, get("/api/memory/recall?topic=temp")).await;
    assert_eq!(body["topic_matches"], 0);
}

// -------- Branches --------

#[tokio::test]
async fn create_and_list_branches() {
    let router = test_router();

    // List starts with just main
    let (status, body) = call_json(router.clone(), get("/api/branches")).await;
    assert_eq!(status, StatusCode::OK);
    let initial = body.as_array().unwrap();
    assert!(initial.iter().any(|b| b["name"] == "main"));

    // Create experiment branch
    let (status, body) = call_json(
        router.clone(),
        post_json(
            "/api/branches",
            json!({"name": "experiment", "from": "main"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["name"], "experiment");

    // List includes it
    let (_, body) = call_json(router, get("/api/branches")).await;
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"main"));
    assert!(names.contains(&"experiment"));
}

#[tokio::test]
async fn writes_isolate_to_branch() {
    let router = test_router();

    // Write to main
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "main fact", "context": "test", "ref": "main"}),
        ),
    )
    .await;

    // Create experiment branch from main
    call_json(
        router.clone(),
        post_json(
            "/api/branches",
            json!({"name": "experiment", "from": "main"}),
        ),
    )
    .await;

    // Write to experiment
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "experiment fact", "context": "test", "ref": "experiment"}),
        ),
    )
    .await;

    // Recall on main — only finds main fact
    let (_, body) = call_json(
        router.clone(),
        get("/api/memory/recall?topic=fact&ref=main"),
    )
    .await;
    let results = body["results"].as_array().unwrap();
    let values: Vec<&str> = results.iter().filter_map(|r| r["value"].as_str()).collect();
    assert!(values.iter().any(|v| v.contains("main fact")));
    assert!(!values.iter().any(|v| v.contains("experiment fact")));

    // Recall on experiment — finds both (main fact inherited at branch point)
    let (_, body) = call_json(router, get("/api/memory/recall?topic=fact&ref=experiment")).await;
    let results = body["results"].as_array().unwrap();
    let values: Vec<&str> = results.iter().filter_map(|r| r["value"].as_str()).collect();
    assert!(values.iter().any(|v| v.contains("experiment fact")));
}

// -------- Diff --------

#[tokio::test]
async fn diff_shows_changes_between_branches() {
    let router = test_router();

    // Seed a fact on main
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "base fact", "context": "test"}),
        ),
    )
    .await;

    // Branch and add a fact
    call_json(
        router.clone(),
        post_json("/api/branches", json!({"name": "exp", "from": "main"})),
    )
    .await;
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "new fact", "context": "test", "ref": "exp"}),
        ),
    )
    .await;

    // Diff
    let (status, body) = call_json(router, get("/api/diff?ref_a=main&ref_b=exp")).await;
    assert_eq!(status, StatusCode::OK);
    let ops = body["ops"].as_array().unwrap();
    assert!(!ops.is_empty(), "expected at least one diff op");
}

// -------- Merge --------

#[tokio::test]
async fn merge_branch_into_main() {
    let router = test_router();

    // Seed main and create experiment branch
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "mainfact licensing decision", "context": "test"}),
        ),
    )
    .await;
    call_json(
        router.clone(),
        post_json("/api/branches", json!({"name": "exp", "from": "main"})),
    )
    .await;
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "expfact architecture decision", "context": "test", "ref": "exp"}),
        ),
    )
    .await;

    // Merge exp into main
    let (status, body) = call_json(
        router.clone(),
        post_json(
            "/api/merge",
            json!({
                "source": "exp",
                "target": "main",
                "description": "Merge experiment back to main",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(body["commit_id"].as_str().unwrap().starts_with("sg_"));

    // After merge, main should see both facts — query "decision" matches both
    let (_, body) = call_json(router, get("/api/memory/recall?topic=decision&ref=main")).await;
    let results = body["results"].as_array().unwrap();
    let values: Vec<&str> = results.iter().filter_map(|r| r["value"].as_str()).collect();
    assert!(
        values.iter().any(|v| v.contains("mainfact")),
        "main should still contain the original fact, got: {:?}",
        values
    );
    assert!(
        values.iter().any(|v| v.contains("expfact")),
        "main should now contain the experiment fact, got: {:?}",
        values
    );
}

// -------- Search / ls --------

#[tokio::test]
async fn search_returns_matching_paths() {
    let router = test_router();

    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "searchable content here", "context": "test"}),
        ),
    )
    .await;

    let (status, body) = call_json(router, get("/api/state/main/search?query=searchable")).await;
    assert_eq!(status, StatusCode::OK);
    let results = body.as_array().unwrap();
    assert!(!results.is_empty());
    assert!(results[0]["value"].as_str().unwrap().contains("searchable"));
}

#[tokio::test]
async fn list_paths_under_memory_prefix() {
    let router = test_router();

    // Add a few facts
    for i in 0..3 {
        call_json(
            router.clone(),
            post_json(
                "/api/memory/remember",
                json!({"fact": format!("fact {}", i), "context": "test"}),
            ),
        )
        .await;
    }

    let (status, body) = call_json(router, get("/api/state/main/paths?prefix=/memory")).await;
    assert_eq!(status, StatusCode::OK);
    let paths = body.as_array().unwrap();
    assert!(paths.len() >= 3);
}

// -------- Log --------

#[tokio::test]
async fn log_returns_commit_history() {
    let router = test_router();

    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "first", "context": "test"}),
        ),
    )
    .await;

    let (status, body) = call_json(router, get("/api/log/main?limit=10")).await;
    assert_eq!(status, StatusCode::OK);
    let commits = body.as_array().unwrap();
    assert!(!commits.is_empty());
    assert!(commits[0]["id"].as_str().unwrap().starts_with("sg_"));
}

// -------- stats/{ref_name} --------

#[tokio::test]
async fn stats_returns_ref_metadata() {
    let router = test_router();

    // Seed a commit so stats has something to report
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "seed", "context": "test"}),
        ),
    )
    .await;

    let (status, body) = call_json(router, get("/api/stats/main")).await;
    assert_eq!(status, StatusCode::OK);
    // Stats is a free-form JSON value — just check it's an object and not empty
    assert!(body.is_object(), "expected stats object, got {:?}", body);
}

// -------- context/{project} --------

#[tokio::test]
async fn context_returns_null_when_project_missing() {
    let router = test_router();
    let (status, body) = call_json(router, get("/api/memory/context/ghost")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["project"], "ghost");
    assert!(body["context"].is_null());
}

// -------- summarize_session --------

#[tokio::test]
async fn summarize_session_writes_key_points_and_decisions() {
    let router = test_router();

    let (status, body) = call_json(
        router.clone(),
        post_json(
            "/api/memory/summarize_session",
            json!({
                "session_id": "abc123",
                "key_points": ["picked BSL-1.1", "sqlite default"],
                "decisions": ["ship v0.70"],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["session_id"], "abc123");
    assert_eq!(body["key_points"], 2);
    assert_eq!(body["decisions"], 1);

    // Verify the summary is actually written to the graph
    let (_, state) = call_json(
        router.clone(),
        get("/api/state/main?path=/sessions/abc123/summary"),
    )
    .await;
    let summary = state.as_str().unwrap_or_default();
    assert!(
        summary.contains("BSL-1.1"),
        "summary missing key point: {}",
        summary
    );

    let (_, decisions) = call_json(
        router,
        get("/api/state/main?path=/sessions/abc123/decisions"),
    )
    .await;
    // decisions are stored as a JSON array
    assert!(
        decisions.to_string().contains("ship v0.70"),
        "decisions missing: {}",
        decisions
    );
}

#[tokio::test]
async fn summarize_session_without_decisions_is_fine() {
    let router = test_router();
    let (status, body) = call_json(
        router,
        post_json(
            "/api/memory/summarize_session",
            json!({
                "session_id": "nodecisions",
                "key_points": ["just a note"],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["decisions"], 0);
}

// -------- what_changed_since --------

#[tokio::test]
async fn what_changed_since_returns_recent_commits() {
    let router = test_router();

    // Write a fact so there's something to report
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "new decision", "context": "test"}),
        ),
    )
    .await;

    // since=1970 should match everything
    let (status, body) = call_json(
        router,
        get("/api/memory/what_changed_since?since=1970-01-01T00:00:00Z"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let changes = body.as_array().unwrap();
    assert!(!changes.is_empty(), "expected at least one change");
    assert!(changes[0].get("description").is_some());
}

#[tokio::test]
async fn what_changed_since_filters_out_old_commits() {
    let router = test_router();

    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "old fact", "context": "test"}),
        ),
    )
    .await;

    // since=2099 should match nothing
    let (status, body) = call_json(
        router,
        get("/api/memory/what_changed_since?since=2099-01-01T00:00:00Z"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 0);
}

// -------- why_did_we --------

#[tokio::test]
async fn why_did_we_returns_blame_traces_for_matching_decisions() {
    let router = test_router();

    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({
                "fact": "We chose Rust for performance reasons",
                "context": "architecture",
            }),
        ),
    )
    .await;

    let (status, body) = call_json(router, get("/api/memory/why_did_we?decision=Rust")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["decision"], "Rust");
    let traces = body["traces"].as_array().unwrap();
    assert!(
        !traces.is_empty(),
        "expected at least one blame trace, got {:?}",
        traces
    );
    assert!(traces[0].get("path").is_some());
    assert!(traces[0].get("blame").is_some());
}

#[tokio::test]
async fn why_did_we_returns_empty_for_unknown_decision() {
    let router = test_router();
    let (status, body) = call_json(
        router,
        get("/api/memory/why_did_we?decision=nonexistent-xyz"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["traces"].as_array().unwrap().len(), 0);
}

// -------- blame --------

#[tokio::test]
async fn blame_returns_commit_provenance_for_path() {
    let router = test_router();

    // Write a fact and grab its path
    let (_, body) = call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "blame target", "context": "test"}),
        ),
    )
    .await;
    let path = body["path"].as_str().unwrap().to_string();

    let (status, body) = call_json(
        router,
        get(&format!("/api/blame/main?path={}", urlencoding(&path))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // blame returns structured data — just check the response isn't empty/null
    assert!(
        !body.is_null(),
        "blame should return non-null for a known path"
    );
}

// -------- Error paths --------

#[tokio::test]
async fn create_branch_from_missing_ref_returns_404() {
    let router = test_router();
    let (status, body) = call_raw(
        router,
        post_json(
            "/api/branches",
            json!({"name": "new-branch", "from": "ghost-ref"}),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "branching from missing ref should return 404"
    );
    // Error body should mention the missing ref so users can diagnose
    assert!(
        body.contains("ghost-ref") || body.contains("not found"),
        "expected descriptive error, got: {}",
        body
    );
}

#[tokio::test]
async fn remember_to_missing_branch_returns_404() {
    let router = test_router();
    let (status, _) = call_raw(
        router,
        post_json(
            "/api/memory/remember",
            json!({
                "fact": "doomed",
                "context": "test",
                "ref": "nonexistent-branch",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn malformed_json_remember_returns_400() {
    let router = test_router();
    let req = Request::builder()
        .uri("/api/memory/remember")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from("{ not valid json"))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    // axum's Json extractor returns 400 for invalid JSON
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn remember_missing_required_field_returns_422() {
    let router = test_router();
    // Missing `fact` field
    let req = Request::builder()
        .uri("/api/memory/remember")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"context": "test"}"#))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    // axum returns 422 for missing fields during Json deserialization
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// -------- Per-session token tracking --------

#[tokio::test]
async fn sessions_endpoint_includes_default_on_fresh_hub() {
    let router = test_router();
    let (status, body) = call_json(router, get("/api/stats/sessions")).await;
    assert_eq!(status, StatusCode::OK);
    let sessions = body.as_array().unwrap();
    // New registry always has the "default" session baked in
    assert!(
        sessions.iter().any(|s| s["session_id"] == "default"),
        "expected 'default' session in fresh registry, got: {:?}",
        sessions
    );
}

#[tokio::test]
async fn recall_with_session_header_creates_new_session() {
    let router = test_router();

    // Seed a fact so recall has something to return
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "per-session test fact", "context": "test"}),
        ),
    )
    .await;

    // Recall with a custom session header
    let (status, _) = call_json(
        router.clone(),
        get_with_session("/api/memory/recall?topic=fact", "alice@example.com"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // alice's session should now exist in the registry
    let (_, body) = call_json(router, get("/api/stats/sessions")).await;
    let sessions = body.as_array().unwrap();
    assert!(
        sessions
            .iter()
            .any(|s| s["session_id"] == "alice@example.com"),
        "expected 'alice@example.com' session, got: {:?}",
        sessions
    );
}

#[tokio::test]
async fn recall_counts_toward_only_the_calling_session() {
    let router = test_router();

    // Seed a fact
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "isolation test fact with some content", "context": "test"}),
        ),
    )
    .await;

    // alice does a recall
    call_json(
        router.clone(),
        get_with_session("/api/memory/recall?topic=isolation", "alice"),
    )
    .await;

    // bob hasn't done anything yet — 404 body is plain text, so
    // use call_raw which doesn't expect JSON.
    let (status, bob) = call_raw(router.clone(), get("/api/stats/tokens/bob")).await;
    // bob's session was never created, so 404
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "got unexpected body: {}",
        bob
    );

    // alice's session exists and has nonzero tokens_used
    let (status, alice) = call_json(router.clone(), get("/api/stats/tokens/alice")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(alice["session_id"], "alice");
    assert!(
        alice["session_tokens_used"].as_u64().unwrap_or(0) > 0,
        "expected alice's tokens_used > 0, got: {}",
        alice
    );
}

#[tokio::test]
async fn token_stats_returns_aggregate_across_sessions() {
    let router = test_router();

    // Seed a fact
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "aggregate test fact content", "context": "test"}),
        ),
    )
    .await;

    // Two different sessions each do a recall
    call_json(
        router.clone(),
        get_with_session("/api/memory/recall?topic=aggregate", "alice"),
    )
    .await;
    call_json(
        router.clone(),
        get_with_session("/api/memory/recall?topic=aggregate", "bob"),
    )
    .await;

    // Check each session individually
    let (_, alice) = call_json(router.clone(), get("/api/stats/tokens/alice")).await;
    let (_, bob) = call_json(router.clone(), get("/api/stats/tokens/bob")).await;
    let alice_used = alice["session_tokens_used"].as_u64().unwrap_or(0);
    let bob_used = bob["session_tokens_used"].as_u64().unwrap_or(0);
    assert!(alice_used > 0);
    assert!(bob_used > 0);

    // The aggregate endpoint should sum both
    let (status, agg) = call_json(router, get("/api/stats/tokens")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agg["session_id"], "_aggregate");
    let agg_used = agg["session_tokens_used"].as_u64().unwrap_or(0);
    assert!(
        agg_used >= alice_used + bob_used,
        "aggregate ({}) should be >= alice ({}) + bob ({})",
        agg_used,
        alice_used,
        bob_used
    );
}

#[tokio::test]
async fn session_token_stats_missing_session_returns_404() {
    let router = test_router();
    let (status, _) = call_raw(router, get("/api/stats/tokens/ghost-session")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn remember_with_session_header_does_not_leak_into_default() {
    let router = test_router();

    // alice writes (but writes go through ensureFlat/mark_all_dirty
    // and don't count into her tokens_used — only reads do)
    call_json(
        router.clone(),
        post_with_session(
            "/api/memory/remember",
            "alice",
            json!({"fact": "alice's fact", "context": "test"}),
        ),
    )
    .await;

    // Verify alice's session now exists (because the extractor
    // always returns a session, and we resolved one inside the write
    // handler via mark_all_dirty indirectly touching every session).
    //
    // Actually, remember() doesn't call session_for() currently — it
    // only mark_all_dirty's. So alice's session may or may not
    // exist depending on whether some other code path already
    // created it. What we CAN test: after alice writes, default's
    // tokens_used is still 0 (writes don't record token usage).
    let (_, default) = call_json(router, get("/api/stats/tokens/default")).await;
    assert_eq!(default["session_tokens_used"].as_u64().unwrap_or(99), 0);
}

// -------- Per-tool agent IDs (T2) --------

#[tokio::test]
async fn remember_without_agent_header_defaults_to_ctxone() {
    let router = test_router();

    // Write a fact without an X-CTXone-Agent header
    let (_, body) = call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "default agent test", "context": "test"}),
        ),
    )
    .await;
    let path = body["path"].as_str().unwrap().to_string();

    // Look at the log to confirm agent_id is "ctxone"
    let (_, log) = call_json(router, get("/api/log/main?limit=5")).await;
    let commits = log.as_array().unwrap();
    let matching = commits.iter().find(|c| {
        c["intent"]["description"]
            .as_str()
            .map(|d| d.contains("default agent test"))
            .unwrap_or(false)
    });
    assert!(
        matching.is_some(),
        "expected a commit with our fact, got: {:?}",
        commits
    );
    assert_eq!(
        matching.unwrap()["agent_id"].as_str().unwrap(),
        "ctxone",
        "default agent should be 'ctxone', got commit: {:?}",
        matching.unwrap()
    );
    // Silence unused-variable warning in the non-assertion path
    let _ = path;
}

#[tokio::test]
async fn remember_with_agent_header_is_recorded_in_blame() {
    let router = test_router();

    call_json(
        router.clone(),
        post_with_agent(
            "/api/memory/remember",
            "claude-code",
            json!({"fact": "per-tool agent test", "context": "test"}),
        ),
    )
    .await;

    let (_, log) = call_json(router, get("/api/log/main?limit=5")).await;
    let commits = log.as_array().unwrap();
    let matching = commits.iter().find(|c| {
        c["intent"]["description"]
            .as_str()
            .map(|d| d.contains("per-tool agent test"))
            .unwrap_or(false)
    });
    assert!(matching.is_some(), "expected the commit to exist");
    assert_eq!(
        matching.unwrap()["agent_id"].as_str().unwrap(),
        "claude-code",
        "agent_id header should override default"
    );
}

#[tokio::test]
async fn forget_records_agent_id_on_rollback_commit() {
    let router = test_router();

    // Write a fact as cursor
    let (_, body) = call_json(
        router.clone(),
        post_with_agent(
            "/api/memory/remember",
            "cursor",
            json!({"fact": "cursor's fact", "context": "test"}),
        ),
    )
    .await;
    let path = body["path"].as_str().unwrap().to_string();

    // Forget it as vscode — rollback should be attributed to vscode,
    // NOT cursor, because agent_id is per-request.
    call_json(
        router.clone(),
        post_with_agent(
            "/api/memory/forget",
            "vs-code",
            json!({"path": path, "reason": "cleanup by vscode"}),
        ),
    )
    .await;

    let (_, log) = call_json(router, get("/api/log/main?limit=10")).await;
    let commits = log.as_array().unwrap();

    // Find the rollback commit
    let rollback = commits
        .iter()
        .find(|c| c["intent"]["category"].as_str() == Some("Rollback"));
    assert!(rollback.is_some(), "expected a Rollback commit");
    assert_eq!(rollback.unwrap()["agent_id"].as_str().unwrap(), "vs-code");

    // And the original write is still attributed to cursor
    let original = commits
        .iter()
        .find(|c| c["intent"]["description"].as_str() == Some("cursor's fact"));
    assert!(original.is_some(), "expected the original commit");
    assert_eq!(original.unwrap()["agent_id"].as_str().unwrap(), "cursor");
}

#[tokio::test]
async fn empty_agent_header_falls_back_to_default() {
    let router = test_router();

    // Empty/whitespace agent header should be ignored, not cause a failure
    call_json(
        router.clone(),
        post_with_agent(
            "/api/memory/remember",
            "   ",
            json!({"fact": "empty agent header test", "context": "test"}),
        ),
    )
    .await;

    let (_, log) = call_json(router, get("/api/log/main?limit=5")).await;
    let commits = log.as_array().unwrap();
    let matching = commits.iter().find(|c| {
        c["intent"]["description"]
            .as_str()
            .map(|d| d.contains("empty agent header test"))
            .unwrap_or(false)
    });
    assert_eq!(matching.unwrap()["agent_id"].as_str().unwrap(), "ctxone");
}

// -------- Token savings metadata --------

#[tokio::test]
async fn recall_response_includes_savings_ratio() {
    let router = test_router();

    // Seed a bunch of facts to make flat-size nontrivial
    for i in 0..10 {
        call_json(
            router.clone(),
            post_json(
                "/api/memory/remember",
                json!({"fact": format!("long-enough fact number {} with some body text", i), "context": "test"}),
            ),
        )
        .await;
    }

    let (_, body) = call_json(router, get("/api/memory/recall?topic=fact&budget=1500")).await;
    assert!(body.get("ctx_tokens_sent").is_some());
    assert!(body.get("ctx_tokens_estimated_flat").is_some());
    assert!(body.get("ctx_savings_ratio").is_some());
}

// Minimal URL encoder for paths containing slashes — keeps the test file
// zero-dependency. Only handles the characters we actually emit.
fn urlencoding(s: &str) -> String {
    s.replace('/', "%2F")
}

// -------- LLM usage capture --------

#[tokio::test]
async fn llm_usage_minimal_body_updates_snapshot() {
    let router = test_router();
    let (status, body) = call_json(
        router,
        post_json(
            "/api/stats/llm_usage",
            json!({
                "input_tokens": 100,
                "output_tokens": 50,
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["session_id"], "default");
    assert_eq!(body["llm_input_tokens"], 100);
    assert_eq!(body["llm_output_tokens"], 50);
    assert_eq!(body["llm_cache_read_tokens"], 0);
    assert_eq!(body["llm_cache_create_tokens"], 0);
    assert_eq!(body["llm_call_count"], 1);
}

#[tokio::test]
async fn llm_usage_full_body_updates_all_fields() {
    let router = test_router();
    let (status, body) = call_json(
        router,
        post_json(
            "/api/stats/llm_usage",
            json!({
                "input_tokens": 2400,
                "output_tokens": 450,
                "cache_read_tokens": 1800,
                "cache_create_tokens": 600,
                "model": "claude-sonnet-4.5",
                "provider": "anthropic",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["llm_input_tokens"], 2400);
    assert_eq!(body["llm_output_tokens"], 450);
    assert_eq!(body["llm_cache_read_tokens"], 1800);
    assert_eq!(body["llm_cache_create_tokens"], 600);
    assert_eq!(body["llm_call_count"], 1);
    assert_eq!(body["last_model"], "claude-sonnet-4.5");
    assert_eq!(body["last_provider"], "anthropic");
}

#[tokio::test]
async fn llm_usage_missing_input_tokens_returns_400() {
    let router = test_router();
    // Missing input_tokens — axum's Json extractor returns 422 for
    // schema violations. Some axum versions return 400. Accept either
    // as "bad input" per the design spec.
    let req = Request::builder()
        .uri("/api/stats/llm_usage")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"output_tokens": 50}"#))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY,
        "expected 400 or 422 for missing input_tokens, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn llm_usage_missing_output_tokens_returns_400() {
    let router = test_router();
    let req = Request::builder()
        .uri("/api/stats/llm_usage")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"input_tokens": 100}"#))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY,
        "expected 400 or 422 for missing output_tokens, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn llm_usage_negative_values_rejected() {
    let router = test_router();
    let req = Request::builder()
        .uri("/api/stats/llm_usage")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"input_tokens": -10, "output_tokens": 5}"#))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    // Negative u64 values fail at JSON parse time — expect 4xx
    assert!(
        resp.status().is_client_error(),
        "expected 4xx for negative tokens, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn llm_usage_accumulates_across_calls() {
    let router = test_router();

    call_json(
        router.clone(),
        post_json(
            "/api/stats/llm_usage",
            json!({"input_tokens": 100, "output_tokens": 50}),
        ),
    )
    .await;

    let (_, body) = call_json(
        router.clone(),
        post_json(
            "/api/stats/llm_usage",
            json!({"input_tokens": 200, "output_tokens": 75}),
        ),
    )
    .await;

    assert_eq!(body["llm_input_tokens"], 300);
    assert_eq!(body["llm_output_tokens"], 125);
    assert_eq!(body["llm_call_count"], 2);
}

#[tokio::test]
async fn llm_usage_respects_session_header() {
    let router = test_router();

    // alice reports usage
    call_json(
        router.clone(),
        post_with_session(
            "/api/stats/llm_usage",
            "alice@example.com",
            json!({"input_tokens": 100, "output_tokens": 50}),
        ),
    )
    .await;

    // bob reports different usage
    call_json(
        router.clone(),
        post_with_session(
            "/api/stats/llm_usage",
            "bob@example.com",
            json!({"input_tokens": 999, "output_tokens": 111}),
        ),
    )
    .await;

    // alice's snapshot shows only her numbers
    let (_, alice) = call_json(router.clone(), get("/api/stats/tokens/alice@example.com")).await;
    assert_eq!(alice["llm_input_tokens"], 100);
    assert_eq!(alice["llm_output_tokens"], 50);
    assert_eq!(alice["llm_call_count"], 1);

    // bob's too
    let (_, bob) = call_json(router.clone(), get("/api/stats/tokens/bob@example.com")).await;
    assert_eq!(bob["llm_input_tokens"], 999);
    assert_eq!(bob["llm_output_tokens"], 111);

    // Aggregate sums both
    let (_, agg) = call_json(router, get("/api/stats/tokens")).await;
    assert_eq!(agg["llm_input_tokens"], 1099);
    assert_eq!(agg["llm_output_tokens"], 161);
    assert_eq!(agg["llm_call_count"], 2);
}

#[tokio::test]
async fn llm_usage_auto_creates_session() {
    let router = test_router();

    // Fresh session ID — never seen before
    call_json(
        router.clone(),
        post_with_session(
            "/api/stats/llm_usage",
            "fresh-session-xyz",
            json!({"input_tokens": 10, "output_tokens": 5}),
        ),
    )
    .await;

    // Session now exists in the registry
    let (status, body) = call_json(router, get("/api/stats/tokens/fresh-session-xyz")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["session_id"], "fresh-session-xyz");
    assert_eq!(body["llm_input_tokens"], 10);
}

#[tokio::test]
async fn recall_response_omits_session_llm_stats_without_report() {
    let router = test_router();

    // Seed a fact to recall
    call_json(
        router.clone(),
        post_json(
            "/api/memory/remember",
            json!({"fact": "recall-no-llm-stats test fact", "context": "test"}),
        ),
    )
    .await;

    let (_, body) = call_json(router, get("/api/memory/recall?topic=recall")).await;
    assert!(
        body.get("session_llm_stats").is_none(),
        "recall shouldn't include session_llm_stats before any usage reported: {}",
        body
    );
}

#[tokio::test]
async fn recall_response_includes_session_llm_stats_after_report() {
    let router = test_router();

    // Seed a fact
    call_json(
        router.clone(),
        post_with_session(
            "/api/memory/remember",
            "alice",
            json!({"fact": "alice recall llm-stats fact", "context": "test"}),
        ),
    )
    .await;

    // alice reports LLM usage
    call_json(
        router.clone(),
        post_with_session(
            "/api/stats/llm_usage",
            "alice",
            json!({
                "input_tokens": 1200,
                "output_tokens": 300,
                "cache_read_tokens": 800,
                "cache_create_tokens": 100,
            }),
        ),
    )
    .await;

    // alice's recall now carries session_llm_stats
    let (_, body) = call_json(
        router,
        get_with_session("/api/memory/recall?topic=recall", "alice"),
    )
    .await;
    let stats = body
        .get("session_llm_stats")
        .expect("session_llm_stats missing");
    assert_eq!(stats["input_tokens_total"], 1200);
    assert_eq!(stats["output_tokens_total"], 300);
    assert_eq!(stats["cache_read_tokens_total"], 800);
    assert_eq!(stats["cache_create_tokens_total"], 100);
    assert_eq!(stats["call_count"], 1);
}

// -------- Pinned list --------

#[tokio::test]
async fn list_pinned_returns_empty_when_none() {
    let router = test_router();
    let (status, body) = call_json(router, get("/api/memory/pinned")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn list_pinned_returns_primed_sections() {
    let router = test_router();

    call_json(
        router.clone(),
        post_json(
            "/api/memory/prime",
            json!({
                "source": "src",
                "pinned": true,
                "sections": [
                    {"title": "A", "body": "body a"},
                    {"title": "B", "body": "body b"},
                ],
            }),
        ),
    )
    .await;

    let (status, body) = call_json(router, get("/api/memory/pinned")).await;
    assert_eq!(status, StatusCode::OK);
    let items = body.as_array().unwrap();
    // Each pinned section has /title and /body leaves = 4 items total
    assert_eq!(items.len(), 4);
}

// -- Projects (namespace registry) --

/// Build a sqlite-backed Hub (the registry needs a real db file) and keep
/// the repo handle so tests can assert on ASG namespaces directly.
fn sqlite_router() -> (tempfile::TempDir, Arc<Repository>, axum::Router) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("hub.db").to_string_lossy().to_string();
    let storage = agentstategraph_storage::SqliteStorage::open(&db_path).expect("sqlite open");
    let repo = Arc::new(Repository::new(Box::new(storage)));
    repo.init().expect("repo init");
    let sessions = Arc::new(SessionRegistry::new());
    let router = http::router_with_db_path(
        repo.clone(),
        sessions,
        http::HubConfig::default(),
        Some(db_path),
        false,
    );
    (dir, repo, router)
}

#[tokio::test]
async fn project_register_creates_namespace_and_round_trips() {
    let (_dir, repo, router) = sqlite_router();

    let (status, body) = call_json(
        router.clone(),
        post_json(
            "/api/projects",
            json!({
                "id": "exampleproj",
                "remote_url": "https://gitlab.example.com/g/exampleproj.git",
                "display_name": "ExampleProj",
                "local_path": "/home/user/example-repo"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register failed: {body}");
    assert_eq!(body["namespace"], "exampleproj"); // defaults to id
    // remote_url is canonical: host/path, no scheme or .git
    assert_eq!(body["remote_url"], "gitlab.example.com/g/exampleproj");

    // The ASG namespace exists now.
    let namespaces = repo.list_namespaces().expect("list namespaces");
    assert!(
        namespaces.iter().any(|n| n.as_str() == "exampleproj"),
        "namespace not created: {namespaces:?}"
    );

    // GET single + list both see it.
    let (status, body) = call_json(router.clone(), get("/api/projects/exampleproj")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["local_paths"], json!(["/home/user/example-repo"]));

    let (status, body) = call_json(router.clone(), get("/api/projects")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn project_duplicate_id_is_conflict() {
    let (_dir, _repo, router) = sqlite_router();
    let req = json!({ "id": "dup" });
    let (status, _) = call_json(router.clone(), post_json("/api/projects", req.clone())).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call_raw(router.clone(), post_json("/api/projects", req)).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn project_invalid_namespace_name_is_bad_request() {
    let (_dir, _repo, router) = sqlite_router();
    // Spaces are invalid in namespace names; validation happens before insert.
    let (status, _) = call_raw(
        router.clone(),
        post_json("/api/projects", json!({ "id": "bad name!" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn project_get_unknown_is_not_found() {
    let (_dir, _repo, router) = sqlite_router();
    let (status, _) = call_raw(router.clone(), get("/api/projects/nope")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn project_add_path_appends() {
    let (_dir, _repo, router) = sqlite_router();
    let (status, _) = call_json(
        router.clone(),
        post_json("/api/projects", json!({ "id": "p1", "local_path": "/a" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call_json(
        router.clone(),
        post_json("/api/projects/p1/paths", json!({ "local_path": "/b" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["local_paths"], json!(["/a", "/b"]));

    // Unknown project → 404, not a silent orphan row.
    let (status, _) = call_raw(
        router.clone(),
        post_json("/api/projects/nope/paths", json!({ "local_path": "/c" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn project_detect_unregistered_cwd_is_not_found() {
    let (_dir, _repo, router) = sqlite_router();
    let tmp = tempfile::tempdir().unwrap();
    let uri = format!(
        "/api/projects/detect?cwd={}",
        urlencoding_encode(tmp.path().to_str().unwrap())
    );
    let (status, body) = call_json(router.clone(), get(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "not_found");
    assert_eq!(body["namespace"], "default");
}

#[tokio::test]
async fn project_detect_finds_ctxproject_file() {
    let (_dir, _repo, router) = sqlite_router();
    let (status, _) = call_json(
        router.clone(),
        post_json("/api/projects", json!({ "id": "found-me" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".ctxproject"), "found-me\n").unwrap();
    let uri = format!(
        "/api/projects/detect?cwd={}",
        urlencoding_encode(tmp.path().to_str().unwrap())
    );
    let (status, body) = call_json(router.clone(), get(&uri)).await;
    assert_eq!(status, StatusCode::OK, "detect failed: {body}");
    assert_eq!(body["status"], "found");
    assert_eq!(body["via"], "ctxproject");
    assert_eq!(body["namespace"], "found-me");
}

/// A `.ctxproject` the hub cannot read must come back as an error, not as
/// `not_found` — the CLI falls back to `default` on `not_found`, and this
/// directory does belong to a project.
#[cfg(unix)]
#[tokio::test]
async fn project_detect_unreadable_marker_is_an_error_not_not_found() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, _repo, router) = sqlite_router();
    let (status, _) = call_json(
        router.clone(),
        post_json("/api/projects", json!({ "id": "hidden" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join(".ctxproject");
    std::fs::write(&marker, "hidden\n").unwrap();
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_to_string(&marker).is_ok() {
        return; // running as root: permissions cannot hide the file
    }
    let uri = format!(
        "/api/projects/detect?cwd={}",
        urlencoding_encode(tmp.path().to_str().unwrap())
    );
    let (status, body) = call_json(router.clone(), get(&uri)).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "body: {body}");
    assert_eq!(body["status"], "error");
    assert!(
        body["error"].as_str().unwrap_or("").contains(".ctxproject"),
        "error should name the unreadable file: {body}"
    );
}

/// Percent-encode a query value (everything but RFC 3986's unreserved set).
fn query_value(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn client_detect_uri(cwd: &str, clues: &[(&str, &str)]) -> String {
    let mut uri = format!(
        "/api/projects/detect?cwd={}&client_read=true",
        query_value(cwd)
    );
    for (k, v) in clues {
        uri.push_str(&format!("&{k}={}", query_value(v)));
    }
    uri
}

/// `ctx` reads the directory and sends what it found; the hub resolves it for
/// a directory that doesn't exist on the hub's machine.
#[tokio::test]
async fn project_detect_resolves_a_clients_clues() {
    let (_dir, _repo, router) = sqlite_router();
    let (status, _) = call_json(
        router.clone(),
        post_json(
            "/api/projects",
            json!({ "id": "remote-proj", "remote_url": "git@git.example.com:g/app.git" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cwd = "/srv/on-another-machine/app";

    let (status, body) = call_json(
        router.clone(),
        get(&client_detect_uri(cwd, &[("ctxproject", "remote-proj")])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "found");
    assert_eq!(body["via"], "ctxproject");
    assert_eq!(body["namespace"], "remote-proj");

    let (status, body) = call_json(
        router.clone(),
        get(&client_detect_uri(
            cwd,
            &[("remote_url", "git@git.example.com:g/app.git")],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["via"], "remote");
    assert_eq!(body["remote_url"], "git.example.com/g/app");

    // The https URL of the same repository finds it too (ctx-hub-sync t-001).
    let (status, body) = call_json(
        router.clone(),
        get(&client_detect_uri(
            cwd,
            &[("remote_url", "https://ci-token@git.example.com/G/app.git/")],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["via"], "remote");
    assert_eq!(body["namespace"], "remote-proj");

    let (status, body) = call_json(router.clone(), get(&client_detect_uri(cwd, &[]))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "not_found");
}

/// With `client_read`, the hub trusts the client's reading and reads nothing
/// itself, even a marker it could see.
#[tokio::test]
async fn project_detect_with_client_read_ignores_the_hubs_filesystem() {
    let (_dir, _repo, router) = sqlite_router();
    let (status, _) = call_json(
        router.clone(),
        post_json("/api/projects", json!({ "id": "on-hub-disk" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".ctxproject"), "on-hub-disk\n").unwrap();
    let cwd = tmp.path().to_str().unwrap();

    // An older client: the hub reads the marker itself.
    let (_, body) = call_json(
        router.clone(),
        get(&format!("/api/projects/detect?cwd={}", query_value(cwd))),
    )
    .await;
    assert_eq!(body["status"], "found", "{body}");

    // A client that read the directory and found nothing.
    let (status, body) = call_json(router.clone(), get(&client_detect_uri(cwd, &[]))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "not_found", "{body}");
}

/// A marker the client couldn't read comes back as the client's error, not
/// as `not_found`.
#[tokio::test]
async fn project_detect_passes_on_a_clients_unreadable_marker() {
    let (_dir, _repo, router) = sqlite_router();
    let (status, body) = call_json(
        router.clone(),
        get(&client_detect_uri(
            "/r",
            &[(
                "ctxproject_error",
                "cannot read /r/.ctxproject: Permission denied",
            )],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["status"], "error");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("/r/.ctxproject"),
        "{body}"
    );
}

#[tokio::test]
async fn projects_require_sqlite_backend() {
    // The default memory-backed test router has no db_path.
    let (status, _) = call_raw(test_router(), get("/api/projects")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Minimal percent-encoder for absolute paths in query strings — test
/// paths only contain [A-Za-z0-9/._-] plus the tempdir's random suffix.
fn urlencoding_encode(s: &str) -> String {
    s.replace('/', "%2F")
}

// -- Namespace threading (X-CTXone-Namespace / ?namespace=) --

fn post_json_ns(uri: &str, ns: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header("x-ctxone-namespace", ns)
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get_ns(uri: &str, ns: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("GET")
        .header("x-ctxone-namespace", ns)
        .body(Body::empty())
        .unwrap()
}

fn put_json_ns(uri: &str, ns: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("PUT")
        .header("content-type", "application/json")
        .header("x-ctxone-namespace", ns)
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get_with_session_ns(uri: &str, session: &str, ns: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("GET")
        .header("x-ctxone-session", session)
        .header("x-ctxone-namespace", ns)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn namespaces_isolate_memory_and_branches() {
    let (_dir, _repo, router) = sqlite_router();

    // Two projects → two namespaces (each gets an initialized main).
    for id in ["repo-a", "repo-b"] {
        let (status, body) = call_json(
            router.clone(),
            post_json("/api/projects", json!({ "id": id })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "register {id} failed: {body}");
    }

    // Write a fact in repo-a's namespace.
    let (status, body) = call_json(
        router.clone(),
        post_json_ns(
            "/api/memory/remember",
            "repo-a",
            json!({ "fact": "the scheduler runs on tokio", "importance": "high" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "remember failed: {body}");

    // Visible in repo-a...
    let (status, body) = call_json(
        router.clone(),
        get_ns("/api/state/main/search?query=scheduler", "repo-a"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.as_array().unwrap().is_empty(),
        "fact not found in its own namespace"
    );

    // ...invisible in repo-b and in default.
    let (status, body) = call_json(
        router.clone(),
        get_ns("/api/state/main/search?query=scheduler", "repo-b"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.as_array().unwrap().is_empty(),
        "namespace leak into repo-b: {body}"
    );
    let (status, body) = call_json(
        router.clone(),
        get("/api/state/main/search?query=scheduler"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.as_array().unwrap().is_empty(),
        "namespace leak into default: {body}"
    );

    // Branches are per-namespace: create one in repo-a, absent in repo-b.
    let (status, _) = call_json(
        router.clone(),
        post_json_ns("/api/branches", "repo-a", json!({ "name": "feature-x" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let branches = |body: &Value| -> Vec<String> {
        body.as_array()
            .unwrap()
            .iter()
            .map(|b| b["name"].as_str().unwrap().to_string())
            .collect()
    };
    let (_, body) = call_json(router.clone(), get_ns("/api/branches", "repo-a")).await;
    assert!(branches(&body).contains(&"feature-x".to_string()));
    let (_, body) = call_json(router.clone(), get_ns("/api/branches", "repo-b")).await;
    assert!(
        !branches(&body).contains(&"feature-x".to_string()),
        "branch leaked across namespaces"
    );
}

// -------- The personal namespace (ctx-hub-sync t-003) --------

/// A project namespace on `router`'s Hub.
async fn register(router: &axum::Router, id: &str) {
    let (status, body) = call_json(
        router.clone(),
        post_json("/api/projects", json!({ "id": id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register {id} failed: {body}");
}

#[tokio::test]
async fn personal_memory_reaches_every_workspaces_recall() {
    let (_dir, _repo, router) = sqlite_router();
    register(&router, "repo-a").await;
    register(&router, "repo-b").await;

    // `ctx remember --personal` names the personal namespace; the first
    // request creates it.
    let (status, body) = call_json(
        router.clone(),
        post_json(
            "/api/memory/remember?namespace=_personal",
            json!({ "fact": "I review diffs before merging", "context": "preferences" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "remember failed: {body}");
    let path = body["path"].as_str().unwrap().to_string();

    for ns in ["repo-a", "repo-b", "default"] {
        let (status, body) = call_json(
            router.clone(),
            get_ns("/api/memory/recall?topic=review%20diffs", ns),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let hit = body["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == path.as_str())
            .unwrap_or_else(|| panic!("{ns} didn't recall it: {body}"))
            .clone();
        assert_eq!(hit["namespace"], "_personal");
    }

    // A workspace's own memory stays its own.
    let (status, _) = call_json(
        router.clone(),
        post_json_ns(
            "/api/memory/remember",
            "repo-a",
            json!({ "fact": "repo-a reviews in pairs" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = call_json(
        router.clone(),
        get_ns("/api/memory/recall?topic=reviews%20in%20pairs", "repo-b"),
    )
    .await;
    assert!(
        !body["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["value"] == "repo-a reviews in pairs"),
        "repo-b recalled repo-a's memory: {body}"
    );
}

#[tokio::test]
async fn personal_pinned_rules_show_beside_a_workspaces_own() {
    let (_dir, _repo, router) = sqlite_router();
    register(&router, "repo-a").await;

    // `ctx agents install` primes the personal namespace.
    let (status, _) = call_json(
        router.clone(),
        post_json(
            "/api/memory/prime?namespace=_personal",
            json!({
                "source": "ctxone-agents",
                "pinned": true,
                "sections": [
                    {"title": "Use recall", "body": "Recall before substantial work"},
                    {"title": "House style", "body": "my own style"},
                ],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // The workspace pins its own house style at the same path.
    let (status, _) = call_json(
        router.clone(),
        post_json_ns(
            "/api/memory/prime",
            "repo-a",
            json!({
                "source": "ctxone-agents",
                "pinned": true,
                "sections": [{"title": "House style", "body": "the project's style"}],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call_json(
        router.clone(),
        get_ns("/api/memory/recall?topic=unrelated-xyz", "repo-a"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pinned: Vec<&Value> = body["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["pinned"] == true)
        .collect();
    assert_eq!(pinned.len(), 2, "{body}");
    let style = pinned
        .iter()
        .find(|r| r["path"] == "/memory/pinned/ctxone-agents/house-style")
        .unwrap();
    assert_eq!(style["body"], "the project's style");
    assert_eq!(style["namespace"], "repo-a");
    let rule = pinned
        .iter()
        .find(|r| r["path"] == "/memory/pinned/ctxone-agents/use-recall")
        .unwrap();
    assert_eq!(rule["namespace"], "_personal");

    // Lens's pinned list says where each leaf came from, and drops the
    // personal copy the workspace overrides.
    let (status, body) = call_json(router.clone(), get_ns("/api/memory/pinned", "repo-a")).await;
    assert_eq!(status, StatusCode::OK);
    let leaves = body.as_array().unwrap();
    let ns_of = |path: &str| -> Vec<String> {
        leaves
            .iter()
            .filter(|l| l["path"] == path)
            .map(|l| l["namespace"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        ns_of("/memory/pinned/ctxone-agents/house-style/body"),
        ["repo-a"]
    );
    assert_eq!(
        ns_of("/memory/pinned/ctxone-agents/use-recall/body"),
        ["_personal"]
    );
}

#[tokio::test]
async fn a_registered_project_is_recorded_in_the_personal_namespace() {
    let (_dir, _repo, router) = sqlite_router();
    let (status, body) = call_json(
        router.clone(),
        post_json(
            "/api/projects",
            json!({
                "id": "github-com-a-app",
                "remote_url": "git@github.com:a/app.git",
                "display_name": "App",
                "local_path": "/home/me/app",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, record) = call_json(
        router,
        get("/api/state/main?path=/ctxone/projects/github-com-a-app&namespace=_personal"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{record}");
    assert_eq!(
        record,
        json!({
            "namespace": "github-com-a-app",
            "remote_url": "github.com/a/app",
            "display_name": "App",
        })
    );
}

#[tokio::test]
async fn names_beginning_with_an_underscore_are_reserved() {
    let (_dir, _repo, router) = sqlite_router();
    for body in [
        json!({ "id": "_personal" }),
        json!({ "id": "_mine" }),
        json!({ "id": "app", "namespace": "_personal" }),
    ] {
        let (status, text) = call_raw(router.clone(), post_json("/api/projects", body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        assert!(text.contains("reserved"), "{text}");
    }
    let (status, body) = call_json(router, get("/api/projects")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.as_array().unwrap().is_empty(), "{body}");
}

#[tokio::test]
async fn session_listing_is_scoped_to_its_workspace() {
    let (_dir, _repo, router) = sqlite_router();
    for id in ["repo-a", "repo-b"] {
        let (status, _) = call_json(
            router.clone(),
            post_json("/api/projects", json!({ "id": id })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // A session ingested into repo-a: a graph node (title) AND a registry
    // entry, which is what an ingest produces — turns record token usage. The
    // list is registry-driven and graph-filtered, so both are needed, exactly
    // as in production.
    call_json(
        router.clone(),
        get_with_session_ns("/api/memory/recall?topic=x", "sess-a", "repo-a"),
    )
    .await;
    let (status, _) = call_json(
        router.clone(),
        put_json_ns("/api/sessions/sess-a/title", "repo-a", json!("Work in A")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A registry-only session: stats via a session header, but never ingested,
    // so it has no node in any namespace.
    call_json(
        router.clone(),
        get_with_session("/api/memory/recall?topic=x", "ghost-session"),
    )
    .await;

    let ids = |body: &Value| -> Vec<String> {
        body.as_array()
            .unwrap()
            .iter()
            .map(|s| s["session_id"].as_str().unwrap().to_string())
            .collect()
    };

    // repo-a shows its session and NOT the registry-only ghost.
    let (_, body) = call_json(router.clone(), get_ns("/api/stats/sessions", "repo-a")).await;
    let a = ids(&body);
    assert!(
        a.contains(&"sess-a".to_string()),
        "repo-a missing its session: {a:?}"
    );
    assert!(
        !a.contains(&"ghost-session".to_string()),
        "ghost leaked into repo-a"
    );

    // repo-b shows neither.
    let (_, body) = call_json(router.clone(), get_ns("/api/stats/sessions", "repo-b")).await;
    let b = ids(&body);
    assert!(
        !b.contains(&"sess-a".to_string()),
        "sess-a leaked into repo-b"
    );
    assert!(
        !b.contains(&"ghost-session".to_string()),
        "ghost leaked into repo-b"
    );

    // default is the catch-all: the registry-only ghost lands here, the placed
    // session does not.
    let (_, body) = call_json(router.clone(), get("/api/stats/sessions")).await;
    let d = ids(&body);
    assert!(
        d.contains(&"ghost-session".to_string()),
        "ghost missing from default: {d:?}"
    );
    assert!(
        !d.contains(&"sess-a".to_string()),
        "placed session leaked into default"
    );
}

#[tokio::test]
async fn plan_relocate_moves_the_plan_and_its_tasks() {
    let (_dir, _repo, router) = sqlite_router();
    for id in ["repo-a", "repo-b"] {
        let (s, _) = call_json(
            router.clone(),
            post_json("/api/projects", json!({ "id": id })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }

    // A plan with a task in repo-a.
    let (s, _) = call_json(
        router.clone(),
        post_json_ns("/api/plans", "repo-a", json!({ "name": "migrate-me" })),
    )
    .await;
    assert!(s.is_success(), "plan create failed: {s}");
    let (s, _) = call_json(
        router.clone(),
        post_json_ns(
            "/api/plans/migrate-me/tasks",
            "repo-a",
            json!({ "title": "do the thing" }),
        ),
    )
    .await;
    assert!(s.is_success(), "task add failed: {s}");

    // Relocate it to repo-b.
    let (s, body) = call_json(
        router.clone(),
        post_json_ns(
            "/api/plans/migrate-me/relocate",
            "repo-a",
            json!({ "to_namespace": "repo-b" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "relocate failed: {body}");
    assert_eq!(body["wrote_target"], json!(true));

    // Present in repo-b, WITH its task. Use list_plans (a child scan) and
    // call_raw so an error body is inspected rather than panicking.
    let (st, body) = call_raw(router.clone(), get_ns("/api/plans/migrate-me", "repo-b")).await;
    assert_eq!(st, StatusCode::OK, "plan not in repo-b after move: {body}");
    assert!(body.contains("migrate-me"));
    assert!(body.contains("do the thing"), "task did not travel: {body}");

    // Gone from repo-a.
    let (st, _) = call_raw(router.clone(), get_ns("/api/plans/migrate-me", "repo-a")).await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "original not removed from source"
    );

    // Same-namespace move is rejected, not a silent no-op.
    let (st, _) = call_raw(
        router.clone(),
        post_json_ns(
            "/api/plans/migrate-me/relocate",
            "repo-b",
            json!({ "to_namespace": "repo-b" }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn namespace_query_param_overrides_header() {
    let (_dir, _repo, router) = sqlite_router();
    let (status, _) = call_json(
        router.clone(),
        post_json("/api/projects", json!({ "id": "qp-ns" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Header says a namespace that doesn't exist; query param wins.
    let req = Request::builder()
        .uri("/api/branches?namespace=qp-ns")
        .method("GET")
        .header("x-ctxone-namespace", "nope")
        .body(Body::empty())
        .unwrap();
    let (status, _) = call_json(router.clone(), req).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn unknown_namespace_is_not_found() {
    let (_dir, _repo, router) = sqlite_router();
    let (status, _) = call_raw(router.clone(), get_ns("/api/branches", "never-registered")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invalid_namespace_name_is_bad_request() {
    let (_dir, _repo, router) = sqlite_router();
    let (status, _) = call_raw(router.clone(), get_ns("/api/branches", "bad name!")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn bulk_turn_write_accepts_multi_megabyte_payload() {
    // Regression: a whole-session bulk turn write legitimately runs to many MB,
    // and Axum's 2 MB default body limit silently 413'd it — dropping a large
    // session's turns while its meta (with fingerprint) still wrote, so a
    // re-sync then skipped it. The raised DefaultBodyLimit must let it through.
    let router = test_router();
    let mut map = serde_json::Map::new();
    for i in 0..1500 {
        map.insert(
            format!("t{i:04}"),
            json!({ "idx": i, "user": "x".repeat(300), "assistant": "y".repeat(2000) }),
        );
    }
    let raw = serde_json::to_vec(&Value::Object(map)).unwrap();
    assert!(
        raw.len() > 2 * 1024 * 1024,
        "payload should exceed the old 2 MB limit, got {} bytes",
        raw.len()
    );
    let req = Request::builder()
        .uri("/api/sessions/bigsess/turns?ref=main")
        .method("PUT")
        .header("content-type", "application/json")
        .body(Body::from(raw))
        .unwrap();
    let (status, v) = call_json(router, req).await;
    assert_eq!(status, StatusCode::OK, "large bulk turn write must not 413");
    assert_eq!(v["turns"], 1500);
}

// -------- Session usage rollup (ctx-hub-sync t-005) --------

fn put_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("PUT")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn opus_usage(input: u64) -> Value {
    json!({
        "by_model": {
            "claude-opus-4-8": {
                "input_tokens": input,
                "output_tokens": 40,
                "cache_read_tokens": 10,
                "cache_create_tokens": 5,
                "call_count": 2
            }
        },
        "last_model": "claude-opus-4-8"
    })
}

#[tokio::test]
async fn session_usage_replaces_the_split_and_lands_in_the_envelope() {
    let router = test_router();
    let (status, body) = call_json(
        router.clone(),
        put_json("/api/sessions/s1/usage", opus_usage(100)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok");

    let (_, stats) = call_json(router.clone(), get("/api/stats/tokens/s1")).await;
    assert_eq!(stats["llm_input_tokens"], 100);
    assert_eq!(stats["llm_call_count"], 2);
    assert_eq!(stats["last_model"], "claude-opus-4-8");
    assert_eq!(
        stats["llm_by_model"]["claude-opus-4-8"]["cache_create_tokens"],
        5
    );

    let (_, envelope) = call_json(
        router.clone(),
        get("/api/state/main?path=/sessions/s1/usage"),
    )
    .await;
    assert_eq!(
        envelope["by_model"]["claude-opus-4-8"]["input_tokens"], 100,
        "{envelope}"
    );
    assert_eq!(envelope["last_model"], "claude-opus-4-8");
    assert_eq!(envelope["tokens_sent"], 0);

    // The same rollup again writes nothing.
    let (status, body) = call_json(
        router.clone(),
        put_json("/api/sessions/s1/usage", opus_usage(100)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "unchanged");

    // A grown session replaces the split rather than adding to it.
    call_json(
        router.clone(),
        put_json("/api/sessions/s1/usage", opus_usage(300)),
    )
    .await;
    let (_, stats) = call_json(router.clone(), get("/api/stats/tokens/s1")).await;
    assert_eq!(stats["llm_input_tokens"], 300);
    assert_eq!(stats["llm_call_count"], 2);
}

#[tokio::test]
async fn session_usage_refuses_an_empty_model_name() {
    let router = test_router();
    let body = json!({ "by_model": { "": { "input_tokens": 1 } } });
    let (status, _) = call_raw(router, put_json("/api/sessions/s1/usage", body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// A Hub with none of a session's statistics, as after an import, still
/// lists the session with its usage and costs its plan, from the rollup.
#[tokio::test]
async fn plan_cost_and_session_list_read_the_rollup_without_statistics() {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let ingested = http::router(repo.clone(), Arc::new(SessionRegistry::new()));
    for (uri, body) in [
        ("/api/sessions/s9/title", json!("Imported work")),
        ("/api/sessions/s9/usage", opus_usage(100)),
    ] {
        let (status, _) = call_json(ingested.clone(), put_json(uri, body)).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }
    let (status, _) = call_json(
        ingested,
        post_json(
            "/api/sessions/s9/links",
            json!({ "plan": "feat-x", "task": "t-001" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let fresh = http::router(repo, Arc::new(SessionRegistry::new()));
    let (_, cost) = call_json(fresh.clone(), get("/api/stats/plan/feat-x/cost")).await;
    assert_eq!(cost["session_count"], 1, "{cost}");
    assert_eq!(cost["totals"]["llm_input_tokens"], 100);
    assert_eq!(cost["sessions"][0]["last_model"], "claude-opus-4-8");

    let (_, list) = call_json(fresh, get("/api/stats/sessions")).await;
    let s9 = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == "s9")
        .unwrap_or_else(|| panic!("s9 missing from {list}"));
    assert_eq!(s9["llm_input_tokens"], 100);
    assert_eq!(s9["name"], "Imported work");
}

// ── ctx secrets scan (ctx-hub-sync t-017) ──────────────────────────────

/// A GitHub-token-shaped string, built at run time so this file holds none.
fn fake_github_token() -> String {
    let body: String = "a1B2c3D4e5F6g7H8i9J0".chars().cycle().take(36).collect();
    format!("ghp_{body}")
}

#[tokio::test]
async fn a_secrets_scan_finds_keys_in_the_workspace_and_personal_and_masks_them() {
    let (_dir, _repo, router) = sqlite_router();
    register(&router, "repo-a").await;
    let token = fake_github_token();
    let aws = format!("AKIA{}", "Q7ZR4M2KXW9PLT3B");

    let (status, body) = call_json(
        router.clone(),
        post_json_ns(
            "/api/memory/remember",
            "repo-a",
            json!({ "fact": format!("CI pushes with {token}"), "context": "ci" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "remember failed: {body}");
    let (status, body) = call_json(
        router.clone(),
        post_json(
            "/api/memory/remember?namespace=_personal",
            json!({ "fact": format!("my AWS key is {aws}") }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "remember failed: {body}");

    let (status, body) = call_json(router.clone(), get_ns("/api/secrets/scan", "repo-a")).await;
    assert_eq!(status, StatusCode::OK, "scan failed: {body}");
    let text = body.to_string();
    assert!(
        !text.contains(&token[4..]) && !text.contains(&aws[4..]),
        "a secret came back: {body}"
    );

    let findings = body["findings"].as_array().unwrap();
    let by_rule = |rule: &str| {
        findings
            .iter()
            .find(|f| f["rule"] == rule)
            .unwrap_or_else(|| panic!("no {rule} finding: {body}"))
    };
    let gh = by_rule("github-token");
    assert_eq!(gh["namespace"], "repo-a");
    assert_eq!(gh["ref"], "main");
    assert_eq!(gh["confidence"], "high");
    assert_eq!(gh["masked"], "ghp_…(40 chars)");
    let forget = gh["forget"].as_str().unwrap();
    assert!(gh["path"].as_str().unwrap().starts_with(forget), "{gh}");
    assert_eq!(by_rule("aws-access-key")["namespace"], "_personal");
    assert_eq!(findings.len(), 2, "{body}");
    assert_eq!(body["turns"], false);
    assert_eq!(body["truncated"], false);

    // The finding's forget path removes it.
    let (status, _) = call_json(
        router.clone(),
        post_json_ns("/api/memory/forget", "repo-a", json!({ "path": forget })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = call_json(router.clone(), get_ns("/api/secrets/scan", "repo-a")).await;
    assert!(
        !body["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["rule"] == "github-token"),
        "still found after forget: {body}"
    );
}

#[tokio::test]
async fn a_secrets_scan_covers_every_workspace_with_all_and_creates_no_namespace() {
    let (_dir, _repo, router) = sqlite_router();
    // A scan reads the personal namespace beside, and doesn't create it.
    // (Registering a project would: it records the project there.)
    let (status, body) = call_json(router.clone(), get("/api/secrets/scan")).await;
    assert_eq!(status, StatusCode::OK, "scan failed: {body}");
    let (_, namespaces) = call_json(router.clone(), get("/api/namespaces")).await;
    assert!(
        !namespaces.to_string().contains("_personal"),
        "{namespaces}"
    );

    register(&router, "repo-a").await;
    register(&router, "repo-b").await;
    let token = fake_github_token();
    let (status, _) = call_json(
        router.clone(),
        post_json_ns(
            "/api/memory/remember",
            "repo-b",
            json!({ "fact": format!("deploys use {token}") }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // repo-a's own scan doesn't see repo-b.
    let (status, body) = call_json(router.clone(), get_ns("/api/secrets/scan", "repo-a")).await;
    assert_eq!(status, StatusCode::OK, "scan failed: {body}");
    assert!(body["findings"].as_array().unwrap().is_empty(), "{body}");

    let (status, body) = call_json(router.clone(), get("/api/secrets/scan?all=true")).await;
    assert_eq!(status, StatusCode::OK, "scan failed: {body}");
    let findings = body["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{body}");
    assert_eq!(findings[0]["namespace"], "repo-b");
    let scanned = body["scanned"].as_array().unwrap();
    for ns in ["default", "repo-a", "repo-b"] {
        assert!(
            scanned.iter().any(|s| s["namespace"] == ns),
            "{ns} not scanned: {body}"
        );
    }

    // One branch only.
    let (_, body) = call_json(router.clone(), get("/api/secrets/scan?all=true&ref=nope")).await;
    assert!(body["scanned"].as_array().unwrap().is_empty(), "{body}");
}

// -------- Raw turns on their own local-only ref (ctx-hub-sync t-002) --------

fn put_with_session(uri: &str, session: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("PUT")
        .header("content-type", "application/json")
        .header("x-ctxone-session", session)
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn turns_live_on_the_turns_ref_and_never_on_a_branch() {
    let (_dir, repo, router) = sqlite_router();
    let turns = json!({
        "t0000": { "text": "hello", "cwd": "/work/app" },
        "t0001": { "text": "again" },
    });
    let (status, v) = call_json(
        router.clone(),
        put_json("/api/sessions/s1/turns?ref=main", turns),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["ref"], "_local/turns");
    let (status, _) = call_json(
        router.clone(),
        post_json(
            "/api/sessions/s1/turns/2?ref=main",
            json!({ "text": "third" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call_json(
        router.clone(),
        put_with_session("/api/sessions/s1/title", "s1", json!("One")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Usage puts the session in the registry, which the session list reads.
    let (status, _) = call_json(
        router.clone(),
        post_with_session(
            "/api/stats/llm_usage",
            "s1",
            json!({ "input_tokens": 10, "output_tokens": 5 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The envelope is on main; the turns aren't, and their ref is local-only.
    assert!(repo.get("main", "/sessions/s1/turns").is_err());
    assert_eq!(repo.get_json("main", "/sessions/s1/title").unwrap(), "One");
    assert!(repo.is_local_only("_local/turns").unwrap());

    // Every reader finds them.
    let (_, one) = call_json(router.clone(), get("/api/sessions/s1/turns/0")).await;
    assert_eq!(one["cwd"], "/work/app");
    let (_, paths) = call_json(router.clone(), get("/api/sessions/s1/turns")).await;
    assert_eq!(
        paths,
        json!([
            "/sessions/s1/turns/t0000",
            "/sessions/s1/turns/t0001",
            "/sessions/s1/turns/t0002"
        ])
    );
    let (_, tree) = call_json(router.clone(), get("/api/sessions/s1/turns?tree=true")).await;
    assert_eq!(tree["t0002"]["text"], "third");
    let (status, _) = call_json(router.clone(), get("/api/sessions/s1/segments")).await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = call_json(router.clone(), get("/api/stats/sessions")).await;
    let s1 = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == "s1")
        .unwrap_or_else(|| panic!("s1 isn't listed: {list}"));
    assert_eq!(s1["has_turns"], true, "{s1}");

    // Branch tools don't list the turns ref, and refuse it by name.
    let (_, branches) = call_json(router.clone(), get("/api/branches")).await;
    assert!(
        branches
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["name"] != "_local/turns"),
        "{branches}"
    );
    for (uri, body) in [
        (
            "/api/merge",
            json!({ "source": "_local/turns", "target": "main" }),
        ),
        (
            "/api/branches",
            json!({ "name": "_local/x", "from": "main" }),
        ),
        (
            "/api/branches",
            json!({ "name": "copy", "from": "_local/turns" }),
        ),
        (
            "/api/branches/reset",
            json!({ "name": "main", "to": "_local/turns" }),
        ),
    ] {
        let (status, body) = call_raw(router.clone(), post_json(uri, body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert!(body.contains("local-only"), "{uri}: {body}");
    }

    // A secrets scan reads the turns ref only when turns are asked for.
    let scanned = |scan: &Value| -> Vec<String> {
        scan["scanned"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["ref"].as_str().unwrap().to_string())
            .collect()
    };
    let (_, scan) = call_json(router.clone(), get("/api/secrets/scan")).await;
    assert_eq!(scanned(&scan), vec!["main"], "{scan}");
    let (_, scan) = call_json(router.clone(), get("/api/secrets/scan?turns=true")).await;
    assert!(
        scanned(&scan).contains(&"_local/turns".to_string()),
        "{scan}"
    );

    // A move takes the turns along, relinked.
    repo.create_namespace("proj").unwrap();
    repo.fork_namespace(agentstategraph_core::Namespace::new("proj").unwrap())
        .init()
        .unwrap();
    let (status, moved) = call_json(
        router.clone(),
        post_json("/api/sessions/s1/move", json!({ "to_namespace": "proj" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["turns_moved"], true);
    let (_, here) = call_json(router.clone(), get("/api/sessions/s1/turns?tree=true")).await;
    assert!(here.is_null(), "{here}");
    let (_, there) = call_json(
        router.clone(),
        get_ns("/api/sessions/s1/turns?tree=true", "proj"),
    )
    .await;
    assert_eq!(there["t0000"]["text"], "hello");

    // A delete takes them with the envelope.
    let req = Request::builder()
        .uri("/api/sessions/s1")
        .method("DELETE")
        .header("x-ctxone-namespace", "proj")
        .body(Body::empty())
        .unwrap();
    let (status, deleted) = call_json(router.clone(), req).await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(deleted["turns_removed"], 4, "{deleted}");
    let (_, gone) = call_json(
        router.clone(),
        get_ns("/api/sessions/s1/turns?tree=true", "proj"),
    )
    .await;
    assert!(gone.is_null(), "{gone}");
}

#[tokio::test]
async fn a_hub_moves_turns_off_its_branches_when_it_upgrades() {
    // A store as schema 2 left it: turns in the envelope, on main.
    let (_dir, repo, _router) = sqlite_router();
    let opts = || {
        agentstategraph::CommitOptions::new(
            "test",
            agentstategraph_core::IntentCategory::Checkpoint,
            "seed",
        )
    };
    repo.set_json(
        "main",
        "/sessions/s1",
        &json!({ "title": "One", "turns": { "t0000": { "text": "hi" } } }),
        opts(),
    )
    .unwrap();
    repo.set_json("main", "/ctxone/schema_version", &json!(2), opts())
        .unwrap();
    ctxone_hub::migrations::run_migrations(&repo).unwrap();
    assert_eq!(
        ctxone_hub::migrations::current_schema_version(&repo),
        ctxone_hub::migrations::CTXONE_SCHEMA_VERSION
    );
    assert!(repo.get("main", "/sessions/s1/turns").is_err());
    assert_eq!(repo.get_json("main", "/sessions/s1/title").unwrap(), "One");
    assert_eq!(
        repo.get_json("_local/turns", "/turns/s1/t0000/text")
            .unwrap(),
        "hi"
    );
}

// -------- Reminders run only where they were made or approved (ctx-hub-sync t-008) --------

fn memory_router() -> (Arc<Repository>, axum::Router) {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let router = http::router(repo.clone(), Arc::new(SessionRegistry::new()));
    (repo, router)
}

fn find<'a>(list: &'a Value, id: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("{id} isn't listed: {list}"))
}

#[tokio::test]
async fn a_pulled_reminder_waits_for_approval_on_this_machine() {
    use agentstategraph::sync::InProcessRemote;
    let (a, a_router) = memory_router();
    let (b, b_router) = memory_router();
    let hub = Repository::new(Box::new(MemoryStorage::new()));
    hub.init().unwrap();
    let remote = InProcessRemote::new(&hub);

    // Made through A's Hub: due now, autonomous, with a command.
    let (status, made) = call_json(
        a_router.clone(),
        post_json(
            "/api/reminders",
            json!({
                "title": "rotate logs",
                "instructions": "run the rotation",
                "due_at": "2020-01-01T00:00:00Z",
                "autonomous": true,
                "commands": ["true"],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["trusted_here"], true);
    let id = made["id"].as_str().unwrap().to_string();
    let (_, due) = call_json(a_router.clone(), get("/api/reminders/due")).await;
    assert_eq!(find(&due, &id)["trusted_here"], true);

    // A pushes; the control ref carries the reminder. B pulls it.
    a.fetch("hub", &remote, &[]).unwrap();
    a.push("hub", &remote, "main").unwrap();
    b.fetch("hub", &remote, &[]).unwrap();
    let (_, due) = call_json(b_router.clone(), get("/api/reminders/due")).await;
    let pulled = find(&due, &id);
    assert_eq!(pulled["title"], "rotate logs");
    assert_eq!(pulled["trusted_here"], false, "{pulled}");
    assert!(
        b.head("_local/trust").is_err(),
        "nothing is trusted on B yet"
    );

    // Approved on B, it may run there; A's own record never travelled.
    let (status, approved) = call_json(
        b_router.clone(),
        post_json(&format!("/api/reminders/{id}/approve"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["trusted_here"], true);
    let (_, due) = call_json(b_router.clone(), get("/api/reminders/due")).await;
    assert_eq!(find(&due, &id)["trusted_here"], true);
    assert!(hub.head("_local/trust").is_err());

    // A reminder that isn't due yet can be approved ahead; it stays pending.
    let (_, later) = call_json(
        a_router.clone(),
        post_json(
            "/api/reminders",
            json!({
                "title": "later",
                "instructions": "-",
                "due_at": "2999-01-01T00:00:00Z",
                "autonomous": true,
            }),
        ),
    )
    .await;
    let later_id = later["id"].as_str().unwrap().to_string();
    a.push("hub", &remote, "main").unwrap();
    b.fetch("hub", &remote, &[]).unwrap();
    let (status, ahead) = call_json(
        b_router.clone(),
        post_json(&format!("/api/reminders/{later_id}/approve"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ahead}");
    assert_eq!(ahead["status"], "pending");
    assert_eq!(ahead["trusted_here"], true);
}
