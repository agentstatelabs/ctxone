//! CTXone plan tools — wrap `agentstategraph-tasks` and surface it as
//! MCP tools + shared helpers for the HTTP endpoints.
//!
//! The substrate crate (`agentstategraph_tasks::TaskStore`) handles
//! plan storage, state transitions, blocker validation, parent rollup,
//! proof storage, plan completion, **and agent assignment**. CTXone
//! adds only:
//!
//! - **MCP tool surface** (`plan_new`, `plan_add`, `plan_start`,
//!   `plan_complete`, `plan_abandon`, `plan_next`, `plan_list`,
//!   `plan_get`, `plan_archive`, `plan_tasks`) with proactive
//!   "CALL THIS WHEN" descriptions. The tool methods themselves live
//!   on `CtxOneServer` in `memory_tools.rs` so every MCP tool shares a
//!   single `ToolRouter`; this module provides the typed parameter
//!   structs and the pure helpers the tool bodies delegate into.
//! - **Response shape** — `task_to_json` / `plan_to_json` render the
//!   substrate types as the JSON shape CTXone exposes on the wire.
//!
//! Earlier versions maintained a `/plan_assignments` sidecar because
//! `Task` lacked an `assigned_to` field. The substrate now carries it
//! natively; the sidecar and its helpers have been removed.

use std::sync::Arc;

use agentstategraph::Repository;
use agentstategraph_tasks::{
    Plan, PlanStatus, Priority, Proof, ProofKind, Task, TaskId, TaskStatus, TaskStore,
    TaskStoreError,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Default path prefix where plans live in the state graph.
pub const PLANS_PREFIX: &str = "/plans";

fn default_ref() -> String {
    "main".to_string()
}

fn default_true() -> bool {
    true
}

pub fn priority_from_str(s: &str) -> Option<Priority> {
    match s.to_ascii_lowercase().as_str() {
        "low" => Some(Priority::Low),
        "medium" => Some(Priority::Medium),
        "high" => Some(Priority::High),
        "critical" => Some(Priority::Critical),
        _ => None,
    }
}

pub fn priority_label(p: Priority) -> &'static str {
    match p {
        Priority::Low => "low",
        Priority::Medium => "medium",
        Priority::High => "high",
        Priority::Critical => "critical",
    }
}

pub fn task_status_label(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Pending => "pending",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Done => "done",
        TaskStatus::Abandoned => "abandoned",
    }
}

pub fn plan_status_label(s: PlanStatus) -> &'static str {
    match s {
        PlanStatus::Active => "active",
        PlanStatus::Completed => "completed",
        PlanStatus::Archived => "archived",
    }
}

pub fn proof_kind_label(k: ProofKind) -> &'static str {
    match k {
        ProofKind::Commit => "commit",
        ProofKind::File => "file",
        ProofKind::Test => "test",
        ProofKind::Text => "text",
    }
}

pub fn plan_status_from_str(s: &str) -> Option<PlanStatus> {
    match s.to_ascii_lowercase().as_str() {
        "active" => Some(PlanStatus::Active),
        "completed" => Some(PlanStatus::Completed),
        "archived" => Some(PlanStatus::Archived),
        _ => None,
    }
}

/// Build a `Proof` from `(kind, value, note?)`.
pub fn parse_proof(
    kind: &str,
    value: &str,
    note: Option<String>,
) -> Result<Proof, PlanToolError> {
    if value.trim().is_empty() {
        return Err(PlanToolError::InvalidProof(
            "proof value must be non-empty".into(),
        ));
    }
    let p = match kind.to_ascii_lowercase().as_str() {
        "commit" => Proof::commit(value),
        "file" => Proof::file(value),
        "test" => Proof::test(value),
        "text" => Proof::text(value),
        other => {
            return Err(PlanToolError::InvalidProof(format!(
                "unknown proof kind '{}' (expected commit|file|test|text)",
                other
            )));
        }
    };
    Ok(match note {
        Some(n) if !n.is_empty() => p.with_note(n),
        _ => p,
    })
}

