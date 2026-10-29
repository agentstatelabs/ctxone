//! A single-tenant Hub on SQLite keeps how each MCP session began in its
//! file, so a session continues after the Hub restarts, only in the workspace
//! and statistics session it began in. Each restart here is a new router on
//! the same file, with its own repository handle, statistics and MCP
//! services, as a new `ctxone-hub` process has.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use agentstategraph::Repository;
use agentstategraph_storage::SqliteStorage;
use ctxone_hub::mcp_sessions::TTL_DAYS;
use ctxone_hub::{http, memory_tools::SessionRegistry};

/// The workspace the sessions here begin in.
const MCP: &str = "/mcp?namespace=web";
/// The statistics session (`X-CTXone-Session`) they begin in.
const STATS: &str = "itest-session";

/// A Hub's SQLite file, which outlives each Hub started on it.
struct HubFile {
    dir: tempfile::TempDir,
}

impl HubFile {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> String {
        self.dir
            .path()
            .join("memory.db")
            .to_string_lossy()
            .into_owned()
    }

    /// Start a Hub on the file, as `ctxone-hub --http --path <file>` does.
    fn start(&self) -> axum::Router {
        let path = self.path();
        let repo = Arc::new(Repository::new(Box::new(
            SqliteStorage::open(&path).expect("sqlite open"),
        )));
        repo.init().expect("repo init");
        let sessions = Arc::new(SessionRegistry::load_from_db(&path));
        let config = http::HubConfig {
            mcp_http: true,
            agent_id: "itest".into(),
            ..Default::default()
        };
        http::router_with_db_path(repo, sessions, config, Some(path), false)
    }

    /// A connection of the test's own.
    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.path()).unwrap()
    }

    /// Whether MCP session `id` is kept in the file.
    fn kept(&self, id: &str) -> bool {
        self.db()
            .query_row(
                "SELECT count(*) FROM ctxone_mcp_sessions WHERE session_id = ?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            > 0
    }
}

/// An MCP request to `uri` for statistics session `stats`, in MCP session
/// `session` if there is one: a POST of `body`, or a DELETE without one.
fn mcp(uri: &str, stats: &str, session: Option<&str>, body: Option<&Value>) -> Request<Body> {
    let mut req = Request::builder()
        .uri(uri)
        .method(if body.is_some() { "POST" } else { "DELETE" })
        // rmcp only lets loopback Host headers through by default.
        .header("host", "localhost")
        .header("accept", "application/json, text/event-stream")
        .header("x-ctxone-session", stats);
    if body.is_some() {
        req = req.header("content-type", "application/json");
    }
    if let Some(id) = session {
        req = req.header("mcp-session-id", id);
    }
    req.body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
        .unwrap()
}

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = app.clone().oneshot(req).await.expect("router call");
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Start an MCP session at `uri` for statistics session `stats` on `app`,
/// and return its id.
async fn mcp_session(app: &axum::Router, uri: &str, stats: &str) -> String {
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "itest", "version": "0" }
        }
    });
    let resp = app
        .clone()
        .oneshot(mcp(uri, stats, None, Some(&init)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "initialize");
    let id = resp.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    resp.into_body().collect().await.unwrap();
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let (status, body) = call(app, mcp(uri, stats, Some(&id), Some(&initialized))).await;
    assert!(status.is_success(), "{status}: {body}");
    id
}

/// The JSON-RPC response in an MCP answer's SSE `data:` frames.
fn answer(body: &str) -> Value {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find(|m| m.get("result").is_some() || m.get("error").is_some())
        .unwrap_or_else(|| panic!("no JSON-RPC response in {body}"))
}

/// List the tools of MCP session `id` at `uri`, for statistics session
/// `stats`, on `app`.
async fn list_tools(app: &axum::Router, uri: &str, stats: &str, id: &str) -> (StatusCode, String) {
    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
    call(app, mcp(uri, stats, Some(id), Some(&list))).await
}

#[tokio::test]
async fn an_mcp_session_survives_a_restart_of_its_hub() {
    let file = HubFile::new();
    let before = file.start();
    let id = mcp_session(&before, MCP, STATS).await;
    assert!(file.kept(&id));
    drop(before);

    // The restarted Hub has never seen the session, and continues it.
    let after = file.start();
    let (status, body) = list_tools(&after, MCP, STATS, &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        answer(&body)["result"]["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|t| t["name"] == "remember")),
        "{body}"
    );
    // In the workspace it began in.
    let remember = json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "remember", "arguments": { "fact": "The web app deploys on Fridays" } }
    });
    let (status, body) = call(&after, mcp(MCP, STATS, Some(&id), Some(&remember))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let text = answer(&body)["result"]["content"][0]["text"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("no tool text in {body}"));
    let remembered: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(remembered["namespace"], "web", "{remembered}");
}

#[tokio::test]
async fn a_session_continues_only_in_its_workspace_and_statistics_session() {
    let file = HubFile::new();
    let id = mcp_session(&file.start(), MCP, STATS).await;
    let hub = file.start();

    let (status, body) = list_tools(&hub, "/mcp?namespace=api", STATS, &id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another workspace: {body}");
    let (status, body) = list_tools(&hub, "/mcp", STATS, &id).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the default workspace: {body}"
    );
    let (status, body) = list_tools(&hub, MCP, "another-session", &id).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another statistics session: {body}"
    );

    let (status, body) = list_tools(&hub, MCP, STATS, &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_session_its_client_closed_continues_nowhere() {
    let file = HubFile::new();
    let before = file.start();
    let id = mcp_session(&before, MCP, STATS).await;

    let (status, body) = call(&before, mcp(MCP, STATS, Some(&id), None)).await;
    assert!(status.is_success(), "close: {status}: {body}");
    assert!(!file.kept(&id));
    let (status, body) = list_tools(&file.start(), MCP, STATS, &id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn a_session_unused_for_a_week_continues_nowhere() {
    let file = HubFile::new();
    let before = file.start();
    let (old, recent) = (
        mcp_session(&before, MCP, STATS).await,
        mcp_session(&before, MCP, STATS).await,
    );
    file.db()
        .execute(
            "UPDATE ctxone_mcp_sessions
             SET seen_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now', printf('-%d days', ?2))
             WHERE session_id = ?1",
            rusqlite::params![old, TTL_DAYS],
        )
        .unwrap();

    let after = file.start();
    let (status, body) = list_tools(&after, MCP, STATS, &old).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    // The next new session forgets it.
    assert!(file.kept(&old));
    mcp_session(&after, MCP, STATS).await;
    assert!(!file.kept(&old));
    let (status, body) = list_tools(&after, MCP, STATS, &recent).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
