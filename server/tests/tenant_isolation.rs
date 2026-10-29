//! Tenant isolation across every REST route and MCP tool
//! (ctx-multi-tenant-hub t-005).
//!
//! Two tenants hold data under the same names: the same workspace, project id
//! and remote, session ids, plans, branch, document and memory contexts. Every
//! text a tenant writes carries its marker. Tenant B then calls every route
//! and every tool, reading, listing, searching and writing with those names
//! and with tenant A's own ids:
//!
//! - no answer B gets carries A's marker or ids;
//! - where B has data of its own, the answer carries B's marker, so the probe
//!   reached real data rather than an empty or failed answer;
//! - none of B's requests opens A;
//! - what A reads afterwards is what it read before.
//!
//! `every_route_and_tool_is_probed` walks the routes `build_router` mounts and
//! the tools MCP lists. A route or tool with no probe here fails it, so a new
//! one can't skip this file.
//!
//! The sweep needs Postgres (`TEST_DATABASE_URL`; `CTXONE_REQUIRE_POSTGRES`
//! turns the skip into a failure, and CI sets both): a Hub keeping tenants in
//! memory has no project registry, so it can't make the shared workspace. It
//! runs twice: as the test's own role, and with row-level security on, as a
//! role it binds (t-011), so that a query it turns away fails a probe.

#[path = "support/row_security.rs"]
mod row_security;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use agentstategraph_storage::PostgresStorage;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use ctxone_hub::http;
use ctxone_hub::tenant::{Tenant, TenantsFile, TokenHash};
use ctxone_hub::tenant_cache::{MemoryTenants, PostgresTenants, TenantCache, TenantOpener};

/// The names both tenants use.
const WORKSPACE: &str = "app";
const SESSION: &str = "s1";
const PLAN: &str = "release";
const SHIPPED: &str = "shipped";
const BRANCH: &str = "feature";
const DOC: &str = "docs/runbook.md";
const REMOTE: &str = "https://git.example.com/team/app.git";
const LOCAL: &str = "/work/app";

const PAST: &str = "2026-01-01T00:00:00Z";
const FUTURE: &str = "2099-01-01T00:00:00Z";

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

/// The ids a tenant's own writes were given.
#[derive(Clone, Debug, Default)]
struct Ids {
    /// A fact in the default workspace, and two in the shared one.
    default_fact: String,
    fact: String,
    /// A fact the REST probes leave alone, for the MCP probes.
    kept_fact: String,
    commit: String,
    /// Reminders: the REST and MCP probes each take one past due through
    /// approve, start and record, and one pending through snooze and cancel.
    reminder: String,
    reminder_pending: String,
    mcp_reminder: String,
    mcp_reminder_pending: String,
    /// Taints: one for REST to remove, one for MCP.
    taint: String,
    mcp_taint: String,
    /// Drafts: one to promote, one to discard.
    draft: String,
    draft_discard: String,
    mcp_session: String,
}

impl Ids {
    /// Every id, to look for in the other tenant's answers.
    fn all(&self) -> Vec<&str> {
        vec![
            &self.default_fact,
            &self.fact,
            &self.kept_fact,
            &self.commit,
            &self.reminder,
            &self.reminder_pending,
            &self.mcp_reminder,
            &self.mcp_reminder_pending,
            &self.taint,
            &self.mcp_taint,
            &self.draft,
            &self.draft_discard,
            &self.mcp_session,
        ]
    }

    /// Stand-ins, to list the probes without a database.
    fn stand_in(tag: &str) -> Self {
        let id = |what: &str| format!("{tag}-{what}");
        Ids {
            default_fact: id("default-fact"),
            fact: id("fact"),
            kept_fact: id("kept-fact"),
            commit: id("commit"),
            reminder: id("reminder"),
            reminder_pending: id("reminder-pending"),
            mcp_reminder: id("mcp-reminder"),
            mcp_reminder_pending: id("mcp-reminder-pending"),
            taint: id("taint"),
            mcp_taint: id("mcp-taint"),
            draft: id("draft"),
            draft_discard: id("draft-discard"),
            mcp_session: id("mcp-session"),
        }
    }
}

/// One tenant: its token, the marker in every text it writes, and its ids.
struct Side {
    tenant: Tenant,
    token: String,
    marker: String,
    ids: Ids,
}

struct TestHub {
    app: axum::Router,
    data: TenantCache,
    _dir: tempfile::TempDir,
}

/// A Hub serving `tenants` (name and token), opening them with `opener`.
fn hub(opener: Arc<dyn TenantOpener>, tenants: &[(&Tenant, &str)]) -> TestHub {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tenants");
    let lines: String = tenants
        .iter()
        .map(|(t, token)| format!("{t} {}\n", TokenHash::of(token).to_hex()))
        .collect();
    std::fs::write(&path, lines).unwrap();
    let data = TenantCache::new(opener);
    let app = http::multi_tenant_router(
        http::HubConfig {
            mcp_http: true,
            agent_id: "itest".into(),
            ..Default::default()
        },
        http::MultiTenant {
            resolver: Arc::new(TenantsFile::open(&path).unwrap()),
            data: data.clone(),
        },
    );
    TestHub {
        app,
        data,
        _dir: dir,
    }
}

/// A REST request as `token`, in the shared workspace and session unless the
/// URI's query names others.
fn request(method: &str, uri: &str, token: &str, body: Option<&Value>) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "hub.example.com")
        .header("authorization", format!("Bearer {token}"))
        .header("x-ctxone-session", SESSION)
        .header("x-ctxone-namespace", WORKSPACE)
        .header("x-ctxone-agent", "itest");
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