/// Errors surfaced from the plan-tool layer. Wraps substrate errors
/// but adds an `InvalidProof` variant and friendlier string coercion.
#[derive(Debug, thiserror::Error)]
pub enum PlanToolError {
    #[error("{0}")]
    Substrate(#[from] TaskStoreError),

    #[error("invalid proof: {0}")]
    InvalidProof(String),

    #[error("invalid priority: {0}")]
    InvalidPriority(String),

    #[error("repository error: {0}")]
    Repo(String),
}

impl From<agentstategraph::RepoError> for PlanToolError {
    fn from(e: agentstategraph::RepoError) -> Self {
        PlanToolError::Repo(e.to_string())
    }
}

/// Build a `TaskStore` bound to the default plans prefix for the
/// given agent. Use this inside every tool/endpoint so the prefix is
/// consistent across the Hub.
pub fn make_store(repo: Arc<Repository>, agent_id: &str) -> TaskStore {
    TaskStore::new(repo, PLANS_PREFIX, agent_id)
}

// -- Response shape helpers -----------------------------------------

/// Render a task as the JSON shape CTXone exposes on the wire.
/// `assigned_to` is now carried natively on `Task`, so this is a
/// straight rename of fields — no sidecar lookup.
pub fn task_to_json(task: &Task) -> serde_json::Value {
    let mut out = serde_json::json!({
        "id": task.id.as_str(),
        "title": task.title,
        "status": task_status_label(task.status),
        "priority": priority_label(task.priority),
        "parent_id": task.parent_id.as_ref().map(|t| t.as_str()),
        "blocked_by": task
            .blocked_by
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>(),
        "assigned_to": task.assigned_to,
        "created_at": task.created_at.to_rfc3339(),
        "created_by": task.created_by,
        "started_at": task.started_at.map(|t| t.to_rfc3339()),
        "started_by": task.started_by,
        "completed_at": task.completed_at.map(|t| t.to_rfc3339()),
        "completed_by": task.completed_by,
        "abandoned_at": task.abandoned_at.map(|t| t.to_rfc3339()),
        "abandoned_reason": task.abandoned_reason,
    });
    if let Some(p) = &task.proof {
        out["proof"] = serde_json::json!({
            "kind": proof_kind_label(p.kind),
            "value": p.value,
            "note": p.note,
        });
    } else {
        out["proof"] = serde_json::Value::Null;
    }
    out
}

/// Render a plan as a JSON object, including task counts keyed by
/// status. The `tasks` field is included only when `with_tasks` is
/// true — the list endpoint keeps responses short, the detail endpoint
/// opts in.
pub fn plan_to_json(plan: &Plan, tasks: &[Task], with_tasks: bool) -> serde_json::Value {
    let mut pending = 0u32;
    let mut in_progress = 0u32;
    let mut done = 0u32;
    let mut abandoned = 0u32;
    for t in tasks {
        match t.status {
            TaskStatus::Pending => pending += 1,
            TaskStatus::InProgress => in_progress += 1,
            TaskStatus::Done => done += 1,
            TaskStatus::Abandoned => abandoned += 1,
        }
    }
    let task_counts = serde_json::json!({
        "pending": pending,
        "in_progress": in_progress,
        "done": done,
        "abandoned": abandoned,
        "total": tasks.len() as u32,
    });

    let mut out = serde_json::json!({
        "name": plan.name,
        "description": plan.description,
        "status": plan_status_label(plan.status),
        "created_at": plan.created_at.to_rfc3339(),
        "created_by": plan.created_by,
        "archived_at": plan.archived_at.map(|t| t.to_rfc3339()),
        "task_counts": task_counts,
    });

    if with_tasks {
        let tasks_json: Vec<serde_json::Value> = tasks.iter().map(task_to_json).collect();
        out["tasks"] = serde_json::Value::Array(tasks_json);
    }

    out
}

// -- MCP / HTTP parameter types -------------------------------------

#[derive(Deserialize, JsonSchema)]
pub struct PlanNewParams {
    /// Name of the plan (kebab-case, no spaces). Used in paths.
    pub name: String,
    /// Optional one or two sentence description.
    pub description: Option<String>,
    /// Branch to write to (default: "main").
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanAddParams {
    /// Plan name to add the task to.
    pub plan_id: String,
    /// Task title (imperative sentence, one line).
    pub title: String,
    /// Optional longer-form task description. Stored in the title for now —
    /// the substrate's Task shape does not carry a description field, so
    /// if present the description is appended after an em dash.
    pub description: Option<String>,
    /// Priority: low | medium | high | critical. Default: medium.
    pub priority: Option<String>,
    /// Parent task id for a subtask. Substrate enforces exactly two levels.
    pub parent_id: Option<String>,
    /// Agent ID this task is intended for (e.g. "claude-code", "codex",
    /// a user email). Enables state-driven orchestration when paired with
    /// `plan_next(assigned_to=...)`. Free-form — any string works.
    pub assigned_to: Option<String>,
    /// Task ids that must be `done` before this task can be started.
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanStartParams {
    pub plan_id: String,
    pub task_id: String,
    /// Optional reason recorded in blame for the transition.
    pub reason: Option<String>,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanCompleteParams {
    pub plan_id: String,
    pub task_id: String,
    /// Proof object: `{kind, value, note?}`. `kind` is one of
    /// commit|file|test|text. Prefer `commit` when available, then
    /// `file`, then `test`, then `text` as a last resort.
    pub proof: ProofParam,
    pub reason: Option<String>,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ProofParam {
    /// commit | file | test | text
    pub kind: String,
    /// The SHA, path, test name, or free-form note.
    pub value: String,
    /// Optional human-readable note.
    pub note: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanAbandonParams {
    pub plan_id: String,
    pub task_id: String,
    /// Required reason — abandon without a reason is rejected.
    pub reason: String,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanNextParams {
    pub plan_id: String,
    /// Filter to tasks assigned to this agent. Pass "me" to resolve to
    /// the MCP server's own agent id (set via `--agent-id` or the
    /// `X-CTXone-Agent` header).
    pub assigned_to: Option<String>,
    /// When true (default), include unassigned tasks alongside ones
    /// assigned to `assigned_to`.
    #[serde(default = "default_true")]
    pub include_unassigned: bool,
    /// When true, return only tasks explicitly assigned to `assigned_to`.
    #[serde(default)]
    pub assigned_only: bool,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanListParams {
    /// Filter by plan status (active|completed|archived). Omit for all.
    pub status_filter: Option<String>,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanGetParams {
    pub plan_id: String,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanArchiveParams {
    pub plan_id: String,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanTasksParams {
    pub plan_id: String,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Serialize)]
pub struct ToolErr {
    pub error: String,
}

pub fn err_json(e: impl std::fmt::Display) -> String {
    serde_json::to_string(&ToolErr {
        error: e.to_string(),
    })
    .unwrap_or_else(|_| "{\"error\":\"unknown\"}".to_string())
}

// -- Top-level plan operations usable by both MCP and HTTP -----------

/// Create a plan. Writes `/plans/<name>/_meta` via the substrate.
pub fn create_plan(
    store: &TaskStore,
    ref_name: &str,
    name: &str,
    description: Option<String>,
) -> Result<Plan, PlanToolError> {
    let p = store.create_plan(ref_name, name, description)?;
    Ok(p)
}

/// Add a task. Assignment is now carried natively on `Task` — the
/// substrate writes it in the same commit as the task body.
#[allow(clippy::too_many_arguments)]
pub fn add_task(
    store: &TaskStore,
    ref_name: &str,
    plan: &str,
    title: &str,
    description: Option<&str>,
    priority: Option<&str>,
    parent_id: Option<&str>,
    assigned_to: Option<&str>,
    blocked_by: Vec<String>,
) -> Result<Task, PlanToolError> {
    let pri = match priority {
        None => Priority::Medium,
        Some(s) => priority_from_str(s)
            .ok_or_else(|| PlanToolError::InvalidPriority(s.to_string()))?,
    };

    let parent = parent_id.map(|s| TaskId(s.to_string()));
    let blockers: Vec<TaskId> = blocked_by.into_iter().map(TaskId).collect();

    // Append description into title if present — the substrate's Task
    // shape has no description field. Keep it compact.
    let full_title = match description {
        Some(d) if !d.is_empty() => format!("{} — {}", title, d),
        _ => title.to_string(),
    };

    let assigned = assigned_to
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    Ok(store.add_task(ref_name, plan, &full_title, pri, parent, blockers, assigned)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstategraph::Repository;
    use agentstategraph_storage::MemoryStorage;

    fn fresh_repo() -> Arc<Repository> {
        let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
        repo.init().expect("repo init");
        repo
    }

    fn fresh_store() -> (Arc<Repository>, TaskStore) {
        let repo = fresh_repo();
        let store = make_store(repo.clone(), "test-agent");
        (repo, store)
    }

    #[test]
    fn parse_proof_commit() {
        let p = parse_proof("commit", "abc123", None).unwrap();
        assert_eq!(p.kind, ProofKind::Commit);
        assert_eq!(p.value, "abc123");
        assert!(p.note.is_none());
    }

    #[test]
    fn parse_proof_with_note() {
        let p = parse_proof("file", "src/foo.rs", Some("notes".into())).unwrap();
        assert_eq!(p.kind, ProofKind::File);
        assert_eq!(p.note.as_deref(), Some("notes"));
    }

    #[test]
    fn parse_proof_rejects_empty_value() {
        assert!(parse_proof("commit", "", None).is_err());
    }

    #[test]
    fn parse_proof_rejects_unknown_kind() {
        assert!(parse_proof("screenshot", "foo", None).is_err());
    }

    #[test]
    fn priority_from_str_parses_known() {
        assert_eq!(priority_from_str("low"), Some(Priority::Low));
        assert_eq!(priority_from_str("MEDIUM"), Some(Priority::Medium));
        assert_eq!(priority_from_str("High"), Some(Priority::High));
        assert_eq!(priority_from_str("critical"), Some(Priority::Critical));
        assert_eq!(priority_from_str("nope"), None);
    }

    #[test]
    fn create_plan_round_trip() {
        let (_repo, store) = fresh_store();
        let plan = create_plan(&store, "main", "p1", Some("desc".into())).unwrap();
        assert_eq!(plan.name, "p1");
        assert_eq!(plan.status, PlanStatus::Active);
        let fetched = store.get_plan("main", "p1").unwrap();
        assert_eq!(plan, fetched);
    }

    #[test]
    fn add_task_records_assignment_natively() {
        let (_repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        let task = add_task(
            &store,
            "main",
            "p1",
            "Do the thing",
            None,
            Some("high"),
            None,
            Some("codex"),
            vec![],
        )
        .unwrap();
        assert_eq!(task.priority, Priority::High);
        assert_eq!(task.assigned_to.as_deref(), Some("codex"));

        let fetched = store.get_task("main", "p1", &task.id).unwrap();
        assert_eq!(fetched.assigned_to.as_deref(), Some("codex"));
    }

    #[test]
    fn add_task_without_assignment_leaves_task_unassigned() {
        let (_repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        let task = add_task(
            &store, "main", "p1", "Do the thing", None, None, None, None, vec![],
        )
        .unwrap();
        assert!(task.assigned_to.is_none());
    }

    #[test]
    fn empty_assigned_to_string_is_treated_as_none() {
        let (_repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        let task = add_task(
            &store, "main", "p1", "t", None, None, None, Some(""), vec![],
        )
        .unwrap();
        assert!(task.assigned_to.is_none());
    }

    #[test]
    fn store_next_task_for_filters_by_assignee() {
        let (_repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        add_task(
            &store, "main", "p1", "for codex", None, Some("high"), None, Some("codex"), vec![],
        )
        .unwrap();
        add_task(
            &store,
            "main",
            "p1",
            "for claude",
            None,
            Some("medium"),
            None,
            Some("claude-code"),
            vec![],
        )
        .unwrap();
        add_task(
            &store, "main", "p1", "unassigned", None, Some("low"), None, None, vec![],
        )
        .unwrap();

        // Assigned agent sees only its own task (plus unassigned if allowed).
        let next = store
            .next_task_for("main", "p1", Some("codex"), true)
            .unwrap()
            .unwrap();
        assert_eq!(next.title, "for codex");

        let next = store
            .next_task_for("main", "p1", Some("claude-code"), true)
            .unwrap()
            .unwrap();
        assert_eq!(next.title, "for claude");

        // Unknown agent with include_unassigned=true falls back to the
        // unassigned task.
        let next = store
            .next_task_for("main", "p1", Some("other"), true)
            .unwrap()
            .unwrap();
        assert_eq!(next.title, "unassigned");

        // Unknown agent with include_unassigned=false sees nothing.
        let next = store.next_task_for("main", "p1", Some("other"), false).unwrap();
        assert!(next.is_none());
    }

    #[test]
    fn task_to_json_includes_assigned_to_natively() {
        let (_repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        let task = add_task(
            &store,
            "main",
            "p1",
            "t",
            None,
            None,
            None,
            Some("claude-code"),
            vec![],
        )
        .unwrap();
        let value = task_to_json(&task);
        assert_eq!(
            value.get("assigned_to").and_then(|v| v.as_str()),
            Some("claude-code")
        );
        assert_eq!(value.get("status").and_then(|v| v.as_str()), Some("pending"));
    }

    #[test]
    fn store_list_plans_by_status_filters() {
        let (_repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        create_plan(&store, "main", "p2", None).unwrap();
        store.archive_plan("main", "p2").unwrap();

        let active = store
            .list_plans_by_status("main", Some(PlanStatus::Active))
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "p1");

        let archived = store
            .list_plans_by_status("main", Some(PlanStatus::Archived))
            .unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].name, "p2");
    }
}
