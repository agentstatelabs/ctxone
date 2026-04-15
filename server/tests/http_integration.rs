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
use ctxone_hub::{http, memory_tools::SessionStats};

/// Build a fresh in-memory Hub + router for each test.
fn test_router() -> axum::Router {
    let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
    repo.init().expect("repo init");
    let session = Arc::new(SessionStats::new());
    http::router(repo, session)
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

    let (_, decisions) = call_json(router, get("/api/state/main?path=/sessions/abc123/decisions")).await;
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
    let (status, body) = call_json(router, get("/api/memory/why_did_we?decision=nonexistent-xyz")).await;
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
async fn create_branch_from_missing_ref_returns_500() {
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
        StatusCode::INTERNAL_SERVER_ERROR,
        "branching from missing ref should fail"
    );
    // Error body should mention the missing ref so users can diagnose
    assert!(
        body.contains("ghost-ref") || body.contains("not found"),
        "expected descriptive error, got: {}",
        body
    );
}

#[tokio::test]
async fn remember_to_missing_branch_returns_500() {
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
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
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