/// Call a route that must succeed, and return its JSON answer.
async fn ok(app: &axum::Router, token: &str, method: &str, uri: &str, body: Value) -> Value {
    let body = (!body.is_null()).then_some(body);
    let (status, text) = call(app, request(method, uri, token, body.as_ref())).await;
    assert!(status.is_success(), "{method} {uri}: {status}: {text}");
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// An MCP request as `token`, in the shared workspace and session.
fn mcp_request(
    token: &str,
    method: &str,
    session: Option<&str>,
    body: Option<&Value>,
) -> Request<Body> {
    let mut req = request(method, &format!("/mcp?namespace={WORKSPACE}"), token, body);
    let headers = req.headers_mut();
    headers.insert(
        "accept",
        "application/json, text/event-stream".parse().unwrap(),
    );
    if let Some(id) = session {
        headers.insert("mcp-session-id", id.parse().unwrap());
    }
    req
}

/// Start an MCP session as `token` and return its id.
async fn mcp_session(app: &axum::Router, token: &str) -> String {
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
        .oneshot(mcp_request(token, "POST", None, Some(&init)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let id = resp.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    resp.into_body().collect().await.unwrap();
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let (status, body) = call(
        app,
        mcp_request(token, "POST", Some(&id), Some(&initialized)),
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    id
}

/// The JSON-RPC messages in an MCP answer, which comes as server-sent events.
fn rpc_messages(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

/// Call MCP tool `name` and return the whole answer and the tool's text.
async fn tool(
    app: &axum::Router,
    token: &str,
    session: &str,
    name: &str,
    arguments: &Value,
) -> (String, String) {
    let call_tool = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    let (status, body) = call(
        app,
        mcp_request(token, "POST", Some(session), Some(&call_tool)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{name}: {body}");
    let text = rpc_messages(&body)
        .iter()
        .find_map(|m| {
            m["result"]["content"][0]["text"]
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_default();
    (body, text)
}

/// The names of the tools MCP lists.
async fn listed_tools(app: &axum::Router, token: &str, session: &str) -> BTreeSet<String> {
    let list = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {} });
    let (status, body) = call(app, mcp_request(token, "POST", Some(session), Some(&list))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    rpc_messages(&body)
        .iter()
        .flat_map(|m| m["result"]["tools"].as_array().cloned().unwrap_or_default())
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect()
}

/// Give a tenant its data, under the names every tenant uses, each text
/// carrying `m`. Returns the ids the writes were given.
async fn seed(app: &axum::Router, token: &str, m: &str) -> Ids {
    let post = |uri: &'static str, body: Value| ok(app, token, "POST", uri, body);
    let mut ids = Ids::default();

    // The shared workspace, by registering the project of the same name.
    let project = json!({
        "id": WORKSPACE, "display_name": format!("{m} app"),
        "remote_url": REMOTE, "local_path": LOCAL,
    });
    post("/api/projects", project).await;

    // Memory: facts in both workspaces, on a branch, primed and imported.
    let default_fact =
        json!({ "fact": format!("{m} rotates the deploy key on Fridays"), "context": "ops" });
    let r = post("/api/memory/remember?namespace=default", default_fact).await;
    ids.default_fact = r["path"].as_str().unwrap().to_owned();
    let fact = json!({ "fact": format!("{m} pins the release train"), "context": "ops" });
    let r = post("/api/memory/remember", fact).await;
    ids.fact = r["path"].as_str().unwrap().to_owned();
    ids.commit = r["commit_id"].as_str().unwrap().to_owned();
    let kept = json!({ "fact": format!("{m} keeps the changelog"), "context": "ops" });
    ids.kept_fact = post("/api/memory/remember", kept).await["path"]
        .as_str()
        .unwrap()
        .to_owned();
    post("/api/branches", json!({ "name": BRANCH })).await;
    let on_branch =
        json!({ "fact": format!("{m} tries a feature"), "context": "ops", "ref": BRANCH });
    post("/api/memory/remember", on_branch).await;
    let sections = json!([{ "title": "Setup", "body": format!("{m} setup notes") }]);
    post(
        "/api/memory/prime",
        json!({ "source": "readme", "sections": sections, "pinned": true }),
    )
    .await;
    let paths = json!({
        "/memory/projects/app": { "summary": format!("{m} context") },
        "/imported/note": format!("{m} note"),
    });
    post("/api/import", json!({ "paths": paths })).await;
    let summary = json!({
        "session_id": SESSION,
        "key_points": [format!("{m} key point")],
        "decisions": [format!("{m} decision")],
    });
    post("/api/memory/summarize_session", summary).await;
    let doc = json!({ "path": DOC, "owner": m, "scope": format!("{m} scope") });
    post("/api/docs", doc).await;

    // Plans: one in progress, and one shipped and sealed as an epoch.
    post(
        "/api/plans",
        json!({ "name": PLAN, "description": format!("{m} release") }),
    )
    .await;
    for title in ["cut the branch", "tag the release"] {
        let task = json!({ "title": format!("{m} {title}") });
        post("/api/plans/release/tasks", task).await;
    }
    post("/api/plans/release/tasks/t-001/start", json!({})).await;
    post(
        "/api/plans/release/tasks/t-001/link",
        json!({ "target": "other/t-001" }),
    )
    .await;
    post(
        "/api/plans",
        json!({ "name": SHIPPED, "description": format!("{m} shipped") }),
    )
    .await;
    post(
        "/api/plans/shipped/tasks",
        json!({ "title": format!("{m} ship it") }),
    )
    .await;
    post("/api/plans/shipped/tasks/t-001/start", json!({})).await;
    let proof = json!({ "proof": { "kind": "text", "value": format!("{m} shipped it") } });
    post("/api/plans/shipped/tasks/t-001/complete", proof).await;
    post(
        "/api/plans/shipped/close",
        json!({ "summary": format!("{m} shipped") }),
    )
    .await;
    post("/api/plans/backfill_epochs", Value::Null).await;

    // A session: turns, title, meta, analysis, a link, statistics.
    let turn = |i: u32, minute: u32, text: &str| {
        json!({
            "turn_index": i,
            "timestamp": format!("2026-10-26T10:{minute:02}:00Z"),
            "tokens": { "input": 5, "output": 3 },
            "model": format!("model-{m}"),
            "user_text": format!("{m} {text}"),
        })
    };
    let turns = json!({ "t0000": turn(0, 0, "first question"), "t0001": turn(1, 1, "follow-up") });
    ok(app, token, "PUT", "/api/sessions/s1/turns", turns).await;
    ok(
        app,
        token,
        "PUT",
        "/api/sessions/s1/title",
        json!({ "title": format!("{m} title") }),
    )
    .await;
    let meta = json!({ "source": "claude-code", "models_used": [format!("model-{m}")] });
    ok(app, token, "PUT", "/api/sessions/s1/meta", meta).await;
    let note = json!({ "note": format!("{m} note") });
    ok(
        app,
        token,
        "PUT",
        "/api/sessions/s1/provenance",
        note.clone(),
    )
    .await;
    ok(app, token, "PUT", "/api/sessions/s1/burn", note.clone()).await;
    ok(app, token, "PUT", "/api/sessions/s1/analysis", note).await;
    let link = json!({ "plan": PLAN, "task": "t-001", "note": format!("{m} link") });
    post("/api/sessions/s1/links", link).await;
    let usage = json!({ "input_tokens": 100, "output_tokens": 50, "model": format!("model-{m}") });
    post("/api/stats/llm_usage", usage.clone()).await;
    // A session no other tenant has, its id carrying the marker: shared ids
    // would hide another tenant's session behind one's own.
    let mut req = request("POST", "/api/stats/llm_usage", token, Some(&usage));
    req.headers_mut()
        .insert("x-ctxone-session", format!("solo-{m}").parse().unwrap());
    let (status, text) = call(app, req).await;
    assert!(status.is_success(), "{status}: {text}");
    ok(
        app,
        token,
        "GET",
        &format!("/api/memory/recall?topic={m}"),
        Value::Null,
    )
    .await;
    // B's recalls, made first by each tenant: a recall cache shared across
    // tenants would hand B what A cached.
    ok(
        app,
        token,
        "GET",
        "/api/memory/recall?topic=release&budget=4000",
        Value::Null,
    )
    .await;
    ok(
        app,
        token,
        "DELETE",
        "/api/sessions/gone?ref_name=main",
        Value::Null,
    )
    .await;

    // Reminders past due (awaiting permission once looked at) and pending.
    let reminder = |title: &str, due: &str| json!({ "title": format!("{m} {title}"), "instructions": format!("{m} instructions"), "due_at": due });
    let id = |r: Value| r["id"].as_str().unwrap().to_owned();
    ids.reminder = id(post("/api/reminders", reminder("rotate keys", PAST)).await);
    ids.reminder_pending = id(post("/api/reminders", reminder("renew certs", FUTURE)).await);
    ids.mcp_reminder = id(post("/api/reminders", reminder("prune logs", PAST)).await);
    ids.mcp_reminder_pending = id(post("/api/reminders", reminder("audit", FUTURE)).await);
    ok(app, token, "GET", "/api/reminders/due", Value::Null).await;

    // Taints.
    let taint = |name: &str| {
        json!({
            "path": "/memory/ops", "name": format!("{m}-{name}"), "kind": "taint",
            "effect": "warn", "reason": format!("{m} reason"), "agent_id": "itest",
        })
    };
    let taint_id = |r: Value| r["taint_id"].as_str().unwrap().to_owned();
    ids.taint = taint_id(post("/api/taint", taint("hold")).await);
    ids.mcp_taint = taint_id(post("/api/taint", taint("watch")).await);

    // Drafts, over MCP: REST has no route for them.
    ids.mcp_session = mcp_session(app, token).await;
    let session = ids.mcp_session.clone();
    let recall = json!({ "topic": "release", "budget": 4000 });
    tool(app, token, &session, "recall", &recall).await;
    for (slot, content) in [(&mut ids.draft, "keep"), (&mut ids.draft_discard, "drop")] {
        let args =
            json!({ "session_id": SESSION, "content": format!("{m} {content}"), "ref": "main" });
        let (_, text) = tool(app, token, &session, "draft_stage", &args).await;
        let staged: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
        *slot = staged["draft_id"].as_str().unwrap().to_owned();
    }
    ids
}

/// What a probe expects of B's answer, besides carrying nothing of A's.
#[derive(Clone, Debug)]
enum Expect {
    /// Success, carrying B's own marker: the probe reached B's data.
    Mine,
    /// Success, carrying this, one of B's ids, where the answer has no text
    /// of B's to carry its marker.
    Carries(String),
    /// Success.
    Ok,
    /// Exactly this status, for one of A's ids or a name B already uses.
    Status(u16),
    /// An MCP tool's refusal or not-found, for one of A's ids.
    Refused,
}

/// One REST request of tenant B's.
struct Probe {
    /// The route as `build_router` mounts it, e.g. `GET /api/plans/{name}`.
    route: &'static str,
    uri: String,
    body: Option<Value>,
    expect: Expect,
}

macro_rules! probe {
    ($route:literal, $uri:expr, $expect:expr) => {
        Probe {
            route: $route,
            uri: $uri.to_string(),
            body: None,
            expect: $expect,
        }
    };
    ($route:literal, $uri:expr, $body:expr, $expect:expr) => {
        Probe {
            route: $route,
            uri: $uri.to_string(),
            body: Some($body),
            expect: $expect,
        }
    };
}

/// Routes that answer without a tenant's data, so they must open none.
const TENANT_FREE: &[&str] = &[
    "GET /api/health",
    "GET /api/tenant",
    "GET /api/help",
    "GET /api/help/manifest",
];

/// Tenant B's REST requests: `m` is its marker, `own` its ids, `other` A's.
/// Reads come first, then writes, then what deletes or moves B's data.
fn rest_probes(m: &str, own: &Ids, other: &Ids) -> Vec<Probe> {
    use Expect::*;
    let proof = json!({ "proof": { "kind": "text", "value": format!("{m} done") } });
    let turn = json!({ "turn_index": 2, "timestamp": "2026-10-26T10:05:00Z", "user_text": format!("{m} more") });
    let taint_removal = json!({ "reason": format!("{m} resolved"), "agent_id": "itest" });
    vec![
        probe!("GET /api/health", "/api/health", Ok),
        probe!("GET /api/tenant", "/api/tenant", Ok),
        probe!("GET /api/help", "/api/help?topic=recall", Ok),
        probe!("GET /api/help/manifest", "/api/help/manifest", Ok),
        // Statistics.
        probe!("GET /api/stats/tokens", "/api/stats/tokens", Ok),
        probe!(
            "GET /api/stats/tokens/{session_id}",
            "/api/stats/tokens/s1",
            Mine
        ),
        probe!("GET /api/stats/sessions", "/api/stats/sessions", Mine),
        probe!(
            "GET /api/stats/sessions",
            "/api/stats/sessions?namespace=default",
            Mine
        ),
        probe!("GET /api/sessions/imported", "/api/sessions/imported", Ok),
        probe!(
            "GET /api/stats/plan/{plan}/cost",
            "/api/stats/plan/release/cost",
            Mine
        ),
        probe!("GET /api/stats/{ref_name}", "/api/stats/main", Ok),
        probe!(
            "GET /api/stats/activity/{ref_name}",
            "/api/stats/activity/main",
            Ok
        ),
        // The graph.
        probe!("GET /api/state/{ref_name}", "/api/state/main?path=/", Mine),
        probe!(
            "GET /api/state/{ref_name}",
            format!("/api/state/main?path={}", other.fact),
            Status(404)
        ),
        probe!(
            "GET /api/state/{ref_name}/paths",
            "/api/state/main/paths",
            Ok
        ),
        probe!(
            "GET /api/state/{ref_name}/search",
            "/api/state/main/search?query=release",
            Mine
        ),
        probe!("GET /api/log/{ref_name}", "/api/log/main?limit=200", Mine),
        probe!(
            "GET /api/log/{ref_name}",
            format!("/api/log/{}", other.commit),
            Status(404)
        ),
        probe!(
            "GET /api/blame/{ref_name}",
            format!("/api/blame/main?path={}", own.fact),
            Mine
        ),
        probe!(
            "GET /api/diff",
            format!("/api/diff?ref_a=main&ref_b={BRANCH}"),
            Mine
        ),
        probe!(
            "GET /api/diff",
            format!("/api/diff?ref_a=main&ref_b={}", other.commit),
            Status(404)
        ),
        probe!("GET /api/branches", "/api/branches", Ok),
        probe!("GET /api/export", "/api/export", Mine),
        probe!("GET /api/export", "/api/export?namespace=default", Mine),
        // Memory.
        probe!(
            "GET /api/memory/recall",
            "/api/memory/recall?topic=release&budget=4000",
            Mine
        ),
        probe!(
            "GET /api/memory/context/{project}",
            "/api/memory/context/app",
            Mine
        ),
        probe!("GET /api/memory/pinned", "/api/memory/pinned", Mine),
        probe!(
            "GET /api/memory/what_changed_since",
            "/api/memory/what_changed_since?since=2000-01-01T00:00:00Z",
            Mine
        ),
        probe!(
            "GET /api/memory/why_did_we",
            "/api/memory/why_did_we?decision=release",
            Mine
        ),
        probe!("GET /api/docs", "/api/docs", Mine),
        // Plans and epochs.
        probe!("GET /api/plans", "/api/plans", Mine),
        probe!("GET /api/plans", "/api/plans?all_namespaces=true", Mine),
        probe!("GET /api/plans/stale", "/api/plans/stale?days=0", Mine),
        probe!("GET /api/epochs", "/api/epochs?all=true", Ok),
        probe!(
            "GET /api/epochs/{id}/export",
            "/api/epochs/plan:app:shipped/export",
            Mine
        ),
        probe!("GET /api/plans/{name}", "/api/plans/release", Mine),
        probe!(
            "GET /api/plans/{name}/tasks",
            "/api/plans/release/tasks",
            Mine
        ),
        probe!(
            "GET /api/plans/{name}/tasks/{task_id}",
            "/api/plans/release/tasks/t-001",
            Mine
        ),
        probe!(
            "GET /api/plans/{name}/next",
            "/api/plans/release/next",
            Mine
        ),
        probe!(
            "GET /api/plans/{name}/provenance",
            "/api/plans/release/provenance",
            Mine
        ),
        // Reminders.
        probe!("GET /api/reminders", "/api/reminders", Mine),
        probe!("GET /api/reminders/due", "/api/reminders/due", Mine),
        probe!(
            "GET /api/reminders/{id}",
            format!("/api/reminders/{}", own.reminder),
            Mine
        ),
        probe!(
            "GET /api/reminders/{id}",
            format!("/api/reminders/{}", other.reminder),
            Status(404)
        ),
        // Sessions.
        probe!(
            "GET /api/sessions/{sid}/turns",
            "/api/sessions/s1/turns",
            Ok
        ),
        probe!(
            "GET /api/sessions/{sid}/turns/{idx}",
            "/api/sessions/s1/turns/0",
            Mine
        ),
        probe!(
            "GET /api/sessions/{sid}/segments",
            "/api/sessions/s1/segments",
            Ok
        ),
        probe!(
            "GET /api/sessions/{sid}/starter",
            "/api/sessions/s1/starter",
            Mine
        ),
        // Kept in memory, so it lost the seed's recall when B closed.
        probe!(
            "GET /api/sessions/{sid}/recall-log",
            "/api/sessions/s1/recall-log",
            Carries(own.fact.clone())
        ),
        probe!(
            "GET /api/sessions/{sid}/title",
            "/api/sessions/s1/title",
            Mine
        ),
        probe!(
            "GET /api/sessions/{sid}/analysis",
            "/api/sessions/s1/analysis",
            Mine
        ),
        probe!("GET /api/namespaces", "/api/namespaces", Ok),
        probe!("GET /api/namespaces/summary", "/api/namespaces/summary", Ok),
        probe!("GET /api/session_tombstones", "/api/session_tombstones", Ok),
        // Taints and projects.
        probe!("GET /api/taint", "/api/taint", Mine),
        probe!(
            "GET /api/taint/check",
            "/api/taint/check?path=/memory/ops&agent_id=other",
            Ok
        ),
        probe!("GET /api/projects", "/api/projects", Mine),
        probe!("GET /api/projects/{id}", "/api/projects/app", Mine),
        probe!(
            "GET /api/projects/detect",
            format!("/api/projects/detect?cwd={LOCAL}&client_read=true&ctxproject=app"),
            Ok
        ),
        // Writes under the shared names, and with A's ids.
        probe!(
            "POST /api/memory/remember",
            "/api/memory/remember",
            json!({ "fact": format!("{m} later"), "context": "ops" }),
            Ok
        ),
        probe!(
            "POST /api/memory/forget",
            "/api/memory/forget",
            json!({ "path": other.fact }),
            Status(404)
        ),
        probe!(
            "POST /api/memory/prime",
            "/api/memory/prime",
            json!({ "source": "readme", "sections": [{ "title": "Setup", "body": format!("{m} again") }] }),
            Ok
        ),
        probe!(
            "POST /api/memory/summarize_session",
            "/api/memory/summarize_session",
            json!({ "session_id": SESSION, "key_points": [format!("{m} more")] }),
            Ok
        ),
        probe!(
            "POST /api/import",
            "/api/import",
            json!({ "paths": { "/imported/later": format!("{m} later") } }),
            Ok
        ),
        probe!("POST /api/fsck", "/api/fsck", json!({}), Ok),
        probe!(
            "POST /api/db/upgrade",
            "/api/db/upgrade",
            json!({ "check": true }),
            Ok
        ),
        probe!(
            "POST /api/docs",
            "/api/docs",
            json!({ "path": DOC, "owner": format!("{m} again") }),
            Mine
        ),
        probe!(
            "POST /api/plans",
            "/api/plans",
            json!({ "name": PLAN }),
            Status(409)
        ),
        probe!(
            "POST /api/plans",
            "/api/plans",
            json!({ "name": "later", "description": format!("{m} later") }),
            Mine
        ),
        probe!(
            "POST /api/plans/{name}/tasks",
            "/api/plans/release/tasks",
            json!({ "title": format!("{m} announce") }),
            Mine
        ),
        probe!(
            "PATCH /api/plans/{name}/tasks/{task_id}",
            "/api/plans/release/tasks/t-002",
            json!({ "title": format!("{m} tag it") }),
            Mine
        ),
        probe!(
            "POST /api/plans/{name}/tasks/{task_id}/start",
            "/api/plans/release/tasks/t-002/start",
            json!({}),
            Mine
        ),
        probe!(
            "POST /api/plans/{name}/tasks/{task_id}/complete",
            "/api/plans/release/tasks/t-001/complete",
            proof.clone(),
            Mine
        ),
        probe!(
            "POST /api/plans/{name}/tasks/{task_id}/link",
            "/api/plans/release/tasks/t-003/link",
            json!({ "target": "other/t-002" }),
            Ok
        ),
        probe!(
            "POST /api/plans/{name}/tasks/{task_id}/abandon",
            "/api/plans/release/tasks/t-003/abandon",
            json!({ "reason": format!("{m} dropped") }),
            Mine
        ),
        probe!(
            "POST /api/plans/{name}/tasks",
            "/api/plans/later/tasks",
            json!({ "title": format!("{m} wrap up") }),
            Mine
        ),
        probe!(
            "POST /api/plans/{name}/tasks/{task_id}/start",
            "/api/plans/later/tasks/t-001/start",
            json!({}),
            Mine
        ),
        probe!(
            "POST /api/plans/{name}/tasks/{task_id}/complete",
            "/api/plans/later/tasks/t-001/complete",
            proof,
            Mine
        ),
        probe!(
            "POST /api/plans/{name}/close",
            "/api/plans/later/close",
            json!({ "summary": format!("{m} wrapped") }),
            Mine
        ),
        probe!(
            "POST /api/plans/backfill_epochs",
            "/api/plans/backfill_epochs",
            Ok
        ),
        probe!(
            "POST /api/plans/{name}/force_complete",
            "/api/plans/release/force_complete",
            json!({ "summary": format!("{m} forced") }),
            Ok
        ),
        probe!(
            "POST /api/plans",
            "/api/plans",
            json!({ "name": "moving" }),
            Ok
        ),
        probe!(
            "POST /api/plans/{name}/move",
            "/api/plans/moving/move",
            json!({ "target_ref": BRANCH }),
            Ok
        ),
        probe!(
            "POST /api/plans",
            "/api/plans",
            json!({ "name": "relocating" }),
            Ok
        ),
        probe!(
            "POST /api/plans/{name}/relocate",
            "/api/plans/relocating/relocate",
            json!({ "to_namespace": "default" }),
            Ok
        ),
        probe!(
            "POST /api/plans",
            "/api/plans",
            json!({ "name": "archived" }),
            Ok
        ),
        probe!(
            "POST /api/plans/{name}/archive",
            "/api/plans/archived/archive",
            Ok
        ),
        probe!("DELETE /api/plans/{name}", "/api/plans/archived", Ok),
        probe!(
            "POST /api/move",
            "/api/move",
            json!({ "path": "/imported", "to_namespace": "default" }),
            Ok
        ),
        probe!(
            "POST /api/merge",
            "/api/merge",
            json!({ "source": BRANCH }),
            Ok
        ),
        probe!(
            "POST /api/branches",
            "/api/branches",
            json!({ "name": "probe" }),
            Ok
        ),
        probe!(
            "POST /api/branches/reset",
            "/api/branches/reset",
            json!({ "name": "probe", "to": other.commit }),
            Status(404)
        ),
        probe!(
            "POST /api/branches/reset",
            "/api/branches/reset",
            json!({ "name": "probe", "to": "main" }),
            Ok
        ),
        probe!("DELETE /api/branches/{name}", "/api/branches/probe", Ok),
        probe!(
            "POST /api/reminders",
            "/api/reminders",
            json!({ "title": format!("{m} later"), "instructions": "later", "due_at": FUTURE }),
            Mine
        ),
        probe!(
            "POST /api/reminders/{id}/snooze",
            format!("/api/reminders/{}/snooze", other.reminder_pending),
            json!({ "until": FUTURE }),
            Status(400)
        ),
        probe!(
            "POST /api/reminders/{id}/snooze",
            format!("/api/reminders/{}/snooze", own.reminder_pending),
            json!({ "until": FUTURE }),
            Mine
        ),
        probe!(
            "POST /api/reminders/{id}/approve",
            format!("/api/reminders/{}/approve", other.reminder),
            json!({}),
            Status(400)
        ),
        probe!(
            "POST /api/reminders/{id}/approve",
            format!("/api/reminders/{}/approve", own.reminder),
            json!({}),
            Mine
        ),
        probe!(
            "POST /api/reminders/{id}/start",
            format!("/api/reminders/{}/start", other.reminder),
            json!({}),
            Status(400)
        ),
        probe!(
            "POST /api/reminders/{id}/start",
            format!("/api/reminders/{}/start", own.reminder),
            json!({}),
            Mine
        ),
        probe!(
            "POST /api/reminders/{id}/record",
            format!("/api/reminders/{}/record", other.reminder),
            json!({ "id": other.reminder, "result": "success" }),
            Status(400)
        ),
        probe!(
            "POST /api/reminders/{id}/record",
            format!("/api/reminders/{}/record", own.reminder),
            json!({ "id": own.reminder, "result": "success" }),
            Mine
        ),
        probe!(
            "POST /api/reminders/{id}/cancel",
            format!("/api/reminders/{}/cancel", other.reminder_pending),
            Status(400)
        ),
        probe!(
            "POST /api/reminders/{id}/cancel",
            format!("/api/reminders/{}/cancel", own.reminder_pending),
            Mine
        ),
        probe!(
            "PUT /api/sessions/{sid}/turns",
            "/api/sessions/s1/turns",
            json!({ "t0000": turn.clone() }),
            Ok
        ),
        probe!(
            "POST /api/sessions/{sid}/turns/{idx}",
            "/api/sessions/s1/turns/2",
            turn,
            Ok
        ),
        probe!(
            "PUT /api/sessions/{sid}/title",
            "/api/sessions/s1/title",
            json!({ "title": format!("{m} renamed") }),
            Mine
        ),
        probe!(
            "PUT /api/sessions/{sid}/meta",
            "/api/sessions/s1/meta",
            json!({ "source": "codex" }),
            Ok
        ),
        probe!(
            "PUT /api/sessions/{sid}/provenance",
            "/api/sessions/s1/provenance",
            json!({ "note": "again" }),
            Ok
        ),
        probe!(
            "PUT /api/sessions/{sid}/burn",
            "/api/sessions/s1/burn",
            json!({ "note": "again" }),
            Ok
        ),
        probe!(
            "PUT /api/sessions/{sid}/analysis",
            "/api/sessions/s1/analysis",
            json!({ "note": "again" }),
            Ok
        ),
        probe!(
            "POST /api/sessions/{sid}/derive_links",
            "/api/sessions/s1/derive_links",
            Ok
        ),
        probe!(
            "POST /api/sessions/{sid}/links",
            "/api/sessions/s1/links",
            json!({ "plan": PLAN, "task": "t-002" }),
            Ok
        ),
        probe!(
            "POST /api/stats/backfill_by_model",
            "/api/stats/backfill_by_model",
            Ok
        ),
        probe!(
            "POST /api/stats/llm_usage",
            "/api/stats/llm_usage",
            json!({ "input_tokens": 1, "output_tokens": 1, "model": format!("model-{m}") }),
            Mine
        ),
        probe!(
            "POST /api/taint",
            "/api/taint",
            json!({ "path": "/memory/facts", "name": format!("{m}-later"), "kind": "taint", "effect": "warn", "reason": "later", "agent_id": "itest" }),
            Ok
        ),
        probe!(
            "DELETE /api/taint/{id}",
            format!("/api/taint/{}", other.taint),
            taint_removal.clone(),
            Status(404)
        ),
        probe!(
            "DELETE /api/taint/{id}",
            format!("/api/taint/{}", own.taint),
            taint_removal,
            Ok
        ),
        probe!(
            "POST /api/projects",
            "/api/projects",
            json!({ "id": WORKSPACE }),
            Status(409)
        ),
        probe!(
            "POST /api/projects",
            "/api/projects",
            json!({ "id": "web", "remote_url": "https://git.example.com/team/web.git" }),
            Ok
        ),
        probe!(
            "POST /api/projects/{id}/paths",
            "/api/projects/app/paths",
            json!({ "local_path": "/work/app-2" }),
            Mine
        ),
        // What deletes or moves B's data.
        probe!(
            "POST /api/memory/forget",
            "/api/memory/forget",
            json!({ "path": own.fact }),
            Ok
        ),
        probe!(
            "POST /api/sessions/{sid}/move",
            "/api/sessions/s1/move",
            json!({ "to_namespace": "default" }),
            Ok
        ),
        probe!(
            "DELETE /api/sessions/{sid}",
            "/api/sessions/s1?namespace=default&ref_name=main",
            Ok
        ),
    ]
}

/// One MCP tool call of tenant B's.
struct ToolProbe {
    tool: &'static str,
    args: Value,
    expect: Expect,
}

macro_rules! tool_probe {
    ($tool:literal, $args:expr, $expect:expr) => {
        ToolProbe {
            tool: $tool,
            args: $args,
            expect: $expect,
        }
    };
}

/// Tenant B's MCP tool calls: `m` is its marker, `own` its ids, `other` A's.
/// They run after the REST probes, which leave B's own data in place except
/// what they said they'd delete or move.
fn tool_probes(m: &str, own: &Ids, other: &Ids) -> Vec<ToolProbe> {
    use Expect::*;
    let none = json!({});
    let refusal = |id: &str| json!({ "taint_id": id, "reason": "resolved", "agent_id": "itest" });
    vec![
        // Reads.
        tool_probe!("project_status", none.clone(), Ok),
        tool_probe!("help", json!({ "topic": "recall" }), Ok),
        tool_probe!(
            "recall",
            json!({ "topic": "release", "budget": 4000 }),
            Mine
        ),
        tool_probe!("context", json!({ "project": "app" }), Mine),
        tool_probe!(
            "what_changed_since",
            json!({ "since": "2000-01-01T00:00:00Z" }),
            Mine
        ),
        tool_probe!("why_did_we", json!({ "decision": "changelog" }), Mine),
        tool_probe!("plan_list", none.clone(), Mine),
        tool_probe!("plan_show", json!({ "plan_id": PLAN }), Mine),
        tool_probe!("plan_tasks", json!({ "plan_id": PLAN }), Mine),
        tool_probe!("plan_stale", json!({ "days": 0 }), Ok),
        tool_probe!("docs_find", json!({ "query": "runbook" }), Mine),
        tool_probe!("namespace_for_plan", json!({ "plan_id": PLAN }), Ok),
        tool_probe!(
            "namespace_for_task",
            json!({ "plan_id": PLAN, "task_id": "t-001" }),
            Ok
        ),
        tool_probe!("branches", none.clone(), Ok),
        tool_probe!("taint_list", none.clone(), Mine),
        tool_probe!(
            "taint_check",
            json!({ "path": "/memory/ops", "agent_id": "other" }),
            Ok
        ),
        tool_probe!("get", json!({ "path": "/" }), Mine),
        tool_probe!("get", json!({ "path": other.fact }), Refused),
        tool_probe!("ls", none.clone(), Ok),
        tool_probe!("search", json!({ "query": "release" }), Mine),
        tool_probe!("log", json!({ "limit": 200 }), Mine),
        tool_probe!("log", json!({ "ref": other.commit }), Refused),
        tool_probe!("blame", json!({ "path": own.kept_fact }), Mine),
        tool_probe!("diff", json!({ "ref_a": "main", "ref_b": BRANCH }), Mine),
        tool_probe!(
            "diff",
            json!({ "ref_a": "main", "ref_b": other.commit }),
            Refused
        ),
        tool_probe!("reminder_list", none.clone(), Mine),
        tool_probe!("remind_me", none.clone(), Mine),
        tool_probe!("reminder_get", json!({ "id": own.mcp_reminder }), Mine),
        tool_probe!("reminder_get", json!({ "id": other.mcp_reminder }), Refused),
        tool_probe!(
            "session_segments",
            json!({ "session_id": "s1", "ref": "main" }),
            Ok
        ),
        tool_probe!(
            "drafts_list",
            json!({ "session_id": SESSION, "ref": "main" }),
            Mine
        ),
        tool_probe!(
            "session_info",
            json!({ "session_id": "gone", "ref": "main" }),
            Ok
        ),
        tool_probe!("code_repos", none.clone(), Ok),
        tool_probe!("get_active_repo", none.clone(), Ok),
        tool_probe!("code_cross_repo_edges", none.clone(), Refused),
        tool_probe!("code_impact", json!({ "target": "get_orders" }), Refused),
        tool_probe!("code_search", json!({ "query": "orders" }), Refused),
        tool_probe!("code_read", json!({ "qname": "app::orders" }), Refused),
        tool_probe!("callers_of", json!({ "qname": "app::orders" }), Refused),
        tool_probe!("callees_of", json!({ "qname": "app::orders" }), Refused),
        // Writes.
        tool_probe!("set_active_repo", json!({ "repo": "" }), Ok),
        tool_probe!(
            "remember",
            json!({ "fact": format!("{m} via mcp"), "context": "ops" }),
            Mine
        ),
        tool_probe!("forget", json!({ "path": other.fact }), Refused),
        tool_probe!(
            "forget",
            json!({ "path": "/memory/pinned/readme/setup" }),
            Ok
        ),
        tool_probe!(
            "prime",
            json!({ "source": "mcp", "sections": [{ "title": "Setup", "body": format!("{m} mcp") }] }),
            Ok
        ),
        tool_probe!(
            "summarize_session",
            json!({ "session_id": SESSION, "key_points": [format!("{m} mcp")] }),
            Ok
        ),
        tool_probe!(
            "record_llm_usage",
            json!({ "input_tokens": 1, "output_tokens": 1, "model": format!("model-{m}") }),
            Mine
        ),
        tool_probe!(
            "plan_new",
            json!({ "name": "mcp-plan", "description": format!("{m} mcp plan") }),
            Mine
        ),
        tool_probe!(
            "plan_add",
            json!({ "plan_id": "mcp-plan", "title": format!("{m} one") }),
            Mine
        ),
        tool_probe!(
            "plan_add",
            json!({ "plan_id": "mcp-plan", "title": format!("{m} two") }),
            Mine
        ),
        tool_probe!(
            "plan_add",
            json!({ "plan_id": "mcp-plan", "title": format!("{m} three") }),
            Mine
        ),
        tool_probe!(
            "plan_start",
            json!({ "plan_id": "mcp-plan", "task_id": "t-001" }),
            Mine
        ),
        tool_probe!("plan_next", json!({ "plan_id": "mcp-plan" }), Mine),
        tool_probe!(
            "plan_edit",
            json!({ "plan_id": "mcp-plan", "task_id": "t-002", "title": format!("{m} two, edited") }),
            Mine
        ),
        tool_probe!(
            "plan_done",
            json!({ "plan_id": "mcp-plan", "task_id": "t-001", "proof": { "kind": "text", "value": format!("{m} done") } }),
            Mine
        ),
        tool_probe!(
            "plan_abandon",
            json!({ "plan_id": "mcp-plan", "task_id": "t-002", "reason": format!("{m} dropped") }),
            Mine
        ),
        tool_probe!(
            "plan_link",
            json!({ "plan_id": "mcp-plan", "task_id": "t-003", "target": "other/t-001" }),
            Ok
        ),
        tool_probe!(
            "plan_complete",
            json!({ "plan_id": "mcp-plan", "summary": format!("{m} completed") }),
            Mine
        ),
        tool_probe!("plan_new", json!({ "name": "mcp-close" }), Ok),
        tool_probe!(
            "plan_add",
            json!({ "plan_id": "mcp-close", "title": "only" }),
            Ok
        ),
        tool_probe!(
            "plan_start",
            json!({ "plan_id": "mcp-close", "task_id": "t-001" }),
            Ok
        ),
        tool_probe!(
            "plan_done",
            json!({ "plan_id": "mcp-close", "task_id": "t-001", "proof": { "kind": "text", "value": "done" } }),
            Ok
        ),
        tool_probe!(
            "plan_close",
            json!({ "plan_id": "mcp-close", "summary": format!("{m} closed") }),
            Mine
        ),
        tool_probe!("plan_new", json!({ "name": "mcp-moving" }), Ok),
        tool_probe!(
            "plan_move",
            json!({ "plan_id": "mcp-moving", "target_ref": BRANCH }),
            Ok
        ),
        tool_probe!("plan_new", json!({ "name": "mcp-archived" }), Ok),
        tool_probe!("plan_archive", json!({ "plan_id": "mcp-archived" }), Ok),
        tool_probe!("branch", json!({ "name": "mcp-branch" }), Ok),
        tool_probe!(
            "branch",
            json!({ "name": "stolen", "from": other.commit }),
            Refused
        ),
        tool_probe!("merge", json!({ "source": BRANCH, "dry_run": true }), Ok),
        tool_probe!(
            "taint_apply",
            json!({ "path": "/memory/facts", "name": format!("{m}-mcp"), "kind": "taint", "effect": "warn", "reason": "mcp", "agent_id": "itest" }),
            Ok
        ),
        tool_probe!("taint_remove", refusal(&other.mcp_taint), Refused),
        tool_probe!("taint_remove", refusal(&own.mcp_taint), Ok),
        tool_probe!(
            "reminder_create",
            json!({ "title": format!("{m} mcp"), "instructions": "mcp", "due_at": FUTURE }),
            Mine
        ),
        tool_probe!(
            "reminder_snooze",
            json!({ "id": other.mcp_reminder_pending, "until": FUTURE }),
            Refused
        ),
        tool_probe!(
            "reminder_snooze",
            json!({ "id": own.mcp_reminder_pending, "until": FUTURE }),
            Mine
        ),
        tool_probe!(
            "reminder_approve",
            json!({ "id": other.mcp_reminder }),
            Refused
        ),
        tool_probe!("reminder_approve", json!({ "id": own.mcp_reminder }), Mine),
        tool_probe!(
            "reminder_start",
            json!({ "id": other.mcp_reminder }),
            Refused
        ),
        tool_probe!("reminder_start", json!({ "id": own.mcp_reminder }), Mine),
        tool_probe!(
            "reminder_record",
            json!({ "id": other.mcp_reminder, "result": "success" }),
            Refused
        ),
        tool_probe!(
            "reminder_record",
            json!({ "id": own.mcp_reminder, "result": "success" }),
            Mine
        ),
        tool_probe!(
            "reminder_cancel",
            json!({ "id": other.mcp_reminder_pending }),
            Refused
        ),
        tool_probe!(
            "reminder_cancel",
            json!({ "id": own.mcp_reminder_pending }),
            Mine
        ),
        tool_probe!(
            "draft_stage",
            json!({ "session_id": SESSION, "content": format!("{m} staged"), "ref": "main" }),
            Ok
        ),
        tool_probe!(
            "draft_promote",
            json!({ "session_id": SESSION, "draft_id": other.draft, "ref": "main" }),
            Refused
        ),
        tool_probe!(
            "draft_promote",
            json!({ "session_id": SESSION, "draft_id": own.draft, "ref": "main" }),
            Ok
        ),
        tool_probe!(
            "draft_discard",
            json!({ "session_id": SESSION, "draft_id": other.draft_discard, "ref": "main" }),
            Refused
        ),
        tool_probe!(
            "draft_discard",
            json!({ "session_id": SESSION, "draft_id": own.draft_discard, "ref": "main" }),
            Ok
        ),
        tool_probe!(
            "session_link_plan",
            json!({ "session_id": SESSION, "plan": PLAN, "task": "t-001", "ref": "main" }),
            Ok
        ),
    ]
}

/// Whether an MCP tool's text is a refusal or a not-found.
fn refused(text: &str) -> bool {
    let lower = text.to_lowercase();
    text.is_empty()
        || lower.starts_with("error")
        || lower.starts_with("no record")
        || lower.starts_with("no context")
        || serde_json::from_str::<Value>(text).is_ok_and(|v| v.get("error").is_some())
}

/// Tenant B's answer to one probe.
struct Answer {
    /// The request, for messages.
    what: String,
    /// What the request carried: an answer may echo it.
    sent: String,
    status: StatusCode,
    body: String,
    /// An MCP tool's text.
    text: Option<String>,
}

/// What's wrong with B's answer: something of A's in it, or `expect` unmet.
fn problems(answer: &Answer, expect: &Expect, b: &Side, a: &Side) -> Vec<String> {
    let Answer {
        what,
        sent,
        status,
        body,
        text,
    } = answer;
    let mut found = Vec::new();
    let mut theirs = vec![a.marker.as_str()];
    theirs.extend(a.ids.all());
    for t in theirs.into_iter().filter(|t| !sent.contains(*t)) {
        if let Some(at) = body.find(t) {
            found.push(format!(
                "LEAK {what}: B's answer carries A's {t:?}: …{}…",
                excerpt(body, at)
            ));
        }
    }
    let ok = status.is_success() && text.as_deref().is_none_or(|t| !refused(t));
    let met = match expect {
        Expect::Mine => ok && body.contains(&b.marker),
        Expect::Carries(id) => ok && body.contains(id.as_str()),
        Expect::Ok => ok,
        Expect::Status(code) => status.as_u16() == *code,
        Expect::Refused => text.as_deref().is_some_and(refused),
    };
    if !met {
        found.push(format!(
            "{what}: expected {expect:?}, got {status}: {}",
            excerpt(body, 0)
        ));
    }
    found
}

/// Up to 400 characters of `s` from a little before byte `at`.
fn excerpt(s: &str, at: usize) -> String {
    let start = (0..=at.saturating_sub(100))
        .rev()
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(0);
    s[start..].chars().take(400).collect()
}

/// What tenant A reads, to compare before and after B's probes.
async fn snapshot(app: &axum::Router, a: &Side) -> Vec<(String, String)> {
    let uris = [
        "/api/export?namespace=default",
        "/api/export",
        "/api/export?ref=feature",
        "/api/log/main?limit=500",
        "/api/log/main?namespace=default&limit=500",
        "/api/plans?all_namespaces=true",
        "/api/plans/release",
        "/api/plans/shipped",
        "/api/epochs?all=true",
        "/api/reminders",
        "/api/taint?include_resolved=true",
        "/api/projects",
        "/api/projects/app",
        "/api/branches",
        "/api/docs",
        "/api/memory/pinned",
        "/api/stats/tokens/s1",
        "/api/sessions/s1/turns/0",
        "/api/sessions/s1/title",
        "/api/session_tombstones",
        "/api/namespaces",
    ];
    let mut reads = Vec::new();
    for uri in uris {
        let mut req = request("GET", uri, &a.token, None);
        req.headers_mut()
            .insert("x-ctxone-session", "snapshot".parse().unwrap());
        let (status, body) = call(app, req).await;
        assert_eq!(status, StatusCode::OK, "A's {uri}: {body}");
        assert!(
            body.contains(&a.marker) || !uri.contains("export"),
            "A's {uri}: {body}"
        );
        reads.push((uri.to_owned(), body));
    }
    reads
}

/// The routes a multi-tenant Hub answers with a 404 of its own, or doesn't
/// mount: they act on the Hub's machine (`tenant_machine_local.rs`).
fn machine_local(route: &str) -> bool {
    let path = route.split_once(' ').map_or(route, |(_, p)| p);
    http::MACHINE_LOCAL_ROUTES.contains(&path) || path.starts_with("/api/code")
}

/// Every `METHOD /path` that `build_router` mounts, read from its source.
fn mounted_routes() -> BTreeSet<String> {
    let source = include_str!("../src/http.rs");
    let start = source.find("\nfn build_router(").expect("build_router");
    let body = &source[start..];
    let end = body.find("\n}\n").expect("the end of build_router");
    let mut routes = BTreeSet::new();
    let mut rest = &body[..end];
    while let Some(at) = rest.find(".route(") {
        rest = &rest[at + ".route(".len()..];
        let args = up_to_close(rest);
        let Some((path, handlers)) = args.split_once(',') else {
            continue;
        };
        // A literal, or the variable of the loop over MACHINE_LOCAL_ROUTES.
        let Some(path) = path
            .trim()
            .strip_prefix('"')
            .and_then(|p| p.strip_suffix('"'))
        else {
            continue;
        };
        for method in methods(handlers) {
            routes.insert(format!("{method} {path}"));
        }
    }
    routes
}

/// `s` up to the parenthesis that closes the one opened before it.
fn up_to_close(s: &str) -> &str {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return &s[..i],
            ')' => depth -= 1,
            _ => {}
        }
    }
    s
}

/// The methods a method router such as `get(a).post(b)` answers.
fn methods(handlers: &str) -> Vec<String> {
    ["get", "post", "put", "patch", "delete"]
        .into_iter()
        .filter(|m| {
            handlers
                .match_indices(&format!("{m}("))
                .any(|(i, _)| !handlers[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
        })
        .map(str::to_uppercase)
        .collect()
}

#[tokio::test]
async fn every_route_and_tool_is_probed() {
    let (own, other) = (Ids::stand_in("b"), Ids::stand_in("a"));

    let mounted = mounted_routes();
    assert!(
        mounted.len() > 100,
        "found only {} routes: {mounted:#?}",
        mounted.len()
    );
    let probed: BTreeSet<&str> = rest_probes("b", &own, &other)
        .iter()
        .map(|p| p.route)
        .collect();
    let unprobed: Vec<&String> = mounted
        .iter()
        .filter(|r| !machine_local(r) && !probed.contains(r.as_str()))
        .collect();
    assert!(
        unprobed.is_empty(),
        "routes no isolation probe calls: {unprobed:#?}"
    );
    let unmounted: Vec<&&str> = probed.iter().filter(|r| !mounted.contains(**r)).collect();
    assert!(
        unmounted.is_empty(),
        "probes of routes build_router doesn't mount: {unmounted:#?}"
    );
    for route in TENANT_FREE {
        assert!(mounted.contains(*route), "{route} isn't mounted");
    }

    let alice = Tenant::new("alice").unwrap();
    let hub = hub(
        Arc::new(MemoryTenants::default()),
        &[(&alice, "alice-token")],
    );
    let session = mcp_session(&hub.app, "alice-token").await;
    let listed = listed_tools(&hub.app, "alice-token", &session).await;
    assert!(listed.len() > 50, "MCP lists only {listed:?}");
    let probed: BTreeSet<&str> = tool_probes("b", &own, &other)
        .iter()
        .map(|p| p.tool)
        .collect();
    let unprobed: Vec<&String> = listed
        .iter()
        .filter(|t| !probed.contains(t.as_str()))
        .collect();
    assert!(
        unprobed.is_empty(),
        "tools no isolation probe calls: {unprobed:#?}"
    );
    let unlisted: Vec<&&str> = probed.iter().filter(|t| !listed.contains(**t)).collect();
    assert!(
        unlisted.is_empty(),
        "probes of tools MCP doesn't list: {unlisted:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenants_with_the_same_names_reach_only_their_own_data() {
    let Some(url) = url() else { return };
    let root = PostgresStorage::connect_tenant_with_pool_size(&url, fresh("root").as_str(), 8)
        .await
        .expect("connect");
    sweep(PostgresTenants::new(root).expect("tenants")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_row_security_on_tenants_with_the_same_names_reach_only_their_own_data() {
    let Some(url) = url() else { return };
    let schema = row_security::Schema::new(&url, "iso").await;
    let root =
        PostgresStorage::connect_tenant_with_pool_size(&schema.app_url, fresh("root").as_str(), 8)
            .await
            .expect("connect");
    let tenants = PostgresTenants::new(root).expect("tenants");
    // Switched on while the Hub's pool is open, as on a live database.
    let url = schema.app_url.clone();
    tokio::task::spawn_blocking(move || row_security::switch_on(&url))
        .await
        .unwrap();
    sweep(tenants).await;
    schema.drop().await;
}

/// Seed two tenants with the same names on `tenants`, then probe every route
/// and tool as one of them.
async fn sweep(tenants: PostgresTenants) {
    let opener = Arc::new(tenants);
    let token = || format!("token-{}", uuid::Uuid::new_v4());
    let marker = |tag: &str| format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let (mut a, mut b) = (
        Side {
            tenant: fresh("iso-a"),
            token: token(),
            marker: marker("alpha"),
            ids: Ids::default(),
        },
        Side {
            tenant: fresh("iso-b"),
            token: token(),
            marker: marker("bravo"),
            ids: Ids::default(),
        },
    );
    let hub = hub(opener, &[(&a.tenant, &a.token), (&b.tenant, &b.token)]);
    a.ids = seed(&hub.app, &a.token, &a.marker).await;
    b.ids = seed(&hub.app, &b.token, &b.marker).await;

    // A's reads, with A then closed: B's requests mustn't open it again.
    hub.data.evict_idle(Duration::ZERO);
    let before = snapshot(&hub.app, &a).await;
    hub.data.evict_idle(Duration::ZERO);
    assert!(
        hub.data.open_tenants().is_empty(),
        "{:?}",
        hub.data.open_tenants()
    );

    // Every problem, so one run shows them all.
    let mut found = Vec::new();
    let mut opened = |what: &str, tenant_free: bool| {
        let open = hub.data.open_tenants();
        if open.contains(&a.tenant) {
            found.push(format!("LEAK {what}: opened A"));
        }
        if tenant_free && !open.is_empty() {
            found.push(format!("{what}: opened {open:?}, but needs no tenant"));
        }
    };
    let mut answers = Vec::new();
    for path in http::MACHINE_LOCAL_ROUTES {
        let uri = path.replace("{*rest}", "app/search");
        let (status, body) = call(&hub.app, request("GET", &uri, &b.token, None)).await;
        opened(&uri, true);
        answers.push((
            Answer {
                what: format!("GET {uri}"),
                sent: uri,
                status,
                body,
                text: None,
            },
            Expect::Status(404),
        ));
    }
    for probe in rest_probes(&b.marker, &b.ids, &a.ids) {
        let method = probe.route.split(' ').next().unwrap();
        let req = request(method, &probe.uri, &b.token, probe.body.as_ref());
        let (status, body) = call(&hub.app, req).await;
        let what = format!("{method} {}", probe.uri);
        opened(&what, TENANT_FREE.contains(&probe.route));
        let sent = format!(
            "{} {}",
            probe.uri,
            probe.body.map_or_else(String::new, |b| b.to_string())
        );
        answers.push((
            Answer {
                what,
                sent,
                status,
                body,
                text: None,
            },
            probe.expect,
        ));
    }

    let session = mcp_session(&hub.app, &b.token).await;
    for probe in tool_probes(&b.marker, &b.ids, &a.ids) {
        let (body, text) = tool(&hub.app, &b.token, &session, probe.tool, &probe.args).await;
        let what = format!("MCP {} {}", probe.tool, probe.args);
        opened(&what, false);
        let sent = probe.args.to_string();
        answers.push((
            Answer {
                what,
                sent,
                status: StatusCode::OK,
                body,
                text: Some(text),
            },
            probe.expect,
        ));
    }

    for (answer, expect) in &answers {
        found.extend(problems(answer, expect, &b, &a));
    }
    let after = snapshot(&hub.app, &a).await;
    for ((uri, was), (_, is)) in before.iter().zip(&after) {
        if was != is {
            let at = was
                .bytes()
                .zip(is.bytes())
                .position(|(x, y)| x != y)
                .unwrap_or(0);
            found.push(format!(
                "LEAK A's {uri} changed while B worked: was …{}…, is …{}…",
                excerpt(was, at),
                excerpt(is, at)
            ));
        }
    }

    // A's MCP session is A's alone: B's token reaches B's own sessions, so
    // A's id is unknown there, and B closing it closes nothing of A's. (A's
    // seeded session closed with A.)
    let a_session = mcp_session(&hub.app, &a.token).await;
    let list = json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list", "params": {} });
    let req = mcp_request(&b.token, "POST", Some(&a_session), Some(&list));
    let (status, body) = call(&hub.app, req).await;
    if status != StatusCode::NOT_FOUND {
        found.push(format!(
            "LEAK B used A's MCP session: {status}: {}",
            excerpt(&body, 0)
        ));
    }
    let req = mcp_request(&b.token, "DELETE", Some(&a_session), None);
    call(&hub.app, req).await;
    let drafts = json!({
        "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": { "name": "drafts_list", "arguments": { "session_id": SESSION, "ref": "main" } }
    });
    let req = mcp_request(&a.token, "POST", Some(&a_session), Some(&drafts));
    let (status, body) = call(&hub.app, req).await;
    if status != StatusCode::OK || !body.contains(&a.ids.draft) {
        found.push(format!(
            "A's MCP session after B closed its id: {status}: {}",
            excerpt(&body, 0)
        ));
    }
    assert!(
        found.is_empty(),
        "{} problems:\n\n{}",
        found.len(),
        found.join("\n\n")
    );
}
